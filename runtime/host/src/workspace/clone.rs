//! Opening a GitHub project: the Host clones the repository under
//! `repos_dir/<owner>/<name>`, adds it as a root, and opens a worktree.
//! Clones run as in-memory jobs (they do not survive a Host restart); each is
//! staged in `repos_dir/.cloning/<clone_id>` and renamed into place, so a
//! half-cloned folder is never at a real path.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sea_orm::DatabaseConnection;
use serde::Serialize;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::repo_init::{self, Identity};
use super::WorkspaceKind;
use crate::db::AppDatabase;
use crate::git_harness::{self, ClonePhase, CloneRun, GitError, GitErrorCode, GithubRepoRow};
use crate::protocol::error::ZwiimError;
use crate::web::event_bridge::EventEmitter;

/// Clones in progress, under `repos_dir`. Never a GitHub login (logins cannot
/// start with `.`).
pub(crate) const STAGING: &str = ".cloning";
/// Most `-N` suffixes tried for a destination.
const MAX_SUFFIX: u32 = 100;
const HELPER_KEY: &str = "credential.https://github.com.helper";

/// `owner/name`, lowercase, for a GitHub remote URL in any of its spellings;
/// `None` for anything else.
pub(crate) fn github_slug(url: &str) -> Option<String> {
    let url = url.trim();
    let path = if let Some(rest) = url.strip_prefix("git@github.com:") {
        rest
    } else if let Some(rest) = url.strip_prefix("ssh://git@github.com/") {
        rest
    } else if let Some(rest) = url.strip_prefix("https://") {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        if !host.eq_ignore_ascii_case("github.com") {
            return None;
        }
        path
    } else {
        return None;
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    Some(format!("{owner}/{name}").to_lowercase())
}

/// The GitHub repository `origin` points at, if any.
pub(crate) async fn origin_slug(path: &Path) -> Option<String> {
    let url = repo_init::git(path, &[], &["config", "--get", "remote.origin.url"])
        .await
        .ok()?;
    github_slug(&url)
}

/// Each GitHub repository a live root workspace has as `origin`, to that
/// root's id. Read fresh: a user may change a remote at any time.
pub(crate) async fn root_origins(conn: &DatabaseConnection) -> HashMap<String, String> {
    let roots: Vec<_> = match super::list(conn).await {
        Ok(list) => list
            .into_iter()
            .filter(|w| matches!(w.kind, WorkspaceKind::Root) && !w.removed && w.is_git_repo)
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e.message, "[zwiim] clone: could not list workspaces");
            return HashMap::new();
        }
    };
    let slugs =
        futures::future::join_all(roots.iter().map(|w| origin_slug(Path::new(&w.path)))).await;
    let mut origins = HashMap::new();
    for (root, slug) in roots.iter().zip(slugs) {
        if let Some(slug) = slug {
            origins
                .entry(slug)
                .or_insert_with(|| root.workspace_id.clone());
        }
    }
    origins
}

/// The `n`th destination for `name` (from 1): `name`, `name-2`, `name-3`, ….
pub(crate) fn candidate(parent: &Path, name: &str, n: u32) -> PathBuf {
    if n == 1 {
        parent.join(name)
    } else {
        parent.join(format!("{name}-{n}"))
    }
}

/// A destination already cloned from `slug` (cloned before, then removed
/// from Zwiim): registering it beats cloning again.
pub(crate) async fn existing_clone(parent: &Path, name: &str, slug: &str) -> Option<PathBuf> {
    for n in 1..=MAX_SUFFIX {
        let path = candidate(parent, name, n);
        if path.join(".git").exists() && origin_slug(&path).await.as_deref() == Some(slug) {
            return Some(path);
        }
    }
    None
}

/// Rename the finished clone to the first free destination.
pub(crate) fn move_into_place(
    staging: &Path,
    parent: &Path,
    name: &str,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(parent)?;
    for n in 1..=MAX_SUFFIX {
        let dest = candidate(parent, name, n);
        if dest.symlink_metadata().is_ok() {
            continue;
        }
        match std::fs::rename(staging, &dest) {
            Ok(()) => return Ok(dest),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no free destination",
    ))
}

/// After cloning: `gh` as a fallback credential helper for an HTTPS origin
/// (D6), and the no-reply identity when Git has no `user.email` (D7). Best
/// effort: a failure is logged and the clone is still added.
pub(crate) async fn configure(path: &Path, gh: Option<&Path>, env: &[(&str, &str)]) {
    let https = repo_init::git(path, env, &["config", "--get", "remote.origin.url"])
        .await
        .is_ok_and(|url| url.trim().starts_with("https://"));
    if https {
        match crate::git_harness::find_gh(gh) {
            Ok(program) => add_helper(path, &program, env).await,
            Err(e) => tracing::warn!(
                code = e.code.as_str(),
                "[zwiim] clone: no gh for the credential helper"
            ),
        }
    }
    if !repo_init::has_email(path, env).await {
        match crate::git_harness::github_account(gh, &path.to_string_lossy()).await {
            Ok(account) => {
                if let Err(e) =
                    repo_init::set_identity(path, &Identity::from_account(&account)).await
                {
                    tracing::warn!(error = %e, "[zwiim] clone: could not set the identity");
                }
            }
            Err(e) => tracing::warn!(
                code = e.code.as_str(),
                "[zwiim] clone: no account for the identity"
            ),
        }
    }
}

async fn add_helper(path: &Path, program: &Path, env: &[(&str, &str)]) {
    let program = program.to_string_lossy();
    if program.contains('\'') {
        tracing::warn!("[zwiim] clone: gh path cannot be quoted; no credential helper");
        return;
    }
    let helper = format!("!'{program}' auth git-credential");
    let existing = repo_init::git(path, env, &["config", "--local", "--get-all", HELPER_KEY])
        .await
        .unwrap_or_default();
    if existing.lines().any(|l| l.trim() == helper) {
        return;
    }
    if let Err(e) = repo_init::git(
        path,
        env,
        &["config", "--local", "--add", HELPER_KEY, &helper],
    )
    .await
    {
        tracing::warn!(error = %e, "[zwiim] clone: could not add the credential helper");
    }
}

