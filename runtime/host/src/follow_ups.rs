//! Zwiim-owned: follow-ups, the prompts a user sends while a session's turn is
//! running.
//!
//! A follow-up sent to an idle session with nothing queued goes out at once.
//! Otherwise it joins the session's queue, saved on its row so every device
//! sees it and a Host restart keeps it. When a turn ends normally the Host
//! sends the first one; a turn that is stopped or fails holds the queue until
//! the user sends one again (`send_now`), so nothing goes out behind an
//! error the user hasn't seen.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sea_orm::EntityTrait;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;

use crate::acp::error::AcpError;
use crate::acp::manager::ConnectionManager;
use crate::acp::types::{AcpEvent, PromptInputBlock};
use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::db::entities::conversation;
use crate::db::service::conversation_service;
use crate::db::AppDatabase;
use crate::protocol::error::ZwiimError;
use crate::session::{connection_for, resolve, send_locked, session_lock, AgentLauncher};
use crate::web::event_bridge::EventEmitter;

/// One queued follow-up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FollowUp {
    pub id: String,
    /// What the user typed; the part an edit replaces.
    pub text: String,
    /// The prompt's other blocks (images, the attachment note), sent after
    /// `text`.
    #[serde(default)]
    pub rest: Vec<PromptInputBlock>,
    /// How many images and files go with it.
    #[serde(default)]
    pub attachments: usize,
    pub created_at: DateTime<Utc>,
}

impl FollowUp {
    pub fn new(text: String, rest: Vec<PromptInputBlock>, attachments: usize) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            text,
            rest,
            attachments,
            created_at: Utc::now(),
        }
    }

    fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.rest.is_empty()
    }

    fn blocks(&self) -> Vec<PromptInputBlock> {
        let typed = (!self.text.trim().is_empty()).then(|| PromptInputBlock::Text {
            text: self.text.clone(),
        });
        typed.into_iter().chain(self.rest.iter().cloned()).collect()
    }
}

/// What a client sees of a queued follow-up.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Queued {
    pub follow_up_id: String,
    pub text: String,
    pub attachments: usize,
}

impl From<&FollowUp> for Queued {
    fn from(f: &FollowUp) -> Self {
        Self {
            follow_up_id: f.id.clone(),
            text: f.text.clone(),
            attachments: f.attachments,
        }
    }
}

/// A row's saved queue; a record that no longer decodes is logged and read
/// as nothing queued.
pub(crate) fn decode(conversation_id: i32, json: &str) -> Vec<FollowUp> {
    serde_json::from_str(json)
        .inspect_err(|e| {
            tracing::warn!(conversation_id, error = %e, "unreadable follow-up queue");
        })
        .unwrap_or_default()
}

/// What became of a follow-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// It went out as the session's next turn.
    Sent,
    /// It waits in the session's queue.
    Queued,
    /// It went into the agent's running turn.
    Steered,
}

fn not_queued() -> ZwiimError {
    ZwiimError::not_found("That follow-up is no longer queued")
}

fn empty() -> ZwiimError {
    ZwiimError::invalid_input("A follow-up needs text or an attachment")
}

/// Whether `row`'s agent is in a turn.
async fn turn_running(connections: &ConnectionManager, row: &conversation::Model) -> bool {
    let Some(connection_id) = connection_for(connections, row).await else {
        return false;
    };
    match connections.get_state(&connection_id).await {
        Some(state) => state.read().await.turn_in_flight,
        None => false,
    }
}

async fn load(db: &AppDatabase, conversation_id: i32) -> Result<Vec<FollowUp>, ZwiimError> {
    Ok(conversation_service::follow_ups(&db.conn, conversation_id)
        .await
        .map_err(AppCommandError::from)?)
}

/// Save `queue` and tell every device the session changed.
async fn save(
    db: &AppDatabase,
    emitter: &EventEmitter,
    conversation_id: i32,
    queue: &[FollowUp],
) -> Result<(), ZwiimError> {
    conversation_service::set_follow_ups(&db.conn, conversation_id, queue)
        .await
        .map_err(AppCommandError::from)?;
    crate::commands::conversations::emit_conversation_upsert(emitter, &db.conn, conversation_id)
        .await;
    Ok(())
}

