//! Saved agent plans: each turn's step list, kept in the session's worktree
//! so a dropped network, a closed laptop or a Host crash does not lose it.
//!
//! A plan lives at `<worktree>/.zwiim/tasks/<session_id>/<plan_id>.json`.
//! Every session has its own folder and every turn that reports steps gets a
//! new `plan_id`, so no two plans share a file; reports within one turn
//! rewrite that turn's file with the next `revision`. The folder is excluded
//! from Git and goes with the worktree when it is removed.
//!
//! Only the Host writes a plan, from the session's own events (see
//! [`spawn_tracker`]). A plan whose turn ended is final: completed when the
//! agent ended its turn, cancelled when the user stopped it, interrupted for
//! any other end. A final plan can be read and never changed.
//!
//! A plan still marked running when no turn is running had its turn cut short
//! by a Host that stopped: [`saved_progress`] reports it as interrupted.
//!
//! A plan is not tied to the agent that made it. `session.continue_plan`
//! starts a new session, with any agent, to carry out the steps a plan left
//! undone; the new session's folder then names that plan in its
//! `continues` file (see [`save_link`]). The old plan stays as it was.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;

use crate::acp::progress::{Progress, ProgressStep, StepStatus};
use crate::acp::types::{AcpEvent, ConnectionStatus, EventEnvelope, PlanEntryInfo};
use crate::app_state::AppState;
use crate::db::entities::conversation;
use crate::models::agent::AgentType;
use crate::models::message::{MessageTurn, TurnRole};
use crate::workspace::zwiim_dir;

/// Version of the plan file.
const VERSION: u32 = 1;
/// Plans kept per session; the oldest go first.
const KEEP_PLANS: usize = 50;
/// In a session's folder: the plan the session was started to continue.
const CONTINUES_FILE: &str = "continues";
/// In a session's folder: the new session that took over one of its plans.
pub const CONTINUED_IN_FILE: &str = "continued-in";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanState {
    Running,
    Completed,
    Interrupted,
    Cancelled,
}

impl PlanState {
    /// Whether the plan's turn is over: it can no longer change.
    pub fn is_final(self) -> bool {
        self != Self::Running
    }

    /// How a turn that ended with `stop_reason` leaves its plan.
    fn after_turn(stop_reason: &str) -> Self {
        match stop_reason {
            "end_turn" => Self::Completed,
            "cancelled" => Self::Cancelled,
            _ => Self::Interrupted,
        }
    }
}