/// Startup: clones cut short by the last exit leave staging folders behind.
pub async fn sweep_cloning(repos_dir: &Path) {
    match tokio::fs::remove_dir_all(repos_dir.join(STAGING)).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(error = %e, "[zwiim] clone: could not sweep staging"),
    }
}

/// How long a finished job stays visible, so a client that reconnects sees
/// the outcome.
pub const RETENTION: Duration = Duration::from_secs(10 * 60);
/// Clones running at once; the rest wait in order.
const MAX_RUNNING: usize = 2;
/// Most often one job's progress is published.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// A clone printing nothing this long has stalled.
const STALL: Duration = Duration::from_secs(120);
/// How long a page of `github.repos` is reused.
const PAGE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CloneState {
    Queued,
    Cloning,
    Adding,
    Done,
    Failed,
    Cancelled,
}

impl CloneState {
    pub fn finished(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

/// Why a clone failed, in Zwiim's words (never Git's).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CloneFailure {
    pub code: String,
    pub message: String,
}

/// One clone as clients see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CloneJob {
    pub clone_id: String,
    pub owner: String,
    pub name: String,
    pub state: CloneState,
    pub phase: Option<ClonePhase>,
    pub percent: Option<u8>,
    /// The root workspace, once `done`.
    pub workspace_id: Option<String>,
    /// The worktree opened on it, once `done`.
    pub worktree_id: Option<String>,
    pub error: Option<CloneFailure>,
}

