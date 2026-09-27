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
use tokio::sync::{mpsc, OnceCell};

use crate::acp::manager::ConnectionManager;
use crate::acp::types::{AcpEvent, ConnectionStatus, EventEnvelope};
use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::protocol::http::SessionSummary;
use crate::session;
use crate::terminal::manager::TerminalManager;
use crate::web::event_bridge::{
    EventEmitter, WebEvent, CONVERSATIONS_BULK_CHANGED_EVENT, CONVERSATION_CHANGED_EVENT,
    FOLDER_CHANGED_EVENT,
};
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
}

#[derive(Debug, Clone, Default)]
struct Projection {
    seq: u64,
    /// In `workspace.list` order.
    workspaces: Vec<WorkspaceInfo>,
    /// `session.list` of each listed workspace.
    sessions: HashMap<String, Vec<SessionSummary>>,
    terminals: BTreeMap<String, TerminalSummary>,
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
        }
    }
}

struct Shared {
    projection: Mutex<Projection>,
    tx: broadcast::Sender<Arc<HostEvent>>,
}

/// The host's feed. Started by its first subscriber; runs for the life of
/// the process.
pub struct HostFeed {
    epoch: String,
    shared: OnceCell<Arc<Shared>>,
}

impl Default for HostFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl HostFeed {
    pub fn new() -> Self {
        Self {
            epoch: uuid::Uuid::new_v4().simple().to_string(),
            shared: OnceCell::new(),
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
        let shared = self.shared.get_or_try_init(|| start(state)).await?;
        // Publishing holds this lock across `seq` assignment and send, so the
        // receiver sees exactly the events after the snapshot.
        let projection = shared.projection.lock().unwrap_or_else(|e| e.into_inner());
        let rx = shared.tx.subscribe();
        Ok((projection.snapshot(&self.epoch), rx))
    }
}

async fn start(state: &Arc<AppState>) -> Result<Arc<Shared>, AppCommandError> {
    // Subscribe before the first read so no change between them is lost.
    let web_rx = state.event_broadcaster.subscribe();
    let bus_rx = state.acp_event_bus.subscribe();
    let mut task = FeedTask {
        db: state.db.conn.clone(),
        connections: state.connection_manager.clone_ref(),
        terminals: state.terminal_manager.clone_ref(),
        emitter: state.emitter.clone(),
        projection: Projection::default(),
        watched: HashMap::new(),
        watch_keys: HashMap::new(),
    };
    task.projection.workspaces = workspace::list(&task.db).await?;
    let ids: Vec<String> = task
        .projection
        .workspaces
        .iter()
        .map(|w| w.workspace_id.clone())
        .collect();
    task.projection.sessions = task.read_sessions(&ids, &HashMap::new()).await;
    task.projection.terminals = task.read_terminals(&task.projection.workspaces);
    task.sync_watchers().await;

    let (tx, _) = broadcast::channel(SUBSCRIBER_CAPACITY);
    let shared = Arc::new(Shared {
        projection: Mutex::new(task.projection.clone()),
        tx,
    });
    tokio::spawn(task.run(shared.clone(), web_rx, bus_rx));
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
}

impl Stale {
    fn any(&self) -> bool {
        self.workspaces
            || self.sessions
            || self.terminals
            || self.all_diffs
            || !self.watched.is_empty()
            || !self.departed.is_empty()
    }

    fn everything(&mut self) {
        self.workspaces = true;
        self.sessions = true;
        self.terminals = true;
        self.all_diffs = true;
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

impl FeedTask {
    async fn run(
        mut self,
        shared: Arc<Shared>,
        mut web_rx: broadcast::Receiver<WebEvent>,
        mut bus_rx: broadcast::Receiver<Arc<EventEnvelope>>,
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

        // Workspaces: the whole list, or just the ones whose git metadata moved.
        let mut workspaces = old.workspaces.clone();
        let mut relist = stale.workspaces;
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
            let changed = if stale.all_diffs {
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

        let events = diff_events(old, &workspaces, &sessions, &terminals, &diffs);
        self.projection = Projection {
            seq: old.seq,
            workspaces,
            sessions,
            terminals,
        };
        self.publish(shared, events);
        self.sync_watchers().await;

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
    use std::path::Path;

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
        let state = Arc::new(AppState::new_for_test(db, data_dir));
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
        let added = workspace::add(&f.state.db, &f.state.emitter, other.to_str().unwrap())
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
        workspace::add(&f.state.db, &f.state.emitter, other.to_str().unwrap())
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
}
