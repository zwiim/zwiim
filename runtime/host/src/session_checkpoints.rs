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
//!
//! [`match_turns`] ties a session's user turns to their start checkpoints:
//! it decides which turns can be reverted and which checkpoint a revert
//! restores.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use futures::future::{BoxFuture, FutureExt, Shared};
use sea_orm::{DatabaseConnection, EntityTrait};
use tokio::sync::broadcast::error::RecvError;

use crate::acp::types::{AcpEvent, ConnectionStatus, EventEnvelope, PromptInputBlock};
use crate::app_state::AppState;
use crate::git_harness::checkpoint::{self, Checkpoint, CheckpointKey, Phase, Saved};
use crate::models::message::{ContentBlock, MessageTurn, TurnRole};

/// Turns of checkpoints kept per session.
const KEEP_TURNS: u32 = 50;
/// How far up a delegation chain to look for the session a child belongs to.
const MAX_DELEGATION_DEPTH: usize = 8;

/// The open turns, one per connection, with their start checkpoints. Cheap to
/// clone; clones share state.
#[derive(Clone, Default)]
pub struct TurnCheckpoints(Arc<Mutex<Inner>>);

/// The checkpoints a chat rewind makes stale: those of session `session_id`
/// at `root` from turn `from` on (see [`rewind_from`]).
#[derive(Debug, Clone)]
pub(crate) struct RewindCut {
    pub root: PathBuf,
    pub session_id: String,
    pub from: u32,
}

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
        action: Option<&'static str>,
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
        let fingerprint = sent_prompt_fingerprint(blocks);
        #[cfg(test)]
        {
            let gate = self.lock().save_gate.take();
            if let Some((reached, resume)) = gate {
                reached.notify_one();
                resume.notified().await;
            }
        }
        let saved =
            checkpoint::save_tagged(root, &key, Phase::Start, fingerprint.as_deref(), action).await;
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

    /// After `conversation_id`'s chat went back to before turn `cut.from`:
    /// end its open turns, then forget that turn and every later one, so the
    /// checkpoints keep matching the conversation. Turn numbers keep counting
    /// up; restore points stay.
    pub(crate) async fn rewind(&self, conversation_id: i32, cut: &RewindCut) {
        self.close_conversation(conversation_id, "interrupted")
            .await;
        checkpoint::drop_from(&cut.root, &cut.session_id, cut.from).await;
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

/// The fingerprint a sent prompt's start checkpoint carries.
fn sent_prompt_fingerprint(blocks: &[PromptInputBlock]) -> Option<String> {
    prompt_fingerprint(blocks.iter().filter_map(|b| match b {
        PromptInputBlock::Text { text } => Some(text.as_str()),
        _ => None,
    }))
}

/// The fingerprint a user turn's start checkpoint carries, taken the same
/// way as [`sent_prompt_fingerprint`] takes it from the prompt that was sent.
fn user_turn_fingerprint(turn: &MessageTurn) -> Option<String> {
    if !matches!(turn.role, TurnRole::User) {
        return None;
    }
    prompt_fingerprint(turn.blocks.iter().filter_map(|b| match b {
        ContentBlock::Text { text } => Some(text.as_str()),
        _ => None,
    }))
}

/// A prompt's text parts, fingerprinted in one canonical form: every run of
/// whitespace, part boundaries included, as one space, with none at either
/// end. Agents do not keep a prompt byte for byte: Codex and Claude trim it,
/// Codex collapses runs of spaces, and either may join parts with a newline.
/// `None` when there is no text.
fn prompt_fingerprint<'a>(parts: impl Iterator<Item = &'a str>) -> Option<String> {
    let canonical = parts
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    (!canonical.is_empty()).then(|| crate::acp::fork::fingerprint_agent_message(&canonical))
}

