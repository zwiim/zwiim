//! Host PR watcher — follows the pull request of each workspace's branch so
//! the Host feed can publish it (`workspace.pr_changed`) and clients never
//! ask GitHub themselves.
//!
//! The feed tells the watcher which workspaces are on a named branch
//! ([`PrCmd::Targets`]) and when something may have moved a pull request
//! ([`PrCmd::Check`]: a push seen by the workspace watcher, a finished agent
//! turn). The watcher also follows every change the Git Harness makes (push,
//! PR create, PR merge), whoever asked for it. Each lookup is one `gh pr
//! view` through the Git Harness, run off the feed's task; results go back to
//! the feed, which publishes only real changes.
//!
//! Polling happens only while a pull request is open: [`PrPolicy::pending`]
//! while checks run or GitHub is still working out mergeability,
//! [`PrPolicy::settled`] after. A merged or closed pull request, a branch
//! without one, and a `gh` that is missing or logged out are not polled; the
//! next trigger looks again. Network failures keep the last state and back
//! off up to [`PrPolicy::max_backoff`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use sea_orm::DatabaseConnection;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::git_harness::{
    self, GitCaller, GitError, GitErrorCode, GitHost, GitOpKind, MergeStatus, PrState,
};

/// Lookups the watcher runs at once.
const MAX_LOOKUPS: usize = 2;

/// How often an open pull request is re-read.
#[derive(Debug, Clone, Copy)]
pub struct PrPolicy {
    /// While checks run or mergeability is unknown.
    pub pending: Duration,
    /// Once every check has finished.
    pub settled: Duration,
    /// The longest wait after repeated network failures.
    pub max_backoff: Duration,
}

impl Default for PrPolicy {
    fn default() -> Self {
        Self {
            pending: Duration::from_secs(15),
            settled: Duration::from_secs(60),
            max_backoff: Duration::from_secs(300),
        }
    }
}

impl PrPolicy {
    /// When to look again after a lookup that ended in `outcome`, having
    /// been polling every `polling` (`None`: not polling). `None`: wait for
    /// a trigger.
    pub(crate) fn next_poll(
        &self,
        outcome: Result<Option<&PrState>, GitErrorCode>,
        polling: Option<Duration>,
    ) -> Option<Duration> {
        match outcome {
            Ok(Some(pr)) if pr.state.is_final() => None,
            Ok(Some(pr)) if pr.checks.pending > 0 || pr.merge.status == MergeStatus::Unknown => {
                Some(self.pending)
            }
            Ok(Some(_)) => Some(self.settled),
            Ok(None) => None,
            Err(GitErrorCode::GhUnavailable | GitErrorCode::GhUnauthenticated) => None,
            Err(GitErrorCode::Network | GitErrorCode::Timeout) => {
                Some(self.backoff(polling.unwrap_or(self.pending)))
            }
            // Anything else: keep following a pull request already followed.
            Err(_) => polling.map(|p| self.backoff(p)),
        }
    }

    fn backoff(&self, interval: Duration) -> Duration {
        (interval * 2).min(self.max_backoff)
    }
}

/// What the Host knows about a workspace's pull request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PrEntry {
    /// The branch's pull request, open or settled; `None` when it has none.
    pub pr: Option<PrState>,
    /// Why the Host cannot see pull requests (`gh` missing or logged out).
    pub error: Option<PrError>,
}

