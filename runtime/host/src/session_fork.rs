//! Zwiim-owned: forking a session from one of its turns.
//!
//! `session.fork` starts a new session from a finished exchange of an existing
//! one, in the same worktree or in a new one holding the files as they were at
//! that turn. The original session is left as it was and keeps running. The
//! agent carries the chat over itself where it can; otherwise the new session
//! opens idle, with a transcript of the chat up to that turn attached to its
//! first message. A new worktree always takes the transcript. No agent is
//! named here: which way a fork goes follows from what the agent answers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sea_orm::sea_query::Expr;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::acp::fork::{resolve_fork_point, LaunchIntent};
use crate::acp::manager::ConnectionManager;
use crate::acp::types::PromptInputBlock;
use crate::app_state::AppState;
use crate::db::entities::conversation::ConversationStatus;
use crate::db::entities::{conversation, folder, session_fork};
use crate::db::error::DbError;
use crate::db::service::conversation_service;
use crate::db::AppDatabase;
use crate::git_harness::checkpoint::{self, Checkpoint, CheckpointKey, Phase, Saved};
use crate::harness::{takes_notices, FORK_NOTICE};
use crate::models::agent::AgentType;
use crate::models::message::{MessageTurn, TurnRole};
use crate::protocol::error::ZwiimError;
use crate::session::{
    history, resolve, row_busy, session_id_of, session_lock, AgentLauncher, RuntimeLauncher,
    StartingGuard,
};
use crate::session_checkpoints::match_turns;
use crate::session_continue::{remove_attachments, store};
use crate::session_options::{apply_on_start, preferred_for_start, remember_start, OptionsRequest};
use crate::session_retry::carried_options;
use crate::session_summary::{self, SummaryKind};
use crate::terminal::manager::TerminalManager;
use crate::web::event_bridge::EventEmitter;
use crate::workspace::{self, WorktreeStart};

/// How long an agent gets to answer a fork once it has started. Past it the
/// fork takes a transcript, so a stuck agent never holds up a client.
#[cfg(not(test))]
const FORK_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(test)]
const FORK_TIMEOUT: Duration = Duration::from_millis(200);

/// Where a fork's new session runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkTarget {
    /// The source session's own worktree, with its files as they are now.
    Current,
    /// A new worktree holding the files as they were at the forked turn.
    NewWorktree,
}

/// The new session and the workspace it runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forked {
    pub session_id: String,
    pub workspace_id: String,
}

/// What a fork needs from the running Host.
struct ForkHost<'a> {
    db: &'a AppDatabase,
    connections: &'a ConnectionManager,
    emitter: &'a EventEmitter,
    terminals: &'a TerminalManager,
    worktrees_dir: &'a Path,
    prefix: Option<&'a str>,
}

/// A prompt and what answered it: the indexes of the user turn and of the last
/// assistant turn before the next prompt. That last turn is the one forked.
struct Exchange {
    user: usize,
    last: usize,
}

/// A session's exchanges in order. A prompt nothing answered has none.
fn exchanges(turns: &[MessageTurn]) -> Vec<Exchange> {
    let users: Vec<usize> = (0..turns.len())
        .filter(|&i| matches!(turns[i].role, TurnRole::User))
        .collect();
    users
        .iter()
        .enumerate()
        .filter_map(|(n, &user)| {
            let next = users.get(n + 1).copied().unwrap_or(turns.len());
            let last = (user + 1..next)
                .rev()
                .find(|&i| matches!(turns[i].role, TurnRole::Assistant))?;
            Some(Exchange { user, last })
        })
        .collect()
}

/// The prompt after `exchange`, if there is one. An exchange without one is
/// the whole chat, so forking it needs no point in the agent's history.
fn next_prompt<'a>(turns: &'a [MessageTurn], exchange: &Exchange) -> Option<&'a MessageTurn> {
    turns[exchange.last..]
        .iter()
        .find(|t| matches!(t.role, TurnRole::User))
}

/// The checkpoint holding the files as `exchange` left them: the end of its
/// turn, or failing that the start of the next one.
fn file_state(
    turns: &[MessageTurn],
    checkpoints: &[Checkpoint],
    exchange: &Exchange,
) -> Option<Checkpoint> {
    let starts: Vec<Checkpoint> = checkpoints
        .iter()
        .filter(|c| c.phase == Phase::Start)
        .cloned()
        .collect();
    let matched = match_turns(turns, &starts);
    let start_of = |turn: &MessageTurn| matched.get(&turn.id).map(|&i| &starts[i]);
    let end = start_of(&turns[exchange.user]).and_then(|start| {
        checkpoints
            .iter()
            .find(|c| c.phase == Phase::End && c.key == start.key)
    });
    end.or_else(|| next_prompt(turns, exchange).and_then(start_of))
        .cloned()
}

/// The turns of session `row` that can be forked, each with the targets it can
/// fork to. Reads only: a latest turn with no checkpoint of its files yet still
/// offers a new worktree, since a fork saves one.
pub(crate) async fn fork_targets(
    db: &AppDatabase,
    row: &conversation::Model,
    turns: &[MessageTurn],
) -> HashMap<String, Vec<ForkTarget>> {
    let mut targets = HashMap::new();
    let Ok(Some(folder)) = folder::Entity::find_by_id(row.folder_id).one(&db.conn).await else {
        return targets;
    };
    let Some(session_id) = row.session_id.as_deref() else {
        return targets;
    };
    let root = workspace::root_workspace_id_for_dir(&db.conn, &folder.path).await;
    let checkpoints = match root {
        Ok(Some(_)) => checkpoint::list(Path::new(&folder.path), session_id).await.ok(),
        _ => None,
    };
    for exchange in exchanges(turns) {
        let mut allowed = Vec::new();
        if folder.parent_id.is_some() {
            allowed.push(ForkTarget::Current);
        }
        if let Some(checkpoints) = &checkpoints {
            if next_prompt(turns, &exchange).is_none()
                || file_state(turns, checkpoints, &exchange).is_some()
            {
                allowed.push(ForkTarget::NewWorktree);
            }
        }
        if !allowed.is_empty() {
            targets.insert(turns[exchange.last].id.clone(), allowed);
        }
    }
    targets
}

/// The session `session_id` was forked from, `None` when it is no fork. A
/// source that is gone keeps its id and loses its title.
pub(crate) async fn forked_from(conn: &DatabaseConnection, session_id: &str) -> Option<Value> {
    let link = session_fork::Entity::find_by_id(session_id).one(conn).await.ok()??;
    let title = resolve(conn, &link.source_session_id).await.ok().and_then(|row| row.title);
    Some(json!({ "session_id": link.source_session_id, "title": title }))
}

/// What stays the same whichever way a fork carries the chat over.
struct Transfer<'a> {
    row: &'a conversation::Model,
    turns: &'a [MessageTurn],
    turn_id: &'a str,
    /// Whether the forked turn ends the chat.
    tail: bool,
    /// The workspace the new session runs in.
    workspace_id: &'a str,
    folder: &'a folder::Model,
    /// Whether the agent may carry the chat over itself. A new worktree always
    /// takes the transcript: agents keep their chats by folder.
    native: bool,
}

