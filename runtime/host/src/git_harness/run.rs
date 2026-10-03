//! Running `git` and `gh`: non-interactive, time-limited, and never outliving
//! the call.
//!
//! Each child starts in its own process group. If the call times out or its
//! future is dropped, the whole group is killed, so a hung credential helper,
//! hook, or ssh under `git` goes with it. A child that exits normally leaves
//! its group alone: anything it deliberately left running (a credential cache
//! daemon) is the user's.

use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use super::error::{GitError, GitErrorCode};

/// Reading the repository: status, diff, log, rev-parse.
pub(super) const READ: Duration = Duration::from_secs(30);
/// Changing it locally. Generous because commit runs the user's hooks.
pub(super) const WRITE: Duration = Duration::from_secs(120);
/// Talking to a remote, hooks included.
pub(super) const REMOTE: Duration = Duration::from_secs(180);
/// One `gh` call.
pub(super) const GITHUB: Duration = Duration::from_secs(60);

pub(super) struct Output {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// Git prints some refusals (`nothing to commit`) on stdout.
    pub fn text(&self) -> String {
        format!("{}\n{}", self.stderr.trim(), self.stdout.trim())
    }
}

/// A `git` command in `root`, prompts off.
pub(super) fn git(root: &str) -> tokio::process::Command {
    let mut cmd = crate::process::tokio_command("git");
    cmd.args(["-c", "core.quotePath=false"])
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_MERGE_AUTOEDIT", "no");
    cmd
}

/// A `gh` command in `root`, prompts, colors, and update checks off.
pub(super) fn gh(program: &Path, root: &str) -> tokio::process::Command {
    let mut cmd = crate::process::tokio_command(program);
    cmd.current_dir(root)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_SPINNER_DISABLED", "1")
        .env("NO_COLOR", "1")
        .env("CLICOLOR", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    cmd
}

/// Run `cmd` to completion within `limit`. `what` names it in errors.
pub(super) async fn run(
    mut cmd: tokio::process::Command,
    limit: Duration,
    what: &str,
) -> Result<Output, GitError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);

    let child = crate::process::spawn_retrying_exec_busy(|| cmd.spawn())
        .await
        .map_err(|e| {
            GitError::new(GitErrorCode::Failed, format!("{what}: could not start"))
                .with_detail(&e.to_string())
        })?;
    let group = GroupKill::new(child.id());

    match tokio::time::timeout(limit, child.wait_with_output()).await {
        Ok(Ok(out)) => {
            group.disarm();
            Ok(Output {
                status: out.status,
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        }
        Ok(Err(e)) => Err(
            GitError::new(GitErrorCode::Failed, format!("{what}: failed"))
                .with_detail(&e.to_string()),
        ),
        Err(_) => Err(GitError::new(
            GitErrorCode::Timeout,
            format!("{what}: timed out after {}s", limit.as_secs()),
        )),
    }
}

/// Kills the child's process group when dropped, unless disarmed.
struct GroupKill {
    #[cfg_attr(not(unix), allow(dead_code))]
    pgid: Option<u32>,
}

impl GroupKill {
    fn new(pid: Option<u32>) -> Self {
        Self { pgid: pid }
    }

    fn disarm(mut self) {
        self.pgid = None;
    }
}

impl Drop for GroupKill {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pgid) = self.pgid.and_then(|p| i32::try_from(p).ok()) {
            // SAFETY: plain syscall on a group this module created; ESRCH when
            // it is already gone is fine.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
}

/// The `gh` to run: `configured`, else the user's installed one (`PATH`, then
/// the usual Homebrew locations, which a GUI-launched Host's `PATH` lacks).
pub(super) fn find_gh(configured: Option<&Path>) -> Result<std::path::PathBuf, GitError> {
    if let Some(path) = configured {
        return if path.is_file() {
            Ok(path.to_path_buf())
        } else {
            Err(gh_unavailable())
        };
    }
    if let Ok(path) = which::which("gh") {
        return Ok(path);
    }
    [
        "/opt/homebrew/bin/gh",
        "/usr/local/bin/gh",
        "/home/linuxbrew/.linuxbrew/bin/gh",
    ]
    .iter()
    .map(Path::new)
    .find(|p| p.is_file())
    .map(Path::to_path_buf)
    .ok_or_else(gh_unavailable)
}

fn gh_unavailable() -> GitError {
    GitError::new(
        GitErrorCode::GhUnavailable,
        "The GitHub CLI (gh) is not installed. Install it from https://cli.github.com and run `gh auth login`.",
    )
}
