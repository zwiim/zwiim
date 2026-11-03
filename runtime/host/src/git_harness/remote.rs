//! Fetch, pull, and push, with the Host's credential handling: a stored
//! account matching an HTTPS remote is supplied through the Host's askpass
//! helper; otherwise Git uses the user's own helpers and SSH agent. Nothing
//! ever prompts.

use super::conflicts::{in_progress, ConflictState};
use super::error::{GitError, GitErrorCode};
use super::local::{config, current_branch, head, not_an_option, ok, require_branch};
use super::op::{FetchOutcome, PullMode, PullOutcome, PushOutcome};
use super::run::{self, REMOTE};
use super::GitHost;

pub(super) async fn fetch(
    host: &GitHost<'_>,
    root: &str,
    remote: Option<&str>,
) -> Result<FetchOutcome, GitError> {
    let remote = match remote {
        Some(r) => {
            not_an_option("remote", r)?;
            r.to_string()
        }
        None => default_remote(root).await?,
    };
    let mut cmd = remote_git(host, root, &remote).await;
    cmd.args(["fetch", &remote]);
    ok("fetch", run::run(cmd, REMOTE, "git fetch").await?)?;
    Ok(FetchOutcome { remote })
}

pub(super) async fn pull(
    host: &GitHost<'_>,
    root: &str,
    mode: PullMode,
) -> Result<PullOutcome, GitError> {
    let branch = require_branch(root).await?;
    let Some(remote) = config(root, &format!("branch.{branch}.remote")).await? else {
        return Err(GitError::new(
            GitErrorCode::NoUpstream,
            format!("Branch {branch} has no upstream; push it first"),
        ));
    };
    let before = head(root).await.ok();
    let mut cmd = remote_git(host, root, &remote).await;
    // `--no-rebase` so the user's `pull.rebase` cannot turn a pull into a
    // history rewrite.
    cmd.args(["pull", "--no-rebase"]);
    cmd.arg(match mode {
        PullMode::FastForwardOnly => "--ff-only",
        PullMode::Merge => "--no-edit",
    });
    ok("pull", run::run(cmd, REMOTE, "git pull").await?)?;
    let after = head(root).await?;
    Ok(PullOutcome {
        branch,
        updated: before.as_deref() != Some(after.as_str()),
        head: after,
    })
}

/// Push the current branch to the branch of the same name on `remote` (else
/// its upstream's remote, else the default remote). The upstream is set
/// whenever it is not already that branch, so a branch started from
/// `origin/main` never pushes into main. Never forces.
///
/// Refused with [`GitErrorCode::Conflict`] while files are unmerged or a
/// merge or rebase is stopped: what is committed is not the resolution yet.
/// Conflict markers in committed files never block a push; they may be
/// meant (fixtures, docs).
pub(super) async fn push(
    host: &GitHost<'_>,
    root: &str,
    remote: Option<&str>,
) -> Result<PushOutcome, GitError> {
    if let Some(ConflictState::InProgress { operation, files }) = in_progress(root).await? {
        if !files.is_empty() || matches!(operation.as_str(), "merge" | "rebase") {
            return Err(GitError::new(
                GitErrorCode::Conflict,
                "Finish resolving the conflicts before pushing",
            )
            .with_detail(&operation));
        }
    }
    let branch = require_branch(root).await?;
    let tracked_remote = config(root, &format!("branch.{branch}.remote")).await?;
    let tracked_ref = config(root, &format!("branch.{branch}.merge")).await?;
    let remote = match remote {
        Some(r) => {
            not_an_option("remote", r)?;
            r.to_string()
        }
        None => match tracked_remote.clone() {
            Some(r) if r != "." => r,
            _ => default_remote(root).await?,
        },
    };
    let full_ref = format!("refs/heads/{branch}");
    let set_upstream = tracked_remote.as_deref() != Some(remote.as_str())
        || tracked_ref.as_deref() != Some(&full_ref);

    let mut cmd = remote_git(host, root, &remote).await;
    cmd.arg("push");
    if set_upstream {
        cmd.arg("--set-upstream");
    }
    cmd.args([remote.as_str(), &format!("{full_ref}:{full_ref}")]);
    ok("push", run::run(cmd, REMOTE, "git push").await?)?;
    Ok(PushOutcome {
        remote,
        branch: current_branch(root).await?.unwrap_or(branch),
        head: head(root).await?,
        set_upstream,
    })
}

/// A `git` command for talking to `remote`: ssh never asks, and a stored
/// account is supplied when one matches.
pub(super) async fn remote_git(
    host: &GitHost<'_>,
    root: &str,
    remote: &str,
) -> tokio::process::Command {
    let mut cmd = run::git(root);
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    crate::git_credential::try_inject_for_repo_remote(
        &mut cmd,
        root,
        Some(remote),
        host.db,
        host.data_dir,
    )
    .await;
    cmd
}

/// `origin` when it exists, else the only remote.
async fn default_remote(root: &str) -> Result<String, GitError> {
    let mut cmd = run::git(root);
    cmd.arg("remote");
    let out = ok(
        "remote",
        run::run(cmd, super::run::READ, "git remote").await?,
    )?;
    let remotes: Vec<&str> = out
        .stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if remotes.contains(&"origin") {
        return Ok("origin".to_string());
    }
    match remotes.as_slice() {
        [only] => Ok(only.to_string()),
        [] => Err(GitError::new(
            GitErrorCode::NotFound,
            "The repository has no remote",
        )),
        _ => Err(GitError::new(
            GitErrorCode::InvalidInput,
            "The repository has several remotes and none is `origin`; name one",
        )),
    }
}
