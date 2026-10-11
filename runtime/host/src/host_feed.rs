//! Host State Feed — the host-wide projection clients receive after
//! `host.subscribe` on `/zwiim/v1/ws`.
//!
//! One task owns a projection of exactly what `workspace.list`,
//! `session.list` (for every listed workspace) and `terminal.list` return.
//! Host signals — folder / conversation / terminal events, agent connection
//! status, and the workspace-state watcher — only mark parts of it stale. The
//! task re-reads those parts through the same facades the operations use,
//! diffs them against the projection and publishes only real changes, each
//! with the next `seq`. Raw host events never reach clients: they are change
//! detectors, not state.
//!
//! File and git activity comes from the existing workspace-state watcher,
//! run in its cheap paths-only mode for every listed workspace. A batch that
//! touches git metadata re-reads the workspace (its branch may have moved);
//! a batch that may change the diff publishes `workspace.diff_changed`, an
//! invalidation with no paths or contents.
//!
//! Pull requests come from the PR watcher (`pr_watch`): the feed tells it
//! which workspaces are on a named branch and when a push or a finished agent
//! turn may have moved their pull request, and publishes what it finds as
//! `workspace.pr_changed`.
//!
//! `epoch` names this feed's generation (one per host process) and `seq` is
//! contiguous within it. There is no replay: a subscriber that falls behind
//! gets a fresh snapshot instead.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sea_orm::DatabaseConnection;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};
use tokio::sync::{mpsc, oneshot, OnceCell};

use crate::acp::manager::ConnectionManager;
use crate::acp::types::{AcpEvent, ConnectionStatus, EventEnvelope};
use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::git_harness::{GitError, GitErrorCode};
use crate::pr_watch::{self, PrCmd, PrEntry, PrResult, PrTarget, PrWatchConfig};
use crate::protocol::http::SessionSummary;
use crate::session;
use crate::terminal::manager::TerminalManager;
use crate::web::event_bridge::{
    EventEmitter, WebEvent, CONVERSATIONS_BULK_CHANGED_EVENT, CONVERSATION_CHANGED_EVENT,
    FOLDER_CHANGED_EVENT,
};
use crate::workspace::clone::{CloneJob, CloneJobs, CloneState};
use crate::workspace::{self, WorkspaceInfo};
use crate::workspace_state;

/// Events a subscriber may fall behind by before it is re-snapshotted.
const SUBSCRIBER_CAPACITY: usize = 1024;

/// Queued inputs folded into one pass at most, so a flood of terminal output
/// on the shared broadcaster cannot starve the re-read.
const MAX_INPUTS_PER_PASS: usize = 4096;

/// How long a departed agent connection is awaited before `live` is re-read.
/// Its `Disconnected` status is broadcast just before the manager drops it.
const DEPARTURE_POLL: Duration = Duration::from_millis(20);
const DEPARTURE_MAX_POLLS: u32 = 250;

const WORKSPACE_STATE_EVENT: &str = "folder://workspace-state-event";
const TERMINAL_EXIT_PREFIX: &str = "terminal://exit/";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TerminalSummary {
    pub terminal_id: String,
    pub title: String,
    /// The workspace the terminal runs in, `None` outside every workspace.
    pub workspace_id: Option<String>,
}

/// One published change: `{seq, type, payload}`.
#[derive(Debug, Clone, Serialize)]
pub struct HostEvent {
    pub seq: u64,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub payload: Value,
}

/// The whole projection at `seq`.
#[derive(Debug, Clone, Serialize)]
pub struct HostSnapshot {
    pub epoch: String,
    pub seq: u64,
    pub workspaces: Vec<WorkspaceInfo>,
    pub sessions: Vec<SessionSummary>,
    pub terminals: Vec<TerminalSummary>,
    /// The pull request of each workspace that has one (or cannot see them).
    pub prs: BTreeMap<String, PrEntry>,
    /// Clone jobs in progress or finished within the retention period,
    /// oldest first.
    pub clones: Vec<CloneJob>,
}

/// How long `git.pr { refresh }` waits for GitHub.
const PR_REFRESH_WAIT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, Default)]
struct Projection {
    seq: u64,
    /// In `workspace.list` order.
    workspaces: Vec<WorkspaceInfo>,
    /// `session.list` of each listed workspace.
    sessions: HashMap<String, Vec<SessionSummary>>,
    terminals: BTreeMap<String, TerminalSummary>,
    prs: BTreeMap<String, PrEntry>,
    clones: Vec<CloneJob>,
}

impl Projection {
    fn snapshot(&self, epoch: &str) -> HostSnapshot {
        HostSnapshot {
            epoch: epoch.to_string(),
            seq: self.seq,
            workspaces: self.workspaces.clone(),
            sessions: self
                .workspaces
                .iter()
                .flat_map(|w| self.sessions.get(&w.workspace_id).into_iter().flatten())
                .cloned()
                .collect(),
            terminals: self.terminals.values().cloned().collect(),
            prs: self.prs.clone(),
            clones: self.clones.clone(),
        }
    }
}

struct Shared {
    projection: Mutex<Projection>,
    tx: broadcast::Sender<Arc<HostEvent>>,
    prs: mpsc::UnboundedSender<PrCmd>,
}

/// The host's feed. Started by its first subscriber; runs for the life of
/// the process.
pub struct HostFeed {
    epoch: String,
    shared: OnceCell<Arc<Shared>>,
    prs: PrWatchConfig,
}

impl Default for HostFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl HostFeed {
    pub fn new() -> Self {
        Self::with_pr_watch(PrWatchConfig::default())
    }

    /// A feed whose PR watcher reaches GitHub as `prs` says.
    pub fn with_pr_watch(prs: PrWatchConfig) -> Self {
        Self {
            epoch: uuid::Uuid::new_v4().simple().to_string(),
            shared: OnceCell::new(),
            prs,
        }
    }

    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    /// The current projection plus a receiver for every event after it.
    pub async fn subscribe(
        &self,
        state: &Arc<AppState>,
    ) -> Result<(HostSnapshot, broadcast::Receiver<Arc<HostEvent>>), AppCommandError> {
        let shared = self.shared(state).await?;
        // Publishing holds this lock across `seq` assignment and send, so the
        // receiver sees exactly the events after the snapshot.
        let projection = shared.projection.lock().unwrap_or_else(|e| e.into_inner());
        let rx = shared.tx.subscribe();
        Ok((projection.snapshot(&self.epoch), rx))
    }

    async fn shared(&self, state: &Arc<AppState>) -> Result<&Arc<Shared>, AppCommandError> {
        self.shared
            .get_or_try_init(|| start(state, self.prs.clone()))
            .await
    }

    /// The pull request the Host knows for `workspace_id`; with `refresh`,
    /// what GitHub says now (published to every subscriber too).
    pub async fn pr(
        &self,
        state: &Arc<AppState>,
        workspace_id: &str,
        refresh: bool,
    ) -> Result<PrEntry, GitError> {
        let shared = self.shared(state).await.map_err(|e| {
            tracing::warn!("[host feed] start failed: {e}");
            GitError::new(GitErrorCode::Failed, "The Host could not read its workspaces")
        })?;
        if !refresh {
            let projection = shared.projection.lock().unwrap_or_else(|e| e.into_inner());
            return Ok(projection.prs.get(workspace_id).cloned().unwrap_or_default());
        }
        let (reply, answer) = oneshot::channel();
        let _ = shared
            .prs
            .send(PrCmd::Refresh(workspace_id.to_string(), reply));
        match tokio::time::timeout(PR_REFRESH_WAIT, answer).await {
            Ok(Ok(answer)) => answer,
            _ => Err(GitError::new(
                GitErrorCode::Timeout,
                "GitHub did not answer in time",
            )),
        }
    }

    /// Something may have changed `workspace_id`'s pull request: look again.
    /// Nothing to do before the feed has started.
    pub fn nudge_pr(&self, workspace_id: &str) {
        if let Some(shared) = self.shared.get() {
            let _ = shared.prs.send(PrCmd::Check(workspace_id.to_string()));
        }
    }
}

async fn start(state: &Arc<AppState>, prs: PrWatchConfig) -> Result<Arc<Shared>, AppCommandError> {
    // Subscribe before the first read so no change between them is lost.
    let web_rx = state.event_broadcaster.subscribe();
    let bus_rx = state.acp_event_bus.subscribe();
    let clones_rx = state.clones.subscribe();
    let (pr_results, pr_rx) = mpsc::unbounded_channel();
    let pr_cmd = pr_watch::spawn(state.db.conn.clone(), state.data_dir.clone(), prs, pr_results);
    let mut task = FeedTask {
        db: state.db.conn.clone(),
        connections: state.connection_manager.clone_ref(),
        terminals: state.terminal_manager.clone_ref(),
        emitter: state.emitter.clone(),
        projection: Projection::default(),
        watched: HashMap::new(),
        watch_keys: HashMap::new(),
        pr_cmd: pr_cmd.clone(),
        pr_targets: Vec::new(),
        clones: state.clones.clone(),
    };
    task.projection.workspaces = workspace::list(&task.db).await?;
    task.projection.clones = task.clones.list();
    let ids: Vec<String> = task
        .projection
        .workspaces
        .iter()
        .map(|w| w.workspace_id.clone())
        .collect();
    task.projection.sessions = task.read_sessions(&ids, &HashMap::new()).await;
    task.projection.terminals = task.read_terminals(&task.projection.workspaces);
    task.sync_watchers().await;
    task.sync_pr_targets();

    let (tx, _) = broadcast::channel(SUBSCRIBER_CAPACITY);
    let shared = Arc::new(Shared {
        projection: Mutex::new(task.projection.clone()),
        tx,
        prs: pr_cmd,
    });
    tokio::spawn(task.run(shared.clone(), web_rx, bus_rx, pr_rx, clones_rx));
    Ok(shared)
}

