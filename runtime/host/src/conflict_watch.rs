//! Host conflict watcher — keeps each workspace's merge state against its
//! base current so the Host feed can publish it
//! (`workspace.conflicts_changed`) before anyone tries to merge.
//!
//! The feed tells the watcher which workspaces are Git repositories
//! ([`ConflictCmd::Targets`]) and when their merge state may have moved: a
//! batch of Git metadata changes ([`ConflictCmd::Check`]) or a finished
//! agent turn ([`ConflictCmd::Fetch`]), which also brings the base up to
//! date first. Nothing is polled. Each check is a would-be merge through the
//! Git Harness, run off the feed's task; at most [`MAX_CHECKS`] run at once,
//! and triggers that arrive while a workspace's check runs fold into one
//! more. Results go back to the feed, which publishes only real changes.
//!
//! Base fetches are rate-limited per root workspace and base to one every
//! [`ConflictPolicy::fetch_interval`]. A fetch that fails (offline, remote
//! gone) is ignored: the check runs on the local refs and the last state
//! stands until a check says otherwise.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;

use crate::git_harness::{self, ConflictCheck, GitError, GitHost};

/// Checks the watcher runs at once.
const MAX_CHECKS: usize = 2;

#[derive(Debug, Clone, Copy)]
pub struct ConflictPolicy {
    /// The least time between two base fetches for one root and base.
    pub fetch_interval: Duration,
}

