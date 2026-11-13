//! Starting the engine (design 5.3). It reads CDP on fd 3 and writes on fd 4
//! (`--remote-debugging-pipe`), so it opens no TCP port and nothing else on
//! the machine can attach to it.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::net::unix::pipe;
use tokio::process::Child;

use super::cdp::Cdp;

/// How to start the engine.
#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub binary: PathBuf,
    pub profile_dir: PathBuf,
    /// `--renderer-process-limit`, from the memory budget (design 11.2).
    pub renderer_limit: Option<u32>,
    /// The engine's stderr goes here, replaced on each start.
    pub log_file: Option<PathBuf>,
}

/// A running engine: its process and its CDP connection.
pub struct Engine {
    pub child: Child,
    pub cdp: Cdp,
}

pub fn args(options: &LaunchOptions) -> Vec<String> {
    let mut args = vec![
        "--remote-debugging-pipe".to_string(),
        format!("--user-data-dir={}", options.profile_dir.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--disable-background-networking".into(),
        "--disable-component-update".into(),
        "--disable-blink-features=AutomationControlled".into(),
        // Metal through ANGLE; without it the shell renders in SwiftShader.
        "--enable-gpu".into(),
    ];
    if let Some(limit) = options.renderer_limit {
        args.push(format!("--renderer-process-limit={limit}"));
    }
    args.push("about:blank".into());
    args
}

/// Spawn the engine and connect to its pipe. The process is in its own
/// group and is killed when the returned [`Engine`] is dropped.
pub fn spawn(options: &LaunchOptions) -> io::Result<Engine> {
    std::fs::create_dir_all(&options.profile_dir)?;
    // Host → engine, and engine → Host.
    let (to_engine_r, to_engine_w) = cloexec_pipe()?;
    let (from_engine_r, from_engine_w) = cloexec_pipe()?;

    let stderr = match &options.log_file {
        Some(path) => Stdio::from(std::fs::File::create(path)?),
        None => Stdio::null(),
    };
    let mut cmd = crate::process::tokio_command(&options.binary);
    cmd.args(args(options))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .kill_on_drop(true)
        .process_group(0);

    let child_read = to_engine_r.as_raw_fd();
    let child_write = from_engine_w.as_raw_fd();
    // SAFETY: runs between fork and exec, and only calls the
    // async-signal-safe `fcntl` and `dup2`.
    unsafe {
        cmd.pre_exec(move || place_pipe_fds(child_read, child_write));
    }
    let child = cmd.spawn()?;
    // The child holds its own copies now.
    drop(to_engine_r);
    drop(from_engine_w);

    let reader = pipe::Receiver::from_owned_fd(from_engine_r)?;
    let writer = pipe::Sender::from_owned_fd(to_engine_w)?;
    Ok(Engine {
        child,
        cdp: Cdp::new(reader, writer),
    })
}

/// Put the child's ends at fd 3 and 4. Each is first copied above both
/// targets, so neither `dup2` overwrites the other end and the copies at 3
/// and 4 lose close-on-exec (`dup2` onto itself would keep it). The
/// temporary copies keep it, so they close at exec.
fn place_pipe_fds(read: RawFd, write: RawFd) -> io::Result<()> {
    // SAFETY: plain fd syscalls on fds this process owns.
    unsafe {
        let read = libc::fcntl(read, libc::F_DUPFD_CLOEXEC, 10);
        let write = libc::fcntl(write, libc::F_DUPFD_CLOEXEC, 10);
        if read < 0 || write < 0 || libc::dup2(read, 3) < 0 || libc::dup2(write, 4) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// A pipe whose ends are not inherited by other children the Host starts.
fn cloexec_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as RawFd; 2];
    // SAFETY: `fds` has room for the two fds `pipe` writes.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `pipe` just returned these fds and nothing else owns them.
    let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    for fd in [&r, &w] {
        // SAFETY: setting a flag on an fd we own.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((r, w))
}

/// The engine's profile, beside the installed engines.
pub fn profile_dir(engine_root: &Path) -> PathBuf {
    engine_root.parent().unwrap_or(engine_root).join("profile")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_match_the_design() {
        let options = LaunchOptions {
            binary: "/e/chrome-headless-shell".into(),
            profile_dir: "/data/browser/profile".into(),
            renderer_limit: Some(2),
            log_file: None,
        };
        let args = args(&options);
        assert_eq!(args[0], "--remote-debugging-pipe");
        assert!(args.contains(&"--user-data-dir=/data/browser/profile".to_string()));
        assert!(args.contains(&"--enable-gpu".to_string()));
        assert!(args.contains(&"--renderer-process-limit=2".to_string()));
        assert!(!args
            .iter()
            .any(|a| a.starts_with("--remote-debugging-port")));
        assert_eq!(args.last().unwrap(), "about:blank");
    }

    /// Starts the real engine. Point `ZWIIM_TEST_ENGINE` at a
    /// `chrome-headless-shell` binary to run it.
    #[tokio::test]
    #[ignore = "needs ZWIIM_TEST_ENGINE"]
    async fn the_real_engine_answers_over_the_pipe() {
        let binary = std::env::var_os("ZWIIM_TEST_ENGINE").expect("ZWIIM_TEST_ENGINE");
        let dir = tempfile::tempdir().unwrap();
        let mut engine = spawn(&LaunchOptions {
            binary: binary.into(),
            profile_dir: dir.path().join("profile"),
            renderer_limit: Some(2),
            log_file: Some(dir.path().join("engine.log")),
        })
        .unwrap();
        let version = engine
            .cdp
            .call("Browser.getVersion", serde_json::json!({}), None)
            .await
            .unwrap();
        assert!(version["product"].as_str().unwrap().contains("Chrome"));

        engine
            .cdp
            .call("Browser.close", serde_json::json!({}), None)
            .await
            .unwrap();
        let status = tokio::time::timeout(std::time::Duration::from_secs(10), engine.child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(status.success(), "{status}");
        engine.cdp.closed().await;
    }
}