/// Which user turns have a start checkpoint that is certainly theirs:
/// `turn id -> index into starts`. `turns` is the session's transcript,
/// oldest first; `starts` its start checkpoints, oldest first.
///
/// Neither list is a copy of the other. Turns sent before checkpoints
/// existed, or whose checkpoints were capped away, come first and have none.
/// A checkpoint may have no turn in the transcript (a delegated agent's
/// prompt, a prompt that never reached the agent), and a turn may have no
/// checkpoint (its save was skipped). So the two are aligned from the newest
/// end, one prompt fingerprint against the other, one step at a time:
///
/// 1. Same fingerprint: the checkpoint is the turn's. Step past both.
/// 2. Otherwise, if an older turn has the checkpoint's fingerprint, the
///    current turn has no checkpoint. Step past the turn.
/// 3. Otherwise the checkpoint belongs to no turn in the transcript. Step
///    past the checkpoint.
///
/// A turn or checkpoint with no fingerprint (a prompt with no text) is
/// stepped past without matching. Then, so that no guess survives: matches
/// in a row that share one fingerprint are dropped together when a turn or
/// checkpoint left unmatched, with that fingerprint, sits between the
/// matches just outside that row. With a repeated prompt and a missing
/// checkpoint there is no telling which copy it was missing from, so none of
/// them is revertible.
pub(crate) fn match_turns(turns: &[MessageTurn], starts: &[Checkpoint]) -> HashMap<String, usize> {
    let users: Vec<(&str, Option<String>)> = turns
        .iter()
        .filter(|t| matches!(t.role, TurnRole::User))
        .map(|t| (t.id.as_str(), user_turn_fingerprint(t)))
        .collect();
    let cps: Vec<Option<&str>> = starts
        .iter()
        .map(|c| c.prompt_fingerprint.as_deref())
        .collect();

    // Matched `(user index, checkpoint index)`, newest first.
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let (mut i, mut j) = (users.len(), cps.len());
    while i > 0 && j > 0 {
        let Some(u) = users[i - 1].1.as_deref() else {
            i -= 1;
            continue;
        };
        let Some(c) = cps[j - 1] else {
            j -= 1;
            continue;
        };
        if u == c {
            pairs.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if users[..i - 1]
            .iter()
            .any(|(_, fp)| fp.as_deref() == Some(c))
        {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    pairs.reverse();

    let matched_users: HashSet<usize> = pairs.iter().map(|p| p.0).collect();
    let matched_cps: HashSet<usize> = pairs.iter().map(|p| p.1).collect();
    let mut out = HashMap::new();
    let mut run_start = 0;
    while run_start < pairs.len() {
        let fp = cps[pairs[run_start].1];
        let mut run_end = run_start + 1;
        while run_end < pairs.len() && cps[pairs[run_end].1] == fp {
            run_end += 1;
        }
        // The matches just outside the row bound where its turns could
        // have gone instead.
        let (lo_u, lo_c) = match run_start {
            0 => (None, None),
            k => (Some(pairs[k - 1].0), Some(pairs[k - 1].1)),
        };
        let (hi_u, hi_c) = pairs
            .get(run_end)
            .map_or((users.len(), cps.len()), |p| (p.0, p.1));
        let between =
            |idx: usize, lo: Option<usize>, hi: usize| lo.is_none_or(|lo| idx > lo) && idx < hi;
        let ambiguous = users.iter().enumerate().any(|(k, (_, f))| {
            !matched_users.contains(&k) && f.as_deref() == fp && between(k, lo_u, hi_u)
        }) || cps
            .iter()
            .enumerate()
            .any(|(k, f)| !matched_cps.contains(&k) && *f == fp && between(k, lo_c, hi_c));
        if !ambiguous {
            for &(u, c) in &pairs[run_start..run_end] {
                out.insert(users[u].0.to_string(), c);
            }
        }
        run_start = run_end;
    }
    out
}

/// The start checkpoints stored for `session_id` at `root`, oldest first.
/// Empty when there are none or they cannot be read: no checkpoint is ever
/// guessed at.
async fn start_checkpoints(root: &Path, session_id: &str) -> Vec<Checkpoint> {
    match checkpoint::list(root, session_id).await {
        Ok(all) => all
            .into_iter()
            .filter(|c| c.phase == Phase::Start)
            .collect(),
        Err(checkpoint::SkipReason::NotGit) => Vec::new(),
        Err(reason) => {
            tracing::warn!(
                session_id = %session_id,
                reason = reason.as_str(),
                detail = ?reason,
                "[zwiim] checkpoints unreadable: no turn is revertible"
            );
            Vec::new()
        }
    }
}

/// The ids of the user turns in `turns` that can be reverted.
pub(crate) async fn revertible_turns(
    root: &Path,
    session_id: &str,
    turns: &[MessageTurn],
) -> HashSet<String> {
    let starts = start_checkpoints(root, session_id).await;
    match_turns(turns, &starts).into_keys().collect()
}

/// The start checkpoint user turn `turn_id` reverts to, when
/// [`match_turns`] ties one to it.
pub(crate) async fn start_checkpoint_of(
    root: &Path,
    session_id: &str,
    turns: &[MessageTurn],
    turn_id: &str,
) -> Option<Checkpoint> {
    let mut starts = start_checkpoints(root, session_id).await;
    let idx = *match_turns(turns, &starts).get(turn_id)?;
    Some(starts.swap_remove(idx))
}

/// The first turn number a chat rewind to before user turn `turn_id` makes
/// stale: that turn's own, else the one after the last earlier turn's, else
/// the first. Checkpoints of turns that never matched go with it, as they could
/// never be reverted to and would blur matching the turns sent next.
pub(crate) async fn rewind_from(
    root: &Path,
    session_id: &str,
    turns: &[MessageTurn],
    turn_id: &str,
) -> u32 {
    let starts = start_checkpoints(root, session_id).await;
    rewind_cut(turns, &starts, turn_id)
}

fn rewind_cut(turns: &[MessageTurn], starts: &[Checkpoint], turn_id: &str) -> u32 {
    let matched = match_turns(turns, starts);
    if let Some(&idx) = matched.get(turn_id) {
        return starts[idx].key.ordinal;
    }
    let earlier: HashSet<&str> = turns
        .iter()
        .take_while(|t| t.id != turn_id)
        .map(|t| t.id.as_str())
        .collect();
    matched
        .iter()
        .filter(|(id, _)| earlier.contains(id.as_str()))
        .map(|(_, &idx)| starts[idx].key.ordinal + 1)
        .max()
        .unwrap_or(1)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(text: &str) -> String {
        prompt_fingerprint(std::iter::once(text)).unwrap()
    }

    /// A transcript: a user turn per prompt, each answered. `None` is a user
    /// turn with no text.
    fn transcript(prompts: &[Option<&str>]) -> Vec<MessageTurn> {
        let turn = |n: usize, role: TurnRole, blocks: Vec<ContentBlock>| MessageTurn {
            id: format!("turn-{n}"),
            role,
            blocks,
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
            .flat_map(|(k, prompt)| {
                let blocks = match prompt {
                    Some(text) => vec![ContentBlock::Text {
                        text: (*text).into(),
                    }],
                    None => vec![ContentBlock::Image {
                        data: "aGk=".into(),
                        mime_type: "image/png".into(),
                        uri: None,
                    }],
                };
                [
                    turn(2 * k, TurnRole::User, blocks),
                    turn(
                        2 * k + 1,
                        TurnRole::Assistant,
                        vec![ContentBlock::Text {
                            text: prompt.unwrap_or("").into(),
                        }],
                    ),
                ]
            })
            .collect()
    }

    /// Start checkpoints, ordinals from 1. `None` is a prompt with no text.
    fn starts(prompts: &[Option<&str>]) -> Vec<Checkpoint> {
        prompts
            .iter()
            .enumerate()
            .map(|(k, prompt)| Checkpoint {
                key: CheckpointKey {
                    session_id: "s".into(),
                    ordinal: k as u32 + 1,
                },
                phase: Phase::Start,
                head: None,
                index_tree: "i".into(),
                worktree_tree: "w".into(),
                prompt_fingerprint: prompt.map(fp),
                action: None,
            })
            .collect()
    }

    /// `(user turn id, checkpoint ordinal)`, oldest first.
    fn matched(turns: &[Option<&str>], cps: &[Option<&str>]) -> Vec<(String, u32)> {
        let cps = starts(cps);
        let mut out: Vec<(String, u32)> = match_turns(&transcript(turns), &cps)
            .into_iter()
            .map(|(id, k)| (id, cps[k].key.ordinal))
            .collect();
        out.sort_by_key(|(id, _)| id[5..].parse::<usize>().unwrap());
        out
    }

    fn pairs(list: &[(&str, u32)]) -> Vec<(String, u32)> {
        list.iter().map(|(id, o)| (id.to_string(), *o)).collect()
    }

    #[test]
    fn a_chat_rewind_cuts_at_the_turn_or_just_after_the_last_earlier_one() {
        let cut = |turns: &[Option<&str>], cps: &[Option<&str>], turn_id: &str| {
            rewind_cut(&transcript(turns), &starts(cps), turn_id)
        };
        let abc = [Some("a"), Some("b"), Some("c")];
        assert_eq!(cut(&abc, &abc, "turn-2"), 2, "its own checkpoint");
        // `b` was never checkpointed: everything after `a`'s goes.
        assert_eq!(cut(&abc, &[Some("a"), Some("c")], "turn-2"), 2);
        assert_eq!(cut(&abc, &[Some("a"), Some("x"), Some("c")], "turn-2"), 2);
        // Nothing earlier matched: every turn goes.
        assert_eq!(cut(&abc, &[Some("x"), Some("c")], "turn-2"), 1);
        assert_eq!(cut(&abc, &[], "turn-2"), 1);
    }

    #[test]
    fn turns_from_before_checkpoints_are_not_revertible() {
        assert_eq!(
            matched(
                &[Some("old"), Some("a"), Some("b")],
                &[Some("a"), Some("b")]
            ),
            pairs(&[("turn-2", 1), ("turn-4", 2)])
        );
    }

    #[test]
    fn checkpoints_with_no_turn_are_stepped_past() {
        // A delegated agent's prompt, checkpointed under the session.
        assert_eq!(
            matched(
                &[Some("a"), Some("b")],
                &[Some("a"), Some("delegated"), Some("b")]
            ),
            pairs(&[("turn-0", 1), ("turn-2", 3)])
        );
    }

    #[test]
    fn a_turn_whose_save_was_skipped_is_not_revertible() {
        assert_eq!(
            matched(&[Some("a"), Some("b"), Some("c")], &[Some("a"), Some("c")]),
            pairs(&[("turn-0", 1), ("turn-4", 2)])
        );
    }

    #[test]
    fn repeated_prompts_each_get_their_own_checkpoint() {
        assert_eq!(
            matched(
                &[Some("go"), Some("continue"), Some("continue")],
                &[Some("go"), Some("continue"), Some("continue")]
            ),
            pairs(&[("turn-0", 1), ("turn-2", 2), ("turn-4", 3)])
        );
        // Another prompt between them tells them apart.
        assert_eq!(
            matched(
                &[Some("continue"), Some("x"), Some("continue")],
                &[Some("x"), Some("continue")]
            ),
            pairs(&[("turn-2", 1), ("turn-4", 2)])
        );
    }

    #[test]
    fn a_repeated_prompt_missing_one_checkpoint_is_never_guessed() {
        // Two "continue" turns, one checkpoint: whose is it?
        assert_eq!(
            matched(
                &[Some("a"), Some("continue"), Some("continue")],
                &[Some("a"), Some("continue")]
            ),
            pairs(&[("turn-0", 1)])
        );
        // A retry after a failed send: two checkpoints, one turn.
        assert_eq!(
            matched(&[Some("a"), Some("c")], &[Some("a"), Some("c"), Some("c")]),
            pairs(&[("turn-0", 1)])
        );
        // A failed send repeating an older prompt.
        assert_eq!(
            matched(&[Some("a"), Some("b")], &[Some("a"), Some("b"), Some("a")]),
            Vec::<(String, u32)>::new()
        );
    }

    #[test]
    fn prompts_without_text_and_different_text_never_match() {
        assert_eq!(
            matched(&[None, Some("a")], &[None, Some("a")]),
            pairs(&[("turn-2", 2)])
        );
        assert_eq!(
            matched(&[Some("x")], &[Some("y")]),
            Vec::<(String, u32)>::new()
        );
        assert_eq!(matched(&[], &[Some("y")]), Vec::<(String, u32)>::new());
        assert_eq!(matched(&[Some("x")], &[]), Vec::<(String, u32)>::new());
    }

    /// What a person types is not what the agent's transcript keeps: Codex
    /// and Claude both trim the prompt and Codex collapses runs of spaces.
    /// A turn read back through the real parser must still meet the
    /// checkpoint saved from the prompt as it was sent.
    const TYPED: &str = "  Fix the bug\n\n    in   parser.rs\twhen\n  it  panics\n";

    fn sent(text: &str) -> Checkpoint {
        let mut cp = starts(&[None]).remove(0);
        cp.prompt_fingerprint =
            sent_prompt_fingerprint(&[PromptInputBlock::Text { text: text.into() }]);
        cp
    }

    fn assert_meets_checkpoint(turns: &[MessageTurn]) {
        let user = turns
            .iter()
            .find(|t| matches!(t.role, TurnRole::User))
            .expect("the parser keeps the user turn");
        assert_eq!(
            match_turns(turns, &[sent(TYPED)]).get(&user.id),
            Some(&0),
            "the parsed turn must meet the checkpoint saved from the prompt sent"
        );
    }

    #[test]
    fn a_codex_turn_meets_the_checkpoint_of_the_prompt_as_typed() {
        use crate::parsers::AgentParser;
        let dir = tempfile::tempdir().unwrap();
        let id = "01a0626c-c601-78f1-a13d-2b26dd168501";
        let line = |ts: &str, kind: &str, payload: serde_json::Value| {
            serde_json::json!({ "timestamp": ts, "type": kind, "payload": payload }).to_string()
        };
        let lines = [
            line(
                "2026-10-19T10:00:00Z",
                "session_meta",
                serde_json::json!({"id": id, "cwd": "/tmp/demo"}),
            ),
            line(
                "2026-10-19T10:00:01Z",
                "event_msg",
                serde_json::json!({"type": "user_message", "message": TYPED}),
            ),
            line(
                "2026-10-19T10:00:02Z",
                "event_msg",
                serde_json::json!({"type": "agent_message", "message": "Fixed."}),
            ),
        ];
        std::fs::write(
            dir.path()
                .join(format!("rollout-2026-10-19T10-00-00-{id}.jsonl")),
            format!("{}\n", lines.join("\n")),
        )
        .unwrap();

        let parser = crate::parsers::codex::CodexParser::with_base_dir(dir.path().to_path_buf());
        assert_meets_checkpoint(&parser.get_conversation(id).unwrap().turns);
    }

    #[test]
    fn a_claude_turn_meets_the_checkpoint_of_the_prompt_as_typed() {
        use crate::parsers::AgentParser;
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("-tmp-demo");
        std::fs::create_dir_all(&project).unwrap();
        let lines = [
            serde_json::json!({
                "type": "user",
                "timestamp": "2026-10-19T10:00:01.000Z",
                "uuid": "u1",
                "cwd": "/tmp/demo",
                "message": {"role": "user", "content": [{"type": "text", "text": TYPED}]}
            })
            .to_string(),
            serde_json::json!({
                "type": "assistant",
                "timestamp": "2026-10-19T10:00:02.000Z",
                "uuid": "a1",
                "message": {
                    "id": "m-a1",
                    "role": "assistant",
                    "model": "claude-opus-5",
                    "content": [{"type": "text", "text": "Fixed."}]
                }
            })
            .to_string(),
        ];
        std::fs::write(project.join("sess-typed.jsonl"), lines.join("\n") + "\n").unwrap();

        let parser = crate::parsers::claude::ClaudeParser::with_base_dir(dir.path().to_path_buf());
        assert_meets_checkpoint(&parser.get_conversation("sess-typed").unwrap().turns);
    }
}