/// What a clone needs from the Host.
#[derive(Clone)]
pub struct CloneEnv {
    pub conn: DatabaseConnection,
    pub emitter: EventEmitter,
    pub repos_dir: PathBuf,
    pub worktrees_dir: PathBuf,
    pub gh: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum CloneStart {
    Job {
        clone_id: String,
    },
    /// A root already has this repository as `origin`.
    Existing {
        workspace_id: String,
    },
}

/// One `github.repos` row: the repository, and the root already opened on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GithubRepo {
    #[serde(flatten)]
    pub row: GithubRepoRow,
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GithubRepoPage {
    pub repos: Vec<GithubRepo>,
    pub next_page: Option<u32>,
}

enum Outcome {
    Done {
        workspace_id: String,
        worktree_id: String,
    },
    Failed(CloneFailure),
    Cancelled,
}

#[derive(Default)]
struct Inner {
    /// In creation order.
    jobs: Vec<CloneJob>,
    queue: VecDeque<(String, CloneEnv)>,
    running: usize,
    /// One per running job, from the moment it leaves the queue.
    cancels: HashMap<String, CancellationToken>,
    last_progress: HashMap<String, Instant>,
}

type Page = (Instant, Vec<GithubRepoRow>, Option<u32>);

/// The Host's clone jobs, in memory, and its cache of `github.repos` pages.
pub struct CloneJobs {
    inner: Mutex<Inner>,
    /// Bumped on every visible change; the host feed watches it.
    notify: watch::Sender<u64>,
    retention: Duration,
    pages: Mutex<HashMap<u32, Page>>,
}

impl CloneJobs {
    pub fn new(retention: Duration) -> Self {
        Self {
            inner: Mutex::default(),
            notify: watch::Sender::new(0),
            retention,
            pages: Mutex::default(),
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.notify.subscribe()
    }

    /// Every job still visible, oldest first.
    pub fn list(&self) -> Vec<CloneJob> {
        self.inner.lock().unwrap().jobs.clone()
    }

    fn changed(&self) {
        self.notify.send_modify(|v| *v = v.wrapping_add(1));
    }

    /// Open `owner/name`: the root already on it, the job already cloning
    /// it, or a new queued job.
    pub async fn start(
        self: &Arc<Self>,
        env: CloneEnv,
        owner: &str,
        name: &str,
    ) -> Result<CloneStart, ZwiimError> {
        super::check_target(owner, name)?;
        let key = format!("{owner}/{name}").to_lowercase();
        if let Some(workspace_id) = root_origins(&env.conn).await.remove(&key) {
            return Ok(CloneStart::Existing { workspace_id });
        }
        self.enqueue(env, owner, name, &key).await
    }

    /// The job already cloning `key`, or a new queued one. A job for `key`
    /// that is `done` may have finished after the caller read the roots: they
    /// are read again (not taken from the job: its root may since be gone).
    async fn enqueue(
        self: &Arc<Self>,
        env: CloneEnv,
        owner: &str,
        name: &str,
        key: &str,
    ) -> Result<CloneStart, ZwiimError> {
        let mut reread = false;
        let clone_id = loop {
            {
                let mut inner = self.inner.lock().unwrap();
                let same = |j: &&CloneJob| format!("{}/{}", j.owner, j.name).to_lowercase() == key;
                if let Some(job) = inner.jobs.iter().filter(same).find(|j| !j.state.finished()) {
                    return Ok(CloneStart::Job {
                        clone_id: job.clone_id.clone(),
                    });
                }
                let done = inner
                    .jobs
                    .iter()
                    .filter(same)
                    .any(|j| j.state == CloneState::Done);
                if reread || !done {
                    let clone_id = format!("c_{}", uuid::Uuid::new_v4().simple());
                    inner.jobs.push(CloneJob {
                        clone_id: clone_id.clone(),
                        owner: owner.to_string(),
                        name: name.to_string(),
                        state: CloneState::Queued,
                        phase: None,
                        percent: None,
                        workspace_id: None,
                        worktree_id: None,
                        error: None,
                    });
                    inner.queue.push_back((clone_id.clone(), env));
                    break clone_id;
                }
            }
            reread = true;
            if let Some(workspace_id) = root_origins(&env.conn).await.remove(key) {
                return Ok(CloneStart::Existing { workspace_id });
            }
        };
        self.changed();
        self.pump();
        Ok(CloneStart::Job { clone_id })
    }

    /// `true` when the clone is stopped (queued or cloning); `false` once it
    /// is adding or finished.
    pub fn cancel(self: &Arc<Self>, clone_id: &str) -> Result<bool, ZwiimError> {
        let mut inner = self.inner.lock().unwrap();
        let Some(state) = inner
            .jobs
            .iter()
            .find(|j| j.clone_id == clone_id)
            .map(|j| j.state)
        else {
            return Err(ZwiimError::not_found("No such clone"));
        };
        if state.finished() || state == CloneState::Adding {
            return Ok(false);
        }
        // Out of the queue already: its task stops at the token and records
        // the outcome (see `begin_adding`).
        if let Some(token) = inner.cancels.get(clone_id) {
            token.cancel();
            return Ok(true);
        }
        inner.queue.retain(|(id, _)| id != clone_id);
        if let Some(job) = inner.jobs.iter_mut().find(|j| j.clone_id == clone_id) {
            job.state = CloneState::Cancelled;
        }
        drop(inner);
        self.changed();
        self.expire(clone_id);
        Ok(true)
    }

    /// Start queued jobs while fewer than [`MAX_RUNNING`] run.
    fn pump(self: &Arc<Self>) {
        let mut inner = self.inner.lock().unwrap();
        while inner.running < MAX_RUNNING {
            let Some((clone_id, env)) = inner.queue.pop_front() else {
                break;
            };
            inner.running += 1;
            let token = CancellationToken::new();
            inner.cancels.insert(clone_id.clone(), token.clone());
            let this = self.clone();
            tokio::spawn(async move {
                // Run in its own task, so a panic still finishes the job.
                let run = {
                    let (this, clone_id, token) = (this.clone(), clone_id.clone(), token.clone());
                    tokio::spawn(async move { this.run(&env, &clone_id, &token).await })
                };
                let outcome = match run.await {
                    Ok(outcome) => outcome,
                    Err(e) => {
                        tracing::error!(error = %e, "[zwiim] clone: the job did not finish");
                        Outcome::Failed(failure("failed", "The clone failed"))
                    }
                };
                this.finish(&clone_id, outcome);
            });
        }
    }

    async fn run(
        self: &Arc<Self>,
        env: &CloneEnv,
        clone_id: &str,
        cancel: &CancellationToken,
    ) -> Outcome {
        let Some((owner, name)) = self
            .inner
            .lock()
            .unwrap()
            .jobs
            .iter()
            .find(|j| j.clone_id == clone_id)
            .map(|j| (j.owner.clone(), j.name.clone()))
        else {
            return Outcome::Cancelled;
        };
        #[cfg(test)]
        if owner == "panics" {
            panic!("a clone job panicked");
        }
        let slug = format!("{owner}/{name}").to_lowercase();
        let parent = env.repos_dir.join(&owner);
        let path = match existing_clone(&parent, &name, &slug).await {
            Some(path) if self.begin_adding(clone_id, cancel) => path,
            Some(_) => return Outcome::Cancelled,
            None => match self
                .clone_into_place(env, clone_id, &owner, &name, &parent, cancel)
                .await
            {
                Ok(path) => path,
                Err(outcome) => return outcome,
            },
        };
        configure(&path, env.gh.as_deref(), &[]).await;
        let db = AppDatabase {
            conn: env.conn.clone(),
        };
        let root = match super::add(&db, &env.emitter, &path.to_string_lossy(), true, None).await {
            Ok(root) => root,
            Err(e) => return Outcome::Failed(zwiim_failure(e)),
        };
        // An empty repository has no `origin/*` branch: `add` made the first
        // commit on the local branch, so the worktree starts there.
        let base = match super::origin_default_branch(&root.path).await {
            Some(_) => None,
            None => crate::commands::folders::resolve_git_head(&root.path)
                .await
                .ok()
                .and_then(|h| h.branch),
        };
        let prefix = crate::settings::branch_prefix(&env.conn, env.gh.as_deref(), &root.path).await;
        match super::create_worktree(
            &db,
            &env.emitter,
            &env.worktrees_dir,
            &root.workspace_id,
            base.as_deref(),
            prefix.as_deref(),
        )
        .await
        {
            Ok(worktree) => Outcome::Done {
                workspace_id: root.workspace_id,
                worktree_id: worktree.workspace_id,
            },
            Err(e) => Outcome::Failed(zwiim_failure(ZwiimError::from(e))),
        }
    }

    /// Clone into `.cloning/<clone_id>`, then rename to the destination.
    async fn clone_into_place(
        self: &Arc<Self>,
        env: &CloneEnv,
        clone_id: &str,
        owner: &str,
        name: &str,
        parent: &Path,
        cancel: &CancellationToken,
    ) -> Result<PathBuf, Outcome> {
        if cancel.is_cancelled() {
            return Err(Outcome::Cancelled);
        }
        self.advance(clone_id, CloneState::Cloning);
        let staging = env.repos_dir.join(STAGING).join(clone_id);
        if let Err(e) = tokio::fs::create_dir_all(env.repos_dir.join(STAGING)).await {
            tracing::warn!(error = %e, "[zwiim] clone: could not create staging");
            return Err(Outcome::Failed(failure("failed", "The clone failed")));
        }
        let cwd = env.repos_dir.to_string_lossy().into_owned();
        let this = self.clone();
        let id = clone_id.to_string();
        let result = git_harness::github_repo_clone(
            env.gh.as_deref(),
            &cwd,
            owner,
            name,
            &staging,
            STALL,
            move |phase, percent| this.progress(&id, phase, percent),
            cancel,
        )
        .await;
        let outcome = match result {
            Ok(CloneRun::Done) if self.begin_adding(clone_id, cancel) => None,
            Ok(_) => Some(Outcome::Cancelled),
            Err(e) => Some(Outcome::Failed(clone_failure(e))),
        };
        if let Some(outcome) = outcome {
            remove_staging(&staging).await;
            return Err(outcome);
        }
        let (from, to, name) = (staging.clone(), parent.to_path_buf(), name.to_string());
        match tokio::task::spawn_blocking(move || move_into_place(&from, &to, &name)).await {
            Ok(Ok(path)) => Ok(path),
            other => {
                tracing::warn!(
                    result = ?other.map(|r| r.map(|_| ())),
                    "[zwiim] clone: could not move into place"
                );
                remove_staging(&staging).await;
                Err(Outcome::Failed(failure("failed", "The clone failed")))
            }
        }
    }

    fn advance(&self, clone_id: &str, state: CloneState) {
        if let Some(job) = self
            .inner
            .lock()
            .unwrap()
            .jobs
            .iter_mut()
            .find(|j| j.clone_id == clone_id)
        {
            job.state = state;
        }
        self.changed();
    }

    /// Past the point of no return: `adding`, unless a cancel came first.
    /// Checked under the lock `cancel` takes, so `cancel` answers `true` only
    /// for a job that does stop.
    fn begin_adding(&self, clone_id: &str, cancel: &CancellationToken) -> bool {
        {
            let mut inner = self.inner.lock().unwrap();
            if cancel.is_cancelled() {
                return false;
            }
            if let Some(job) = inner.jobs.iter_mut().find(|j| j.clone_id == clone_id) {
                job.state = CloneState::Adding;
            }
        }
        self.changed();
        true
    }

    /// Record progress; publish a phase change at once, the rest at most
    /// every [`PROGRESS_EVERY`].
    fn progress(&self, clone_id: &str, phase: ClonePhase, percent: u8) {
        {
            let mut guard = self.inner.lock().unwrap();
            let Inner {
                jobs,
                last_progress,
                ..
            } = &mut *guard;
            let Some(job) = jobs.iter_mut().find(|j| j.clone_id == clone_id) else {
                return;
            };
            let now = Instant::now();
            let new_phase = job.phase != Some(phase);
            let recent = last_progress
                .get(clone_id)
                .is_some_and(|at| now.duration_since(*at) < PROGRESS_EVERY);
            if (!new_phase && recent) || (!new_phase && job.percent == Some(percent)) {
                return;
            }
            job.phase = Some(phase);
            job.percent = Some(percent);
            last_progress.insert(clone_id.to_string(), now);
        }
        self.changed();
    }

    fn finish(self: &Arc<Self>, clone_id: &str, outcome: Outcome) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.running -= 1;
            // `cancel` said `true` (the token is only cancelled before
            // `adding`): a failure racing it still ends `cancelled`. Read
            // under the lock `cancel` takes, so no cancel slips in after.
            let cancelled = inner
                .cancels
                .remove(clone_id)
                .is_some_and(|t| t.is_cancelled());
            let outcome = match outcome {
                Outcome::Failed(_) if cancelled => Outcome::Cancelled,
                outcome => outcome,
            };
            inner.last_progress.remove(clone_id);
            if let Some(job) = inner.jobs.iter_mut().find(|j| j.clone_id == clone_id) {
                match outcome {
                    Outcome::Done {
                        workspace_id,
                        worktree_id,
                    } => {
                        job.state = CloneState::Done;
                        job.workspace_id = Some(workspace_id);
                        job.worktree_id = Some(worktree_id);
                    }
                    Outcome::Failed(error) => {
                        job.state = CloneState::Failed;
                        job.error = Some(error);
                    }
                    Outcome::Cancelled => job.state = CloneState::Cancelled,
                }
            }
        }
        self.changed();
        self.expire(clone_id);
        self.pump();
    }

