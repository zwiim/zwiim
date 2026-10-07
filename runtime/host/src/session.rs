//! Zwiim-owned session facade over the Conversation engine.
//!
//! A session is a client-facing Conversation row (Regular or Chat) addressed on
//! the client boundary only by its opaque `session_id` (minted in
//! `conversation::ActiveModel::before_save`). Delegate and Loop conversations
//! have no `session_id` and stay internal. `conversation.id`, the live
//! `connection_id`, and ACP identifiers never leave the host.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Serialize};

use crate::acp::error::AcpError;
use crate::acp::manager::ConnectionManager;
use crate::acp::plan_approval::{PlanApprovalAnswer, PlanApprovalDecision};
use crate::acp::question::{QuestionAnswer, QuestionAnswerItem};
use crate::acp::types::PromptInputBlock;
use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::db::entities::conversation::{self, ConversationStatus};
use crate::db::error::DbError;
use crate::db::AppDatabase;
use crate::models::agent::AgentType;
use crate::models::message::MessageTurn;
use crate::protocol::error::ZwiimError;
use crate::session_options::{OptionsRequest, SessionOptions};
use crate::web::event_bridge::EventEmitter;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionInfo {
    pub session_id: String,
    /// `None` for folderless chat sessions (their scratch folder is internal).
    pub workspace_id: Option<String>,
    pub title: Option<String>,
    pub agent_type: String,
    pub status: ConversationStatus,
    /// Whether an agent process is currently attached to this session.
    pub attached: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn db_err(e: sea_orm::DbErr) -> AppCommandError {
    AppCommandError::from(DbError::from(e))
}

/// `session_id → Conversation row`. Only live (not soft-deleted) rows resolve;
/// a restored row resolves again under the same `session_id`.
pub async fn resolve(
    conn: &DatabaseConnection,
    session_id: &str,
) -> Result<conversation::Model, AppCommandError> {
    conversation::Entity::find()
        .filter(conversation::Column::SessionId.eq(session_id))
        .filter(conversation::Column::DeletedAt.is_null())
        .one(conn)
        .await
        .map_err(db_err)?
        .ok_or_else(|| {
            AppCommandError::not_found("Session not found").with_detail(session_id.to_string())
        })
}

/// `session_id → live connection_id`, host-internal only. The connection is
/// found by its bound conversation row, falling back to the agent-native
/// session id for a connection that has not bound its row yet. `None` when no
/// agent process is currently attached.
pub(crate) async fn live_connection_id(
    conn: &DatabaseConnection,
    connections: &ConnectionManager,
    session_id: &str,
) -> Result<Option<String>, AppCommandError> {
    let row = resolve(conn, session_id).await?;
    Ok(connection_for(connections, &row).await)
}

/// The directory session `session_id` runs in. Host-internal: used to make
/// paths the agent reports relative to the workspace.
pub(crate) async fn workspace_dir(conn: &DatabaseConnection, session_id: &str) -> Option<String> {
    let row = resolve(conn, session_id).await.ok()?;
    crate::db::entities::folder::Entity::find_by_id(row.folder_id)
        .one(conn)
        .await
        .ok()?
        .map(|f| f.path)
}

pub(crate) async fn connection_for(
    connections: &ConnectionManager,
    row: &conversation::Model,
) -> Option<String> {
    if let Some(id) = connections.find_connection_by_conversation_id(row.id).await {
        return Some(id);
    }
    let agent_type: Option<AgentType> =
        serde_json::from_value(serde_json::Value::String(row.agent_type.clone())).ok();
    match (row.external_id.as_deref(), agent_type) {
        (Some(external_id), Some(agent_type)) => {
            connections
                .find_connection_by_external_id(external_id, agent_type)
                .await
        }
        _ => None,
    }
}

fn to_info(
    row: conversation::Model,
    workspace_id: Option<String>,
    attached: bool,
) -> Option<SessionInfo> {
    Some(SessionInfo {
        session_id: row.session_id?,
        workspace_id,
        title: row.title,
        agent_type: row.agent_type,
        status: row.status,
        attached,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

/// Live sessions in workspace `workspace_id`, newest first. A removed
/// worktree still lists the sessions it kept.
pub async fn list_for_workspace(
    conn: &DatabaseConnection,
    connections: &ConnectionManager,
    workspace_id: &str,
) -> Result<Vec<SessionInfo>, AppCommandError> {
    let folder = crate::workspace::resolve_readable(conn, workspace_id).await?;
    let rows = conversation::Entity::find()
        .filter(conversation::Column::FolderId.eq(folder.id))
        .filter(conversation::Column::SessionId.is_not_null())
        .filter(conversation::Column::DeletedAt.is_null())
        .order_by_desc(conversation::Column::CreatedAt)
        .all(conn)
        .await
        .map_err(db_err)?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let attached = connection_for(connections, &row).await.is_some();
        out.extend(to_info(row, Some(workspace_id.to_string()), attached));
    }
    Ok(out)
}

/// Completed turns of session `session_id`, read from the agent-native
/// transcript through the existing conversation-detail path, so it works with
/// no live agent and after a host restart. A turn still running is cut off at
/// its user prompt: clients take it from the live snapshot, so history and
/// snapshot never render the same prompt twice.
pub async fn history(
    state: &AppState,
    session_id: &str,
) -> Result<Vec<MessageTurn>, AppCommandError> {
    let row = resolve(&state.db.conn, session_id).await?;
    let mut detail = crate::commands::conversations::get_folder_conversation_with_live_core(
        &state.db.conn,
        &state.connection_manager,
        &state.emitter,
        row.id,
        None,
    )
    .await?;
    if let Some(in_flight) = detail.in_flight_user_turn_id.as_deref() {
        if let Some(i) = detail.turns.iter().position(|t| t.id == in_flight) {
            detail.turns.truncate(i);
        }
    }
    Ok(detail.turns)
}

// ─── Session orchestration (Zwiim protocol `session.*`) ───────────────────

/// Starts the agent process behind a session and returns its (host-internal)
/// connection id. [`RuntimeLauncher`] is the production implementation; tests
/// substitute one that registers a synthetic connection.
pub trait AgentLauncher: Send + Sync {
    /// `resume_external_id` is the agent-native session to load, `None` for a
    /// brand-new agent session. `preferred` are native option values the agent
    /// is set to before it is first used; values it no longer offers are
    /// skipped.
    fn launch(
        &self,
        agent_type: AgentType,
        working_dir: String,
        resume_external_id: Option<String>,
        preferred: BTreeMap<String, String>,
    ) -> impl Future<Output = Result<String, ZwiimError>> + Send;
}

/// Launches through the existing ACP runtime: agent runtime env, installed
/// check, `ConnectionManager::spawn_agent`.
pub struct RuntimeLauncher<'a> {
    pub state: &'a AppState,
}

impl AgentLauncher for RuntimeLauncher<'_> {
    async fn launch(
        &self,
        agent_type: AgentType,
        working_dir: String,
        resume_external_id: Option<String>,
        preferred: BTreeMap<String, String>,
    ) -> Result<String, ZwiimError> {
        let runtime_env = crate::commands::acp::build_session_runtime_env(
            &self.state.db,
            agent_type,
            resume_external_id.as_deref(),
            &self.state.data_dir,
        )
        .await?;
        crate::commands::acp::verify_agent_installed(agent_type).await?;
        Ok(self
            .state
            .connection_manager
            .spawn_agent(
                agent_type,
                Some(working_dir),
                resume_external_id,
                runtime_env,
                "zwiim".to_string(),
                self.state.emitter.clone(),
                None,
                preferred,
            )
            .await?)
    }
}

/// Per-session serialization of launch + prompt, so concurrent sends to a
/// session with no live agent spawn exactly one.
static SESSION_LOCKS: LazyLock<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Default::default);