impl PrEntry {
    pub fn is_empty(&self) -> bool {
        self.pr.is_none() && self.error.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrError {
    pub code: &'static str,
    pub message: String,
}

/// A workspace on a named branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrTarget {
    pub workspace_id: String,
    pub path: String,
    pub branch: String,
}

pub(crate) enum PrCmd {
    /// Every workspace that may have a pull request, replacing the last list.
    Targets(Vec<PrTarget>),
    /// Something may have changed the workspace's pull request.
    Check(String),
    /// Look now and answer with what GitHub says.
    Refresh(String, oneshot::Sender<Result<PrEntry, GitError>>),
}

/// A lookup's answer for the feed to publish.
#[derive(Debug)]
pub(crate) struct PrResult {
    pub workspace_id: String,
    /// The branch it was looked up for; stale once the workspace moved on.
    pub branch: String,
    pub entry: PrEntry,
}

/// How the watcher reaches GitHub.
#[derive(Debug, Clone, Default)]
pub struct PrWatchConfig {
    /// The `gh` to run; `None` finds the user's.
    pub gh: Option<PathBuf>,
    pub policy: PrPolicy,
}

struct Env {
    db: DatabaseConnection,
    data_dir: PathBuf,
    gh: Option<PathBuf>,
}

/// One lookup's answer, moved once from its task to the watcher.
#[allow(clippy::large_enum_variant)]
enum Lookup {
    /// The branch is the repository's default branch.
    DefaultBranch,
    /// The repository has no remote, so no pull request either. Looked at
    /// again on the next trigger: a remote may be added.
    NoRemote,
    Done(Result<Option<PrState>, GitError>),
}

type Waiter = oneshot::Sender<Result<PrEntry, GitError>>;

struct Tracked {
    target: PrTarget,
    /// Tells this target's lookups from those of whatever it replaced.
    generation: u64,
    /// `Some(false)` once the branch turned out to be the default branch.
    eligible: Option<bool>,
    due: Option<Instant>,
    /// The poll interval in force; `None` when not polling.
    polling: Option<Duration>,
    running: bool,
    /// A trigger arrived while a lookup ran.
    again: bool,
    waiters: Vec<Waiter>,
}

impl Tracked {
    fn new(target: PrTarget, generation: u64) -> Self {
        Self {
            target,
            generation,
            eligible: None,
            due: Some(Instant::now()),
            polling: None,
            running: false,
            again: false,
            waiters: Vec::new(),
        }
    }