/// Fork session `row` at `turn_id` into `target`. On success the new session
/// is idle, unless it went through a summary, whose reading is its first turn.
///
/// Refused with `turn_in_progress` while the source works, `fork_unavailable`
/// when `turn_id` doesn't end an exchange, `root_workspace` for `Current` where
/// no session can start and `checkpoint_unavailable` when the files at the turn
/// are gone. A failed fork to a new worktree takes the worktree back out.
///
/// The caller holds session `row`'s lock.
async fn fork_locked<L: AgentLauncher>(
    host: &ForkHost<'_>,
    launcher: &L,
    row: &conversation::Model,
    turns: &[MessageTurn],
    turn_id: &str,
    target: ForkTarget,
) -> Result<Forked, ZwiimError> {
    if row_busy(host.connections, row).await {
        return Err(ZwiimError::turn_in_progress());
    }
    let workspace_id = workspace::workspace_id_of(&host.db.conn, row.folder_id)
        .await?
        .ok_or_else(|| ZwiimError::not_found("Session has no workspace"))?;
    let folder = workspace::resolve(&host.db.conn, &workspace_id).await?;
    let exchanges = exchanges(turns);
    let exchange = exchanges
        .iter()
        .find(|e| turns[e.last].id == turn_id)
        .ok_or_else(ZwiimError::fork_unavailable)?;
    let tail = next_prompt(turns, exchange).is_none();

    let forked = match target {
        ForkTarget::Current => {
            if folder.parent_id.is_none() {
                return Err(ZwiimError::root_workspace());
            }
            let transfer = Transfer {
                row,
                turns,
                turn_id,
                tail,
                workspace_id: &workspace_id,
                folder: &folder,
                native: true,
            };
            let session_id = carry_over(host, launcher, &transfer).await?;
            Forked {
                session_id,
                workspace_id,
            }
        }
        ForkTarget::NewWorktree => {
            let state = checkpoint_at(host, row, &folder, turns, exchange, tail).await?;
            let root_id = workspace::root_workspace_id_for_dir(&host.db.conn, &folder.path)
                .await?
                .ok_or_else(ZwiimError::fork_unavailable)?;
            let info = workspace::create_worktree_at(
                host.db,
                host.emitter,
                host.worktrees_dir,
                &root_id,
                WorktreeStart::Fork {
                    checkpoint: &state,
                    base_branch: folder.base_branch.as_deref(),
                    base_commit: folder.base_commit.as_deref(),
                },
                host.prefix,
            )
            .await?;
            let new_folder = workspace::resolve(&host.db.conn, &info.workspace_id).await?;
            let transfer = Transfer {
                row,
                turns,
                turn_id,
                tail,
                workspace_id: &info.workspace_id,
                folder: &new_folder,
                native: false,
            };
            match carry_over(host, launcher, &transfer).await {
                Ok(session_id) => Forked {
                    session_id,
                    workspace_id: info.workspace_id,
                },
                Err(e) => {
                    take_back(host, &info.workspace_id, info.branch.as_deref(), &root_id).await;
                    return Err(e);
                }
            }
        }
    };

    Ok(forked)
}

/// The checkpoint a new worktree starts from. The latest turn may have none
/// yet (an end never saved), and then its files are still there to save.
async fn checkpoint_at(
    host: &ForkHost<'_>,
    row: &conversation::Model,
    folder: &folder::Model,
    turns: &[MessageTurn],
    exchange: &Exchange,
    tail: bool,
) -> Result<Checkpoint, ZwiimError> {
    let session_id = row.session_id.as_deref().ok_or_else(ZwiimError::internal)?;
    let root = Path::new(&folder.path);
    host.connections
        .turn_checkpoints
        .close_conversation(row.id, "fork")
        .await;
    let checkpoints = checkpoint::list(root, session_id).await.unwrap_or_default();
    if let Some(state) = file_state(turns, &checkpoints, exchange) {
        return Ok(state);
    }
    if !tail {
        return Err(ZwiimError::checkpoint_unavailable());
    }
    let ordinal = checkpoint::next_ordinal(root, session_id)
        .await
        .map_err(|_| ZwiimError::checkpoint_unavailable())?;
    let key = CheckpointKey {
        session_id: session_id.to_string(),
        ordinal,
    };
    let Saved::Ref { .. } = checkpoint::save(root, &key, Phase::End, None).await else {
        return Err(ZwiimError::checkpoint_unavailable());
    };
    checkpoint::list(root, session_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|c| c.phase == Phase::End && c.key == key)
        .ok_or_else(ZwiimError::checkpoint_unavailable)
}

/// Take a worktree made for a fork that failed back out, with its fresh branch
/// (best effort). A session already running there keeps it.
async fn take_back(host: &ForkHost<'_>, workspace_id: &str, branch: Option<&str>, root_id: &str) {
    let removed = workspace::remove_worktree(
        host.db,
        host.emitter,
        host.connections,
        host.terminals,
        workspace_id,
        true,
        None,
    )
    .await;
    if let Err(e) = removed {
        tracing::warn!(code = e.code, "could not remove a failed fork's worktree");
        return;
    }
    let (Some(branch), Ok(root)) = (branch, workspace::resolve(&host.db.conn, root_id).await)
    else {
        return;
    };
    let deleted = crate::commands::folders::git_delete_branch(root.path, branch.to_string(), true);
    if let Err(e) = deleted.await {
        tracing::warn!(error = %e, "could not delete a failed fork's branch");
    }
}

/// Start the new session, through the agent if it can fork, else idle with a
/// transcript. Returns its `session_id`.
async fn carry_over<L: AgentLauncher>(
    host: &ForkHost<'_>,
    launcher: &L,
    transfer: &Transfer<'_>,
) -> Result<String, ZwiimError> {
    let row = transfer.row;
    let agent: AgentType = serde_json::from_value(serde_json::Value::String(row.agent_type.clone()))
        .map_err(|_| ZwiimError::internal())?;
    let source_session_id = row.session_id.as_deref().ok_or_else(ZwiimError::internal)?;
    let options = carried_options(host.db, host.connections, row, source_session_id).await;
    let title = match &row.title {
        Some(title) => format!("Fork of {title}"),
        None => "Forked session".to_string(),
    };
    if let Some(session_id) = fork_natively(host, launcher, transfer, agent, &options, &title).await? {
        if let Err(e) = record_link(host.db, transfer, &session_id, None, None).await {
            tracing::warn!(error = %e, "could not record where a forked session came from");
        }
        return Ok(session_id);
    }
    fork_from_transcript(host, transfer, agent, &options, &title).await
}

/// Record where forked session `session_id` came from and what its first
/// messages owe its agent: the `transcript`, the JSON `options` no launch has
/// applied yet, and, for an older turn whose worktree kept the later files, the
/// warning about them.
async fn record_link(
    db: &AppDatabase,
    transfer: &Transfer<'_>,
    session_id: &str,
    transcript: Option<String>,
    options: Option<String>,
) -> Result<(), DbError> {
    let source_session_id = transfer
        .row
        .session_id
        .clone()
        .ok_or_else(|| DbError::NotFound("a forked session's source id".into()))?;
    let link = session_fork::ActiveModel {
        session_id: Set(session_id.to_string()),
        source_session_id: Set(source_session_id),
        source_turn_id: Set(transfer.turn_id.to_string()),
        created_at: Set(chrono::Utc::now()),
        pending_transcript: Set(transcript),
        transcript_sent: Set(false),
        notice_owed: Set(transfer.native && !transfer.tail),
        pending_options: Set(options),
    };
    link.insert(&db.conn).await?;
    Ok(())
}