/// One turn's plan as it is saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRecord {
    pub v: u32,
    pub plan_id: String,
    pub session_id: String,
    /// The agent that made the plan.
    pub agent: AgentType,
    /// 1 for the first report of the turn, one more for each after it.
    pub revision: u32,
    pub state: PlanState,
    pub steps: Vec<ProgressStep>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl PlanRecord {
    fn start(session_id: &str, agent: AgentType, progress: Progress) -> Self {
        let now = now_ms();
        Self {
            v: VERSION,
            plan_id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            agent,
            revision: 1,
            state: PlanState::Running,
            steps: progress.steps,
            created_at_ms: now,
            updated_at_ms: now,
        }
    }

    fn revise(&mut self, progress: Progress) {
        self.steps = progress.steps;
        self.revision += 1;
        self.updated_at_ms = now_ms();
    }

    fn finish(&mut self, state: PlanState) {
        self.state = state;
        self.revision += 1;
        self.updated_at_ms = now_ms();
    }

    pub fn progress(&self) -> Progress {
        Progress {
            steps: self.steps.clone(),
        }
    }

    /// Whether any step is still to be done.
    pub fn is_unfinished(&self) -> bool {
        self.steps
            .iter()
            .any(|step| step.status != StepStatus::Completed)
    }

    /// A record read from disk is used only if it is whole and within the
    /// same limits a live report is held to.
    fn is_valid(&self, session_id: &str) -> bool {
        self.v == VERSION
            && self.session_id == session_id
            && is_safe_id(&self.plan_id)
            && Progress::new(self.steps.clone()).is_ok_and(|p| p.steps == self.steps)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("This plan is finished and can't be changed.")]
    Final,
    #[error("invalid plan or session id")]
    InvalidId,
    #[error("plan file: {0}")]
    Io(#[from] std::io::Error),
    #[error("plan encoding: {0}")]
    Encode(#[from] serde_json::Error),
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Ids become path segments: letters, digits and `-` only.
fn is_safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn session_dir(root: &Path, session_id: &str) -> Result<PathBuf, PlanError> {
    if !is_safe_id(session_id) {
        return Err(PlanError::InvalidId);
    }
    Ok(root.join(zwiim_dir::TASKS).join(session_id))
}

fn plan_path(root: &Path, session_id: &str, plan_id: &str) -> Result<PathBuf, PlanError> {
    if !is_safe_id(plan_id) {
        return Err(PlanError::InvalidId);
    }
    Ok(session_dir(root, session_id)?.join(format!("{plan_id}.json")))
}

async fn read(path: &Path, session_id: &str) -> Option<PlanRecord> {
    let bytes = tokio::fs::read(path).await.ok()?;
    let record: PlanRecord = serde_json::from_slice(&bytes).ok()?;
    let named = path.file_stem().and_then(|s| s.to_str()) == Some(record.plan_id.as_str());
    (named && record.is_valid(session_id)).then_some(record)
}

/// Save `record`, replacing its previous revision. Refuses to touch a plan
/// already saved as final. The file is written whole and then moved into
/// place, so a crash leaves the old revision or the new one, never half.
pub async fn save(root: &Path, record: &PlanRecord) -> Result<(), PlanError> {
    let dir = session_dir(root, &record.session_id)?;
    let path = plan_path(root, &record.session_id, &record.plan_id)?;
    if let Some(saved) = read(&path, &record.session_id).await {
        if saved.state.is_final() {
            return Err(PlanError::Final);
        }
    }
    write_whole(root, &dir, &path, &serde_json::to_vec_pretty(record)?).await?;
    if record.state.is_final() {
        make_read_only(&path).await;
    }
    Ok(())
}

/// Write `bytes` to `path` in `dir` (created if missing) through a temporary
/// file, so a crash leaves the old file or the new one, never half.
async fn write_whole(root: &Path, dir: &Path, path: &Path, bytes: &[u8]) -> Result<(), PlanError> {
    if !tokio::fs::try_exists(dir).await.unwrap_or(false) {
        // Older worktrees and the main checkout have no excluded `.zwiim/` yet.
        crate::commands::folder_links::add_to_git_exclude(root, ".zwiim/").await;
        tokio::fs::create_dir_all(dir).await?;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("plan");
    let part = dir.join(format!(".{name}.part"));
    tokio::fs::write(&part, bytes).await?;
    if let Err(e) = tokio::fs::rename(&part, path).await {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e.into());
    }
    Ok(())
}

/// Best-effort: the refusal in [`save`] is the rule; this only keeps other
/// programs from editing a final plan by accident.
async fn make_read_only(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let read_only = std::fs::Permissions::from_mode(0o444);
        if let Err(e) = tokio::fs::set_permissions(path, read_only).await {
            tracing::debug!("[plans] could not make {} read-only: {e}", path.display());
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// The session's saved plans, oldest first. Unreadable files are skipped.
async fn list(root: &Path, session_id: &str) -> Vec<PlanRecord> {
    let Ok(dir) = session_dir(root, session_id) else {
        return Vec::new();
    };
    let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
        return Vec::new();
    };
    let mut plans = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            if let Some(record) = read(&path, session_id).await {
                plans.push(record);
            }
        }
    }
    plans.sort_by(|a, b| (a.created_at_ms, &a.plan_id).cmp(&(b.created_at_ms, &b.plan_id)));
    plans
}

/// The session's newest plan.
pub async fn latest(root: &Path, session_id: &str) -> Option<PlanRecord> {
    list(root, session_id).await.pop()
}

/// The plan, in another session, that a session was started to continue,
/// with enough of that session to name it without opening it. Links saved
/// before the extra fields existed read as unknown (`None`, 0).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanLink {
    pub session_id: String,
    pub plan_id: String,
    /// The agent of the session the plan came from.
    #[serde(default)]
    pub agent: Option<AgentType>,
    /// Steps done when the plan stopped.
    #[serde(default)]
    pub completed: usize,
    /// Steps in the plan.
    #[serde(default)]
    pub total: usize,
    /// The title of the session the plan came from.
    #[serde(default)]
    pub title: Option<String>,
}

/// Record that session `session_id` continues `link`'s plan.
pub async fn save_link(root: &Path, session_id: &str, link: &PlanLink) -> Result<(), PlanError> {
    if !is_safe_id(&link.session_id) || !is_safe_id(&link.plan_id) {
        return Err(PlanError::InvalidId);
    }
    let dir = session_dir(root, session_id)?;
    let path = dir.join(CONTINUES_FILE);
    write_whole(root, &dir, &path, &serde_json::to_vec_pretty(link)?).await
}

/// The plan session `session_id` continues, if it was started to.
pub async fn link(root: &Path, session_id: &str) -> Option<PlanLink> {
    let path = session_dir(root, session_id).ok()?.join(CONTINUES_FILE);
    let bytes = tokio::fs::read(path).await.ok()?;
    let link: PlanLink = serde_json::from_slice(&bytes).ok()?;
    (is_safe_id(&link.session_id) && is_safe_id(&link.plan_id)).then_some(link)
}

/// The new session that took over one of this session's plans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuedIn {
    pub session_id: String,
    pub plan_id: String,
}