/// What a pass has to re-read.
#[derive(Default)]
struct Stale {
    workspaces: bool,
    sessions: bool,
    terminals: bool,
    /// Watcher batches per workspace: the changed paths, or `None` when the
    /// watcher could not enumerate them.
    watched: HashMap<String, Option<Vec<String>>>,
    /// An input was lost: every diff may have changed.
    all_diffs: bool,
    /// Agent connections that reported leaving.
    departed: Vec<String>,
    /// Agent connections that finished a turn.
    turns: Vec<String>,
    clones: bool,
}

impl Stale {
    fn any(&self) -> bool {
        self.workspaces
            || self.sessions
            || self.terminals
            || self.all_diffs
            || !self.watched.is_empty()
            || !self.departed.is_empty()
            || !self.turns.is_empty()
            || self.clones
    }

    fn everything(&mut self) {
        self.workspaces = true;
        self.sessions = true;
        self.terminals = true;
        self.all_diffs = true;
        self.clones = true;
    }
}

struct WatchedRoot {
    path: String,
    keys: Vec<String>,
}

struct FeedTask {
    db: DatabaseConnection,
    connections: ConnectionManager,
    terminals: TerminalManager,
    emitter: EventEmitter,
    projection: Projection,
    /// Listed live workspaces with a paths-only watcher stream, by id.
    watched: HashMap<String, WatchedRoot>,
    /// Watcher root spelling → workspace id.
    watch_keys: HashMap<String, String>,
    pr_cmd: mpsc::UnboundedSender<PrCmd>,
    /// What the PR watcher was last told to follow.
    pr_targets: Vec<PrTarget>,
    clones: Arc<CloneJobs>,
}

/// Spellings a watcher event may name `path` by: as given and canonical.
fn root_keys(path: &str) -> Vec<String> {
    let mut keys = vec![path.to_string()];
    if let Ok(canonical) = std::fs::canonicalize(path) {
        let canonical = canonical.to_string_lossy().replace('\\', "/");
        if canonical != path {
            keys.push(canonical);
        }
    }
    keys
}

fn is_git_metadata(path: &str) -> bool {
    path == ".git" || path.starts_with(".git/")
}

/// The batch may be a push of `branch`: its remote-tracking ref moved.
fn may_be_push(paths: Option<&Vec<String>>, branch: &str) -> bool {
    let Some(paths) = paths else {
        return true;
    };
    let suffix = format!("/{branch}");
    paths.iter().any(|p| {
        p == ".git/packed-refs"
            || (p.starts_with(".git/refs/remotes/") && p.ends_with(&suffix))
    })
}

/// The workspaces that may have a pull request: live, Git, on a named
/// branch. The watcher itself skips the default branch.
fn pr_targets(workspaces: &[WorkspaceInfo]) -> Vec<PrTarget> {
    workspaces
        .iter()
        .filter(|w| !w.removed && w.is_git_repo)
        .filter_map(|w| {
            let branch = w.branch.as_deref().filter(|b| !b.is_empty() && *b != "HEAD")?;
            Some(PrTarget {
                workspace_id: w.workspace_id.clone(),
                path: w.path.clone(),
                branch: branch.to_string(),
            })
        })
        .collect()
}

fn pr_changed(workspace_id: &str, entry: &PrEntry) -> (&'static str, Value) {
    (
        "workspace.pr_changed",
        json!({ "workspace_id": workspace_id, "pr": entry.pr, "error": entry.error }),
    )
}

/// `clone.changed` for each new or changed job, `clone.removed` for each
/// expired one.
fn clone_events(old: &[CloneJob], new: &[CloneJob]) -> Vec<(&'static str, Value)> {
    let mut events = Vec::new();
    for job in new {
        if old.iter().find(|o| o.clone_id == job.clone_id) != Some(job) {
            events.push(("clone.changed", json!({ "clone": job })));
        }
    }
    for gone in old
        .iter()
        .filter(|o| !new.iter().any(|j| j.clone_id == o.clone_id))
    {
        events.push(("clone.removed", json!({ "clone_id": gone.clone_id })));
    }
    events
}