/// Have the agent fork its own chat. `Ok(None)` means it would not or could
/// not, with nothing left behind. Everything else about the new connection
/// that fails is a real failure.
async fn fork_natively<L: AgentLauncher>(
    host: &ForkHost<'_>,
    launcher: &L,
    transfer: &Transfer<'_>,
    agent: AgentType,
    options: &OptionsRequest,
    title: &str,
) -> Result<Option<String>, ZwiimError> {
    if !transfer.native {
        return Ok(None);
    }
    let Some(source) = transfer.row.external_id.clone() else {
        return Ok(None);
    };
    let point = if transfer.tail {
        None
    } else {
        match resolve_fork_point(transfer.turns, transfer.turn_id, agent) {
            Some(point) => Some(point),
            None => return Ok(None),
        }
    };
    let preferred = preferred_for_start(&host.db.conn, agent, options).await?;
    let target = transfer.folder;
    // Counted as starting, and the worktree stays put, until the row is linked.
    let _starting = StartingGuard::new(transfer.workspace_id);
    let (_, _launching) = workspace::begin_launch(&host.db.conn, target.id).await?;
    let (reply, answer) = oneshot::channel();
    let launched = launcher
        .launch(
            agent,
            target.path.clone(),
            target.id,
            LaunchIntent::Fork {
                source,
                point,
                reply,
            },
            preferred.clone().unwrap_or_default(),
        )
        .await;
    let connection_id = match launched {
        Ok(connection_id) => connection_id,
        Err(e) => {
            tracing::warn!(code = e.code, "agent did not start for a fork");
            return Ok(None);
        }
    };
    let refused = match tokio::time::timeout(FORK_TIMEOUT, answer).await {
        Ok(Ok(Ok(_))) => None,
        Ok(Ok(Err(e))) => Some(e.to_string()),
        Ok(Err(_)) => Some("connection closed".to_string()),
        Err(_) => Some("timed out".to_string()),
    };
    if let Some(reason) = refused {
        tracing::info!(reason, "agent did not fork; using a transcript");
        let _ = host.connections.disconnect(&connection_id).await;
        return Ok(None);
    }
    if preferred.is_none() {
        if let Err(e) = apply_on_start(host.connections, &connection_id, options).await {
            let _ = host.connections.disconnect(&connection_id).await;
            return Err(e);
        }
    }
    let linked = host
        .connections
        .link_forked_session(
            host.db,
            &connection_id,
            target.id,
            title,
            options.model.as_deref(),
        )
        .await;
    let conversation_id = match linked {
        Ok(id) => id,
        Err(e) => {
            let _ = host.connections.disconnect(&connection_id).await;
            return Err(e.into());
        }
    };
    let session_id = session_id_of(host.db, conversation_id).await?;
    if let Some(values) = preferred.filter(|values| !values.is_empty()) {
        remember_start(&host.db.conn, &session_id, values).await;
    }
    Ok(Some(session_id))
}

/// Open a new idle session holding a transcript of the chat up to the forked
/// turn, for its first message to carry. No agent starts until that message.
async fn fork_from_transcript(
    host: &ForkHost<'_>,
    transfer: &Transfer<'_>,
    agent: AgentType,
    options: &OptionsRequest,
    title: &str,
) -> Result<String, ZwiimError> {
    let chat = session_summary::through_turn(transfer.turns, transfer.turn_id)
        .ok_or_else(ZwiimError::fork_unavailable)?;
    let preferred = preferred_for_start(&host.db.conn, agent, options).await?;
    // The agent has never said what it takes here, so its first launch applies them.
    let held_options = if preferred.is_none() {
        Some(serde_json::to_string(options).map_err(|_| ZwiimError::internal())?)
    } else {
        None
    };
    let transcript = session_summary::build(
        &session_summary::turns_from_history(chat),
        &agent.to_string(),
        &[],
        SummaryKind::Forked,
    );
    let file_name = match &transfer.row.title {
        Some(title) => format!("Transcript of {title}.md"),
        None => "Transcript of session.md".to_string(),
    };
    // Counted as starting, and the worktree stays put, until the row is linked.
    let _starting = StartingGuard::new(transfer.workspace_id);
    let (_, _launching) = workspace::begin_launch(&host.db.conn, transfer.folder.id).await?;
    let root = Path::new(&transfer.folder.path);
    let path = store(root, &file_name, transcript.as_bytes()).await?;
    let opened = open_idle(host, transfer, agent, options, title, &path, held_options).await;
    let (conversation_id, session_id) = match opened {
        Ok(opened) => opened,
        Err(e) => {
            remove_attachments(&[path]).await;
            return Err(e);
        }
    };
    if let Some(values) = preferred.filter(|values| !values.is_empty()) {
        remember_start(&host.db.conn, &session_id, values).await;
    }
    crate::commands::conversations::emit_conversation_upsert(
        host.emitter,
        &host.db.conn,
        conversation_id,
    )
    .await;
    Ok(session_id)
}

/// The row of an idle forked session, with `transcript` and `held_options`
/// waiting for its first message. Leaves no row behind on failure.
async fn open_idle(
    host: &ForkHost<'_>,
    transfer: &Transfer<'_>,
    agent: AgentType,
    options: &OptionsRequest,
    title: &str,
    transcript: &Path,
    held_options: Option<String>,
) -> Result<(i32, String), ZwiimError> {
    let conn = &host.db.conn;
    let folder_id = transfer.folder.id;
    let row = conversation_service::create(conn, folder_id, agent, Some(title.to_string()), None)
        .await
        .map_err(|_| ZwiimError::internal())?;
    let filled = async {
        conversation_service::update_status(conn, row.id, ConversationStatus::PendingReview).await?;
        conversation_service::lock_title(conn, row.id).await?;
        if let Some(model) = &options.model {
            conversation_service::seed_model_if_empty(conn, row.id, model).await?;
        }
        let session_id = row
            .session_id
            .clone()
            .ok_or_else(|| DbError::NotFound("a forked session's id".into()))?;
        let held = transcript.to_string_lossy().into_owned();
        record_link(host.db, transfer, &session_id, Some(held), held_options).await?;
        Ok::<_, DbError>(session_id)
    }
    .await;
    match filled {
        Ok(session_id) => Ok((row.id, session_id)),
        Err(e) => {
            tracing::warn!(error = %e, "could not open a forked session");
            if let Err(e) = conversation_service::soft_delete(conn, row.id).await {
                tracing::warn!(error = %e, "could not drop a failed fork's row");
            }
            Err(ZwiimError::internal())
        }
    }
}

/// What a forked session's message took on, for [`message_sent`] to settle.
#[derive(Default)]
pub(crate) struct Owed {
    notice: bool,
    options: Option<OptionsRequest>,
}