/// Send `follow_up` to session `session_id` now if its agent is idle and
/// nothing is queued, else queue it behind what is.
pub async fn add<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    emitter: &EventEmitter,
    session_id: &str,
    follow_up: FollowUp,
    client_message_id: Option<String>,
) -> Result<Outcome, ZwiimError> {
    if follow_up.is_empty() {
        return Err(empty());
    }
    let row = resolve(&db.conn, session_id).await?;
    let lock = session_lock(session_id);
    let _guard = lock.lock().await;
    let mut queue = load(db, row.id).await?;
    if queue.is_empty() && !turn_running(connections, &row).await {
        let sent = send_locked(
            db,
            connections,
            launcher,
            &row,
            follow_up.blocks(),
            client_message_id,
            None,
        )
        .await;
        match sent {
            Ok(()) => return Ok(Outcome::Sent),
            // A turn began without the session lock (an agent's own turn):
            // queue behind it.
            Err(e) if e.code == "turn_in_progress" => {}
            Err(e) => return Err(e),
        }
    }
    queue.push(follow_up);
    save(db, emitter, row.id, &queue).await?;
    Ok(Outcome::Queued)
}

/// Put `follow_up` into the running turn of session `session_id` when its
/// agent takes messages mid-turn, else `add` it: sent now when idle, queued
/// while a turn runs.
pub async fn steer<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    emitter: &EventEmitter,
    session_id: &str,
    follow_up: FollowUp,
    client_message_id: Option<String>,
) -> Result<Outcome, ZwiimError> {
    if follow_up.is_empty() {
        return Err(empty());
    }
    let row = resolve(&db.conn, session_id).await?;
    // Attachments alone carry no note to steer with; they wait their turn.
    let target = if follow_up.text.trim().is_empty() {
        None
    } else {
        steerable(connections, &row).await
    };
    if let Some(connection_id) = target {
        // The agent gets every block; the note keeps the typed text.
        let blocks = (!follow_up.rest.is_empty()).then(|| follow_up.blocks());
        match connections
            .submit_feedback(&connection_id, follow_up.text.clone(), blocks)
            .await
        {
            Ok(_) => return Ok(Outcome::Steered),
            // The turn ended first, or the agent turned it down: it joins
            // the queue unchanged.
            Err(AcpError::NoActiveTurn) => {}
            Err(e) => {
                tracing::warn!(session_id, error = %e, "[zwiim] steer failed; queueing instead");
            }
        }
    }
    add(
        db,
        connections,
        launcher,
        emitter,
        session_id,
        follow_up,
        client_message_id,
    )
    .await
}

/// `row`'s connection, when its agent is in a turn it takes messages into.
/// Only the agent's own steering counts: a note left for the agent to fetch
/// could still be unread when the turn ends.
async fn steerable(connections: &ConnectionManager, row: &conversation::Model) -> Option<String> {
    let connection_id = connection_for(connections, row).await?;
    let state = connections.get_state(&connection_id).await?;
    let s = state.read().await;
    (s.turn_in_flight && s.native_steering_available).then_some(connection_id)
}

/// Replace the typed text of queued follow-up `id`.
pub async fn edit(
    db: &AppDatabase,
    emitter: &EventEmitter,
    session_id: &str,
    id: &str,
    text: String,
) -> Result<(), ZwiimError> {
    let row = resolve(&db.conn, session_id).await?;
    let lock = session_lock(session_id);
    let _guard = lock.lock().await;
    let mut queue = load(db, row.id).await?;
    let item = queue
        .iter_mut()
        .find(|f| f.id == id)
        .ok_or_else(not_queued)?;
    item.text = text;
    if item.is_empty() {
        return Err(empty());
    }
    save(db, emitter, row.id, &queue).await
}

