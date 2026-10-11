//! Cloning a GitHub repository through the user's own `gh`, reporting Git's
//! progress as it goes. No overall time limit (a big repository takes long);
//! instead the clone fails when Git prints nothing for `stall`. Cancelling or
//! failing kills the whole process group, so no `git` outlives the call.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use super::error::{classify_gh, GitError, GitErrorCode};
use super::run::{self, GroupKill};

/// Which part of the clone Git is reporting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClonePhase {
    Receiving,
    Resolving,
    Checkout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneRun {
    Done,
    Cancelled,
}

/// Bytes of non-progress stderr kept for classifying a failure.
const TAIL: usize = 4096;
/// Longest stderr line kept; Git's progress lines are short.
const MAX_LINE: usize = 8192;
/// How long a killed clone may take to be reaped.
const REAP: Duration = Duration::from_secs(10);

enum Stop {
    Cancelled,
    Stalled,
    Read(String),
}

/// `gh repo clone <owner>/<name> <dest> -- --progress` in `cwd`.
#[allow(clippy::too_many_arguments)]
pub async fn github_repo_clone(
    gh: Option<&Path>,
    cwd: &str,
    owner: &str,
    name: &str,
    dest: &Path,
    stall: Duration,
    mut on_progress: impl FnMut(ClonePhase, u8),
    cancel: &CancellationToken,
) -> Result<CloneRun, GitError> {
    let program = run::find_gh(gh)?;
    let mut cmd = run::gh(&program, cwd);
    cmd.args(["repo", "clone", &format!("{owner}/{name}")])
        .arg(dest)
        .args(["--", "--progress"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = crate::process::spawn_retrying_exec_busy(|| cmd.spawn())
        .await
        .map_err(|e| {
            GitError::new(GitErrorCode::Failed, "gh repo clone: could not start")
                .with_detail(&e.to_string())
        })?;
    let group = GroupKill::new(child.id());
    let mut stderr = child.stderr.take().expect("stderr is piped");

    let mut buf = [0u8; 8192];
    let mut line = Vec::new();
    let mut tail = String::new();
    let stop = loop {
        let read = tokio::select! {
            biased;
            _ = cancel.cancelled() => break Some(Stop::Cancelled),
            r = tokio::time::timeout(stall, stderr.read(&mut buf)) => r,
        };
        match read {
            Err(_) => break Some(Stop::Stalled),
            Ok(Err(e)) => break Some(Stop::Read(e.to_string())),
            Ok(Ok(0)) => break None,
            Ok(Ok(n)) => {
                for &b in &buf[..n] {
                    if b == b'\r' || b == b'\n' {
                        take_line(&mut line, &mut tail, &mut on_progress);
                    } else if line.len() < MAX_LINE {
                        line.push(b);
                    }
                }
            }
        }
    };
    take_line(&mut line, &mut tail, &mut on_progress);

    if let Some(stop) = stop {
        drop(group); // kills the group
        let _ = tokio::time::timeout(REAP, child.wait()).await;
        return match stop {
            Stop::Cancelled => Ok(CloneRun::Cancelled),
            Stop::Stalled => Err(GitError::new(
                GitErrorCode::Timeout,
                format!("gh repo clone: no progress for {}s", stall.as_secs_f32()),
            )),
            Stop::Read(e) => Err(GitError::new(
                GitErrorCode::Failed,
                "gh repo clone: lost its output",
            )
            .with_detail(&e)),
        };
    }
    // Stderr closed: the clone is exiting.
    let status = match tokio::time::timeout(stall, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => {
            return Err(GitError::new(GitErrorCode::Failed, "gh repo clone: failed")
                .with_detail(&e.to_string()))
        }
        Err(_) => {
            drop(group);
            let _ = tokio::time::timeout(REAP, child.wait()).await;
            return Err(GitError::new(
                GitErrorCode::Timeout,
                "gh repo clone: did not exit",
            ));
        }
    };
    group.disarm();
    if status.success() {
        Ok(CloneRun::Done)
    } else {
        Err(classify_clone(status.code(), &tail))
    }
}

/// A finished stderr line: progress goes to `on_progress`, anything else to
/// the bounded `tail` used to classify a failure.
fn take_line(line: &mut Vec<u8>, tail: &mut String, on_progress: &mut impl FnMut(ClonePhase, u8)) {
    if line.is_empty() {
        return;
    }
    let text = String::from_utf8_lossy(line).into_owned();
    line.clear();
    if let Some((phase, percent)) = parse_progress(&text) {
        on_progress(phase, percent);
        return;
    }
    tail.push_str(text.trim());
    tail.push('\n');
    if tail.len() > TAIL {
        let mut cut = tail.len() - TAIL;
        while !tail.is_char_boundary(cut) {
            cut += 1;
        }
        tail.drain(..cut);
    }
}

/// `Receiving objects:  42% (…)` and its siblings, as `(phase, percent)`.
fn parse_progress(line: &str) -> Option<(ClonePhase, u8)> {
    let line = line.trim();
    let (phase, rest) = [
        ("Receiving objects:", ClonePhase::Receiving),
        ("Resolving deltas:", ClonePhase::Resolving),
        ("Updating files:", ClonePhase::Checkout),
    ]
    .into_iter()
    .find_map(|(prefix, phase)| line.strip_prefix(prefix).map(|rest| (phase, rest)))?;
    let percent: u8 = rest.trim_start().split('%').next()?.trim().parse().ok()?;
    (percent <= 100).then_some((phase, percent))
}

/// Git's own clone refusals first; then whatever `gh` said.
fn classify_clone(exit: Option<i32>, tail: &str) -> GitError {
    let lower = tail.to_lowercase();
    if lower.contains("repository not found") || lower.contains("could not resolve to a repository")
    {
        GitError::new(
            GitErrorCode::NotFound,
            "gh repo clone: repository not found",
        )
        .with_detail(tail)
    } else if [
        "permission denied (publickey",
        "host key verification failed",
        "authentication failed",
        "could not read username",
    ]
    .iter()
    .any(|s| lower.contains(s))
    {
        GitError::new(
            GitErrorCode::AuthFailed,
            "gh repo clone: authentication failed",
        )
        .with_detail(tail)
    } else {
        classify_gh("repo clone", exit, tail)
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::git_harness::fake_gh;

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
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A bare repository with one commit on `main`.
    fn bare(dir: &Path) -> PathBuf {
        let src = dir.join("src");
        std::fs::create_dir(&src).unwrap();
        git(&src, &["init", "-q", "-b", "main"]);
        std::fs::write(src.join("README"), "hi\n").unwrap();
        git(&src, &["add", "."]);
        git(&src, &["commit", "-q", "-m", "base"]);
        let bare = dir.join("remote.git");
        git(
            dir,
            &[
                "clone",
                "-q",
                "--bare",
                src.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        bare
    }

    fn gone(pid: i32) -> bool {
        // SAFETY: signal 0 only probes for existence.
        unsafe { libc::kill(pid, 0) != 0 }
    }

    async fn wait_gone(pidfile: &Path) {
        let pid: i32 = std::fs::read_to_string(pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !gone(pid) {
            assert!(Instant::now() < deadline, "pid {pid} outlived the clone");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn arms(dir: &Path, bare: &Path) -> String {
        let pidfile = dir.join("pid");
        format!(
            r#""repo clone octo/app"*) git clone -q 'file://{bare}' "$4";;
"repo clone octo/progress"*) printf 'Receiving objects:  50%% (1/2)\rReceiving objects: 100%% (2/2), done.\nResolving deltas: 100%% (1/1), done.\nUpdating files:  33%% (1/3)\r' >&2; mkdir -p "$4";;
"repo clone octo/slow"*) sleep 30 & echo $! > '{pid}'; wait;;
"repo clone octo/gone"*) echo "GraphQL: Could not resolve to a Repository with the name 'octo/gone'. (repository)" >&2; exit 1;;
"repo clone octo/denied"*) echo "git@github.com: Permission denied (publickey)." >&2; exit 128;;
"repo clone octo/ssh"*) printf '%s' "$GIT_SSH_COMMAND" > '{ssh}'; mkdir -p "$4";;"#,
            bare = bare.display(),
            pid = pidfile.display(),
            ssh = dir.join("ssh").display(),
        )
    }

    /// A no-op run of the fake `gh`, so its first real exec — on this
    /// sandbox, macOS pays a one-time Gatekeeper/provenance check on a
    /// freshly written, freshly chmod +x'd file (measured 190-680ms here,
    /// 0ms on repeat execs of the same file) — happens before the timed
    /// scenario, not during it.
    fn warm(gh: &Path) {
        let _ = std::process::Command::new(gh).arg("warm-up").output();
    }

    async fn clone(
        dir: &Path,
        name: &str,
        stall: Duration,
        cancel: &CancellationToken,
    ) -> (Result<CloneRun, GitError>, Vec<(ClonePhase, u8)>, PathBuf) {
        let bare = bare(dir);
        let (gh, _) = fake_gh(dir, &arms(dir, &bare));
        warm(&gh);
        let dest = dir.join("dest");
        let mut seen = Vec::new();
        let r = github_repo_clone(
            Some(&gh),
            dir.to_str().unwrap(),
            "octo",
            name,
            &dest,
            stall,
            |phase, pct| seen.push((phase, pct)),
            cancel,
        )
        .await;
        (r, seen, dest)
    }

    #[tokio::test]
    async fn clones_into_dest() {
        let dir = tempfile::tempdir().unwrap();
        let (r, _, dest) = clone(
            dir.path(),
            "app",
            Duration::from_secs(30),
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(r.unwrap(), CloneRun::Done);
        assert!(dest.join("README").is_file());
    }

    #[tokio::test]
    async fn reports_progress_per_phase() {
        let dir = tempfile::tempdir().unwrap();
        let (r, seen, _) = clone(
            dir.path(),
            "progress",
            Duration::from_secs(30),
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(r.unwrap(), CloneRun::Done);
        assert_eq!(
            seen,
            vec![
                (ClonePhase::Receiving, 50),
                (ClonePhase::Receiving, 100),
                (ClonePhase::Resolving, 100),
                (ClonePhase::Checkout, 33),
            ]
        );
    }

    #[test]
    fn parse_progress_ignores_other_lines() {
        assert_eq!(parse_progress("remote: Counting objects: 10% (1/10)"), None);
        assert_eq!(parse_progress("Receiving objects: 101% (1/1)"), None);
        assert_eq!(parse_progress("Cloning into 'x'..."), None);
        assert_eq!(
            parse_progress("  Resolving deltas:   7% (7/100)"),
            Some((ClonePhase::Resolving, 7))
        );
    }

    #[tokio::test]
    async fn a_stalled_clone_times_out_and_leaves_no_process() {
        let dir = tempfile::tempdir().unwrap();
        let (r, _, _) = clone(
            dir.path(),
            "slow",
            Duration::from_millis(300),
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(r.unwrap_err().code, GitErrorCode::Timeout);
        wait_gone(&dir.path().join("pid")).await;
    }

    #[tokio::test]
    async fn cancelling_kills_the_whole_group() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            trigger.cancel();
        });
        let started = Instant::now();
        let (r, _, _) = clone(dir.path(), "slow", Duration::from_secs(30), &cancel).await;
        assert_eq!(r.unwrap(), CloneRun::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(10));
        wait_gone(&dir.path().join("pid")).await;
    }

    #[tokio::test]
    async fn classifies_missing_and_refused() {
        let dir = tempfile::tempdir().unwrap();
        let never = CancellationToken::new();
        let (r, _, _) = clone(dir.path(), "gone", Duration::from_secs(30), &never).await;
        assert_eq!(r.unwrap_err().code, GitErrorCode::NotFound);
        let dir = tempfile::tempdir().unwrap();
        let (r, _, _) = clone(dir.path(), "denied", Duration::from_secs(30), &never).await;
        assert_eq!(r.unwrap_err().code, GitErrorCode::AuthFailed);
    }

    #[tokio::test]
    async fn ssh_never_prompts_unless_the_user_set_its_command() {
        let dir = tempfile::tempdir().unwrap();
        let never = CancellationToken::new();
        let (r, _, _) = clone(dir.path(), "ssh", Duration::from_secs(30), &never).await;
        assert_eq!(r.unwrap(), CloneRun::Done);
        let expected =
            std::env::var("GIT_SSH_COMMAND").unwrap_or_else(|_| "ssh -o BatchMode=yes".to_string());
        let seen = std::fs::read_to_string(dir.path().join("ssh")).unwrap();
        assert_eq!(seen, expected);
    }

    #[tokio::test]
    async fn without_gh_is_gh_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let r = github_repo_clone(
            Some(&dir.path().join("no-gh")),
            dir.path().to_str().unwrap(),
            "octo",
            "app",
            &dir.path().join("dest"),
            Duration::from_secs(1),
            |_, _| {},
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(r.unwrap_err().code, GitErrorCode::GhUnavailable);
    }
}
