//! Operations on the workspace's own repository.

use std::path::{Component, Path};
use std::time::Duration;

use sea_orm::DatabaseConnection;

use super::error::{classify_git, GitError, GitErrorCode};
use super::op::{BranchOutcome, CommitOutcome, StageOutcome, StatusEntry, StatusReport};
use super::run::{self, Output, READ, WRITE};
use crate::commands::folders::{self, GitLogResult};
use crate::db::entities::folder;
use crate::workspace_diff::WorkspaceDiff;

const DEFAULT_LOG: u32 = 20;
const MAX_LOG: u32 = 200;

pub(super) async fn status(root: &str) -> Result<StatusReport, GitError> {
    let entries = within(
        READ,
        "git status",
        folders::git_status(root.to_string(), None),
    )
    .await?;
    let mut cmd = run::git(root);
    cmd.args([
        "--no-optional-locks",
        "status",
        "--porcelain=v2",
        "--branch",
        "--untracked-files=no",
    ]);
    let header = ok("status", run::run(cmd, READ, "git status").await?)?;
    let mut report = StatusReport {
        entries: entries
            .into_iter()
            .map(|e| StatusEntry {
                status: e.status,
                path: e.file,
            })
            .collect(),
        ..StatusReport::default()
    };
    for line in header.stdout.lines() {
        let Some(rest) = line.strip_prefix("# branch.") else {
            continue;
        };
        let (key, value) = rest.split_once(' ').unwrap_or((rest, ""));
        match key {
            "oid" if value != "(initial)" => report.head = Some(value.to_string()),
            "head" if value != "(detached)" => report.branch = Some(value.to_string()),
            "upstream" => report.upstream = Some(value.to_string()),
            "ab" => {
                let mut counts = value
                    .split(' ')
                    .map(|n| n.trim_start_matches(['+', '-']).parse().unwrap_or(0));
                report.ahead = counts.next().unwrap_or(0);
                report.behind = counts.next().unwrap_or(0);
            }
            _ => {}
        }
    }
    Ok(report)
}

pub(super) async fn diff(
    row: &folder::Model,
    file: Option<&str>,
) -> Result<WorkspaceDiff, GitError> {
    if let Some(file) = file {
        relative(file)?;
    }
    within(READ, "git diff", crate::workspace_diff::diff(row, file)).await
}

pub(super) async fn log(root: &str, limit: Option<u32>) -> Result<GitLogResult, GitError> {
    let limit = limit.unwrap_or(DEFAULT_LOG).clamp(1, MAX_LOG);
    within(
        READ,
        "git log",
        folders::git_log(
            root.to_string(),
            Some(limit),
            None,
            None,
            None,
            None,
            Some(false),
            Some(false),
        ),
    )
    .await
}

pub(super) async fn branch_create(
    root: &str,
    name: &str,
    from: Option<&str>,
) -> Result<BranchOutcome, GitError> {
    branch_name(root, name).await?;
    if let Some(from) = from {
        not_an_option("from", from)?;
    }
    // `--no-track`: a branch started from `origin/main` is new work, not a
    // copy of main; its upstream is set when it is first pushed.
    let mut cmd = run::git(root);
    cmd.args(["switch", "--no-track", "-c", name]);
    if let Some(from) = from {
        cmd.arg(from);
    }
    ok("switch", run::run(cmd, WRITE, "git switch").await?)?;
    Ok(BranchOutcome {
        branch: name.to_string(),
        head: head(root).await?,
    })
}

pub(super) async fn switch(root: &str, branch: &str) -> Result<BranchOutcome, GitError> {
    not_an_option("branch", branch)?;
    let mut cmd = run::git(root);
    cmd.args(["switch", branch]);
    ok("switch", run::run(cmd, WRITE, "git switch").await?)?;
    Ok(BranchOutcome {
        branch: current_branch(root)
            .await?
            .unwrap_or_else(|| branch.to_string()),
        head: head(root).await?,
    })
}

pub(super) async fn stage(root: &str, paths: &[String]) -> Result<StageOutcome, GitError> {
    add(root, paths).await?;
    Ok(StageOutcome {
        staged: staged(root, &[]).await?,
    })
}

pub(super) async fn commit(
    db: &DatabaseConnection,
    root: &str,
    message: &str,
    paths: Option<&[String]>,
) -> Result<CommitOutcome, GitError> {
    if message.trim().is_empty() {
        return Err(GitError::new(
            GitErrorCode::InvalidInput,
            "A commit needs a message",
        ));
    }
    if let Some(paths) = paths {
        add(root, paths).await?;
    }

    let mut cmd = run::git(root);
    // The same author the Host's own commits use: the stored account
    // matching the remote, else the user's Git config.
    if let Some((name, email)) = crate::git_credential::resolve_commit_author(root, db).await {
        cmd.args([
            "-c",
            &format!("user.name={name}"),
            "-c",
            &format!("user.email={email}"),
        ]);
    }
    cmd.args(["commit", "-m", message]);
    if let Some(paths) = paths {
        // `git commit -- <paths>` commits exactly these, leaving anything
        // else that is staged for a later commit.
        cmd.arg("--").args(paths.iter().map(|p| literal(p)));
    }
    ok("commit", run::run(cmd, WRITE, "git commit").await?)?;

    let files = git_lines(
        root,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "-z",
            "--root",
            "HEAD",
        ],
    )
    .await?;
    Ok(CommitOutcome {
        commit: head(root).await?,
        branch: current_branch(root).await?,
        files,
    })
}