impl FeedTask {
    async fn run(
        mut self,
        shared: Arc<Shared>,
        mut web_rx: broadcast::Receiver<WebEvent>,
        mut bus_rx: broadcast::Receiver<Arc<EventEnvelope>>,
        mut pr_rx: mpsc::UnboundedReceiver<PrResult>,
        mut clones_rx: tokio::sync::watch::Receiver<u64>,
    ) {
        let (recheck_tx, mut recheck_rx) = mpsc::unbounded_channel::<()>();
        let mut stale = Stale::default();
        loop {
            tokio::select! {
                r = web_rx.recv() => match r {
                    Ok(ev) => self.note_web(&ev, &mut stale),
                    Err(RecvError::Lagged(_)) => stale.everything(),
                    Err(RecvError::Closed) => return,
                },
                r = bus_rx.recv() => match r {
                    Ok(env) => note_bus(&env, &mut stale),
                    Err(RecvError::Lagged(_)) => stale.everything(),
                    Err(RecvError::Closed) => return,
                },
                Some(()) = recheck_rx.recv() => stale.sessions = true,
                r = clones_rx.changed() => match r {
                    Ok(()) => stale.clones = true,
                    Err(_) => return,
                },
                Some(result) = pr_rx.recv() => {
                    self.apply_pr(&shared, result);
                    continue;
                }
            }
            // Fold whatever is already queued into this pass.
            for _ in 0..MAX_INPUTS_PER_PASS {
                match web_rx.try_recv() {
                    Ok(ev) => self.note_web(&ev, &mut stale),
                    Err(TryRecvError::Lagged(_)) => stale.everything(),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Closed) => return,
                }
            }
            for _ in 0..MAX_INPUTS_PER_PASS {
                match bus_rx.try_recv() {
                    Ok(env) => note_bus(&env, &mut stale),
                    Err(TryRecvError::Lagged(_)) => stale.everything(),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Closed) => return,
                }
            }
            while recheck_rx.try_recv().is_ok() {
                stale.sessions = true;
            }
            if clones_rx.has_changed().unwrap_or(false) {
                clones_rx.borrow_and_update();
                stale.clones = true;
            }
            if stale.any() {
                self.pass(&shared, std::mem::take(&mut stale), &recheck_tx)
                    .await;
            }
        }
    }

    fn note_web(&self, ev: &WebEvent, stale: &mut Stale) {
        match ev.channel.as_str() {
            FOLDER_CHANGED_EVENT | CONVERSATIONS_BULK_CHANGED_EVENT => {
                stale.workspaces = true;
                stale.sessions = true;
            }
            CONVERSATION_CHANGED_EVENT => stale.sessions = true,
            WORKSPACE_STATE_EVENT => {
                let Some(root) = ev.payload.get("root_path").and_then(Value::as_str) else {
                    return;
                };
                let Some(workspace_id) = root_keys(root)
                    .iter()
                    .find_map(|k| self.watch_keys.get(k))
                    .cloned()
                else {
                    return;
                };
                let paths: Vec<String> = ev
                    .payload
                    .get("changed_paths")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
                    .unwrap_or_default();
                let entry = stale.watched.entry(workspace_id).or_insert(Some(Vec::new()));
                match (entry.as_mut(), paths.is_empty()) {
                    (Some(known), false) => known.extend(paths),
                    // Empty means the batch overflowed: anything may have changed.
                    _ => *entry = None,
                }
            }
            channel
                if channel.starts_with(TERMINAL_EXIT_PREFIX)
                    || channel.starts_with(crate::terminal::manager::TERMINAL_SPAWNED_PREFIX) =>
            {
                stale.terminals = true
            }
            _ => {}
        }
    }

    /// Re-read what `stale` names, diff it against the projection, publish
    /// the changes.
    async fn pass(&mut self, shared: &Shared, stale: Stale, recheck: &mpsc::UnboundedSender<()>) {
        let old = &self.projection;

        // Clones: a job that just finished added workspaces, so relist in
        // this same pass and publish them before its `done` (R4).
        let clones = if stale.clones {
            self.clones.list()
        } else {
            old.clones.clone()
        };
        let newly_done = clones.iter().any(|j| {
            j.state == CloneState::Done
                && !old
                    .clones
                    .iter()
                    .any(|o| o.clone_id == j.clone_id && o.state == CloneState::Done)
        });

        // Workspaces: the whole list, or just the ones whose git metadata moved.
        let mut workspaces = old.workspaces.clone();
        let mut relist = stale.workspaces || newly_done;
        if !relist {
            for (workspace_id, paths) in &stale.watched {
                let touches_git = paths
                    .as_ref()
                    .is_none_or(|p| p.iter().any(|p| is_git_metadata(p)));
                if !touches_git {
                    continue;
                }
                let Some(slot) = workspaces
                    .iter_mut()
                    .find(|w| &w.workspace_id == workspace_id)
                else {
                    continue;
                };
                let reread = match workspace::resolve_readable(&self.db, workspace_id).await {
                    Ok(row) => workspace::info(&self.db, row).await,
                    Err(e) => Err(e),
                };
                match reread {
                    Ok(info) => *slot = info,
                    Err(_) => relist = true,
                }
            }
        }
        if relist {
            match workspace::list(&self.db).await {
                Ok(list) => workspaces = list,
                Err(e) => tracing::warn!("[host feed] workspace list failed: {e}"),
            }
        }

        // Sessions: every listed workspace when stale, else only new ones.
        let ids: Vec<String> = workspaces
            .iter()
            .filter(|w| stale.sessions || !old.sessions.contains_key(&w.workspace_id))
            .map(|w| w.workspace_id.clone())
            .collect();
        let mut sessions = self.read_sessions(&ids, &old.sessions).await;
        for w in &workspaces {
            if let Some(kept) = old.sessions.get(&w.workspace_id) {
                sessions
                    .entry(w.workspace_id.clone())
                    .or_insert_with(|| kept.clone());
            }
        }

        let terminals = if stale.terminals || relist {
            self.read_terminals(&workspaces)
        } else {
            old.terminals.clone()
        };

        // Diff invalidations, only for live listed workspaces.
        let mut diffs: Vec<String> = Vec::new();
        for w in workspaces.iter().filter(|w| !w.removed) {
            // A new branch or base (Continue) moves the diff with no file event.
            let moved = old
                .workspaces
                .iter()
                .find(|o| o.workspace_id == w.workspace_id)
                .is_some_and(|o| o.branch != w.branch || o.base_commit != w.base_commit);
            let changed = if stale.all_diffs || moved {
                true
            } else {
                match stale.watched.get(&w.workspace_id) {
                    None => false,
                    Some(None) => true,
                    Some(Some(paths)) => {
                        workspace_state::should_refresh_git_status_for_paths(&w.path, paths).await
                    }
                }
            };
            if changed {
                diffs.push(w.workspace_id.clone());
            }
        }

        // Pull requests: a branch that changed or left starts over.
        let mut prs = old.prs.clone();
        let mut pr_events = Vec::new();
        for (id, entry) in &old.prs {
            let before = old.workspaces.iter().find(|w| &w.workspace_id == id);
            let now = workspaces.iter().find(|w| &w.workspace_id == id);
            let same = matches!((before, now), (Some(b), Some(n)) if b.branch == n.branch && !n.removed);
            if !same {
                prs.remove(id);
                if now.is_some() && !entry.is_empty() {
                    pr_events.push(pr_changed(id, &PrEntry::default()));
                }
            }
        }
        // A push may have opened or moved one.
        let mut pr_checks: Vec<String> = Vec::new();
        for w in workspaces.iter().filter(|w| !w.removed) {
            let Some(branch) = w.branch.as_deref() else {
                continue;
            };
            if let Some(paths) = stale.watched.get(&w.workspace_id) {
                if may_be_push(paths.as_ref(), branch) {
                    pr_checks.push(w.workspace_id.clone());
                }
            }
        }
        for connection_id in &stale.turns {
            let Some(state) = self.connections.get_state(connection_id).await else {
                continue;
            };
            let dir = state.read().await.working_dir.clone();
            if let Some(id) = dir.and_then(|d| workspace::workspace_for_dir(&workspaces, &d.to_string_lossy())) {
                pr_checks.push(id);
            }
        }

        let mut events = diff_events(old, &workspaces, &sessions, &terminals, &diffs);
        events.extend(pr_events);
        events.extend(clone_events(&old.clones, &clones));
        self.projection = Projection {
            seq: old.seq,
            workspaces,
            sessions,
            terminals,
            prs,
            clones,
        };
        self.publish(shared, events);
        self.sync_watchers().await;
        self.sync_pr_targets();
        for id in pr_checks {
            let _ = self.pr_cmd.send(PrCmd::Check(id));
        }

        for connection_id in stale.departed {
            if self.connections.get_state(&connection_id).await.is_none() {
                continue;
            }
            let connections = self.connections.clone_ref();
            let recheck = recheck.clone();
            tokio::spawn(async move {
                for _ in 0..DEPARTURE_MAX_POLLS {
                    if connections.get_state(&connection_id).await.is_none() {
                        break;
                    }
                    tokio::time::sleep(DEPARTURE_POLL).await;
                }
                let _ = recheck.send(());
            });
        }
    }

    /// Publish a lookup's answer if it is still about the workspace's branch
    /// and changes what clients know.
    fn apply_pr(&mut self, shared: &Shared, result: PrResult) {
        let current = self.projection.workspaces.iter().any(|w| {
            w.workspace_id == result.workspace_id
                && !w.removed
                && w.branch.as_deref() == Some(result.branch.as_str())
        });
        if !current {
            return;
        }
        let known = self.projection.prs.get(&result.workspace_id);
        if known.cloned().unwrap_or_default() == result.entry {
            return;
        }
        if result.entry.is_empty() {
            self.projection.prs.remove(&result.workspace_id);
        } else {
            self.projection
                .prs
                .insert(result.workspace_id.clone(), result.entry.clone());
        }
        let event = pr_changed(&result.workspace_id, &result.entry);
        self.publish(shared, vec![event]);
    }

    /// Tell the PR watcher which workspaces to follow when that changed.
    fn sync_pr_targets(&mut self) {
        let targets = pr_targets(&self.projection.workspaces);
        if targets != self.pr_targets {
            self.pr_targets = targets.clone();
            let _ = self.pr_cmd.send(PrCmd::Targets(targets));
        }
    }

    fn publish(&mut self, shared: &Shared, events: Vec<(&'static str, Value)>) {
        if events.is_empty() {
            return;
        }
        let mut published = shared.projection.lock().unwrap_or_else(|e| e.into_inner());
        for (kind, payload) in events {
            self.projection.seq += 1;
            let _ = shared.tx.send(Arc::new(HostEvent {
                seq: self.projection.seq,
                kind,
                payload,
            }));
        }
        *published = self.projection.clone();
    }

    /// `session.list` of each of `ids`. A workspace that cannot be read keeps
    /// what `previous` had; the next workspace re-read settles it.
    async fn read_sessions(
        &self,
        ids: &[String],
        previous: &HashMap<String, Vec<SessionSummary>>,
    ) -> HashMap<String, Vec<SessionSummary>> {
        let mut out = HashMap::new();
        for id in ids {
            match session::list_for_workspace(&self.db, &self.connections, id).await {
                Ok(list) => {
                    out.insert(id.clone(), list.into_iter().map(SessionSummary::from).collect());
                }
                Err(e) => {
                    tracing::warn!("[host feed] session list failed: {e}");
                    if let Some(kept) = previous.get(id) {
                        out.insert(id.clone(), kept.clone());
                    }
                }
            }
        }
        out
    }

    fn read_terminals(&self, workspaces: &[WorkspaceInfo]) -> BTreeMap<String, TerminalSummary> {
        self.terminals
            .list_with_exit_check(Some(&self.emitter))
            .into_iter()
            .map(|t| {
                (
                    t.id.clone(),
                    TerminalSummary {
                        workspace_id: workspace::workspace_for_dir(workspaces, &t.working_dir),
                        terminal_id: t.id,
                        title: t.title,
                    },
                )
            })
            .collect()
    }

    /// Keep one paths-only watcher stream per listed live workspace.
    async fn sync_watchers(&mut self) {
        let want: HashMap<&str, &str> = self
            .projection
            .workspaces
            .iter()
            .filter(|w| !w.removed)
            .map(|w| (w.workspace_id.as_str(), w.path.as_str()))
            .collect();
        let gone: Vec<String> = self
            .watched
            .iter()
            .filter(|(id, root)| want.get(id.as_str()) != Some(&root.path.as_str()))
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            if let Some(root) = self.watched.remove(&id) {
                for key in &root.keys {
                    self.watch_keys.remove(key);
                }
                let _ =
                    workspace_state::stop_workspace_state_stream_core(root.path, false).await;
            }
        }
        let new: Vec<(String, String)> = want
            .iter()
            .filter(|(id, _)| !self.watched.contains_key(**id))
            .map(|(id, path)| (id.to_string(), path.to_string()))
            .collect();
        for (id, path) in new {
            let started = workspace_state::start_workspace_state_stream_core(
                self.emitter.clone(),
                path.clone(),
                false,
            )
            .await;
            if let Err(e) = started {
                // Retried on the next workspace re-read.
                tracing::warn!("[host feed] workspace watcher failed to start: {e}");
                continue;
            }
            let keys = root_keys(&path);
            for key in &keys {
                self.watch_keys.insert(key.clone(), id.clone());
            }
            self.watched.insert(id, WatchedRoot { path, keys });
        }
    }
}

