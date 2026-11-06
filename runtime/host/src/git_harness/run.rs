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
/// One `gh` call that creates or changes something (`repo create --push`).
pub(super) const GITHUB: Duration = Duration::from_secs(60);
/// A read-only `gh` call (account lookup, orgs, "does this repo exist"):
/// short, so a packet-dropping network fails fast instead of holding the
/// caller's own timeout (the SDK client's request timeout is 60s, and
/// `workspace.inspect` makes two of these in sequence).
pub(super) const GITHUB_READ: Duration = Duration::from_secs(15);

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
    cmd: tokio::process::Command,
    limit: Duration,
    what: &str,
) -> Result<Output, GitError> {
    run_inner(cmd, None, limit, what).await
}

/// As [`run`], with `input` written to the child's stdin (then closed).
pub(super) async fn run_with_input(
    cmd: tokio::process::Command,
    input: &[u8],
    limit: Duration,
    what: &str,
) -> Result<Output, GitError> {
    run_inner(cmd, Some(input), limit, what).await
}

/// Spawn `cmd` in its own process group and wait for it within `limit`,
/// feeding it `input` when given (stdin is null otherwise).
async fn run_inner(
    mut cmd: tokio::process::Command,
    input: Option<&[u8]>,
    limit: Duration,
    what: &str,
) -> Result<Output, GitError> {
    use tokio::io::AsyncWriteExt;

    cmd.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = crate::process::spawn_retrying_exec_busy(|| cmd.spawn())
        .await
        .map_err(|e| {
            GitError::new(GitErrorCode::Failed, format!("{what}: could not start"))
                .with_detail(&e.to_string())
        })?;
    let group = GroupKill::new(child.id());
    let stdin = child.stdin.take();

    // Stdin is fed while the output is read: a child that answers as it
    // reads would otherwise fill its output pipe and wait on us forever.
    let feed = async move {
        if let (Some(mut stdin), Some(input)) = (stdin, input) {
            // A child that exits early closes the pipe; its status says why.
            let _ = stdin.write_all(input).await;
        }
    };
    let finished = tokio::time::timeout(limit, async move {
        let ((), output) = tokio::join!(feed, child.wait_with_output());
        output
    })
    .await;
    match finished {
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
pub(super) struct GroupKill {
    #[cfg_attr(not(unix), allow(dead_code))]
    pgid: Option<u32>,
}

impl GroupKill {
    pub(super) fn new(pid: Option<u32>) -> Self {
        Self { pgid: pid }
    }

    pub(super) fn disarm(mut self) {
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

/// The `gh` to run: `configured`, else the one shipped next to the Host
/// executable, else the user's installed one (`PATH`, then the usual Homebrew
/// locations, which a GUI-launched Host's `PATH` lacks).
pub(super) fn find_gh(configured: Option<&Path>) -> Result<std::path::PathBuf, GitError> {
    let exe = crate::update::runtime::self_exe();
    find_gh_from(configured, exe.parent())
}

fn find_gh_from(
    configured: Option<&Path>,
    host_dir: Option<&Path>,
) -> Result<std::path::PathBuf, GitError> {
    if let Some(path) = configured {
        return if path.is_file() {
            Ok(path.to_path_buf())
        } else {
            Err(gh_unavailable())
        };
    }
    if let Some(path) = host_dir.and_then(shipped_gh_in) {
        return Ok(path);
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

/// The `gh` shipped next to the Host executable, if it is there.
pub(super) fn shipped_gh() -> Option<std::path::PathBuf> {
    shipped_gh_in(crate::update::runtime::self_exe().parent()?)
}

fn shipped_gh_in(host_dir: &Path) -> Option<std::path::PathBuf> {
    Some(host_dir.join(crate::update::install::GH_BIN)).filter(|p| is_executable(p))
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

fn gh_unavailable() -> GitError {
    GitError::new(
        GitErrorCode::GhUnavailable,
        "The GitHub CLI (gh) is not installed. Install it from https://cli.github.com and run `gh auth login`.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_exe(path: &Path, mode: u32) {
        std::fs::write(path, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        #[cfg(not(unix))]
        let _ = mode;
    }

    #[test]
    fn prefers_the_gh_shipped_next_to_the_host() {
        let dir = tempfile::tempdir().unwrap();
        let shipped = dir.path().join(crate::update::install::GH_BIN);
        write_exe(&shipped, 0o755);
        assert_eq!(find_gh_from(None, Some(dir.path())).unwrap(), shipped);
    }

    #[test]
    fn configured_gh_wins_over_the_shipped_one() {
        let dir = tempfile::tempdir().unwrap();
        write_exe(&dir.path().join(crate::update::install::GH_BIN), 0o755);
        let configured = dir.path().join("my-gh");
        write_exe(&configured, 0o755);
        assert_eq!(
            find_gh_from(Some(&configured), Some(dir.path())).unwrap(),
            configured
        );
        let missing = dir.path().join("absent");
        assert_eq!(
            find_gh_from(Some(&missing), Some(dir.path()))
                .unwrap_err()
                .code,
            GitErrorCode::GhUnavailable
        );
    }

    #[cfg(unix)]
    #[test]
    fn skips_a_shipped_gh_that_is_not_executable() {
        let dir = tempfile::tempdir().unwrap();
        let shipped = dir.path().join(crate::update::install::GH_BIN);
        write_exe(&shipped, 0o644);
        // Falls through to the user's install, whatever that is here.
        assert_ne!(find_gh_from(None, Some(dir.path())).ok(), Some(shipped));
    }
}