    /// Drop a finished job after the retention period.
    fn expire(self: &Arc<Self>, clone_id: &str) {
        let this = self.clone();
        let clone_id = clone_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(this.retention).await;
            let removed = {
                let mut inner = this.inner.lock().unwrap();
                let before = inner.jobs.len();
                inner.jobs.retain(|j| j.clone_id != clone_id);
                inner.jobs.len() != before
            };
            if removed {
                this.changed();
            }
        });
    }

    /// One page of the account's repositories, each with the root already
    /// opened on it. Pages are reused for [`PAGE_TTL`]; `refresh` drops them all.
    pub async fn github_repos(
        &self,
        conn: &DatabaseConnection,
        gh: Option<&Path>,
        cwd: &str,
        page: u32,
        refresh: bool,
    ) -> Result<GithubRepoPage, ZwiimError> {
        let cached = {
            let mut pages = self.pages.lock().unwrap();
            if refresh {
                pages.clear();
            }
            pages
                .get(&page)
                .filter(|(at, ..)| at.elapsed() < PAGE_TTL)
                .map(|(_, rows, next)| (rows.clone(), *next))
        };
        let (rows, next_page) = match cached {
            Some(hit) => hit,
            None => {
                let (rows, next) = git_harness::github_repos(gh, cwd, page)
                    .await
                    .map_err(repos_error)?;
                self.pages
                    .lock()
                    .unwrap()
                    .insert(page, (Instant::now(), rows.clone(), next));
                (rows, next)
            }
        };
        let origins = root_origins(conn).await;
        let repos = rows
            .into_iter()
            .map(|row| {
                let workspace_id = origins
                    .get(&format!("{}/{}", row.owner, row.name).to_lowercase())
                    .cloned();
                GithubRepo { row, workspace_id }
            })
            .collect();
        Ok(GithubRepoPage { repos, next_page })
    }
}

fn failure(code: &str, message: &str) -> CloneFailure {
    CloneFailure {
        code: code.to_string(),
        message: message.to_string(),
    }
}

