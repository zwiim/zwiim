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
use crate::acp::fork::LaunchIntent;
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
use crate::models::message::{MessageTurn, TurnRole};
use crate::protocol::error::ZwiimError;
use crate::session_checkpoints::TurnTag;
use crate::session_options::{Mode, OptionsRequest, SessionOptions};
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
    /// Prompts queued behind the running turn, first to go out first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub follow_ups: Vec<crate::follow_ups::Queued>,
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
    let follow_ups = row
        .follow_ups
        .as_deref()
        .map(|json| crate::follow_ups::decode(row.id, json))
        .unwrap_or_default()
        .iter()
        .map(crate::follow_ups::Queued::from)
        .collect();
    Some(SessionInfo {
        follow_ups,
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

/// The live session's current history generation (see
/// `SessionState.history_generation`), `None` when no agent is attached.
/// Read before the transcript so a turn finishing in between leaves the
/// client holding an older generation, which makes it re-read rather than
/// miss the turn.
pub async fn history_generation(state: &AppState, session_id: &str) -> Option<u64> {
    let conn_id =
        live_connection_id(&state.db.conn, &state.connection_manager, session_id).await.ok()??;
    let live = state.connection_manager.get_state(&conn_id).await?;
    let generation = live.read().await.history_generation;
    Some(generation)
}

// ─── Session orchestration (Zwiim protocol `session.*`) ───────────────────

/// Starts the agent process behind a session and returns its (host-internal)
/// connection id. [`RuntimeLauncher`] is the production implementation; tests
/// substitute one that registers a synthetic connection.
pub trait AgentLauncher: Send + Sync {
    /// `folder_id` is `working_dir`'s workspace, whose variables
    /// (`crate::workspace::scripts::workspace_env`) the agent gets. `intent` is
    /// the agent session to open.
    /// `preferred` are native option values the agent is set to before it is
    /// first used; values it no longer offers are skipped.
    fn launch(
        &self,
        agent_type: AgentType,
        working_dir: String,
        folder_id: i32,
        intent: LaunchIntent,
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
        folder_id: i32,
        intent: LaunchIntent,
        preferred: BTreeMap<String, String>,
    ) -> Result<String, ZwiimError> {
        let existing_session = match &intent {
            LaunchIntent::New => None,
            LaunchIntent::Resume(id) => Some(id.as_str()),
        };
        let runtime_env = crate::commands::acp::build_session_runtime_env(
            &self.state.db,
            agent_type,
            existing_session,
            &self.state.data_dir,
        )
        .await?;
        crate::commands::acp::verify_agent_installed(agent_type).await?;
        let conn = &self.state.db.conn;
        let repository_env = match crate::db::entities::folder::Entity::find_by_id(folder_id)
            .one(conn)
            .await
        {
            Ok(Some(workspace)) => crate::workspace::scripts::workspace_env(conn, &workspace).await,
            _ => Default::default(),
        }
        .into_iter()
        .collect();
        Ok(self
            .state
            .connection_manager
            .spawn_agent_with_env(
                agent_type,
                Some(working_dir),
                intent,
                runtime_env,
                repository_env,
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

pub(crate) fn session_lock(session_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = SESSION_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    // Drop locks nobody holds or waits on.
    locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    locks.entry(session_id.to_string()).or_default().clone()
}

/// Sessions the Host is busy with outside a turn: those whose lock is held
/// (starting a prompt, changing options, stopping, deleting, reverting,
/// queueing or draining follow-ups).
pub(crate) fn sessions_sending() -> Vec<String> {
    let locks = SESSION_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    locks
        .iter()
        .filter(|(_, lock)| lock.try_lock().is_err())
        .map(|(id, _)| id.clone())
        .collect()
}

/// New sessions being started, by the workspace they start in. They have no
/// session id yet, so they are only counted.
static STARTING: LazyLock<std::sync::Mutex<HashMap<String, usize>>> =
    LazyLock::new(Default::default);

/// A new session being started in a workspace, counted until dropped.
pub(crate) struct StartingGuard(String);

impl StartingGuard {
    pub(crate) fn new(workspace_id: &str) -> Self {
        let mut starting = STARTING.lock().unwrap_or_else(|e| e.into_inner());
        *starting.entry(workspace_id.to_string()).or_default() += 1;
        Self(workspace_id.to_string())
    }
}

impl Drop for StartingGuard {
    fn drop(&mut self) {
        let mut starting = STARTING.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = starting.get_mut(&self.0) {
            *n -= 1;
            if *n == 0 {
                starting.remove(&self.0);
            }
        }
    }
}

/// The workspaces new sessions are being started in, with how many each.
pub(crate) fn workspaces_starting() -> Vec<(String, usize)> {
    let starting = STARTING.lock().unwrap_or_else(|e| e.into_inner());
    starting.iter().map(|(id, n)| (id.clone(), *n)).collect()
}

/// Hold a start in `workspace_id` for the caller's span (tests).
#[cfg(test)]
pub(crate) fn hold_start(workspace_id: &str) -> impl Drop {
    StartingGuard::new(workspace_id)
}

fn empty_prompt() -> ZwiimError {
    ZwiimError::invalid_input("Prompt must contain at least one block")
}

pub(crate) async fn session_id_of(db: &AppDatabase, conversation_id: i32) -> Result<String, ZwiimError> {
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
    start_tracked(
        db,
        connections,
        launcher,
        workspace_id,
        agent_type,
        blocks,
        client_message_id,
        options,
        &mut false,
    )
    .await
}

/// `start`, setting `dispatched` once the first prompt is on its way, so a
/// caller can tell an error that left the agent running on its blocks from one
/// that did not.
pub(crate) async fn start_tracked<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    workspace_id: &str,
    agent_type: AgentType,
    blocks: Vec<PromptInputBlock>,
    client_message_id: Option<String>,
    options: &OptionsRequest,
    dispatched: &mut bool,
) -> Result<String, ZwiimError> {
    if blocks.is_empty() {
        return Err(empty_prompt());
    }
    let preferred =
        crate::session_options::preferred_for_start(&db.conn, agent_type, options).await?;
    // Counted as starting (a when-idle restart waits) until the first prompt
    // is sent.
    let starting = StartingGuard::new(workspace_id);
    let (folder, launching) = crate::workspace::resolve_for_launch(&db.conn, workspace_id).await?;
    if folder.parent_id.is_none() {
        return Err(ZwiimError::root_workspace());
    }
    // `launching` is held through the first prompt, so the new agent's turn
    // is under way before anything can change the workspace's branch.
    let connection_id = launcher
        .launch(
            agent_type,
            folder.path,
            folder.id,
            LaunchIntent::New,
            preferred.clone().unwrap_or_default(),
        )
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
    drop(starting);
    match sent {
        Ok(Some(conversation_id)) => {
            *dispatched = true;
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
        .launch(
            agent_type,
            folder.path.clone(),
            folder.id,
            row.external_id.clone().into(),
            preferred,
        )
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
    send_tagged(db, connections, launcher, session_id, blocks, client_message_id, None).await
}

/// As [`send`], for a turn the Host started rather than the user typed:
/// `tag` goes on the turn's start checkpoint and its end is checked.
pub(crate) async fn send_tagged<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    session_id: &str,
    blocks: Vec<PromptInputBlock>,
    client_message_id: Option<String>,
    tag: Option<TurnTag>,
) -> Result<(), ZwiimError> {
    let row = resolve(&db.conn, session_id).await?;
    let lock = session_lock(session_id);
    let _guard = lock.lock().await;
    if blocks.is_empty() && !crate::session_fork::holds_transcript(&db.conn, &row).await {
        return Err(empty_prompt());
    }
    send_locked(
        db,
        connections,
        launcher,
        &row,
        blocks,
        client_message_id,
        tag,
    )
    .await
}

/// The body of [`send_tagged`] for a caller already holding `row`'s session
/// lock. Once the prompt is dispatched, any saved can't-reach turn is stale:
/// it is cleared, and a failure to clear is logged rather than failing a
/// prompt that already went out.
pub(crate) async fn send_locked<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    row: &conversation::Model,
    mut blocks: Vec<PromptInputBlock>,
    client_message_id: Option<String>,
    tag: Option<TurnTag>,
) -> Result<(), ZwiimError> {
    // Held until the turn is under way, so no branch switch (Continue) lands
    // between its busy check and this turn starting. Never resume in a removed
    // worktree, nor anywhere else instead of it.
    let (folder, _entered) = crate::workspace::begin_launch(&db.conn, row.folder_id).await?;
    if folder.parent_id.is_none() {
        return Err(ZwiimError::root_workspace());
    }
    let owed = crate::session_fork::prepare_message(db, connections, row, &mut blocks).await?;
    let (connection_id, spawned) = ensure_live(db, connections, launcher, row, &folder).await?;
    if spawned {
        crate::session_fork::apply_held_options(connections, &connection_id, &owed).await;
    }
    // Branch A of `send_prompt_linked`: a freshly resumed connection adopts
    // this row; an already-linked one ignores both ids.
    let sent = connections
        .send_prompt_linked_tagged(
            db,
            &connection_id,
            blocks,
            Some(row.folder_id),
            Some(row.id),
            None,
            client_message_id,
            tag,
        )
        .await;
    if let Err(e) = sent {
        if spawned {
            let _ = connections.disconnect(&connection_id).await;
        }
        return Err(e.into());
    }
    crate::session_fork::message_sent(db, row, &owed, spawned).await;
    // Cleared after dispatch so a failed send keeps the Retry; a new turn can't
    // fail as can't-reach (60s or more) before this runs, so it can't be erased.
    if let Err(e) =
        crate::db::service::conversation_service::set_unreachable_turn(&db.conn, row.id, None).await
    {
        tracing::warn!(
            conversation_id = row.id,
            error = %e,
            "could not clear the saved unreachable turn"
        );
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

/// Whether a session in folder `folder_id` other than row `except` has a
/// turn running.
pub(crate) async fn other_busy_in_folder(
    db: &DatabaseConnection,
    connections: &ConnectionManager,
    folder_id: i32,
    except: i32,
) -> Result<bool, AppCommandError> {
    let rows = conversation::Entity::find()
        .filter(conversation::Column::FolderId.eq(folder_id))
        .filter(conversation::Column::Id.ne(except))
        .filter(conversation::Column::SessionId.is_not_null())
        .filter(conversation::Column::DeletedAt.is_null())
        .all(db)
        .await
        .map_err(db_err)?;
    for row in rows {
        if row_busy(connections, &row).await {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether the agent attached to session `row`, if any, is working.
pub(crate) async fn row_busy(connections: &ConnectionManager, row: &conversation::Model) -> bool {
    let Some(connection_id) = connection_for(connections, row).await else {
        return false;
    };
    match connections.get_state(&connection_id).await {
        Some(state) => is_busy(&*state.read().await),
        None => false,
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

/// Wait up to `STOP_TURN_WAIT` for the running turn to end. Whether it did.
async fn wait_turn_end(state: &tokio::sync::RwLock<crate::acp::SessionState>) -> bool {
    let deadline = tokio::time::Instant::now() + STOP_TURN_WAIT;
    while state.read().await.turn_in_flight {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    true
}

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
        connections
            .turn_checkpoints
            .close_conversation(row.id, "stopped")
            .await;
        return Ok(());
    };
    if let Some(state) = connections.get_state(&connection_id).await {
        if is_busy(&*state.read().await) {
            // Best effort: the disconnect below ends the turn regardless.
            let _ = connections.cancel(&db.conn, &connection_id).await;
            wait_turn_end(&state).await;
        }
    }
    let disconnected = match connections.disconnect(&connection_id).await {
        Ok(()) | Err(AcpError::ConnectionNotFound(_)) => Ok(()),
        Err(e) => Err(e.into()),
    };
    // The turn's end checkpoint exists by the time stop returns.
    connections
        .turn_checkpoints
        .close_conversation(row.id, "stopped")
        .await;
    disconnected
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
    let mut checkpoint_roots = connections
        .turn_checkpoints
        .forget_conversation(row.id)
        .await;
    crate::commands::conversations::delete_conversation_with_cleanup_core(
        emitter,
        &db.conn,
        row.id,
    )
    .await?;
    if let Ok(Some(folder)) = crate::db::entities::folder::Entity::find_by_id(row.folder_id)
        .one(&db.conn)
        .await
    {
        checkpoint_roots.push(folder.path.into());
    }
    checkpoint_roots.sort();
    checkpoint_roots.dedup();
    for root in checkpoint_roots {
        crate::git_harness::checkpoint::gc_session(&root, session_id).await;
    }
    Ok(())
}

/// What `session.revert` did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RevertOutcome {
    /// The workspace's files and staged changes are back to how they were
    /// when the turn was sent.
    pub files_restored: bool,
    /// The conversation was taken back to before the turn: the agent carries
    /// on from the answer that preceded it. `false` when the session's agent
    /// cannot rewind its chat (see [`can_rewind_chat`]), or when the rewind
    /// failed after the files were restored.
    pub chat_rewound: bool,
    /// The checked-out commit is not the one the turn started on (the agent
    /// committed, or the branch changed). It was left as it is.
    pub head_moved: bool,
}

/// Whether `agent`'s sessions can be rewound to before a turn: its adapter
/// forks its session at an exact earlier answer.
pub fn can_rewind_chat(agent: AgentType) -> bool {
    matches!(
        agent,
        AgentType::Codex | AgentType::ClaudeCode | AgentType::DeepSeek
    )
}

/// Take session `session_id` back to before user turn `turn_id`.
///
/// With `files`, the workspace's files and staged changes return to the
/// turn's start checkpoint; HEAD, branches and ignored files are never
/// touched, and the state replaced is kept for recovery (see
/// [`crate::git_harness::checkpoint::restore`]). One exception: a turn the
/// Host ran to resolve merge conflicts whose commits were never pushed has
/// its branch moved back to where the turn started, so the merge it
/// committed is undone too. A merge or rebase left in progress is aborted
/// first; whatever the agent had resolved of it is not kept.
///
/// With `chat`, the agent's session is forked just after the answer that
/// preceded the turn and the session carries on from the fork: same
/// `session_id`, the turn and everything after it gone from its history. The
/// history before the rewind is kept out of sight. Checkpoints of the turns
/// rewound away are dropped, so later turns map onto their own. An agent that
/// cannot rewind ([`can_rewind_chat`]) leaves the chat as it is, reported as
/// `chat_rewound: false`; the first turn, having no answer before it, cannot
/// be rewound to (`checkpoint_unavailable`).
///
/// With both, the files are restored first and the chat rewound after. A
/// rewind that then fails does not hide the restore: the result reports
/// `files_restored: true, chat_rewound: false`.
///
/// A root workspace (the repository folder itself) is never reverted, files
/// or chat: any revert there is refused with `root_workspace` before anything
/// changes. A reset would rewrite the user's own checkout, and a chat rewind
/// may have to resume the agent, which never runs there.
///
/// Refused with `turn_in_progress` while the session's turn runs,
/// `workspace_busy` while another agent works in the workspace or the
/// repository is mid-operation (other than a merge or rebase), and `checkpoint_unavailable` when the turn
/// has no checkpoint that is certainly its own: a guessed state is never
/// restored, nor a chat forked anywhere but exactly before the turn.
pub async fn revert<L: AgentLauncher>(
    state: &AppState,
    launcher: &L,
    session_id: &str,
    turn_id: &str,
    files: bool,
    chat: bool,
) -> Result<RevertOutcome, ZwiimError> {
    if !files && !chat {
        return Err(ZwiimError::invalid_input(
            "Choose what to revert: files, chat or both",
        ));
    }
    let row = resolve(&state.db.conn, session_id).await?;
    let lock = session_lock(session_id);
    let _guard = lock.lock().await;
    let turns = history(state, session_id).await?;
    revert_locked(
        &state.db,
        &state.connection_manager,
        launcher,
        &row,
        &turns,
        turn_id,
        files,
        chat,
    )
    .await
}

/// [`revert`] under the session lock, with the session's transcript.
#[allow(clippy::too_many_arguments)]
async fn revert_locked<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    row: &conversation::Model,
    turns: &[MessageTurn],
    turn_id: &str,
    files: bool,
    chat: bool,
) -> Result<RevertOutcome, ZwiimError> {
    let session_id = row.session_id.as_deref().ok_or_else(ZwiimError::internal)?;
    // Same order as a send: session lock, then the workspace.
    let _exclusive = crate::workspace::enter_exclusive(row.folder_id).await;
    let folder = crate::db::entities::folder::Entity::find_by_id(row.folder_id)
        .one(&db.conn)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ZwiimError::not_found("Workspace not found"))?;
    let own = connection_for(connections, row).await;
    if let Some(connection_id) = &own {
        if let Some(state) = connections.get_state(connection_id).await {
            if is_busy(&*state.read().await) {
                return Err(ZwiimError::turn_in_progress());
            }
        }
    }
    let (busy, session_ids) =
        crate::workspace::busy_agents(&db.conn, connections, &folder, own.as_deref()).await?;
    if busy > 0 {
        return Err(ZwiimError::workspace_busy(busy, 0, session_ids));
    }
    let Some(idx) = turns
        .iter()
        .position(|t| t.id == turn_id && matches!(t.role, TurnRole::User))
    else {
        return Err(ZwiimError::checkpoint_unavailable());
    };
    // Everything that can refuse the revert does so before anything changes.
    if folder.parent_id.is_none() {
        return Err(ZwiimError::root_workspace());
    }
    let fork_point = match chat {
        true => chat_rewind_point(row, &folder, turns, idx)?,
        false => None,
    };
    let root = std::path::Path::new(&folder.path);
    let mut outcome = RevertOutcome::default();
    // Read before the restore and the fork: afterwards neither the transcript
    // nor the checkpoints are the ones the turn was matched against.
    let rewind = match fork_point {
        Some(point) => Some((
            point,
            crate::session_checkpoints::RewindCut {
                root: root.to_path_buf(),
                session_id: session_id.to_string(),
                from: crate::session_checkpoints::rewind_from(root, session_id, turns, turn_id)
                    .await,
            },
        )),
        None => None,
    };

    if files {
        let restored = restore_files(root, session_id, turns, turn_id).await?;
        outcome.files_restored = true;
        outcome.head_moved = restored.head_moved;
    }
    let Some((point, cut)) = rewind else {
        return Ok(outcome);
    };
    let from_turn = cut.from;
    if let Err(e) = rewind_chat(db, connections, launcher, row, &folder, point, cut).await {
        if !outcome.files_restored {
            return Err(e);
        }
        tracing::warn!(
            session_id = %session_id,
            code = %e.code,
            "[zwiim] session chat not rewound; its files were"
        );
        return Ok(outcome);
    }
    outcome.chat_rewound = true;
    tracing::info!(
        session_id = %session_id,
        from_turn,
        "[zwiim] session chat rewound"
    );
    Ok(outcome)
}

/// Where to fork to rewind `row`'s chat to before user turn `turns[idx]`: just
/// after the answer right before it. `None` when the agent cannot rewind its
/// chat at all, which is not an error.
fn chat_rewind_point(
    row: &conversation::Model,
    folder: &crate::db::entities::folder::Model,
    turns: &[MessageTurn],
    idx: usize,
) -> Result<Option<crate::acp::fork::ForkPoint>, ZwiimError> {
    let agent: AgentType =
        serde_json::from_value(serde_json::Value::String(row.agent_type.clone()))
            .map_err(|_| ZwiimError::internal())?;
    if !can_rewind_chat(agent) {
        return Ok(None);
    }
    // The rewind may have to resume the agent: never there, as for a send.
    crate::workspace::ensure_not_removed(folder)?;
    // The first turn has no answer before it to carry on from, and a turn
    // that follows an unanswered one would lose that one too.
    let previous = idx
        .checked_sub(1)
        .map(|i| &turns[i])
        .filter(|t| matches!(t.role, TurnRole::Assistant))
        .ok_or_else(ZwiimError::checkpoint_unavailable)?;
    crate::acp::fork::resolve_fork_point(turns, &previous.id, agent)
        .map(Some)
        .ok_or_else(ZwiimError::checkpoint_unavailable)
}

/// Restore the workspace at `root` to user turn `turn_id`'s start checkpoint.
async fn restore_files(
    root: &std::path::Path,
    session_id: &str,
    turns: &[MessageTurn],
    turn_id: &str,
) -> Result<crate::git_harness::checkpoint::Restored, ZwiimError> {
    let cp = crate::session_checkpoints::start_checkpoint_of(root, session_id, turns, turn_id)
        .await
        .ok_or_else(ZwiimError::checkpoint_unavailable)?;
    let mapped = |e: crate::git_harness::GitError| {
        use crate::git_harness::GitErrorCode;
        match e.code {
            GitErrorCode::Conflict => ZwiimError::repository_busy(e.detail.as_deref()),
            GitErrorCode::NotAGitRepository => ZwiimError::checkpoint_unavailable(),
            _ => {
                tracing::warn!(
                    code = e.code.as_str(),
                    message = %e.message,
                    detail = ?e.detail,
                    "[zwiim] revert failed"
                );
                ZwiimError::internal()
            }
        }
    };
    use crate::git_harness::checkpoint;
    checkpoint::abort_operation(root, &cp)
        .await
        .map_err(mapped)?;
    if cp.action.as_deref() == Some(crate::workspace::resolve::RESOLVE_ACTION) {
        let end = crate::session_checkpoints::end_checkpoint_of(root, &cp).await;
        let end_head = end.as_ref().and_then(|e| e.head.as_deref());
        checkpoint::unwind_unpushed(root, &cp, end_head)
            .await
            .map_err(mapped)?;
    }
    let restored = checkpoint::restore(root, &cp).await.map_err(mapped)?;
    tracing::info!(
        session_id = %session_id,
        turn = cp.key.ordinal,
        head_moved = restored.head_moved,
        "[zwiim] session files reverted"
    );
    Ok(restored)
}

/// Fork `row`'s agent session at `point`, resuming the agent first if it is
/// not live, and re-point the session at the fork, dropping the checkpoints
/// `cut` names. The history before the rewind stays on a row of its own,
/// deleted as it is written so no client ever lists it. Once the agent has
/// forked, all of this completes even if the caller goes away (see
/// [`ConnectionManager::fork_session_rewind`]), and the session's subscribers
/// get a fresh snapshot.
async fn rewind_chat<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    row: &conversation::Model,
    folder: &crate::db::entities::folder::Model,
    point: crate::acp::fork::ForkPoint,
    cut: crate::session_checkpoints::RewindCut,
) -> Result<(), ZwiimError> {
    let (connection_id, spawned) = ensure_live(db, connections, launcher, row, folder).await?;
    let forked = connections
        .fork_session_rewind(
            db,
            &connection_id,
            Some(row.id),
            Some(row.folder_id),
            point,
            cut,
        )
        .await;
    let Err(e) = forked else {
        return Ok(());
    };
    if spawned {
        let _ = connections.disconnect(&connection_id).await;
    }
    tracing::warn!(
        session_id = ?row.session_id,
        error = %e,
        "[zwiim] session chat rewind failed"
    );
    Err(match e {
        // The agent no longer has the answer to fork after.
        AcpError::ForkPointRejected(_) => ZwiimError::checkpoint_unavailable(),
        e => e.into(),
    })
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

/// What answering a request led to, beyond the answer itself.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RespondOutcome {
    /// The session a plan was handed off to.
    pub handed_off_session_id: Option<String>,
}

/// Answer the pending request `request_id` of session `session_id`. The
/// request must be the one currently pending on that session's agent.
///
/// A plan answer does more than reply to the agent:
/// - `approve` moves the agent to unattended (Auto) where it offers it, else
///   to the session's normal mode;
/// - `request_changes` with feedback waits for the rejected turn to end, then
///   sends the feedback as the next user message;
/// - `hand_off` starts a new session in the same workspace, seeded with the
///   plan, then stops this one. When the new session cannot start, nothing
///   is answered and the plan stays pending.
pub async fn respond<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    session_id: &str,
    request_id: &str,
    answer: RequestAnswer,
) -> Result<RespondOutcome, ZwiimError> {
    let connection_id = live_connection_id(&db.conn, connections, session_id)
        .await?
        .ok_or_else(ZwiimError::session_not_live)?;
    let state = connections
        .get_state(&connection_id)
        .await
        .ok_or_else(ZwiimError::session_not_live)?;
    let (pending, agent_type, plan_exit_agent, plan_markdown) = {
        let s = state.read().await;
        let plan_exit_agent = s
            .pending_permission
            .as_ref()
            .filter(|p| p.tool_call.get("kind").and_then(|k| k.as_str()) == Some("switch_mode"))
            .map(|_| s.agent_type);
        let plan = s
            .pending_plan_approval
            .as_ref()
            .filter(|p| p.approval_id == request_id);
        let pending = match &answer {
            RequestAnswer::Permission { .. } => s
                .pending_permission
                .as_ref()
                .is_some_and(|p| p.request_id == request_id),
            RequestAnswer::Question { .. } => s
                .pending_question
                .as_ref()
                .is_some_and(|q| q.question_id == request_id),
            RequestAnswer::PlanApproval { .. } => plan.is_some(),
        };
        let plan_markdown = plan.map(|p| p.plan_markdown.clone()).unwrap_or_default();
        (pending, s.agent_type, plan_exit_agent, plan_markdown)
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
            let answer = PlanApprovalAnswer { decision, feedback };
            match decision {
                PlanApprovalDecision::Approve => {
                    connections
                        .answer_plan_approval(&connection_id, request_id, answer)
                        .await?;
                    crate::session_options::sync_plan_approve(
                        &db.conn,
                        connections,
                        &connection_id,
                        session_id,
                    )
                    .await;
                }
                PlanApprovalDecision::RequestChanges => {
                    let feedback = answer.normalized_feedback();
                    connections
                        .answer_plan_approval(&connection_id, request_id, answer)
                        .await?;
                    if !feedback.is_empty() {
                        if !wait_turn_end(&state).await {
                            let _ = connections.cancel(&db.conn, &connection_id).await;
                            wait_turn_end(&state).await;
                        }
                        send(
                            db,
                            connections,
                            launcher,
                            session_id,
                            vec![PromptInputBlock::Text { text: feedback }],
                            None,
                        )
                        .await?;
                    }
                }
                PlanApprovalDecision::Abandon => {
                    connections
                        .answer_plan_approval(&connection_id, request_id, answer)
                        .await?;
                }
                PlanApprovalDecision::HandOff => {
                    return hand_off(
                        db,
                        connections,
                        launcher,
                        session_id,
                        &connection_id,
                        request_id,
                        agent_type,
                        plan_markdown,
                        answer,
                    )
                    .await;
                }
            }
        }
    }
    Ok(RespondOutcome::default())
}

/// Hand a plan off: start a new session in the same workspace with the plan
/// as its first prompt, in unattended (Auto) where the agent offers it, then
/// decline the plan here and stop this session.
#[allow(clippy::too_many_arguments)]
async fn hand_off<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    session_id: &str,
    connection_id: &str,
    request_id: &str,
    agent_type: AgentType,
    plan_markdown: String,
    answer: PlanApprovalAnswer,
) -> Result<RespondOutcome, ZwiimError> {
    let row = resolve(&db.conn, session_id).await?;
    let workspace_id = crate::workspace::workspace_id_of(&db.conn, row.folder_id)
        .await?
        .ok_or_else(|| ZwiimError::not_found("Session has no workspace"))?;
    let unattended = match connections.get_state(connection_id).await {
        Some(state) => {
            let options = state.read().await.config_options.clone().unwrap_or_default();
            crate::session_options::project(&options)
                .mode
                .is_some_and(|m| m.available.contains(&Mode::Unattended))
        }
        None => false,
    };
    let options = OptionsRequest {
        mode: unattended.then_some(Mode::Unattended),
        ..OptionsRequest::default()
    };
    let new_session_id = start(
        db,
        connections,
        launcher,
        &workspace_id,
        agent_type,
        vec![PromptInputBlock::Text {
            text: format!("Implement the following plan:\n\n{plan_markdown}"),
        }],
        None,
        &options,
    )
    .await?;
    if let Err(e) = connections
        .answer_plan_approval(connection_id, request_id, answer)
        .await
    {
        tracing::warn!("[session] failed to answer the handed-off plan: {e}");
    }
    if let Err(e) = stop(db, connections, session_id).await {
        tracing::warn!(
            "[session] failed to stop the handed-off session: {}",
            e.message
        );
    }
    Ok(RespondOutcome {
        handed_off_session_id: Some(new_session_id),
    })
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
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    /// A connection and the `refs/zwiim` refs present when its prompt arrived.
    type RefsAtPrompt = (String, Vec<String>);
    /// A connection and the blocks of a prompt it received.
    type BlocksAtPrompt = (String, Vec<PromptInputBlock>);
    type ForkAsked = (String, Option<crate::acp::fork::ForkPoint>);

    /// Launches in-process fake agents: a live command channel whose commands
    /// are recorded, and an agent-native session id like a real ACP handshake.
    pub(crate) struct FakeLauncher {
        pub connections: ConnectionManager,
        pub launches: AtomicUsize,
        pub resumed: std::sync::Mutex<Vec<Option<String>>>,
        /// The directory each agent was launched in, in launch order.
        pub working_dirs: std::sync::Mutex<Vec<String>>,
        pub commands: Arc<std::sync::Mutex<Vec<(String, &'static str)>>>,
        /// The text of every prompt, per connection, in order.
        pub prompts: Arc<std::sync::Mutex<Vec<(String, String)>>>,
        /// Every prompt's blocks as the agent received them, per connection.
        pub prompt_blocks: Arc<std::sync::Mutex<Vec<BlocksAtPrompt>>>,
        /// Every `SetConfigOption` as `(connection, config_id, value)`.
        pub config_sets: Arc<std::sync::Mutex<Vec<(String, String, String)>>>,
        /// The `refs/zwiim` refs of the agent's repository at the moment each
        /// prompt reached it, per connection, in order.
        pub refs_at_prompt: Arc<std::sync::Mutex<Vec<RefsAtPrompt>>>,
        /// Every fork asked of an agent, per connection, with where to.
        pub forks: Arc<std::sync::Mutex<Vec<ForkAsked>>>,
        /// Agents refuse to fork.
        pub fork_fails: Arc<std::sync::atomic::AtomicBool>,
        /// Agents refuse to fork at the point asked, as one that no longer
        /// has that message does.
        pub fork_point_rejected: Arc<std::sync::atomic::AtomicBool>,
        /// When set, an agent answers a fork only once this is notified.
        pub fork_gate: Arc<std::sync::Mutex<Option<Arc<tokio::sync::Notify>>>>,
        /// No agent starts.
        pub launch_fails: AtomicBool,
        /// What agents advertise as soon as they start.
        pub advertises: std::sync::Mutex<Option<Vec<crate::acp::types::SessionConfigOptionInfo>>>,
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
                prompts: Default::default(),
                prompt_blocks: Default::default(),
                config_sets: Default::default(),
                refs_at_prompt: Default::default(),
                forks: Default::default(),
                fork_fails: Default::default(),
                fork_point_rejected: Default::default(),
                fork_gate: Default::default(),
                launch_fails: Default::default(),
                advertises: Default::default(),
                delay: Duration::ZERO,
            }
        }

        pub(crate) fn prompts_for(&self, conn: &str) -> Vec<String> {
            self.prompts
                .lock()
                .unwrap()
                .iter()
                .filter(|(c, _)| c == conn)
                .map(|(_, t)| t.clone())
                .collect()
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
            _folder_id: i32,
            intent: LaunchIntent,
            _preferred: BTreeMap<String, String>,
        ) -> Result<String, ZwiimError> {
            tokio::time::sleep(self.delay).await;
            if self.launch_fails.load(Ordering::SeqCst) {
                return Err(ZwiimError::internal());
            }
            let resume_external_id = intent.resume_id().map(str::to_string);
            let n = self.launches.fetch_add(1, Ordering::SeqCst) + 1;
            let id = format!("fake-conn-{n}");
            self.working_dirs.lock().unwrap().push(working_dir.clone());
            let mut rx = self
                .connections
                .insert_test_connection_live(
                    &id,
                    agent_type,
                    Some(working_dir.clone().into()),
                    EventEmitter::Noop,
                )
                .await;
            let native = resume_external_id
                .clone()
                .unwrap_or_else(|| format!("native-{n}"));
            {
                let state = self.connections.get_state(&id).await.expect("state");
                let mut state = state.write().await;
                state.external_id = Some(native);
                if let Some(options) = self.advertises.lock().unwrap().clone() {
                    state.config_options = Some(options);
                    state.selectors_ready = true;
                }
            }
            self.resumed.lock().unwrap().push(resume_external_id);
            let log = self.commands.clone();
            let prompts = self.prompts.clone();
            let prompt_blocks = self.prompt_blocks.clone();
            let config_sets = self.config_sets.clone();
            let refs_at_prompt = self.refs_at_prompt.clone();
            let forks = self.forks.clone();
            let fork_fails = self.fork_fails.clone();
            let fork_point_rejected = self.fork_point_rejected.clone();
            let fork_gate = self.fork_gate.clone();
            let connections = self.connections.clone_ref();
            let conn = id.clone();
            tokio::spawn(async move {
                while let Some(cmd) = rx.recv().await {
                    let kind = match cmd {
                        ConnectionCommand::Prompt {
                            blocks, notices, ..
                        } => {
                            if Path::new(&working_dir).join(".git").exists() {
                                refs_at_prompt
                                    .lock()
                                    .unwrap()
                                    .push((conn.clone(), zwiim_refs(Path::new(&working_dir))));
                            }
                            // The text as the agent gets it, notices included.
                            let text = crate::harness::compose_turn(blocks.clone(), &notices)
                                .iter()
                                .filter_map(|b| match b {
                                    PromptInputBlock::Text { text } => Some(text.as_str()),
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            prompts.lock().unwrap().push((conn.clone(), text));
                            prompt_blocks.lock().unwrap().push((conn.clone(), blocks));
                            "prompt"
                        }
                        ConnectionCommand::SetConfigOption {
                            config_id,
                            value_id,
                        } => {
                            config_sets
                                .lock()
                                .unwrap()
                                .push((conn.clone(), config_id.clone(), value_id.clone()));
                            // Like an agent: it holds the value from here on.
                            if let Some(state) = connections.get_state(&conn).await {
                                let mut state = state.write().await;
                                let option = state
                                    .config_options
                                    .iter_mut()
                                    .flatten()
                                    .find(|o| o.id == config_id);
                                if let Some(crate::acp::types::SessionConfigKindInfo::Select(select)) =
                                    option.map(|o| &mut o.kind)
                                {
                                    select.current_value = value_id;
                                }
                            }
                            "other"
                        }
                        ConnectionCommand::Cancel => "cancel",
                        ConnectionCommand::RespondPermission { .. } => "respond_permission",
                        ConnectionCommand::Disconnect => "disconnect",
                        // Like an agent's fork: the connection carries on in
                        // the forked agent session.
                        ConnectionCommand::Fork { fork_point, reply } => {
                            let n = {
                                let mut forks = forks.lock().unwrap();
                                forks.push((conn.clone(), fork_point));
                                forks.len()
                            };
                            let gate = fork_gate.lock().unwrap().clone();
                            if let Some(gate) = gate {
                                gate.notified().await;
                            }
                            let result = if fork_fails.load(Ordering::SeqCst) {
                                Err(crate::acp::error::AcpError::protocol(
                                    "fork refused".to_string(),
                                ))
                            } else if fork_point_rejected.load(Ordering::SeqCst) {
                                Err(crate::acp::error::AcpError::ForkPointRejected(
                                    "no such message".to_string(),
                                ))
                            } else {
                                let state = connections.get_state(&conn).await.expect("state");
                                let mut state = state.write().await;
                                let original = state.external_id.clone().unwrap_or_default();
                                let forked = format!("native-fork-{n}");
                                state.external_id = Some(forked.clone());
                                Ok(crate::acp::types::ForkProtocolResult {
                                    forked_session_id: forked,
                                    original_session_id: original,
                                })
                            };
                            let _ = reply.send(result);
                            "fork"
                        }
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

    /// A Git workspace: a committed repository registered as a worktree.
    pub(crate) async fn git_workspace(db: &AppDatabase) -> (String, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("zwiim-orch-git-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "commit.gpgsign", "false"],
        ] {
            fixture_git(&dir, args);
        }
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        fixture_git(&dir, &["add", "-A"]);
        fixture_git(&dir, &["commit", "-q", "-m", "init"]);
        let folder = seed_worktree_folder(db, dir.to_str().unwrap()).await;
        let wid = crate::workspace::workspace_id_of(&db.conn, folder)
            .await
            .unwrap()
            .expect("workspace_id");
        (wid, dir)
    }

    pub(crate) fn fixture_git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Every `refs/zwiim` ref in `dir`'s repository, sorted.
    pub(crate) fn zwiim_refs(dir: &Path) -> Vec<String> {
        let mut refs: Vec<String> =
            fixture_git(dir, &["for-each-ref", "--format=%(refname)", "refs/zwiim"])
                .lines()
                .map(str::to_string)
                .collect();
        refs.sort();
        refs
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

    pub(crate) async fn settle() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    pub(crate) fn checkpoint_ref(sid: &str, turn: u32, phase: &str) -> String {
        format!("refs/zwiim/checkpoints/{sid}/{turn}/{phase}")
    }

    /// End `conn`'s turn with `stop_reason` and hand the turn-end event to
    /// the checkpoint tracker, as the Host's event bus does.
    pub(crate) async fn end_turn_observed(connections: &ConnectionManager, conn: &str, stop_reason: &str) {
        let (state, emitter) = connections.get_state_and_emitter(conn).await.unwrap();
        let payload = AcpEvent::TurnComplete {
            session_id: "native".into(),
            stop_reason: stop_reason.into(),
            agent_type: "codex".into(),
        };
        emit_with_state(&state, &emitter, payload.clone()).await;
        let seq = state.read().await.event_seq;
        connections
            .turn_checkpoints
            .observe(&crate::acp::types::EventEnvelope {
                seq,
                connection_id: conn.to_string(),
                payload,
            });
    }

    async fn wait_for_ref(dir: &Path, name: &str) -> bool {
        for _ in 0..250 {
            if zwiim_refs(dir).iter().any(|r| r == name) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    #[tokio::test]
    async fn start_checkpoint_precedes_prompt() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());

        let sid = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::Codex,
            text("first"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        settle().await;
        end_turn_observed(&mgr, "fake-conn-1", "end_turn").await;
        assert!(wait_for_ref(&dir, &checkpoint_ref(&sid, 1, "end")).await);
        send(&db, &mgr, &launcher, &sid, text("second"), None)
            .await
            .unwrap();
        settle().await;

        // Each prompt reached the agent only after its own start ref existed.
        let seen = launcher.refs_at_prompt.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_eq!(seen[0].1, vec![checkpoint_ref(&sid, 1, "start")]);
        assert_eq!(
            seen[1].1,
            vec![
                checkpoint_ref(&sid, 1, "end"),
                checkpoint_ref(&sid, 1, "start"),
                checkpoint_ref(&sid, 2, "start"),
            ]
        );
        let listed = crate::git_harness::checkpoint::list(&dir, &sid)
            .await
            .unwrap();
        assert_eq!(
            listed[0].prompt_fingerprint.as_deref(),
            Some(crate::acp::fork::fingerprint_agent_message("first").as_str())
        );
        assert_eq!(
            listed.last().unwrap().prompt_fingerprint.as_deref(),
            Some(crate::acp::fork::fingerprint_agent_message("second").as_str())
        );
        // A checkpoint never touches the user's branch or files.
        assert_eq!(fixture_git(&dir, &["status", "--porcelain"]), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn turn_is_admitted_while_its_start_checkpoint_is_saved() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::Codex,
            text("first"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        settle().await;
        end_turn_observed(&mgr, "fake-conn-1", "end_turn").await;
        assert!(wait_for_ref(&dir, &checkpoint_ref(&sid, 1, "end")).await);
        let in_flight = || async {
            mgr.get_state("fake-conn-1")
                .await
                .unwrap()
                .read()
                .await
                .turn_in_flight
        };
        assert!(!in_flight().await);

        // While the start checkpoint is being written the turn already holds
        // the gate, and the prompt has not reached the agent.
        let (reached, resume) = mgr.turn_checkpoints.pause_before_save();
        let probe = async {
            reached.notified().await;
            let seen = (in_flight().await, launcher.prompts.lock().unwrap().len());
            resume.notify_one();
            seen
        };
        let (sent, (gated, prompts)) = tokio::join!(
            send(&db, &mgr, &launcher, &sid, text("second"), None),
            probe
        );
        sent.unwrap();
        assert!(gated, "turn_in_flight is set during the start checkpoint");
        assert_eq!(prompts, 1, "the second prompt waits for its checkpoint");
        end_turn_observed(&mgr, "fake-conn-1", "end_turn").await;
        assert!(wait_for_ref(&dir, &checkpoint_ref(&sid, 2, "end")).await);

        // A prompt abandoned during its checkpoint gives the gate back.
        let (reached, _resume) = mgr.turn_checkpoints.pause_before_save();
        tokio::select! {
            _ = send(&db, &mgr, &launcher, &sid, text("third"), None) => {
                panic!("the paused send cannot finish")
            }
            _ = reached.notified() => {}
        }
        let mut reopened = false;
        for _ in 0..50 {
            if !in_flight().await {
                reopened = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(reopened, "a dropped prompt must not leave the gate set");
        assert_eq!(launcher.prompts.lock().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn end_checkpoint_on_cancel_and_stop() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());

        let sid = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::Codex,
            text("one"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        settle().await;
        // The agent wrote a file, then the user cancelled.
        std::fs::write(dir.join("b.txt"), "two\n").unwrap();
        end_turn_observed(&mgr, "fake-conn-1", "cancelled").await;
        assert!(
            wait_for_ref(&dir, &checkpoint_ref(&sid, 1, "end")).await,
            "cancel takes the end checkpoint"
        );

        send(&db, &mgr, &launcher, &sid, text("two"), None)
            .await
            .unwrap();
        settle().await;
        assert!(zwiim_refs(&dir).contains(&checkpoint_ref(&sid, 2, "start")));
        // Stop with the turn still running: the end checkpoint exists by the
        // time stop returns.
        stop(&db, &mgr, &sid).await.unwrap();
        assert!(
            zwiim_refs(&dir).contains(&checkpoint_ref(&sid, 2, "end")),
            "{:?}",
            zwiim_refs(&dir)
        );

        let listed = crate::git_harness::checkpoint::list(&dir, &sid)
            .await
            .unwrap();
        let end1 = listed
            .iter()
            .find(|c| c.key.ordinal == 1 && c.phase == crate::git_harness::checkpoint::Phase::End)
            .unwrap();
        let start1 = &listed[0];
        assert_ne!(
            start1.worktree_tree, end1.worktree_tree,
            "b.txt is in turn 1's end"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn delete_removes_the_sessions_checkpoints() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());

        let sid = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::Codex,
            text("one"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        settle().await;
        end_turn_observed(&mgr, "fake-conn-1", "end_turn").await;
        assert!(wait_for_ref(&dir, &checkpoint_ref(&sid, 1, "end")).await);
        delete(&db, &EventEmitter::Noop, &mgr, &sid).await.unwrap();
        assert_eq!(zwiim_refs(&dir), Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&dir);
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
                _folder_id: i32,
                _intent: LaunchIntent,
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
            &launcher,
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
            &launcher,
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
            &launcher,
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
            &launcher,
            &sid,
            &p.approval_id,
            RequestAnswer::PlanApproval {
                decision: PlanApprovalDecision::Abandon,
                feedback: None,
            },
        )
        .await
        .unwrap();
        let answer = p.answer_rx.await.unwrap();
        assert_eq!(answer.decision, PlanApprovalDecision::Abandon);

        // No live agent: nothing to answer.
        mgr.disconnect(conn).await.unwrap();
        let err = respond(
            &db,
            &mgr,
            &launcher,
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

    /// A launcher whose agents never start.
    struct FailingLauncher;

    impl AgentLauncher for FailingLauncher {
        async fn launch(
            &self,
            _agent_type: AgentType,
            _working_dir: String,
            _folder_id: i32,
            _intent: LaunchIntent,
            _preferred: BTreeMap<String, String>,
        ) -> Result<String, ZwiimError> {
            Err(ZwiimError::session_not_live())
        }
    }

    fn plan_answer(decision: PlanApprovalDecision, feedback: Option<&str>) -> RequestAnswer {
        RequestAnswer::PlanApproval {
            decision,
            feedback: feedback.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn plan_adjustments_follow_the_ended_turn_as_the_next_message() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = started(&db, &mgr, &launcher, &wid).await;
        let conn = "fake-conn-1";

        // Like Claude: rejecting the plan interrupts, ending the turn.
        let p = mgr
            .register_plan_approval(conn, "tc-plan".into(), "# Plan".into())
            .await
            .expect("registered");
        let ender = {
            let mgr = mgr.clone_ref();
            tokio::spawn(async move {
                let answer = p.answer_rx.await.unwrap();
                finish_turn(&mgr, conn).await;
                answer
            })
        };
        let out = respond(
            &db,
            &mgr,
            &launcher,
            &sid,
            &p.approval_id,
            plan_answer(PlanApprovalDecision::RequestChanges, Some("  smaller  ")),
        )
        .await
        .unwrap();
        assert_eq!(out.handed_off_session_id, None);
        let answer = ender.await.unwrap();
        assert_eq!(answer.decision, PlanApprovalDecision::RequestChanges);
        settle().await;
        assert_eq!(launcher.prompts_for(conn), ["hi", "smaller"]);
        assert_eq!(launcher.commands_for(conn), ["prompt", "prompt"]);
    }

    #[tokio::test]
    async fn plan_adjustments_cancel_a_turn_that_does_not_end() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = started(&db, &mgr, &launcher, &wid).await;
        let conn = "fake-conn-1";
        let p = mgr
            .register_plan_approval(conn, "tc-plan".into(), "# Plan".into())
            .await
            .expect("registered");
        ends_turn_on_cancel(&mgr, &launcher, conn);
        respond(
            &db,
            &mgr,
            &launcher,
            &sid,
            &p.approval_id,
            plan_answer(PlanApprovalDecision::RequestChanges, Some("smaller")),
        )
        .await
        .unwrap();
        settle().await;
        assert_eq!(launcher.commands_for(conn), ["prompt", "cancel", "prompt"]);
        assert_eq!(launcher.prompts_for(conn), ["hi", "smaller"]);
    }

    #[tokio::test]
    async fn plan_changes_without_feedback_send_nothing() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = started(&db, &mgr, &launcher, &wid).await;
        let conn = "fake-conn-1";
        let p = mgr
            .register_plan_approval(conn, "tc-plan".into(), "# Plan".into())
            .await
            .expect("registered");
        respond(
            &db,
            &mgr,
            &launcher,
            &sid,
            &p.approval_id,
            plan_answer(PlanApprovalDecision::RequestChanges, Some("   ")),
        )
        .await
        .unwrap();
        assert_eq!(
            p.answer_rx.await.unwrap().decision,
            PlanApprovalDecision::RequestChanges
        );
        settle().await;
        assert_eq!(launcher.commands_for(conn), ["prompt"]);
    }

    #[tokio::test]
    async fn plan_hand_off_starts_a_new_session_and_stops_the_old_one() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = started(&db, &mgr, &launcher, &wid).await;
        let conn = "fake-conn-1";
        let p = mgr
            .register_plan_approval(conn, "tc-plan".into(), "# Plan\nDo X".into())
            .await
            .expect("registered");
        ends_turn_on_cancel(&mgr, &launcher, conn);
        let out = respond(
            &db,
            &mgr,
            &launcher,
            &sid,
            &p.approval_id,
            plan_answer(PlanApprovalDecision::HandOff, None),
        )
        .await
        .unwrap();
        let new_sid = out.handed_off_session_id.expect("handed off");
        assert_ne!(new_sid, sid);
        assert_eq!(
            p.answer_rx.await.unwrap().decision,
            PlanApprovalDecision::HandOff
        );

        let new_row = resolve(&db.conn, &new_sid).await.unwrap();
        let old_row = resolve(&db.conn, &sid).await.unwrap();
        assert_eq!(new_row.folder_id, old_row.folder_id, "same workspace");
        assert_eq!(new_row.agent_type, old_row.agent_type);
        settle().await;
        assert_eq!(
            launcher.prompts_for("fake-conn-2"),
            ["Implement the following plan:\n\n# Plan\nDo X"]
        );
        assert!(
            launcher.config_sets.lock().unwrap().is_empty(),
            "default options"
        );
        assert!(launcher.commands_for(conn).contains(&"disconnect"));
        assert_eq!(
            live_connection_id(&db.conn, &mgr, &sid).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn a_failed_hand_off_leaves_the_plan_card_pending() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = started(&db, &mgr, &launcher, &wid).await;
        let conn = "fake-conn-1";
        let p = mgr
            .register_plan_approval(conn, "tc-plan".into(), "# Plan".into())
            .await
            .expect("registered");
        let err = respond(
            &db,
            &mgr,
            &FailingLauncher,
            &sid,
            &p.approval_id,
            plan_answer(PlanApprovalDecision::HandOff, None),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "session_not_live");
        let state = mgr.get_state(conn).await.unwrap();
        assert!(state.read().await.pending_plan_approval.is_some());
        settle().await;
        assert!(!launcher.commands_for(conn).contains(&"disconnect"));
    }

    #[tokio::test]
    async fn plan_hand_off_starts_the_new_session_in_auto() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = started(&db, &mgr, &launcher, &wid).await;
        let conn = "fake-conn-1";
        mgr.get_state(conn)
            .await
            .unwrap()
            .write()
            .await
            .config_options = Some(crate::session_options::tests::claude("plan"));
        crate::db::service::app_metadata_service::upsert_value(
            &db.conn,
            "zwiim.agent_options:claude_code",
            &serde_json::to_string(&crate::session_options::tests::claude("default")).unwrap(),
        )
        .await
        .unwrap();
        let p = mgr
            .register_plan_approval(conn, "tc-plan".into(), "# Plan".into())
            .await
            .expect("registered");
        ends_turn_on_cancel(&mgr, &launcher, conn);
        let out = respond(
            &db,
            &mgr,
            &launcher,
            &sid,
            &p.approval_id,
            plan_answer(PlanApprovalDecision::HandOff, None),
        )
        .await
        .unwrap();
        let new_sid = out.handed_off_session_id.expect("handed off");
        assert_eq!(
            crate::session_options::preferred_values(&db.conn, &new_sid)
                .await
                .get("mode")
                .map(String::as_str),
            Some("bypassPermissions")
        );
    }

    /// Answer a plan card with Approve on a session whose agent reports
    /// `options`, then return the mode sets that followed.
    async fn approved_mode_sets(
        options: Vec<crate::acp::types::SessionConfigOptionInfo>,
    ) -> Vec<(String, String, String)> {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = started(&db, &mgr, &launcher, &wid).await;
        let conn = "fake-conn-1";
        mgr.get_state(conn).await.unwrap().write().await.config_options = Some(options);
        crate::db::service::app_metadata_service::upsert_value(
            &db.conn,
            &format!("zwiim.session_options:{sid}"),
            r#"{"values":{},"normal_mode":"acceptEdits"}"#,
        )
        .await
        .unwrap();
        let p = mgr
            .register_plan_approval(conn, "tc-plan".into(), "# Plan".into())
            .await
            .expect("registered");
        respond(
            &db,
            &mgr,
            &launcher,
            &sid,
            &p.approval_id,
            plan_answer(PlanApprovalDecision::Approve, None),
        )
        .await
        .unwrap();
        assert_eq!(
            p.answer_rx.await.unwrap().decision,
            PlanApprovalDecision::Approve
        );
        settle().await;
        let sets = launcher.config_sets.lock().unwrap().clone();
        sets
    }

    #[tokio::test]
    async fn plan_approve_moves_the_agent_to_auto() {
        let set = |value: &str| ("fake-conn-1".to_string(), "mode".to_string(), value.to_string());
        assert_eq!(
            approved_mode_sets(crate::session_options::tests::claude("plan")).await,
            [set("bypassPermissions")]
        );
        assert_eq!(
            approved_mode_sets(crate::session_options::tests::codex("workspace-write", "default"))
                .await,
            [set("agent-full-access")]
        );
    }

    #[tokio::test]
    async fn plan_approve_without_auto_returns_to_the_remembered_normal_mode() {
        let mut options = crate::session_options::tests::claude("plan");
        crate::session_options::tests::drop_full_access(&mut options);
        assert_eq!(
            approved_mode_sets(options).await,
            [(
                "fake-conn-1".to_string(),
                "mode".to_string(),
                "acceptEdits".to_string()
            )]
        );
    }

    // ── revert ──────────────────────────────────────────────────────────

    /// The transcript of `prompts`, each answered: user turns `turn-0`,
    /// `turn-2`, ...
    pub(crate) fn transcript(prompts: &[&str]) -> Vec<MessageTurn> {
        use crate::models::message::{ContentBlock, TurnRole};
        let turn = |n: usize, role: TurnRole, text: &str| MessageTurn {
            id: format!("turn-{n}"),
            role,
            blocks: vec![ContentBlock::Text { text: text.into() }],
            timestamp: chrono::Utc::now(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
            agent_message_id: None,
        };
        prompts
            .iter()
            .enumerate()
            .flat_map(|(k, p)| {
                [
                    turn(2 * k, TurnRole::User, p),
                    turn(2 * k + 1, TurnRole::Assistant, "done"),
                ]
            })
            .collect()
    }

    /// A Codex session in the Git workspace `dir` that ran `prompts` to the
    /// end. During turn `k` the agent writes `a.txt` as `after <prompt> <k>`.
    pub(crate) async fn ran(
        db: &AppDatabase,
        mgr: &ConnectionManager,
        launcher: &FakeLauncher,
        wid: &str,
        dir: &Path,
        prompts: &[&str],
    ) -> String {
        ran_as(db, mgr, launcher, wid, dir, AgentType::Codex, prompts).await
    }

    /// [`ran`] with an `agent` session, on `launcher`'s first connection.
    pub(crate) async fn ran_as(
        db: &AppDatabase,
        mgr: &ConnectionManager,
        launcher: &FakeLauncher,
        wid: &str,
        dir: &Path,
        agent: AgentType,
        prompts: &[&str],
    ) -> String {
        let sid = start(
            db,
            mgr,
            launcher,
            wid,
            agent,
            text(prompts[0]),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        for (k, prompt) in prompts.iter().enumerate() {
            if k > 0 {
                send(db, mgr, launcher, &sid, text(prompt), None)
                    .await
                    .unwrap();
            }
            settle().await;
            std::fs::write(dir.join("a.txt"), format!("after {prompt} {k}\n")).unwrap();
            end_turn_observed(mgr, "fake-conn-1", "end_turn").await;
            assert!(wait_for_ref(dir, &checkpoint_ref(&sid, k as u32 + 1, "end")).await);
        }
        sid
    }

    pub(crate) async fn revert_turn(
        db: &AppDatabase,
        mgr: &ConnectionManager,
        sid: &str,
        turns: &[MessageTurn],
        turn_id: &str,
    ) -> Result<RevertOutcome, ZwiimError> {
        let launcher = FakeLauncher::new(mgr.clone_ref());
        revert_with(db, mgr, &launcher, sid, turns, turn_id, true, false).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn revert_with(
        db: &AppDatabase,
        mgr: &ConnectionManager,
        launcher: &FakeLauncher,
        sid: &str,
        turns: &[MessageTurn],
        turn_id: &str,
        files: bool,
        chat: bool,
    ) -> Result<RevertOutcome, ZwiimError> {
        let row = resolve(&db.conn, sid).await.unwrap();
        revert_locked(db, mgr, launcher, &row, turns, turn_id, files, chat).await
    }

    fn a_txt(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("a.txt")).unwrap()
    }

    fn revert_refs(dir: &Path, sid: &str) -> usize {
        let prefix = format!("refs/zwiim/checkpoints/{sid}/revert-");
        zwiim_refs(dir).iter().filter(|r| r.starts_with(&prefix)).count()
    }

    #[tokio::test]
    async fn revert_puts_back_the_files_a_turn_started_with() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = ran(&db, &mgr, &launcher, &wid, &dir, &["first", "second"]).await;
        std::fs::write(dir.join("new.txt"), "made by the turn\n").unwrap();
        let turns = transcript(&["first", "second"]);

        let out = revert_turn(&db, &mgr, &sid, &turns, "turn-2").await.unwrap();
        assert_eq!(
            out,
            RevertOutcome {
                files_restored: true,
                chat_rewound: false,
                head_moved: false,
            }
        );
        assert_eq!(a_txt(&dir), "after first 0\n");
        assert!(!dir.join("new.txt").exists());

        revert_turn(&db, &mgr, &sid, &turns, "turn-0").await.unwrap();
        assert_eq!(a_txt(&dir), "one\n");
        assert_eq!(fixture_git(&dir, &["status", "--porcelain"]), "");
        assert_eq!(revert_refs(&dir, &sid), 2, "each revert is recoverable");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn revert_unknown_turn_is_checkpoint_unavailable() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = ran(&db, &mgr, &launcher, &wid, &dir, &["first"]).await;
        let turns = transcript(&["first"]);

        for (turns, turn_id) in [
            (&turns, "turn-9"),
            // Not a user turn.
            (&turns, "turn-1"),
            // A prompt no checkpoint was saved for.
            (&transcript(&["something else"]), "turn-0"),
        ] {
            let err = revert_turn(&db, &mgr, &sid, turns, turn_id)
                .await
                .unwrap_err();
            assert_eq!(err.code, "checkpoint_unavailable", "{turn_id}");
        }
        assert_eq!(a_txt(&dir), "after first 0\n");
        assert_eq!(revert_refs(&dir, &sid), 0);

        // A workspace that is not a Git repository has no checkpoints.
        let plain = workspace(&db).await;
        let other = start(
            &db,
            &mgr,
            &launcher,
            &plain,
            AgentType::Codex,
            text("first"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        finish_turn(&mgr, "fake-conn-2").await;
        let err = revert_turn(&db, &mgr, &other, &turns, "turn-0")
            .await
            .unwrap_err();
        assert_eq!(err.code, "checkpoint_unavailable");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn revert_refused_during_turn() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = ran(&db, &mgr, &launcher, &wid, &dir, &["first"]).await;
        send(&db, &mgr, &launcher, &sid, text("second"), None)
            .await
            .unwrap();
        settle().await;
        std::fs::write(dir.join("a.txt"), "mid-turn\n").unwrap();

        let err = revert_turn(&db, &mgr, &sid, &transcript(&["first"]), "turn-0")
            .await
            .unwrap_err();
        assert_eq!(err.code, "turn_in_progress");
        assert_eq!(a_txt(&dir), "mid-turn\n");
        assert_eq!(revert_refs(&dir, &sid), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn revert_refused_when_busy() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = ran(&db, &mgr, &launcher, &wid, &dir, &["first"]).await;
        let turns = transcript(&["first"]);

        // The repository is mid-cherry-pick (a merge or rebase is aborted
        // instead: see revert_restores_over_merge_in_progress).
        let head = fixture_git(&dir, &["rev-parse", "HEAD"]);
        std::fs::write(dir.join(".git/CHERRY_PICK_HEAD"), head).unwrap();
        let err = revert_turn(&db, &mgr, &sid, &turns, "turn-0")
            .await
            .unwrap_err();
        assert_eq!(err.code, "workspace_busy");
        assert_eq!(err.details.as_ref().unwrap()["operation"], "cherry_pick");
        assert_eq!(a_txt(&dir), "after first 0\n");
        std::fs::remove_file(dir.join(".git/CHERRY_PICK_HEAD")).unwrap();

        // Another session's agent is working in the workspace.
        let other = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::Codex,
            text("other"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        settle().await;
        let err = revert_turn(&db, &mgr, &sid, &turns, "turn-0")
            .await
            .unwrap_err();
        assert_eq!(err.code, "workspace_busy");
        let details = err.details.unwrap();
        assert_eq!(details["live_sessions"], 1);
        assert_eq!(details["session_ids"], serde_json::json!([other]));
        assert_eq!(a_txt(&dir), "after first 0\n");
        assert_eq!(revert_refs(&dir, &sid), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn revert_restores_over_merge_in_progress() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = ran(&db, &mgr, &launcher, &wid, &dir, &["first"]).await;
        fixture_git(&dir, &["commit", "-q", "-am", "first"]);
        let start = fixture_git(&dir, &["rev-parse", "HEAD^{tree}"]);
        // A branch that changes a.txt the other way.
        fixture_git(&dir, &["checkout", "-q", "-b", "other", "HEAD~1"]);
        std::fs::write(dir.join("a.txt"), "theirs\n").unwrap();
        fixture_git(&dir, &["commit", "-q", "-am", "theirs"]);
        fixture_git(&dir, &["checkout", "-q", "-"]);

        // The second turn starts clean and ends mid-merge.
        send(&db, &mgr, &launcher, &sid, text("second"), None)
            .await
            .unwrap();
        settle().await;
        let merge = std::process::Command::new("git")
            .args(["merge", "-q", "other"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(!merge.status.success());
        // The end checkpoint is skipped: the worktree is mid-merge.
        end_turn_observed(&mgr, "fake-conn-1", "end_turn").await;
        settle().await;

        let turns = transcript(&["first", "second"]);
        let out = revert_turn(&db, &mgr, &sid, &turns, "turn-2")
            .await
            .unwrap();
        assert!(out.files_restored);
        assert!(!out.head_moved);
        assert!(!dir.join(".git/MERGE_HEAD").exists());
        assert_eq!(fixture_git(&dir, &["status", "--porcelain"]), "");
        assert_eq!(fixture_git(&dir, &["rev-parse", "HEAD^{tree}"]), start);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn revert_plain_turn_keeps_head() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = ran(&db, &mgr, &launcher, &wid, &dir, &["first"]).await;
        let before = fixture_git(&dir, &["rev-parse", "HEAD"]);
        // The turn committed its change.
        fixture_git(&dir, &["commit", "-q", "-am", "first"]);
        let after = fixture_git(&dir, &["rev-parse", "HEAD"]);

        let turns = transcript(&["first"]);
        let out = revert_turn(&db, &mgr, &sid, &turns, "turn-0")
            .await
            .unwrap();
        assert!(out.files_restored);
        assert!(out.head_moved);
        assert_eq!(fixture_git(&dir, &["rev-parse", "HEAD"]), after);
        assert_ne!(after, before);
        assert_eq!(a_txt(&dir), "one\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn history_marks_revertible_turns() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = ran(
            &db,
            &mgr,
            &launcher,
            &wid,
            &dir,
            &["first", "continue", "continue"],
        )
        .await;
        // The oldest turn predates checkpoints.
        let turns = transcript(&["old", "first", "continue", "continue"]);

        let revertible = crate::session_checkpoints::revertible_turns(&dir, &sid, &turns).await;
        let mut ids: Vec<&str> = revertible.iter().map(String::as_str).collect();
        ids.sort();
        assert_eq!(ids, ["turn-2", "turn-4", "turn-6"]);

        let projected = crate::protocol::events::project_history_with(&turns, None, &revertible);
        crate::protocol::events::tests::assert_no_internal_keys(&projected);
        let flags: Vec<(String, serde_json::Value)> = projected
            .as_array()
            .unwrap()
            .iter()
            .map(|t| (t["role"].as_str().unwrap().to_string(), t["revertible"].clone()))
            .collect();
        let user = |b: bool| ("user".to_string(), serde_json::Value::Bool(b));
        let agent = ("assistant".to_string(), serde_json::Value::Null);
        assert_eq!(
            flags,
            [
                user(false),
                agent.clone(),
                user(true),
                agent.clone(),
                user(true),
                agent.clone(),
                user(true),
                agent,
            ]
        );
        for t in projected.as_array().unwrap() {
            if t["role"] == "assistant" {
                assert!(t.get("revertible").is_none(), "{t}");
            }
        }

        // Each "continue" goes back to its own start.
        revert_turn(&db, &mgr, &sid, &turns, "turn-6").await.unwrap();
        assert_eq!(a_txt(&dir), "after continue 1\n");
        revert_turn(&db, &mgr, &sid, &turns, "turn-4").await.unwrap();
        assert_eq!(a_txt(&dir), "after first 0\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── chat rewind ─────────────────────────────────────────────────────

    /// Every ordinal checkpoint ref of `sid`, as `(turn, phase)`.
    fn turn_refs(dir: &Path, sid: &str) -> Vec<(u32, String)> {
        let prefix = format!("refs/zwiim/checkpoints/{sid}/");
        let mut refs: Vec<(u32, String)> = zwiim_refs(dir)
            .iter()
            .filter_map(|r| {
                let (turn, phase) = r.strip_prefix(&prefix)?.split_once('/')?;
                Some((turn.parse().ok()?, phase.to_string()))
            })
            .collect();
        refs.sort();
        refs
    }

    fn phases(turns: &[u32]) -> Vec<(u32, String)> {
        turns
            .iter()
            .flat_map(|&t| [(t, "end".to_string()), (t, "start".to_string())])
            .collect()
    }

    async fn row_of(db: &AppDatabase, sid: &str) -> conversation::Model {
        resolve(&db.conn, sid).await.unwrap()
    }

    /// The row that holds agent session `external_id`, deleted or not.
    async fn holder_of(db: &AppDatabase, external_id: &str) -> Option<conversation::Model> {
        use sea_orm::{ColumnTrait, QueryFilter};
        conversation::Entity::find()
            .filter(conversation::Column::ExternalId.eq(external_id))
            .one(&db.conn)
            .await
            .unwrap()
    }

    const FILES_ONLY: (bool, bool) = (true, false);
    const CHAT_ONLY: (bool, bool) = (false, true);
    const BOTH: (bool, bool) = (true, true);

    #[tokio::test]
    async fn revert_chat_forks_at_previous_assistant_turn() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let prompts = ["first", "second", "third"];
        let sid = ran(&db, &mgr, &launcher, &wid, &dir, &prompts).await;
        let turns = transcript(&prompts);

        let (files, chat) = CHAT_ONLY;
        let out = revert_with(&db, &mgr, &launcher, &sid, &turns, "turn-4", files, chat)
            .await
            .unwrap();
        assert_eq!(
            out,
            RevertOutcome {
                files_restored: false,
                chat_rewound: true,
                head_moved: false,
            }
        );
        // The agent answered "done" every turn: the fork names the second
        // answer, the one just before "third".
        let forks = launcher.forks.lock().unwrap().clone();
        assert_eq!(forks.len(), 1);
        assert_eq!(forks[0].0, "fake-conn-1");
        let point = forks[0].1.clone().expect("a fork point, never the tail");
        assert_eq!(
            point.message_fingerprint.as_deref(),
            Some(crate::acp::fork::fingerprint_agent_message("done").as_str())
        );
        assert_eq!(point.message_occurrence, Some(2));
        // Chat only: the files stay as the last turn left them.
        assert_eq!(a_txt(&dir), "after third 2\n");
        assert_eq!(revert_refs(&dir, &sid), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn revert_chat_keeps_session_id() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let prompts = ["first", "second"];
        let sid = ran(&db, &mgr, &launcher, &wid, &dir, &prompts).await;
        let before = row_of(&db, &sid).await;
        // The agent went idle: the rewind resumes it first.
        stop(&db, &mgr, &sid).await.unwrap();

        let (files, chat) = CHAT_ONLY;
        let out = revert_with(
            &db,
            &mgr,
            &launcher,
            &sid,
            &transcript(&prompts),
            "turn-2",
            files,
            chat,
        )
        .await
        .unwrap();
        assert!(out.chat_rewound);
        assert_eq!(
            launcher.resumed.lock().unwrap().last().cloned().flatten(),
            Some("native-1".to_string())
        );
        let after = row_of(&db, &sid).await;
        assert_eq!(after.id, before.id, "the same session, re-pointed");
        assert_eq!(after.external_id.as_deref(), Some("native-fork-1"));
        assert_eq!(after.title, before.title, "no fork marker on the session");
        assert_eq!(after.title_locked, before.title_locked);
        // It carries on: the next prompt goes to the rewound agent session.
        send(&db, &mgr, &launcher, &sid, text("again"), None)
            .await
            .unwrap();
        settle().await;
        assert_eq!(launcher.prompts_for("fake-conn-2"), ["again"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn revert_chat_hides_sibling() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let prompts = ["first", "second"];
        let sid = ran(&db, &mgr, &launcher, &wid, &dir, &prompts).await;

        let (files, chat) = CHAT_ONLY;
        revert_with(
            &db,
            &mgr,
            &launcher,
            &sid,
            &transcript(&prompts),
            "turn-2",
            files,
            chat,
        )
        .await
        .unwrap();
        // The pre-rewind conversation is kept, out of sight.
        let sibling = holder_of(&db, "native-1").await.expect("kept");
        assert!(sibling.deleted_at.is_some());
        let listed: Vec<String> = list_for_workspace(&db.conn, &mgr, &wid)
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.session_id)
            .collect();
        assert_eq!(listed, std::slice::from_ref(&sid));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn revert_chat_first_turn_refused() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let prompts = ["first", "second"];
        let sid = ran(&db, &mgr, &launcher, &wid, &dir, &prompts).await;
        let turns = transcript(&prompts);

        for (files, chat) in [CHAT_ONLY, BOTH] {
            let err = revert_with(&db, &mgr, &launcher, &sid, &turns, "turn-0", files, chat)
                .await
                .unwrap_err();
            assert_eq!(err.code, "checkpoint_unavailable", "files={files}");
        }
        assert!(launcher.forks.lock().unwrap().is_empty());
        assert_eq!(a_txt(&dir), "after second 1\n", "nothing was restored");
        assert_eq!(revert_refs(&dir, &sid), 0);
        // The files alone can still go back.
        let out = revert_with(&db, &mgr, &launcher, &sid, &turns, "turn-0", true, false)
            .await
            .unwrap();
        assert!(out.files_restored && !out.chat_rewound);
        assert_eq!(a_txt(&dir), "one\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn revert_chat_unsupported_agent_files_only() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let prompts = ["first", "second"];
        let sid = ran_as(&db, &mgr, &launcher, &wid, &dir, AgentType::Gemini, &prompts).await;
        let turns = transcript(&prompts);

        let (files, chat) = BOTH;
        let out = revert_with(&db, &mgr, &launcher, &sid, &turns, "turn-2", files, chat)
            .await
            .unwrap();
        assert_eq!(
            out,
            RevertOutcome {
                files_restored: true,
                chat_rewound: false,
                head_moved: false,
            }
        );
        assert_eq!(a_txt(&dir), "after first 0\n");
        let (files, chat) = CHAT_ONLY;
        let out = revert_with(&db, &mgr, &launcher, &sid, &turns, "turn-2", files, chat)
            .await
            .unwrap();
        assert_eq!(out, RevertOutcome::default());
        assert!(launcher.forks.lock().unwrap().is_empty());
        assert_eq!(row_of(&db, &sid).await.external_id.as_deref(), Some("native-1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn revert_chat_drops_the_checkpoints_of_rewound_turns() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let prompts = ["first", "second", "third"];
        let sid = ran(&db, &mgr, &launcher, &wid, &dir, &prompts).await;

        let (files, chat) = BOTH;
        let out = revert_with(
            &db,
            &mgr,
            &launcher,
            &sid,
            &transcript(&prompts),
            "turn-2",
            files,
            chat,
        )
        .await
        .unwrap();
        assert!(out.files_restored && out.chat_rewound);
        assert_eq!(a_txt(&dir), "after first 0\n");
        assert_eq!(turn_refs(&dir, &sid), phases(&[1]));
        assert_eq!(revert_refs(&dir, &sid), 1, "the restore stays recoverable");

        // The same prompt again: its checkpoint is its own, and a later
        // revert finds it.
        send(&db, &mgr, &launcher, &sid, text("second"), None)
            .await
            .unwrap();
        settle().await;
        std::fs::write(dir.join("a.txt"), "after second again\n").unwrap();
        end_turn_observed(&mgr, "fake-conn-1", "end_turn").await;
        assert!(wait_for_ref(&dir, &checkpoint_ref(&sid, 4, "end")).await);
        assert_eq!(turn_refs(&dir, &sid), phases(&[1, 4]));
        let turns = transcript(&["first", "second"]);
        let revertible = crate::session_checkpoints::revertible_turns(&dir, &sid, &turns).await;
        assert!(revertible.contains("turn-2"), "{revertible:?}");
        revert_turn(&db, &mgr, &sid, &turns, "turn-2").await.unwrap();
        assert_eq!(a_txt(&dir), "after first 0\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn revert_files_and_chat_reports_the_restore_when_the_rewind_fails() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let prompts = ["first", "second"];
        let sid = ran(&db, &mgr, &launcher, &wid, &dir, &prompts).await;
        let turns = transcript(&prompts);
        launcher.fork_fails.store(true, Ordering::SeqCst);

        let (files, chat) = BOTH;
        let out = revert_with(&db, &mgr, &launcher, &sid, &turns, "turn-2", files, chat)
            .await
            .unwrap();
        assert_eq!(
            out,
            RevertOutcome {
                files_restored: true,
                chat_rewound: false,
                head_moved: false,
            }
        );
        assert_eq!(a_txt(&dir), "after first 0\n");
        // The conversation was not rewound, so its checkpoints still match it.
        assert_eq!(turn_refs(&dir, &sid), phases(&[1, 2]));
        assert_eq!(row_of(&db, &sid).await.external_id.as_deref(), Some("native-1"));

        // Chat alone has nothing else to report: the failure is the answer.
        let (files, chat) = CHAT_ONLY;
        let err = revert_with(&db, &mgr, &launcher, &sid, &turns, "turn-2", files, chat)
            .await
            .unwrap_err();
        assert_eq!(err.code, "internal");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn revert_chat_works_outside_git() {
        let db = fresh_in_memory_db().await;
        let wid = workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::Codex,
            text("first"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        finish_turn(&mgr, "fake-conn-1").await;
        let prompts = ["first", "second"];

        let (files, chat) = CHAT_ONLY;
        let out = revert_with(
            &db,
            &mgr,
            &launcher,
            &sid,
            &transcript(&prompts),
            "turn-2",
            files,
            chat,
        )
        .await
        .unwrap();
        assert!(out.chat_rewound);
        let (files, chat) = BOTH;
        let err = revert_with(
            &db,
            &mgr,
            &launcher,
            &sid,
            &transcript(&prompts),
            "turn-2",
            files,
            chat,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "checkpoint_unavailable");
    }

    async fn listed(db: &AppDatabase, mgr: &ConnectionManager, wid: &str) -> Vec<String> {
        list_for_workspace(&db.conn, mgr, wid)
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.session_id)
            .collect()
    }

    /// Whether `conn`'s subscribers were told its history was rewound.
    async fn told_rewound(mgr: &ConnectionManager, conn: &str) -> bool {
        let state = mgr.get_state(conn).await.expect("state");
        let state = state.read().await;
        state
            .recent_events_after(0)
            .unwrap_or_default()
            .iter()
            .any(|e| matches!(e.payload, AcpEvent::HistoryRewound))
    }

    /// Hold every fork `launcher`'s agents are asked for until notified.
    fn hold_forks(launcher: &FakeLauncher) -> Arc<tokio::sync::Notify> {
        let gate = Arc::new(tokio::sync::Notify::new());
        *launcher.fork_gate.lock().unwrap() = Some(gate.clone());
        gate
    }

    #[tokio::test]
    async fn revert_chat_completes_when_the_caller_goes_away() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let prompts = ["first", "second"];
        let sid = ran(&db, &mgr, &launcher, &wid, &dir, &prompts).await;
        assert_eq!(turn_refs(&dir, &sid), phases(&[1, 2]));
        let gate = hold_forks(&launcher);
        let turns = transcript(&prompts);

        // The caller goes away once the agent has been asked to fork.
        let (files, chat) = CHAT_ONLY;
        let mut revert = Box::pin(revert_with(
            &db, &mgr, &launcher, &sid, &turns, "turn-2", files, chat,
        ));
        while launcher.forks.lock().unwrap().is_empty() {
            tokio::select! {
                _ = &mut revert => panic!("the fork is held"),
                _ = tokio::time::sleep(Duration::from_millis(2)) => {}
            }
        }
        drop(revert);
        gate.notify_one();

        // The rewind still completes, all of it.
        let mut done = false;
        for _ in 0..500 {
            let repointed = row_of(&db, &sid).await.external_id.as_deref() == Some("native-fork-1");
            let hidden = holder_of(&db, "native-1")
                .await
                .is_some_and(|kept| kept.deleted_at.is_some());
            if repointed && hidden && told_rewound(&mgr, "fake-conn-1").await {
                done = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(done, "the rewind completed without its caller");
        assert_eq!(
            turn_refs(&dir, &sid),
            phases(&[1]),
            "its checkpoints dropped"
        );
        assert_eq!(listed(&db, &mgr, &wid).await, std::slice::from_ref(&sid));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn revert_chat_to_an_answer_the_agent_no_longer_has() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let prompts = ["first", "second"];
        let sid = ran(&db, &mgr, &launcher, &wid, &dir, &prompts).await;
        let turns = transcript(&prompts);
        launcher.fork_point_rejected.store(true, Ordering::SeqCst);

        let (files, chat) = CHAT_ONLY;
        let err = revert_with(&db, &mgr, &launcher, &sid, &turns, "turn-2", files, chat)
            .await
            .unwrap_err();
        assert_eq!(err.code, "checkpoint_unavailable");
        assert_eq!(
            row_of(&db, &sid).await.external_id.as_deref(),
            Some("native-1")
        );
        assert_eq!(turn_refs(&dir, &sid), phases(&[1, 2]));
        assert!(!told_rewound(&mgr, "fake-conn-1").await);

        let (files, chat) = BOTH;
        let out = revert_with(&db, &mgr, &launcher, &sid, &turns, "turn-2", files, chat)
            .await
            .unwrap();
        assert!(out.files_restored && !out.chat_rewound);
        assert_eq!(a_txt(&dir), "after first 0\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn revert_in_a_root_workspace_is_refused_up_front() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let prompts = ["first", "second"];
        let sid = ran(&db, &mgr, &launcher, &wid, &dir, &prompts).await;
        let turns = transcript(&prompts);
        use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, IntoActiveModel};
        // The workspace turns out to be a root one.
        let mut folder =
            crate::db::entities::folder::Entity::find_by_id(row_of(&db, &sid).await.folder_id)
                .one(&db.conn)
                .await
                .unwrap()
                .unwrap()
                .into_active_model();
        folder.parent_id = Set(None);
        folder.update(&db.conn).await.unwrap();

        for (files, chat) in [FILES_ONLY, CHAT_ONLY, BOTH] {
            let err = revert_with(&db, &mgr, &launcher, &sid, &turns, "turn-2", files, chat)
                .await
                .unwrap_err();
            assert_eq!(err.code, "root_workspace", "files={files} chat={chat}");
        }
        assert!(launcher.forks.lock().unwrap().is_empty());
        assert_eq!(a_txt(&dir), "after second 1\n", "nothing was restored");
        assert_eq!(revert_refs(&dir, &sid), 0);
        assert_eq!(turn_refs(&dir, &sid), phases(&[1, 2]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn revert_takes_the_session_lock() {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let data = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::new_for_test(db, data.path().to_path_buf()));
        let mgr = &state.connection_manager;
        let launcher = Arc::new(FakeLauncher::new(mgr.clone_ref()));
        let prompts = ["first", "second"];
        let sid = ran(&state.db, mgr, &launcher, &wid, &dir, &prompts).await;

        let err = revert(&state, &*launcher, &sid, "turn-2", false, false)
            .await
            .unwrap_err();
        assert_eq!(err.code, "invalid_input");

        // Another operation holds the session: the revert waits its turn.
        let lock = session_lock(&sid);
        let held = lock.lock().await;
        let task = tokio::spawn({
            let (state, launcher, sid) = (state.clone(), launcher.clone(), sid.clone());
            async move { revert(&state, &*launcher, &sid, "turn-2", true, true).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!task.is_finished(), "the revert waits for the session lock");
        drop(held);
        // The fake agent keeps no transcript, so the turn is not one the
        // session has: refused, with nothing touched.
        let err = task.await.unwrap().unwrap_err();
        assert_eq!(err.code, "checkpoint_unavailable");
        assert!(launcher.forks.lock().unwrap().is_empty());
        assert_eq!(a_txt(&dir), "after second 1\n");
        assert_eq!(revert_refs(&dir, &sid), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