fn session_lock(session_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = SESSION_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    // Drop locks nobody holds or waits on.
    locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    locks.entry(session_id.to_string()).or_default().clone()
}

fn empty_prompt() -> ZwiimError {
    ZwiimError::invalid_input("Prompt must contain at least one block")
}

async fn session_id_of(db: &AppDatabase, conversation_id: i32) -> Result<String, ZwiimError> {
    conversation::Entity::find_by_id(conversation_id)
        .one(&db.conn)
        .await
        .map_err(db_err)?
        .and_then(|row| row.session_id)
        .ok_or_else(ZwiimError::internal)
}

/// Create a new session in `workspace_id`: spawn `agent_type` there, set its
/// initial `options`, and send the first prompt, which creates the
/// Conversation row (and its `session_id`). An option the agent does not offer
/// or a failed first prompt disconnects the new agent.
pub async fn start<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    workspace_id: &str,
    agent_type: AgentType,
    blocks: Vec<PromptInputBlock>,
    client_message_id: Option<String>,
    options: &OptionsRequest,
) -> Result<String, ZwiimError> {
    if blocks.is_empty() {
        return Err(empty_prompt());
    }
    let preferred =
        crate::session_options::preferred_for_start(&db.conn, agent_type, options).await?;
    let (folder, launching) = crate::workspace::resolve_for_launch(&db.conn, workspace_id).await?;
    if folder.parent_id.is_none() {
        return Err(ZwiimError::root_workspace());
    }
    // `launching` is held through the first prompt, so the new agent's turn
    // is under way before anything can change the workspace's branch.
    let connection_id = launcher
        .launch(agent_type, folder.path, None, preferred.clone().unwrap_or_default())
        .await?;
    if preferred.is_none() {
        if let Err(e) =
            crate::session_options::apply_on_start(connections, &connection_id, options).await
        {
            let _ = connections.disconnect(&connection_id).await;
            return Err(e);
        }
    }
    let sent = connections
        .send_prompt_linked_with_message_id(
            db,
            &connection_id,
            blocks,
            Some(folder.id),
            None,
            None,
            client_message_id,
        )
        .await;
    drop(launching);
    match sent {
        Ok(Some(conversation_id)) => {
            let session_id = session_id_of(db, conversation_id).await?;
            if let Some(values) = preferred.filter(|v| !v.is_empty()) {
                crate::session_options::remember_start(&db.conn, &session_id, values).await;
            }
            Ok(session_id)
        }
        Ok(None) => {
            let _ = connections.disconnect(&connection_id).await;
            Err(ZwiimError::internal())
        }
        Err(e) => {
            let _ = connections.disconnect(&connection_id).await;
            Err(e.into())
        }
    }
}

/// The live connection of session `row`, resuming its agent session through
/// the launcher in `folder` when none is attached (idle sweep, crash, host
/// restart). Returns whether this call spawned it. Callers hold the session
/// lock and have entered `folder` (`workspace::begin_launch`).
async fn ensure_live<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    row: &conversation::Model,
    folder: &crate::db::entities::folder::Model,
) -> Result<(String, bool), ZwiimError> {
    if let Some(connection_id) = connection_for(connections, row).await {
        return Ok((connection_id, false));
    }
    let agent_type: AgentType =
        serde_json::from_value(serde_json::Value::String(row.agent_type.clone()))
            .map_err(|_| ZwiimError::internal())?;
    let preferred = match row.session_id.as_deref() {
        Some(session_id) => crate::session_options::preferred_values(&db.conn, session_id).await,
        None => BTreeMap::new(),
    };
    let connection_id = launcher
        .launch(agent_type, folder.path.clone(), row.external_id.clone(), preferred)
        .await?;
    Ok((connection_id, true))
}