/// Record that session `session_id`'s plan was taken over by a new session.
pub async fn save_continued_in(
    root: &Path,
    session_id: &str,
    link: &ContinuedIn,
) -> Result<(), PlanError> {
    if !is_safe_id(&link.session_id) || !is_safe_id(&link.plan_id) {
        return Err(PlanError::InvalidId);
    }
    let dir = session_dir(root, session_id)?;
    let path = dir.join(CONTINUED_IN_FILE);
    write_whole(root, &dir, &path, &serde_json::to_vec_pretty(link)?).await
}

/// The new session that took over plan `plan_id` of session `session_id`,
/// only while it exists: deleting it frees the plan, and a link naming
/// another plan does not count.
pub async fn continued_in(
    conn: &DatabaseConnection,
    root: &Path,
    session_id: &str,
    plan_id: &str,
) -> Option<String> {
    let path = session_dir(root, session_id).ok()?.join(CONTINUED_IN_FILE);
    let bytes = tokio::fs::read(path).await.ok()?;
    let link: ContinuedIn = serde_json::from_slice(&bytes).ok()?;
    if link.plan_id != plan_id || !is_safe_id(&link.session_id) {
        return None;
    }
    crate::session::resolve(conn, &link.session_id)
        .await
        .ok()
        .map(|_| link.session_id)
}

/// Whether a client may offer to continue `plan`: its turn stopped short
/// (not finished on its own) with steps left, the session can still be
/// reached, no new session took it over, and the user has not written since
/// (a later user turn means the chat moved on).
pub fn is_resumable(
    plan: &PlanRecord,
    history: &[MessageTurn],
    unreachable: bool,
    continued: bool,
) -> bool {
    matches!(plan.state, PlanState::Interrupted | PlanState::Cancelled)
        && !unreachable
        && !continued
        && plan.is_unfinished()
        && !history.iter().any(|turn| {
            matches!(turn.role, TurnRole::User)
                && turn.timestamp.timestamp_millis() > plan.updated_at_ms
        })
}

/// A new plan begins: earlier plans of the session still marked running lost
/// their turn's end (a Host that stopped), so they are interrupted.
async fn interrupt_running(root: &Path, session_id: &str) {
    for mut record in list(root, session_id).await {
        if record.state == PlanState::Running {
            record.finish(PlanState::Interrupted);
            if let Err(e) = save(root, &record).await {
                tracing::warn!("[plans] could not close plan {}: {e}", record.plan_id);
            }
        }
    }
}

