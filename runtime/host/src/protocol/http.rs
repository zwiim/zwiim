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

use super::error::{ZJson, ZwiimError};
use crate::acp::types::PromptInputBlock;
use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::folders::{self as folder_commands, FileTreeNode};
use crate::commands::terminal::prepare_credential_env;
use crate::db::entities::conversation::ConversationStatus;
use crate::db::service::folder_service;
use crate::models::agent::AgentType;
use crate::session::{self, RequestAnswer, RuntimeLauncher};
use crate::terminal::error::TerminalError;
use crate::terminal::manager::SpawnOptions;
use crate::workspace::{self, WorkspaceInfo, WorktreeRemoved};
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

pub async fn host_hello(Extension(state): Extension<Arc<AppState>>) -> ZResult<Value> {
    let host_id = crate::identity::host::ensure_host_id(&state.db.conn)
        .await
        .map_err(db_error)?;
    let owner = crate::identity::host::owner(&state.db.conn)
        .await
        .map_err(db_error)?;
    Ok(Json(json!({
        "host_id": host_id,
        "protocol": PROTOCOL_VERSION,
        "version": env!("CARGO_PKG_VERSION"),
        "owner": owner,
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
        return Err(ZwiimError::invalid_input("user_id must be 1-128 characters"));
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

// ─── agent ───────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct AgentSummary {
    pub agent: AgentType,
    pub name: String,
    /// Whether a session can be started with this agent on this host.
    pub available: bool,
    /// Why not: `not_installed`, `unsupported_platform` or `disabled`.
    pub reason: Option<&'static str>,
    /// What a new session can choose, from what the agent last advertised
    /// here; `null` before its first session on this host.
    pub options: Option<crate::session_options::SessionOptions>,
}

/// The host's agents and whether each can be used. Detection only: nothing is
/// installed.
pub async fn agent_list(Extension(state): Extension<Arc<AppState>>) -> ZResult<Vec<AgentSummary>> {
    use crate::acp::error::AcpError;
    let settings = crate::db::service::agent_setting_service::list_map_by_agent_type(
        &state.db.conn,
    )
    .await
    .map_err(db_error)?;
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
        let reason = match check {
            Err(AcpError::PlatformNotSupported(_)) => Some("unsupported_platform"),
            Err(_) => Some("not_installed"),
            Ok(()) if !enabled => Some("disabled"),
            Ok(()) => None,
        };
        let options = match crate::session_options::agent_name(agent) {
            Some(name) => crate::session_options::for_new_session(&state.db.conn, &name).await,
            None => None,
        };
        out.push(AgentSummary {
            agent,
            name: crate::acp::registry::get_agent_meta(agent).name.to_string(),
            available: reason.is_none(),
            reason,
            options,
        });
    }
    Ok(Json(out))
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceAddParams {
    /// Absolute directory on the host, chosen by the user on the host.
    pub path: String,
}

pub async fn workspace_add(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<WorkspaceAddParams>,
) -> ZResult<WorkspaceInfo> {
    Ok(Json(workspace::add(&state.db, &state.emitter, &p.path).await?))
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

pub async fn workspace_create_worktree(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<WorkspaceParams>,
) -> ZResult<WorkspaceInfo> {
    Ok(Json(
        workspace::create_worktree(
            &state.db,
            &state.emitter,
            &state.data_dir.join("worktrees"),
            &p.workspace_id,
        )
        .await?,
    ))
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
    Ok(Json(
        workspace::remove_worktree(
            &state.db,
            &state.emitter,
            &state.connection_manager,
            &state.terminal_manager,
            &p.workspace_id,
            p.force,
        )
        .await?,
    ))
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

// ─── terminal ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalSpawnParams {
    pub workspace_id: String,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub initial_command: Option<String>,
}

pub async fn terminal_spawn(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<TerminalSpawnParams>,
) -> ZResult<Value> {
    // Held until the terminal is registered, so a concurrent worktree removal
    // either sees it or has already removed the workspace.
    let (folder, _launching) =
        workspace::resolve_for_launch(&state.db.conn, &p.workspace_id).await?;
    let root = folder.path;
    let terminal_id = state
        .terminal_manager
        .spawn_with_id(
            SpawnOptions {
                terminal_id: uuid::Uuid::new_v4().to_string(),
                working_dir: root,
                owner_window_label: "zwiim".to_string(),
                shell: p.shell,
                initial_command: p.initial_command,
                extra_env: prepare_credential_env(&state.data_dir),
                temp_files: vec![],
            },
            state.emitter.clone(),
        )
        .map_err(terminal_err)?;
    Ok(Json(json!({ "terminal_id": terminal_id })))
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

/// Prompt content accepted from clients: text and inline images only.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Block {
    Text { text: String },
    Image { data: String, mime_type: String },
}

fn prompt_blocks(blocks: Vec<Block>) -> Vec<PromptInputBlock> {
    blocks
        .into_iter()
        .map(|b| match b {
            Block::Text { text } => PromptInputBlock::Text { text },
            Block::Image { data, mime_type } => PromptInputBlock::Image {
                data,
                mime_type,
                uri: None,
            },
        })
        .collect()
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
    let session_id = session::start(
        &state.db,
        &state.connection_manager,
        &RuntimeLauncher { state: &state },
        &p.workspace_id,
        p.agent,
        prompt_blocks(p.blocks),
        p.client_message_id,
        &p.options,
    )
    .await?;
    Ok(Json(json!({ "session_id": session_id })))
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
    session::send(
        &state.db,
        &state.connection_manager,
        &RuntimeLauncher { state: &state },
        &p.session_id,
        prompt_blocks(p.blocks),
        p.client_message_id,
    )
    .await?;
    Ok(Json(json!({ "session_id": p.session_id })))
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
    Ok(Json(json!({ "session_id": p.session_id, "options": options })))
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
    let options =
        session::set_options(&state.db, &state.connection_manager, &p.session_id, &request)
            .await?;
    Ok(Json(json!({ "session_id": p.session_id, "options": options })))
}

pub async fn session_cancel(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionParams>,
) -> ZResult<Value> {
    session::cancel(&state.db, &state.connection_manager, &p.session_id).await?;
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
    Ok(Json(sessions.into_iter().map(SessionSummary::from).collect()))
}

pub async fn session_history(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SessionParams>,
) -> ZResult<Value> {
    let turns = session::history(&state, &p.session_id).await?;
    Ok(Json(json!({
        "session_id": p.session_id,
        "turns": super::events::project_history(&turns),
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
    session::respond(
        &state.db,
        &state.connection_manager,
        &p.session_id,
        &p.request_id,
        p.answer,
    )
    .await?;
    ok()
}
