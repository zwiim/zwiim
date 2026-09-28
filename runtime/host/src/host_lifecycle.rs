//! The Host process lifecycle: one Host per data directory, a graceful drain
//! before it exits, and restarts that wait for the Host to be idle.
//!
//! - **Instance lock.** `<data_dir>/host.lock` is held for the life of the
//!   worker, taken before the database is opened. A second Host for the same
//!   data directory exits with [`EXIT_ALREADY_RUNNING`] without touching the
//!   database, its migrations or the discovery file.
//! - **Drain.** Stopping or restarting first refuses new operations
//!   (`host_restarting`), lets operations already running finish, then ends
//!   the remaining work *visibly*: running turns are cancelled through the same
//!   path as `session.cancel` (a request waiting on the user is declined and
//!   clients see the turn end), terminals are closed (clients see them exit),
//!   agents are disconnected, and connected clients are told the Host is going
//!   away. Sessions, their transcripts and workspaces are kept; the next
//!   `session.send` resumes a session on the new process.
//! - **Restart.** `when_idle` waits until no turn runs, no request waits on
//!   the user and no terminal is open, reporting `restart_pending` with what
//!   blocks it until then. `now` is the forced restart: it drains at once, with
//!   the semantics above. A supervised worker exits with
//!   [`crate::update::runtime::EXIT_RESTART`] so the supervisor relaunches it;
//!   an unsupervised one re-executes itself.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use tokio::sync::Notify;

use crate::app_state::AppState;
use crate::db::entities::conversation;

/// Exit code of a Host that found another Host running on its data
/// directory (`EX_CANTCREAT`). Never a failed upgrade: the supervisor
/// propagates it without rolling back.
pub const EXIT_ALREADY_RUNNING: u8 = 73;

/// Exit code of a Host whose database was written by a newer Zwiim
/// (`EX_DATAERR`). The data is left untouched.
pub const EXIT_DATA_TOO_NEW: u8 = 65;

pub const LOCK_FILE: &str = "host.lock";

/// How long a drain lets operations already running finish.
const REQUEST_WAIT: Duration = if cfg!(test) {
    Duration::from_millis(500)
} else {
    Duration::from_secs(10)
};

/// How long a drain waits for cancelled turns to end before disconnecting
/// their agents regardless.
const TURN_WAIT: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(10)
};

/// How often a pending `when_idle` restart checks whether the Host is idle.
const IDLE_POLL: Duration = if cfg!(test) {
    Duration::from_millis(20)
} else {
    Duration::from_secs(1)
};

// ─── instance lock ──────────────────────────────────────────────────────

/// Exclusive ownership of a data directory, released when dropped (and by
/// the OS when the process ends, however it ends).
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

#[derive(Debug)]
pub enum LockError {
    /// Another Host holds the lock; `pid` as it recorded it, if readable.
    Held {
        pid: Option<u32>,
    },
    Io(io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Held { pid: Some(pid) } => write!(f, "another Host (pid {pid}) is running"),
            LockError::Held { pid: None } => write!(f, "another Host is running"),
            LockError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for LockError {
    fn from(e: io::Error) -> Self {
        LockError::Io(e)
    }
}

impl InstanceLock {
    /// Take the lock for `data_dir` without waiting.
    pub fn acquire(data_dir: &Path) -> Result<Self, LockError> {
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join(LOCK_FILE);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                let mut recorded = String::new();
                let pid = file
                    .read_to_string(&mut recorded)
                    .ok()
                    .and_then(|_| recorded.trim().parse().ok());
                return Err(LockError::Held { pid });
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
        // Record who holds it, for the refused Host's message.
        file.set_len(0)?;
        write!(file, "{}", std::process::id())?;
        file.flush()?;
        Ok(Self { _file: file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

// ─── lifecycle state ────────────────────────────────────────────────────

/// Why the Host is shutting down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownKind {
    Stop,
    Restart,
}

/// What keeps the Host from restarting without interrupting anyone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Blockers {
    pub live_turns: usize,
    pub pending_requests: usize,
    pub terminals: usize,
    /// Sessions whose agents are working (an agent not bound to a session
    /// yet is only counted).
    pub session_ids: Vec<String>,
}

impl Blockers {
    pub fn is_idle(&self) -> bool {
        self.live_turns == 0 && self.pending_requests == 0 && self.terminals == 0
    }
}

/// What a drain ended, for the log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DrainReport {
    pub cancelled_turns: usize,
    pub declined_requests: usize,
    pub terminals_closed: usize,
    pub agents_stopped: usize,
}

#[derive(Default)]
pub struct Lifecycle {
    draining: AtomicBool,
    restart_pending: AtomicBool,
    in_flight: AtomicUsize,
    requests_done: Notify,
    requested: Mutex<Option<ShutdownKind>>,
    request_made: Notify,
}

/// An operation in progress; the drain waits for it.
pub struct RequestGuard(Arc<Lifecycle>);

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if self.0.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.requests_done.notify_waiters();
        }
    }
}

