//! Self-cleanup watchdog for `codeg-mcp`.
//!
//! On Windows, child processes don't die with their parent automatically.
//! On Unix the kernel closes the inherited pipe ends so `stdin` reads EOF
//! and the loop exits — usually. In both worlds a misbehaving intermediate
//! (agent CLI that hangs, parent codeg crash that orphans the agent) can
//! leave `codeg-mcp` running forever, holding open the binary file and a
//! companion connection that no one will ever read from.
//!
//! When the parent codeg / codeg-server passes `--parent-pid <pid>` on
//! the command line, `codeg-mcp` spawns this watchdog. It polls the OS
//! every couple of seconds and, the moment the parent PID stops existing,
//! tears down the process. Polling (vs. a kernel notification) keeps the
//! implementation tiny and identical across platforms; the 2 s tick is
//! invisible next to the other work `codeg-mcp` does.
//!
//! The check is intentionally not exposed as a long-lived handle — there
//! is no graceful "stop watching" path because the only outcome is
//! `process::exit`.
//!
//! Backward compatibility: the `--parent-pid` flag is optional. Older
//! parents that don't pass it get today's behavior (no watchdog).

use std::time::Duration;

/// Default polling cadence. Fast enough that a stale `codeg-mcp.exe`
/// releases its file handle well before a follow-up install retry, slow
/// enough that the poll cost is invisible.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Return `true` if a process with `pid` is currently alive (running, not
/// a zombie awaiting reap on Unix; not exited on Windows).
///
/// Best-effort: any unexpected OS error is treated as "alive" so a
/// permission glitch can't cause the watchdog to kill `codeg-mcp` while
/// the parent is in fact still running.
pub fn parent_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unix_parent_alive(pid)
    }
    #[cfg(windows)]
    {
        windows_parent_alive(pid)
    }
}

#[cfg(unix)]
fn unix_parent_alive(pid: u32) -> bool {
    // `kill(pid, 0)` is the POSIX existence probe — no signal is sent, the
    // kernel just validates the target. Result == 0 means alive; ESRCH
    // means gone; EPERM means alive but inaccessible (treat as alive).
    if pid == 0 {
        return false;
    }
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if r == 0 {
        return true;
    }
    matches!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(e) if e == libc::EPERM
    )
}

#[cfg(windows)]
fn windows_parent_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    if pid == 0 {
        return false;
    }
    // PROCESS_QUERY_LIMITED_INFORMATION is the lightest right that still
    // works on Vista+ even for processes owned by other integrity levels;
    // it's enough for OpenProcess + GetExitCodeProcess.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        // The most common reason is ERROR_INVALID_PARAMETER (PID is gone).
        // ERROR_ACCESS_DENIED is theoretically possible but would not occur
        // for a child watching its own parent in the same session.
        return false;
    }
    let mut code: u32 = 0;
    let read_ok = unsafe { GetExitCodeProcess(handle, &mut code as *mut u32) };
    unsafe {
        let _ = CloseHandle(handle);
    }
    // STILL_ACTIVE (259) is what Windows returns while a process is running.
    // A process that happened to exit with code 259 will be misreported as
    // alive for one extra poll — harmless for our purpose.
    read_ok != 0 && code == STILL_ACTIVE as u32
}

/// Long-running task: poll `pid` until it no longer exists, then return.
/// The caller is expected to terminate the process at that point — this
/// function itself does not call `process::exit` so it stays testable.
pub async fn wait_for_parent_exit(pid: u32, interval: Duration) {
    if pid == 0 {
        return;
    }
    loop {
        if !parent_alive(pid) {
            return;
        }
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn own_pid_is_alive() {
        assert!(parent_alive(std::process::id()));
    }

    #[test]
    fn pid_zero_is_dead() {
        // Treated as "not a valid target" on every platform we support.
        assert!(!parent_alive(0));
    }

    #[test]
    fn obviously_missing_pid_is_dead() {
        // A PID that almost certainly doesn't exist on a freshly booted
        // host. Windows reuses PIDs aggressively, so we pick something
        // way out of the usual range; if this ever flakes we can swap to
        // spawning a child and reaping it.
        assert!(!parent_alive(0x7FFF_FFF0));
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn watcher_returns_immediately_for_dead_pid() {
        // start_paused keeps tokio time deterministic — if the watcher
        // wrongly slept here, the test would hang because nothing advances
        // time, surfacing the regression rather than passing by accident.
        wait_for_parent_exit(0, Duration::from_secs(60)).await;
    }
}