    fn trigger(&mut self) {
        if self.running {
            self.again = true;
        } else if self.eligible != Some(false) {
            self.due = Some(Instant::now());
        }
    }
}

/// Start the watcher; results go to `results`.
pub(crate) fn spawn(
    db: DatabaseConnection,
    data_dir: PathBuf,
    config: PrWatchConfig,
    results: mpsc::UnboundedSender<PrResult>,
) -> mpsc::UnboundedSender<PrCmd> {
    let (tx, rx) = mpsc::unbounded_channel();
    let watcher = Watcher {
        env: Arc::new(Env {
            db,
            data_dir,
            gh: config.gh,
        }),
        policy: config.policy,
        tracked: HashMap::new(),
        generations: 0,
        results,
    };
    tokio::spawn(watcher.run(rx, git_harness::subscribe_changes()));
    tx
}

struct Watcher {
    env: Arc<Env>,
    policy: PrPolicy,
    tracked: HashMap<String, Tracked>,
    generations: u64,
    results: mpsc::UnboundedSender<PrResult>,
}

/// A finished lookup: workspace, generation, branch, outcome.
type Done = (String, u64, String, Lookup);

impl Watcher {
    async fn run(
        mut self,
        mut cmds: mpsc::UnboundedReceiver<PrCmd>,
        mut changes: tokio::sync::broadcast::Receiver<git_harness::GitChange>,
    ) {
        let (done_tx, mut done_rx) = mpsc::unbounded_channel::<Done>();
        loop {
            let next = self
                .tracked
                .values()
                .filter(|t| !t.running)
                .filter_map(|t| t.due)
                .min();
            tokio::select! {
                cmd = cmds.recv() => match cmd {
                    Some(cmd) => self.command(cmd),
                    None => return,
                },
                change = changes.recv() => match change {
                    Ok(change) => {
                        if matches!(change.kind, GitOpKind::Push | GitOpKind::PrCreate | GitOpKind::PrMerge) {
                            self.trigger(&change.workspace_id);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        self.tracked.values_mut().for_each(Tracked::trigger);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        changes = git_harness::subscribe_changes();
                    }
                },
                Some((id, generation, branch, lookup)) = done_rx.recv() => {
                    self.finished(id, generation, branch, lookup)
                }
                _ = sleep_until(next), if next.is_some() => {}
            }
            self.start_due(&done_tx);
        }
    }

    fn command(&mut self, cmd: PrCmd) {
        match cmd {
            PrCmd::Targets(targets) => {
                let mut next: HashMap<String, Tracked> = HashMap::new();
                for target in targets {
                    let kept = self
                        .tracked
                        .remove(&target.workspace_id)
                        .filter(|t| t.target == target);
                    let tracked = match kept {
                        Some(kept) => kept,
                        None => {
                            self.generations += 1;
                            Tracked::new(target.clone(), self.generations)
                        }
                    };
                    next.insert(target.workspace_id, tracked);
                }
                // Workspaces that left: whoever waits gets "no pull request".
                for (_, gone) in self.tracked.drain() {
                    for w in gone.waiters {
                        let _ = w.send(Ok(PrEntry::default()));
                    }
                }
                self.tracked = next;
            }
            PrCmd::Check(id) => self.trigger(&id),
            PrCmd::Refresh(id, reply) => match self.tracked.get_mut(&id) {
                Some(t) => {
                    t.eligible = None;
                    t.waiters.push(reply);
                    t.trigger();
                }
                None => {
                    let _ = reply.send(Ok(PrEntry::default()));
                }
            },
        }
    }

    fn trigger(&mut self, id: &str) {
        if let Some(t) = self.tracked.get_mut(id) {
            t.trigger();
        }
    }

    fn start_due(&mut self, done: &mpsc::UnboundedSender<Done>) {
        let now = Instant::now();
        let mut running = self.tracked.values().filter(|t| t.running).count();
        let mut due: Vec<(Instant, String)> = self
            .tracked
            .iter()
            .filter(|(_, t)| !t.running && t.due.is_some_and(|d| d <= now))
            .map(|(id, t)| (t.due.unwrap(), id.clone()))
            .collect();
        due.sort();
        for (_, id) in due {
            if running >= MAX_LOOKUPS {
                break;
            }
            let Some(t) = self.tracked.get_mut(&id) else {
                continue;
            };
            t.running = true;
            t.due = None;
            running += 1;
            let env = self.env.clone();
            let target = t.target.clone();
            let check_default = t.eligible.is_none();
            let generation = t.generation;
            let done = done.clone();
            tokio::spawn(async move {
                let lookup = lookup(&env, &target, check_default).await;
                let _ = done.send((target.workspace_id, generation, target.branch, lookup));
            });
        }
    }

    fn finished(&mut self, id: String, generation: u64, branch: String, lookup: Lookup) {
        // A workspace that moved on has its new branch looked up anew.
        let Some(t) = self
            .tracked
            .get_mut(&id)
            .filter(|t| t.generation == generation)
        else {
            return;
        };
        t.running = false;
        let waiters = std::mem::take(&mut t.waiters);
        let answer = match lookup {
            Lookup::DefaultBranch => {
                t.eligible = Some(false);
                t.polling = None;
                t.again = false;
                Ok(PrEntry::default())
            }
            Lookup::NoRemote => {
                t.polling = None;
                Ok(PrEntry::default())
            }
            Lookup::Done(result) => {
                t.eligible = Some(true);
                let outcome = result.as_ref().map(Option::as_ref).map_err(|e| e.code);
                t.polling = self.policy.next_poll(outcome, t.polling);
                t.due = t.polling.map(|p| Instant::now() + p);
                let entry = match &result {
                    Ok(pr) => Some(PrEntry {
                        pr: pr.clone(),
                        error: None,
                    }),
                    Err(e)
                        if matches!(
                            e.code,
                            GitErrorCode::GhUnavailable | GitErrorCode::GhUnauthenticated
                        ) =>
                    {
                        Some(PrEntry {
                            pr: None,
                            error: Some(PrError {
                                code: e.code.as_str(),
                                message: e.message.clone(),
                            }),
                        })
                    }
                    // Keep showing what was last known.
                    Err(_) => None,
                };
                if let Some(entry) = &entry {
                    let _ = self.results.send(PrResult {
                        workspace_id: id.clone(),
                        branch,
                        entry: entry.clone(),
                    });
                }
                match (entry, result) {
                    (Some(entry), _) => Ok(entry),
                    (None, Err(e)) => Err(e),
                    (None, Ok(_)) => unreachable!("a successful lookup always has an entry"),
                }
            }
        };
        if std::mem::take(&mut t.again) && t.eligible != Some(false) {
            t.due = Some(Instant::now());
        }
        for w in waiters {
            let _ = w.send(answer.clone());
        }
    }
}

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

async fn lookup(env: &Env, target: &PrTarget, check_default: bool) -> Lookup {
    let remotes = crate::process::tokio_command("git")
        .arg("remote")
        .current_dir(&target.path)
        .output()
        .await;
    if !remotes.is_ok_and(|o| o.status.success() && !o.stdout.trim_ascii().is_empty()) {
        return Lookup::NoRemote;
    }
    if check_default {
        let default = crate::workspace::origin_default_branch(&target.path).await;
        let default = default
            .as_deref()
            .map(|b| b.strip_prefix("origin/").unwrap_or(b));
        if default == Some(target.branch.as_str()) {
            return Lookup::DefaultBranch;
        }
    }
    let host = GitHost {
        gh: env.gh.as_deref(),
        ..GitHost::new(&env.db, &env.data_dir)
    };
    let caller = match GitCaller::new(Some(target.workspace_id.clone()), None) {
        Ok(caller) => caller,
        Err(e) => return Lookup::Done(Err(e)),
    };
    Lookup::Done(git_harness::pr_state(&host, &caller, &target.branch).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_harness::{PrChecksSummary, PrLifecycle, PrMergeInfo};

    fn pr(state: PrLifecycle, pending: u32, status: MergeStatus) -> PrState {
        PrState {
            number: 1,
            url: String::new(),
            title: String::new(),
            state,
            base: "main".into(),
            head: "feat".into(),
            head_sha: "h".into(),
            merged_at: None,
            checks: PrChecksSummary {
                pending,
                ..Default::default()
            },
            merge: PrMergeInfo {
                allowed: false,
                status,
                reason: None,
                method: None,
                methods: vec![],
            },
        }
    }

    #[test]
    fn open_pull_requests_are_polled_until_settled() {
        let p = PrPolicy::default();
        let s = Duration::from_secs;
        let running = pr(PrLifecycle::Open, 2, MergeStatus::Blocked);
        let unknown = pr(PrLifecycle::Open, 0, MergeStatus::Unknown);
        let settled = pr(PrLifecycle::Open, 0, MergeStatus::Clean);
        let draft = pr(PrLifecycle::Draft, 0, MergeStatus::Draft);
        assert_eq!(p.next_poll(Ok(Some(&running)), None), Some(s(15)));
        assert_eq!(p.next_poll(Ok(Some(&unknown)), Some(s(60))), Some(s(15)));
        assert_eq!(p.next_poll(Ok(Some(&settled)), Some(s(15))), Some(s(60)));
        assert_eq!(p.next_poll(Ok(Some(&draft)), None), Some(s(60)));
        // Settled for good, or nothing to follow: wait for a trigger.
        for done in [PrLifecycle::Merged, PrLifecycle::Closed] {
            assert_eq!(
                p.next_poll(Ok(Some(&pr(done, 1, MergeStatus::Unknown))), Some(s(15))),
                None
            );
        }
        assert_eq!(p.next_poll(Ok(None), Some(s(15))), None);
        for code in [GitErrorCode::GhUnavailable, GitErrorCode::GhUnauthenticated] {
            assert_eq!(p.next_poll(Err(code), Some(s(15))), None);
        }
    }

    #[test]
    fn network_failures_back_off_to_five_minutes() {
        let p = PrPolicy::default();
        let mut polling = Some(Duration::from_secs(15));
        let mut seen = vec![];
        for _ in 0..6 {
            polling = p.next_poll(Err(GitErrorCode::Network), polling);
            seen.push(polling.unwrap().as_secs());
        }
        assert_eq!(seen, [30, 60, 120, 240, 300, 300]);
        assert_eq!(
            p.next_poll(Err(GitErrorCode::Timeout), None),
            Some(Duration::from_secs(30))
        );
        // Success resets the interval.
        let settled = pr(PrLifecycle::Open, 0, MergeStatus::Clean);
        assert_eq!(
            p.next_poll(Ok(Some(&settled)), polling),
            Some(Duration::from_secs(60))
        );
        // Other failures keep following an open pull request, and start
        // nothing new.
        assert_eq!(
            p.next_poll(Err(GitErrorCode::Failed), Some(Duration::from_secs(60))),
            Some(Duration::from_secs(120))
        );
        assert_eq!(p.next_poll(Err(GitErrorCode::Failed), None), None);
    }
}
