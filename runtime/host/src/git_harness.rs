//! The Git Harness: workspace-scoped Git and GitHub operations the Host runs
//! on an agent's behalf, so routine Git work (inspect, branch, stage, commit,
//! fetch, pull, push, open and follow a pull request) goes through one typed
//! Host capability instead of an agent's own shell.
//!
//! [`execute`] takes a [`GitCaller`] and a [`GitOp`] and returns a
//! [`GitOutcome`] or a [`GitError`]. Every operation is scoped to a Zwiim
//! workspace, named by its `workspace_id` and resolved by the Host's workspace
//! facade: the repository root always comes from the workspace row, never
//! from the caller. `session_id` only attributes the call in logs.
//!
//! The Harness is a capability, not a policy. It decides nothing about
//! approval: [`GitOp::kind`] and [`GitOp::mutates`] are facts an agent
//! integration reads to apply its own approval and plan-mode rules. What keeps
//! the surface safe is its shape. There is no raw `git <args>`, no force push,
//! no reset, clean or branch deletion; those stay with the agent's own tools
//! and whatever approval they already carry.
//!
//! Execution: every subprocess runs non-interactively, with a per-operation
//! time limit, in its own process group that is killed when the call times out
//! or its future is dropped. Remote Git uses the Host's credential handling
//! (stored accounts, else the user's own helpers and SSH agent); GitHub
//! operations run the user's installed `gh` with the user's own `gh` login.
//! Nothing here stores or passes a token.
//!
//! This module knows nothing about how an agent reached it. Agent integrations
//! depend on it; it never depends on them.

mod account;
pub mod checkpoint;
mod clone;
mod error;
mod github;
mod local;
mod op;
mod pr_state;
mod remote;
mod run;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use sea_orm::DatabaseConnection;

use crate::app_error::AppErrorCode;
use crate::db::entities::folder;

pub use account::{
    github_account, github_owners, github_repo_create, github_repo_exists, github_repos,
    GithubAccount, GithubOwner, GithubOwnerKind, GithubRepoRow,
};
pub use clone::{github_repo_clone, ClonePhase, CloneRun};
pub use error::{GitError, GitErrorCode};
pub use op::{
    BranchOutcome, CommitOutcome, FetchOutcome, GitOp, GitOpKind, GitOutcome, MergeMethod, PrCheck,
    PrComment, PrComments, PrCreated, PrMerged, PrReview, PrReviewComment, PrSummary, PullMode,
    PullOutcome, PushOutcome, StageOutcome, StatusEntry, StatusReport,
};
pub use pr_state::{
    CheckState, MergeStatus, PrCheckItem, PrChecksSummary, PrLifecycle, PrMergeInfo, PrState,
};

/// The `gh` the Host runs: `configured`, else the user's installed one.
pub fn find_gh(configured: Option<&Path>) -> Result<PathBuf, GitError> {
    run::find_gh(configured)
}

/// The Host services the Harness runs against.
#[derive(Clone, Copy)]
pub struct GitHost<'a> {
    pub db: &'a DatabaseConnection,
    /// The Host's effective data dir; remote Git finds its credential helper
    /// there.
    pub data_dir: &'a Path,
    /// The `gh` executable. `None` discovers the user's installed `gh` on each
    /// GitHub operation.
    pub gh: Option<&'a Path>,
}

impl<'a> GitHost<'a> {
    pub fn new(db: &'a DatabaseConnection, data_dir: &'a Path) -> Self {
        Self {
            db,
            data_dir,
            gh: None,
        }
    }
}

/// Who asks: the workspace the operation is scoped to, and optionally the
/// Zwiim session it is attributed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCaller {
    pub workspace_id: String,
    pub session_id: Option<String>,
}