/// Send a prompt to session `session_id`, resuming its agent first if it is
/// not live. The session keeps its `session_id` across the resume.
pub async fn send<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    session_id: &str,
    blocks: Vec<PromptInputBlock>,
    client_message_id: Option<String>,
) -> Result<(), ZwiimError> {
    if blocks.is_empty() {
        return Err(empty_prompt());
    }
    let row = resolve(&db.conn, session_id).await?;
    let lock = session_lock(session_id);
    let _guard = lock.lock().await;
    // Held until the turn is under way, so no branch switch (Continue) lands
    // between its busy check and this turn starting. Never resume in a removed
    // worktree, nor anywhere else instead of it.
    let (folder, _entered) = crate::workspace::begin_launch(&db.conn, row.folder_id).await?;
    if folder.parent_id.is_none() {
        return Err(ZwiimError::root_workspace());
    }
    let (connection_id, spawned) = ensure_live(db, connections, launcher, &row, &folder).await?;
    // Branch A of `send_prompt_linked`: a freshly resumed connection adopts
    // this row; an already-linked one ignores both ids.
    let sent = connections
        .send_prompt_linked_with_message_id(
            db,
            &connection_id,
            blocks,
            Some(row.folder_id),
            Some(row.id),
            None,
            client_message_id,
        )
        .await;
    if let Err(e) = sent {
        if spawned {
            let _ = connections.disconnect(&connection_id).await;
        }
        return Err(e.into());
    }
    Ok(())
}

/// Session `session_id`'s options; `None` when its agent has never advertised
/// any on this host.
pub async fn options(
    db: &AppDatabase,
    connections: &ConnectionManager,
    session_id: &str,
) -> Result<Option<SessionOptions>, ZwiimError> {
    let row = resolve(&db.conn, session_id).await?;
    Ok(crate::session_options::read(&db.conn, connections, &row, session_id).await)
}

/// Change session `session_id`'s options between turns, then return them. A
/// session with no live agent keeps the change for its next respawn.
pub async fn set_options(
    db: &AppDatabase,
    connections: &ConnectionManager,
    session_id: &str,
    request: &OptionsRequest,
) -> Result<Option<SessionOptions>, ZwiimError> {
    let row = resolve(&db.conn, session_id).await?;
    let lock = session_lock(session_id);
    let _guard = lock.lock().await;
    crate::session_options::change(&db.conn, connections, &row, session_id, request).await?;
    Ok(crate::session_options::read(&db.conn, connections, &row, session_id).await)
}

/// Cancel the running turn. A session with no live agent has nothing to
/// cancel: success.
pub async fn cancel(
    db: &AppDatabase,
    connections: &ConnectionManager,
    session_id: &str,
) -> Result<(), ZwiimError> {
    match live_connection_id(&db.conn, connections, session_id).await? {
        Some(connection_id) => Ok(connections.cancel(&db.conn, &connection_id).await?),
        None => Ok(()),
    }
}

/// Whether an agent is working: a turn runs, or a request waits on the user.
pub(crate) fn is_busy(s: &crate::acp::SessionState) -> bool {
    s.turn_in_flight
        || s.pending_permission.is_some()
        || s.pending_question.is_some()
        || s.pending_plan_approval.is_some()
}

/// How long `stop` waits for a cancelled turn to end before disconnecting.
const STOP_TURN_WAIT: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(5)
};

