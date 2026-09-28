//! `zwiim-host --supervise` — a minimal process supervisor that owns the
//! lifecycle of the real worker, so an update can switch versions and have
//! the *new* version relaunched deterministically.
//!
//! It also behaves like an init (PID 1 in Docker): it forwards
//! `SIGTERM`/`SIGINT` to the worker for a graceful stop, and reaps
//! reparented orphan children so they don't pile up as zombies.
//!
//! Which binary runs: `versions/<current>/zwiim-host` from the Zwiim
//! installer (`update::install`), read again before every spawn; this
//! executable only when nothing is installed. On start the supervisor first
//! offers the Host it was shipped with to the installer (`install::seed`):
//! the first install runs it, a newer one is staged without a download.
//!
//! Relaunch contract: the worker exits with
//! [`crate::update::runtime::EXIT_RESTART`] (after switching `current` when
//! it applies an update). We wait `ZWIIM_RESTART_DELAY_MS` and respawn.
//!
//! Failed-update self-heal: a version whose trial is still open is on
//! probation. If it exits before committing its trial, or does not commit
//! within its deadline (it is then killed), the installer rolls it back
//! (database snapshot, previous version, blocked) and the previous version is
//! relaunched — once. Any other exit is propagated as-is — a fatal config
//! error or a crash after the trial should stop the process rather than
//! hot-loop.

use crate::update::runtime;

/// Build the worker argv: our own args minus the `--supervise` flag.
fn worker_args() -> Vec<String> {
    std::env::args()
        .skip(1)
        .filter(|a| a != "--supervise")
        .collect()
}

/// Roll back an unproven version. Returns `true` if a rollback was
/// performed (caller should relaunch the restored, previous version).
fn attempt_rollback(reason: &str) -> bool {
    tracing::info!("[supervise] {reason}; rolling back to the previous version");
    match crate::update::install::rollback_current(reason) {
        Ok(()) => {
            tracing::info!("[supervise] rollback complete; relaunching previous version");
            true
        }
        Err(e) => {
            tracing::error!("[supervise] rollback failed: {e}; propagating original exit");
            false
        }
    }
}

/// The binary to start now: the installed current version, else this one.
fn worker_exe() -> std::path::PathBuf {
    if !crate::update::install::versioned_installs() {
        return runtime::self_exe();
    }
    std::env::var_os("ZWIIM_DATA_DIR")
        .map(std::path::PathBuf::from)
        .and_then(|d| crate::update::install::worker_exe(&crate::update::install::root(&d)))
        .unwrap_or_else(runtime::self_exe)
}

/// Offer the Host this supervisor was shipped with to the installer.
fn seed_bundled() {
    if !crate::update::install::versioned_installs() {
        return;
    }
    let Some(data_dir) = std::env::var_os("ZWIIM_DATA_DIR").map(std::path::PathBuf::from) else {
        return;
    };
    let root = crate::update::install::root(&data_dir);
    match crate::update::install::seed(&root, &runtime::self_exe(), env!("CARGO_PKG_VERSION")) {
        Ok(crate::update::install::Seeded::Installed(v)) => {
            tracing::info!("[supervise] installed Host {v}")
        }
        Ok(crate::update::install::Seeded::Staged(v)) => {
            tracing::info!("[supervise] Host {v} from this app is staged; it applies on the next update restart")
        }
        Ok(crate::update::install::Seeded::Unchanged) => {}
        Err(e) => tracing::warn!("[supervise] could not install the bundled Host: {e}"),
    }
}

/// How long a version on trial may take to commit before it is killed and
/// rolled back: its trial window plus time to start.
fn trial_deadline() -> std::time::Duration {
    std::time::Duration::from_secs(runtime::upgrade_trial_secs() + TRIAL_START_GRACE_SECS)
}

const TRIAL_START_GRACE_SECS: u64 = 90;