/// Give the message `blocks` of session `row` what the session still owes its
/// agent. A slash command takes none of it: it is for the agent's own command
/// parser, and the next message has it instead.
///
/// `transcript_sent` is written before the send, so a turn too quick to wait
/// for still finds it. A refused send leaves it set, which is harmless: nothing
/// ended, and the next message writes it again.
pub(crate) async fn prepare_message(
    db: &AppDatabase,
    connections: &ConnectionManager,
    row: &conversation::Model,
    blocks: &mut Vec<PromptInputBlock>,
) -> Result<Owed, ZwiimError> {
    let Some(session_id) = row.session_id.clone() else {
        return Ok(Owed::default());
    };
    let link = session_fork::Entity::find_by_id(session_id)
        .one(&db.conn)
        .await
        .map_err(|_| ZwiimError::internal())?;
    let Some(link) = link else {
        return Ok(Owed::default());
    };
    let takes = takes_notices(blocks);
    if let Some(path) = link.pending_transcript.as_deref().map(PathBuf::from) {
        if takes && tokio::fs::metadata(&path).await.is_err() {
            let mut gone = link_of(&link);
            gone.pending_transcript = Set(None);
            gone.transcript_sent = Set(false);
            gone.update(&db.conn).await.map_err(|_| ZwiimError::internal())?;
            return Err(ZwiimError::transcript_missing());
        }
        if takes {
            blocks.push(PromptInputBlock::Text {
                text: crate::attachments::note(std::slice::from_ref(&path)),
            });
        }
        if link.transcript_sent != takes {
            let mut sent = link_of(&link);
            sent.transcript_sent = Set(takes);
            sent.update(&db.conn).await.map_err(|_| ZwiimError::internal())?;
        }
    }
    let notice = takes && link.notice_owed;
    if notice {
        connections.queue_notice(row.id, FORK_NOTICE.to_string());
    }
    let options = link.pending_options.as_deref().and_then(|held| {
        serde_json::from_str(held)
            .inspect_err(|e| tracing::warn!(error = %e, "could not read a fork's held options"))
            .ok()
    });
    Ok(Owed { notice, options })
}

/// The link's row, for an update of only what the caller sets.
fn link_of(link: &session_fork::Model) -> session_fork::ActiveModel {
    session_fork::ActiveModel {
        session_id: sea_orm::Unchanged(link.session_id.clone()),
        ..Default::default()
    }
}

/// Apply the options held for the agent just spawned on `connection_id`. A
/// failure is logged: a stale option must not hold up the message.
pub(crate) async fn apply_held_options(connections: &ConnectionManager, connection_id: &str, owed: &Owed) {
    let Some(options) = &owed.options else {
        return;
    };
    if let Err(e) = apply_on_start(connections, connection_id, options).await {
        tracing::warn!(code = e.code, "could not apply a fork's options");
    }
}

/// The message `prepare_message` readied is on its way, on an agent this send
/// `spawned` or not. A failure is logged: the worst left is a second copy.
pub(crate) async fn message_sent(db: &AppDatabase, row: &conversation::Model, owed: &Owed, spawned: bool) {
    let settled = spawned && owed.options.is_some();
    let Some(session_id) = row.session_id.clone().filter(|_| owed.notice || settled) else {
        return;
    };
    let mut link = session_fork::ActiveModel {
        session_id: sea_orm::Unchanged(session_id),
        ..Default::default()
    };
    if owed.notice {
        link.notice_owed = Set(false);
    }
    if settled {
        link.pending_options = Set(None);
    }
    if let Err(e) = link.update(&db.conn).await {
        tracing::warn!(error = %e, "could not settle what a forked session owed");
    }
}

/// A turn of conversation `conversation_id` ended well, so the transcript it
/// carried reached the agent for good. A failed turn leaves the transcript for
/// the next message. A failure is logged: the worst left is a second copy.
pub(crate) async fn turn_ended_well(conn: &DatabaseConnection, conversation_id: i32) {
    let row = conversation::Entity::find_by_id(conversation_id).one(conn).await;
    let session_id = match row {
        Ok(row) => row.and_then(|row| row.session_id),
        Err(e) => {
            tracing::warn!(error = %e, "could not read a finished turn's session");
            return;
        }
    };
    let Some(session_id) = session_id else {
        return;
    };
    let cleared = session_fork::Entity::update_many()
        .col_expr(session_fork::Column::PendingTranscript, Expr::value(None::<String>))
        .col_expr(session_fork::Column::TranscriptSent, Expr::value(false))
        .filter(session_fork::Column::SessionId.eq(session_id))
        .filter(session_fork::Column::TranscriptSent.eq(true))
        .exec(conn)
        .await;
    if let Err(e) = cleared {
        tracing::warn!(error = %e, "could not clear a delivered transcript");
    }
}

/// Fork session `session_id` at `turn_id`, on its own task: a fork makes a
/// worktree and maybe an agent, so a client that goes away mustn't stop it halfway.
pub async fn fork(
    state: Arc<AppState>,
    session_id: String,
    turn_id: String,
    target: ForkTarget,
) -> Result<Forked, ZwiimError> {
    tokio::spawn(run(state, session_id, turn_id, target))
        .await
        .map_err(|_| ZwiimError::internal())?
}