/// Stop session `session_id`'s agent, keeping the session. A running turn is
/// cancelled (declining any request waiting on the user) and given
/// `STOP_TURN_WAIT` to end; then the agent is disconnected, which ends its
/// process tree and any background work with it. The row and the
/// agent-native transcript stay, the session reports `live: false`, and the
/// next `send` resumes it. A session with no live agent is already stopped:
/// success.
pub async fn stop(
    db: &AppDatabase,
    connections: &ConnectionManager,
    session_id: &str,
) -> Result<(), ZwiimError> {
    let row = resolve(&db.conn, session_id).await?;
    let lock = session_lock(session_id);
    let _guard = lock.lock().await;
    let Some(connection_id) = connection_for(connections, &row).await else {
        return Ok(());
    };
    if let Some(state) = connections.get_state(&connection_id).await {
        if is_busy(&*state.read().await) {
            // Best effort: the disconnect below ends the turn regardless.
            let _ = connections.cancel(&db.conn, &connection_id).await;
            let deadline = tokio::time::Instant::now() + STOP_TURN_WAIT;
            while state.read().await.turn_in_flight && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    match connections.disconnect(&connection_id).await {
        Ok(()) | Err(AcpError::ConnectionNotFound(_)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Delete session `session_id`. Refused with `turn_in_progress` while a turn
/// runs (cancel it first); an idle attached agent is stopped. The row is
/// soft-deleted through the conversation delete path, so clients get
/// `session.removed`; the agent-native transcript stays on disk.
pub async fn delete(
    db: &AppDatabase,
    emitter: &EventEmitter,
    connections: &ConnectionManager,
    session_id: &str,
) -> Result<(), ZwiimError> {
    let row = resolve(&db.conn, session_id).await?;
    let lock = session_lock(session_id);
    let _guard = lock.lock().await;
    if let Some(connection_id) = connection_for(connections, &row).await {
        if let Some(state) = connections.get_state(&connection_id).await {
            if state.read().await.turn_in_flight {
                return Err(ZwiimError::turn_in_progress());
            }
        }
        connections.disconnect(&connection_id).await?;
    }
    crate::commands::conversations::delete_conversation_with_cleanup_core(
        emitter,
        &db.conn,
        row.id,
    )
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionReplyItem {
    pub question_id: String,
    pub labels: Vec<String>,
}

/// A client's answer to a pending `agent.request`, routed by its `kind`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", content = "answer", rename_all = "snake_case")]
pub enum RequestAnswer {
    Permission {
        option_id: String,
    },
    Question {
        #[serde(default)]
        answers: Vec<QuestionReplyItem>,
        #[serde(default)]
        declined: bool,
    },
    PlanApproval {
        decision: PlanApprovalDecision,
        #[serde(default)]
        feedback: Option<String>,
    },
}

/// Answer the pending request `request_id` of session `session_id`. The
/// request must be the one currently pending on that session's agent.
pub async fn respond(
    db: &AppDatabase,
    connections: &ConnectionManager,
    session_id: &str,
    request_id: &str,
    answer: RequestAnswer,
) -> Result<(), ZwiimError> {
    let connection_id = live_connection_id(&db.conn, connections, session_id)
        .await?
        .ok_or_else(ZwiimError::session_not_live)?;
    let state = connections
        .get_state(&connection_id)
        .await
        .ok_or_else(ZwiimError::session_not_live)?;
    let (pending, plan_exit_agent) = {
        let s = state.read().await;
        let plan_exit_agent = s
            .pending_permission
            .as_ref()
            .filter(|p| p.tool_call.get("kind").and_then(|k| k.as_str()) == Some("switch_mode"))
            .map(|_| s.agent_type);
        let pending = match &answer {
            RequestAnswer::Permission { .. } => s
                .pending_permission
                .as_ref()
                .is_some_and(|p| p.request_id == request_id),
            RequestAnswer::Question { .. } => s
                .pending_question
                .as_ref()
                .is_some_and(|q| q.question_id == request_id),
            RequestAnswer::PlanApproval { .. } => s
                .pending_plan_approval
                .as_ref()
                .is_some_and(|p| p.approval_id == request_id),
        };
        (pending, plan_exit_agent)
    };
    if !pending {
        return Err(ZwiimError::not_found("No such pending request"));
    }
    match answer {
        RequestAnswer::Permission { option_id } => {
            connections
                .respond_permission(&connection_id, request_id, &option_id)
                .await?;
            if let Some(agent) = plan_exit_agent {
                crate::session_options::sync_plan_exit(connections, &connection_id, agent, &option_id)
                    .await;
            }
        }
        RequestAnswer::Question { answers, declined } => {
            let answer = QuestionAnswer {
                answers: answers
                    .into_iter()
                    .map(|a| QuestionAnswerItem {
                        question_id: a.question_id,
                        labels: a.labels,
                    })
                    .collect(),
                declined,
            };
            connections
                .answer_question(&connection_id, request_id, answer)
                .await?
        }
        RequestAnswer::PlanApproval { decision, feedback } => {
            connections
                .answer_plan_approval(
                    &connection_id,
                    request_id,
                    PlanApprovalAnswer { decision, feedback },
                )
                .await?
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::conversation::ConversationKind;
    use crate::db::service::conversation_service;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};
    use crate::web::event_bridge::EventEmitter;
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, IntoActiveModel};

    async fn row(db: &crate::db::AppDatabase, id: i32) -> conversation::Model {
        conversation::Entity::find_by_id(id)
            .one(&db.conn)
            .await
            .unwrap()
            .expect("row")
    }

    #[tokio::test]
    async fn session_id_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let (sid, conv_id) = {
            let db = crate::db::init_database(dir.path(), "test").await.unwrap();
            let folder = seed_folder(&db, "/tmp/zwiim-session-restart").await;
            let conv = seed_conversation(&db, folder, AgentType::ClaudeCode).await;
            (row(&db, conv).await.session_id.expect("minted"), conv)
        };
        let db = crate::db::init_database(dir.path(), "test").await.unwrap();
        assert_eq!(
            row(&db, conv_id).await.session_id.as_deref(),
            Some(sid.as_str())
        );
        assert_eq!(resolve(&db.conn, &sid).await.unwrap().id, conv_id);
    }

    #[tokio::test]
    async fn new_conversation_gets_new_session_id_and_internal_kinds_get_none() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/zwiim-session-new").await;
        let a = row(
            &db,
            seed_conversation(&db, folder, AgentType::ClaudeCode).await,
        )
        .await;
        let b = row(
            &db,
            seed_conversation(&db, folder, AgentType::ClaudeCode).await,
        )
        .await;
        assert!(a.session_id.is_some() && b.session_id.is_some());
        assert_ne!(a.session_id, b.session_id);
        assert_ne!(a.session_id.as_deref(), Some(a.id.to_string().as_str()));

        // Delegate (child of `a`) and Loop rows are internal: no session_id.
        let delegate = conversation::ActiveModel {
            folder_id: Set(folder),
            agent_type: Set(a.agent_type.clone()),
            status: Set(ConversationStatus::InProgress),
            kind: Set(ConversationKind::Delegate),
            parent_id: Set(Some(a.id)),
            message_count: Set(0),
            title_locked: Set(false),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            ..Default::default()
        }
        .insert(&db.conn)
        .await
        .unwrap();
        assert_eq!(delegate.session_id, None);
        let listed = list_for_workspace(
            &db.conn,
            &ConnectionManager::new(),
            &crate::workspace::workspace_id_of(&db.conn, folder)
                .await
                .unwrap()
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(listed.len(), 2, "delegate is not a client-facing session");
    }

    #[tokio::test]
    async fn session_id_survives_external_id_and_model_changes() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/zwiim-session-ext").await;
        let id = seed_conversation(&db, folder, AgentType::ClaudeCode).await;
        let sid = row(&db, id).await.session_id;

        conversation_service::bind_external_id(&db.conn, id, "ext-1", &[])
            .await
            .unwrap();
        // A resumed/idle-respawned agent reports a successor session id that
        // continues the old one: same row, same session.
        let split =
            conversation_service::bind_external_id(&db.conn, id, "ext-2", &["ext-1".to_string()])
                .await
                .unwrap();
        assert_eq!(split, None);
        let after = row(&db, id).await;
        assert_eq!(after.external_id.as_deref(), Some("ext-2"));
        assert_eq!(after.session_id, sid);

        let mut am = after.into_active_model();
        am.model = Set(Some("other-model".into()));
        let updated = am.update(&db.conn).await.unwrap();
        assert_eq!(updated.session_id, sid);
    }

    #[tokio::test]
    async fn split_keeps_live_session_and_mints_one_for_preserved_history() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/zwiim-session-split").await;
        let id = seed_conversation(&db, folder, AgentType::ClaudeCode).await;
        let sid = row(&db, id).await.session_id;
        conversation_service::bind_external_id(&db.conn, id, "ext-1", &[])
            .await
            .unwrap();

        let preserved = conversation_service::bind_external_id(&db.conn, id, "ext-other", &[])
            .await
            .unwrap()
            .expect("unrelated session splits the row");

        assert_eq!(row(&db, id).await.session_id, sid, "live row keeps its id");
        let preserved = row(&db, preserved).await;
        assert_eq!(preserved.external_id.as_deref(), Some("ext-1"));
        assert!(preserved.session_id.is_some());
        assert_ne!(preserved.session_id, sid);
    }

    #[tokio::test]
    async fn session_id_survives_soft_delete_and_restore() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/zwiim-session-restore").await;
        let id = seed_conversation(&db, folder, AgentType::ClaudeCode).await;
        let sid = row(&db, id).await.session_id.unwrap();

        conversation_service::soft_delete(&db.conn, id)
            .await
            .unwrap();
        assert!(resolve(&db.conn, &sid).await.is_err());
        assert!(
            conversation_service::restore_soft_deleted(&db.conn, id, folder)
                .await
                .unwrap()
        );
        assert_eq!(resolve(&db.conn, &sid).await.unwrap().id, id);
    }

    #[tokio::test]
    async fn resolver_follows_the_live_connection_across_reconnect() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/zwiim-session-live").await;
        let id = seed_conversation(&db, folder, AgentType::ClaudeCode).await;
        conversation_service::bind_external_id(&db.conn, id, "ext-1", &[])
            .await
            .unwrap();
        let sid = row(&db, id).await.session_id.unwrap();

        let mgr = ConnectionManager::new();
        assert_eq!(
            live_connection_id(&db.conn, &mgr, &sid).await.unwrap(),
            None
        );

        mgr.insert_test_connection("conn-a", AgentType::ClaudeCode, None, EventEmitter::Noop)
            .await;
        mgr.get_state("conn-a")
            .await
            .unwrap()
            .write()
            .await
            .conversation_id = Some(id);
        assert_eq!(
            live_connection_id(&db.conn, &mgr, &sid)
                .await
                .unwrap()
                .as_deref(),
            Some("conn-a")
        );

        // The agent reconnects as a new connection that has only reported its
        // agent-native session so far: same session, new connection.
        let mgr = ConnectionManager::new();
        mgr.insert_test_connection("conn-b", AgentType::ClaudeCode, None, EventEmitter::Noop)
            .await;
        mgr.get_state("conn-b")
            .await
            .unwrap()
            .write()
            .await
            .external_id = Some("ext-1".into());
        assert_eq!(
            live_connection_id(&db.conn, &mgr, &sid)
                .await
                .unwrap()
                .as_deref(),
            Some("conn-b")
        );
        assert_eq!(row(&db, id).await.session_id.as_deref(), Some(sid.as_str()));

        let listed = list_for_workspace(
            &db.conn,
            &mgr,
            &crate::workspace::workspace_id_of(&db.conn, folder)
                .await
                .unwrap()
                .unwrap(),
        )
        .await
        .unwrap();
        assert!(listed[0].attached);
    }

    #[tokio::test]
    async fn session_info_carries_no_raw_ids() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/zwiim-session-json").await;
        seed_conversation(&db, folder, AgentType::ClaudeCode).await;
        let wid = crate::workspace::workspace_id_of(&db.conn, folder)
            .await
            .unwrap()
            .unwrap();
        let listed = list_for_workspace(&db.conn, &ConnectionManager::new(), &wid)
            .await
            .unwrap();
        let json = serde_json::to_value(&listed[0]).unwrap();
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "agent_type",
                "attached",
                "created_at",
                "session_id",
                "status",
                "title",
                "updated_at",
                "workspace_id"
            ]
        );
    }
}

