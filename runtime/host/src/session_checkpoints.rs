//! Per-turn checkpoints of agent sessions: a start checkpoint before every
//! prompt reaches its agent, and an end checkpoint when the turn ends.
//!
//! The start checkpoint is taken synchronously on the prompt path, so no
//! agent tool can run before it exists (or before its skip is recorded). The
//! end checkpoint follows the turn's end: a turn-end event (any stop reason),
//! the agent disconnecting, the prompt never being delivered, or the session
//! being stopped. A Host that dies mid-turn leaves the start alone; the
//! session's next turn reports it as interrupted.
//!
//! A delegated child checkpoints under its root session, so every turn in a
//! session's tree shares one numbering. What each turn changed is logged
//! here and not sent to clients. The Git side is
//! [`crate::git_harness::checkpoint`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use futures::future::{BoxFuture, FutureExt, Shared};
use sea_orm::{DatabaseConnection, EntityTrait};
use tokio::sync::broadcast::error::RecvError;

use crate::acp::types::{AcpEvent, ConnectionStatus, EventEnvelope, PromptInputBlock};
use crate::app_state::AppState;
use crate::git_harness::checkpoint::{self, CheckpointKey, Phase, Saved};

/// Turns of checkpoints kept per session.
const KEEP_TURNS: u32 = 50;
/// How far up a delegation chain to look for the session a child belongs to.
const MAX_DELEGATION_DEPTH: usize = 8;

/// The open turns, one per connection, with their start checkpoints. Cheap to
/// clone; clones share state.
#[derive(Clone, Default)]
pub struct TurnCheckpoints(Arc<Mutex<Inner>>);

#[derive(Default)]
struct Inner {
    /// By connection id.
    open: HashMap<String, OpenTurn>,
    /// End checkpoints being written.
    closing: Vec<Closing>,
    /// The last turn number handed out, by session id. Keeps two children of
    /// one session from taking the same number.
    last_ordinal: HashMap<String, u32>,
    /// Tests: the next start checkpoint signals the first and waits for the
    /// second before it is saved.
    #[cfg(test)]
    save_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}

struct OpenTurn {
    conversation_id: i32,
    root: PathBuf,
    key: CheckpointKey,
    /// The connection's event sequence before the prompt was sent: a turn-end
    /// event at or below it belongs to an earlier turn.
    start_seq: u64,
    /// Why the start checkpoint was not taken, if it was not.
    start_skipped: Option<&'static str>,
}

struct Closing {
    connection_id: String,
    conversation_id: i32,
    root: PathBuf,
    done: Shared<BoxFuture<'static, ()>>,
}

impl TurnCheckpoints {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Take the start checkpoint of the turn `blocks` is about to begin on
    /// `connection_id`. Returns once it is saved or skipped; never fails.
    pub(crate) async fn before_prompt(
        &self,
        db: &DatabaseConnection,
        connection_id: &str,
        conversation_id: Option<i32>,
        working_dir: Option<&Path>,
        start_seq: u64,
        blocks: &[PromptInputBlock],
    ) {
        let (Some(conversation_id), Some(root)) = (conversation_id, working_dir) else {
            return;
        };
        self.settle(connection_id, conversation_id).await;
        if !crate::git_repo::is_git_repo(root) {
            tracing::debug!(
                conversation_id,
                reason = "not_git",
                "[zwiim] turn checkpoint skipped"
            );
            return;
        }
        let Some(session_id) = session_of(db, conversation_id).await else {
            return;
        };

        // Without the stored turns the next number would be a guess, and a
        // guess could reuse one: skip this turn's checkpoints instead.
        let stored = match checkpoint::list(root, &session_id).await {
            Ok(stored) => stored,
            Err(reason) => {
                tracing::warn!(
                    session_id = %session_id,
                    reason = reason.as_str(),
                    detail = ?reason,
                    "[zwiim] turn checkpoint skipped: stored checkpoints unreadable"
                );
                return;
            }
        };
        let stored_last = stored.iter().map(|c| c.key.ordinal).max().unwrap_or(0);
        let (ordinal, first_since_start) = {
            let mut inner = self.lock();
            let first = !inner.last_ordinal.contains_key(&session_id);
            let last = inner.last_ordinal.entry(session_id.clone()).or_insert(0);
            *last = (*last).max(stored_last) + 1;
            (*last, first)
        };
        let unfinished = stored_last > 0
            && !stored
                .iter()
                .any(|c| c.key.ordinal == stored_last && c.phase == Phase::End);
        if first_since_start && unfinished {
            tracing::info!(
                session_id = %session_id,
                turn = stored_last,
                "[zwiim] previous turn interrupted: it has no end checkpoint"
            );
        }

        let key = CheckpointKey {
            session_id,
            ordinal,
        };
        let fingerprint =
            prompt_text(blocks).map(|t| crate::acp::fork::fingerprint_agent_message(&t));
        #[cfg(test)]
        {
            let gate = self.lock().save_gate.take();
            if let Some((reached, resume)) = gate {
                reached.notify_one();
                resume.notified().await;
            }
        }
        let saved = checkpoint::save(root, &key, Phase::Start, fingerprint.as_deref()).await;
        log_saved(&key, Phase::Start, &saved);
        let start_skipped = match &saved {
            Saved::Ref { .. } => None,
            Saved::Skipped(reason) => Some(reason.as_str()),
        };

        let cap_root = root.to_path_buf();
        let cap_session = key.session_id.clone();
        self.lock().open.insert(
            connection_id.to_string(),
            OpenTurn {
                conversation_id,
                root: root.to_path_buf(),
                key,
                start_seq,
                start_skipped,
            },
        );
        tokio::spawn(async move { checkpoint::cap(&cap_root, &cap_session, KEEP_TURNS).await });
    }