async fn run(
    state: Arc<AppState>,
    session_id: String,
    turn_id: String,
    target: ForkTarget,
) -> Result<Forked, ZwiimError> {
    let row = resolve(&state.db.conn, &session_id).await?;
    let prefix = match target {
        ForkTarget::NewWorktree => {
            let cwd = crate::protocol::http::gh_cwd();
            crate::settings::branch_prefix(&state.db.conn, state.gh.as_deref(), &cwd).await
        }
        ForkTarget::Current => None,
    };
    let lock = session_lock(&session_id);
    let guard = lock.lock().await;
    let turns = history(&state, &session_id).await?;
    let host = ForkHost {
        db: &state.db,
        connections: &state.connection_manager,
        emitter: &state.emitter,
        terminals: &state.terminal_manager,
        worktrees_dir: &state.worktrees_dir,
        prefix: prefix.as_deref(),
    };
    let launcher = RuntimeLauncher { state: &state };
    let forked = fork_locked(&host, &launcher, &row, &turns, &turn_id, target).await?;
    drop(guard);
    if target == ForkTarget::NewWorktree {
        // The repository's setup script starts in its own terminal; neither
        // the reply nor the new session waits for it.
        workspace::scripts::setup_new_worktree(
            &state.db.conn,
            &state.terminal_manager,
            &state.emitter,
            &forked.workspace_id,
            crate::commands::terminal::prepare_credential_env(&state.data_dir),
        )
        .await;
    }
    crate::protocol::http::announce_sessions_changed(&state, &[&forked.session_id]).await;
    Ok(forked)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    use super::*;
    use sea_orm::ConnectionTrait;

    use crate::acp::lifecycle::handle_event;
    use crate::acp::types::{AcpEvent, EventEnvelope};
    use crate::db::service::folder_service;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
    use crate::models::agent::AgentType;
    use crate::session::orchestration_tests::{
        checkpoint_ref, end_turn_observed, fixture_git, ran, ran_as, settle, text, transcript, FakeLauncher,
    };
    use crate::session::send;
    use crate::session_options::tests::claude;
    use crate::web::event_bridge::emit_with_state;

    struct Fixture {
        db: AppDatabase,
        mgr: ConnectionManager,
        launcher: FakeLauncher,
        terminals: TerminalManager,
        root: PathBuf,
        wt: PathBuf,
        wid: String,
        root_wid: String,
        worktrees: PathBuf,
        _dir: tempfile::TempDir,
    }

    /// A real repository with one worktree, as `create_worktree` makes them.
    async fn forkable() -> Fixture {
        let db = fresh_in_memory_db().await;
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let root = base.join("repo");
        let wt = base.join("wt");
        let worktrees = base.join("worktrees");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&worktrees).unwrap();
        fixture_git(&root, &["init", "-q", "-b", "main"]);
        fixture_git(&root, &["config", "user.email", "t@example.com"]);
        fixture_git(&root, &["config", "user.name", "T"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        fixture_git(&root, &["add", "a.txt"]);
        fixture_git(&root, &["commit", "-q", "-m", "first"]);
        fixture_git(
            &root,
            &["worktree", "add", "-q", "-b", "wt-1", wt.to_str().unwrap()],
        );
        let root_folder = seed_folder(&db, root.to_str().unwrap()).await;
        let wt_folder =
            folder_service::add_folder_with_parent(&db.conn, wt.to_str().unwrap(), Some(root_folder))
                .await
                .unwrap()
                .id;
        let wid = workspace::workspace_id_of(&db.conn, wt_folder)
            .await
            .unwrap()
            .unwrap();
        let root_wid = workspace::workspace_id_of(&db.conn, root_folder)
            .await
            .unwrap()
            .unwrap();
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        Fixture {
            db,
            mgr,
            launcher,
            terminals: TerminalManager::new(),
            root,
            wt,
            wid,
            root_wid,
            worktrees,
            _dir: dir,
        }
    }

    async fn ran_two(f: &Fixture) -> (String, Vec<MessageTurn>) {
        let prompts = ["one", "two"];
        let sid = ran(&f.db, &f.mgr, &f.launcher, &f.wid, &f.wt, &prompts).await;
        (sid, transcript(&prompts))
    }

    async fn fork_it(
        f: &Fixture,
        sid: &str,
        turns: &[MessageTurn],
        turn_id: &str,
        target: ForkTarget,
    ) -> Result<Forked, ZwiimError> {
        let row = resolve(&f.db.conn, sid).await.unwrap();
        let host = ForkHost {
            db: &f.db,
            connections: &f.mgr,
            emitter: &EventEmitter::Noop,
            terminals: &f.terminals,
            worktrees_dir: &f.worktrees,
            prefix: None,
        };
        fork_locked(&host, &f.launcher, &row, turns, turn_id, target).await
    }

    /// The transcript session `session_id` is still to hand its agent.
    async fn pending(f: &Fixture, session_id: &str) -> Option<String> {
        session_fork::Entity::find_by_id(session_id.to_string())
            .one(&f.db.conn)
            .await
            .unwrap()
            .expect("recorded")
            .pending_transcript
    }

    /// What session `session_id` still owes its agent.
    async fn link(f: &Fixture, session_id: &str) -> session_fork::Model {
        session_fork::Entity::find_by_id(session_id.to_string())
            .one(&f.db.conn)
            .await
            .unwrap()
            .expect("recorded")
    }

    async fn say(f: &Fixture, session_id: &str, what: &str) -> Result<(), ZwiimError> {
        send(&f.db, &f.mgr, &f.launcher, session_id, text(what), None).await
    }

    /// The turn on `conn` ends with `reason`, as far as the Host sees it.
    async fn finish_turn(f: &Fixture, conn: &str, reason: &str) {
        settle().await;
        end_turn_observed(&f.mgr, conn, reason).await;
        let envelope = EventEnvelope {
            seq: 0,
            connection_id: conn.to_string(),
            payload: AcpEvent::TurnComplete {
                session_id: "native".into(),
                stop_reason: reason.into(),
                agent_type: "codex".into(),
            },
        };
        handle_event(&f.db.conn, &f.mgr, &envelope, None).await.unwrap();
    }

    /// A fork of `turn_id` in the same worktree, which the agent refused.
    async fn idle_fork(f: &Fixture, turn_id: &str) -> Forked {
        let (sid, turns) = ran_two(f).await;
        f.launcher.fork_fails.store(true, Ordering::SeqCst);
        fork_it(f, &sid, &turns, turn_id, ForkTarget::Current)
            .await
            .unwrap()
    }

    fn drop_ref(dir: &Path, name: &str) {
        fixture_git(dir, &["update-ref", "-d", name]);
    }

    fn file_at(workspace: &folder::Model) -> String {
        std::fs::read_to_string(Path::new(&workspace.path).join("a.txt")).unwrap()
    }

    fn worktree_count(f: &Fixture) -> usize {
        fixture_git(&f.root, &["worktree", "list"]).lines().count()
    }

    fn branches(f: &Fixture) -> Vec<String> {
        fixture_git(&f.root, &["branch", "--format=%(refname:short)"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[tokio::test]
    async fn latest_turn_forks_natively_in_the_same_worktree() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        let source = resolve(&f.db.conn, &sid).await.unwrap();

        let forked = fork_it(&f, &sid, &turns, "turn-3", ForkTarget::Current)
            .await
            .unwrap();

        assert_eq!(forked.workspace_id, f.wid);
        assert_ne!(forked.session_id, sid);
        let asked = f.launcher.fork_launches.lock().unwrap().clone();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].0, source.external_id.clone().unwrap());
        assert_eq!(asked[0].1, None, "the latest turn is the whole chat");
        assert_eq!(asked[0].2, f.wt.to_str().unwrap());

        let new_row = resolve(&f.db.conn, &forked.session_id).await.unwrap();
        assert_eq!(new_row.folder_id, source.folder_id);
        assert_eq!(new_row.external_id.as_deref(), Some("native-fork-2"));
        assert_eq!(
            new_row.title,
            source.title.as_ref().map(|title| format!("Fork of {title}"))
        );
        assert!(f.launcher.prompts_for("fake-conn-2").is_empty());
        assert_eq!(pending(&f, &forked.session_id).await, None);

        let after = resolve(&f.db.conn, &sid).await.unwrap();
        assert_eq!(after.title, source.title);
        assert_eq!(after.external_id, source.external_id);
        assert_eq!(f.launcher.prompts_for("fake-conn-1").len(), 2);

        let link = session_fork::Entity::find_by_id(forked.session_id.clone())
            .one(&f.db.conn)
            .await
            .unwrap()
            .expect("recorded");
        assert_eq!(link.source_session_id, sid);
        assert_eq!(link.source_turn_id, "turn-3");
    }

    #[tokio::test]
    async fn older_turn_names_its_point_and_warns_about_the_files() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;

        let forked = fork_it(&f, &sid, &turns, "turn-1", ForkTarget::Current)
            .await
            .unwrap();

        let asked = f.launcher.fork_launches.lock().unwrap().clone();
        assert!(asked[0].1.is_some(), "an older turn is pointed at");
        send(
            &f.db,
            &f.mgr,
            &f.launcher,
            &forked.session_id,
            text("go on"),
            None,
        )
        .await
        .unwrap();
        let prompts = f.launcher.prompts_for("fake-conn-2");
        assert!(prompts.join("\n").contains("forked from an earlier turn"));
    }

    #[tokio::test]
    async fn latest_turn_gets_no_files_warning() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;

        let forked = fork_it(&f, &sid, &turns, "turn-3", ForkTarget::Current)
            .await
            .unwrap();

        send(
            &f.db,
            &f.mgr,
            &f.launcher,
            &forked.session_id,
            text("go on"),
            None,
        )
        .await
        .unwrap();
        let prompts = f.launcher.prompts_for("fake-conn-2");
        assert!(!prompts.join("\n").contains("forked from an earlier turn"));
    }

    #[tokio::test]
    async fn refusal_opens_an_idle_session_with_the_chat_up_to_the_turn() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        let source = resolve(&f.db.conn, &sid).await.unwrap();
        let source_title = source.title.clone().unwrap();
        f.launcher.fork_fails.store(true, Ordering::SeqCst);

        let forked = fork_it(&f, &sid, &turns, "turn-1", ForkTarget::Current)
            .await
            .unwrap();

        assert!(f.mgr.get_state("fake-conn-2").await.is_none(), "closed");
        assert!(f.mgr.get_state("fake-conn-3").await.is_none(), "no agent yet");
        let new_row = resolve(&f.db.conn, &forked.session_id).await.unwrap();
        assert_eq!(new_row.external_id, None);
        assert_eq!(new_row.title, Some(format!("Fork of {source_title}")));
        let path = pending(&f, &forked.session_id).await.unwrap();
        assert_eq!(
            Path::new(&path).file_name().unwrap().to_str().unwrap(),
            format!("Transcript of {source_title}.md")
        );
        let transcript = std::fs::read_to_string(&path).unwrap();
        assert!(transcript.contains("one"));
        assert!(!transcript.contains("two"), "stops at the forked turn");
    }

    #[tokio::test]
    async fn first_message_carries_the_transcript_once() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        f.launcher.fork_fails.store(true, Ordering::SeqCst);
        let forked = fork_it(&f, &sid, &turns, "turn-3", ForkTarget::Current)
            .await
            .unwrap();
        let path = pending(&f, &forked.session_id).await.unwrap();

        say(&f, &forked.session_id, "go on").await.unwrap();
        assert_eq!(pending(&f, &forked.session_id).await, Some(path.clone()));
        finish_turn(&f, "fake-conn-3", "end_turn").await;
        say(&f, &forked.session_id, "and on").await.unwrap();

        let prompts = f.launcher.prompts_for("fake-conn-3");
        assert_eq!(prompts.len(), 2);
        assert!(prompts[0].contains("go on") && prompts[0].contains(&path));
        assert!(!prompts[1].contains(&path));
        assert_eq!(pending(&f, &forked.session_id).await, None);
        let new_row = resolve(&f.db.conn, &forked.session_id).await.unwrap();
        assert!(new_row.external_id.is_some(), "the idle row is adopted");
    }

    #[tokio::test]
    async fn failed_first_message_keeps_the_transcript() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        f.launcher.fork_fails.store(true, Ordering::SeqCst);
        let forked = fork_it(&f, &sid, &turns, "turn-3", ForkTarget::Current)
            .await
            .unwrap();
        let path = pending(&f, &forked.session_id).await.unwrap();

        f.launcher.launch_fails.store(true, Ordering::SeqCst);
        say(&f, &forked.session_id, "go on").await.unwrap_err();
        assert_eq!(pending(&f, &forked.session_id).await, Some(path.clone()));

        f.launcher.launch_fails.store(false, Ordering::SeqCst);
        say(&f, &forked.session_id, "go on").await.unwrap();
        let prompts = f.launcher.prompts_for("fake-conn-3");
        assert!(prompts[0].contains(&path));
    }

    #[tokio::test]
    async fn turn_that_fails_leaves_the_transcript_for_the_next_message() {
        let f = forkable().await;
        let forked = idle_fork(&f, "turn-3").await;
        let path = pending(&f, &forked.session_id).await.unwrap();

        say(&f, &forked.session_id, "go on").await.unwrap();
        finish_turn(&f, "fake-conn-3", "refusal").await;
        assert_eq!(pending(&f, &forked.session_id).await, Some(path.clone()));
        say(&f, &forked.session_id, "again").await.unwrap();
        finish_turn(&f, "fake-conn-3", "end_turn").await;
        assert_eq!(pending(&f, &forked.session_id).await, None);

        let prompts = f.launcher.prompts_for("fake-conn-3");
        assert!(prompts[1].contains(&path));
    }

    #[tokio::test]
    async fn slash_command_leaves_transcript_and_notice_for_the_next_message() {
        let f = forkable().await;
        let forked = idle_fork(&f, "turn-1").await;
        let path = pending(&f, &forked.session_id).await.unwrap();

        say(&f, &forked.session_id, "/compact").await.unwrap();
        finish_turn(&f, "fake-conn-3", "end_turn").await;
        assert_eq!(pending(&f, &forked.session_id).await, Some(path.clone()));
        assert!(link(&f, &forked.session_id).await.notice_owed);
        say(&f, &forked.session_id, "go on").await.unwrap();

        let prompts = f.launcher.prompts_for("fake-conn-3");
        assert!(!prompts[0].contains(&path));
        assert!(!prompts[0].contains("forked from an earlier turn"));
        assert!(prompts[1].contains(&path));
        assert!(prompts[1].contains("forked from an earlier turn"));
    }

    #[tokio::test]
    async fn notice_is_owed_on_the_link_until_a_message_takes_it() {
        let f = forkable().await;
        let forked = idle_fork(&f, "turn-1").await;
        let new_row = resolve(&f.db.conn, &forked.session_id).await.unwrap();

        assert!(link(&f, &forked.session_id).await.notice_owed);
        assert!(f.mgr.pending_notices(new_row.id).is_empty(), "a restart keeps it");

        say(&f, &forked.session_id, "go on").await.unwrap();
        assert!(!link(&f, &forked.session_id).await.notice_owed);
    }

    #[tokio::test]
    async fn refused_message_leaves_one_notice_for_the_next() {
        let f = forkable().await;
        let forked = idle_fork(&f, "turn-1").await;

        f.launcher.launch_fails.store(true, Ordering::SeqCst);
        say(&f, &forked.session_id, "go on").await.unwrap_err();
        assert!(link(&f, &forked.session_id).await.notice_owed);
        f.launcher.launch_fails.store(false, Ordering::SeqCst);
        say(&f, &forked.session_id, "go on").await.unwrap();

        let prompts = f.launcher.prompts_for("fake-conn-3");
        assert_eq!(prompts[0].matches("forked from an earlier turn").count(), 1);
    }

    #[tokio::test]
    async fn transcript_gone_refuses_once_then_goes_on_without_it() {
        let f = forkable().await;
        let forked = idle_fork(&f, "turn-3").await;
        let path = pending(&f, &forked.session_id).await.unwrap();
        std::fs::remove_file(&path).unwrap();

        let err = say(&f, &forked.session_id, "go on").await.unwrap_err();
        assert_eq!(err.code, "transcript_missing");
        assert_eq!(pending(&f, &forked.session_id).await, None);
        assert!(f.launcher.prompts_for("fake-conn-3").is_empty());

        say(&f, &forked.session_id, "go on").await.unwrap();
        assert!(!f.launcher.prompts_for("fake-conn-3")[0].contains(&path));
    }

    /// Run `claude` on mode `plan` with no agent report on this Host, so a
    /// fork can't work out its native values yet.
    async fn on_plan_mode(f: &Fixture) -> (String, Vec<MessageTurn>) {
        let prompts = ["one", "two"];
        let sid = ran_as(&f.db, &f.mgr, &f.launcher, &f.wid, &f.wt, AgentType::ClaudeCode, &prompts).await;
        let state = f.mgr.get_state("fake-conn-1").await.unwrap();
        state.write().await.config_options = Some(claude("plan"));
        (sid, transcript(&prompts))
    }

    #[tokio::test]
    async fn options_of_an_unknown_agent_wait_for_the_first_launch() {
        let f = forkable().await;
        let (sid, turns) = on_plan_mode(&f).await;
        f.launcher.fork_fails.store(true, Ordering::SeqCst);
        let forked = fork_it(&f, &sid, &turns, "turn-3", ForkTarget::NewWorktree)
            .await
            .unwrap();
        assert!(link(&f, &forked.session_id).await.pending_options.is_some());

        *f.launcher.advertises.lock().unwrap() = Some(claude("default"));
        say(&f, &forked.session_id, "go on").await.unwrap();

        let sets = f.launcher.config_sets.lock().unwrap().clone();
        assert_eq!(sets, vec![("fake-conn-2".into(), "mode".into(), "plan".into())]);
        assert_eq!(link(&f, &forked.session_id).await.pending_options, None);
    }

    #[tokio::test]
    async fn options_the_agent_lost_never_hold_up_the_first_message() {
        let f = forkable().await;
        let (sid, turns) = on_plan_mode(&f).await;
        f.launcher.fork_fails.store(true, Ordering::SeqCst);
        let forked = fork_it(&f, &sid, &turns, "turn-3", ForkTarget::NewWorktree)
            .await
            .unwrap();

        *f.launcher.advertises.lock().unwrap() = Some(vec![]);
        say(&f, &forked.session_id, "go on").await.unwrap();

        assert_eq!(f.launcher.prompts_for("fake-conn-2").len(), 1);
        assert_eq!(link(&f, &forked.session_id).await.pending_options, None);
    }

    #[tokio::test]
    async fn options_of_a_known_agent_are_remembered_for_the_launch() {
        let f = forkable().await;
        let (sid, turns) = on_plan_mode(&f).await;
        crate::session_options::remember_agent_report(&f.db.conn, "claude_code", &claude("default")).await;
        f.launcher.fork_fails.store(true, Ordering::SeqCst);

        let forked = fork_it(&f, &sid, &turns, "turn-3", ForkTarget::NewWorktree)
            .await
            .unwrap();

        assert_eq!(link(&f, &forked.session_id).await.pending_options, None);
        let values = crate::session_options::preferred_values(&f.db.conn, &forked.session_id).await;
        assert_eq!(values.get("mode").map(String::as_str), Some("plan"));
    }

    #[tokio::test]
    async fn older_turn_warns_about_the_files_on_the_first_message() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        f.launcher.fork_fails.store(true, Ordering::SeqCst);
        let forked = fork_it(&f, &sid, &turns, "turn-1", ForkTarget::Current)
            .await
            .unwrap();

        say(&f, &forked.session_id, "go on").await.unwrap();

        let prompts = f.launcher.prompts_for("fake-conn-3");
        assert!(prompts.join("\n").contains("forked from an earlier turn"));
    }

    #[tokio::test]
    async fn transcript_name_survives_any_title() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        let id = resolve(&f.db.conn, &sid).await.unwrap().id;
        conversation_service::update_title(&f.db.conn, id, "fix/auth".into())
            .await
            .unwrap();
        f.launcher.fork_fails.store(true, Ordering::SeqCst);

        let forked = fork_it(&f, &sid, &turns, "turn-3", ForkTarget::Current)
            .await
            .unwrap();

        let path = pending(&f, &forked.session_id).await.unwrap();
        assert!(path.ends_with("/Transcript of fixauth.md"), "{path}");
    }

    #[tokio::test]
    async fn turn_with_no_point_opens_idle() {
        let f = forkable().await;
        let prompts = ["one", "two"];
        let sid = ran_as(
            &f.db,
            &f.mgr,
            &f.launcher,
            &f.wid,
            &f.wt,
            AgentType::Gemini,
            &prompts,
        )
        .await;

        let forked = fork_it(&f, &sid, &transcript(&prompts), "turn-1", ForkTarget::Current)
            .await
            .unwrap();

        assert!(f.launcher.fork_launches.lock().unwrap().is_empty());
        assert!(f.mgr.get_state("fake-conn-2").await.is_none());
        assert!(pending(&f, &forked.session_id).await.is_some());
    }

    #[tokio::test]
    async fn rejected_point_opens_idle() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        f.launcher.fork_point_rejected.store(true, Ordering::SeqCst);

        let forked = fork_it(&f, &sid, &turns, "turn-1", ForkTarget::Current)
            .await
            .unwrap();

        assert!(f.mgr.get_state("fake-conn-2").await.is_none());
        assert!(pending(&f, &forked.session_id).await.is_some());
    }

    #[tokio::test]
    async fn agent_that_never_answers_opens_idle() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        f.launcher.fork_hangs.store(true, Ordering::SeqCst);

        let forked = fork_it(&f, &sid, &turns, "turn-3", ForkTarget::Current)
            .await
            .unwrap();

        assert!(f.mgr.get_state("fake-conn-2").await.is_none());
        assert!(pending(&f, &forked.session_id).await.is_some());
    }

    #[tokio::test]
    async fn connection_lost_before_answering_opens_idle() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        f.launcher.fork_lost.store(true, Ordering::SeqCst);

        let forked = fork_it(&f, &sid, &turns, "turn-3", ForkTarget::Current)
            .await
            .unwrap();

        assert!(pending(&f, &forked.session_id).await.is_some());
    }

    #[tokio::test]
    async fn new_worktree_holds_the_files_at_an_older_turn() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        let head = fixture_git(&f.wt, &["rev-parse", "HEAD"]);

        let forked = fork_it(&f, &sid, &turns, "turn-1", ForkTarget::NewWorktree)
            .await
            .unwrap();

        assert_ne!(forked.workspace_id, f.wid);
        let target = workspace::resolve(&f.db.conn, &forked.workspace_id)
            .await
            .unwrap();
        assert_eq!(file_at(&target), "after one 0\n");
        assert_eq!(
            std::fs::read_to_string(f.wt.join("a.txt")).unwrap(),
            "after two 1\n",
            "the source keeps its files"
        );
        assert_eq!(fixture_git(&f.wt, &["rev-parse", "HEAD"]), head);
    }

    #[tokio::test]
    async fn new_worktree_always_takes_the_transcript() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;

        let forked = fork_it(&f, &sid, &turns, "turn-3", ForkTarget::NewWorktree)
            .await
            .unwrap();

        assert!(f.launcher.fork_launches.lock().unwrap().is_empty());
        assert!(f.mgr.get_state("fake-conn-2").await.is_none());
        let target = workspace::resolve(&f.db.conn, &forked.workspace_id)
            .await
            .unwrap();
        let path = pending(&f, &forked.session_id).await.unwrap();
        assert!(path.starts_with(&target.path), "{path}");

        say(&f, &forked.session_id, "go on").await.unwrap();

        assert!(f.launcher.prompts_for("fake-conn-2")[0].contains(&path));
    }

    #[tokio::test]
    async fn latest_turn_with_no_end_checkpoint_saves_the_files_now() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        drop_ref(&f.wt, &checkpoint_ref(&sid, 2, "end"));
        std::fs::write(f.wt.join("a.txt"), "edited\n").unwrap();

        let forked = fork_it(&f, &sid, &turns, "turn-3", ForkTarget::NewWorktree)
            .await
            .unwrap();

        let target = workspace::resolve(&f.db.conn, &forked.workspace_id)
            .await
            .unwrap();
        assert_eq!(file_at(&target), "edited\n");
    }

    #[tokio::test]
    async fn older_turn_with_no_end_checkpoint_uses_the_next_start() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        drop_ref(&f.wt, &checkpoint_ref(&sid, 1, "end"));

        let forked = fork_it(&f, &sid, &turns, "turn-1", ForkTarget::NewWorktree)
            .await
            .unwrap();

        let target = workspace::resolve(&f.db.conn, &forked.workspace_id)
            .await
            .unwrap();
        assert_eq!(file_at(&target), "after one 0\n");
    }

    #[tokio::test]
    async fn older_turn_without_checkpoints_creates_nothing() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        drop_ref(&f.wt, &checkpoint_ref(&sid, 1, "end"));
        drop_ref(&f.wt, &checkpoint_ref(&sid, 2, "start"));

        let err = fork_it(&f, &sid, &turns, "turn-1", ForkTarget::NewWorktree)
            .await
            .unwrap_err();

        assert_eq!(err.code, "checkpoint_unavailable");
        assert_eq!(worktree_count(&f), 2);
        assert!(f.launcher.fork_launches.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn running_turn_refuses_the_fork() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        send(&f.db, &f.mgr, &f.launcher, &sid, text("three"), None)
            .await
            .unwrap();
        settle().await;

        let err = fork_it(&f, &sid, &turns, "turn-3", ForkTarget::Current)
            .await
            .unwrap_err();

        assert_eq!(err.code, "turn_in_progress");
    }

    #[tokio::test]
    async fn waiting_permission_refuses_the_fork() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        let (state, emitter) = f.mgr.get_state_and_emitter("fake-conn-1").await.unwrap();
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

        let err = fork_it(&f, &sid, &turns, "turn-3", ForkTarget::Current)
            .await
            .unwrap_err();

        assert_eq!(err.code, "turn_in_progress");
    }

    #[tokio::test]
    async fn only_the_end_of_an_exchange_forks() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;

        for turn_id in ["turn-0", "turn-2", "stale"] {
            let err = fork_it(&f, &sid, &turns, turn_id, ForkTarget::Current)
                .await
                .unwrap_err();
            assert_eq!(err.code, "fork_unavailable", "{turn_id}");
        }
    }

    #[tokio::test]
    async fn root_workspace_takes_no_session_of_its_own() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        let source = resolve(&f.db.conn, &sid).await.unwrap();
        let root_folder = workspace::resolve(&f.db.conn, &f.root_wid).await.unwrap();
        let row = conversation::Model {
            folder_id: root_folder.id,
            ..source
        };
        let host = ForkHost {
            db: &f.db,
            connections: &f.mgr,
            emitter: &EventEmitter::Noop,
            terminals: &f.terminals,
            worktrees_dir: &f.worktrees,
            prefix: None,
        };

        let err = fork_locked(&host, &f.launcher, &row, &turns, "turn-3", ForkTarget::Current)
            .await
            .unwrap_err();

        assert_eq!(err.code, "root_workspace");
    }

    #[tokio::test]
    async fn failed_new_worktree_fork_leaves_nothing_behind() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        let conversations = conversation::Entity::find().all(&f.db.conn);
        let before = conversations.await.unwrap().len();
        f.db.conn
            .execute_unprepared("DROP TABLE session_forks")
            .await
            .unwrap();

        let err = fork_it(&f, &sid, &turns, "turn-1", ForkTarget::NewWorktree)
            .await
            .unwrap_err();

        assert_eq!(err.code, "internal");
        let rows = conversation::Entity::find().all(&f.db.conn).await.unwrap();
        let live = rows.iter().filter(|row| row.deleted_at.is_none()).count();
        assert_eq!(live, before);
        assert_eq!(worktree_count(&f), 2);
        assert_eq!(branches(&f), ["main", "wt-1"]);
        let made = std::fs::read_dir(f.worktrees.join("repo")).unwrap();
        assert_eq!(made.count(), 0);
    }

    #[tokio::test]
    async fn transcript_that_cannot_be_held_for_later_is_not_left_behind() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        f.launcher.fork_fails.store(true, Ordering::SeqCst);
        f.db.conn
            .execute_unprepared("DROP TABLE session_forks")
            .await
            .unwrap();

        fork_it(&f, &sid, &turns, "turn-3", ForkTarget::Current)
            .await
            .unwrap_err();

        let stored = f.wt.join(crate::workspace::zwiim_dir::ATTACHMENTS);
        assert_eq!(std::fs::read_dir(stored).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn source_with_no_live_agent_still_forks() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        f.mgr.disconnect("fake-conn-1").await.unwrap();

        fork_it(&f, &sid, &turns, "turn-3", ForkTarget::Current)
            .await
            .unwrap();

        assert_eq!(f.launcher.fork_launches.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn source_keeps_running_after_a_fork() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;

        fork_it(&f, &sid, &turns, "turn-3", ForkTarget::Current)
            .await
            .unwrap();
        send(&f.db, &f.mgr, &f.launcher, &sid, text("three"), None)
            .await
            .unwrap();

        assert_eq!(f.launcher.prompts_for("fake-conn-1").len(), 3);
    }

    #[tokio::test]
    async fn targets_follow_what_each_turn_can_do() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        let row = resolve(&f.db.conn, &sid).await.unwrap();

        let targets = fork_targets(&f.db, &row, &turns).await;
        let both = vec![ForkTarget::Current, ForkTarget::NewWorktree];
        assert_eq!(targets.get("turn-1"), Some(&both));
        assert_eq!(targets.get("turn-3"), Some(&both));
        assert_eq!(targets.get("turn-0"), None);

        drop_ref(&f.wt, &checkpoint_ref(&sid, 1, "end"));
        drop_ref(&f.wt, &checkpoint_ref(&sid, 2, "start"));
        let targets = fork_targets(&f.db, &row, &turns).await;
        assert_eq!(targets.get("turn-1"), Some(&vec![ForkTarget::Current]));
        assert_eq!(targets.get("turn-3"), Some(&both));
    }

    #[tokio::test]
    async fn root_workspace_turns_only_fork_to_a_new_worktree() {
        let f = forkable().await;
        let (sid, turns) = ran_two(&f).await;
        let source = resolve(&f.db.conn, &sid).await.unwrap();
        let root_folder = workspace::resolve(&f.db.conn, &f.root_wid).await.unwrap();
        let row = conversation::Model {
            folder_id: root_folder.id,
            ..source
        };

        let targets = fork_targets(&f.db, &row, &turns).await;

        assert_eq!(targets.get("turn-3"), Some(&vec![ForkTarget::NewWorktree]));
    }

    #[tokio::test]
    async fn folder_outside_git_only_forks_in_place() {
        let db = fresh_in_memory_db().await;
        let wid = crate::session::orchestration_tests::workspace(&db).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = crate::session::start(
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
        let row = resolve(&db.conn, &sid).await.unwrap();

        let targets = fork_targets(&db, &row, &transcript(&["one"])).await;

        assert_eq!(targets.get("turn-1"), Some(&vec![ForkTarget::Current]));
    }
}