/// R1: a clone failure as a fixed Zwiim code and message; what Git said is
/// logged on the Host only.
fn clone_failure(e: GitError) -> CloneFailure {
    let (code, message) = match e.code {
        GitErrorCode::GhUnavailable => {
            ("gh_unavailable", "GitHub CLI is not installed on the Host")
        }
        GitErrorCode::GhUnauthenticated => (
            "gh_unauthenticated",
            "GitHub CLI on the Host is not logged in",
        ),
        GitErrorCode::Network => ("network", "GitHub could not be reached"),
        GitErrorCode::NotFound => (
            "not_found",
            "The repository was not found, or this account cannot see it",
        ),
        GitErrorCode::AuthFailed => ("auth_failed", "GitHub refused the credentials"),
        GitErrorCode::Timeout => ("timeout", "The clone stopped making progress"),
        _ => ("failed", "The clone failed"),
    };
    tracing::warn!(code = e.code.as_str(), message = %e.message, detail = ?e.detail, "[zwiim] clone failed");
    failure(code, message)
}

/// An add or worktree failure keeps its Zwiim code and message.
fn zwiim_failure(e: ZwiimError) -> CloneFailure {
    tracing::warn!(code = e.code, message = %e.message, "[zwiim] clone: could not open the clone");
    CloneFailure {
        code: e.code.to_string(),
        message: e.message,
    }
}

/// `github.repos`: `gh` missing, signed out or unreachable keep their codes;
/// anything else is `github_unreachable`, logged.
fn repos_error(e: GitError) -> ZwiimError {
    match e.code {
        GitErrorCode::GhUnavailable
        | GitErrorCode::GhUnauthenticated
        | GitErrorCode::AuthFailed
        | GitErrorCode::Network
        | GitErrorCode::Timeout => ZwiimError::from(e),
        _ => {
            tracing::warn!(code = e.code.as_str(), message = %e.message, detail = ?e.detail, "[zwiim] github.repos failed");
            ZwiimError::github_unreachable()
        }
    }
}

async fn remove_staging(staging: &Path) {
    if let Err(e) = tokio::fs::remove_dir_all(staging).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(error = %e, "[zwiim] clone: could not remove staging");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::app_state::AppState;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn repo_with_origin(dir: &Path, origin: &str) {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q", "-b", "main"]);
        git(dir, &["remote", "add", "origin", origin]);
    }

    #[test]
    fn github_slug_normalizes_every_github_spelling() {
        for url in [
            "git@github.com:Octo/App.git",
            "git@github.com:octo/app",
            "ssh://git@github.com/octo/app.git",
            "https://github.com/octo/app",
            "https://github.com/Octo/App.git/",
            "https://user:x@github.com/octo/app.git",
        ] {
            assert_eq!(github_slug(url).as_deref(), Some("octo/app"), "{url}");
        }
        for url in [
            "https://gitlab.com/octo/app",
            "git@example.com:octo/app.git",
            "https://github.com/octo",
            "https://github.com/octo/app/tree/main",
            "/local/path",
            "",
        ] {
            assert_eq!(github_slug(url), None, "{url}");
        }
    }

    #[tokio::test]
    async fn root_origins_maps_github_roots_only() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_in_memory_db().await;
        let a = dir.path().join("a");
        repo_with_origin(&a, "git@github.com:Octo/App.git");
        let b = dir.path().join("b");
        repo_with_origin(&b, "https://gitlab.com/octo/other");
        let a_id = seed_folder(&db, a.to_str().unwrap()).await;
        seed_folder(&db, b.to_str().unwrap()).await;
        let a_ws = crate::workspace::workspace_id_of(&db.conn, a_id)
            .await
            .unwrap()
            .unwrap();
        let origins = root_origins(&db.conn).await;
        assert_eq!(origins.len(), 1);
        assert_eq!(origins.get("octo/app"), Some(&a_ws));
    }

    #[tokio::test]
    async fn existing_clone_finds_the_same_origin_only() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("octo");
        std::fs::create_dir_all(parent.join("app")).unwrap(); // plain folder
        repo_with_origin(&parent.join("app-3"), "https://github.com/octo/app.git");
        assert_eq!(
            existing_clone(&parent, "app", "octo/app").await,
            Some(parent.join("app-3"))
        );
        assert_eq!(existing_clone(&parent, "app", "octo/other").await, None);
    }

    #[test]
    fn move_into_place_skips_taken_names() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("octo");
        std::fs::create_dir_all(parent.join("app")).unwrap();
        std::fs::write(parent.join("app").join("mine.txt"), "keep").unwrap();
        let staging = dir.path().join(STAGING).join("c_1");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("README"), "hi").unwrap();
        let dest = move_into_place(&staging, &parent, "app").unwrap();
        assert_eq!(dest, parent.join("app-2"));
        assert!(dest.join("README").is_file());
        assert!(!staging.exists());
        assert_eq!(
            std::fs::read_to_string(parent.join("app").join("mine.txt")).unwrap(),
            "keep"
        );
    }

    #[tokio::test]
    async fn sweep_cloning_removes_staging_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(STAGING).join("c_x")).unwrap();
        std::fs::create_dir_all(dir.path().join("octo").join("app")).unwrap();
        sweep_cloning(dir.path()).await;
        assert!(!dir.path().join(STAGING).exists());
        assert!(dir.path().join("octo").join("app").exists());
        sweep_cloning(dir.path()).await; // nothing to sweep is fine
    }

    const ACCOUNT: &str = r#""api user") echo '{"login":"octo","id":42,"name":"Octo Cat"}';;"#;

    fn local(dir: &Path, key: &str) -> Vec<String> {
        let out = std::process::Command::new("git")
            .args(["config", "--local", "--get-all", key])
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(String::from)
            .collect()
    }

    #[tokio::test]
    async fn configure_adds_the_gh_helper_once_for_https() {
        let dir = tempfile::tempdir().unwrap();
        let (gh, _) = crate::git_harness::fake_gh(dir.path(), ACCOUNT);
        let repo = dir.path().join("r");
        repo_with_origin(&repo, "https://github.com/octo/app.git");
        let global = dir.path().join("global");
        std::fs::write(&global, "[user]\n\temail = me@example.com\n").unwrap();
        let env = [
            ("GIT_CONFIG_GLOBAL", global.to_str().unwrap()),
            ("GIT_CONFIG_SYSTEM", "/dev/null"),
        ];
        configure(&repo, Some(&gh), &env).await;
        configure(&repo, Some(&gh), &env).await;
        assert_eq!(
            local(&repo, "credential.https://github.com.helper"),
            vec![format!("!'{}' auth git-credential", gh.display())]
        );
        // An effective email exists: no identity written (D7).
        assert!(local(&repo, "user.email").is_empty());
    }

    #[tokio::test]
    async fn configure_skips_the_helper_for_ssh_and_sets_identity_without_email() {
        let dir = tempfile::tempdir().unwrap();
        let (gh, _) = crate::git_harness::fake_gh(dir.path(), ACCOUNT);
        let repo = dir.path().join("r");
        repo_with_origin(&repo, "git@github.com:octo/app.git");
        let env = [
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_SYSTEM", "/dev/null"),
        ];
        configure(&repo, Some(&gh), &env).await;
        assert!(local(&repo, "credential.https://github.com.helper").is_empty());
        assert_eq!(
            local(&repo, "user.email"),
            vec!["42+octo@users.noreply.github.com"]
        );
        assert_eq!(local(&repo, "user.name"), vec!["Octo Cat"]);
    }

    /// Fake `gh` arms: `octo/app` clones the fixture's bare repository and
    /// points `origin` at GitHub (with a local rewrite so fetches still work);
    /// `acme/*` hangs; `octo/gone` does not exist; `octo/empty` clones an
    /// empty repository; `octo/late` closes its output, then fails;
    /// `user/repos` lists `Octo/App`.
    const GH: &str = r#""api user") echo '{"login":"octo","id":42,"name":"Octo Cat"}';;