/// Keep the session's newest [`KEEP_PLANS`].
async fn prune(root: &Path, session_id: &str) {
    let plans = list(root, session_id).await;
    let excess = plans.len().saturating_sub(KEEP_PLANS);
    for record in &plans[..excess] {
        if let Ok(path) = plan_path(root, session_id, &record.plan_id) {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
}

/// Where a connection's plans go.
#[derive(Debug, Clone)]
pub(crate) struct PlanTarget {
    pub root: PathBuf,
    pub session_id: String,
    pub agent: AgentType,
}

/// Finds the session and worktree a connection's plans belong to. A trait so
/// the tracker can be tested without a running Host.
#[async_trait]
pub(crate) trait PlanTargets: Send + Sync {
    /// `None` for connections with no session of their own (delegated
    /// children and the Host's internal chats): their plans are not saved.
    async fn target(&self, connection_id: &str) -> Option<PlanTarget>;
}

struct OpenPlan {
    root: PathBuf,
    record: PlanRecord,
}

/// The plan each connection's running turn is writing, by connection id.
#[derive(Default)]
pub(crate) struct PlanTracker {
    open: HashMap<String, OpenPlan>,
}

impl PlanTracker {
    pub(crate) async fn observe(&mut self, targets: &dyn PlanTargets, envelope: &EventEnvelope) {
        let connection_id = envelope.connection_id.as_str();
        match &envelope.payload {
            AcpEvent::PlanUpdate { entries } => self.update(targets, connection_id, entries).await,
            AcpEvent::TurnComplete { stop_reason, .. } => {
                self.close(connection_id, PlanState::after_turn(stop_reason))
                    .await
            }
            AcpEvent::StatusChanged {
                status: ConnectionStatus::Disconnected,
            } => self.close(connection_id, PlanState::Interrupted).await,
            // A new prompt while a plan is open: the last turn's end was missed.
            AcpEvent::UserMessage { .. } => self.close(connection_id, PlanState::Interrupted).await,
            _ => {}
        }
    }

    async fn update(
        &mut self,
        targets: &dyn PlanTargets,
        connection_id: &str,
        entries: &[PlanEntryInfo],
    ) {
        // An empty list (an agent clearing its plan) or one past the limits
        // leaves the last saved revision as it was.
        let Ok(progress) = Progress::new(Progress::from_plan_entries(entries).steps) else {
            return;
        };
        if let Some(open) = self.open.get_mut(connection_id) {
            open.record.revise(progress);
            if let Err(e) = save(&open.root, &open.record).await {
                tracing::warn!("[plans] could not save plan {}: {e}", open.record.plan_id);
            }
            return;
        }
        let Some(target) = targets.target(connection_id).await else {
            return;
        };
        let record = PlanRecord::start(&target.session_id, target.agent, progress);
        interrupt_running(&target.root, &target.session_id).await;
        if let Err(e) = save(&target.root, &record).await {
            tracing::warn!("[plans] could not save plan {}: {e}", record.plan_id);
            return;
        }
        prune(&target.root, &target.session_id).await;
        self.open.insert(
            connection_id.to_string(),
            OpenPlan {
                root: target.root,
                record,
            },
        );
    }

    async fn close(&mut self, connection_id: &str, state: PlanState) {
        let Some(mut open) = self.open.remove(connection_id) else {
            return;
        };
        open.record.finish(state);
        if let Err(e) = save(&open.root, &open.record).await {
            tracing::warn!("[plans] could not close plan {}: {e}", open.record.plan_id);
        }
    }
}

/// Connections resolved through the running Host: the connection's
/// conversation row gives the session, the session gives the worktree.
struct HostPlanTargets {
    state: Arc<AppState>,
}

#[async_trait]
impl PlanTargets for HostPlanTargets {
    async fn target(&self, connection_id: &str) -> Option<PlanTarget> {
        let live = self
            .state
            .connection_manager
            .get_state(connection_id)
            .await?;
        let (conversation_id, agent) = {
            let live = live.read().await;
            (live.conversation_id?, live.agent_type)
        };
        let db = &self.state.db.conn;
        let row = conversation::Entity::find_by_id(conversation_id)
            .one(db)
            .await
            .ok()??;
        let session_id = row.session_id?;
        let root = crate::session::workspace_dir(db, &session_id).await?;
        Some(PlanTarget {
            root: PathBuf::from(root),
            session_id,
            agent,
        })
    }
}

/// Save each session's plans as its agent reports them.
pub fn spawn_tracker(state: Arc<AppState>) {
    let mut rx = state.acp_event_bus.subscribe();
    let targets = HostPlanTargets { state };
    tokio::spawn(async move {
        let mut tracker = PlanTracker::default();
        loop {
            match rx.recv().await {
                // Handled in order, one at a time: each revision is saved
                // before the next is read.
                Ok(envelope) => tracker.observe(&targets, &envelope).await,
                // A missed turn end is closed by the session's next prompt or
                // next plan.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            }
        }
    });
}

/// The session's newest plan for `session.history`, with its state as of
/// now: a plan saved as running whose session has no turn running was cut
/// short, so it reads as interrupted.
pub async fn saved_progress(
    state: &AppState,
    row: &conversation::Model,
    root: &Path,
) -> Option<PlanRecord> {
    let session_id = row.session_id.as_deref()?;
    let mut record = latest(root, session_id).await?;
    if record.state == PlanState::Running && !turn_running(state, row).await {
        record.state = PlanState::Interrupted;
    }
    Some(record)
}

async fn turn_running(state: &AppState, row: &conversation::Model) -> bool {
    let Some(connection_id) = crate::session::connection_for(&state.connection_manager, row).await
    else {
        return false;
    };
    match state.connection_manager.get_state(&connection_id).await {
        Some(live) => live.read().await.turn_in_flight,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "0b6f1c2e-0000-4000-8000-000000000001";

    fn steps(statuses: &[StepStatus]) -> Progress {
        Progress {
            steps: statuses
                .iter()
                .enumerate()
                .map(|(i, status)| ProgressStep {
                    title: format!("step {i}"),
                    status: *status,
                })
                .collect(),
        }
    }

    fn entries(statuses: &[&str]) -> Vec<PlanEntryInfo> {
        statuses
            .iter()
            .enumerate()
            .map(|(i, status)| PlanEntryInfo {
                content: format!("step {i}"),
                priority: "high".to_string(),
                status: status.to_string(),
            })
            .collect()
    }

    fn record() -> PlanRecord {
        PlanRecord::start(
            SESSION,
            AgentType::ClaudeCode,
            steps(&[StepStatus::Pending]),
        )
    }

    struct Fixed(Option<PlanTarget>);

    #[async_trait]
    impl PlanTargets for Fixed {
        async fn target(&self, _: &str) -> Option<PlanTarget> {
            self.0.clone()
        }
    }

    fn fixed(root: &Path) -> Fixed {
        Fixed(Some(PlanTarget {
            root: root.to_path_buf(),
            session_id: SESSION.to_string(),
            agent: AgentType::Codex,
        }))
    }

    fn event(connection_id: &str, payload: AcpEvent) -> EventEnvelope {
        EventEnvelope {
            seq: 0,
            connection_id: connection_id.to_string(),
            payload,
        }
    }

    fn plan_update(statuses: &[&str]) -> AcpEvent {
        AcpEvent::PlanUpdate {
            entries: entries(statuses),
        }
    }

    fn turn_complete(stop_reason: &str) -> AcpEvent {
        AcpEvent::TurnComplete {
            session_id: "agent-session".to_string(),
            stop_reason: stop_reason.to_string(),
            agent_type: "codex".to_string(),
        }
    }

    /// A stopped 3-step plan (1 done, 1 in progress, 1 to do) updated at `T`.
    const T: i64 = 1_700_000_000_000;

    fn stopped(state: PlanState) -> PlanRecord {
        let mut plan = PlanRecord::start(
            SESSION,
            AgentType::ClaudeCode,
            steps(&[
                StepStatus::Completed,
                StepStatus::InProgress,
                StepStatus::Pending,
            ]),
        );
        plan.state = state;
        plan.updated_at_ms = T;
        plan
    }

    fn turn_at(role: TurnRole, at_ms: i64) -> MessageTurn {
        MessageTurn {
            id: "turn-0".into(),
            role,
            blocks: Vec::new(),
            timestamp: chrono::DateTime::from_timestamp_millis(at_ms).unwrap(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
            agent_message_id: None,
        }
    }

    #[test]
    fn is_resumable_when_interrupted_with_steps_left() {
        let history = [turn_at(TurnRole::User, T - 30_000)];
        assert!(is_resumable(
            &stopped(PlanState::Interrupted),
            &history,
            false,
            false
        ));
    }

    #[test]
    fn is_resumable_when_cancelled_with_steps_left() {
        assert!(is_resumable(
            &stopped(PlanState::Cancelled),
            &[],
            false,
            false
        ));
    }

    #[test]
    fn not_resumable_when_completed_state() {
        assert!(!is_resumable(
            &stopped(PlanState::Completed),
            &[],
            false,
            false
        ));
    }

    #[test]
    fn not_resumable_when_all_steps_done() {
        let mut plan = stopped(PlanState::Interrupted);
        for step in &mut plan.steps {
            step.status = StepStatus::Completed;
        }
        assert!(!is_resumable(&plan, &[], false, false));
    }

    #[test]
    fn not_resumable_when_unreachable() {
        assert!(!is_resumable(
            &stopped(PlanState::Interrupted),
            &[],
            true,
            false
        ));
    }

    #[test]
    fn not_resumable_when_continued() {
        assert!(!is_resumable(
            &stopped(PlanState::Interrupted),
            &[],
            false,
            true
        ));
    }

    #[test]
    fn not_resumable_after_a_later_user_turn() {
        let history = [turn_at(TurnRole::User, T + 5_000)];
        assert!(!is_resumable(
            &stopped(PlanState::Interrupted),
            &history,
            false,
            false
        ));
    }

    #[test]
    fn is_resumable_ignores_turn_that_made_the_plan() {
        let history = [
            turn_at(TurnRole::User, T - 30_000),
            turn_at(TurnRole::Assistant, T + 1_000),
        ];
        assert!(is_resumable(
            &stopped(PlanState::Interrupted),
            &history,
            false,
            false
        ));
    }

    /// A real session row (and so a real workspace) to link to.
    async fn live_session() -> (crate::db::AppDatabase, String) {
        use crate::session::orchestration_tests::{settle, text, workspace, FakeLauncher};
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let mgr = crate::acp::manager::ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let wid = workspace(&db).await;
        let sid = crate::session::start(
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
        settle().await;
        (db, sid)
    }

    const PLAN: &str = "7d0f9a3c-0000-4000-8000-000000000001";

    fn forward(session_id: &str, plan_id: &str) -> ContinuedIn {
        ContinuedIn {
            session_id: session_id.to_string(),
            plan_id: plan_id.to_string(),
        }
    }

    #[tokio::test]
    async fn continued_in_round_trips() {
        let (db, new_id) = live_session().await;
        let dir = tempfile::tempdir().unwrap();
        save_continued_in(dir.path(), SESSION, &forward(&new_id, PLAN))
            .await
            .unwrap();
        assert_eq!(
            continued_in(&db.conn, dir.path(), SESSION, PLAN).await,
            Some(new_id)
        );
    }

    #[tokio::test]
    async fn continued_in_ignores_deleted_session() {
        let (db, _) = live_session().await;
        let dir = tempfile::tempdir().unwrap();
        let gone = "0b6f1c2e-0000-4000-8000-0000000000ff";
        save_continued_in(dir.path(), SESSION, &forward(gone, PLAN))
            .await
            .unwrap();
        assert_eq!(
            continued_in(&db.conn, dir.path(), SESSION, PLAN).await,
            None
        );
    }

    #[tokio::test]
    async fn continued_in_ignores_other_plan() {
        let (db, new_id) = live_session().await;
        let dir = tempfile::tempdir().unwrap();
        save_continued_in(dir.path(), SESSION, &forward(&new_id, PLAN))
            .await
            .unwrap();
        let other = "7d0f9a3c-0000-4000-8000-000000000002";
        assert_eq!(
            continued_in(&db.conn, dir.path(), SESSION, other).await,
            None
        );
    }

    #[tokio::test]
    async fn save_continued_in_refuses_unsafe_id() {
        let dir = tempfile::tempdir().unwrap();
        let link = forward("0b6f1c2e-0000-4000-8000-0000000000ff", PLAN);
        assert!(matches!(
            save_continued_in(dir.path(), "../x", &link).await,
            Err(PlanError::InvalidId)
        ));
    }

    #[tokio::test]
    async fn saves_under_the_session_folder_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let record = record();
        save(dir.path(), &record).await.unwrap();
        let path = dir
            .path()
            .join(".zwiim/tasks")
            .join(SESSION)
            .join(format!("{}.json", record.plan_id));
        assert!(path.exists());
        assert_eq!(latest(dir.path(), SESSION).await, Some(record));
        // Only the plan is left: the temporary file was moved into place.
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1);
    }

    #[tokio::test]
    async fn two_plans_never_share_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (record(), record());
        assert_ne!(a.plan_id, b.plan_id);
        save(dir.path(), &a).await.unwrap();
        save(dir.path(), &b).await.unwrap();
        assert_eq!(list(dir.path(), SESSION).await.len(), 2);
    }

    #[tokio::test]
    async fn a_final_plan_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut record = record();
        record.finish(PlanState::Completed);
        save(dir.path(), &record).await.unwrap();

        let mut changed = record.clone();
        changed.revise(steps(&[StepStatus::Completed]));
        assert!(matches!(
            save(dir.path(), &changed).await,
            Err(PlanError::Final)
        ));
        assert_eq!(latest(dir.path(), SESSION).await, Some(record.clone()));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = plan_path(dir.path(), SESSION, &record.plan_id).unwrap();
            let mode = std::fs::metadata(path).unwrap().permissions().mode();
            assert_eq!(mode & 0o222, 0, "final plan file is not writable");
        }
    }

    #[tokio::test]
    async fn rejects_ids_that_are_not_path_safe() {
        let dir = tempfile::tempdir().unwrap();
        let mut bad = record();
        bad.session_id = "../escape".to_string();
        assert!(matches!(
            save(dir.path(), &bad).await,
            Err(PlanError::InvalidId)
        ));
        let mut bad = record();
        bad.plan_id = "a/b".to_string();
        assert!(matches!(
            save(dir.path(), &bad).await,
            Err(PlanError::InvalidId)
        ));
    }

    #[tokio::test]
    async fn skips_files_that_are_not_whole_plans() {
        let dir = tempfile::tempdir().unwrap();
        let good = record();
        save(dir.path(), &good).await.unwrap();
        let folder = session_dir(dir.path(), SESSION).unwrap();
        std::fs::write(folder.join("broken.json"), b"{ not json").unwrap();
        // Renamed: its id no longer matches its file name.
        let mut other = record();
        other.created_at_ms += 10;
        std::fs::write(
            folder.join("renamed.json"),
            serde_json::to_vec(&other).unwrap(),
        )
        .unwrap();
        // Past the step limits.
        let mut blank = record();
        blank.created_at_ms += 20;
        blank.steps[0].title = "  ".to_string();
        std::fs::write(
            folder.join(format!("{}.json", blank.plan_id)),
            serde_json::to_vec(&blank).unwrap(),
        )
        .unwrap();
        assert_eq!(list(dir.path(), SESSION).await, vec![good]);
    }

    #[tokio::test]
    async fn keeps_the_newest_plans() {
        let dir = tempfile::tempdir().unwrap();
        let mut newest = None;
        for k in 0..KEEP_PLANS + 3 {
            let mut record = record();
            record.created_at_ms = k as i64;
            save(dir.path(), &record).await.unwrap();
            newest = Some(record);
        }
        prune(dir.path(), SESSION).await;
        let plans = list(dir.path(), SESSION).await;
        assert_eq!(plans.len(), KEEP_PLANS);
        assert_eq!(plans[0].created_at_ms, 3);
        assert_eq!(plans.last(), newest.as_ref());
    }

    #[tokio::test]
    async fn a_turn_writes_one_plan_and_ends_it() {
        let dir = tempfile::tempdir().unwrap();
        let targets = fixed(dir.path());
        let mut tracker = PlanTracker::default();

        tracker
            .observe(
                &targets,
                &event("c1", plan_update(&["in_progress", "pending"])),
            )
            .await;
        tracker
            .observe(
                &targets,
                &event("c1", plan_update(&["completed", "in_progress"])),
            )
            .await;
        let running = latest(dir.path(), SESSION).await.unwrap();
        assert_eq!(running.state, PlanState::Running);
        assert_eq!(running.revision, 2);
        assert_eq!(running.agent, AgentType::Codex);
        assert_eq!(running.steps[0].status, StepStatus::Completed);

        // Every step done mid-turn: still open until the turn ends.
        tracker
            .observe(
                &targets,
                &event("c1", plan_update(&["completed", "completed"])),
            )
            .await;
        assert_eq!(
            latest(dir.path(), SESSION).await.unwrap().state,
            PlanState::Running
        );

        tracker
            .observe(&targets, &event("c1", turn_complete("end_turn")))
            .await;
        let done = latest(dir.path(), SESSION).await.unwrap();
        assert_eq!(done.plan_id, running.plan_id);
        assert_eq!(done.state, PlanState::Completed);
        assert_eq!(list(dir.path(), SESSION).await.len(), 1);

        // The next turn's plan is a new one; the completed plan stays as it was.
        tracker
            .observe(&targets, &event("c1", plan_update(&["pending"])))
            .await;
        let plans = list(dir.path(), SESSION).await;
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0], done);
        assert_ne!(plans[1].plan_id, done.plan_id);
    }

    #[tokio::test]
    async fn turn_ends_map_to_plan_states() {
        for (payload, expected) in [
            (turn_complete("end_turn"), PlanState::Completed),
            (turn_complete("cancelled"), PlanState::Cancelled),
            (turn_complete("max_tokens"), PlanState::Interrupted),
            (
                AcpEvent::StatusChanged {
                    status: ConnectionStatus::Disconnected,
                },
                PlanState::Interrupted,
            ),
            (
                AcpEvent::UserMessage {
                    message_id: "m".to_string(),
                    blocks: Vec::new(),
                },
                PlanState::Interrupted,
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let targets = fixed(dir.path());
            let mut tracker = PlanTracker::default();
            tracker
                .observe(&targets, &event("c1", plan_update(&["pending"])))
                .await;
            tracker.observe(&targets, &event("c1", payload)).await;
            assert_eq!(latest(dir.path(), SESSION).await.unwrap().state, expected);
        }
    }

    #[tokio::test]
    async fn a_new_plan_interrupts_one_left_running() {
        let dir = tempfile::tempdir().unwrap();
        // Left running by a Host that stopped mid-turn.
        let mut stale = record();
        stale.created_at_ms -= 1_000;
        save(dir.path(), &stale).await.unwrap();

        let targets = fixed(dir.path());
        let mut tracker = PlanTracker::default();
        tracker
            .observe(&targets, &event("c1", plan_update(&["pending"])))
            .await;
        let plans = list(dir.path(), SESSION).await;
        assert_eq!(plans[0].plan_id, stale.plan_id);
        assert_eq!(plans[0].state, PlanState::Interrupted);
        assert_eq!(plans[1].state, PlanState::Running);
    }

    #[tokio::test]
    async fn skips_empty_plans_and_sessionless_connections() {
        let dir = tempfile::tempdir().unwrap();
        let mut tracker = PlanTracker::default();
        tracker
            .observe(&fixed(dir.path()), &event("c1", plan_update(&[])))
            .await;
        tracker
            .observe(&Fixed(None), &event("c2", plan_update(&["pending"])))
            .await;
        assert!(latest(dir.path(), SESSION).await.is_none());
        // A turn end with no plan open writes nothing.
        tracker
            .observe(&fixed(dir.path()), &event("c1", turn_complete("end_turn")))
            .await;
        assert!(latest(dir.path(), SESSION).await.is_none());
    }

    #[tokio::test]
    async fn a_link_names_the_continued_plan() {
        let dir = tempfile::tempdir().unwrap();
        assert!(link(dir.path(), SESSION).await.is_none());
        let continued = PlanLink {
            session_id: "0b6f1c2e-0000-4000-8000-000000000002".to_string(),
            plan_id: "p1".to_string(),
            agent: Some(AgentType::Codex),
            completed: 1,
            total: 3,
            title: Some("Fix the leak".to_string()),
        };
        save_link(dir.path(), SESSION, &continued).await.unwrap();
        assert_eq!(link(dir.path(), SESSION).await, Some(continued));
        // The link is not read as a plan.
        assert!(list(dir.path(), SESSION).await.is_empty());

        let bad = PlanLink {
            session_id: "../x".to_string(),
            plan_id: "p1".to_string(),
            agent: None,
            completed: 0,
            total: 0,
            title: None,
        };
        assert!(matches!(
            save_link(dir.path(), SESSION, &bad).await,
            Err(PlanError::InvalidId)
        ));
    }

    #[test]
    fn unfinished_while_any_step_is_not_done() {
        let mut record = record();
        assert!(record.is_unfinished());
        record.steps[0].status = StepStatus::InProgress;
        assert!(record.is_unfinished());
        record.steps[0].status = StepStatus::Completed;
        assert!(!record.is_unfinished());
    }

    #[test]
    fn record_file_shape() {
        let mut record = record();
        record.plan_id = "p1".to_string();
        record.created_at_ms = 1;
        record.updated_at_ms = 2;
        assert_eq!(
            serde_json::to_value(&record).unwrap(),
            serde_json::json!({
                "v": 1,
                "plan_id": "p1",
                "session_id": SESSION,
                "agent": "claude_code",
                "revision": 1,
                "state": "running",
                "steps": [{ "title": "step 0", "status": "pending" }],
                "created_at_ms": 1,
                "updated_at_ms": 2,
            })
        );
    }
}