impl Lifecycle {
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    pub fn restart_pending(&self) -> bool {
        self.restart_pending.load(Ordering::SeqCst)
    }

    /// The shutdown requested so far, if any.
    pub fn requested(&self) -> Option<ShutdownKind> {
        *self.requested.lock().unwrap()
    }

    /// Ask the Host to shut down. A stop overrides a restart not yet begun
    /// (the machine is going away); otherwise the first request stands.
    pub fn request(&self, kind: ShutdownKind) {
        let mut requested = self.requested.lock().unwrap();
        match (*requested, kind) {
            (None, _) | (Some(ShutdownKind::Restart), ShutdownKind::Stop) => {
                *requested = Some(kind);
                drop(requested);
                self.request_made.notify_waiters();
            }
            _ => {}
        }
    }

    /// Resolves with the first shutdown request.
    pub async fn wait_for_request(&self) -> ShutdownKind {
        loop {
            let made = self.request_made.notified();
            if let Some(kind) = self.requested() {
                return kind;
            }
            made.await;
        }
    }

    /// Admit an operation, or `None` once the Host is draining.
    pub fn begin_request(self: &Arc<Self>) -> Option<RequestGuard> {
        // Count first, then check: paired with the drain setting the flag
        // before reading the count, an operation is either refused or waited
        // for, never missed.
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        let guard = RequestGuard(self.clone());
        if self.is_draining() {
            return None;
        }
        Some(guard)
    }

    fn start_draining(&self) {
        self.draining.store(true, Ordering::SeqCst);
    }

    /// Wait for admitted operations to finish, at most `limit`. Returns how
    /// many were still running when it gave up.
    async fn wait_for_requests(&self, limit: Duration) -> usize {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let done = self.requests_done.notified();
            let n = self.in_flight.load(Ordering::SeqCst);
            if n == 0 {
                return 0;
            }
            if tokio::time::timeout_at(deadline, done).await.is_err() {
                return self.in_flight.load(Ordering::SeqCst);
            }
        }
    }
}

// ─── blockers, restart, drain ───────────────────────────────────────────

/// What is running on the Host right now.
pub async fn blockers(state: &AppState) -> Blockers {
    let mut b = Blockers::default();
    let mut conversation_ids = Vec::new();
    for info in state.connection_manager.list_connections().await {
        let Some(s) = state.connection_manager.get_state(&info.id).await else {
            continue;
        };
        let s = s.read().await;
        if !crate::session::is_busy(&s) {
            continue;
        }
        if s.turn_in_flight {
            b.live_turns += 1;
        }
        if s.pending_permission.is_some()
            || s.pending_question.is_some()
            || s.pending_plan_approval.is_some()
        {
            b.pending_requests += 1;
        }
        conversation_ids.extend(s.conversation_id);
    }
    b.terminals = state
        .terminal_manager
        .list_with_exit_check(Some(&state.emitter))
        .len();
    if !conversation_ids.is_empty() {
        b.session_ids = conversation::Entity::find()
            .filter(conversation::Column::Id.is_in(conversation_ids))
            .filter(conversation::Column::SessionId.is_not_null())
            .all(&state.db.conn)
            .await
            .map(|rows| rows.into_iter().filter_map(|c| c.session_id).collect())
            .unwrap_or_default();
        b.session_ids.sort();
    }
    b
}

/// Where a restart request left the Host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartState {
    /// Draining now; the Host comes back on its own.
    Restarting,
    /// Waiting for the work in `blockers` to finish.
    RestartPending,
}