"api user/repos"*) echo '[{"full_name":"Octo/App","private":true,"size":5,"default_branch":"main"},{"full_name":"octo/lib","private":false,"size":1}]';;
"repo clone octo/app"*) git clone -q 'file://{bare}' "$4" && git -C "$4" remote set-url origin https://github.com/octo/app.git && git -C "$4" config 'url.file://{bare}.insteadOf' https://github.com/octo/app.git;;
"repo clone acme/"*) sleep 30 & echo $! >> '{pids}'; wait;;
"repo clone octo/gone"*) echo "GraphQL: Could not resolve to a Repository with the name 'octo/gone'. (repository)" >&2; exit 1;;
"repo clone octo/late"*) exec 2>&-; touch '{late}'; sleep 2; exit 1;;
"repo clone octo/empty"*) git clone -q 'file://{empty}' "$4" 2>/dev/null && git -C "$4" remote set-url origin https://github.com/octo/empty.git && git -C "$4" config 'url.file://{empty}.insteadOf' https://github.com/octo/empty.git;;"#;

    struct Jobs {
        dir: tempfile::TempDir,
        state: Arc<AppState>,
        calls: std::path::PathBuf,
    }

    async fn jobs() -> Jobs {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        git(&src, &["init", "-q", "-b", "main"]);
        std::fs::write(src.join("README"), "hi\n").unwrap();
        git(&src, &["add", "."]);
        git(
            &src,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "commit",
                "-q",
                "-m",
                "base",
            ],
        );
        let bare = dir.path().join("remote.git");
        git(
            dir.path(),
            &[
                "clone",
                "-q",
                "--bare",
                src.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        let empty = dir.path().join("empty.git");
        git(
            dir.path(),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                empty.to_str().unwrap(),
            ],
        );
        let body = GH
            .replace("{bare}", bare.to_str().unwrap())
            .replace("{empty}", empty.to_str().unwrap())
            .replace("{late}", dir.path().join("late").to_str().unwrap())
            .replace("{pids}", dir.path().join("pids").to_str().unwrap());
        let (gh, calls) = crate::git_harness::fake_gh(dir.path(), &body);
        let data_dir = dir.path().join("data");
        std::fs::create_dir(&data_dir).unwrap();
        let db = crate::db::init_database(&data_dir, "test").await.unwrap();
        let mut state = AppState::new_for_test(db, data_dir);
        state.gh = Some(gh);
        Jobs {
            dir,
            state: Arc::new(state),
            calls,
        }
    }

    impl Jobs {
        async fn start(&self, owner: &str, name: &str) -> Result<CloneStart, ZwiimError> {
            self.state
                .clones
                .start(self.state.clone_env(), owner, name)
                .await
        }

        async fn job_id(&self, owner: &str, name: &str) -> String {
            match self.start(owner, name).await.unwrap() {
                CloneStart::Job { clone_id } => clone_id,
                other => panic!("expected a job, got {other:?}"),
            }
        }

        async fn wait(&self, id: &str, what: &str, done: impl Fn(&CloneJob) -> bool) -> CloneJob {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                if let Some(job) = self
                    .state
                    .clones
                    .list()
                    .into_iter()
                    .find(|j| j.clone_id == id)
                {
                    if done(&job) {
                        return job;
                    }
                }
                assert!(Instant::now() < deadline, "clone {id} never {what}");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }

        async fn finish(&self, id: &str) -> CloneJob {
            self.wait(id, "finished", |j| j.state.finished()).await
        }

        fn calls(&self) -> String {
            std::fs::read_to_string(&self.calls).unwrap_or_default()
        }

        fn repos(&self) -> &std::path::Path {
            &self.state.repos_dir
        }

        async fn path_of(&self, workspace_id: &str) -> String {
            crate::workspace::list(&self.state.db.conn)
                .await
                .unwrap()
                .into_iter()
                .find(|w| w.workspace_id == workspace_id)
                .unwrap()
                .path
        }
    }

    fn staging_empty(repos: &std::path::Path) -> bool {
        std::fs::read_dir(repos.join(STAGING)).map_or(true, |mut d| d.next().is_none())
    }

    #[tokio::test]
    async fn a_clone_becomes_a_root_with_a_worktree() {
        let j = jobs().await;
        let id = j.job_id("octo", "app").await;
        assert!(id.starts_with("c_"));
        let job = j.finish(&id).await;
        assert_eq!(job.state, CloneState::Done, "{job:?}");
        assert!(job.error.is_none());
        let root = job.workspace_id.clone().unwrap();
        let wt = job.worktree_id.clone().unwrap();
        let root_path = j.path_of(&root).await;
        assert_eq!(
            std::path::Path::new(&root_path),
            j.repos().join("octo").join("app").canonicalize().unwrap()
        );
        assert!(!j.path_of(&wt).await.is_empty());
        assert!(staging_empty(j.repos()));
        assert_eq!(
            local(std::path::Path::new(&root_path), HELPER_KEY).len(),
            1,
            "gh credential helper added once"
        );
        // Opening it again goes to the existing root.
        assert_eq!(
            j.start("Octo", "APP").await.unwrap(),
            CloneStart::Existing { workspace_id: root }
        );
    }

    #[tokio::test]
    async fn an_empty_repository_gets_a_first_commit_and_a_worktree() {
        let j = jobs().await;
        let job = j.finish(&j.job_id("octo", "empty").await).await;
        assert_eq!(job.state, CloneState::Done, "{job:?}");
        let root = j.path_of(job.workspace_id.as_deref().unwrap()).await;
        assert!(root.ends_with("/octo/empty"), "{root}");
        assert!(!j
            .path_of(job.worktree_id.as_deref().unwrap())
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn a_start_racing_a_finished_clone_goes_to_its_root() {
        let j = jobs().await;
        let job = j.finish(&j.job_id("octo", "app").await).await;
        assert_eq!(job.state, CloneState::Done, "{job:?}");
        // `start` read the roots just before this job finished.
        let raced = j
            .state
            .clones
            .enqueue(j.state.clone_env(), "octo", "app", "octo/app")
            .await
            .unwrap();
        assert_eq!(
            raced,
            CloneStart::Existing {
                workspace_id: job.workspace_id.unwrap()
            }
        );
    }

    #[tokio::test]
    async fn an_existing_same_origin_folder_is_registered_without_cloning() {
        let j = jobs().await;
        let dest = j.repos().join("octo").join("app");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let bare = j.dir.path().join("remote.git");
        git(
            j.dir.path(),
            &[
                "clone",
                "-q",
                bare.to_str().unwrap(),
                dest.to_str().unwrap(),
            ],
        );
        git(
            &dest,
            &["remote", "set-url", "origin", "git@github.com:octo/app.git"],
        );
        let job = j.finish(&j.job_id("octo", "app").await).await;
        assert_eq!(job.state, CloneState::Done, "{job:?}");
        assert!(!j.calls().contains("repo clone"), "{}", j.calls());
        assert_eq!(
            std::path::Path::new(&j.path_of(job.workspace_id.as_deref().unwrap()).await),
            dest.canonicalize().unwrap()
        );
    }

    #[tokio::test]
    async fn a_taken_name_with_another_origin_clones_beside_it() {
        let j = jobs().await;
        let taken = j.repos().join("octo").join("app");
        std::fs::create_dir_all(&taken).unwrap();
        std::fs::write(taken.join("mine.txt"), "keep").unwrap();
        let job = j.finish(&j.job_id("octo", "app").await).await;
        assert_eq!(job.state, CloneState::Done, "{job:?}");
        let path = j.path_of(job.workspace_id.as_deref().unwrap()).await;
        assert!(path.ends_with("/octo/app-2"), "{path}");
        assert_eq!(
            std::fs::read_to_string(taken.join("mine.txt")).unwrap(),
            "keep"
        );
        assert!(!taken.join(".git").exists());
    }

    #[tokio::test]
    async fn a_missing_repository_fails_with_a_fixed_message_and_leaves_nothing() {
        let j = jobs().await;
        let job = j.finish(&j.job_id("octo", "gone").await).await;
        assert_eq!(job.state, CloneState::Failed);
        assert_eq!(
            job.error,
            Some(CloneFailure {
                code: "not_found".into(),
                message: "The repository was not found, or this account cannot see it".into(),
            })
        );
        assert!(staging_empty(j.repos()));
        assert!(!j.repos().join("octo").join("gone").exists());
    }

    #[tokio::test]
    async fn without_gh_the_job_fails_gh_unavailable() {
        let j = jobs().await;
        let mut env = j.state.clone_env();
        env.gh = Some(j.dir.path().join("no-gh"));
        let CloneStart::Job { clone_id } = j.state.clones.start(env, "octo", "app").await.unwrap()
        else {
            panic!("expected a job");
        };
        let job = j.finish(&clone_id).await;
        assert_eq!(job.error.unwrap().code, "gh_unavailable");
    }

    #[tokio::test]
    async fn hostile_targets_are_refused_before_gh_runs() {
        let j = jobs().await;
        for (owner, name) in [
            ("-o", "app"),
            ("octo", "ext::x"),
            ("octo", "a b"),
            ("octo", ".."),
            ("https://github.com/o/n", "app"),
            ("octo", "app/../../x"),
        ] {
            let e = j.start(owner, name).await.unwrap_err();
            assert_eq!(e.code, "invalid_input", "{owner}/{name}");
        }
        assert!(j.calls().is_empty(), "{}", j.calls());
        assert!(j.state.clones.list().is_empty());
    }

    #[tokio::test]
    async fn jobs_dedupe_queue_two_at_a_time_and_cancel() {
        let j = jobs().await;
        let a = j.job_id("acme", "a").await;
        let b = j.job_id("acme", "b").await;
        let c = j.job_id("acme", "c").await;
        assert_eq!(j.job_id("ACME", "A").await, a, "same repository, same job");
        j.wait(&a, "cloning", |x| x.state == CloneState::Cloning)
            .await;
        j.wait(&b, "cloning", |x| x.state == CloneState::Cloning)
            .await;
        assert_eq!(
            j.wait(&c, "listed", |_| true).await.state,
            CloneState::Queued
        );
        // Both hung clones are really running (a freshly written script's
        // first exec can take hundreds of ms on macOS) before they are
        // cancelled, so the process check below means something.
        let started = Instant::now() + Duration::from_secs(20);
        while std::fs::read_to_string(j.dir.path().join("pids")).map_or(0, |p| p.lines().count())
            < 2
        {
            assert!(Instant::now() < started, "hung clones never started");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert!(j.state.clones.cancel(&c).unwrap());
        assert_eq!(j.finish(&c).await.state, CloneState::Cancelled);
        assert!(j.state.clones.cancel(&a).unwrap());
        assert_eq!(j.finish(&a).await.state, CloneState::Cancelled);
        assert!(j.state.clones.cancel(&b).unwrap());
        assert_eq!(j.finish(&b).await.state, CloneState::Cancelled);
        assert!(!j.state.clones.cancel(&a).unwrap(), "finished: no effect");
        assert_eq!(
            j.state.clones.cancel("c_nope").unwrap_err().code,
            "not_found"
        );

        assert!(staging_empty(j.repos()));
        // Every hung clone process is gone (Review Focus 2).
        let pids = std::fs::read_to_string(j.dir.path().join("pids")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        for pid in pids.lines().map(|p| p.trim().parse::<i32>().unwrap()) {
            // SAFETY: signal 0 only probes for existence.
            while unsafe { libc::kill(pid, 0) } == 0 {
                assert!(
                    Instant::now() < deadline,
                    "pid {pid} outlived its cancelled clone"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }

    #[tokio::test]
    async fn a_clone_failing_after_a_cancel_ends_cancelled() {
        let j = jobs().await;
        let id = j.job_id("octo", "late").await;
        // The clone has closed its output and is about to fail.
        let deadline = Instant::now() + Duration::from_secs(20);
        while !j.dir.path().join("late").exists() {
            assert!(Instant::now() < deadline, "the clone never started");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(j.state.clones.cancel(&id).unwrap());
        let job = j.finish(&id).await;
        assert_eq!(job.state, CloneState::Cancelled, "{job:?}");
        assert!(job.error.is_none());
        assert!(staging_empty(j.repos()));
    }

    #[tokio::test]
    async fn a_panicking_job_fails_and_frees_its_slot() {
        let j = jobs().await;
        let id = j.job_id("panics", "app").await;
        let job = j.finish(&id).await;
        assert_eq!(job.state, CloneState::Failed);
        assert_eq!(
            job.error,
            Some(CloneFailure {
                code: "failed".into(),
                message: "The clone failed".into(),
            })
        );
        assert_eq!(j.state.clones.inner.lock().unwrap().running, 0);
        // Not deduped onto the dead job.
        assert_ne!(j.job_id("panics", "app").await, id);
    }

    #[tokio::test]
    async fn finished_jobs_expire_after_retention() {
        let j = jobs().await; // `new_for_test` retention: 2 s
        let id = j.job_id("octo", "gone").await;
        j.finish(&id).await;
        let deadline = Instant::now() + Duration::from_secs(6);
        while j.state.clones.list().iter().any(|x| x.clone_id == id) {
            assert!(Instant::now() < deadline, "finished job never expired");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn the_repository_list_is_cached_per_page_and_matches_roots() {
        let j = jobs().await;
        let root = j.dir.path().join("mine");
        repo_with_origin(&root, "git@github.com:Octo/App.git");
        let folder =
            crate::db::test_helpers::seed_folder(&j.state.db, root.to_str().unwrap()).await;
        let root_id = crate::workspace::workspace_id_of(&j.state.db.conn, folder)
            .await
            .unwrap()
            .unwrap();
        let cwd = j.dir.path().to_str().unwrap();
        let list = |refresh| {
            j.state
                .clones
                .github_repos(&j.state.db.conn, j.state.gh.as_deref(), cwd, 1, refresh)
        };

        let page = list(false).await.unwrap();
        assert_eq!(page.next_page, None);
        assert_eq!(page.repos.len(), 2);
        assert_eq!(
            (
                page.repos[0].row.owner.as_str(),
                page.repos[0].row.name.as_str()
            ),
            ("Octo", "App")
        );
        assert_eq!(
            page.repos[0].workspace_id.as_deref(),
            Some(root_id.as_str())
        );
        assert_eq!(page.repos[1].workspace_id, None);
        list(false).await.unwrap();
        assert_eq!(
            j.calls().matches("api user/repos").count(),
            1,
            "second read is cached"
        );
        list(true).await.unwrap();
        assert_eq!(
            j.calls().matches("api user/repos").count(),
            2,
            "refresh skips the cache"
        );
        let wire = serde_json::to_value(&page).unwrap();
        assert_eq!(wire["repos"][0]["owner"], "Octo");
        assert_eq!(wire["repos"][0]["workspace_id"], root_id.as_str());
    }

    #[tokio::test]
    async fn the_repository_list_maps_gh_failures_to_zwiim_errors() {
        let j = jobs().await;
        let cwd = j.dir.path().to_str().unwrap();
        let e = j
            .state
            .clones
            .github_repos(
                &j.state.db.conn,
                Some(&j.dir.path().join("no-gh")),
                cwd,
                1,
                false,
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, "gh_unavailable");
    }
}
