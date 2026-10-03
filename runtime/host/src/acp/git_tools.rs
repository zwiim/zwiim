//! zwiim-mcp's Git tools: the ACP-side adapter onto the Git Harness.
//!
//! An agent calls `git_<op>` on its `zwiim-mcp` companion. The companion maps
//! the tool arguments straight into a typed [`GitOp`] ([`op_from_tool_call`])
//! and sends it to the Host over the existing broker socket. The listener hands
//! it to [`GitToolAccess`], which works out who is asking from the calling
//! connection — the workspace its conversation is in, and that session's
//! `session_id` for attribution — applies plan mode, and calls
//! [`git_harness::execute`]. The outcome travels back as a [`reply`] the
//! companion renders ([`render_result`]).
//!
//! Everything agent-specific stays on this side: the connection, the
//! conversation, the token, the session mode. The Harness only ever sees a
//! [`GitCaller`] and a [`GitOp`].
//!
//! Approval is the agent's own: these are ordinary MCP tools on `zwiim-mcp`,
//! so each call goes through whatever approval the agent applies to MCP tools,
//! exactly like the companion's other tools. The one rule added here is plan
//! mode: while the session is planning, an operation that [`GitOp::mutates`]
//! is refused before it reaches the Harness.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::acp::manager::ConnectionManager;
use crate::db::entities::{conversation, folder};
use crate::db::AppDatabase;
use crate::git_harness::{self, GitCaller, GitError, GitErrorCode, GitHost, GitOp, GitOpKind};
use crate::session_options::Mode;

/// Every tool name starts with this; the rest is the operation's name.
const TOOL_PREFIX: &str = "git_";

/// The operations exposed as tools, in the order the schema lists them.
pub const KINDS: [GitOpKind; 15] = [
    GitOpKind::Status,
    GitOpKind::Diff,
    GitOpKind::Log,
    GitOpKind::BranchCreate,
    GitOpKind::Switch,
    GitOpKind::Stage,
    GitOpKind::Commit,
    GitOpKind::Fetch,
    GitOpKind::Pull,
    GitOpKind::Push,
    GitOpKind::PrCreate,
    GitOpKind::PrView,
    GitOpKind::PrChecks,
    GitOpKind::PrComments,
    GitOpKind::PrMerge,
];

/// The refusal code for a mutating operation while the session plans.
pub const PLAN_MODE: &str = "plan_mode";

/// The tool that runs `kind`.
pub fn tool_name(kind: GitOpKind) -> String {
    format!("{TOOL_PREFIX}{}", kind.as_str())
}

/// The operation tool `name` runs, if it is a Git tool.
pub fn kind_of_tool(name: &str) -> Option<GitOpKind> {
    let op = name.strip_prefix(TOOL_PREFIX)?;
    KINDS.into_iter().find(|k| k.as_str() == op)
}

/// Map a `git_<op>` tool call's arguments into its [`GitOp`]. The arguments
/// are the operation's own fields, nothing more: an unknown field, a missing
/// required one, or a wrong type is refused here, before anything runs.
/// Values the Harness validates itself (paths, branch names) are left to it.
pub fn op_from_tool_call(tool: &str, arguments: &Value) -> Result<GitOp, String> {
    let kind = kind_of_tool(tool).ok_or_else(|| format!("unknown tool: {tool}"))?;
    let mut fields = match arguments {
        Value::Null => Map::new(),
        Value::Object(map) => map.clone(),
        _ => return Err(format!("{tool}: arguments must be an object")),
    };
    // The operation comes from the tool name only.
    if fields.contains_key("op") {
        return Err(format!("{tool}: unknown field `op`"));
    }
    // A unit variant of an internally tagged enum ignores extra fields, so
    // `deny_unknown_fields` does not reach it; hold status to the same rule.
    if kind == GitOpKind::Status {
        if let Some(field) = fields.keys().next() {
            return Err(format!(
                "{tool}: unknown field `{field}`; it takes no arguments"
            ));
        }
    }
    fields.insert("op".into(), Value::String(kind.as_str().into()));
    serde_json::from_value(Value::Object(fields)).map_err(|e| format!("{tool}: {e}"))
}

/// A refused or failed Git tool call, as the agent sees it: a Harness error
/// code, or [`PLAN_MODE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitToolError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl From<GitError> for GitToolError {
    fn from(e: GitError) -> Self {
        Self {
            code: e.code.as_str().to_string(),
            message: e.message,
            detail: e.detail,
        }
    }
}

/// The answer for a caller that cannot be placed in any workspace.
pub fn no_workspace() -> GitToolError {
    GitError::new(GitErrorCode::NoWorkspace, "Git operations need a workspace").into()
}

/// The broker reply for a Git tool call: `{ok: true, result}` with the
/// serialized outcome, or `{ok: false, error}`.
pub fn reply(result: Result<Value, GitToolError>) -> Value {
    match result {
        Ok(result) => json!({ "ok": true, "result": result }),
        Err(error) => json!({ "ok": false, "error": error }),
    }
}

