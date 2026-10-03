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

mod error;
mod github;
mod local;
mod op;
mod remote;
mod run;

use std::path::Path;
use std::time::Instant;

use sea_orm::DatabaseConnection;

use crate::app_error::AppErrorCode;
use crate::db::entities::folder;

pub use error::{GitError, GitErrorCode};
pub use op::{
    BranchOutcome, CommitOutcome, FetchOutcome, GitOp, GitOpKind, GitOutcome, MergeMethod, PrCheck,
    PrComment, PrComments, PrCreated, PrMerged, PrReview, PrReviewComment, PrSummary, PullMode,
    PullOutcome, PushOutcome, StageOutcome, StatusEntry, StatusReport,
};

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
    let started = Instant::now();
    let result = run_op(host, caller, op).await;
    let elapsed_ms = started.elapsed().as_millis();
    match &result {
        Ok(_) => tracing::info!(
            op = kind.as_str(),
            workspace_id = %caller.workspace_id,
            session_id = ?caller.session_id,
            elapsed_ms,
            "[git-harness] ok"
        ),
        Err(e) => tracing::info!(
            op = kind.as_str(),
            workspace_id = %caller.workspace_id,
            session_id = ?caller.session_id,
            elapsed_ms,
            code = e.code.as_str(),
            "[git-harness] failed"
        ),
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
        GitOp::PrMerge { number, method } => github::pr_merge(host, root, number, method)
            .await
            .map(GitOutcome::PrMerge),
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
mod tests;