/// `git add -A` exactly `paths`. A path whose deletion is already staged is
/// skipped: it is in neither the index nor the tree, and `git add` would
/// refuse it as unknown.
async fn add(root: &str, paths: &[String]) -> Result<(), GitError> {
    if paths.is_empty() {
        return Err(GitError::new(GitErrorCode::InvalidPath, "No paths given"));
    }
    for path in paths {
        relative(path)?;
    }
    let deleted = staged(root, &["--diff-filter=D"]).await?;
    let pending: Vec<String> = paths
        .iter()
        .filter(|p| !deleted.iter().any(|d| d == p.trim_end_matches('/')))
        .map(|p| literal(p))
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    let mut cmd = run::git(root);
    cmd.args(["add", "-A", "--"]).args(&pending);
    ok("add", run::run(cmd, WRITE, "git add").await?)?;
    Ok(())
}

async fn staged(root: &str, filter: &[&str]) -> Result<Vec<String>, GitError> {
    let mut args = vec!["diff", "--cached", "--name-only", "-z"];
    args.extend_from_slice(filter);
    git_lines(root, &args).await
}

/// `git <args>` whose output is NUL-separated paths.
async fn git_lines(root: &str, args: &[&str]) -> Result<Vec<String>, GitError> {
    let mut cmd = run::git(root);
    cmd.args(args);
    let out = ok(args[0], run::run(cmd, READ, "git").await?)?;
    Ok(out
        .stdout
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect())
}

pub(super) async fn head(root: &str) -> Result<String, GitError> {
    let mut cmd = run::git(root);
    cmd.args(["rev-parse", "HEAD"]);
    let out = ok("rev-parse", run::run(cmd, READ, "git rev-parse").await?)?;
    Ok(out.stdout.trim().to_string())
}

/// The checked-out branch; `None` on a detached `HEAD`.
pub(super) async fn current_branch(root: &str) -> Result<Option<String>, GitError> {
    let mut cmd = run::git(root);
    cmd.args(["symbolic-ref", "--quiet", "--short", "HEAD"]);
    let out = run::run(cmd, READ, "git symbolic-ref").await?;
    let branch = out.stdout.trim();
    Ok((out.status.success() && !branch.is_empty()).then(|| branch.to_string()))
}

/// The checked-out branch, or `detached_head`.
pub(super) async fn require_branch(root: &str) -> Result<String, GitError> {
    current_branch(root).await?.ok_or_else(|| {
        GitError::new(
            GitErrorCode::DetachedHead,
            "HEAD is not on a branch; switch to or create one first",
        )
    })
}

/// `git config --get <key>`, `None` when unset.
pub(super) async fn config(root: &str, key: &str) -> Result<Option<String>, GitError> {
    let mut cmd = run::git(root);
    cmd.args(["config", "--get", key]);
    let out = run::run(cmd, READ, "git config").await?;
    let value = out.stdout.trim();
    Ok((out.status.success() && !value.is_empty()).then(|| value.to_string()))
}

/// The run's output when it succeeded, else its classified failure.
pub(super) fn ok(op: &str, out: Output) -> Result<Output, GitError> {
    if out.status.success() {
        Ok(out)
    } else {
        Err(classify_git(op, &out.text()))
    }
}

/// A Host Git helper (status, diff, log), bounded by `limit`. Their main
/// `git` dies with a dropped call; the rest are short read-only queries that
/// cannot prompt (stdin is closed) and end on their own.
async fn within<T>(
    limit: Duration,
    what: &str,
    fut: impl std::future::Future<Output = Result<T, crate::app_error::AppCommandError>>,
) -> Result<T, GitError> {
    match tokio::time::timeout(limit, fut).await {
        Ok(result) => result.map_err(GitError::from_app),
        Err(_) => Err(GitError::new(
            GitErrorCode::Timeout,
            format!("{what}: timed out after {}s", limit.as_secs()),
        )),
    }
}

/// Reject a path that is empty, absolute, or climbs out of the workspace.
pub(super) fn relative(path: &str) -> Result<(), GitError> {
    let invalid = |why: &str| GitError::new(GitErrorCode::InvalidPath, format!("{why}: {path}"));
    if path.is_empty() || path.contains('\0') {
        return Err(invalid("Empty path"));
    }
    for component in Path::new(path).components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(invalid("Paths are relative to the workspace"))
            }
            Component::ParentDir => return Err(invalid("Path leaves the workspace")),
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(())
}

/// A path as a pathspec that matches only itself: no globs, no magic.
fn literal(path: &str) -> String {
    format!(":(literal){path}")
}

/// Reject an argument Git would read as an option.
pub(super) fn not_an_option(what: &str, value: &str) -> Result<(), GitError> {
    if value.trim().is_empty() || value.starts_with('-') {
        return Err(GitError::new(
            GitErrorCode::InvalidInput,
            format!("Invalid {what}: {value:?}"),
        ));
    }
    Ok(())
}

async fn branch_name(root: &str, name: &str) -> Result<(), GitError> {
    not_an_option("branch name", name)?;
    let mut cmd = run::git(root);
    cmd.args(["check-ref-format", "--branch", name]);
    if run::run(cmd, READ, "git check-ref-format")
        .await?
        .status
        .success()
    {
        Ok(())
    } else {
        Err(GitError::new(
            GitErrorCode::InvalidInput,
            format!("Not a valid branch name: {name:?}"),
        ))
    }
}