/// Render a [`reply`] as the MCP tool result. A failure is `isError: true`
/// with its code first, so the agent can branch on it.
pub fn render_result(outcome: &Value) -> Value {
    if outcome.get("ok").and_then(Value::as_bool) == Some(true) {
        let result = outcome.get("result").cloned().unwrap_or(Value::Null);
        let text = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
        return json!({
            "content": [{ "type": "text", "text": text }],
            "isError": false,
            "structuredContent": result,
        });
    }
    let error = outcome.get("error").cloned().unwrap_or(Value::Null);
    let field = |k: &str| error.get(k).and_then(Value::as_str);
    let mut text = format!(
        "{}: {}",
        field("code").unwrap_or("failed"),
        field("message").unwrap_or("the Git operation failed")
    );
    if let Some(detail) = field("detail") {
        text.push_str("\n\n");
        text.push_str(detail);
    }
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": true,
        "structuredContent": error,
    })
}

/// What the broker listener needs to run a Git tool call. A trait so the
/// listener stays unit-testable without a live `ConnectionManager`.
#[async_trait]
pub trait GitToolAccess: Send + Sync {
    /// Run `op` for the agent on parent connection `parent_connection_id`.
    async fn run(&self, parent_connection_id: &str, op: GitOp) -> Result<Value, GitToolError>;
}

/// The calling connection, as far as a Git tool call cares.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentContext {
    /// The conversation the connection is in, if it has one yet.
    pub conversation_id: Option<i32>,
    /// Whether the session is in Zwiim's `plan` mode.
    pub planning: bool,
}

/// Production [`GitToolAccess`]: the live connection's state for the context,
/// the database for the workspace, the Host data dir for remote credentials.
pub struct HostGitTools {
    pub manager: Arc<ConnectionManager>,
    pub db: Arc<AppDatabase>,
    pub data_dir: PathBuf,
}

#[async_trait]
impl GitToolAccess for HostGitTools {
    async fn run(&self, parent_connection_id: &str, op: GitOp) -> Result<Value, GitToolError> {
        let ctx = match self.manager.get_state(parent_connection_id).await {
            Some(state) => {
                let s = state.read().await;
                AgentContext {
                    conversation_id: s.conversation_id,
                    planning: s.config_options.as_deref().is_some_and(is_planning),
                }
            }
            None => AgentContext::default(),
        };
        run_for(
            &self.db.conn,
            &GitHost::new(&self.db.conn, &self.data_dir),
            ctx,
            op,
        )
        .await
    }
}

fn is_planning(options: &[crate::acp::types::SessionConfigOptionInfo]) -> bool {
    crate::session_options::project(options)
        .mode
        .is_some_and(|m| m.current == Mode::Plan)
}

/// Run `op` for an agent in `ctx`: refuse a mutating operation while it plans,
/// resolve its caller, and hand the rest to the Harness.
pub async fn run_for(
    conn: &DatabaseConnection,
    host: &GitHost<'_>,
    ctx: AgentContext,
    op: GitOp,
) -> Result<Value, GitToolError> {
    if ctx.planning && op.mutates() {
        return Err(GitToolError {
            code: PLAN_MODE.into(),
            message: format!(
                "git {} changes the repository, and this session is in plan mode. \
                 Read-only Git tools still work; leave plan mode to make changes.",
                op.kind().as_str()
            ),
            detail: None,
        });
    }
    let caller = caller_for(conn, ctx.conversation_id).await?;
    let outcome = git_harness::execute(host, &caller, op).await?;
    serde_json::to_value(&outcome).map_err(|e| GitToolError {
        code: GitErrorCode::Failed.as_str().into(),
        message: format!("could not encode the result: {e}"),
        detail: None,
    })
}

/// The Harness caller for conversation `conversation_id`: the workspace of the
/// folder it runs in, attributed to its session. Removed and deleted
/// workspaces are passed through for the Harness to name; a conversation with
/// no workspace is `no_workspace`.
pub async fn caller_for(
    conn: &DatabaseConnection,
    conversation_id: Option<i32>,
) -> Result<GitCaller, GitError> {
    let row = match conversation_id {
        Some(id) => conversation::Entity::find_by_id(id)
            .one(conn)
            .await
            .map_err(|e| {
                GitError::new(GitErrorCode::Failed, "could not read the session")
                    .with_detail(&e.to_string())
            })?,
        None => None,
    };
    let Some(row) = row else {
        return GitCaller::new(None, None);
    };
    let workspace_id = folder::Entity::find_by_id(row.folder_id)
        .one(conn)
        .await
        .map_err(|e| {
            GitError::new(GitErrorCode::Failed, "could not read the workspace")
                .with_detail(&e.to_string())
        })?
        .and_then(|f| f.workspace_id);
    GitCaller::new(workspace_id, row.session_id)
}

#[cfg(test)]
mod tests;