impl GitCaller {
    /// A caller for `workspace_id`. Work that has no workspace (a chat
    /// scratch folder, an internal conversation) is `no_workspace`.
    pub fn new(workspace_id: Option<String>, session_id: Option<String>) -> Result<Self, GitError> {
        match workspace_id {
            Some(id) if !id.trim().is_empty() => Ok(Self {
                workspace_id: id,
                session_id,
            }),
            _ => Err(GitError::new(
                GitErrorCode::NoWorkspace,
                "Git operations need a workspace",
            )),
        }
    }
}

/// Run `op` in the caller's workspace.
pub async fn execute(
    host: &GitHost<'_>,
    caller: &GitCaller,
    op: GitOp,
) -> Result<GitOutcome, GitError> {
    let kind = op.kind();
    let mutates = op.mutates();
    let started = Instant::now();
    let result = run_op(host, caller, op).await;
    log_result(kind.as_str(), caller, started, result.as_ref().err());
    if result.is_ok() && mutates {
        announce(&caller.workspace_id, kind);
    }
    result
}

fn log_result(op: &str, caller: &GitCaller, started: Instant, error: Option<&GitError>) {
    let elapsed_ms = started.elapsed().as_millis();
    match error {
        None => tracing::info!(
            op,
            workspace_id = %caller.workspace_id,
            session_id = ?caller.session_id,
            elapsed_ms,
            "[git-harness] ok"
        ),
        Some(e) => tracing::info!(
            op,
            workspace_id = %caller.workspace_id,
            session_id = ?caller.session_id,
            elapsed_ms,
            code = e.code.as_str(),
            "[git-harness] failed"
        ),
    }
}

/// A change the Harness made: `kind` succeeded in `workspace_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitChange {
    pub workspace_id: String,
    pub kind: GitOpKind,
}

fn changes() -> &'static tokio::sync::broadcast::Sender<GitChange> {
    static CHANGES: OnceLock<tokio::sync::broadcast::Sender<GitChange>> = OnceLock::new();
    CHANGES.get_or_init(|| tokio::sync::broadcast::channel(256).0)
}

/// Every change the Harness makes from now on, whoever asked for it. Lets
/// Host state that depends on the repository or GitHub (a pull request's
/// status) re-read it right away instead of waiting to notice.
pub fn subscribe_changes() -> tokio::sync::broadcast::Receiver<GitChange> {
    changes().subscribe()
}

fn announce(workspace_id: &str, kind: GitOpKind) {
    let _ = changes().send(GitChange {
        workspace_id: workspace_id.to_string(),
        kind,
    });
}

/// The pull request for `branch` in the caller's workspace — open, merged or
/// closed — or `None` when the branch has none.
pub async fn pr_state(
    host: &GitHost<'_>,
    caller: &GitCaller,
    branch: &str,
) -> Result<Option<PrState>, GitError> {
    let started = Instant::now();
    let result = async {
        let row = resolve_workspace(host.db, &caller.workspace_id).await?;
        crate::git_repo::ensure_git_repo(&row.path).map_err(GitError::from_app)?;
        github::pr_state(host, &row.path, branch).await
    }
    .await;
    if let Err(e) = &result {
        log_result("pr_state", caller, started, Some(e));
    }
    result
}

/// Merge pull request `number` for the user, but only if its head is still
/// `head_sha` — the commit they saw — and GitHub would merge it now. `method`
/// defaults to the repository's preferred one.
pub async fn merge_pr(
    host: &GitHost<'_>,
    caller: &GitCaller,
    number: u64,
    head_sha: &str,
    method: Option<MergeMethod>,
) -> Result<PrMerged, GitError> {
    let started = Instant::now();
    let result = async {
        if head_sha.trim().is_empty() {
            return Err(GitError::new(
                GitErrorCode::InvalidInput,
                "A merge needs the head commit it was reviewed at",
            ));
        }
        let row = resolve_workspace(host.db, &caller.workspace_id).await?;
        crate::git_repo::ensure_git_repo(&row.path).map_err(GitError::from_app)?;
        github::pr_merge(host, &row.path, Some(number), method, Some(head_sha)).await
    }
    .await;
    log_result("pr_merge", caller, started, result.as_ref().err());
    if result.is_ok() {
        announce(&caller.workspace_id, GitOpKind::PrMerge);
    }
    result
}