/// Restart once nothing is running. Restarts at once when already idle;
/// otherwise leaves a pending restart that fires when the blockers clear.
pub async fn restart_when_idle(state: &Arc<AppState>) -> (RestartState, Blockers) {
    let lifecycle = &state.lifecycle;
    if lifecycle.requested().is_some() {
        return (RestartState::Restarting, Blockers::default());
    }
    let now = blockers(state).await;
    if now.is_idle() {
        lifecycle.request(ShutdownKind::Restart);
        return (RestartState::Restarting, now);
    }
    if !lifecycle.restart_pending.swap(true, Ordering::SeqCst) {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(IDLE_POLL).await;
                if state.lifecycle.requested().is_some() {
                    return;
                }
                if blockers(&state).await.is_idle() {
                    tracing::info!("[lifecycle] host idle; restarting as requested");
                    state.lifecycle.request(ShutdownKind::Restart);
                    return;
                }
            }
        });
    }
    tracing::info!(
        "[lifecycle] restart pending: {} turn(s), {} request(s), {} terminal(s) running",
        now.live_turns,
        now.pending_requests,
        now.terminals
    );
    (RestartState::RestartPending, now)
}

/// Restart now, ending whatever runs (see the module docs).
pub async fn restart_now(state: &AppState) -> (RestartState, Blockers) {
    let now = blockers(state).await;
    state.lifecycle.request(ShutdownKind::Restart);
    (RestartState::Restarting, now)
}

/// End all work on the Host so the process can exit without losing state.
/// Idempotent enough to call once per process; the caller then stops serving
/// and exits.
pub async fn drain(state: &AppState) -> DrainReport {
    let lifecycle = &state.lifecycle;
    lifecycle.start_draining();
    let mut report = DrainReport::default();

    let unfinished = lifecycle.wait_for_requests(REQUEST_WAIT).await;
    if unfinished > 0 {
        tracing::warn!("[lifecycle] {unfinished} operation(s) still running at drain; continuing");
    }

    // Running turns and requests waiting on the user end through the same
    // path as `session.cancel`: clients see the turn end and the request
    // resolve, and the transcript keeps what the agent produced.
    let mut cancelled = Vec::new();
    for info in state.connection_manager.list_connections().await {
        let Some(s) = state.connection_manager.get_state(&info.id).await else {
            continue;
        };
        let (turn, request) = {
            let s = s.read().await;
            if !crate::session::is_busy(&s) {
                continue;
            }
            (
                s.turn_in_flight,
                s.pending_permission.is_some()
                    || s.pending_question.is_some()
                    || s.pending_plan_approval.is_some(),
            )
        };
        report.cancelled_turns += usize::from(turn);
        report.declined_requests += usize::from(request);
        if let Err(e) = state
            .connection_manager
            .cancel(&state.db.conn, &info.id)
            .await
        {
            tracing::warn!("[lifecycle] cancelling a turn failed: {e}");
        }
        cancelled.push(s);
    }
    let deadline = tokio::time::Instant::now() + TURN_WAIT;
    for s in &cancelled {
        while s.read().await.turn_in_flight && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    report.terminals_closed = state.terminal_manager.kill_all();
    report.agents_stopped = state.connection_manager.disconnect_all().await;
    // Connected clients are told the Host is going away and reconnect.
    state.web_server_state.shutdown_signal().trigger();

    tracing::info!(
        "[lifecycle] drained: cancelled {} turn(s), declined {} request(s), closed {} terminal(s), stopped {} agent(s)",
        report.cancelled_turns,
        report.declined_requests,
        report.terminals_closed,
        report.agents_stopped
    );
    report
}

/// Flush the write-ahead log into the database file, so the files on disk
/// are complete even before the connection pool is dropped.
pub async fn persist(state: &AppState) {
    if let Err(e) = state
        .db
        .conn
        .execute_unprepared("PRAGMA wal_checkpoint(TRUNCATE)")
        .await
    {
        tracing::warn!("[lifecycle] WAL checkpoint failed: {e}");
    }
}

/// Resolves on the first `SIGTERM` / `SIGINT` (Ctrl-C). A second one ends
/// the process at once, for an operator who will not wait for the drain.
pub async fn stop_signal() {
    wait_signal().await;
    tokio::spawn(async {
        wait_signal().await;
        tracing::warn!("[lifecycle] second stop signal; exiting without finishing the drain");
        std::process::exit(1);
    });
}

#[cfg(unix)]
async fn wait_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        return std::future::pending().await;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

#[cfg(not(unix))]
async fn wait_signal() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests;