    /// Pause the next start checkpoint just before it is saved: the first
    /// `Notify` fires when it gets there, the second lets it go on.
    #[cfg(test)]
    pub(crate) fn pause_before_save(&self) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
        let gate = (Arc::default(), Arc::default());
        self.lock().save_gate = Some(gate.clone());
        gate
    }

    /// The prompt never reached the agent: close its turn now.
    pub(crate) async fn prompt_not_sent(&self, connection_id: &str) {
        let turn = self.lock().open.remove(connection_id);
        if let Some(turn) = turn {
            self.close(connection_id, turn, "not_sent").await;
        }
    }

    /// Follow one event from the Host's event bus: a turn-end event or a
    /// disconnect ends that connection's open turn.
    pub(crate) fn observe(&self, envelope: &EventEnvelope) {
        let reason = match &envelope.payload {
            AcpEvent::TurnComplete { stop_reason, .. } => stop_reason.as_str(),
            AcpEvent::StatusChanged {
                status: ConnectionStatus::Disconnected,
            } => "disconnected",
            _ => return,
        };
        let turn = {
            let mut inner = self.lock();
            match inner.open.get(&envelope.connection_id) {
                Some(turn) if envelope.seq > turn.start_seq => {
                    inner.open.remove(&envelope.connection_id)
                }
                _ => None,
            }
        };
        if let Some(turn) = turn {
            // Not awaited: the tracker must keep reading the bus. Anyone who
            // needs the end checkpoint waits on it through `closing`.
            drop(self.close(&envelope.connection_id, turn, reason));
        }
    }

    /// End every open turn of `conversation_id` and wait until all its end
    /// checkpoints are written.
    pub(crate) async fn close_conversation(&self, conversation_id: i32, reason: &str) {
        let turns = self.take_open(|_, t| t.conversation_id == conversation_id);
        for (connection_id, turn) in turns {
            drop(self.close(&connection_id, turn, reason));
        }
        self.wait_closing(|c| c.conversation_id == conversation_id)
            .await;
    }

    /// Drop `conversation_id`'s open turns without checkpointing them and wait
    /// for its pending end checkpoints, so nothing writes its refs afterwards.
    /// Returns every repository its turns were in.
    pub(crate) async fn forget_conversation(&self, conversation_id: i32) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self
            .take_open(|_, t| t.conversation_id == conversation_id)
            .into_iter()
            .map(|(_, t)| {
                self.lock().last_ordinal.remove(&t.key.session_id);
                t.root
            })
            .collect();
        roots.extend(
            self.wait_closing(|c| c.conversation_id == conversation_id)
                .await,
        );
        roots.sort();
        roots.dedup();
        roots
    }

    /// Before a new turn: end turns this one supersedes (the connection's
    /// previous turn whose end was never seen, or the conversation's turn on
    /// a connection that is gone), and wait for their end checkpoints.
    async fn settle(&self, connection_id: &str, conversation_id: i32) {
        let stale =
            self.take_open(|id, t| id == connection_id || t.conversation_id == conversation_id);
        for (id, turn) in stale {
            let reason = if id == connection_id {
                "unobserved"
            } else {
                "interrupted"
            };
            drop(self.close(&id, turn, reason));
        }
        self.wait_closing(|c| {
            c.connection_id == connection_id || c.conversation_id == conversation_id
        })
        .await;
    }

    fn take_open(&self, pick: impl Fn(&str, &OpenTurn) -> bool) -> Vec<(String, OpenTurn)> {
        let mut inner = self.lock();
        let ids: Vec<String> = inner
            .open
            .iter()
            .filter(|(id, t)| pick(id, t))
            .map(|(id, _)| id.clone())
            .collect();
        ids.into_iter()
            .filter_map(|id| inner.open.remove(&id).map(|t| (id, t)))
            .collect()
    }

    /// Start writing `turn`'s end checkpoint. The returned future resolves
    /// when it is written; dropping it does not cancel the write.
    fn close(
        &self,
        connection_id: &str,
        turn: OpenTurn,
        reason: &str,
    ) -> Shared<BoxFuture<'static, ()>> {
        let reason = reason.to_string();
        let conversation_id = turn.conversation_id;
        let root = turn.root.clone();
        let handle = tokio::spawn(end_turn(turn, reason));
        let done = handle.map(|_| ()).boxed().shared();
        let mut inner = self.lock();
        inner.closing.retain(|c| c.done.peek().is_none());
        inner.closing.push(Closing {
            connection_id: connection_id.to_string(),
            conversation_id,
            root,
            done: done.clone(),
        });
        done
    }

    /// Wait for the pending end checkpoints `pick` selects; returns their
    /// repositories.
    async fn wait_closing(&self, pick: impl Fn(&Closing) -> bool) -> Vec<PathBuf> {
        let pending: Vec<(PathBuf, Shared<BoxFuture<'static, ()>>)> = {
            let inner = self.lock();
            inner
                .closing
                .iter()
                .filter(|c| pick(c))
                .map(|c| (c.root.clone(), c.done.clone()))
                .collect()
        };
        let mut roots = Vec::with_capacity(pending.len());
        for (root, done) in pending {
            done.await;
            roots.push(root);
        }
        roots
    }
}