#[cfg(test)]
pub(crate) mod orchestration_tests {
    use super::*;
    use crate::acp::connection::ConnectionCommand;
    use crate::acp::question::{QuestionOption, QuestionSpec};
    use crate::acp::types::{AcpEvent, PermissionOptionInfo};
    use crate::db::test_helpers::{fresh_in_memory_db, seed_worktree_folder};
    use crate::web::event_bridge::{emit_with_state, EventEmitter};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// Launches in-process fake agents: a live command channel whose commands
    /// are recorded, and an agent-native session id like a real ACP handshake.
    pub(crate) struct FakeLauncher {
        pub connections: ConnectionManager,
        pub launches: AtomicUsize,
        pub resumed: std::sync::Mutex<Vec<Option<String>>>,
        /// The directory each agent was launched in, in launch order.
        pub working_dirs: std::sync::Mutex<Vec<String>>,
        pub commands: Arc<std::sync::Mutex<Vec<(String, &'static str)>>>,
        pub delay: Duration,
    }

    impl FakeLauncher {
        pub(crate) fn new(connections: ConnectionManager) -> Self {
            Self {
                connections,
                launches: AtomicUsize::new(0),
                resumed: Default::default(),
                working_dirs: Default::default(),
                commands: Default::default(),
                delay: Duration::ZERO,
            }
        }

        pub(crate) fn commands_for(&self, conn: &str) -> Vec<&'static str> {
            self.commands
                .lock()
                .unwrap()
                .iter()
                .filter(|(c, _)| c == conn)
                .map(|(_, k)| *k)
                .collect()
        }
    }