impl Default for ConflictPolicy {
    fn default() -> Self {
        Self {
            fetch_interval: Duration::from_secs(120),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ConflictWatchConfig {
    pub policy: ConflictPolicy,
}

/// A workspace whose merge state is followed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConflictTarget {
    pub workspace_id: String,
    pub path: String,
    /// The root workspace; its worktrees share its remote-tracking refs.
    pub root_id: String,
    pub worktree: bool,
    pub base_branch: Option<String>,
}

pub(crate) enum ConflictCmd {
    /// Every workspace to follow, replacing the last list.
    Targets(Vec<ConflictTarget>),
    /// The workspace's Git metadata changed.
    Check(String),
    /// An agent turn ended in the workspace: fetch its base, then check.
    Fetch(String),
}

/// A check's answer for the feed to publish.
#[derive(Debug)]
pub(crate) struct ConflictResult {
    pub workspace_id: String,
    /// `None`: there is nothing to compare with.
    pub check: Option<ConflictCheck>,
}

/// Start the watcher; results go to `results`.
pub(crate) fn spawn(
    db: DatabaseConnection,
    data_dir: PathBuf,
    config: ConflictWatchConfig,
    results: mpsc::UnboundedSender<ConflictResult>,
) -> mpsc::UnboundedSender<ConflictCmd> {
    let (tx, rx) = mpsc::unbounded_channel();
    let watcher = Watcher {
        env: Arc::new(Env {
            db,
            data_dir,
            fetch_interval: config.policy.fetch_interval,
            last_fetch: Mutex::new(HashMap::new()),
        }),
        tracked: HashMap::new(),
        generations: 0,
        results,
    };
    tokio::spawn(watcher.run(rx));
    tx
}

/// Whether a watcher batch (`None`: it overflowed) may have moved a merge
/// state: any Git metadata change except the objects, reflogs and checkpoint
/// refs a check or checkpoint writes itself.
pub(crate) fn needs_conflict_check(paths: Option<&Vec<String>>) -> bool {
    let Some(paths) = paths else {
        return true;
    };
    paths.iter().any(|p| {
        (p == ".git" || p.starts_with(".git/"))
            && ![".git/objects/", ".git/logs/", ".git/refs/zwiim/"]
                .iter()
                .any(|own| p.starts_with(own))
    })
}

struct Env {
    db: DatabaseConnection,
    data_dir: PathBuf,
    fetch_interval: Duration,
    /// The last fetch attempt per root workspace and base.
    last_fetch: Mutex<HashMap<(String, String), Instant>>,
}

impl Env {
    /// Whether `root_id` may fetch `base` now; a yes counts as the attempt,
    /// whether or not the fetch then succeeds.
    fn may_fetch(&self, root_id: &str, base: &str) -> bool {
        let mut last = self.last_fetch.lock().unwrap_or_else(|e| e.into_inner());
        let key = (root_id.to_string(), base.to_string());
        let now = Instant::now();
        if last
            .get(&key)
            .is_some_and(|at| now.duration_since(*at) < self.fetch_interval)
        {
            return false;
        }
        last.insert(key, now);
        true
    }
}

struct Tracked {
    target: ConflictTarget,
    generation: u64,
    /// A check is owed.
    queued: bool,
    /// The owed check fetches the base first.
    fetch: bool,
    running: bool,
}

struct Watcher {
    env: Arc<Env>,
    tracked: HashMap<String, Tracked>,
    generations: u64,
    results: mpsc::UnboundedSender<ConflictResult>,
}

/// A finished check: workspace, generation, outcome.
type Done = (String, u64, Result<Option<ConflictCheck>, GitError>);

impl Watcher {
    async fn run(mut self, mut cmds: mpsc::UnboundedReceiver<ConflictCmd>) {
        let (done_tx, mut done_rx) = mpsc::unbounded_channel::<Done>();
        loop {
            tokio::select! {
                cmd = cmds.recv() => match cmd {
                    Some(cmd) => self.command(cmd),
                    None => return,
                },
                Some((id, generation, outcome)) = done_rx.recv() => {
                    self.finished(id, generation, outcome)
                }
            }
            self.start_due(&done_tx);
        }
    }

    fn command(&mut self, cmd: ConflictCmd) {
        match cmd {
            ConflictCmd::Targets(targets) => {
                let mut next = HashMap::new();
                for target in targets {
                    let kept = self
                        .tracked
                        .remove(&target.workspace_id)
                        .filter(|t| t.target == target);
                    let tracked = kept.unwrap_or_else(|| {
                        self.generations += 1;
                        Tracked {
                            target: target.clone(),
                            generation: self.generations,
                            queued: true,
                            fetch: false,
                            running: false,
                        }
                    });
                    next.insert(target.workspace_id, tracked);
                }
                self.tracked = next;
            }
            ConflictCmd::Check(id) => self.trigger(&id, false),
            ConflictCmd::Fetch(id) => self.trigger(&id, true),
        }
    }

    fn trigger(&mut self, id: &str, fetch: bool) {
        if let Some(t) = self.tracked.get_mut(id) {
            t.queued = true;
            t.fetch |= fetch;
        }
    }

    fn start_due(&mut self, done: &mpsc::UnboundedSender<Done>) {
        let mut running = self.tracked.values().filter(|t| t.running).count();
        for t in self.tracked.values_mut() {
            if running >= MAX_CHECKS {
                break;
            }
            if t.running || !t.queued {
                continue;
            }
            t.running = true;
            t.queued = false;
            running += 1;
            let fetch = std::mem::take(&mut t.fetch);
            let env = self.env.clone();
            let target = t.target.clone();
            let generation = t.generation;
            let done = done.clone();
            tokio::spawn(async move {
                let outcome = check(&env, &target, fetch).await;
                let _ = done.send((target.workspace_id, generation, outcome));
            });
        }
    }

    fn finished(
        &mut self,
        id: String,
        generation: u64,
        outcome: Result<Option<ConflictCheck>, GitError>,
    ) {
        // A workspace that moved on is checked anew.
        let Some(t) = self
            .tracked
            .get_mut(&id)
            .filter(|t| t.generation == generation)
        else {
            return;
        };
        t.running = false;
        match outcome {
            Ok(check) => {
                let _ = self.results.send(ConflictResult {
                    workspace_id: id,
                    check,
                });
            }
            // The last state stands until a check says otherwise.
            Err(e) => tracing::debug!("[ConflictWatch] check of {id} failed: {}", e.message),
        }
    }
}

/// One check: the workspace's base, fetched first when `fetch` and the rate
/// limit allows, then a would-be merge on the local refs.
async fn check(
    env: &Env,
    target: &ConflictTarget,
    fetch: bool,
) -> Result<Option<ConflictCheck>, GitError> {
    let root = Path::new(&target.path);
    let Some(base) =
        git_harness::conflict_base(root, target.worktree, target.base_branch.as_deref()).await?
    else {
        return Ok(None);
    };
    if fetch && env.may_fetch(&target.root_id, &base) {
        let host = GitHost::new(&env.db, &env.data_dir);
        if let Err(e) = git_harness::fetch_base(&host, root, &base).await {
            tracing::debug!("[ConflictWatch] fetch of {base} failed: {}", e.message);
        }
    }
    git_harness::check_conflicts(root, &base).await
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

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

    struct Rig {
        _dir: tempfile::TempDir,
        repo: PathBuf,
        /// Lines appended by every fetch that reaches the remote.
        fetches: PathBuf,
        cmd: mpsc::UnboundedSender<ConflictCmd>,
        results: mpsc::UnboundedReceiver<ConflictResult>,
    }

    /// A root repo tracking `origin/main`, whose fetches each append a line
    /// to `fetches` after `delay` seconds, and a watcher following it.
    async fn rig(policy: ConflictPolicy, delay: u32) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        let origin = dir.path().join("origin.git");
        git(dir.path(), &["init", "-q", "--bare", origin.to_str().unwrap()]);
        git(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
        git(&repo, &["push", "-q", "-u", "origin", "main"]);
        let fetches = dir.path().join("fetches");
        let pack = dir.path().join("upload-pack");
        std::fs::write(
            &pack,
            format!(
                "#!/bin/sh\nsleep {delay}\necho x >> '{}'\nexec git upload-pack \"$@\"\n",
                fetches.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&pack, std::fs::Permissions::from_mode(0o755)).unwrap();
        git(
            &repo,
            &["config", "remote.origin.uploadpack", pack.to_str().unwrap()],
        );
        let data_dir = dir.path().join("data");
        std::fs::create_dir(&data_dir).unwrap();
        let db = crate::db::init_database(&data_dir, "test").await.unwrap();
        let (results_tx, results) = mpsc::unbounded_channel();
        let cmd = spawn(
            db.conn,
            data_dir,
            ConflictWatchConfig { policy },
            results_tx,
        );
        let repo = repo.canonicalize().unwrap();
        cmd.send(ConflictCmd::Targets(vec![ConflictTarget {
            workspace_id: "ws-root".into(),
            path: repo.to_string_lossy().into_owned(),
            root_id: "ws-root".into(),
            worktree: false,
            base_branch: None,
        }]))
        .ok()
        .unwrap();
        Rig {
            _dir: dir,
            repo,
            fetches,
            cmd,
            results,
        }
    }

    impl Rig {
        async fn result(&mut self) -> ConflictResult {
            tokio::time::timeout(Duration::from_secs(15), self.results.recv())
                .await
                .expect("a check result")
                .expect("watcher alive")
        }

        async fn quiet(&mut self, ms: u64) {
            if let Ok(r) =
                tokio::time::timeout(Duration::from_millis(ms), self.results.recv()).await
            {
                panic!("unexpected result: {r:?}");
            }
        }

        fn fetch_count(&self) -> usize {
            std::fs::read_to_string(&self.fetches)
                .map(|s| s.lines().count())
                .unwrap_or(0)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn checks_coalesce_per_workspace() {
        let mut r = rig(ConflictPolicy::default(), 0).await;
        // The first check runs from the target; three triggers while it runs
        // fold into one more.
        for _ in 0..3 {
            r.cmd.send(ConflictCmd::Check("ws-root".into())).ok().unwrap();
        }
        let first = r.result().await;
        assert_eq!(first.workspace_id, "ws-root");
        let check = first.check.expect("a base to compare with");
        assert_eq!(check.base, "origin/main");
        r.result().await;
        r.quiet(1000).await;
        // Unknown workspaces are ignored.
        r.cmd.send(ConflictCmd::Check("ws-gone".into())).ok().unwrap();
        r.quiet(300).await;
        let _ = &r.repo;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn base_fetch_rate_limited() {
        let mut r = rig(ConflictPolicy::default(), 0).await;
        r.result().await;
        assert_eq!(r.fetch_count(), 0, "a target's first check does not fetch");
        r.cmd.send(ConflictCmd::Fetch("ws-root".into())).ok().unwrap();
        r.result().await;
        r.cmd.send(ConflictCmd::Fetch("ws-root".into())).ok().unwrap();
        r.result().await;
        assert_eq!(r.fetch_count(), 1, "two turn ends within the interval fetch once");

        let mut r = rig(
            ConflictPolicy {
                fetch_interval: Duration::ZERO,
            },
            0,
        )
        .await;
        r.result().await;
        for _ in 0..2 {
            r.cmd.send(ConflictCmd::Fetch("ws-root".into())).ok().unwrap();
            r.result().await;
        }
        assert_eq!(r.fetch_count(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failed_fetch_still_checks_local_refs() {
        let mut r = rig(ConflictPolicy::default(), 0).await;
        r.result().await;
        git(&r.repo, &["remote", "set-url", "origin", "/nonexistent/origin.git"]);
        r.cmd.send(ConflictCmd::Fetch("ws-root".into())).ok().unwrap();
        let after = r.result().await.check.expect("checked on local refs");
        assert_eq!(after.base, "origin/main");
    }

    #[test]
    fn own_object_writes_do_not_recheck() {
        let batch = |paths: &[&str]| paths.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        assert!(!needs_conflict_check(Some(&batch(&[
            ".git/objects/ab/cdef",
            ".git/logs/HEAD",
            ".git/refs/zwiim/checkpoints/x",
        ]))));
        // Working files alone move neither HEAD, the base nor the index.
        assert!(!needs_conflict_check(Some(&batch(&["src/a.rs"]))));
        assert!(!needs_conflict_check(Some(&batch(&[]))));
        assert!(needs_conflict_check(Some(&batch(&[".git/objects/ab/cdef", ".git/index"]))));
        assert!(needs_conflict_check(Some(&batch(&[".git/refs/remotes/origin/main"]))));
        assert!(needs_conflict_check(Some(&batch(&[".git"]))));
        // An overflowed batch may hold anything.
        assert!(needs_conflict_check(None));
    }
}