/// Save `turn`'s end checkpoint and log what the turn changed.
async fn end_turn(turn: OpenTurn, reason: String) {
    let saved = checkpoint::save(&turn.root, &turn.key, Phase::End, None).await;
    log_saved(&turn.key, Phase::End, &saved);
    let interrupted = reason != "end_turn";
    let skipped = match (&turn.start_skipped, &saved) {
        (Some(why), _) => Some(format!("start: {why}")),
        (None, Saved::Skipped(why)) => Some(format!("end: {}", why.as_str())),
        (None, Saved::Ref { .. }) => None,
    };
    if let Some(skipped) = skipped {
        tracing::info!(
            session_id = %turn.key.session_id,
            turn = turn.key.ordinal,
            reason = %reason,
            interrupted,
            skipped = %skipped,
            "[zwiim] turn changes unavailable"
        );
        return;
    }
    match checkpoint::changes(&turn.root, &turn.key).await {
        Some(changes) => {
            tracing::info!(
                session_id = %turn.key.session_id,
                turn = turn.key.ordinal,
                reason = %reason,
                interrupted,
                files = changes.file_count,
                added = changes.added,
                deleted = changes.deleted,
                truncated = changes.truncated,
                "[zwiim] turn changes"
            );
            tracing::debug!(
                session_id = %turn.key.session_id,
                turn = turn.key.ordinal,
                files = ?changes.files,
                "[zwiim] turn changed files"
            );
        }
        None => tracing::info!(
            session_id = %turn.key.session_id,
            turn = turn.key.ordinal,
            reason = %reason,
            interrupted,
            skipped = "diff_failed",
            "[zwiim] turn changes unavailable"
        ),
    }
}

fn log_saved(key: &CheckpointKey, phase: Phase, saved: &Saved) {
    let phase = match phase {
        Phase::Start => "start",
        Phase::End => "end",
    };
    match saved {
        Saved::Ref { .. } => tracing::debug!(
            session_id = %key.session_id,
            turn = key.ordinal,
            phase,
            "[zwiim] turn checkpoint saved"
        ),
        Saved::Skipped(reason) => tracing::info!(
            session_id = %key.session_id,
            turn = key.ordinal,
            phase,
            reason = reason.as_str(),
            detail = ?reason,
            "[zwiim] turn checkpoint skipped"
        ),
    }
}

/// The text of a prompt, as its text blocks run together; `None` when it has
/// none worth fingerprinting.
fn prompt_text(blocks: &[PromptInputBlock]) -> Option<String> {
    let text: String = blocks
        .iter()
        .filter_map(|b| match b {
            PromptInputBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    (!text.trim().is_empty()).then_some(text)
}

/// The session `conversation_id` belongs to: its own, or for a delegated
/// child, the nearest ancestor's.
async fn session_of(db: &DatabaseConnection, conversation_id: i32) -> Option<String> {
    let mut id = conversation_id;
    for _ in 0..MAX_DELEGATION_DEPTH {
        let row = crate::db::entities::conversation::Entity::find_by_id(id)
            .one(db)
            .await
            .ok()??;
        if let Some(session_id) = row.session_id {
            return Some(session_id);
        }
        id = row.parent_id?;
    }
    None
}

/// End open turns as the Host's event bus reports their ends.
pub fn spawn_tracker(state: Arc<AppState>) {
    let mut rx = state.acp_event_bus.subscribe();
    let turns = state.connection_manager.turn_checkpoints.clone();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(envelope) => turns.observe(&envelope),
                // A missed turn end is closed by the connection's next prompt
                // or by stopping the session.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            }
        }
    });
}