    impl AgentLauncher for FakeLauncher {
        async fn launch(
            &self,
            agent_type: AgentType,
            working_dir: String,
            resume_external_id: Option<String>,
            _preferred: BTreeMap<String, String>,
        ) -> Result<String, ZwiimError> {
            tokio::time::sleep(self.delay).await;
            let n = self.launches.fetch_add(1, Ordering::SeqCst) + 1;
            let id = format!("fake-conn-{n}");
            self.working_dirs.lock().unwrap().push(working_dir.clone());
            let mut rx = self
                .connections
                .insert_test_connection_live(
                    &id,
                    agent_type,
                    Some(working_dir.into()),
                    EventEmitter::Noop,
                )
                .await;
            let native = resume_external_id
                .clone()
                .unwrap_or_else(|| format!("native-{n}"));
            self.connections
                .get_state(&id)
                .await
                .expect("state")
                .write()
                .await
                .external_id = Some(native);
            self.resumed.lock().unwrap().push(resume_external_id);
            let log = self.commands.clone();
            let conn = id.clone();
            tokio::spawn(async move {
                while let Some(cmd) = rx.recv().await {
                    let kind = match cmd {
                        ConnectionCommand::Prompt { .. } => "prompt",
                        ConnectionCommand::Cancel => "cancel",
                        ConnectionCommand::RespondPermission { .. } => "respond_permission",
                        ConnectionCommand::Disconnect => "disconnect",
                        _ => "other",
                    };
                    log.lock().unwrap().push((conn.clone(), kind));
                }
            });
            Ok(id)
        }
    }

    pub(crate) fn text(t: &str) -> Vec<PromptInputBlock> {
        vec![PromptInputBlock::Text { text: t.into() }]
    }

    pub(crate) async fn workspace(db: &AppDatabase) -> String {
        let dir = std::env::temp_dir().join(format!("zwiim-orch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let folder = seed_worktree_folder(db, dir.to_str().unwrap()).await;
        crate::workspace::workspace_id_of(&db.conn, folder)
            .await
            .unwrap()
            .expect("workspace_id")
    }

    /// End the in-flight turn so the next prompt passes the turn gate.
    pub(crate) async fn finish_turn(connections: &ConnectionManager, conn: &str) {
        let (state, emitter) = connections.get_state_and_emitter(conn).await.unwrap();
        emit_with_state(
            &state,
            &emitter,
            AcpEvent::TurnComplete {
                session_id: "native".into(),
                stop_reason: "end_turn".into(),
                agent_type: "claude_code".into(),
            },
        )
        .await;
    }

    async fn settle() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    #[tokio::test]
    async fn start_creates_session_and_send_reuses_live_agent() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());

        let sid = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::ClaudeCode,
            text("hi"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        let row = resolve(&db.conn, &sid).await.unwrap();
        assert_eq!(row.session_id.as_deref(), Some(sid.as_str()));
        assert_eq!(
            live_connection_id(&db.conn, &mgr, &sid)
                .await
                .unwrap()
                .as_deref(),
            Some("fake-conn-1")
        );

        finish_turn(&mgr, "fake-conn-1").await;
        send(&db, &mgr, &launcher, &sid, text("again"), None)
            .await
            .unwrap();
        settle().await;
        assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
        assert_eq!(launcher.commands_for("fake-conn-1"), ["prompt", "prompt"]);

        // A second prompt mid-turn is refused, not queued onto a new agent.
        let busy = send(&db, &mgr, &launcher, &sid, text("x"), None)
            .await
            .unwrap_err();
        assert_eq!(busy.code, "turn_in_progress");
        assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_send_waits_while_the_workspace_is_held_exclusively() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::ClaudeCode,
            text("hi"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        finish_turn(&mgr, "fake-conn-1").await;
        let folder_id = resolve(&db.conn, &sid).await.unwrap().folder_id;
        let in_flight = || async {
            mgr.get_state("fake-conn-1").await.unwrap().read().await.turn_in_flight
        };

        let exclusive = crate::workspace::enter_exclusive(folder_id).await;
        let sending = send(&db, &mgr, &launcher, &sid, text("again"), None);
        tokio::pin!(sending);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut sending)
                .await
                .is_err(),
            "the send waits for the workspace"
        );
        assert!(!in_flight().await, "no turn starts while the workspace is held");

        drop(exclusive);
        sending.await.unwrap();
        assert!(in_flight().await);
    }

    #[tokio::test]
    async fn delete_waits_for_the_turn_then_stops_the_agent() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let emitter = EventEmitter::Noop;

        let sid = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::ClaudeCode,
            text("hi"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        let busy = delete(&db, &emitter, &mgr, &sid).await.unwrap_err();
        assert_eq!(busy.code, "turn_in_progress");
        assert!(resolve(&db.conn, &sid).await.is_ok(), "nothing deleted mid-turn");

        finish_turn(&mgr, "fake-conn-1").await;
        delete(&db, &emitter, &mgr, &sid).await.unwrap();
        settle().await;
        assert!(mgr.get_state("fake-conn-1").await.is_none(), "agent stopped");
        assert!(resolve(&db.conn, &sid).await.is_err());
        assert!(list_for_workspace(&db.conn, &mgr, &wid)
            .await
            .unwrap()
            .is_empty());
        let gone = send(&db, &mgr, &launcher, &sid, text("again"), None)
            .await
            .unwrap_err();
        assert_eq!(gone.code, "not_found");
        assert_eq!(launcher.launches.load(Ordering::SeqCst), 1, "never resumed");

        // A session with no agent attached deletes directly.
        let other = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::ClaudeCode,
            text("hi"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        mgr.disconnect("fake-conn-2").await.unwrap();
        delete(&db, &emitter, &mgr, &other).await.unwrap();
        assert!(resolve(&db.conn, &other).await.is_err());
    }

    #[tokio::test]
    async fn send_resumes_dead_agent_under_same_session_id() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());

        let sid = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::ClaudeCode,
            text("hi"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        // Idle sweep / crash / restart: the agent is gone.
        mgr.disconnect("fake-conn-1").await.unwrap();
        assert!(live_connection_id(&db.conn, &mgr, &sid)
            .await
            .unwrap()
            .is_none());
        let external_id = resolve(&db.conn, &sid).await.unwrap().external_id;
        assert_eq!(external_id.as_deref(), Some("native-1"));

        send(&db, &mgr, &launcher, &sid, text("back"), None)
            .await
            .unwrap();
        settle().await;
        assert_eq!(launcher.launches.load(Ordering::SeqCst), 2);
        assert_eq!(launcher.resumed.lock().unwrap()[1], external_id);
        assert_eq!(
            live_connection_id(&db.conn, &mgr, &sid)
                .await
                .unwrap()
                .as_deref(),
            Some("fake-conn-2")
        );
        assert_eq!(launcher.commands_for("fake-conn-2"), ["prompt"]);
        // Same session row, same id.
        let row = resolve(&db.conn, &sid).await.unwrap();
        assert_eq!(row.session_id.as_deref(), Some(sid.as_str()));
    }