async fn run_op(host: &GitHost<'_>, caller: &GitCaller, op: GitOp) -> Result<GitOutcome, GitError> {
    let row = resolve_workspace(host.db, &caller.workspace_id).await?;
    crate::git_repo::ensure_git_repo(&row.path).map_err(GitError::from_app)?;
    let root = row.path.as_str();
    match op {
        GitOp::Status => local::status(root).await.map(GitOutcome::Status),
        GitOp::Diff { file } => local::diff(&row, file.as_deref())
            .await
            .map(GitOutcome::Diff),
        GitOp::Log { limit } => local::log(root, limit).await.map(GitOutcome::Log),
        GitOp::BranchCreate { name, from } => local::branch_create(root, &name, from.as_deref())
            .await
            .map(GitOutcome::BranchCreate),
        GitOp::BranchRename { name } => local::branch_rename(&row, &name)
            .await
            .map(GitOutcome::BranchRename),
        GitOp::Switch { branch } => local::switch(root, &branch).await.map(GitOutcome::Switch),
        GitOp::Stage { paths } => local::stage(root, &paths).await.map(GitOutcome::Stage),
        GitOp::Commit { message, paths } => {
            local::commit(host.db, root, &message, paths.as_deref())
                .await
                .map(GitOutcome::Commit)
        }
        GitOp::Fetch { remote } => remote::fetch(host, root, remote.as_deref())
            .await
            .map(GitOutcome::Fetch),
        GitOp::Pull { mode } => remote::pull(host, root, mode).await.map(GitOutcome::Pull),
        GitOp::Push { remote } => remote::push(host, root, remote.as_deref())
            .await
            .map(GitOutcome::Push),
        GitOp::PrCreate {
            title,
            body,
            base,
            draft,
        } => github::pr_create(host, root, &title, &body, base.as_deref(), draft)
            .await
            .map(GitOutcome::PrCreate),
        GitOp::PrView { number } => github::pr_view(host, root, number)
            .await
            .map(GitOutcome::PrView),
        GitOp::PrChecks { number } => github::pr_checks(host, root, number)
            .await
            .map(|checks| GitOutcome::PrChecks { checks }),
        GitOp::PrComments { number } => github::pr_comments(host, root, number)
            .await
            .map(GitOutcome::PrComments),
        GitOp::PrMerge { number, method } => {
            github::pr_merge(host, root, number, Some(method), None)
                .await
                .map(GitOutcome::PrMerge)
        }
    }
}

/// `workspace_id → Folder row` through the Host's workspace facade: an unknown
/// workspace is `no_workspace`, a removed worktree `workspace_removed`.
async fn resolve_workspace(
    db: &DatabaseConnection,
    workspace_id: &str,
) -> Result<folder::Model, GitError> {
    if workspace_id.trim().is_empty() {
        return Err(GitError::new(
            GitErrorCode::NoWorkspace,
            "Git operations need a workspace",
        ));
    }
    crate::workspace::resolve(db, workspace_id)
        .await
        .map_err(|e| match e.code {
            AppErrorCode::NotFound => {
                GitError::new(GitErrorCode::NoWorkspace, "The workspace does not exist")
            }
            _ => GitError::from_app(e),
        })
}

#[cfg(test)]
mod checkpoint_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
/// A stand-in `gh` running `body` (a `case "$*"` arm list), recording each
/// invocation's arguments in `calls`.
pub(crate) fn fake_gh(dir: &Path, body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let script = dir.join("gh");
    let calls = dir.join("gh-calls");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$* | prompt=$GH_PROMPT_DISABLED\" >> '{}'\ncase \"$*\" in\n{body}\nesac\n",
            calls.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    (script, calls)
}
