//! `zwiim-power-helper`: a root launch daemon that keeps a Mac laptop awake
//! with the lid closed while the signed Zwiim Host holds a connection to it.
//!
//! It runs only `/usr/bin/pmset`, with fixed arguments, and puts sleep back
//! on start, when the last hold ends, on SIGTERM, and when a guard rail trips.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

#[cfg(target_os = "macos")]
mod auth;
mod core;
mod probe;
mod server;

#[cfg(target_os = "macos")]
fn main() {
    daemon::run();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("zwiim-power-helper: macOS only");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
mod daemon {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::time::Duration;

    use crate::core::{caller_requirement, Core, Pmset};
    use crate::{auth, probe, server::Server};

    /// Where the Host finds the helper. Root owns `/var/run`.
    const SOCKET: &str = "/var/run/com.zwiim.power-helper.sock";

    /// How often the guard rails are re-checked and a failed reset retried.
    const TICK: Duration = Duration::from_secs(30);

    /// The Host's signing team, compiled in. Without it no caller is accepted.
    const TEAM_ID: Option<&str> = option_env!("ZWIIM_APPLE_TEAM_ID");

    struct PmsetCommand;

    impl Pmset for PmsetCommand {
        fn set_disablesleep(&mut self, on: bool) -> bool {
            let value = if on { "1" } else { "0" };
            let applied = probe::pmset(&["-a", "disablesleep", value]).is_some();
            if !applied {
                eprintln!("zwiim-power-helper: setting disablesleep {value} failed");
            }
            applied
        }
    }

    pub fn run() {
        // Signals are taken by one thread with sigwait, so every thread
        // started below must inherit them blocked.
        let signals = block_signals();
        let requirement = TEAM_ID.and_then(caller_requirement);
        if requirement.is_none() {
            eprintln!("zwiim-power-helper: no signing team compiled in; refusing every caller");
        }
        let server = Server::new(
            Core::new(PmsetCommand),
            move |stream: &std::os::unix::net::UnixStream| {
                requirement
                    .as_deref()
                    .is_some_and(|r| auth::peer_is_host(stream, r))
            },
            probe::read_power,
        );

        let _ = std::fs::remove_file(SOCKET);
        let listener = match UnixListener::bind(SOCKET) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("zwiim-power-helper: cannot listen on {SOCKET}: {e}");
                std::process::exit(1);
            }
        };
        // The signature check is the gate, not the file mode.
        if let Err(e) = std::fs::set_permissions(SOCKET, std::fs::Permissions::from_mode(0o666)) {
            eprintln!("zwiim-power-helper: cannot open up {SOCKET}: {e}");
            std::process::exit(1);
        }

        let stopping = server.clone();
        std::thread::spawn(move || {
            let mut signal = 0;
            // SAFETY: `signals` is an initialised set; `signal` is writable.
            unsafe { libc::sigwait(&signals, &mut signal) };
            if !stopping.shutdown() {
                eprintln!(
                    "zwiim-power-helper: could not put sleep back; \
                     run `sudo pmset -a disablesleep 0`"
                );
            }
            let _ = std::fs::remove_file(SOCKET);
            std::process::exit(0);
        });

        let ticking = server.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(TICK);
            ticking.tick();
        });

        server.serve(listener);
    }

    fn block_signals() -> libc::sigset_t {
        // SAFETY: the set is initialised by sigemptyset before any use, and
        // pthread_sigmask only reads it.
        unsafe {
            let mut set = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGTERM);
            libc::sigaddset(&mut set, libc::SIGINT);
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
            set
        }
    }
}