/// Drop queued follow-up `id`. One already gone is not an error: it was sent
/// or removed from another device.
pub async fn remove(
    db: &AppDatabase,
    emitter: &EventEmitter,
    session_id: &str,
    id: &str,
) -> Result<(), ZwiimError> {
    let row = resolve(&db.conn, session_id).await?;
    let lock = session_lock(session_id);
    let _guard = lock.lock().await;
    let mut queue = load(db, row.id).await?;
    let before = queue.len();
    queue.retain(|f| f.id != id);
    if queue.len() == before {
        return Ok(());
    }
    save(db, emitter, row.id, &queue).await
}

/// Send queued follow-up `id` now if the agent is idle, which also resumes a
/// held queue: the rest follow as turns end. While a turn runs it moves to
/// the front, to go next.
pub async fn send_now<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    emitter: &EventEmitter,
    session_id: &str,
    id: &str,
) -> Result<Outcome, ZwiimError> {
    let row = resolve(&db.conn, session_id).await?;
    let lock = session_lock(session_id);
    let _guard = lock.lock().await;
    let mut queue = load(db, row.id).await?;
    let at = queue
        .iter()
        .position(|f| f.id == id)
        .ok_or_else(not_queued)?;
    let item = queue.remove(at);
    if !turn_running(connections, &row).await {
        let sent = send_locked(
            db,
            connections,
            launcher,
            &row,
            item.blocks(),
            Some(item.id.clone()),
            None,
        )
        .await;
        match sent {
            Ok(()) => {
                save(db, emitter, row.id, &queue).await?;
                return Ok(Outcome::Sent);
            }
            Err(e) if e.code == "turn_in_progress" => {}
            Err(e) => return Err(e),
        }
    }
    queue.insert(0, item);
    save(db, emitter, row.id, &queue).await?;
    Ok(Outcome::Queued)
}

/// Send the first follow-up queued on conversation `conversation_id`, if its
/// agent is idle. A follow-up that fails to go out stays first in line.
pub(crate) async fn drain<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    emitter: &EventEmitter,
    conversation_id: i32,
) {
    let row = match conversation::Entity::find_by_id(conversation_id)
        .one(&db.conn)
        .await
    {
        Ok(Some(row)) if row.deleted_at.is_none() => row,
        _ => return,
    };
    let Some(session_id) = row.session_id.clone() else {
        return;
    };
    let lock = session_lock(&session_id);
    let _guard = lock.lock().await;
    let Ok(mut queue) = load(db, row.id).await else {
        return;
    };
    if queue.is_empty() || turn_running(connections, &row).await {
        return;
    }
    let next = queue[0].clone();
    let sent = send_locked(
        db,
        connections,
        launcher,
        &row,
        next.blocks(),
        Some(next.id.clone()),
        None,
    )
    .await;
    if let Err(e) = sent {
        tracing::warn!(
            session_id,
            code = e.code,
            "[zwiim] queued follow-up could not be sent; it stays queued"
        );
        return;
    }
    queue.remove(0);
    if let Err(e) = save(db, emitter, row.id, &queue).await {
        tracing::warn!(
            session_id,
            code = e.code,
            "[zwiim] could not save the follow-up queue"
        );
    }
}

/// Send each session's next follow-up as its turn ends normally.
pub fn spawn_tracker(state: Arc<AppState>) {
    let mut rx = state.acp_event_bus.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(envelope) => {
                    let AcpEvent::TurnComplete { stop_reason, .. } = &envelope.payload else {
                        continue;
                    };
                    // Stopped, refused or failed: hold the queue for the user.
                    if stop_reason != "end_turn" {
                        continue;
                    }
                    let state = state.clone();
                    let connection_id = envelope.connection_id.clone();
                    tokio::spawn(async move { after_turn(&state, &connection_id).await });
                }
                // A missed turn end leaves the queue held; sending one resumes it.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            }
        }
    });
}

async fn after_turn(state: &AppState, connection_id: &str) {
    let Some(live) = state.connection_manager.get_state(connection_id).await else {
        return;
    };
    let Some(conversation_id) = live.read().await.conversation_id else {
        return;
    };
    drain(
        &state.db,
        &state.connection_manager,
        &crate::session::RuntimeLauncher { state },
        &state.emitter,
        conversation_id,
    )
    .await;
}