fn note_bus(env: &EventEnvelope, stale: &mut Stale) {
    match &env.payload {
        AcpEvent::StatusChanged { status } => {
            stale.sessions = true;
            if matches!(
                status,
                ConnectionStatus::Disconnected | ConnectionStatus::Error
            ) {
                stale.departed.push(env.connection_id.clone());
            }
        }
        AcpEvent::Error { terminal: true, .. } => {
            stale.sessions = true;
            stale.departed.push(env.connection_id.clone());
        }
        AcpEvent::ConversationLinked { .. } => stale.sessions = true,
        AcpEvent::TurnComplete { .. } => stale.turns.push(env.connection_id.clone()),
        _ => {}
    }
}

/// The public events that turn `old` into the new projection, in an order a
/// client can apply one by one: upserts before the removals that could
/// orphan them, workspaces before their sessions.
fn diff_events(
    old: &Projection,
    workspaces: &[WorkspaceInfo],
    sessions: &HashMap<String, Vec<SessionSummary>>,
    terminals: &BTreeMap<String, TerminalSummary>,
    diffs: &[String],
) -> Vec<(&'static str, Value)> {
    let mut events = Vec::new();

    for w in workspaces {
        let before = old.workspaces.iter().find(|o| o.workspace_id == w.workspace_id);
        if before != Some(w) {
            events.push(("workspace.changed", json!({ "workspace": w })));
        }
    }

    let old_sessions: HashMap<&str, &SessionSummary> = old
        .sessions
        .values()
        .flatten()
        .map(|s| (s.session_id.as_str(), s))
        .collect();
    let mut listed: HashSet<&str> = HashSet::new();
    for w in workspaces {
        for s in sessions.get(&w.workspace_id).into_iter().flatten() {
            listed.insert(s.session_id.as_str());
            if old_sessions.get(s.session_id.as_str()) != Some(&s) {
                events.push(("session.changed", json!({ "session": s })));
            }
        }
    }
    for w in &old.workspaces {
        for s in old.sessions.get(&w.workspace_id).into_iter().flatten() {
            if !listed.contains(s.session_id.as_str()) {
                events.push((
                    "session.removed",
                    json!({ "session_id": s.session_id, "workspace_id": w.workspace_id }),
                ));
            }
        }
    }

    for o in &old.workspaces {
        if !workspaces.iter().any(|w| w.workspace_id == o.workspace_id) {
            events.push(("workspace.removed", json!({ "workspace_id": o.workspace_id })));
        }
    }

    for id in diffs {
        events.push(("workspace.diff_changed", json!({ "workspace_id": id })));
    }

    for (id, t) in terminals {
        if old.terminals.get(id) != Some(t) {
            events.push(("terminal.changed", json!({ "terminal": t })));
        }
    }
    for id in old.terminals.keys() {
        if !terminals.contains_key(id) {
            events.push(("terminal.removed", json!({ "terminal_id": id })));
        }
    }

    events
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tokio::sync::broadcast::Receiver;

    use super::*;
    use crate::commands::conversations::{
        delete_conversation_with_cleanup_core, emit_conversation_upsert,
        update_conversation_status_core, update_conversation_title_core,
    };
    use crate::commands::folders::{converge_removed_worktree_folder, emit_folder_deleted};
    use crate::db::service::folder_service;
    use crate::db::test_helpers::{seed_conversation, seed_folder};
    use crate::models::agent::AgentType;
    use crate::protocol::events::tests::assert_no_internal_keys;
    use crate::terminal::manager::SpawnOptions;
    use crate::web::event_bridge::emit_with_state;

    const CONN: &str = "conn-feed-4c1d";

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    struct Fixture {
        dir: tempfile::TempDir,
        root: String,
        root_id: String,
        folder_id: i32,
        state: Arc<AppState>,
    }

    /// A host over an on-disk database with one committed repo as its root
    /// workspace.
    async fn fixture() -> Fixture {
        fixture_with(PrWatchConfig::default()).await
    }

    async fn fixture_with(prs: PrWatchConfig) -> Fixture {
        fixture_full(prs, None).await
    }

    /// With `gh`, a stand-in `gh` running that `case` arm list, `{bare}`
    /// replaced by a bare clone of the root repo.
    async fn fixture_full(prs: PrWatchConfig, gh: Option<&str>) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join(".gitignore"), "ignored/\n").unwrap();
        std::fs::write(repo.join("tracked.txt"), "one\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        let data_dir = dir.path().join("data");
        std::fs::create_dir(&data_dir).unwrap();
        let db = crate::db::init_database(&data_dir, "test").await.unwrap();
        let root = repo.canonicalize().unwrap().to_string_lossy().into_owned();
        let folder_id = seed_folder(&db, &root).await;
        let mut state = AppState::new_for_test(db, data_dir);
        state.host_feed = Arc::new(HostFeed::with_pr_watch(prs));
        if let Some(body) = gh {
            let bare = dir.path().join("remote.git");
            git(
                dir.path(),
                &[
                    "clone",
                    "-q",
                    "--bare",
                    repo.to_str().unwrap(),
                    bare.to_str().unwrap(),
                ],
            );
            let body = body.replace("{bare}", bare.to_str().unwrap());
            state.gh = Some(crate::git_harness::fake_gh(dir.path(), &body).0);
        }
        let state = Arc::new(state);
        let root_id = workspace::workspace_id_of(&state.db.conn, folder_id)
            .await
            .unwrap()
            .unwrap();
        Fixture {
            dir,
            root,
            root_id,
            folder_id,
            state,
        }
    }

    impl Fixture {
        async fn subscribe(&self) -> (HostSnapshot, Feed) {
            let (snapshot, rx) = self.state.host_feed.subscribe(&self.state).await.unwrap();
            self.assert_clean(&serde_json::to_value(&snapshot).unwrap());
            let seq = snapshot.seq;
            (
                snapshot,
                Feed {
                    rx,
                    seq,
                    tmp: self.tmp_spellings(),
                },
            )
        }

        fn tmp_spellings(&self) -> Vec<String> {
            let tmp = self.dir.path();
            vec![
                tmp.to_string_lossy().into_owned(),
                tmp.canonicalize().unwrap().to_string_lossy().into_owned(),
            ]
        }

        fn assert_clean(&self, v: &Value) {
            assert_clean(v, &self.tmp_spellings());
        }

        async fn create_worktree(&self) -> WorkspaceInfo {
            workspace::create_worktree(
                &self.state.db,
                &self.state.emitter,
                &self.state.worktrees_dir,
                &self.root_id,
                None,
            )
            .await
            .unwrap()
        }

        async fn folder_of(&self, workspace_id: &str) -> i32 {
            workspace::resolve_readable(&self.state.db.conn, workspace_id)
                .await
                .unwrap()
                .id
        }

        /// A new session row in `folder_id`, announced as a conversation upsert.
        async fn new_session(&self, folder_id: i32) -> (i32, String) {
            let conv = seed_conversation(&self.state.db, folder_id, AgentType::ClaudeCode).await;
            emit_conversation_upsert(&self.state.emitter, &self.state.db.conn, conv).await;
            let sid = crate::db::entities::conversation::Entity::find_by_id(conv)
                .one(&self.state.db.conn)
                .await
                .unwrap()
                .unwrap()
                .session_id
                .unwrap();
            (conv, sid)
        }

        async fn emit(&self, event: AcpEvent) {
            let (state, emitter) = self
                .state
                .connection_manager
                .get_state_and_emitter(CONN)
                .await
                .unwrap();
            emit_with_state(&state, &emitter, event).await;
        }

        fn spawn_terminal(&self) -> String {
            self.state
                .terminal_manager
                .spawn_with_id(
                    SpawnOptions {
                        terminal_id: uuid::Uuid::new_v4().to_string(),
                        working_dir: self.root.clone(),
                        owner_window_label: "zwiim".to_string(),
                        shell: Some("/bin/sh".to_string()),
                        initial_command: None,
                        extra_env: None,
                        temp_files: vec![],
                    },
                    self.state.emitter.clone(),
                )
                .unwrap()
        }
    }

    use sea_orm::EntityTrait;

    fn assert_clean(v: &Value, tmp: &[String]) {
        assert_no_internal_keys(v);
        let text = v.to_string();
        assert!(!text.contains(CONN), "frame leaked a connection id: {text}");
        for t in tmp {
            assert!(!text.contains(t.as_str()), "frame leaked a host path: {text}");
        }
    }

    struct Feed {
        rx: Receiver<Arc<HostEvent>>,
        seq: u64,
        tmp: Vec<String>,
    }

    impl Feed {
        /// The next event; asserts `seq` continues and nothing internal leaks.
        async fn next(&mut self) -> Arc<HostEvent> {
            let ev = tokio::time::timeout(Duration::from_secs(15), self.rx.recv())
                .await
                .expect("feed event within 15s")
                .expect("feed open");
            assert_eq!(ev.seq, self.seq + 1, "seq is contiguous");
            self.seq = ev.seq;
            assert_clean(&serde_json::to_value(&*ev).unwrap(), &self.tmp);
            ev
        }

        /// Skip events until one matches.
        async fn until(&mut self, what: &str, pred: impl Fn(&HostEvent) -> bool) -> Arc<HostEvent> {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            loop {
                assert!(tokio::time::Instant::now() < deadline, "no {what}");
                let ev = self.next().await;
                if pred(&ev) {
                    return ev;
                }
            }
        }

        /// Everything published within `ms`.
        async fn drain(&mut self, ms: u64) -> Vec<Arc<HostEvent>> {
            let mut out = Vec::new();
            let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
            while let Ok(Ok(ev)) = tokio::time::timeout_at(deadline, self.rx.recv()).await {
                assert_eq!(ev.seq, self.seq + 1, "seq is contiguous");
                self.seq = ev.seq;
                assert_clean(&serde_json::to_value(&*ev).unwrap(), &self.tmp);
                out.push(ev);
            }
            out
        }
    }

    fn is(kind: &'static str, key: &'static str, id: String) -> impl Fn(&HostEvent) -> bool {
        move |e| e.kind == kind && e.payload[key] == id.as_str()
    }

    fn session_is(id: String, check: impl Fn(&Value) -> bool) -> impl Fn(&HostEvent) -> bool {
        move |e| {
            e.kind == "session.changed" && e.payload["session"]["session_id"] == id.as_str() && {
                check(&e.payload["session"])
            }
        }
    }

    fn workspace_is(id: String, check: impl Fn(&Value) -> bool) -> impl Fn(&HostEvent) -> bool {
        move |e| {
            e.kind == "workspace.changed"
                && e.payload["workspace"]["workspace_id"] == id.as_str()
                && check(&e.payload["workspace"])
        }
    }

    async fn listed_sessions(state: &AppState, workspaces: &[WorkspaceInfo]) -> Vec<Value> {
        let mut out = Vec::new();
        for w in workspaces {
            for s in session::list_for_workspace(&state.db.conn, &state.connection_manager, &w.workspace_id)
                .await
                .unwrap()
            {
                out.push(serde_json::to_value(SessionSummary::from(s)).unwrap());
            }
        }
        out
    }

    #[tokio::test]
    async fn snapshot_equals_the_list_operations() {
        let f = fixture().await;
        let wt = f.create_worktree().await;
        f.new_session(f.folder_id).await;
        f.new_session(f.folder_of(&wt.workspace_id).await).await;
        let terminal = f.spawn_terminal();

        let (snapshot, _feed) = f.subscribe().await;
        assert_eq!(snapshot.seq, 0);
        assert_eq!(snapshot.epoch, f.state.host_feed.epoch());

        let workspaces = workspace::list(&f.state.db.conn).await.unwrap();
        assert_eq!(snapshot.workspaces, workspaces);
        assert_eq!(snapshot.workspaces.len(), 2);
        assert_eq!(
            serde_json::to_value(&snapshot.sessions).unwrap(),
            Value::Array(listed_sessions(&f.state, &workspaces).await)
        );
        assert_eq!(snapshot.sessions.len(), 2);
        let mut terminals: Vec<String> = f
            .state
            .terminal_manager
            .list_with_exit_check(None)
            .into_iter()
            .map(|t| t.id)
            .collect();
        terminals.sort();
        assert_eq!(
            snapshot
                .terminals
                .iter()
                .map(|t| t.terminal_id.clone())
                .collect::<Vec<_>>(),
            terminals
        );
        assert!(terminals.contains(&terminal));
        f.state.terminal_manager.kill(&terminal).unwrap();
    }

    #[tokio::test]
    async fn workspace_add_revive_and_removal() {
        let f = fixture().await;
        let (_, mut feed) = f.subscribe().await;

        let other = f.dir.path().join("other");
        std::fs::create_dir(&other).unwrap();
        let added = workspace::add(
            &f.state.db,
            &f.state.emitter,
            other.to_str().unwrap(),
            false,
            None,
        )
        .await
        .unwrap();
        feed.until(
            "added workspace",
            workspace_is(added.workspace_id.clone(), |w| w["removed"] == false),
        )
        .await;

        // A legacy permanent removal of the root: gone from the listing.
        let other_folder = f.folder_of(&added.workspace_id).await;
        folder_service::soft_delete_folder(&f.state.db.conn, other_folder)
            .await
            .unwrap();
        emit_folder_deleted(&f.state.emitter, other_folder);
        feed.until(
            "workspace.removed",
            is("workspace.removed", "workspace_id", added.workspace_id.clone()),
        )
        .await;

        // Re-adding the path revives the same workspace.
        workspace::add(
            &f.state.db,
            &f.state.emitter,
            other.to_str().unwrap(),
            false,
            None,
        )
        .await
        .unwrap();
        feed.until(
            "revived workspace",
            workspace_is(added.workspace_id.clone(), |_| true),
        )
        .await;
    }

    #[tokio::test]
    async fn worktree_creation_and_removal() {
        let f = fixture().await;
        let (_, mut feed) = f.subscribe().await;

        // Removed with sessions: stays listed, as removed.
        let kept = f.create_worktree().await;
        let ev = feed
            .until("worktree", workspace_is(kept.workspace_id.clone(), |_| true))
            .await;
        assert_eq!(ev.payload["workspace"]["kind"], "worktree");
        assert_eq!(ev.payload["workspace"]["parent_workspace_id"], f.root_id.as_str());
        let (_, sid) = f.new_session(f.folder_of(&kept.workspace_id).await).await;
        feed.until("session", session_is(sid.clone(), |_| true)).await;
        workspace::remove_worktree(
            &f.state.db,
            &f.state.emitter,
            &f.state.connection_manager,
            &f.state.terminal_manager,
            &kept.workspace_id,
            true,
        )
        .await
        .unwrap();
        feed.until(
            "removed worktree",
            workspace_is(kept.workspace_id.clone(), |w| {
                w["removed"] == true && w["branch"].is_null()
            }),
        )
        .await;
        let after = feed.drain(800).await;
        assert!(
            after.iter().all(|e| e.kind != "workspace.removed" && e.kind != "session.removed"),
            "{after:?}"
        );

        // Removed without sessions: unlisted.
        let gone = f.create_worktree().await;
        feed.until("worktree", workspace_is(gone.workspace_id.clone(), |_| true))
            .await;
        workspace::remove_worktree(
            &f.state.db,
            &f.state.emitter,
            &f.state.connection_manager,
            &f.state.terminal_manager,
            &gone.workspace_id,
            false,
        )
        .await
        .unwrap();
        feed.until(
            "workspace.removed",
            is("workspace.removed", "workspace_id", gone.workspace_id.clone()),
        )
        .await;
    }

    #[tokio::test]
    async fn legacy_reparent_moves_the_session_to_the_root() {
        let f = fixture().await;
        let wt = f.create_worktree().await;
        let wt_folder = f.folder_of(&wt.workspace_id).await;
        let (_, sid) = f.new_session(wt_folder).await;
        let (snapshot, mut feed) = f.subscribe().await;
        assert!(snapshot
            .sessions
            .iter()
            .any(|s| s.session_id == sid && s.workspace_id.as_deref() == Some(&wt.workspace_id)));

        converge_removed_worktree_folder(
            &f.state.db,
            &f.state.emitter,
            wt_folder,
            Some(f.folder_id),
            &wt.path,
        )
        .await;
        let root = f.root_id.clone();
        feed.until(
            "reparented session",
            session_is(sid.clone(), move |s| s["workspace_id"] == root.as_str()),
        )
        .await;
        feed.until(
            "workspace.removed",
            is("workspace.removed", "workspace_id", wt.workspace_id.clone()),
        )
        .await;
    }

    #[tokio::test]
    async fn session_creation_title_status_and_removal() {
        let f = fixture().await;
        let (_, mut feed) = f.subscribe().await;

        let (conv, sid) = f.new_session(f.folder_id).await;
        let root = f.root_id.clone();
        feed.until(
            "created",
            session_is(sid.clone(), move |s| {
                s["workspace_id"] == root.as_str() && s["live"] == false
            }),
        )
        .await;

        // Legacy mutations, through their own signals.
        update_conversation_title_core(&f.state.db.conn, conv, "Renamed".into())
            .await
            .unwrap();
        emit_conversation_upsert(&f.state.emitter, &f.state.db.conn, conv).await;
        feed.until("title", session_is(sid.clone(), |s| s["title"] == "Renamed"))
            .await;

        update_conversation_status_core(&f.state.db.conn, conv, "completed".into())
            .await
            .unwrap();
        emit_conversation_upsert(&f.state.emitter, &f.state.db.conn, conv).await;
        feed.until("status", session_is(sid.clone(), |s| s["status"] == "completed"))
            .await;

        // An unchanged re-announcement publishes nothing.
        emit_conversation_upsert(&f.state.emitter, &f.state.db.conn, conv).await;
        assert!(feed.drain(500).await.is_empty());

        delete_conversation_with_cleanup_core(&f.state.emitter, &f.state.db.conn, conv)
            .await
            .unwrap();
        let ev = feed
            .until("session.removed", is("session.removed", "session_id", sid.clone()))
            .await;
        assert_eq!(ev.payload["workspace_id"], f.root_id.as_str());
    }

    #[tokio::test]
    async fn live_follows_the_agent_connection() {
        let f = fixture().await;
        let (conv, sid) = f.new_session(f.folder_id).await;
        let (_, mut feed) = f.subscribe().await;

        let _rx = f
            .state
            .connection_manager
            .insert_test_connection_live(
                CONN,
                AgentType::ClaudeCode,
                Some(f.root.clone().into()),
                f.state.emitter.clone(),
            )
            .await;
        let st = f.state.connection_manager.get_state(CONN).await.unwrap();
        st.write().await.conversation_id = Some(conv);
        f.emit(AcpEvent::StatusChanged {
            status: ConnectionStatus::Connected,
        })
        .await;
        feed.until("attached", session_is(sid.clone(), |s| s["live"] == true))
            .await;

        // Working: the lifecycle marks the row in progress.
        f.emit(AcpEvent::ConversationStatusChanged {
            conversation_id: conv,
            status: crate::db::entities::conversation::ConversationStatus::InProgress,
        })
        .await;
        crate::db::service::conversation_service::update_status(
            &f.state.db.conn,
            conv,
            crate::db::entities::conversation::ConversationStatus::InProgress,
        )
        .await
        .unwrap();
        emit_conversation_upsert(&f.state.emitter, &f.state.db.conn, conv).await;
        feed.until(
            "working",
            session_is(sid.clone(), |s| s["status"] == "in_progress" && s["live"] == true),
        )
        .await;

        // The agent exits: `Disconnected` is broadcast while the manager still
        // holds the connection, which it drops a moment later.
        f.emit(AcpEvent::StatusChanged {
            status: ConnectionStatus::Disconnected,
        })
        .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        f.state.connection_manager.disconnect(CONN).await.unwrap();
        feed.until("detached", session_is(sid.clone(), |s| s["live"] == false))
            .await;
    }

    #[tokio::test]
    async fn idle_sweep_detaches_the_session() {
        let f = fixture().await;
        let (conv, sid) = f.new_session(f.folder_id).await;
        let _rx = f
            .state
            .connection_manager
            .insert_test_connection_live(
                CONN,
                AgentType::ClaudeCode,
                Some(f.root.clone().into()),
                f.state.emitter.clone(),
            )
            .await;
        let st = f.state.connection_manager.get_state(CONN).await.unwrap();
        st.write().await.conversation_id = Some(conv);
        let (snapshot, mut feed) = f.subscribe().await;
        assert!(snapshot.sessions.iter().any(|s| s.session_id == sid && s.live));

        let state = st.clone();
        let emitter = f.state.emitter.clone();
        assert_eq!(f.state.connection_manager.sweep_idle(Duration::ZERO).await, 1);
        // What the connection task broadcasts on its way out.
        emit_with_state(
            &state,
            &emitter,
            AcpEvent::StatusChanged {
                status: ConnectionStatus::Disconnected,
            },
        )
        .await;
        feed.until("swept", session_is(sid.clone(), |s| s["live"] == false))
            .await;
    }

    #[tokio::test]
    async fn git_init_in_a_plain_workspace_makes_it_a_repository() {
        let f = fixture().await;
        let plain = f.dir.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        let plain = plain.canonicalize().unwrap();
        let folder = seed_folder(&f.state.db, plain.to_str().unwrap()).await;
        let plain_id = workspace::workspace_id_of(&f.state.db.conn, folder)
            .await
            .unwrap()
            .unwrap();
        let (snapshot, mut feed) = f.subscribe().await;
        let listed = serde_json::to_value(&snapshot).unwrap();
        let listed = listed["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["workspace_id"] == plain_id.as_str())
            .unwrap()
            .clone();
        assert_eq!(listed["is_git_repo"], false);
        tokio::time::sleep(Duration::from_millis(500)).await;
        feed.drain(300).await;

        git(&plain, &["init", "-q", "-b", "main"]);
        feed.until(
            "repository",
            workspace_is(plain_id, |w| w["is_git_repo"] == true && w["branch"] == "main"),
        )
        .await;
    }

    #[tokio::test]
    async fn terminals_follow_spawn_exit_and_kill() {
        let f = fixture().await;
        let (_, mut feed) = f.subscribe().await;

        let exits = f.spawn_terminal();
        let ev = feed
            .until("spawned", |e| {
                e.kind == "terminal.changed" && e.payload["terminal"]["terminal_id"] == exits.as_str()
            })
            .await;
        assert_eq!(ev.payload["terminal"]["title"], "Terminal");
        f.state.terminal_manager.write(&exits, b"exit\n").unwrap();
        feed.until("exited", is("terminal.removed", "terminal_id", exits.clone()))
            .await;

        let killed = f.spawn_terminal();
        feed.until("spawned", |e| {
            e.kind == "terminal.changed" && e.payload["terminal"]["terminal_id"] == killed.as_str()
        })
        .await;
        f.state.terminal_manager.kill(&killed).unwrap();
        feed.until("killed", is("terminal.removed", "terminal_id", killed.clone()))
            .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_reports_diff_and_branch_changes() {
        let f = fixture().await;
        let wt = f.create_worktree().await;
        let (_, mut feed) = f.subscribe().await;
        // Let the watchers settle.
        tokio::time::sleep(Duration::from_millis(500)).await;
        feed.drain(300).await;
        let repo = Path::new(&f.root);
        let root_diff = is("workspace.diff_changed", "workspace_id", f.root_id.clone());

        // Many files at once: invalidations, not one event per file.
        for i in 0..20 {
            std::fs::write(repo.join(format!("new-{i}.txt")), "x").unwrap();
        }
        feed.until("diff after create", &root_diff).await;
        let burst = feed.drain(1500).await;
        assert!(
            burst.iter().filter(|e| root_diff(e)).count() <= 1,
            "one invalidation per batch: {burst:?}"
        );
        assert!(burst.iter().all(|e| e.kind != "workspace.changed"), "{burst:?}");

        std::fs::write(repo.join("tracked.txt"), "two\n").unwrap();
        feed.until("diff after modify", &root_diff).await;
        std::fs::remove_file(repo.join("new-0.txt")).unwrap();
        feed.until("diff after delete", &root_diff).await;
        git(repo, &["add", "tracked.txt"]);
        feed.until("diff after stage", &root_diff).await;
        git(repo, &["commit", "-q", "-m", "change"]);
        feed.until("diff after commit", &root_diff).await;
        feed.drain(800).await;

        // Ignored files never invalidate the diff.
        std::fs::create_dir(repo.join("ignored")).unwrap();
        std::fs::write(repo.join("ignored/build.log"), "x").unwrap();
        let quiet = feed.drain(1500).await;
        assert!(quiet.iter().all(|e| e.kind != "workspace.diff_changed"), "{quiet:?}");

        // A checkout moves the branch.
        git(repo, &["checkout", "-q", "-b", "feature"]);
        feed.until(
            "branch",
            workspace_is(f.root_id.clone(), |w| w["branch"] == "feature"),
        )
        .await;

        // A linked worktree's own git metadata.
        let wt_path = Path::new(&wt.path);
        std::fs::write(wt_path.join("wt.txt"), "x").unwrap();
        let wt_diff = is("workspace.diff_changed", "workspace_id", wt.workspace_id.clone());
        feed.until("worktree diff", &wt_diff).await;
        git(wt_path, &["add", "wt.txt"]);
        feed.until("worktree stage", &wt_diff).await;
        git(wt_path, &["checkout", "-q", "-b", "renamed"]);
        feed.until(
            "worktree branch",
            workspace_is(wt.workspace_id.clone(), |w| w["branch"] == "renamed"),
        )
        .await;
        // An agent renaming its branch in a shell, not with the tool.
        git(wt_path, &["branch", "-m", "named-by-agent"]);
        feed.until(
            "worktree rename",
            workspace_is(wt.workspace_id.clone(), |w| w["branch"] == "named-by-agent"),
        )
        .await;
    }

    #[tokio::test]
    async fn lost_inputs_are_recovered_by_a_full_reread() {
        let f = fixture().await;
        let (_, mut feed) = f.subscribe().await;
        // A change that announces nothing, then more inputs than the feed can
        // buffer: the lag alone must bring the projection back in line.
        let conv = seed_conversation(&f.state.db, f.folder_id, AgentType::ClaudeCode).await;
        for _ in 0..5000 {
            f.state.event_broadcaster.send("noise://tick", &json!({}));
        }
        let sid = crate::db::entities::conversation::Entity::find_by_id(conv)
            .one(&f.state.db.conn)
            .await
            .unwrap()
            .unwrap()
            .session_id
            .unwrap();
        feed.until("recovered session", session_is(sid, |_| true)).await;
    }

    #[tokio::test]
    async fn seq_is_contiguous_and_a_restart_is_a_new_epoch() {
        let f = fixture().await;
        let (first, mut feed) = f.subscribe().await;
        for _ in 0..3 {
            f.new_session(f.folder_id).await;
        }
        for _ in 0..3 {
            feed.next().await;
        }
        assert_eq!(feed.seq, 3);

        // A second subscriber starts where the first one is.
        let (again, _) = f.subscribe().await;
        assert_eq!((again.epoch.as_str(), again.seq), (first.epoch.as_str(), 3));
        assert_eq!(again.sessions.len(), 3);

        // Restart: same state, new generation, seq from zero.
        let data_dir = f.state.data_dir.clone();
        let db = crate::db::init_database(&data_dir, "test").await.unwrap();
        let restarted = Arc::new(AppState::new_for_test(db, data_dir));
        let (snapshot, _) = restarted.host_feed.subscribe(&restarted).await.unwrap();
        assert_ne!(snapshot.epoch, first.epoch);
        assert_eq!(snapshot.seq, 0);
        assert_eq!(snapshot.workspaces, again.workspaces);
        assert_eq!(snapshot.sessions, again.sessions);
    }

    // ── pull requests ───────────────────────────────────────────────────

    /// A stand-in `gh`: `pr view` answers with `<dir>/pr.json`, or "no pull
    /// request" while it does not exist. Calls are logged to `<dir>/calls`.
    fn fake_gh(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let script = dir.join("gh");
        let d = dir.display();
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
echo "$*" >> '{d}/calls'
case "$*" in
"repo view"*) echo '{{"squashMergeAllowed":true,"mergeCommitAllowed":false,"rebaseMergeAllowed":false}}' ;;
"pr view"*) if [ -f '{d}/pr.json' ]; then cat '{d}/pr.json'; else echo 'no pull requests found for branch "x"' >&2; exit 1; fi ;;
*) echo "unexpected: $*" >&2; exit 1 ;;
esac
"#
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    fn set_pr(dir: &Path, state: &str, merge: &str, checks: &[(&str, &str)]) {
        let rollup: Vec<Value> = checks
            .iter()
            .map(|(status, conclusion)| {
                json!({"__typename": "CheckRun", "name": "ci", "status": status, "conclusion": conclusion})
            })
            .collect();
        let pr = json!({
            "number": 12, "title": "Feature", "state": state,
            "url": "https://github.com/o/r/pull/12", "isDraft": false,
            "baseRefName": "main", "headRefName": "feature", "headRefOid": "abc",
            "mergeStateStatus": merge, "statusCheckRollup": rollup,
            "mergedAt": if state == "MERGED" { json!("2026-10-03T00:00:00Z") } else { Value::Null },
        });
        // Replace atomically: the watcher may be reading it.
        let tmp = dir.join("pr.json.tmp");
        std::fs::write(&tmp, pr.to_string()).unwrap();
        std::fs::rename(tmp, dir.join("pr.json")).unwrap();
    }

    fn pr_views(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("calls"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("pr view"))
            .count()
    }

    /// Wait for the `n`th lookup, then make sure no more follow at once.
    async fn settle_views(dir: &Path, n: usize) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while pr_views(dir) < n {
            assert!(tokio::time::Instant::now() < deadline, "no lookup {n}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert_eq!(pr_views(dir), n);
    }

    fn quick_prs(gh: PathBuf) -> PrWatchConfig {
        PrWatchConfig {
            gh: Some(gh),
            policy: pr_watch::PrPolicy {
                pending: Duration::from_millis(200),
                settled: Duration::from_millis(400),
                fresh: Duration::ZERO,
                max_backoff: Duration::from_secs(2),
            },
        }
    }

    /// A bare `origin` holding `main`.
    fn add_origin(f: &Fixture) {
        let origin = f.dir.path().join("origin.git");
        git(f.dir.path(), &["init", "-q", "--bare", origin.to_str().unwrap()]);
        let repo = Path::new(&f.root);
        git(repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
        git(repo, &["push", "-q", "origin", "main"]);
    }

    fn pr_is(id: String, check: impl Fn(&Value) -> bool) -> impl Fn(&HostEvent) -> bool {
        move |e| {
            e.kind == "workspace.pr_changed" && e.payload["workspace_id"] == id.as_str() && check(&e.payload)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_pull_request_is_followed_from_open_to_merged() {
        let gh_dir = tempfile::tempdir().unwrap();
        let gh_dir = gh_dir.path().to_path_buf();
        let f = fixture_with(quick_prs(fake_gh(&gh_dir))).await;
        add_origin(&f);
        git(Path::new(&f.root), &["checkout", "-q", "-b", "feature"]);
        let (snapshot, mut feed) = f.subscribe().await;
        assert!(snapshot.prs.is_empty());

        // No pull request: looked up once at start, then never polled.
        settle_views(&gh_dir, 1).await;
        assert!(feed.drain(100).await.iter().all(|e| e.kind != "workspace.pr_changed"));

        // Opened: a refresh finds it, every subscriber hears it.
        set_pr(&gh_dir, "OPEN", "BLOCKED", &[("IN_PROGRESS", "")]);
        let entry = f.state.host_feed.pr(&f.state, &f.root_id, true).await.unwrap();
        let pr = entry.pr.unwrap();
        assert_eq!((pr.number, pr.checks.pending), (12, 1));
        let opened = feed
            .until("opened", pr_is(f.root_id.clone(), |p| p["pr"]["state"] == "open"))
            .await;
        assert_eq!(opened.payload["pr"]["checks"]["pending"], 1);
        assert_eq!(opened.payload["pr"]["merge"]["allowed"], false);
        assert!(opened.payload["error"].is_null());

        // Checks pass: the poll notices.
        set_pr(&gh_dir, "OPEN", "CLEAN", &[("COMPLETED", "SUCCESS")]);
        let passed = feed.next().await;
        assert_eq!(passed.kind, "workspace.pr_changed");
        assert_eq!(passed.payload["pr"]["checks"]["passed"], 1);
        assert_eq!(passed.payload["pr"]["merge"]["allowed"], true);
        assert_eq!(passed.payload["pr"]["merge"]["method"], "squash");

        // Merged: published once, then no more polling.
        set_pr(&gh_dir, "MERGED", "UNKNOWN", &[("COMPLETED", "SUCCESS")]);
        let merged = feed.next().await;
        assert_eq!(merged.kind, "workspace.pr_changed");
        assert_eq!(merged.payload["pr"]["state"], "merged");
        assert_eq!(merged.seq, passed.seq + 1);
        assert_eq!(passed.seq, opened.seq + 1);
        let views = pr_views(&gh_dir);
        let after = feed.drain(1500).await;
        assert!(after.is_empty(), "{after:?}");
        assert_eq!(pr_views(&gh_dir), views, "a merged pull request is not polled");

        // A late subscriber sees it in the snapshot.
        let (late, _) = f.subscribe().await;
        assert_eq!(late.prs[&f.root_id].pr.as_ref().unwrap().state, crate::git_harness::PrLifecycle::Merged);
        assert_eq!(f.state.host_feed.pr(&f.state, &f.root_id, false).await.unwrap(), late.prs[&f.root_id]);

        // Back on the default branch: the pull request is gone.
        git(Path::new(&f.root), &["checkout", "-q", "main"]);
        feed.until("cleared", pr_is(f.root_id.clone(), |p| p["pr"].is_null())).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_new_pull_request_is_polled_until_its_checks_show_up() {
        let gh_dir = tempfile::tempdir().unwrap();
        let gh_dir = gh_dir.path().to_path_buf();
        let mut config = quick_prs(fake_gh(&gh_dir));
        // As live: a settled pull request waits much longer than a pending one.
        config.policy.settled = Duration::from_secs(60);
        config.policy.fresh = Duration::from_secs(60);
        let f = fixture_with(config).await;
        add_origin(&f);
        git(Path::new(&f.root), &["checkout", "-q", "-b", "feature"]);
        let (_, mut feed) = f.subscribe().await;
        settle_views(&gh_dir, 1).await;

        // Just opened: GitHub reports it clean, with no checks yet.
        set_pr(&gh_dir, "OPEN", "CLEAN", &[]);
        f.state.host_feed.pr(&f.state, &f.root_id, true).await.unwrap();
        feed.until("opened", pr_is(f.root_id.clone(), |p| p["pr"]["state"] == "open")).await;

        // CI registers its runs: seen at the pending pace, not a minute later.
        set_pr(&gh_dir, "OPEN", "UNSTABLE", &[("IN_PROGRESS", "")]);
        let running = feed
            .until("checks", pr_is(f.root_id.clone(), |p| p["pr"]["checks"]["pending"] == 1))
            .await;
        assert_eq!(running.payload["pr"]["checks"]["pending"], 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn gh_logged_out_is_published_once() {
        let gh_dir = tempfile::tempdir().unwrap();
        let gh_dir = gh_dir.path().to_path_buf();
        std::fs::create_dir_all(&gh_dir).unwrap();
        let gh = gh_dir.join("gh");
        std::fs::write(
            &gh,
            format!("#!/bin/sh\necho \"$*\" >> '{}/calls'\necho 'To get started with GitHub CLI, please run:  gh auth login' >&2\nexit 4\n", gh_dir.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let f = fixture_with(quick_prs(gh)).await;
        add_origin(&f);
        git(Path::new(&f.root), &["checkout", "-q", "-b", "feature"]);
        let (_, mut feed) = f.subscribe().await;
        let ev = feed
            .until("logged out", pr_is(f.root_id.clone(), |p| p["error"]["code"] == "gh_unauthenticated"))
            .await;
        assert!(ev.payload["pr"].is_null());
        settle_views(&gh_dir, 1).await;
        assert!(feed.drain(100).await.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_push_from_a_worktree_finds_its_pull_request() {
        let gh_dir = tempfile::tempdir().unwrap();
        let gh_dir = gh_dir.path().to_path_buf();
        let f = fixture_with(quick_prs(fake_gh(&gh_dir))).await;
        add_origin(&f);
        let wt = f.create_worktree().await;
        let branch = wt.branch.clone().expect("a worktree is on a branch");
        let (_, mut feed) = f.subscribe().await;
        // The worktree's branch is looked up at start (no PR yet); the root
        // is on the default branch and never is.
        settle_views(&gh_dir, 1).await;
        feed.drain(100).await;

        // The agent pushes from the worktree and opens the pull request with
        // its own tools: only the remote-tracking ref in the common git dir
        // moves.
        set_pr(&gh_dir, "OPEN", "BLOCKED", &[("QUEUED", "")]);
        git(Path::new(&wt.path), &["push", "-q", "origin", &branch]);
        let ev = feed
            .until("pushed", pr_is(wt.workspace_id.clone(), |p| p["pr"]["number"] == 12))
            .await;
        assert_eq!(ev.payload["pr"]["checks"]["pending"], 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn continue_publishes_the_workspace_its_pr_and_its_diff() {
        let gh_dir = tempfile::tempdir().unwrap();
        let gh_dir = gh_dir.path().to_path_buf();
        let gh = gh_dir.join("gh");
        let f = fixture_with(quick_prs(gh.clone())).await;
        let repo = Path::new(&f.root);
        // The Harness's git reads the user's config: keep their hooks out.
        let hooks = f.dir.path().join("hooks");
        std::fs::create_dir(&hooks).unwrap();
        git(repo, &["config", "core.hooksPath", hooks.to_str().unwrap()]);
        add_origin(&f);
        let wt = f.create_worktree().await;
        let branch = wt.branch.clone().unwrap();
        let wt_path = Path::new(&wt.path);

        // The worktree's pull request is pushed, then merged on GitHub.
        std::fs::write(wt_path.join("feature.txt"), "x").unwrap();
        git(wt_path, &["add", "feature.txt"]);
        git(wt_path, &["commit", "-q", "-m", "feature"]);
        git(wt_path, &["push", "-q", "origin", &branch]);
        let head = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(wt_path)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        let origin = f.dir.path().join("origin.git");
        git(f.dir.path(), &["clone", "-q", "-b", "main", origin.to_str().unwrap(), "other"]);
        let other = f.dir.path().join("other");
        git(&other, &["merge", "-q", "--no-ff", "-m", "merge", &format!("origin/{branch}")]);
        git(&other, &["push", "-q", "origin", "main"]);
        let pr = json!({
            "number": 12, "title": "Feature", "state": "MERGED",
            "url": "https://github.com/o/r/pull/12", "isDraft": false,
            "baseRefName": "main", "headRefName": branch, "headRefOid": head,
            "mergeStateStatus": "UNKNOWN", "statusCheckRollup": [],
            "mergedAt": "2026-10-03T00:00:00Z",
        });
        std::fs::write(
            &gh,
            format!(
                "#!/bin/sh\ncase \"$*\" in\n\"pr view {branch} \"*) echo '{pr}' ;;\n\"repo view\"*) echo '{{\"squashMergeAllowed\":true,\"mergeCommitAllowed\":false,\"rebaseMergeAllowed\":false}}' ;;\n*) echo 'no pull requests found' >&2; exit 1 ;;\nesac\n"
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();

        let (_, mut feed) = f.subscribe().await;
        f.state.host_feed.pr(&f.state, &wt.workspace_id, true).await.unwrap();
        feed.until("merged", pr_is(wt.workspace_id.clone(), |p| p["pr"]["state"] == "merged"))
            .await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        feed.drain(300).await;

        let host = crate::git_harness::GitHost {
            db: &f.state.db.conn,
            data_dir: &f.state.data_dir,
            gh: Some(&gh),
        };
        let info = workspace::continue_after_merge(
            &f.state.db,
            &f.state.emitter,
            &f.state.connection_manager,
            &host,
            &wt.workspace_id,
            12,
        )
        .await
        .unwrap();
        let (new_branch, base_commit) = (info.branch.unwrap(), info.base_commit.unwrap());

        // The pass that publishes the new baseline also invalidates the diff
        // and clears the merged pull request.
        feed.until(
            "continued",
            workspace_is(wt.workspace_id.clone(), |w| {
                w["branch"] == new_branch.as_str()
                    && w["base_branch"] == "origin/main"
                    && w["base_commit"] == base_commit.as_str()
            }),
        )
        .await;
        let diff = feed.next().await;
        assert!(
            is("workspace.diff_changed", "workspace_id", wt.workspace_id.clone())(&diff),
            "{diff:?}"
        );
        feed.until("pr cleared", pr_is(wt.workspace_id.clone(), |p| p["pr"].is_null()))
            .await;
    }

    // ── clones ──────────────────────────────────────────────────────────

    const CLONE_GH: &str = r#""api user") echo '{"login":"octo","id":42,"name":"Octo Cat"}';;
"repo clone octo/app"*) printf 'Receiving objects:  50%% (1/2)\r' >&2; git clone -q 'file://{bare}' "$4" && git -C "$4" remote set-url origin https://github.com/octo/app.git && git -C "$4" config 'url.file://{bare}.insteadOf' https://github.com/octo/app.git;;
"repo clone acme/"*) sleep 30;;"#;

    fn clone_is(id: String, state: &'static str) -> impl Fn(&HostEvent) -> bool {
        move |e| {
            e.kind == "clone.changed"
                && e.payload["clone"]["clone_id"] == id.as_str()
                && e.payload["clone"]["state"] == state
        }
    }

    async fn start_clone(f: &Fixture, owner: &str, name: &str) -> String {
        match f
            .state
            .clones
            .start(f.state.clone_env(), owner, name)
            .await
            .unwrap()
        {
            crate::workspace::clone::CloneStart::Job { clone_id } => clone_id,
            other => panic!("expected a job, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_clone_streams_forward_and_is_done_after_its_workspaces() {
        let f = fixture_full(PrWatchConfig::default(), Some(CLONE_GH)).await;
        let (snapshot, mut feed) = f.subscribe().await;
        assert!(snapshot.clones.is_empty());
        let id = start_clone(&f, "octo", "app").await;

        let order = ["queued", "cloning", "adding", "done"];
        let mut seen: Vec<&str> = Vec::new();
        let mut listed = std::collections::HashSet::new();
        let done = loop {
            let ev = feed.next().await;
            match ev.kind {
                "workspace.changed" => {
                    listed.insert(
                        ev.payload["workspace"]["workspace_id"]
                            .as_str()
                            .unwrap()
                            .to_string(),
                    );
                }
                "clone.changed" if ev.payload["clone"]["clone_id"] == id.as_str() => {
                    let state = ev.payload["clone"]["state"].as_str().unwrap();
                    assert_ne!(state, "failed", "{}", ev.payload);
                    let at = order.iter().position(|s| *s == state).unwrap();
                    if let Some(last) = seen.last() {
                        assert!(
                            order.iter().position(|s| s == last).unwrap() <= at,
                            "{seen:?} then {state}"
                        );
                    }
                    seen.push(order[at]);
                    if state == "done" {
                        break ev.payload["clone"].clone();
                    }
                }
                _ => {}
            }
        };
        // R4: both workspaces were announced before `done`.
        assert!(
            listed.contains(done["workspace_id"].as_str().unwrap()),
            "{done}"
        );
        assert!(
            listed.contains(done["worktree_id"].as_str().unwrap()),
            "{done}"
        );
        // A fresh snapshot carries the finished job (reconnect recovery).
        let (snapshot, _again) = f.subscribe().await;
        assert_eq!(snapshot.clones.len(), 1);
        assert_eq!(snapshot.clones[0].clone_id, id);
        // Removed after the test retention (2 s).
        feed.until("clone.removed", is("clone.removed", "clone_id", id.clone()))
            .await;
        let (snapshot, _) = f.subscribe().await;
        assert!(snapshot.clones.is_empty());
    }

    #[tokio::test]
    async fn a_reconnect_mid_clone_sees_it_in_the_snapshot_and_cancel_is_published() {
        let f = fixture_full(PrWatchConfig::default(), Some(CLONE_GH)).await;
        let (_, mut feed) = f.subscribe().await;
        let id = start_clone(&f, "acme", "big").await;
        feed.until("cloning", clone_is(id.clone(), "cloning")).await;
        let (snapshot, _) = f.subscribe().await;
        let job = snapshot
            .clones
            .iter()
            .find(|j| j.clone_id == id)
            .expect("clone in snapshot");
        assert_eq!(job.state, crate::workspace::clone::CloneState::Cloning);
        assert!(f.state.clones.cancel(&id).unwrap());
        feed.until("cancelled", clone_is(id, "cancelled")).await;
    }
}