#[cfg(unix)]
pub fn run() -> ! {
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    static WORKER_PID: AtomicI32 = AtomicI32::new(0);
    static TERMINATING: AtomicBool = AtomicBool::new(false);

    extern "C" fn forward_signal(sig: libc::c_int) {
        TERMINATING.store(true, Ordering::SeqCst);
        let pid = WORKER_PID.load(Ordering::SeqCst);
        if pid > 0 {
            // SAFETY: `kill` is async-signal-safe.
            unsafe {
                libc::kill(pid, sig);
            }
        }
    }

    // SAFETY: installing handlers before spawning any child. `signal` is fine
    // for our forward-and-flag use; we deliberately keep the handler trivial.
    unsafe {
        libc::signal(
            libc::SIGTERM,
            forward_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            forward_signal as *const () as libc::sighandler_t,
        );
    }

    seed_bundled();
    let args = worker_args();
    let delay = runtime::restart_delay_ms();

    let spawn_worker = || -> std::io::Result<std::process::Child> {
        let exe = worker_exe();
        tracing::info!("[supervise] starting {}", exe.display());
        let bundled = crate::update::install::in_app_bundle(&runtime::self_exe());
        std::process::Command::new(&exe)
            .args(&args)
            .env(crate::update::install::ENV_BUNDLED, if bundled { "1" } else { "0" })
            .env(runtime::ENV_SUPERVISED, "1")
            .env(runtime::ENV_RESTART_DELAY_MS, delay.to_string())
            .spawn()
    };

    // Spawn + publish the PID, then close the spawn→store race: if a stop
    // signal landed in that window the handler couldn't forward it (the PID
    // was still 0), so forward it here.
    let spawn_and_track = || -> std::io::Result<std::process::Child> {
        let child = spawn_worker()?;
        let pid = child.id() as i32;
        WORKER_PID.store(pid, Ordering::SeqCst);
        if crate::update::install::upgrade_staged() {
            // A version on trial that neither commits nor exits (hung) is
            // killed at its deadline; the exit then rolls it back.
            std::thread::spawn(move || {
                std::thread::sleep(trial_deadline());
                if WORKER_PID.load(Ordering::SeqCst) == pid && crate::update::install::upgrade_staged() {
                    tracing::error!("[supervise] new version did not prove itself in time; stopping it");
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
            });
        }
        if TERMINATING.load(Ordering::SeqCst) {
            // SAFETY: `kill` is async-signal-safe; `pid` is our just-spawned child.
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
        Ok(child)
    };

    // Spawn a launch that may be on trial. If spawning a freshly-upgraded
    // binary fails outright — wrong architecture, ENOEXEC, missing dynamic
    // loader, truncated file — the new version can't boot at all, which is
    // exactly the failure the trial guards against; the run-then-die path
    // never sees it because there is no process to wait on. Roll back and
    // spawn the restored previous version instead. The marker was already
    // consumed, so without this a container restart would re-run the same
    // unspawnable binary forever. Exits only if even the fallback spawn fails.
    let spawn_trial = |on_trial: &mut bool| -> std::process::Child {
        match spawn_and_track() {
            Ok(child) => child,
            Err(e)
                if *on_trial && attempt_rollback(&format!("new version failed to spawn ({e})")) =>
            {
                *on_trial = false;
                spawn_and_track().unwrap_or_else(|e2| {
                    tracing::error!("[supervise][FATAL] failed to spawn restored worker: {e2}");
                    std::process::exit(1);
                })
            }
            Err(e) => {
                tracing::error!("[supervise][FATAL] failed to spawn worker: {e}");
                std::process::exit(1);
            }
        }
    };

    // A worker is "on trial" when this launch is the trial of a freshly
    // swapped version — i.e. a staged-upgrade marker is present at launch. A
    // plain restart (no pending upgrade) has no marker and is not on trial, so
    // it is never auto-rolled-back. The initial launch can also be a trial if a
    // swap completed in a prior lifetime and the container was restarted before
    // the upgrade-restart fired.
    //
    // We only *peek* (don't consume): the marker must stay on disk for the
    // whole trial window so a second `perform_update` on the trial worker is
    // refused — consuming it here would let that perform clobber `.bak` with
    // the still-unproven version, so a trial-failure rollback would restore the
    // bad version. The worker itself clears the marker once it survives the
    // trial window (proving the upgrade); `rollback` clears it on revert.
    let mut on_trial = crate::update::install::upgrade_staged();
    // Stamp the trial start *before* spawning. The worker clears the marker at
    // (its own start + trial); the supervisor rolls back on an abnormal exit
    // within (spawned_at + trial). If `spawned_at` were taken after the spawn
    // and the parent were preempted past the child's startup, the rollback
    // window could outlast the marker and a second perform could clobber `.bak`
    // in the gap. Stamping first guarantees spawned_at <= child start, so the
    // marker always covers the whole window. (The windows path does the same.)
    let mut spawned_at = std::time::Instant::now();
    let mut child = spawn_trial(&mut on_trial);
    tracing::info!("[supervise] worker started (pid {})", child.id());

    loop {
        let worker_pid = WORKER_PID.load(Ordering::SeqCst);
        let mut status: libc::c_int = 0;
        // Block until *any* child changes state. As PID 1 we reap every
        // reparented orphan here; only the worker's exit drives decisions.
        let pid = unsafe { libc::waitpid(-1, &mut status, 0) };

        if pid == -1 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if errno == libc::EINTR {
                continue; // interrupted by a forwarded signal; retry
            }
            if errno == libc::ECHILD {
                // No children left — nothing more to supervise.
                std::process::exit(if TERMINATING.load(Ordering::SeqCst) {
                    0
                } else {
                    1
                });
            }
            tracing::error!("[supervise][FATAL] waitpid failed (errno {errno})");
            std::process::exit(1);
        }

        if pid != worker_pid {
            // Reaped an orphan zombie; ignore and keep going.
            continue;
        }

        // The worker exited. Decode how.
        let _ = child.try_wait(); // let std reap its own bookkeeping (no-op)
                                  // Clear the pid immediately: during the relaunch delay below a
                                  // forwarded signal must not `kill()` a PID the OS may have recycled.
        WORKER_PID.store(0, Ordering::SeqCst);
        if TERMINATING.load(Ordering::SeqCst) {
            tracing::info!("[supervise] worker stopped during shutdown; exiting");
            std::process::exit(0);
        }

        let elapsed = spawned_at.elapsed();

        if libc::WIFEXITED(status) {
            let code = libc::WEXITSTATUS(status);
            if code == runtime::EXIT_RESTART {
                tracing::info!("[supervise] upgrade restart requested; relaunching in {delay}ms");
                std::thread::sleep(std::time::Duration::from_millis(delay));
                if TERMINATING.load(Ordering::SeqCst) {
                    std::process::exit(0);
                }
                // On trial only if an upgrade is staged (marker present); a
                // plain restart leaves no marker and relaunches without
                // probation. Peek, don't consume — a restart that lands inside
                // an in-flight trial keeps the marker, so probation correctly
                // carries across it (and the second-perform guard stays armed).
                on_trial = crate::update::install::upgrade_staged();
                // Stamp before spawning (see the initial launch) so the marker
                // always outlives the supervisor's rollback window.
                spawned_at = std::time::Instant::now();
                child = spawn_trial(&mut on_trial);
                tracing::info!("[supervise] worker relaunched (pid {})", child.id());
                continue;
            }
            // A freshly-upgraded worker that dies fast can't boot — restore
            // the previous version and relaunch it (once; the restored worker
            // is no longer on_trial, so a second failure propagates).
            // Another Host owns the data directory: not a broken upgrade,
            // so never a reason to roll back.
            let already_running =
                code == i32::from(crate::host_lifecycle::EXIT_ALREADY_RUNNING);
            if on_trial
                && !already_running
                && crate::update::install::upgrade_staged()
                && attempt_rollback(&format!(
                    "new version exited (code {code}) after {:.1}s",
                    elapsed.as_secs_f64()
                ))
            {
                on_trial = false;
                child = spawn_and_track().unwrap_or_else(|e| {
                    tracing::error!("[supervise][FATAL] failed to spawn restored worker: {e}");
                    std::process::exit(1);
                });
                spawned_at = std::time::Instant::now();
                tracing::info!(
                    "[supervise] previous version relaunched (pid {})",
                    child.id()
                );
                continue;
            }
            tracing::info!("[supervise] worker exited with code {code}; propagating");
            std::process::exit(code);
        }

        if libc::WIFSIGNALED(status) {
            let sig = libc::WTERMSIG(status);
            if on_trial
                && crate::update::install::upgrade_staged()
                && attempt_rollback(&format!(
                    "new version was killed by signal {sig} after {:.1}s",
                    elapsed.as_secs_f64()
                ))
            {
                on_trial = false;
                child = spawn_and_track().unwrap_or_else(|e| {
                    tracing::error!("[supervise][FATAL] failed to spawn restored worker: {e}");
                    std::process::exit(1);
                });
                spawned_at = std::time::Instant::now();
                tracing::info!(
                    "[supervise] previous version relaunched (pid {})",
                    child.id()
                );
                continue;
            }
            tracing::info!("[supervise] worker killed by signal {sig}; exiting");
            // Mirror the conventional 128+signal exit code.
            std::process::exit(128 + sig);
        }

        // Stopped/continued or otherwise non-terminal — keep waiting.
    }
}

#[cfg(windows)]
pub fn run() -> ! {
    // Windows has no PID 1 / zombie semantics. A straightforward
    // spawn → wait → relaunch loop is enough; standalone Windows servers
    // typically also sit under a service manager.
    let exe = runtime::self_exe();
    let args = worker_args();
    let delay = runtime::restart_delay_ms();
    let trial = std::time::Duration::from_secs(runtime::upgrade_trial_secs());
    // On trial only if a swap is staged at this launch (see the unix path for
    // the full rationale); a plain restart is never auto-rolled-back. Peek,
    // don't consume — the worker clears the marker once it survives the trial.
    let mut on_trial = crate::update::install::upgrade_staged();

    loop {
        let spawned_at = std::time::Instant::now();
        let bundled = crate::update::install::in_app_bundle(&runtime::self_exe());
        let mut child = std::process::Command::new(worker_exe())
            .args(&args)
            .env(crate::update::install::ENV_BUNDLED, if bundled { "1" } else { "0" })
            .env(runtime::ENV_SUPERVISED, "1")
            .env(runtime::ENV_RESTART_DELAY_MS, delay.to_string())
            .spawn()
            .unwrap_or_else(|e| {
                tracing::error!("[supervise][FATAL] failed to spawn worker: {e}");
                std::process::exit(1);
            });

        let status = child.wait().unwrap_or_else(|e| {
            tracing::error!("[supervise][FATAL] wait failed: {e}");
            std::process::exit(1);
        });
        let elapsed = spawned_at.elapsed();

        match status.code() {
            Some(code) if code == runtime::EXIT_RESTART => {
                tracing::info!("[supervise] upgrade restart requested; relaunching in {delay}ms");
                std::thread::sleep(std::time::Duration::from_millis(delay));
                // Probation for the next launch only if an upgrade is staged.
                // Peek, don't consume — see the unix path for the rationale.
                on_trial = crate::update::install::upgrade_staged();
            }
            Some(code) => {
                // A freshly-upgraded worker that dies fast can't boot —
                // restore the previous version and relaunch it once.
                if on_trial
                    && code != i32::from(crate::host_lifecycle::EXIT_ALREADY_RUNNING)
                    && crate::update::install::upgrade_staged()
                    && attempt_rollback(&format!(
                        "new version exited (code {code}) after {:.1}s",
                        elapsed.as_secs_f64()
                    ))
                {
                    on_trial = false;
                    tracing::info!("[supervise] relaunching previous version");
                    continue;
                }
                std::process::exit(code);
            }
            None => {
                if on_trial
                    && crate::update::install::upgrade_staged()
                    && attempt_rollback("new version terminated abnormally")
                {
                    on_trial = false;
                    tracing::info!("[supervise] relaunching previous version");
                    continue;
                }
                std::process::exit(1);
            }
        }
    }
}