    #[tokio::test]
    async fn concurrent_sends_to_dead_session_spawn_one_agent() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let mut launcher = FakeLauncher::new(mgr.clone_ref());

        let sid = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::ClaudeCode,
            text("hi"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        mgr.disconnect("fake-conn-1").await.unwrap();
        launcher.delay = Duration::from_millis(50);

        let (a, b, c) = tokio::join!(
            send(&db, &mgr, &launcher, &sid, text("a"), None),
            send(&db, &mgr, &launcher, &sid, text("b"), None),
            send(&db, &mgr, &launcher, &sid, text("c"), None),
        );
        assert_eq!(
            launcher.launches.load(Ordering::SeqCst),
            2,
            "one resume only"
        );
        let results = [a, b, c];
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        for r in results.iter().filter_map(|r| r.as_ref().err()) {
            assert_eq!(r.code, "turn_in_progress");
        }
    }

    #[tokio::test]
    async fn start_failure_disconnects_the_new_agent() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());

        let err = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::ClaudeCode,
            vec![],
            None,
            &Default::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "invalid_input");
        assert_eq!(launcher.launches.load(Ordering::SeqCst), 0);

        // The prompt itself fails (agent died right after spawn).
        struct DeadLauncher(ConnectionManager);
        impl AgentLauncher for DeadLauncher {
            async fn launch(
                &self,
                agent_type: AgentType,
                _working_dir: String,
                _resume: Option<String>,
                _preferred: BTreeMap<String, String>,
            ) -> Result<String, ZwiimError> {
                self.0
                    .insert_test_connection("dead", agent_type, None, EventEmitter::Noop)
                    .await;
                Ok("dead".into())
            }
        }
        let err = start(
            &db,
            &mgr,
            &DeadLauncher(mgr.clone_ref()),
            &wid,
            AgentType::ClaudeCode,
            text("hi"),
            None,
            &Default::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "session_not_live");
        assert!(mgr.get_state("dead").await.is_none(), "no orphan agent");

        let err = start(
            &db,
            &mgr,
            &launcher,
            "nope",
            AgentType::ClaudeCode,
            text("hi"),
            None,
            &Default::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "not_found");
    }

    #[tokio::test]
    async fn cancel_reaches_live_agent_and_is_ok_when_idle() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::ClaudeCode,
            text("hi"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();

        cancel(&db, &mgr, &sid).await.unwrap();
        settle().await;
        assert_eq!(launcher.commands_for("fake-conn-1"), ["prompt", "cancel"]);

        mgr.disconnect("fake-conn-1").await.unwrap();
        cancel(&db, &mgr, &sid).await.unwrap();
        assert_eq!(
            launcher.launches.load(Ordering::SeqCst),
            1,
            "cancel never resumes"
        );
    }

    /// Like an agent, end the turn once it is told to cancel.
    fn ends_turn_on_cancel(mgr: &ConnectionManager, launcher: &FakeLauncher, conn: &'static str) {
        let mgr = mgr.clone_ref();
        let log = launcher.commands.clone();
        tokio::spawn(async move {
            while !log.lock().unwrap().contains(&(conn.to_string(), "cancel")) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            finish_turn(&mgr, conn).await;
        });
    }

    async fn started(db: &AppDatabase, mgr: &ConnectionManager, launcher: &FakeLauncher, wid: &str) -> String {
        start(db, mgr, launcher, wid, AgentType::ClaudeCode, text("hi"), None, &Default::default())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn stop_ends_the_turn_then_disconnects_and_send_resumes_the_same_session() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = started(&db, &mgr, &launcher, &wid).await;
        let before = resolve(&db.conn, &sid).await.unwrap();

        // Mid-turn: cancelled, awaited, then disconnected — well within the bound.
        ends_turn_on_cancel(&mgr, &launcher, "fake-conn-1");
        let at = std::time::Instant::now();
        stop(&db, &mgr, &sid).await.unwrap();
        assert!(at.elapsed() < STOP_TURN_WAIT, "returned once the turn ended");
        settle().await;
        assert_eq!(
            launcher.commands_for("fake-conn-1"),
            ["prompt", "cancel", "disconnect"]
        );
        assert!(mgr.get_state("fake-conn-1").await.is_none(), "agent gone");

        // The session stays, not live, with its transcript link.
        let after = resolve(&db.conn, &sid).await.unwrap();
        assert_eq!((after.id, after.external_id.clone()), (before.id, before.external_id.clone()));
        let listed = list_for_workspace(&db.conn, &mgr, &wid).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].attached);

        // Stopping a stopped session is fine and never resumes it.
        stop(&db, &mgr, &sid).await.unwrap();
        assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);

        // The next send resumes the agent-native session under the same id.
        send(&db, &mgr, &launcher, &sid, text("back"), None).await.unwrap();
        settle().await;
        assert_eq!(launcher.launches.load(Ordering::SeqCst), 2);
        assert_eq!(launcher.resumed.lock().unwrap()[1], before.external_id);
        assert_eq!(launcher.commands_for("fake-conn-2"), ["prompt"]);
        assert_eq!(
            live_connection_id(&db.conn, &mgr, &sid).await.unwrap().as_deref(),
            Some("fake-conn-2")
        );
    }

    #[tokio::test]
    async fn stop_cancels_a_waiting_permission_and_is_bounded() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = started(&db, &mgr, &launcher, &wid).await;
        let (state, emitter) = mgr.get_state_and_emitter("fake-conn-1").await.unwrap();
        emit_with_state(
            &state,
            &emitter,
            AcpEvent::PermissionRequest {
                request_id: "perm-1".into(),
                tool_call: serde_json::json!({"toolCallId": "tc-1", "title": "Run ls"}),
                options: vec![],
                queued: 0,
            },
        )
        .await;
        assert!(state.read().await.pending_permission.is_some());

        // The agent never ends its turn: stop still disconnects after the bound.
        let at = std::time::Instant::now();
        stop(&db, &mgr, &sid).await.unwrap();
        assert!(at.elapsed() >= STOP_TURN_WAIT);
        settle().await;
        assert_eq!(
            launcher.commands_for("fake-conn-1"),
            ["prompt", "cancel", "disconnect"],
            "the request is cancelled, never answered"
        );
        assert!(mgr.get_state("fake-conn-1").await.is_none());
        assert!(resolve(&db.conn, &sid).await.is_ok());
    }

    #[tokio::test]
    async fn stop_ends_an_idle_agent_that_has_background_work() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = started(&db, &mgr, &launcher, &wid).await;
        finish_turn(&mgr, "fake-conn-1").await;
        {
            let state = mgr.get_state("fake-conn-1").await.unwrap();
            let mut s = state.write().await;
            s.background_outstanding = 1;
            s.background_activity_at = Some(Utc::now());
            assert!(s.has_active_background_work(Utc::now()), "the idle sweep keeps it");
        }

        stop(&db, &mgr, &sid).await.unwrap();
        settle().await;
        // Idle: nothing to cancel; the disconnect ends the agent's process tree.
        assert_eq!(launcher.commands_for("fake-conn-1"), ["prompt", "disconnect"]);
        assert!(mgr.get_state("fake-conn-1").await.is_none());
        assert!(resolve(&db.conn, &sid).await.is_ok());
    }

    #[tokio::test]
    async fn stopping_one_session_leaves_another_in_the_same_workspace_live() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let first = started(&db, &mgr, &launcher, &wid).await;
        let second = started(&db, &mgr, &launcher, &wid).await;
        finish_turn(&mgr, "fake-conn-1").await;

        stop(&db, &mgr, &first).await.unwrap();
        settle().await;
        assert!(live_connection_id(&db.conn, &mgr, &first).await.unwrap().is_none());
        assert_eq!(
            live_connection_id(&db.conn, &mgr, &second).await.unwrap().as_deref(),
            Some("fake-conn-2")
        );
        assert_eq!(launcher.commands_for("fake-conn-2"), ["prompt"], "untouched mid-turn");
        let live: Vec<_> = list_for_workspace(&db.conn, &mgr, &wid)
            .await
            .unwrap()
            .into_iter()
            .map(|s| (s.session_id, s.attached))
            .collect();
        assert!(live.contains(&(first, false)) && live.contains(&(second, true)));
    }

    #[tokio::test]
    async fn respond_routes_permission_question_and_plan_approval() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::ClaudeCode,
            text("hi"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        let conn = "fake-conn-1";

        // Permission: forwarded to the agent.
        let (state, emitter) = mgr.get_state_and_emitter(conn).await.unwrap();
        emit_with_state(
            &state,
            &emitter,
            AcpEvent::PermissionRequest {
                request_id: "perm-1".into(),
                tool_call: serde_json::json!({"toolCallId": "tc-1", "title": "Run ls"}),
                options: vec![PermissionOptionInfo {
                    option_id: "allow".into(),
                    name: "Allow".into(),
                    kind: "allow_once".into(),
                    meta: None,
                }],
                queued: 0,
            },
        )
        .await;
        let wrong = respond(
            &db,
            &mgr,
            &sid,
            "perm-other",
            RequestAnswer::Permission {
                option_id: "allow".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(wrong.code, "not_found");
        respond(
            &db,
            &mgr,
            &sid,
            "perm-1",
            RequestAnswer::Permission {
                option_id: "allow".into(),
            },
        )
        .await
        .unwrap();
        settle().await;
        assert_eq!(
            launcher.commands_for(conn),
            ["prompt", "respond_permission"]
        );

        // Question: the parked ask receives the answer.
        let q = mgr
            .register_question(
                conn,
                vec![QuestionSpec {
                    id: "q1".into(),
                    question: "Which?".into(),
                    header: "Pick".into(),
                    multi_select: false,
                    options: vec![
                        QuestionOption {
                            label: "A".into(),
                            description: String::new(),
                        },
                        QuestionOption {
                            label: "B".into(),
                            description: String::new(),
                        },
                    ],
                    is_secret: false,
                }],
            )
            .await
            .expect("registered");
        respond(
            &db,
            &mgr,
            &sid,
            &q.question_id,
            RequestAnswer::Question {
                answers: vec![QuestionReplyItem {
                    question_id: "q1".into(),
                    labels: vec!["B".into()],
                }],
                declined: false,
            },
        )
        .await
        .unwrap();
        let outcome = q.answer_rx.await.unwrap();
        assert!(!outcome.declined);
        assert_eq!(outcome.answers.len(), 1);

        // Plan approval: the parked approval receives the decision.
        let p = mgr
            .register_plan_approval(conn, "tc-plan".into(), "# Plan".into())
            .await
            .expect("registered");
        respond(
            &db,
            &mgr,
            &sid,
            &p.approval_id,
            RequestAnswer::PlanApproval {
                decision: PlanApprovalDecision::RequestChanges,
                feedback: Some("smaller".into()),
            },
        )
        .await
        .unwrap();
        let answer = p.answer_rx.await.unwrap();
        assert_eq!(answer.decision, PlanApprovalDecision::RequestChanges);
        assert_eq!(answer.feedback.as_deref(), Some("smaller"));

        // No live agent: nothing to answer.
        mgr.disconnect(conn).await.unwrap();
        let err = respond(
            &db,
            &mgr,
            &sid,
            "perm-1",
            RequestAnswer::Permission {
                option_id: "allow".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "session_not_live");
    }
}
