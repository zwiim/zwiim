//! Zwiim protocol HTTP operations (`POST /zwiim/v1/<op>`).
//!
//! Each handler is a thin translation: resolve `workspace_id` / `session_id`
//! to the internal row, call the existing runtime function, and project the
//! result onto snake_case Zwiim fields. Bodies are `deny_unknown_fields`, so a
//! client cannot smuggle a host path or internal id into an operation.

use std::sync::Arc;

use axum::{extract::Extension, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::compat;
use super::error::{ZJson, ZwiimError};
use crate::acp::types::PromptInputBlock;
use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::folders::{self as folder_commands, FileTreeNode};
use crate::commands::terminal::prepare_credential_env;
use crate::db::entities::conversation::ConversationStatus;
use crate::db::service::folder_service;
use crate::git_harness::{self, GitCaller, GitHost, MergeMethod, PrAutoMerged, PrMerged};
use crate::harness::ResolveStrategy;
use crate::models::agent::AgentType;
use crate::pr_watch::PrEntry;
use crate::session::{self, RequestAnswer, RuntimeLauncher};
use crate::terminal::error::TerminalError;
use crate::terminal::manager::SpawnOptions;
use crate::web::auth::{ConnectionPath, Principal};
use crate::workspace::resolve::{self, ResolveStarted};
use crate::workspace::{self, WorkspaceInfo, WorktreeRemoved};
use crate::workspace_browse::{self, BrowseEntry, BrowseListing};
use crate::workspace_diff::{self, WorkspaceDiff};

type ZResult<T> = Result<Json<T>, ZwiimError>;

/// Zwiim protocol version served under `/zwiim/v1`.
pub const PROTOCOL_VERSION: &str = "zwiim/1";

fn ok() -> ZResult<Value> {
    Ok(Json(json!({})))
}

fn terminal_err(e: TerminalError) -> ZwiimError {
    match e {
        TerminalError::NotFound(_) => ZwiimError::not_found("Terminal not found"),
        other => {
            tracing::warn!(error = %other, "[zwiim] terminal error translated to `internal`");
            ZwiimError::internal()
        }
    }
}

// ─── host ────────────────────────────────────────────────────────────────

fn db_error(e: crate::db::error::DbError) -> ZwiimError {
    ZwiimError::from(AppCommandError::from(e))
}

/// `host.hello`: identity plus the compatibility handshake. The body is
/// optional and read leniently (see `compat`); a client that sends nothing
/// still gets every field.
pub async fn host_hello(
    Extension(state): Extension<Arc<AppState>>,
    body: axum::body::Bytes,
) -> ZResult<Value> {
    let host_id = crate::identity::host::ensure_host_id(&state.db.conn)
        .await
        .map_err(db_error)?;
    let owner = crate::identity::host::owner(&state.db.conn)
        .await
        .map_err(db_error)?;
    let client = compat::parse_hello_client(&body);
    Ok(Json(json!({
        "host_id": host_id,
        "protocol": PROTOCOL_VERSION,
        "version": env!("CARGO_PKG_VERSION"),
        "owner": owner,
        "revision": compat::PROTOCOL_REVISION,
        "min_client_revision": compat::MIN_CLIENT_REVISION,
        "capabilities": compat::CAPABILITIES,
        "compat": compat::client_verdict(client.as_ref()),
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostClaimParams {
    /// The signed-in account's identity-provider user id.
    pub user_id: String,
    #[serde(default)]
    pub email: Option<String>,
}

/// Associate this host with the signed-in account. The first claim wins;
/// another account is refused with `host_owned`, never transferred.
pub async fn host_claim(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<HostClaimParams>,
) -> ZResult<Value> {
    use crate::identity::host::{claim, Claim, HostOwner};
    let user_id = p.user_id.trim();
    if user_id.is_empty() || user_id.len() > 128 {
        return Err(ZwiimError::invalid_input(
            "user_id must be 1-128 characters",
        ));
    }
    let email = p.email.filter(|e| !e.trim().is_empty() && e.len() <= 320);
    let claimant = HostOwner {
        user_id: user_id.to_string(),
        email,
    };
    match claim(&state.db.conn, claimant).await.map_err(db_error)? {
        Claim::Owned(owner) => Ok(Json(json!({ "owner": owner }))),
        Claim::OwnedByOther(owner) => Err(ZwiimError::host_owned(owner.email)),
    }
}

// ─── cloud ───────────────────────────────────────────────────────────────
//
// Enrollment with Zwiim Cloud requires local host authority. Status is
// public facts only: no token or key is ever returned.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloudEnrollParams {
    /// The Cloud's origin (`https://…`; `http` only on loopback).
    pub cloud_url: String,
    /// Single-use token minted by the account in the Cloud.
    pub enrollment_token: String,
    #[serde(default)]
    pub name: Option<String>,
}

pub async fn cloud_enroll(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<CloudEnrollParams>,
) -> ZResult<Value> {
    let name = p
        .name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    if name
        .as_ref()
        .is_some_and(|n| n.len() > 128 || n.chars().any(char::is_control))
    {
        return Err(ZwiimError::invalid_input(
            "name must be at most 128 bytes, without control characters",
        ));
    }
    Ok(Json(
        crate::cloud::enroll(&state, &p.cloud_url, &p.enrollment_token, name).await?,
    ))
}

pub async fn cloud_status(Extension(state): Extension<Arc<AppState>>) -> ZResult<Value> {
    Ok(Json(crate::cloud::status(&state).await?))
}

// ─── lan ─────────────────────────────────────────────────────────────────
//
// Direct LAN access for paired devices: local host authority only, like
// Cloud enrollment. Status is public facts only.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanSetParams {
    pub enabled: bool,
}

pub async fn lan_status(Extension(state): Extension<Arc<AppState>>) -> ZResult<Value> {
    Ok(Json(crate::lan::status(&state).await?))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HostRestartParams {
    #[serde(default)]
    pub mode: RestartMode,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum RestartMode {
    /// Once no turn runs, no request waits on the user and no session is
    /// starting a prompt; open terminals are closed.
    #[default]
    WhenIdle,
    /// At once: running turns are cancelled, waiting requests declined and
    /// terminals closed; sessions and workspaces are kept.
    Now,
}

/// `host.restart`: restart the Host process (local authority only).
pub async fn host_restart(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<HostRestartParams>,
) -> ZResult<Value> {
    use crate::host_lifecycle;
    let (restart, blockers) = match p.mode {
        RestartMode::WhenIdle => host_lifecycle::restart_when_idle(&state).await,
        RestartMode::Now => host_lifecycle::restart_now(&state).await,
    };
    Ok(Json(json!({ "state": restart, "blockers": blockers })))
}

/// `host.update.status`: versions, what is available or staged, how the
/// last update went, whether a restart waits for the Host to be idle, and
/// what keeps it from restarting now.
/// Any paired device may ask.
pub async fn host_update_status(Extension(state): Extension<Arc<AppState>>) -> ZResult<Value> {
    let mut status = crate::update::service::status();
    status["restart_pending"] = json!(state.lifecycle.restart_pending());
    status["blockers"] = json!(crate::host_lifecycle::blockers(&state).await);
    Ok(Json(status))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateChannelParams {
    pub channel: crate::update::manifest::Channel,
}

/// `host.update.set_channel`: which releases this Host follows. Beta takes
/// stable releases too. Local host authority only.
pub async fn host_update_set_channel(ZJson(p): ZJson<UpdateChannelParams>) -> ZResult<Value> {
    let status = crate::update::service::set_channel(p.channel).map_err(|e| {
        tracing::error!("[update] could not save the update channel: {e}");
        ZwiimError::internal()
    })?;
    Ok(Json(status))
}

/// `host.folders`: where this Host keeps its data, the worktrees it makes and
/// the repositories it clones. Fixed for the Host's life, so a client can show
/// them; they are changed by starting the Host with other ones.
pub async fn host_folders(Extension(state): Extension<Arc<AppState>>) -> ZResult<Value> {
    Ok(Json(json!({
        "data": state.data_dir.to_string_lossy(),
        "worktrees": state.worktrees_dir.to_string_lossy(),
        "repositories": state.repos_dir.to_string_lossy(),
    })))
}

/// `host.diagnostics`: what support needs to know about this Host — its
/// versions, protocol, adapters, update and rollback state, and database
/// migrations. Facts only: never credentials, tokens, keys or paths.
pub async fn host_diagnostics(Extension(state): Extension<Arc<AppState>>) -> ZResult<Value> {
    use crate::adapters::ManagedAdapter;
    use crate::db::schema_guard::{self, SchemaState};
    let settings =
        crate::db::service::agent_setting_service::list_map_by_agent_type(&state.db.conn)
            .await
            .map_err(db_error)?;
    let adapters: serde_json::Map<String, Value> = ManagedAdapter::ALL
        .iter()
        .map(|m| {
            let agent = m.agent();
            let env = crate::commands::acp::build_runtime_env_from_setting(
                agent,
                settings.get(&agent),
                None,
            );
            let s = crate::adapters::status(*m, &env);
            (
                m.id().to_string(),
                json!({
                    "version": s.version,
                    "locked_version": s.locked_version,
                    "installed": s.installed,
                    "healthy": s.healthy,
                    "authenticated": s.authenticated,
                    "setup": s.setup,
                    "source": s.source,
                }),
            )
        })
        .collect();
    let schema = match schema_guard::inspect::<crate::db::migration::Migrator>(&state.db.conn)
        .await
        .map_err(db_error)?
    {
        SchemaState::Fresh => json!({ "state": "fresh" }),
        SchemaState::Current => json!({ "state": "current" }),
        SchemaState::Pending(p) => json!({ "state": "pending", "migrations": p }),
        SchemaState::Newer(n) => json!({ "state": "newer", "migrations": n }),
    };
    let startup = crate::db::startup_migration();
    let update = crate::update::service::status();
    let info = crate::update::service::release_info();
    Ok(Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "target": info["target"],
        "channel": update["channel"],
        "protocol": info["protocol"],
        "release_signing": {
            "release_key": info["release_key"],
            "platform_signature": update["platform_signature"],
        },
        "adapters": adapters,
        "update": {
            "supported": update["supported"],
            "unsupported_reason": update["unsupported_reason"],
            "activity": update["activity"],
            "available": update["available"].get("version"),
            "staged": update["staged"],
            "trial": update["trial"],
            "previous": update["previous"],
            "last_result": update["last_result"],
            "last_checked": update["last_checked"],
            "bundled_with_desktop": update["bundled_with_desktop"],
        },
        "database": {
            "schema": schema,
            "migrations_known": <crate::db::migration::Migrator as sea_orm_migration::MigratorTrait>::migrations().len(),
            "applied_at_start": startup.map(|m| m.applied.len()).unwrap_or(0),
            "snapshot_at_start": startup.is_some_and(|m| m.snapshot.is_some()),
            "snapshots": schema_guard::snapshots(&state.data_dir).len(),
        },
    })))
}

/// `host.update.check`: look for a newer release now and stage it.
pub async fn host_update_check() -> ZResult<Value> {
    Ok(Json(crate::update::service::check().await))
}

/// `host.update.apply`: restart into the staged version, through the same
/// drain as `host.restart`. The owner's paired devices may ask: the Host
/// fetched and verified the update itself, so no client handles binaries.
pub async fn host_update_apply(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<HostRestartParams>,
) -> ZResult<Value> {
    use crate::host_lifecycle;
    use crate::update::service::{arm_apply, ApplyError};
    let version = arm_apply().map_err(|e| match e {
        ApplyError::Unsupervised => ZwiimError::update_unavailable(
            "This Host was not started by its supervisor, so it cannot switch versions",
        ),
        ApplyError::NothingStaged => ZwiimError::update_unavailable("No update is ready to apply"),
    })?;
    let (restart, blockers) = match p.mode {
        RestartMode::WhenIdle => host_lifecycle::restart_when_idle(&state).await,
        RestartMode::Now => host_lifecycle::restart_now(&state).await,
    };
    Ok(Json(
        json!({ "version": version, "state": restart, "blockers": blockers }),
    ))
}

/// Any paired device may ask: it is already connected, and learns only the
/// port and addresses it can dial.
pub async fn host_lan_endpoint(Extension(state): Extension<Arc<AppState>>) -> ZResult<Value> {
    Ok(Json(crate::lan::endpoint(&state)))
}

pub async fn lan_set(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<LanSetParams>,
) -> ZResult<Value> {
    Ok(Json(crate::lan::set(&state, p.enabled).await?))
}

// ─── keep awake ──────────────────────────────────────────────────────────
//
// Whether this computer stays awake so the host stays reachable: local host
// authority only, like LAN access.

pub async fn host_keep_awake_status(Extension(state): Extension<Arc<AppState>>) -> ZResult<Value> {
    Ok(Json(json!(crate::power::status(&state).await?)))
}

pub async fn host_keep_awake_set(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<crate::power::KeepAwakePatch>,
) -> ZResult<Value> {
    Ok(Json(json!(crate::power::set(&state, p).await?)))
}

// ─── user instructions ───────────────────────────────────────────────────
//
// The user's own text for their agents, Host-wide and per root workspace.

pub async fn host_instructions(
    Extension(state): Extension<Arc<AppState>>,
) -> ZResult<crate::harness::user_instructions::UserInstructions> {
    Ok(Json(crate::harness::user_instructions::load(
        &state.instructions_path,
    )))
}

pub async fn host_instructions_set(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<crate::protocol::instructions::InstructionsChange>,
) -> ZResult<crate::harness::user_instructions::UserInstructions> {
    Ok(Json(
        crate::protocol::instructions::set(&state.db.conn, &state.instructions_path, p).await?,
    ))
}

// ─── device ──────────────────────────────────────────────────────────────
//
// Pairing, listing and revocation require local host authority (see
// `web::auth::require_local_authority`); `device.pair` is authorized by the
// one-time code it carries.

/// Create a one-time key-pairing invitation: the v2 QR payload.
pub async fn device_pair_invite(
    Extension(state): Extension<Arc<AppState>>,
) -> ZResult<crate::identity::pairing::KeyPairingInvitation> {
    let host_keys = state
        .host_keys
        .get()
        .ok_or_else(ZwiimError::identity_unavailable)?;
    let host_id = crate::identity::host::ensure_host_id(&state.db.conn)
        .await
        .map_err(db_error)?;
    Ok(Json(
        state
            .pairing_invitations
            .create_for_key(host_id, host_keys.static_pubkey()),
    ))
}

pub async fn device_pair(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(claim): ZJson<crate::identity::pairing::KeyPairingClaim>,
) -> ZResult<crate::identity::pairing::KeyPairingGrant> {
    let host_id = crate::identity::host::ensure_host_id(&state.db.conn)
        .await
        .map_err(db_error)?;
    Ok(Json(
        crate::identity::pairing::pair_device(
            &state.db.conn,
            &state.pairing_invitations,
            host_id,
            claim,
        )
        .await?,
    ))
}

pub async fn device_list(Extension(state): Extension<Arc<AppState>>) -> ZResult<Value> {
    let devices: Vec<crate::identity::trusted_devices::DeviceInfo> =
        crate::identity::trusted_devices::list(&state.db.conn)
            .await
            .map_err(db_error)?;
    Ok(Json(json!({ "devices": devices })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceRevokeParams {
    pub device_id: String,
}

/// Revoke a device: it stops authenticating at once, and its open event
/// sockets are closed.
pub async fn device_revoke(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<DeviceRevokeParams>,
) -> ZResult<Value> {
    if !crate::identity::trusted_devices::revoke(&state.db.conn, &p.device_id)
        .await
        .map_err(db_error)?
    {
        return Err(ZwiimError::not_found("Device not found"));
    }
    state.device_revocations.announce(&p.device_id);
    tracing::info!("[identity] device {} revoked", p.device_id);
    ok()
}

// ─── settings ────────────────────────────────────────────────────────────

pub async fn settings_get(Extension(state): Extension<Arc<AppState>>) -> ZResult<Value> {
    let values = crate::settings::get(&state.db.conn).await?;
    Ok(Json(json!({ "values": values })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsUpdateParams {
    pub values: serde_json::Map<String, Value>,
}

pub async fn settings_update(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SettingsUpdateParams>,
) -> ZResult<Value> {
    crate::settings::update(&state.db.conn, &p.values).await?;
    settings_get(Extension(state)).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositorySettingsGetParams {
    pub workspace_id: String,
}

/// The settings of the repository of a workspace: its own for a repository,
/// the one it was made from for a worktree.
pub async fn repository_settings_get(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<RepositorySettingsGetParams>,
) -> ZResult<Value> {
    let values = crate::repository_settings::get(&state.db.conn, &p.workspace_id).await?;
    Ok(Json(json!({ "values": values })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositorySettingsUpdateParams {
    pub workspace_id: String,
    pub values: serde_json::Map<String, Value>,
}

pub async fn repository_settings_update(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<RepositorySettingsUpdateParams>,
) -> ZResult<Value> {
    crate::repository_settings::update(&state.db.conn, &p.workspace_id, &p.values).await?;
    let values = crate::repository_settings::get(&state.db.conn, &p.workspace_id).await?;
    Ok(Json(json!({ "values": values })))
}

/// Fill a repository's unset scripts from its Conductor settings.
pub async fn repository_settings_import(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<RepositorySettingsGetParams>,
) -> ZResult<Value> {
    let imported =
        crate::repository_settings::import_conductor(&state.db.conn, &p.workspace_id).await?;
    let values = crate::repository_settings::get(&state.db.conn, &p.workspace_id).await?;
    Ok(Json(json!({ "values": values, "imported": imported })))
}

// ─── agent ───────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct AgentSummary {
    pub agent: AgentType,
    pub name: String,
    /// Whether a session can be started with this agent on this host.
    pub available: bool,
    /// Why not: `not_installed`, `unsupported_platform` or `disabled`; for a
    /// host-managed adapter also `installing`, `needs_network` or
    /// `setup_failed`.
    pub reason: Option<&'static str>,
    /// `supported` (Claude Code, Codex) or `experimental` (every other agent:
    /// available as-is, without release guarantees).
    pub support: &'static str,
    /// The host-managed adapter's state; `null` for agents the host does not
    /// manage.
    pub adapter: Option<crate::adapters::AdapterStatus>,
    /// What a new session can choose, from what the agent last advertised
    /// here; `null` before its first session on this host.
    pub options: Option<crate::session_options::SessionOptions>,
    /// The effort a new session can choose on each model that has run here,
    /// by model value; `null` for a model that offers none. A model not
    /// listed has not run here yet: `options.effort` is the best guess.
    pub effort_by_model:
        std::collections::BTreeMap<String, Option<crate::session_options::ChoiceOption>>,
    /// The slash commands it offered in its last session here; `null`
    /// before its first.
    pub commands: Option<Vec<crate::session_commands::SessionCommand>>,
    /// Whether it is the agent new sessions start with (`defaults.agent`).
    pub default: bool,
    /// The Host's defaults this agent offers, already current in `options`
    /// and `effort_by_model`; a field is `null` where none applies.
    pub defaults: crate::settings::AgentDefaults,
}

/// The host's agents and whether each can be used. Detection only: nothing is
/// installed.
pub async fn agent_list(Extension(state): Extension<Arc<AppState>>) -> ZResult<Vec<AgentSummary>> {
    use crate::acp::error::AcpError;
    let settings =
        crate::db::service::agent_setting_service::list_map_by_agent_type(&state.db.conn)
            .await
            .map_err(db_error)?;
    let default_agent = crate::settings::default_agent(&state.db.conn).await;
    let agents = crate::acp::registry::builtin_acp_agents();
    let checks = futures::future::join_all(
        agents
            .iter()
            .map(|a| crate::commands::acp::verify_agent_installed(*a)),
    )
    .await;
    let mut out = Vec::with_capacity(agents.len());
    for (agent, check) in agents.into_iter().zip(checks) {
        let enabled = settings.get(&agent).map(|s| s.enabled).unwrap_or(true);
        let managed = crate::adapters::ManagedAdapter::for_agent(agent);
        let adapter = managed.map(|m| {
            let env = crate::commands::acp::build_runtime_env_from_setting(
                agent,
                settings.get(&agent),
                None,
            );
            crate::adapters::status(m, &env)
        });
        let reason = match check {
            Err(AcpError::PlatformNotSupported(_)) => Some("unsupported_platform"),
            Err(_) => match adapter.as_ref().map(|a| &a.setup) {
                Some(crate::adapters::Setup::Installing) => Some("installing"),
                Some(crate::adapters::Setup::NeedsNetwork { .. }) => Some("needs_network"),
                Some(crate::adapters::Setup::Failed { .. }) => Some("setup_failed"),
                _ => Some("not_installed"),
            },
            Ok(()) if !enabled => Some("disabled"),
            Ok(()) => None,
        };
        let (mut options, mut effort_by_model, commands, defaults) =
            match crate::session_options::agent_name(agent) {
                Some(name) => (
                    crate::session_options::for_new_session(&state.db.conn, &name).await,
                    crate::session_options::effort_by_model(&state.db.conn, &name).await,
                    crate::session_commands::for_new_session(&state.db.conn, &name).await,
                    crate::settings::agent_defaults(&state.db.conn, &name).await,
                ),
                None => (None, Default::default(), None, Default::default()),
            };
        let defaults = match options.as_mut() {
            Some(o) => {
                let offered = crate::settings::offered(&defaults, o, &effort_by_model);
                crate::settings::apply(o, &mut effort_by_model, &offered);
                offered
            }
            None => Default::default(),
        };
        // A supported agent that has not run here yet: learn its options so a
        // first session can choose them too.
        if reason.is_none() && managed.is_some() && options.is_none() {
            crate::session_options::learn_in_background(state.clone(), agent);
        }
        out.push(AgentSummary {
            agent,
            name: crate::acp::registry::get_agent_meta(agent).name.to_string(),
            available: reason.is_none(),
            reason,
            support: if managed.is_some() {
                "supported"
            } else {
                "experimental"
            },
            adapter,
            options,
            effort_by_model,
            commands,
            default: default_agent == Some(agent),
            defaults,
        });
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSetupParams {
    pub agent: AgentType,
}

/// `agent.setup`: install (or repair) a host-managed adapter from this
/// build's lock, in the background. Safe to repeat: a setup already running
/// is not started twice. Answers with the adapter's state; `agent.list`
/// follows it (local authority only).
pub async fn agent_setup(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(params): ZJson<AgentSetupParams>,
) -> ZResult<crate::adapters::AdapterStatus> {
    let Some(adapter) = crate::adapters::ManagedAdapter::for_agent(params.agent) else {
        return Err(ZwiimError::invalid_request(
            "only Claude Code and Codex are set up by the host",
        ));
    };
    crate::adapters::setup_in_background(adapter);
    let setting =
        crate::db::service::agent_setting_service::get_by_agent_type(&state.db.conn, params.agent)
            .await
            .ok()
            .flatten();
    let env =
        crate::commands::acp::build_runtime_env_from_setting(params.agent, setting.as_ref(), None);
    Ok(Json(crate::adapters::status(adapter, &env)))
}

// ─── agent sign-in ───────────────────────────────────────────────────────
//
// Status goes to every client; signing in, keys and sign-out are the local
// operator's alone. A key comes in with `agent.save_key` and never goes out:
// no response, event, log line or error carries it.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAuthParams {
    pub agent: AgentType,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSaveKeyParams {
    pub agent: AgentType,
    pub key: String,
}

fn auth_adapter(agent: AgentType) -> Result<crate::adapters::ManagedAdapter, ZwiimError> {
    crate::adapters::ManagedAdapter::for_agent(agent).ok_or_else(|| {
        ZwiimError::invalid_request("only Claude Code and Codex sign in on the host")
    })
}

fn auth_error(e: crate::agent_auth::AuthError) -> ZwiimError {
    use crate::agent_auth::AuthError;
    match e {
        AuthError::InvalidKey(message) => ZwiimError::invalid_input(message),
        AuthError::NotInstalled => ZwiimError::agent_not_installed(),
        AuthError::Store(_) | AuthError::SignOut(_) => {
            tracing::warn!("[agent-auth] {e}");
            ZwiimError::agent_auth_failed()
        }
    }
}

async fn auth_status_of(
    state: &AppState,
    adapter: crate::adapters::ManagedAdapter,
) -> crate::agent_auth::AgentAuthStatus {
    let agent = adapter.agent();
    let setting =
        crate::db::service::agent_setting_service::get_by_agent_type(&state.db.conn, agent)
            .await
            .ok()
            .flatten();
    let env = crate::commands::acp::build_runtime_env_from_setting(agent, setting.as_ref(), None);
    crate::agent_auth::status(agent, &env).expect("a managed agent has a sign-in status")
}

/// `agent.auth_status`: how Claude Code and Codex are signed in on this host.
/// Read fresh on every call.
pub async fn agent_auth_status(
    Extension(state): Extension<Arc<AppState>>,
) -> ZResult<Vec<crate::agent_auth::AgentAuthStatus>> {
    let mut out = Vec::new();
    for adapter in crate::adapters::ManagedAdapter::ALL {
        out.push(auth_status_of(&state, adapter).await);
    }
    Ok(Json(out))
}

/// `agent.sign_in`: open the provider's sign-in page in the browser on the
/// host's Mac; `sign_in` reads `running` until it finishes. Asking again
/// starts over (local authority only).
pub async fn agent_sign_in(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(params): ZJson<AgentAuthParams>,
) -> ZResult<crate::agent_auth::AgentAuthStatus> {
    let adapter = auth_adapter(params.agent)?;
    crate::agent_auth::start_sign_in(adapter).map_err(auth_error)?;
    Ok(Json(auth_status_of(&state, adapter).await))
}

/// `agent.save_key`: keep an API key for the agent in the host's Keychain. It
/// is used while the agent has no account sign-in; running sessions pick it
/// up when restarted (local authority only).
pub async fn agent_save_key(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(params): ZJson<AgentSaveKeyParams>,
) -> ZResult<crate::agent_auth::AgentAuthStatus> {
    let AgentSaveKeyParams { agent, key } = params;
    let key = zeroize::Zeroizing::new(key);
    let adapter = auth_adapter(agent)?;
    crate::agent_auth::save_key(adapter, &key)
        .await
        .map_err(auth_error)?;
    mark_sessions_stale(&state, agent).await;
    Ok(Json(auth_status_of(&state, adapter).await))
}

/// `agent.remove_key`: delete the agent's saved API key (local authority
/// only).
pub async fn agent_remove_key(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(params): ZJson<AgentAuthParams>,
) -> ZResult<crate::agent_auth::AgentAuthStatus> {
    let adapter = auth_adapter(params.agent)?;
    crate::agent_auth::remove_key(adapter)
        .await
        .map_err(auth_error)?;
    mark_sessions_stale(&state, params.agent).await;
    Ok(Json(auth_status_of(&state, adapter).await))
}

/// `agent.sign_out`: delete the saved key, sign the provider account out and
/// end the agent's running sessions, so none keeps using the old sign-in
/// (local authority only).
pub async fn agent_sign_out(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(params): ZJson<AgentAuthParams>,
) -> ZResult<crate::agent_auth::AgentAuthStatus> {
    let adapter = auth_adapter(params.agent)?;
    // Sessions end first, so none can sign the agent back in meanwhile.
    let ended = state
        .connection_manager
        .disconnect_by_agent_type(params.agent)
        .await;
    if ended > 0 {
        tracing::info!(
            "[agent-auth] ended {ended} {} session(s) at sign-out",
            adapter.id()
        );
    }
    crate::agent_auth::sign_out(adapter)
        .await
        .map_err(auth_error)?;
    Ok(Json(auth_status_of(&state, adapter).await))
}

/// Running sessions of `agent` started with the old key: flag them for a
/// restart.
async fn mark_sessions_stale(state: &AppState, agent: AgentType) {
    crate::commands::acp::refresh_config_staleness(
        &state.connection_manager,
        &state.db,
        &state.data_dir,
        &[agent],
        crate::acp::types::ConfigStaleKind::AgentConfig,
    )
    .await;
}

// ─── workspace ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceParams {
    pub workspace_id: String,
}

pub async fn workspace_list(
    Extension(state): Extension<Arc<AppState>>,
) -> ZResult<Vec<WorkspaceInfo>> {
    Ok(Json(workspace::list(&state.db.conn).await?))
}

/// Exactly one of `path`, `home_path` and `workspace_id`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceAddParams {
    /// Absolute directory on the host, chosen by the user on the host. Local
    /// host authority only: paired devices never send host paths.
    #[serde(default)]
    pub path: Option<String>,
    /// Folder names under the host user's home directory (`workspace.browse`).
    #[serde(default)]
    pub home_path: Option<Vec<String>>,
    /// An already-registered root workspace, to re-run `add` (e.g. to
    /// publish it) without the client knowing its host path.
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// Confirmed by the user after a `large_folder` refusal.
    #[serde(default)]
    pub allow_large: bool,
    /// Create private GitHub repository `owner/name` from the folder, add it
    /// as `origin` and push. Absent: a local repository only.
    #[serde(default)]
    pub publish: Option<PublishParams>,
}

/// `workspace.add`'s `publish`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishParams {
    pub owner: String,
    pub name: String,
}

/// The host folder a `workspace.add` / `workspace.inspect` names: `path`
/// (Host principal only), `home_path` (a folder under home, never home), or
/// `workspace_id` (an already-registered root; any principal, since it never
/// carries a host path).
async fn add_path(
    state: &AppState,
    principal: Option<&Principal>,
    path: Option<String>,
    home_path: Option<Vec<String>>,
    workspace_id: Option<String>,
) -> Result<String, ZwiimError> {
    match (path, home_path, workspace_id) {
        (Some(path), None, None) => {
            if !matches!(principal, Some(Principal::Host)) {
                return Err(ZwiimError::permission_denied(
                    "Paired devices add a folder by home_path",
                ));
            }
            Ok(path)
        }
        (None, Some(segments), None) => {
            // An agent over the whole home folder is not a workspace.
            if segments.is_empty() {
                return Err(ZwiimError::invalid_input(
                    "Choose a folder inside the home folder",
                ));
            }
            let home = workspace::home_dir().ok_or_else(ZwiimError::internal)?;
            Ok(workspace_browse::resolve_home_path(home, &segments)?
                .to_string_lossy()
                .into_owned())
        }
        (None, None, Some(workspace_id)) => {
            workspace::root_path(&state.db.conn, &workspace_id).await
        }
        _ => Err(ZwiimError::invalid_input(
            "Give exactly one of path, home_path and workspace_id",
        )),
    }
}

pub async fn workspace_add(
    Extension(state): Extension<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    ZJson(p): ZJson<WorkspaceAddParams>,
) -> ZResult<WorkspaceInfo> {
    let path = add_path(
        &state,
        principal.as_deref(),
        p.path,
        p.home_path,
        p.workspace_id,
    )
    .await?;
    let publish = p.publish.map(|p| workspace::Publish {
        owner: p.owner,
        name: p.name,
        gh: state.gh.clone(),
    });
    Ok(Json(
        workspace::add(&state.db, &state.emitter, &path, p.allow_large, publish).await?,
    ))
}

/// Exactly one of `path`, `home_path` and `workspace_id`, as `workspace.add`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceInspectParams {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub home_path: Option<Vec<String>>,
    #[serde(default)]
    pub workspace_id: Option<String>,
}

pub async fn workspace_inspect(
    Extension(state): Extension<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    ZJson(p): ZJson<WorkspaceInspectParams>,
) -> ZResult<workspace::Inspection> {
    let path = add_path(
        &state,
        principal.as_deref(),
        p.path,
        p.home_path,
        p.workspace_id,
    )
    .await?;
    Ok(Json(
        workspace::inspect(&state.db, &path, state.gh.as_deref()).await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceBrowseParams {
    /// Folder names under the host user's home directory; `[]` is home.
    pub path: Vec<String>,
}

/// The folders inside a folder under the host user's home directory, for
/// choosing one to add. Never lists or names anything outside home.
pub async fn workspace_browse(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<WorkspaceBrowseParams>,
) -> ZResult<BrowseListing> {
    let home = workspace::home_dir().ok_or_else(ZwiimError::internal)?;
    let live: Vec<String> = workspace::list(&state.db.conn)
        .await?
        .into_iter()
        .filter(|w| !w.removed)
        .map(|w| w.path)
        .collect();
    let listing = tokio::task::spawn_blocking(move || {
        let known = live
            .iter()
            .filter_map(|p| std::path::Path::new(p).canonicalize().ok())
            .collect();
        workspace_browse::browse(home, &p.path, &known)
    })
    .await
    .map_err(|_| ZwiimError::internal())??;
    Ok(Json(listing))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCreateFolderParams {
    /// The folder to create it in, as folder names under home; `[]` is home.
    pub path: Vec<String>,
    /// One plain, visible folder name.
    pub name: String,
}

/// Make one new folder inside a folder under the host user's home directory,
/// so it can then be added. Not a filesystem API: nothing else is written.
pub async fn workspace_create_folder(
    ZJson(p): ZJson<WorkspaceCreateFolderParams>,
) -> ZResult<BrowseEntry> {
    let home = workspace::home_dir().ok_or_else(ZwiimError::internal)?;
    let entry = tokio::task::spawn_blocking(move || {
        workspace_browse::create_folder(home, &p.path, &p.name)
    })
    .await
    .map_err(|_| ZwiimError::internal())??;
    Ok(Json(entry))
}

#[derive(Serialize)]
pub struct WorkspaceDetail {
    #[serde(flatten)]
    pub info: WorkspaceInfo,
    pub git_branch: Option<String>,
    pub default_agent: Option<AgentType>,
}

pub async fn workspace_open(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<WorkspaceParams>,
) -> ZResult<WorkspaceDetail> {
    let folder = workspace::resolve(&state.db.conn, &p.workspace_id).await?;
    let detail = folder_service::get_folder_by_id(&state.db.conn, folder.id)
        .await
        .map_err(|e| ZwiimError::from(AppCommandError::from(e)))?
        .ok_or_else(|| ZwiimError::not_found("Workspace not found"))?;
    Ok(Json(WorkspaceDetail {
        info: workspace::info(&state.db.conn, folder).await?,
        git_branch: detail.git_branch,
        default_agent: detail.default_agent_type,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeCreateParams {
    pub workspace_id: String,
    /// A branch from `git.branches` to start from; the root's HEAD when absent.
    #[serde(default)]
    pub base: Option<String>,
}

pub async fn workspace_create_worktree(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<WorktreeCreateParams>,
) -> ZResult<WorkspaceInfo> {
    let prefix =
        crate::settings::branch_prefix(&state.db.conn, state.gh.as_deref(), &gh_cwd()).await;
    let info = workspace::create_worktree(
        &state.db,
        &state.emitter,
        &state.worktrees_dir,
        &p.workspace_id,
        p.base.as_deref(),
        prefix.as_deref(),
    )
    .await?;
    // The repository's setup script starts in its own terminal; neither the
    // reply nor an agent started there waits for it.
    workspace::scripts::setup_new_worktree(
        &state.db.conn,
        &state.terminal_manager,
        &state.emitter,
        &info.workspace_id,
        prepare_credential_env(&state.data_dir),
    )
    .await;
    Ok(Json(info))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunScriptParams {
    pub workspace_id: String,
    pub script: workspace::scripts::Script,
}

pub async fn workspace_run_script(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<RunScriptParams>,
) -> ZResult<Value> {
    let terminal_id = workspace::scripts::run_script(
        &state.db.conn,
        &state.terminal_manager,
        &state.emitter,
        &p.workspace_id,
        p.script,
        prepare_credential_env(&state.data_dir),
    )
    .await?;
    Ok(Json(json!({ "terminal_id": terminal_id })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewsParams {
    pub workspace_id: String,
}

pub async fn workspace_previews(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<PreviewsParams>,
) -> ZResult<Value> {
    let previews =
        workspace::previews::previews(&state.db.conn, &state.terminal_manager, &p.workspace_id).await?;
    Ok(Json(json!({ "previews": previews })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeRemoveParams {
    pub workspace_id: String,
    /// Discard uncommitted changes. Never overrides the running-work guard.
    #[serde(default)]
    pub force: bool,
}

pub async fn workspace_remove_worktree(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<WorktreeRemoveParams>,
) -> ZResult<WorktreeRemoved> {
    let mut host = GitHost::new(&state.db.conn, &state.data_dir);
    host.gh = state.gh.as_deref();
    let delete_branch = crate::settings::delete_branch_on_remove(&state.db.conn)
        .await
        .then_some(&host);
    Ok(Json(
        workspace::remove_worktree(
            &state.db,
            &state.emitter,
            &state.connection_manager,
            &state.terminal_manager,
            &p.workspace_id,
            p.force,
            delete_branch,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceGroupCreateParams {
    pub name: String,
}

pub async fn workspace_group_create(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<WorkspaceGroupCreateParams>,
) -> ZResult<workspace::layout::WorkspaceGroup> {
    Ok(Json(
        workspace::layout::create_group(&state.db, &state.emitter, &p.name).await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceGroupRenameParams {
    pub group_id: String,
    pub name: String,
}

pub async fn workspace_group_rename(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<WorkspaceGroupRenameParams>,
) -> ZResult<workspace::layout::WorkspaceGroup> {
    Ok(Json(
        workspace::layout::rename_group(&state.db, &state.emitter, &p.group_id, &p.name).await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceGroupDeleteParams {
    pub group_id: String,
}

pub async fn workspace_group_delete(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<WorkspaceGroupDeleteParams>,
) -> ZResult<Value> {
    workspace::layout::delete_group(&state.db, &state.emitter, &p.group_id).await?;
    ok()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceLayoutSetParams {
    pub entries: Vec<workspace::layout::LayoutEntryInput>,
}

/// Returns nothing: an HTTP reply can land after a newer feed event and put old
/// order back. The feed is the only source of layout.
pub async fn workspace_layout_set(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<WorkspaceLayoutSetParams>,
) -> ZResult<Value> {
    workspace::layout::set(&state.db, &state.emitter, p.entries).await?;
    ok()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRemoveParams {
    pub workspace_id: String,
    /// Discard the worktrees' uncommitted changes. Never overrides the
    /// running-work guard.
    #[serde(default)]
    pub force: bool,
}

/// Take a repository off the Host, archiving its worktrees. Its own folder
/// is left as it is.
pub async fn workspace_remove(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<WorkspaceRemoveParams>,
) -> ZResult<workspace::WorkspaceRemoved> {
    let mut host = GitHost::new(&state.db.conn, &state.data_dir);
    host.gh = state.gh.as_deref();
    let delete_branch = crate::settings::delete_branch_on_remove(&state.db.conn)
        .await
        .then_some(&host);
    Ok(Json(
        workspace::remove(
            &state.db,
            &state.emitter,
            &state.connection_manager,
            &state.terminal_manager,
            &p.workspace_id,
            p.force,
            delete_branch,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceContinueParams {
    pub workspace_id: String,
    /// The merged pull request the user saw; any other is `pr_changed`.
    pub number: u64,
}

/// Move a worktree whose pull request merged onto a fresh branch from the
/// pull request's base. Same workspace and sessions; the old branch is kept.
pub async fn workspace_continue(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<WorkspaceContinueParams>,
) -> ZResult<WorkspaceInfo> {
    let host = GitHost::new(&state.db.conn, &state.data_dir);
    let prefix =
        crate::settings::branch_prefix(&state.db.conn, state.gh.as_deref(), &gh_cwd()).await;
    let info = workspace::continue_after_merge(
        &state.db,
        &state.emitter,
        &state.connection_manager,
        &host,
        &p.workspace_id,
        p.number,
        prefix.as_deref(),
    )
    .await?;
    state.host_feed.nudge_pr(&info.workspace_id);
    Ok(Json(info))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceResolveConflictsParams {
    pub workspace_id: String,
    /// The session whose agent resolves.
    pub session_id: String,
    /// Defaults to `merge`.
    #[serde(default)]
    pub strategy: Option<ResolveStrategy>,
}

/// How long a resolve waits for GitHub's current pull request state.
const RESOLVE_PR_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Ask a session's agent to resolve a worktree's conflicts with its base.
/// The agent pushes the result while the worktree's pull request is open, and
/// is sent even when the branch merges cleanly here if GitHub reports that
/// pull request conflicting.
pub async fn workspace_resolve_conflicts(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<WorkspaceResolveConflictsParams>,
) -> ZResult<ResolveStarted> {
    // What GitHub says now: a pull request opened, merged or made conflicting
    // since the last poll changes whether and where the agent pushes. A slow
    // answer falls back to the last one known.
    let fresh = tokio::time::timeout(
        RESOLVE_PR_WAIT,
        state.host_feed.pr(&state, &p.workspace_id, true),
    )
    .await;
    let entry = match fresh {
        Ok(Ok(entry)) => Ok(entry),
        _ => state.host_feed.pr(&state, &p.workspace_id, false).await,
    };
    let pr = entry.ok().and_then(|entry| entry.pr);
    let push = resolve::pushes_to(pr.as_ref().map(|pr| pr.state));
    let github_conflicts = resolve::conflicts_on_github(pr.as_ref())
        .then(|| pr.as_ref().map(|pr| pr.base.as_str()))
        .flatten();
    let started = resolve::resolve_conflicts(
        &state.db,
        &state.connection_manager,
        &RuntimeLauncher { state: &state },
        &state.data_dir,
        &state.instructions_path,
        &p.workspace_id,
        &p.session_id,
        p.strategy.unwrap_or_default(),
        push,
        github_conflicts,
    )
    .await?;
    state
        .host_feed
        .resolve_started(&state, &p.workspace_id, &p.session_id)
        .await;
    Ok(Json(started))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeParams {
    pub workspace_id: String,
    /// Relative subdirectory; the workspace root when absent.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub max_depth: Option<usize>,
}

/// Re-root node paths returned for a subdirectory walk onto the workspace.
fn prefix_tree(nodes: &mut [FileTreeNode], prefix: &str) {
    for node in nodes {
        match node {
            FileTreeNode::File { path, .. } => *path = format!("{prefix}/{path}"),
            FileTreeNode::Dir { path, children, .. } => {
                *path = format!("{prefix}/{path}");
                prefix_tree(children, prefix);
            }
        }
    }
}

pub async fn workspace_tree(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<TreeParams>,
) -> ZResult<Vec<FileTreeNode>> {
    let root = workspace::resolve(&state.db.conn, &p.workspace_id)
        .await?
        .path;
    let sub = p
        .path
        .as_deref()
        .map(|s| s.trim_matches('/'))
        .filter(|s| !s.is_empty() && *s != ".");
    let dir = match sub {
        Some(rel) => folder_commands::resolve_tree_path(std::path::Path::new(&root), rel)?,
        None => std::path::PathBuf::from(&root),
    };
    if !dir.is_dir() {
        return Err(ZwiimError::not_found("Directory not found"));
    }
    let mut tree =
        folder_commands::get_file_tree(dir.to_string_lossy().into_owned(), p.max_depth).await?;
    if let Some(rel) = sub {
        prefix_tree(&mut tree, rel);
    }
    Ok(Json(tree))
}

// ─── files ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileReadParams {
    pub workspace_id: String,
    pub path: String,
}

pub async fn files_read(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<FileReadParams>,
) -> ZResult<folder_commands::FileEditContent> {
    let root = workspace::resolve(&state.db.conn, &p.workspace_id)
        .await?
        .path;
    Ok(Json(
        folder_commands::read_file_for_edit(root, p.path).await?,
    ))
}

/// Most bytes one ranged `files.download` returns.
pub const RANGE_MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileDownloadParams {
    pub workspace_id: String,
    pub path: String,
    #[serde(default)]
    pub offset: Option<u64>,
    #[serde(default)]
    pub length: Option<u64>,
    /// Refuse a symlink that resolves to a hidden file `path` doesn't name.
    #[serde(default)]
    pub guard_hidden: bool,
}

/// Whether `target`, what `path` resolved to under `root`, is hidden when
/// `path` doesn't say so. Page code can't see that from the path alone: a repo
/// can commit `notes.txt -> .env`.
pub(super) fn link_reaches_hidden(root: &std::path::Path, target: &std::path::Path, path: &str) -> bool {
    let Ok(resolved) = target.strip_prefix(root) else {
        return true;
    };
    resolved != std::path::Path::new(path)
        && resolved
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
}

/// Any file in the workspace as base64: whole (up to 10 MB), or a range of
/// at most 8 MiB when `offset` or `length` is given. Symlinks are followed
/// only while they stay inside the workspace. Over the Cloud relay no file
/// past 10 MB is read at all.
pub async fn files_download(
    Extension(state): Extension<Arc<AppState>>,
    path: Option<Extension<ConnectionPath>>,
    ZJson(p): ZJson<FileDownloadParams>,
) -> ZResult<Value> {
    use base64::Engine as _;
    use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};
    let root = workspace::resolve(&state.db.conn, &p.workspace_id)
        .await?
        .path;
    let root = std::path::Path::new(&root);
    let joined = folder_commands::resolve_tree_path(root, &p.path)?;
    let not_found = || ZwiimError::not_found("File not found");
    let (Ok(root), Ok(target)) = (root.canonicalize(), joined.canonicalize()) else {
        return Err(not_found());
    };
    if !target.starts_with(&root) || !target.is_file() {
        return Err(not_found());
    }
    if p.guard_hidden && link_reaches_hidden(&root, &target, &p.path) {
        return Err(not_found());
    }
    let size = std::fs::metadata(&target).map_err(|_| not_found())?.len();
    let cap = crate::attachments::ATTACHMENT_MAX_BYTES as u64;
    let via = path.map(|Extension(p)| p).unwrap_or(ConnectionPath::Local);
    if via == ConnectionPath::Relay && size > cap {
        return Err(ZwiimError::file_too_large_for_remote(size));
    }
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mime_type = crate::attachments::mime_for(&name);
    let b64 = base64::engine::general_purpose::STANDARD;

    if p.offset.is_none() && p.length.is_none() {
        if size > cap {
            return Err(ZwiimError::file_too_large());
        }
        let bytes = tokio::fs::read(&target).await.map_err(|_| not_found())?;
        return Ok(Json(json!({
            "mime_type": mime_type,
            "name": name,
            "size": bytes.len(),
            "data": b64.encode(&bytes),
        })));
    }

    let offset = p.offset.unwrap_or(0);
    if offset > size {
        return Err(ZwiimError::invalid_input("offset is past the end of the file"));
    }
    let want = p.length.unwrap_or(RANGE_MAX_BYTES).min(RANGE_MAX_BYTES).min(size - offset);
    let mut file = tokio::fs::File::open(&target).await.map_err(|_| not_found())?;
    file.seek(std::io::SeekFrom::Start(offset)).await.map_err(|_| not_found())?;
    let mut bytes = Vec::with_capacity(want as usize);
    file.take(want).read_to_end(&mut bytes).await.map_err(|_| not_found())?;
    Ok(Json(json!({
        "mime_type": mime_type,
        "name": name,
        "size": size,
        "offset": offset,
        "data": b64.encode(&bytes),
    })))
}

// ─── attachments ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentUploadParams {
    pub workspace_id: String,
    pub name: String,
    /// Standard base64.
    pub data: String,
}

pub async fn attachments_upload(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<AttachmentUploadParams>,
) -> ZResult<Value> {
    use base64::Engine as _;
    let root = workspace::resolve(&state.db.conn, &p.workspace_id)
        .await?
        .path;
    // Refuse before decoding: base64 is 4/3 of the decoded size.
    if p.data.len() / 4 * 3 > crate::attachments::ATTACHMENT_MAX_BYTES + 3 {
        return Err(ZwiimError::attachment_too_large());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(p.data.as_bytes())
        .map_err(|_| ZwiimError::invalid_request("data is not valid base64"))?;
    let stored = crate::attachments::store(std::path::Path::new(&root), &p.name, &bytes).await?;
    Ok(Json(json!({
        "attachment_id": stored.attachment_id,
        "name": stored.name,
        "size": stored.size,
        "mime_type": stored.mime_type,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileWriteParams {
    pub workspace_id: String,
    pub path: String,
    pub content: String,
    /// `etag` from the last read; the write is rejected if the file changed.
    #[serde(default)]
    pub etag: Option<String>,
}

pub async fn files_write(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<FileWriteParams>,
) -> ZResult<folder_commands::FileSaveResult> {
    let root = workspace::resolve(&state.db.conn, &p.workspace_id)
        .await?
        .path;
    Ok(Json(
        folder_commands::save_file_content(root, p.path, p.content, p.etag).await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileCreateParams {
    pub workspace_id: String,
    /// Relative parent directory; `""` for the workspace root.
    #[serde(default)]
    pub path: String,
    pub name: String,
    /// `"file"` or `"dir"`.
    pub kind: String,
}

pub async fn files_create(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<FileCreateParams>,
) -> ZResult<Value> {
    let root = workspace::resolve(&state.db.conn, &p.workspace_id)
        .await?
        .path;
    let path = folder_commands::create_file_tree_entry(root, p.path, p.name, p.kind).await?;
    Ok(Json(json!({ "path": path })))
}

// ─── git ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitStatusParams {
    pub workspace_id: String,
    #[serde(default)]
    pub show_all_untracked: Option<bool>,
}

pub async fn git_status(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<GitStatusParams>,
) -> ZResult<Vec<folder_commands::GitStatusEntry>> {
    let root = workspace::resolve(&state.db.conn, &p.workspace_id)
        .await?
        .path;
    Ok(Json(
        folder_commands::git_status(root, p.show_all_untracked).await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitLogParams {
    pub workspace_id: String,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub skip: Option<u32>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub all_branches: Option<bool>,
    #[serde(default)]
    pub with_files: Option<bool>,
}

pub async fn git_branches(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<WorkspaceParams>,
) -> ZResult<workspace::WorktreeBases> {
    let root = workspace::resolve(&state.db.conn, &p.workspace_id)
        .await?
        .path;
    Ok(Json(workspace::worktree_bases(&root).await?))
}

pub async fn git_log(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<GitLogParams>,
) -> ZResult<folder_commands::GitLogResult> {
    let root = workspace::resolve(&state.db.conn, &p.workspace_id)
        .await?
        .path;
    Ok(Json(
        folder_commands::git_log(
            root,
            p.limit,
            p.branch,
            p.remote,
            p.skip,
            p.author,
            p.all_branches,
            p.with_files,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitCommitParams {
    pub workspace_id: String,
    pub message: String,
    /// Repo-relative paths to stage and commit.
    pub files: Vec<String>,
}

pub async fn git_commit(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<GitCommitParams>,
) -> ZResult<folder_commands::GitCommitResult> {
    let folder = workspace::resolve(&state.db.conn, &p.workspace_id).await?;
    Ok(Json(
        folder_commands::git_commit_core(
            &state.emitter,
            Some(folder.id),
            &state.db.conn,
            &folder.path,
            &p.message,
            &p.files,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitDiffParams {
    pub workspace_id: String,
    /// Workspace-relative path of one changed file: return its patch too.
    #[serde(default)]
    pub file: Option<String>,
}

pub async fn git_diff(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<GitDiffParams>,
) -> ZResult<WorkspaceDiff> {
    let folder = workspace::resolve(&state.db.conn, &p.workspace_id).await?;
    Ok(Json(
        workspace_diff::diff(&folder, p.file.as_deref()).await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitPrParams {
    pub workspace_id: String,
    /// Ask GitHub now instead of answering with what the Host knows.
    #[serde(default)]
    pub refresh: bool,
}

/// The pull request of the workspace's branch, as the Host follows it.
pub async fn git_pr(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<GitPrParams>,
) -> ZResult<PrEntry> {
    workspace::resolve_readable(&state.db.conn, &p.workspace_id).await?;
    Ok(Json(
        state
            .host_feed
            .pr(&state, &p.workspace_id, p.refresh)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitPrMergeParams {
    pub workspace_id: String,
    pub number: u64,
    /// The head commit the user saw; a pull request that moved since is
    /// `pr_changed`.
    pub head_sha: String,
    /// Defaults to the repository's preferred method.
    #[serde(default)]
    pub method: Option<MergeMethod>,
}

/// Merge the workspace's pull request now. Never queues an automatic merge,
/// bypasses branch protection or deletes the branch.
pub async fn git_pr_merge(
    Extension(state): Extension<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    ZJson(p): ZJson<GitPrMergeParams>,
) -> ZResult<PrMerged> {
    workspace::resolve(&state.db.conn, &p.workspace_id).await?;
    let caller = GitCaller::new(Some(p.workspace_id.clone()), None)?;
    let host = GitHost::new(&state.db.conn, &state.data_dir);
    let result = git_harness::merge_pr(&host, &caller, p.number, &p.head_sha, p.method).await;
    let by = match principal.as_deref() {
        Some(Principal::Device(id)) => id.as_str(),
        Some(Principal::Host) => "host",
        None => "unknown",
    };
    tracing::info!(
        workspace_id = %p.workspace_id,
        number = p.number,
        by,
        outcome = result.as_ref().err().map_or("merged", |e| e.code.as_str()),
        "[git] pull request merge"
    );
    // Whatever happened, the pull request may look different now.
    state.host_feed.nudge_pr(&p.workspace_id);
    Ok(Json(result?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitPrAutoMergeParams {
    pub workspace_id: String,
    pub number: u64,
    /// On, so GitHub merges it once its requirements are met; or off.
    pub enabled: bool,
    /// The head commit the user saw; needed to turn auto-merge on.
    #[serde(default)]
    pub head_sha: Option<String>,
    /// Defaults to the repository's preferred method.
    #[serde(default)]
    pub method: Option<MergeMethod>,
}

/// Turn auto-merge for the workspace's pull request on or off. GitHub does
/// the merge, so it happens whether or not the Host is running.
pub async fn git_pr_auto_merge(
    Extension(state): Extension<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    ZJson(p): ZJson<GitPrAutoMergeParams>,
) -> ZResult<PrAutoMerged> {
    workspace::resolve(&state.db.conn, &p.workspace_id).await?;
    let caller = GitCaller::new(Some(p.workspace_id.clone()), None)?;
    let host = GitHost::new(&state.db.conn, &state.data_dir);
    let result = git_harness::auto_merge_pr(
        &host,
        &caller,
        p.number,
        p.enabled,
        p.head_sha.as_deref(),
        p.method,
    )
    .await;
    let by = match principal.as_deref() {
        Some(Principal::Device(id)) => id.as_str(),
        Some(Principal::Host) => "host",
        None => "unknown",
    };
    let outcome = match &result {
        Ok(done) => done.outcome.as_str(),
        Err(e) => e.code.as_str(),
    };
    tracing::info!(
        workspace_id = %p.workspace_id,
        number = p.number,
        enabled = p.enabled,
        by,
        outcome,
        "[git] pull request auto-merge"
    );
    state.host_feed.nudge_pr(&p.workspace_id);
    Ok(Json(result?))
}

// ─── github ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoAvailableParams {
    pub owner: String,
    pub name: String,
}

#[derive(Serialize)]
pub struct RepoAvailable {
    available: bool,
}

/// Whether `owner/name` is free on GitHub for the signed-in account.
pub async fn github_repo_available(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<RepoAvailableParams>,
) -> ZResult<RepoAvailable> {
    workspace::check_target(&p.owner, &p.name)?;
    let exists =
        git_harness::github_repo_exists(state.gh.as_deref(), &gh_cwd(), &p.owner, &p.name).await?;
    Ok(Json(RepoAvailable { available: !exists }))
}

/// `github.auth_status`: how the Host's `gh` is signed in to GitHub, with the
/// code to enter while a sign-in runs.
pub async fn github_auth_status(
    Extension(state): Extension<Arc<AppState>>,
) -> ZResult<git_harness::GithubAuth> {
    Ok(Json(
        state
            .github_auth
            .status(state.gh.as_deref(), &gh_cwd())
            .await,
    ))
}

/// `github.sign_in`: start signing the Host's `gh` in through GitHub's device
/// flow and return the code once gh shows it. Signed in already, or a sign-in
/// running, reads as it is.
pub async fn github_sign_in(
    Extension(state): Extension<Arc<AppState>>,
) -> ZResult<git_harness::GithubAuth> {
    Ok(Json(
        state
            .github_auth
            .sign_in(state.gh.as_deref(), &gh_cwd())
            .await,
    ))
}

/// `github.cancel_sign_in`: end the sign-in in progress, if any.
pub async fn github_cancel_sign_in(
    Extension(state): Extension<Arc<AppState>>,
) -> ZResult<git_harness::GithubAuth> {
    Ok(Json(
        state
            .github_auth
            .cancel(state.gh.as_deref(), &gh_cwd())
            .await,
    ))
}

/// Where the Host runs read-only `gh` calls: the home directory, else temp.
pub(crate) fn gh_cwd() -> String {
    workspace::home_dir()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir)
        .to_string_lossy()
        .into_owned()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubReposParams {
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub refresh: bool,
}

/// One page of the signed-in account's GitHub repositories, each with the
/// root already opened on it.
pub async fn github_repos(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<GithubReposParams>,
) -> ZResult<workspace::clone::GithubRepoPage> {
    let page = p.page.unwrap_or(1);
    if page == 0 {
        return Err(ZwiimError::invalid_input("Pages start at 1"));
    }
    let listed = state
        .clones
        .github_repos(
            &state.db.conn,
            state.gh.as_deref(),
            &gh_cwd(),
            page,
            p.refresh,
        )
        .await?;
    Ok(Json(listed))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloneParams {
    pub owner: String,
    pub name: String,
}

/// Open `owner/name` as a workspace: the root already on it, or a new clone
/// job.
pub async fn workspace_clone(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<CloneParams>,
) -> ZResult<workspace::clone::CloneStart> {
    Ok(Json(
        state
            .clones
            .start(state.clone_env(), &p.owner, &p.name)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloneCancelParams {
    pub clone_id: String,
}

#[derive(Serialize)]
pub struct CloneCancelled {
    cancelled: bool,
}

/// Cancel a clone job still queued or cloning; no effect once it is adding
/// or finished.
pub async fn workspace_clone_cancel(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<CloneCancelParams>,
) -> ZResult<CloneCancelled> {
    Ok(Json(CloneCancelled {
        cancelled: state.clones.cancel(&p.clone_id)?,
    }))
}

// ─── terminal ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalSpawnParams {
    pub workspace_id: String,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub initial_command: Option<String>,
    /// Run this agent's CLI in place of a shell.
    #[serde(default)]
    pub agent: Option<AgentType>,
    /// A slash command for the agent's CLI to run.
    #[serde(default)]
    pub command: Option<String>,
}

/// The longest slash command run in an agent's CLI.
const AGENT_COMMAND_MAX_CHARS: usize = 200;

/// `/name [args]` on one line, with nothing a terminal would act on.
fn valid_agent_command(command: &str) -> bool {
    command.len() > 1
        && command.chars().count() <= AGENT_COMMAND_MAX_CHARS
        && command.starts_with('/')
        && !command.chars().any(char::is_control)
}

pub async fn terminal_spawn(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<TerminalSpawnParams>,
) -> ZResult<Value> {
    if p.agent.is_some() && (p.shell.is_some() || p.initial_command.is_some()) {
        return Err(ZwiimError::invalid_request(
            "agent cannot be combined with shell or initial_command",
        ));
    }
    if let Some(command) = &p.command {
        if p.agent.is_none() {
            return Err(ZwiimError::invalid_request("command needs an agent"));
        }
        if !valid_agent_command(command) {
            return Err(ZwiimError::invalid_input(
                "command must be one line starting with / (at most 200 characters)",
            ));
        }
    }
    let (initial_command, paste) = match p.agent {
        Some(agent) => {
            use crate::adapters::ManagedAdapter;
            let adapter = ManagedAdapter::for_agent(agent).ok_or_else(|| {
                ZwiimError::invalid_request("only Claude Code and Codex run in a terminal")
            })?;
            // Claude Code runs a slash command given as its first prompt;
            // Codex would send it to the model, so it is pasted in instead.
            let (prompt, paste) = match adapter {
                ManagedAdapter::Claude => (p.command, None),
                ManagedAdapter::Codex => (None, p.command),
            };
            let line = crate::agent_auth::vendor_cli_line(adapter, prompt.as_deref())
                .map_err(auth_error)?;
            (Some(line), paste)
        }
        None => (p.initial_command, None),
    };
    // Held until the terminal is registered, so a concurrent worktree removal
    // either sees it or has already removed the workspace.
    let (folder, _launching) =
        workspace::resolve_for_launch(&state.db.conn, &p.workspace_id).await?;
    if folder.parent_id.is_none() {
        return Err(ZwiimError::root_workspace());
    }
    // The worktree's variables, beneath the Host's own (credentials).
    let mut env = workspace::scripts::workspace_env(&state.db.conn, &folder).await;
    env.extend(prepare_credential_env(&state.data_dir).unwrap_or_default());
    let root = folder.path;
    let terminal_id = state
        .terminal_manager
        .spawn_with_id(
            SpawnOptions {
                terminal_id: uuid::Uuid::new_v4().to_string(),
                working_dir: root,
                owner_window_label: "zwiim".to_string(),
                shell: p.shell,
                initial_command,
                extra_env: Some(env),
                temp_files: vec![],
                title: None,
            },
            state.emitter.clone(),
        )
        .map_err(terminal_err)?;
    if let Some(command) = paste {
        run_when_settled(
            state.terminal_manager.clone_ref(),
            terminal_id.clone(),
            command,
        );
    }
    Ok(Json(json!({ "terminal_id": terminal_id })))
}

const SETTLE_POLL: std::time::Duration = std::time::Duration::from_millis(150);
/// Output quiet this long: the CLI has drawn its screen.
const SETTLE_QUIET: std::time::Duration = std::time::Duration::from_millis(600);
/// A CLI still drawing after this long gets nothing pasted.
const SETTLE_LIMIT: std::time::Duration = std::time::Duration::from_secs(20);
/// A CLI that has not shown the pasted command by then gets no Enter.
const ECHO_LIMIT: std::time::Duration = std::time::Duration::from_secs(3);
/// A program turns on bracketed paste once it reads its own input (a CLI's
/// screen is up); until then a paste would land in the shell or the tty.
const BRACKETED_PASTE_ON: &str = "\x1b[?2004h";

/// Run `command` in a terminal's program once it takes pastes and its
/// output has gone quiet. It is pasted (bracketed, so a question on screen
/// takes none of its letters as answers), and Enter follows only once the
/// program shows the command back: one still asking something of its own (a
/// sign-in, trusting the folder) shows nothing, and Enter would answer it for
/// the user.
pub(super) fn run_when_settled(
    terminals: crate::terminal::manager::TerminalManager,
    terminal_id: String,
    command: String,
) {
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + SETTLE_LIMIT;
        let (mut seen, mut quiet, mut takes_paste) = (0, std::time::Duration::ZERO, false);
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(SETTLE_POLL).await;
            let Some(seq) = terminals.output_seq(&terminal_id) else {
                return;
            };
            if seq != seen {
                // From one chunk back, in case the switch spans two.
                takes_paste = takes_paste
                    || terminals
                        .output_since(&terminal_id, seen.saturating_sub(1))
                        .is_some_and(|out| out.contains(BRACKETED_PASTE_ON));
                (seen, quiet) = (seq, std::time::Duration::ZERO);
                continue;
            }
            if takes_paste {
                quiet += SETTLE_POLL;
                if quiet >= SETTLE_QUIET {
                    paste_and_enter(&terminals, &terminal_id, &command, seq).await;
                    return;
                }
            }
        }
        tracing::info!("[zwiim] agent CLI never settled to take a paste; command not pasted");
    });
}

async fn paste_and_enter(
    terminals: &crate::terminal::manager::TerminalManager,
    terminal_id: &str,
    command: &str,
    seq: u64,
) {
    let paste = format!("\x1b[200~{command}\x1b[201~");
    if let Err(e) = terminals.write(terminal_id, paste.as_bytes()) {
        tracing::warn!(error = %e, "[zwiim] agent command not pasted");
        return;
    }
    let deadline = tokio::time::Instant::now() + ECHO_LIMIT;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(SETTLE_POLL).await;
        let Some(output) = terminals.output_since(terminal_id, seq) else {
            return;
        };
        if crate::browser::service_url::strip_ansi(&output).contains(command) {
            if let Err(e) = terminals.write(terminal_id, b"\r") {
                tracing::warn!(error = %e, "[zwiim] agent command not entered");
            }
            return;
        }
    }
    tracing::info!("[zwiim] agent CLI did not show the command; Enter not pressed");
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalParams {
    pub terminal_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalWriteParams {
    pub terminal_id: String,
    pub data: String,
}

pub async fn terminal_write(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<TerminalWriteParams>,
) -> ZResult<Value> {
    state
        .terminal_manager
        .write(&p.terminal_id, p.data.as_bytes())
        .map_err(terminal_err)?;
    ok()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalResizeParams {
    pub terminal_id: String,
    pub cols: u16,
    pub rows: u16,
}

pub async fn terminal_resize(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<TerminalResizeParams>,
) -> ZResult<Value> {
    state
        .terminal_manager
        .resize(&p.terminal_id, p.cols, p.rows)
        .map_err(terminal_err)?;
    ok()
}

pub async fn terminal_kill(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<TerminalParams>,
) -> ZResult<Value> {
    state
        .terminal_manager
        .kill(&p.terminal_id)
        .map_err(terminal_err)?;
    ok()
}

pub async fn terminal_list(Extension(state): Extension<Arc<AppState>>) -> ZResult<Value> {
    let workspaces = workspace::list(&state.db.conn).await?;
    let terminals = state
        .terminal_manager
        .list_with_exit_check(Some(&state.emitter))
        .into_iter()
        .map(|t| {
            json!({
                "terminal_id": t.id,
                "title": t.title,
                "workspace_id": workspace::workspace_for_dir(&workspaces, &t.working_dir),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(Value::Array(terminals)))
}

pub async fn terminal_snapshot(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<TerminalParams>,
) -> ZResult<Value> {
    let snap = state.terminal_manager.snapshot(&p.terminal_id);
    Ok(Json(json!({
        "terminal_id": p.terminal_id,
        "alive": snap.alive,
        "data": snap.data,
        "seq": snap.seq,
    })))
}

// ─── session ─────────────────────────────────────────────────────────────

/// Prompt content accepted from clients: text, inline images (older
/// clients), and attachments uploaded to the session's workspace.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Block {
    Text { text: String },
    Image { data: String, mime_type: String },
    Attachment { attachment_id: String },
}

fn has_attachments(blocks: &[Block]) -> bool {
    blocks.iter().any(|b| matches!(b, Block::Attachment { .. }))
}

/// Client blocks → agent prompt. Attachments, resolved in workspace `root`,
/// become one note appended after the other blocks.
fn prompt_blocks(
    blocks: Vec<Block>,
    root: Option<&std::path::Path>,
) -> Result<Vec<PromptInputBlock>, ZwiimError> {
    let mut out = Vec::with_capacity(blocks.len() + 1);
    let mut attached = Vec::new();
    for b in blocks {
        match b {
            Block::Text { text } => out.push(PromptInputBlock::Text { text }),
            Block::Image { data, mime_type } => out.push(PromptInputBlock::Image {
                data,
                mime_type,
                uri: None,
            }),
            Block::Attachment { attachment_id } => {
                let root = root.ok_or_else(ZwiimError::attachment_not_found)?;
                attached.push(crate::attachments::resolve(root, &attachment_id)?);
            }
        }
    }
    if !attached.is_empty() {
        out.push(PromptInputBlock::Text {
            text: crate::attachments::note(&attached),
        });
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionStartParams {
    pub workspace_id: String,
    pub agent: AgentType,
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub client_message_id: Option<String>,
    /// Initial session options; absent ones keep the agent's defaults.
    #[serde(default)]
    pub options: crate::session_options::OptionsRequest,
}

pub async fn session_start(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionStartParams>,
) -> ZResult<Value> {
    let root = if has_attachments(&p.blocks) {
        Some(workspace::resolve(&state.db.conn, &p.workspace_id).await?.path)
    } else {
        None
    };
    let blocks = prompt_blocks(p.blocks, root.as_deref().map(std::path::Path::new))?;
    let options = with_start_defaults(&state.db.conn, p.agent, p.options).await;
    let session_id = session::start(
        &state.db,
        &state.connection_manager,
        &RuntimeLauncher { state: &state },
        &p.workspace_id,
        p.agent,
        blocks,
        p.client_message_id,
        &options,
    )
    .await?;
    Ok(Json(json!({ "session_id": session_id })))
}

/// `request` with what it leaves out taken from the Host's defaults for
/// `agent`, where the agent offers them. Unchanged before the agent's first
/// session here, when what it offers is not known.
async fn with_start_defaults(
    db: &sea_orm::DatabaseConnection,
    agent: AgentType,
    request: crate::session_options::OptionsRequest,
) -> crate::session_options::OptionsRequest {
    let Some(name) = crate::session_options::agent_name(agent) else {
        return request;
    };
    let Some(options) = crate::session_options::for_new_session(db, &name).await else {
        return request;
    };
    let efforts = crate::session_options::effort_by_model(db, &name).await;
    let chosen = crate::settings::agent_defaults(db, &name).await;
    let offered = crate::settings::offered(&chosen, &options, &efforts);
    crate::settings::fill_start(&request, &offered, &options, &efforts)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSendParams {
    pub session_id: String,
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub client_message_id: Option<String>,
}

pub async fn session_send(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionSendParams>,
) -> ZResult<Value> {
    let root = if has_attachments(&p.blocks) {
        session::resolve(&state.db.conn, &p.session_id).await?;
        session::workspace_dir(&state.db.conn, &p.session_id).await
    } else {
        None
    };
    let blocks = prompt_blocks(p.blocks, root.as_deref().map(std::path::Path::new))?;
    session::send(
        &state.db,
        &state.connection_manager,
        &RuntimeLauncher { state: &state },
        &p.session_id,
        blocks,
        p.client_message_id,
    )
    .await?;
    Ok(Json(json!({ "session_id": p.session_id })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionFollowUpParams {
    pub session_id: String,
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub client_message_id: Option<String>,
    /// Into the running turn, where the agent can take it.
    #[serde(default)]
    pub steer: bool,
}

/// A follow-up: sent now when the agent is idle and nothing is queued,
/// otherwise queued behind what is; with `steer`, put into the running turn
/// where the agent can take it.
pub async fn session_follow_up(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionFollowUpParams>,
) -> ZResult<Value> {
    let root = if has_attachments(&p.blocks) {
        session::resolve(&state.db.conn, &p.session_id).await?;
        session::workspace_dir(&state.db.conn, &p.session_id).await
    } else {
        None
    };
    let attachments = p
        .blocks
        .iter()
        .filter(|b| !matches!(b, Block::Text { .. }))
        .count();
    let mut blocks = prompt_blocks(p.blocks, root.as_deref().map(std::path::Path::new))?;
    // The first block is what the user typed, the part an edit replaces.
    let text = match blocks.first() {
        Some(PromptInputBlock::Text { text }) => {
            let text = text.clone();
            blocks.remove(0);
            text
        }
        _ => String::new(),
    };
    let follow_up = crate::follow_ups::FollowUp::new(text, blocks, attachments);
    let launcher = RuntimeLauncher { state: &state };
    let (db, connections, emitter) = (&state.db, &state.connection_manager, &state.emitter);
    let (session_id, client_message_id) = (&p.session_id, p.client_message_id);
    let outcome = if p.steer {
        crate::follow_ups::steer(
            db,
            connections,
            &launcher,
            emitter,
            session_id,
            follow_up,
            client_message_id,
        )
        .await?
    } else {
        crate::follow_ups::add(
            db,
            connections,
            &launcher,
            emitter,
            session_id,
            follow_up,
            client_message_id,
        )
        .await?
    };
    Ok(Json(json!({ "outcome": outcome })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FollowUpParams {
    pub session_id: String,
    pub follow_up_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FollowUpEditParams {
    pub session_id: String,
    pub follow_up_id: String,
    pub text: String,
}

pub async fn session_follow_up_edit(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<FollowUpEditParams>,
) -> ZResult<Value> {
    crate::follow_ups::edit(&state.db, &state.emitter, &p.session_id, &p.follow_up_id, p.text).await?;
    ok()
}

pub async fn session_follow_up_remove(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<FollowUpParams>,
) -> ZResult<Value> {
    crate::follow_ups::remove(&state.db, &state.emitter, &p.session_id, &p.follow_up_id).await?;
    ok()
}

/// Send one queued follow-up now, or move it first while a turn runs.
pub async fn session_follow_up_send(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<FollowUpParams>,
) -> ZResult<Value> {
    let outcome = crate::follow_ups::send_now(
        &state.db,
        &state.connection_manager,
        &RuntimeLauncher { state: &state },
        &state.emitter,
        &p.session_id,
        &p.follow_up_id,
    )
    .await?;
    Ok(Json(json!({ "outcome": outcome })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionParams {
    pub session_id: String,
}

pub async fn session_options(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionParams>,
) -> ZResult<Value> {
    let options = session::options(&state.db, &state.connection_manager, &p.session_id).await?;
    Ok(Json(
        json!({ "session_id": p.session_id, "options": options }),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSetOptionsParams {
    pub session_id: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub mode: Option<crate::session_options::Mode>,
}

/// Change a session's model / effort / mode between turns.
pub async fn session_set_options(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionSetOptionsParams>,
) -> ZResult<Value> {
    let request = crate::session_options::OptionsRequest {
        model: p.model,
        effort: p.effort,
        mode: p.mode,
    };
    if request.is_empty() {
        return Err(ZwiimError::invalid_input("Nothing to change"));
    }
    let options = session::set_options(
        &state.db,
        &state.connection_manager,
        &p.session_id,
        &request,
    )
    .await?;
    Ok(Json(
        json!({ "session_id": p.session_id, "options": options }),
    ))
}

pub async fn session_cancel(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionParams>,
) -> ZResult<Value> {
    session::cancel(&state.db, &state.connection_manager, &p.session_id).await?;
    ok()
}

pub async fn session_stop(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionParams>,
) -> ZResult<Value> {
    session::stop(&state.db, &state.connection_manager, &p.session_id).await?;
    ok()
}

pub async fn session_delete(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionParams>,
) -> ZResult<Value> {
    session::delete(
        &state.db,
        &state.emitter,
        &state.connection_manager,
        &p.session_id,
    )
    .await?;
    // Its browser tabs and their context go with it.
    match state.browser.get() {
        Some(browser) => browser.close_session(&p.session_id).await,
        None => {
            let removed =
                crate::browser::store::remove_session(&state.db.conn, &p.session_id).await;
            if let Err(e) = removed {
                tracing::warn!(error = %e, "could not remove a deleted session's browser tabs");
            }
        }
    }
    ok()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub workspace_id: Option<String>,
    pub title: Option<String>,
    pub agent: Option<AgentType>,
    pub status: ConversationStatus,
    /// Whether an agent process is currently attached to the session.
    pub live: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Prompts queued behind the running turn; absent when none are.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub follow_ups: Vec<crate::follow_ups::Queued>,
}

impl From<session::SessionInfo> for SessionSummary {
    fn from(s: session::SessionInfo) -> Self {
        Self {
            agent: serde_json::from_value(Value::String(s.agent_type)).ok(),
            session_id: s.session_id,
            workspace_id: s.workspace_id,
            title: s.title,
            status: s.status,
            live: s.attached,
            created_at: s.created_at,
            updated_at: s.updated_at,
            follow_ups: s.follow_ups,
        }
    }
}

pub async fn session_list(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<WorkspaceParams>,
) -> ZResult<Vec<SessionSummary>> {
    let sessions =
        session::list_for_workspace(&state.db.conn, &state.connection_manager, &p.workspace_id)
            .await?;
    Ok(Json(
        sessions.into_iter().map(SessionSummary::from).collect(),
    ))
}

pub async fn session_history(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionParams>,
) -> ZResult<Value> {
    let generation = session::history_generation(&state, &p.session_id).await;
    let turns = session::history(&state, &p.session_id).await?;
    let root = session::workspace_dir(&state.db.conn, &p.session_id).await;
    let revertible = match &root {
        Some(dir) => {
            crate::session_checkpoints::revertible_turns(
                std::path::Path::new(dir),
                &p.session_id,
                &turns,
            )
            .await
        }
        None => Default::default(),
    };
    // The saved can't-reach turn, so a client that opens the session later
    // still shows its error card. Only `{message, code}` leaves the Host, the
    // message scrubbed like the live `agent.error`.
    let row = session::resolve(&state.db.conn, &p.session_id).await?;
    let last_error =
        crate::db::service::conversation_service::unreachable_turn(&state.db.conn, row.id)
            .await
            .map_err(crate::app_error::AppCommandError::from)?
            .map(|turn| {
                json!({
                    "message": super::events::scrub(&turn.message),
                    "code": crate::unreachable::CODE,
                })
            });
    // The newest saved plan, so a client that comes back after a drop or a
    // Host restart still sees how far the agent got, and whether it can be
    // continued (`session.continue_plan`).
    let mut continued_in = None;
    let progress = match &root {
        Some(dir) => {
            let dir = std::path::Path::new(dir);
            match crate::session_plans::saved_progress(&state, &row, dir).await {
                Some(record) => {
                    continued_in = crate::session_plans::continued_in(
                        &state.db.conn,
                        dir,
                        &p.session_id,
                        &record.plan_id,
                    )
                    .await;
                    let resumable = crate::session_plans::is_resumable(
                        &record,
                        &turns,
                        last_error.is_some(),
                        continued_in.is_some(),
                    );
                    let mut value = super::events::saved_plan(&record, root.as_deref());
                    value["resumable"] = json!(resumable);
                    Some(value)
                }
                None => None,
            }
        }
        None => None,
    };
    // The plan this session was started to finish (`session.continue_plan`).
    let continues = match &root {
        Some(dir) => crate::session_plans::link(std::path::Path::new(dir), &p.session_id).await,
        None => None,
    };
    let mut projected = super::events::project_history_with(&turns, root.as_deref(), &revertible);
    let fork_targets = crate::session_fork::fork_targets(&state.db, &row, &turns).await;
    super::events::with_fork_targets(&mut projected, &fork_targets);
    let forked_from = crate::session_fork::forked_from(&state.db.conn, &p.session_id).await;
    let pending_transcript = crate::session_fork::pending_transcript(
        &state.db.conn,
        &state.connection_manager,
        &row,
        root.as_deref(),
    )
    .await;
    Ok(Json(json!({
        "session_id": p.session_id,
        "turns": projected,
        "history_generation": generation,
        "last_error": last_error,
        "progress": progress,
        "continues": continues,
        "continued_in": continued_in.map(|session_id| json!({ "session_id": session_id })),
        "forked_from": forked_from,
        "pending_transcript": pending_transcript,
    })))
}

/// Send the session's saved can't-reach turn again, unchanged.
pub async fn session_retry(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionParams>,
) -> ZResult<Value> {
    crate::session_retry::ensure_retryable(&state.db, &state.connection_manager, &p.session_id)
        .await?;
    let history = session::history(&state, &p.session_id).await?;
    crate::session_retry::retry(
        &state.db,
        &state.connection_manager,
        &RuntimeLauncher { state: &state },
        &p.session_id,
        &history,
    )
    .await?;
    Ok(Json(json!({ "session_id": p.session_id })))
}

/// `session.retry_in_new_chat`: the saved can't-reach turn as the first
/// prompt of a new session, with a summary of this one's chat attached.
pub async fn session_retry_in_new_chat(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionParams>,
) -> ZResult<Value> {
    crate::session_retry::ensure_retryable(&state.db, &state.connection_manager, &p.session_id)
        .await?;
    let history = session::history(&state, &p.session_id).await?;
    let session_id = crate::session_retry::retry_in_new_chat(
        &state.db,
        &state.connection_manager,
        &RuntimeLauncher { state: &state },
        &p.session_id,
        &history,
    )
    .await?;
    // When a plan was handed over, the old session says where it went and
    // the new one has its "(continued)" title; otherwise this only lists the
    // new session afresh.
    announce_sessions_changed(&state, &[p.session_id.as_str(), session_id.as_str()]).await;
    Ok(Json(json!({ "session_id": session_id })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionContinuePlanParams {
    pub session_id: String,
    /// The agent to carry on with; the session's own when absent.
    #[serde(default)]
    pub agent: Option<AgentType>,
}

/// `session.continue_plan`: a new session, with any agent, that finishes
/// this session's newest plan.
pub async fn session_continue_plan(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionContinuePlanParams>,
) -> ZResult<Value> {
    let history = session::history(&state, &p.session_id).await?;
    let continued = crate::session_continue::continue_plan(
        &state.db,
        &state.connection_manager,
        &RuntimeLauncher { state: &state },
        &p.session_id,
        p.agent,
        &history,
    )
    .await?;
    // The old session now says where its plan went; the new one has its
    // "(continued)" title.
    announce_sessions_changed(&state, &[p.session_id.as_str(), continued.session_id.as_str()])
        .await;
    Ok(Json(json!({
        "session_id": continued.session_id,
        "plan_id": continued.plan_id,
    })))
}

/// Tell every device that each of `session_ids` changed.
pub(crate) async fn announce_sessions_changed(state: &AppState, session_ids: &[&str]) {
    for session_id in session_ids {
        if let Ok(row) = session::resolve(&state.db.conn, session_id).await {
            announce_session_changed(state, row.id).await;
        }
    }
}

/// Tell every device that session `conversation_id` changed.
pub(crate) async fn announce_session_changed(state: &AppState, conversation_id: i32) {
    crate::commands::conversations::emit_conversation_upsert(
        &state.emitter,
        &state.db.conn,
        conversation_id,
    )
    .await;
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionForkParams {
    pub session_id: String,
    pub turn_id: String,
    pub target: crate::session_fork::ForkTarget,
}

/// Start a new session from a finished exchange of this one. The original
/// session is left as it was.
pub async fn session_fork(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionForkParams>,
) -> ZResult<Value> {
    let forked = crate::session_fork::fork(state, p.session_id, p.turn_id, p.target).await?;
    Ok(Json(json!({
        "session_id": forked.session_id,
        "workspace_id": forked.workspace_id,
    })))
}

/// Take the waiting transcript out of a forked session, for every device.
pub async fn session_transcript_remove(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionParams>,
) -> ZResult<Value> {
    crate::session_fork::remove_transcript(&state.db, &state.connection_manager, &p.session_id).await?;
    announce_sessions_changed(&state, &[&p.session_id]).await;
    ok()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRevertParams {
    pub session_id: String,
    pub turn_id: String,
    pub files: bool,
    pub chat: bool,
}

pub async fn session_revert(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionRevertParams>,
) -> ZResult<Value> {
    let out = session::revert(
        &state,
        &RuntimeLauncher { state: &state },
        &p.session_id,
        &p.turn_id,
        p.files,
        p.chat,
    )
    .await?;
    Ok(Json(json!({
        "files_restored": out.files_restored,
        "chat_rewound": out.chat_rewound,
        "head_moved": out.head_moved,
    })))
}

#[derive(Deserialize)]
pub struct SessionRespondParams {
    pub session_id: String,
    pub request_id: String,
    #[serde(flatten)]
    pub answer: RequestAnswer,
}

pub async fn session_respond(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionRespondParams>,
) -> ZResult<Value> {
    let outcome = session::respond(
        &state.db,
        &state.connection_manager,
        &RuntimeLauncher { state: &state },
        &p.session_id,
        &p.request_id,
        p.answer,
    )
    .await?;
    match outcome.handed_off_session_id {
        Some(session_id) => Ok(Json(json!({ "session_id": session_id }))),
        None => ok(),
    }
}
