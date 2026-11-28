//! Zwiim-owned: the connector platform's `/zwiim/v1` operations (connector
//! design r3 §10). `connector.list`, `connector.create`,
//! `connector.validate`, `connector.set_secret`, `connector.revoke`,
//! `connector.tools`, and the grants that say which sessions may use a
//! connector: `connector.grant.list`, `connector.grant.set`,
//! `connector.grant.remove`, `connector.tool_policy.set` and
//! `connector.grant.allow_all`.
//!
//! A connector is named by `connector_id`. It is answered as a
//! [`ConnectorView`], never its rows: the
//! definition, its credential field names and the credential store's
//! secret names stay in the Host. Secrets go in with
//! `connector.create` or `connector.set_secret` and never come back out.

use std::sync::Arc;

use axum::{extract::Extension, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use crate::app_state::AppState;
use crate::connectors::budget;
use crate::connectors::credentials::CredentialError;
use crate::connectors::definition::{Auth, Runtime, UserAuth, UserConnector};
use crate::connectors::egress::NetworkClass;
use crate::connectors::executions::{self, Execution, ExecutionError, ExecutionState};
use crate::connectors::grants::{self, Fallback, Grant, GrantError, Scope, ToolPolicy};
use crate::connectors::runtime::{
    AnswerError, ApprovalAnswer, CreateError, RevokeError, ValidateError,
};
use crate::connectors::store::{
    self, Connector, CredentialStore, ListedTool, SnapshotChanges, StoreError,
};
use crate::models::AgentType;
use crate::protocol::error::{ZJson, ZwiimError};
use crate::web::auth::Principal;

type ZResult<T> = Result<Json<T>, ZwiimError>;

// ── shapes ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorView {
    pub connector_id: String,
    /// The service's name, from its definition, e.g. `Linear`.
    pub name: String,
    /// The user's name for this connector, e.g. `Work`.
    pub label: String,
    /// The server name agents see, e.g. `linear_work`.
    pub slug: String,
    pub url: String,
    /// `validating`, `active`, `needs_reauth`, `error`, `revoking` or `revoked`.
    pub status: String,
    /// Why the connector is `error` or `needs_reauth` (its service refused
    /// the key), or what its revocation waits for.
    pub error: Option<String>,
    pub last_validated_at: Option<String>,
    pub created_at: String,
    /// Where its key is kept, `keychain` or `file` (unencrypted, on a
    /// headless Host); null when the server takes no key.
    pub credentials: Option<String>,
    /// The origins it may reach and whether its key goes to each.
    pub endpoints: Vec<EndpointView>,
    /// Tools its server listed at the last validation.
    pub tool_count: usize,
    /// The most its tools that are on may take of an agent's context,
    /// about, in token-equivalents (revision 50).
    pub tool_budget: u32,
    /// What its tools that are on take now, about (revision 50).
    pub tools_used: u32,
    /// The user's switch: off, agents are shown none of its tools
    /// (revision 51).
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointView {
    pub origin: String,
    pub credentials: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolView {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
    pub output_schema: Option<serde_json::Value>,
    /// The server's own hints (`readOnlyHint`, …), as it sent them.
    pub annotations: Option<serde_json::Value>,
    /// Agents are shown it (revision 50).
    pub enabled: bool,
    /// About what it takes of an agent's context, in token-equivalents
    /// (revision 50).
    pub estimate: u32,
}

impl From<ListedTool> for ToolView {
    fn from(listed: ListedTool) -> Self {
        let tool = listed.tool;
        Self {
            name: tool.name,
            title: tool.title,
            description: tool.description,
            input_schema: tool.input_schema,
            output_schema: tool.output_schema,
            annotations: tool.annotations,
            enabled: listed.enabled,
            estimate: listed.estimate,
        }
    }
}

/// What a validation changed in the connector's tools, by name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangesView {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

impl From<SnapshotChanges> for ChangesView {
    fn from(changes: SnapshotChanges) -> Self {
        Self {
            added: changes.added,
            changed: changes.changed,
            removed: changes.removed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProblemView {
    pub code: String,
    pub message: String,
}

impl From<ZwiimError> for ProblemView {
    fn from(e: ZwiimError) -> Self {
        Self {
            code: e.code.into(),
            message: e.message,
        }
    }
}

async fn view(state: &AppState, connector: Connector) -> Result<ConnectorView, ZwiimError> {
    let conn = &state.db.conn;
    let definition = store::definition(conn, &connector.definition_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal("a connector's definition is gone"))?;
    let tools = store::listed(conn, &connector.id).await.map_err(internal)?;
    let url = match &definition.runtime {
        Runtime::McpHttp { url, .. } => url.clone(),
        Runtime::McpStdio { .. } => String::new(),
    };
    let credentials = match &definition.auth {
        Auth::Static { .. } => Some(
            match connector.store {
                CredentialStore::Keychain => "keychain",
                CredentialStore::File => "file",
            }
            .to_string(),
        ),
        Auth::None | Auth::Oauth { .. } => None,
    };
    Ok(ConnectorView {
        connector_id: connector.id,
        name: definition.display.name,
        label: connector.label,
        slug: connector.slug,
        url,
        status: connector.status.as_str().into(),
        error: connector.last_error,
        last_validated_at: connector.last_validated_at.map(|t| t.to_rfc3339()),
        created_at: connector.created_at.to_rfc3339(),
        credentials,
        endpoints: definition
            .security
            .endpoints
            .into_iter()
            .map(|e| EndpointView {
                origin: e.origin,
                credentials: e.credentials,
            })
            .collect(),
        tool_count: tools.len(),
        tool_budget: connector.tool_budget,
        tools_used: store::used(&tools),
        enabled: connector.enabled,
    })
}

async fn view_of(state: &AppState, id: &str) -> Result<ConnectorView, ZwiimError> {
    let connector = store::connector(&state.db.conn, id)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    view(state, connector).await
}

// ── errors ────────────────────────────────────────────────────────────────

fn internal(e: impl std::fmt::Display) -> ZwiimError {
    tracing::warn!(error = %e, "[zwiim] connector error translated to `internal`");
    ZwiimError::internal()
}

fn not_found() -> ZwiimError {
    ZwiimError::not_found("No such connector")
}

fn credential_error(e: &CredentialError) -> ZwiimError {
    match e {
        CredentialError::Missing(_) => ZwiimError::invalid_input("Enter the server's key"),
        CredentialError::Unknown(_) => ZwiimError::invalid_input("This server takes no key"),
        CredentialError::Invalid(_) => {
            ZwiimError::invalid_input("The key can't be sent in a header")
        }
        CredentialError::StoreUnavailable(cause) => {
            tracing::warn!("[connectors] credential store unavailable: {cause}");
            ZwiimError::keychain_unavailable()
        }
    }
}

fn validate_error(e: ValidateError) -> ZwiimError {
    match e {
        ValidateError::NotFound(_) => not_found(),
        ValidateError::Unavailable(status) => ZwiimError::new(
            StatusCode::CONFLICT,
            "connector_unavailable",
            format!("The connector is {status}"),
        ),
        ValidateError::DefinitionChanged => ZwiimError::new(
            StatusCode::CONFLICT,
            "definition_changed",
            "The connector's definition changed since it was connected; connect it again",
        ),
        ValidateError::Credential(e) => credential_error(&e),
        // Already redacted, and recorded on the connector.
        ValidateError::Failed(message) => {
            ZwiimError::new(StatusCode::BAD_GATEWAY, "connection_failed", message)
        }
        // Recorded on the connector as `needs_reauth`.
        ValidateError::KeyRefused => ZwiimError::new(
            StatusCode::BAD_GATEWAY,
            "connection_failed",
            "The server refused the key; paste a new one",
        ),
        ValidateError::Store(e) => internal(e),
    }
}

fn create_error(e: CreateError) -> ZwiimError {
    match e {
        CreateError::Definition(e) | CreateError::Store(StoreError::Definition(e)) => {
            ZwiimError::invalid_input(e.to_string())
        }
        CreateError::Store(StoreError::InvalidLabel) => {
            ZwiimError::invalid_input("A label is 1 to 40 characters")
        }
        CreateError::Credential(e) => credential_error(&e),
        CreateError::Store(e) => internal(e),
    }
}

/// A key is entered on the Host's computer, where it is kept.
fn host_only(principal: Option<Extension<Principal>>) -> Result<(), ZwiimError> {
    match principal {
        Some(Extension(Principal::Device(_))) => Err(ZwiimError::permission_denied(
            "Keys for connectors are entered on the Host's computer",
        )),
        _ => Ok(()),
    }
}

// ── connector.list ───────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListParams {}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListResult {
    pub connectors: Vec<ConnectorView>,
}

/// `connector.list {}` → `{connectors}`, oldest first.
pub async fn connector_list(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(_): ZJson<ListParams>,
) -> ZResult<ListResult> {
    let mut connectors = Vec::new();
    for connector in store::connectors(&state.db.conn).await.map_err(internal)? {
        connectors.push(view(&state, connector).await?);
    }
    Ok(Json(ListResult { connectors }))
}

// ── connector.create ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthParams {
    None,
    /// `Authorization: Bearer <secret>`.
    Bearer,
    /// The secret as the whole value of `header`.
    Header {
        header: String,
    },
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkParams {
    #[default]
    Public,
    /// A server on the user's own machine or network.
    Local,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateParams {
    pub name: String,
    pub url: String,
    pub auth: AuthParams,
    #[serde(default)]
    pub network: NetworkParams,
    pub label: String,
    #[serde(default)]
    pub secret: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateResult {
    pub connector: ConnectorView,
    /// Why connecting failed; null when it succeeded. The connector is
    /// kept either way.
    pub validation_error: Option<ProblemView>,
}

/// `connector.create {name, url, auth, network?, label, secret?}` →
/// `{connector, validation_error}`. Only on the Host itself: a key is
/// entered where it is kept.
pub async fn connector_create(
    Extension(state): Extension<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    ZJson(p): ZJson<CreateParams>,
) -> ZResult<CreateResult> {
    host_only(principal)?;
    let server = UserConnector {
        name: p.name,
        url: p.url,
        auth: match p.auth {
            AuthParams::None => UserAuth::None,
            AuthParams::Bearer => UserAuth::Bearer,
            AuthParams::Header { header } => UserAuth::Header { header },
        },
        network: match p.network {
            NetworkParams::Public => NetworkClass::Public,
            NetworkParams::Local => NetworkClass::Local,
        },
    };
    let created = state
        .connectors()
        .await
        .create(server, &p.label, p.secret.as_deref())
        .await
        .map_err(create_error)?;
    let validation_error = created.validation.err().map(|e| validate_error(e).into());
    Ok(Json(CreateResult {
        connector: view_of(&state, &created.id).await?,
        validation_error,
    }))
}

// ── connector.validate ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorParams {
    pub connector_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ValidateResult {
    pub connector: ConnectorView,
    pub changes: ChangesView,
}

/// `connector.validate {connector_id}` → `{connector, changes}`.
/// Connects afresh and lists the server's tools.
pub async fn connector_validate(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<ConnectorParams>,
) -> ZResult<ValidateResult> {
    let changes = state
        .connectors()
        .await
        .validate(&p.connector_id)
        .await
        .map_err(validate_error)?;
    Ok(Json(ValidateResult {
        connector: view_of(&state, &p.connector_id).await?,
        changes: changes.into(),
    }))
}

// ── connector.set_secret ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetSecretParams {
    pub connector_id: String,
    pub secret: String,
}

/// `connector.set_secret {connector_id, secret}` →
/// `{connector, validation_error}`, like `connector.create`. Replaces a
/// wrong, expired or rotated key and connects again; the connector, its
/// label and tools are kept. Only on the Host itself.
pub async fn connector_set_secret(
    Extension(state): Extension<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    ZJson(p): ZJson<SetSecretParams>,
) -> ZResult<CreateResult> {
    host_only(principal)?;
    let connectors = state.connectors().await;
    connectors
        .set_secret(&p.connector_id, &p.secret)
        .await
        .map_err(validate_error)?;
    let validation_error = connectors
        .validate(&p.connector_id)
        .await
        .err()
        .map(|e| validate_error(e).into());
    Ok(Json(CreateResult {
        connector: view_of(&state, &p.connector_id).await?,
        validation_error,
    }))
}

// ── connector.revoke ─────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct ConnectorResult {
    pub connector: ConnectorView,
}

/// `connector.revoke {connector_id}` → `{connector}`. The connector is
/// unusable at once; one still `revoking` waits for the keychain to delete
/// its key, and revoking it again retries.
pub async fn connector_revoke(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<ConnectorParams>,
) -> ZResult<ConnectorResult> {
    match state.connectors().await.revoke(&p.connector_id).await {
        Ok(()) | Err(RevokeError::WaitingForStore(_)) => {}
        Err(RevokeError::NotFound(_)) => return Err(not_found()),
        Err(RevokeError::Store(e)) => return Err(internal(e)),
    }
    Ok(Json(ConnectorResult {
        connector: view_of(&state, &p.connector_id).await?,
    }))
}

// ── connector.tools ──────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct ToolsResult {
    pub tools: Vec<ToolView>,
}

/// `connector.tools {connector_id}` → `{tools}`: what its server listed at
/// the last validation, by name.
pub async fn connector_tools(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<ConnectorParams>,
) -> ZResult<ToolsResult> {
    store::connector(&state.db.conn, &p.connector_id)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    Ok(Json(ToolsResult {
        tools: tools_of(&state, &p.connector_id).await?,
    }))
}

async fn tools_of(state: &AppState, id: &str) -> Result<Vec<ToolView>, ZwiimError> {
    let tools = store::listed(&state.db.conn, id).await.map_err(internal)?;
    Ok(tools.into_iter().map(ToolView::from).collect())
}

// ── tool switches ────────────────────────────────────────────────────────

/// Who may change which tools agents see: only the Host's own computer.
fn switches_host_only(principal: Option<Extension<Principal>>) -> Result<(), ZwiimError> {
    match principal {
        Some(Extension(Principal::Device(_))) => Err(ZwiimError::permission_denied(
            "Connector switches and limits are changed on the Host's computer",
        )),
        _ => Ok(()),
    }
}

fn switch_error(e: StoreError) -> ZwiimError {
    match e {
        StoreError::ConnectorNotFound(_) => not_found(),
        StoreError::ToolNotFound(tool) => {
            ZwiimError::not_found(format!("The connector lists no tool {tool:?}"))
        }
        StoreError::OverBudget {
            needs,
            left,
            budget,
        } => {
            let mut e = ZwiimError::new(
                StatusCode::CONFLICT,
                "over_budget",
                format!(
                    "This tool needs about {needs} of the connector's {budget}; {left} are \
                     left. Turn another tool off or raise the limit"
                ),
            );
            e.details = Some(serde_json::json!({
                "needs": needs, "left": left, "tool_budget": budget,
            }));
            e
        }
        StoreError::BudgetBelowUse { used } => {
            let mut e = ZwiimError::new(
                StatusCode::CONFLICT,
                "over_budget",
                format!("Its tools that are on take about {used}; turn some off first"),
            );
            e.details = Some(serde_json::json!({ "used": used }));
            e
        }
        StoreError::BudgetOutOfRange(_) => ZwiimError::invalid_input(format!(
            "A connector's tool limit is {} to {}",
            budget::MIN,
            budget::MAX
        )),
        e => internal(e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSetEnabledParams {
    pub connector_id: String,
    pub tool: String,
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ToolSwitchResult {
    pub connector: ConnectorView,
    pub tools: Vec<ToolView>,
}

/// `connector.tool.set_enabled {connector_id, tool, enabled}` →
/// `{connector, tools}`. Turning a tool on that doesn't fit in what the
/// connector's limit leaves is refused (`over_budget`).
pub async fn connector_tool_set_enabled(
    Extension(state): Extension<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    ZJson(p): ZJson<ToolSetEnabledParams>,
) -> ZResult<ToolSwitchResult> {
    switches_host_only(principal)?;
    store::set_tool_enabled(&state.db.conn, &p.connector_id, &p.tool, p.enabled)
        .await
        .map_err(switch_error)?;
    state.connectors().await.exposure_changed();
    Ok(Json(ToolSwitchResult {
        connector: view_of(&state, &p.connector_id).await?,
        tools: tools_of(&state, &p.connector_id).await?,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetEnabledParams {
    pub connector_id: String,
    pub enabled: bool,
}

/// `connector.set_enabled {connector_id, enabled}` → `{connector}`. One that
/// is off keeps its key, tools and grants; agents are shown none of its
/// tools and its calls are refused.
pub async fn connector_set_enabled(
    Extension(state): Extension<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    ZJson(p): ZJson<SetEnabledParams>,
) -> ZResult<ConnectorResult> {
    switches_host_only(principal)?;
    let connector = store::set_enabled(&state.db.conn, &p.connector_id, p.enabled)
        .await
        .map_err(switch_error)?;
    state.connectors().await.exposure_changed();
    Ok(Json(ConnectorResult {
        connector: view(&state, connector).await?,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolBudgetSetParams {
    pub connector_id: String,
    pub tool_budget: u32,
}

/// `connector.tool_budget.set {connector_id, tool_budget}` → `{connector}`.
/// Refused outside 1,000 to 10,000 (`invalid_input`), or below what its
/// tools that are on take (`over_budget`).
pub async fn connector_tool_budget_set(
    Extension(state): Extension<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    ZJson(p): ZJson<ToolBudgetSetParams>,
) -> ZResult<ConnectorResult> {
    switches_host_only(principal)?;
    let connector = store::set_tool_budget(&state.db.conn, &p.connector_id, p.tool_budget)
        .await
        .map_err(switch_error)?;
    Ok(Json(ConnectorResult {
        connector: view(&state, connector).await?,
    }))
}

// ── grants ────────────────────────────────────────────────────────────────

/// One grant: which sessions may use the connector, and how its tools are
/// treated there.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantView {
    pub grant_id: i32,
    /// A root workspace; null for every workspace.
    pub workspace_id: Option<String>,
    /// One agent; null for any.
    pub agent: Option<AgentType>,
    /// `ask` or `deny`, for tools with no row.
    pub default_policy: Fallback,
    /// `ask` or `deny`, for tools first seen after the grant was made.
    pub new_tools: Fallback,
    pub created_at: String,
    pub tools: Vec<ToolPolicyView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPolicyView {
    pub name: String,
    pub policy: ToolPolicy,
    /// An `allow` given for an earlier version of the tool, or for a tool
    /// no longer listed: it asks again.
    pub changed: bool,
}

async fn grant_view(state: &AppState, grant: Grant) -> Result<GrantView, ZwiimError> {
    let current: std::collections::HashMap<String, String> =
        store::tools(&state.db.conn, &grant.connector_id)
            .await
            .map_err(internal)?
            .into_iter()
            .map(|tool| {
                let hash = tool.hash();
                (tool.name, hash)
            })
            .collect();
    Ok(GrantView {
        grant_id: grant.id,
        workspace_id: grant.scope.workspace_id,
        agent: grant
            .scope
            .agent
            .map(|agent| serde_json::from_value(serde_json::Value::String(agent)))
            .transpose()
            .map_err(internal)?,
        default_policy: grant.default_policy,
        new_tools: grant.new_tools,
        created_at: grant.created_at.to_rfc3339(),
        tools: grant
            .tools
            .into_iter()
            .map(|row| ToolPolicyView {
                changed: row.policy == ToolPolicy::Allow
                    && current.get(&row.tool) != row.schema_hash.as_ref(),
                name: row.tool,
                policy: row.policy,
            })
            .collect(),
    })
}

fn grant_error(e: GrantError) -> ZwiimError {
    match e {
        GrantError::ConnectorNotFound(_) => not_found(),
        GrantError::GrantNotFound(_) => ZwiimError::not_found("No such grant"),
        GrantError::ToolNotFound(tool) => {
            ZwiimError::not_found(format!("The connector lists no tool {tool:?}"))
        }
        e @ (GrantError::Corrupt(_) | GrantError::Db(_)) => internal(e),
    }
}

/// Who may change what agents can do: only the Host's own computer.
fn grants_host_only(principal: Option<Extension<Principal>>) -> Result<(), ZwiimError> {
    match principal {
        Some(Extension(Principal::Device(_))) => Err(ZwiimError::permission_denied(
            "Grants are changed on the Host's computer",
        )),
        _ => Ok(()),
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GrantsResult {
    pub grants: Vec<GrantView>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GrantResult {
    pub grant: GrantView,
}

/// `connector.grant.list {connector_id}` → `{grants}`.
pub async fn connector_grant_list(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<ConnectorParams>,
) -> ZResult<GrantsResult> {
    let mut views = Vec::new();
    store::connector(&state.db.conn, &p.connector_id)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    for grant in grants::grants(&state.db.conn, &p.connector_id)
        .await
        .map_err(grant_error)?
    {
        views.push(grant_view(&state, grant).await?);
    }
    Ok(Json(GrantsResult { grants: views }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantSetParams {
    pub connector_id: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub agent: Option<AgentType>,
    pub default_policy: Fallback,
    pub new_tools: Fallback,
}

/// `connector.grant.set {connector_id, workspace_id?, agent?,
/// default_policy, new_tools}` → `{grant}`: grants the connector to that
/// scope, or changes the scope's grant, keeping its tool rows. A workspace
/// must be a root workspace; its worktrees go with it.
pub async fn connector_grant_set(
    Extension(state): Extension<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    ZJson(p): ZJson<GrantSetParams>,
) -> ZResult<GrantResult> {
    grants_host_only(principal)?;
    if let Some(workspace_id) = &p.workspace_id {
        crate::workspace::root_path(&state.db.conn, workspace_id).await?;
    }
    let agent = p
        .agent
        .map(|agent| match serde_json::to_value(agent) {
            Ok(serde_json::Value::String(name)) => Ok(name),
            _ => Err(internal("an agent type isn't a name")),
        })
        .transpose()?;
    let scope = Scope {
        workspace_id: p.workspace_id,
        agent,
    };
    let grant = grants::set_grant(
        &state.db.conn,
        &p.connector_id,
        scope,
        p.default_policy,
        p.new_tools,
    )
    .await
    .map_err(grant_error)?;
    state.connectors().await.exposure_changed();
    Ok(Json(GrantResult {
        grant: grant_view(&state, grant).await?,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantParams {
    pub grant_id: i32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Empty {}

/// `connector.grant.remove {grant_id}` → `{}`. Sessions in its scope stop
/// being able to use the connector.
pub async fn connector_grant_remove(
    Extension(state): Extension<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    ZJson(p): ZJson<GrantParams>,
) -> ZResult<Empty> {
    grants_host_only(principal)?;
    grants::remove_grant(&state.db.conn, p.grant_id)
        .await
        .map_err(grant_error)?;
    state.connectors().await.exposure_changed();
    Ok(Json(Empty {}))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPolicySetParams {
    pub grant_id: i32,
    pub tool: String,
    /// `allow`, `ask` or `deny`; null removes the row.
    pub policy: Option<ToolPolicy>,
}

/// `connector.tool_policy.set {grant_id, tool, policy}` → `{grant}`. An
/// `allow` holds for the tool as it is listed now.
pub async fn connector_tool_policy_set(
    Extension(state): Extension<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    ZJson(p): ZJson<ToolPolicySetParams>,
) -> ZResult<GrantResult> {
    grants_host_only(principal)?;
    let grant = grants::set_tool_policy(&state.db.conn, p.grant_id, &p.tool, p.policy)
        .await
        .map_err(grant_error)?;
    state.connectors().await.exposure_changed();
    Ok(Json(GrantResult {
        grant: grant_view(&state, grant).await?,
    }))
}

/// `connector.grant.allow_all {grant_id}` → `{grant}`: allows every tool
/// the connector lists now, each as it is now. Denied tools stay denied.
pub async fn connector_grant_allow_all(
    Extension(state): Extension<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    ZJson(p): ZJson<GrantParams>,
) -> ZResult<GrantResult> {
    grants_host_only(principal)?;
    let grant = grants::allow_all(&state.db.conn, p.grant_id)
        .await
        .map_err(grant_error)?;
    state.connectors().await.exposure_changed();
    Ok(Json(GrantResult {
        grant: grant_view(&state, grant).await?,
    }))
}

// ── executions ────────────────────────────────────────────────────────────

/// One tool call an agent made through the connector. Its arguments are
/// never kept, only their top-level keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionView {
    pub execution_id: String,
    pub tool: String,
    /// `received`, `rejected`, `cancelled_before_dispatch`, `dispatched`,
    /// `succeeded`, `tool_error`, `failed` or `outcome_unknown`, and the
    /// approval states once approvals land.
    pub state: ExecutionState,
    /// Why it was rejected, or why its outcome is unknown.
    pub reason: Option<String>,
    /// What the agent was told, for a call that didn't succeed.
    pub detail: Option<String>,
    pub arg_keys: Vec<String>,
    pub received_at: String,
    /// When it reached the service; null if it never did.
    pub dispatched_at: Option<String>,
    pub finished_at: Option<String>,
    /// The service's round trip.
    pub upstream_ms: Option<i64>,
    /// The rest of the call's time in the Host.
    pub overhead_ms: Option<i64>,
}

impl From<Execution> for ExecutionView {
    fn from(e: Execution) -> Self {
        Self {
            execution_id: e.id,
            tool: e.tool_name,
            state: e.state,
            reason: e.reason,
            detail: e.detail.as_deref().map(super::events::scrub),
            arg_keys: e.arg_keys,
            received_at: e.received_at.to_rfc3339(),
            dispatched_at: e.dispatched_at.map(|t| t.to_rfc3339()),
            finished_at: e.finished_at.map(|t| t.to_rfc3339()),
            upstream_ms: e.upstream_ms,
            overhead_ms: e.overhead_ms,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionsParams {
    pub connector_id: String,
    /// The `next_cursor` of the previous page.
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecutionsResult {
    pub executions: Vec<ExecutionView>,
    pub next_cursor: Option<String>,
}

const DEFAULT_PAGE: u64 = 50;
const MAX_PAGE: u64 = 200;

fn execution_error(e: ExecutionError) -> ZwiimError {
    internal(e)
}

/// `connector.executions {connector_id, cursor?, limit?}` →
/// `{executions, next_cursor}`: its calls, newest first.
pub async fn connector_executions(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<ExecutionsParams>,
) -> ZResult<ExecutionsResult> {
    let conn = &state.db.conn;
    store::connector(conn, &p.connector_id)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    if let Some(cursor) = &p.cursor {
        let known = executions::execution(conn, cursor)
            .await
            .map_err(execution_error)?
            .is_some_and(|e| e.connector_id == p.connector_id);
        if !known {
            return Err(ZwiimError::invalid_input("Unknown cursor"));
        }
    }
    let limit = p.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
    // One extra row tells whether a next page exists.
    let mut page = executions::executions(conn, &p.connector_id, limit + 1, p.cursor.as_deref())
        .await
        .map_err(execution_error)?;
    let next_cursor = if page.len() as u64 > limit {
        page.truncate(limit as usize);
        page.last().map(|e| e.id.clone())
    } else {
        None
    };
    Ok(Json(ExecutionsResult {
        executions: page.into_iter().map(ExecutionView::from).collect(),
        next_cursor,
    }))
}

// ── approvals ─────────────────────────────────────────────────────────────

/// `session.respond {session_id, request_id, kind: "connector_approval",
/// answer: {decision, binding_hash}}`: the user's answer to a connector
/// call waiting in that session. `always` and `always_deny` change a grant,
/// so only the Host itself may give them.
pub async fn connector_respond(
    state: &Arc<AppState>,
    principal: Option<Extension<Principal>>,
    session_id: &str,
    approval_id: &str,
    decision: ApprovalAnswer,
    binding_hash: &str,
) -> Result<(), ZwiimError> {
    let decided_by = match &principal {
        Some(Extension(Principal::Device(device_id))) => format!("device:{device_id}"),
        _ => "host".to_string(),
    };
    if decision.changes_grant() {
        grants_host_only(principal)?;
    }
    let live =
        crate::session::live_connection_id(&state.db.conn, &state.connection_manager, session_id)
            .await?
            .ok_or_else(ZwiimError::session_not_live)?;
    state
        .connectors()
        .await
        .answer_approval(&live, approval_id, binding_hash, decision, &decided_by)
        .await
        .map_err(answer_error)
}

fn answer_error(e: AnswerError) -> ZwiimError {
    match e {
        AnswerError::NotFound => ZwiimError::not_found("No such request"),
        AnswerError::Mismatch => ZwiimError::new(
            StatusCode::CONFLICT,
            "approval_mismatch",
            "This answer is for another call",
        ),
        AnswerError::AlreadyResolved { status, decided_by } => {
            let mut e = ZwiimError::new(
                StatusCode::CONFLICT,
                "already_resolved",
                "This call was already answered",
            );
            // Who answered, without device ids: "Answered on another device".
            let by = decided_by.map(|by| if by == "host" { "host" } else { "device" });
            e.details = Some(serde_json::json!({ "status": status, "decided_by": by }));
            e
        }
        AnswerError::Expired => ZwiimError::new(
            StatusCode::CONFLICT,
            "approval_expired",
            "No one answered in time",
        ),
        AnswerError::Store(message) => internal(message),
    }
}
