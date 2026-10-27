//! The Host's side of the power helper: a root daemon that keeps a closed
//! Mac awake while the Host holds a connection to it.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use super::backend::{HoldGuard, LidSupport};

/// Where the helper listens.
pub(crate) const SOCKET: &str = "/var/run/com.zwiim.power-helper.sock";

/// The helper is local; past this it is not answering.
const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Probe {
    /// Installed, running, and it takes this Host's calls.
    Ready,
    /// Running, but it refused or hung up.
    Refused,
    /// Nothing listening.
    Missing,
    /// Connected, but no answer in time: it may be busy setting sleep for
    /// another caller, so this says nothing new about support.
    Busy,
}

/// Ask the helper whether it takes our calls, without holding anything.
pub(crate) fn probe(path: &Path) -> Probe {
    let Ok(stream) = UnixStream::connect(path) else {
        return Probe::Missing;
    };
    match request(&stream, "release") {
        Ok(reply) if reply == "ok" => Probe::Ready,
        Err(e) if timed_out(&e) => Probe::Busy,
        _ => Probe::Refused,
    }
}

/// Take the lid hold; it lasts as long as the returned guard.
pub(crate) fn hold(path: &Path) -> Result<HelperHold, String> {
    let stream =
        UnixStream::connect(path).map_err(|e| format!("power helper not reachable: {e}"))?;
    match request(&stream, "hold") {
        Ok(reply) if reply == "ok" => Ok(HelperHold { stream }),
        Ok(reply) => Err(format!("power helper refused: {reply}")),
        Err(e) => Err(format!("power helper did not answer: {e}")),
    }
}

/// What the lid setting can offer on this Mac.
pub(crate) fn lid_support(has_lid: bool, signed: bool, probe: Probe) -> LidSupport {
    match (has_lid, probe) {
        (false, _) => LidSupport::NoLid,
        (true, Probe::Ready) => LidSupport::Available,
        // Only a signed build can register the helper; it would refuse an
        // unsigned Host anyway.
        (true, Probe::Missing) if signed => LidSupport::NeedsSetup,
        _ => LidSupport::Unavailable,
    }
}

/// One line out, one line back.
fn request(stream: &UnixStream, verb: &str) -> std::io::Result<String> {
    // macOS refuses socket options once the peer has hung up; that is a
    // refusal, not a missing helper.
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    (&*stream).write_all(format!("{verb}\n").as_bytes())?;
    let mut line = String::new();
    match BufReader::new(stream).read_line(&mut line)? {
        0 => Err(std::io::ErrorKind::UnexpectedEof.into()),
        _ => Ok(line.trim_end().to_string()),
    }
}

/// A socket timeout reads as `WouldBlock` on macOS.
fn timed_out(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// The open connection is the hold: the helper releases when it closes.
pub(crate) struct HelperHold {
    stream: UnixStream,
}

impl HoldGuard for HelperHold {
    /// Alive until the helper hangs up (it does when a guard rail trips).
    fn is_alive(&mut self) -> bool {
        let mut byte = 0u8;
        // SAFETY: a one-byte peek into a local buffer on a socket we own.
        let n = unsafe {
            libc::recv(
                self.stream.as_raw_fd(),
                (&mut byte as *mut u8).cast(),
                1,
                libc::MSG_PEEK | libc::MSG_DONTWAIT,
            )
        };
        match n {
            0 => false,
            n if n > 0 => true,
            _ => matches!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::WouldBlock
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::time::{Duration, Instant};

    use super::*;

    /// A helper that answers every line with `reply`, and hangs up after
    /// `lines` lines.
    fn helper(reply: &'static str, lines: usize) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("helper.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                for _ in 0..lines {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    stream.write_all(format!("{reply}\n").as_bytes()).unwrap();
                }
            }
        });
        (dir, path)
    }

    #[test]
    fn probe_ready_refused_missing() {
        let (_d, ok) = helper("ok", 1);
        assert_eq!(probe(&ok), Probe::Ready);
        let (_d, refused) = helper("error unauthorized", 1);
        assert_eq!(probe(&refused), Probe::Refused);
        let (_d, silent) = helper("ok", 0);
        assert_eq!(probe(&silent), Probe::Refused);
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(probe(&dir.path().join("none.sock")), Probe::Missing);
    }

    /// A helper busy setting sleep answers late: that is not a refusal.
    #[test]
    fn probe_unanswered_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("helper.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            std::thread::sleep(TIMEOUT * 2);
        });
        assert_eq!(probe(&path), Probe::Busy);
    }

    #[test]
    fn lid_support_table() {
        use LidSupport::*;
        for probe in [Probe::Ready, Probe::Refused, Probe::Missing, Probe::Busy] {
            assert_eq!(lid_support(false, true, probe), NoLid);
            assert_eq!(lid_support(false, false, probe), NoLid);
        }
        assert_eq!(lid_support(true, true, Probe::Ready), Available);
        assert_eq!(lid_support(true, false, Probe::Ready), Available);
        assert_eq!(lid_support(true, true, Probe::Missing), NeedsSetup);
        assert_eq!(lid_support(true, false, Probe::Missing), Unavailable);
        assert_eq!(lid_support(true, true, Probe::Refused), Unavailable);
        assert_eq!(lid_support(true, false, Probe::Refused), Unavailable);
        assert_eq!(lid_support(true, true, Probe::Busy), Unavailable);
    }

    #[test]
    fn hold_is_alive_until_helper_closes() {
        // Answers `hold`, then waits for a second line that never comes
        // before hanging up: alive for as long as the helper keeps it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("helper.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let (hang_up, hung_up) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            assert_eq!(line, "hold\n");
            stream.write_all(b"ok\n").unwrap();
            hung_up.recv().unwrap();
        });
        let mut guard = hold(&path).unwrap();
        assert!(guard.is_alive());
        std::thread::sleep(Duration::from_millis(100));
        assert!(guard.is_alive());
        hang_up.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while guard.is_alive() {
            assert!(Instant::now() < deadline, "still alive");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn hold_refused_is_an_error() {
        let (_d, path) = helper("error on_battery", 1);
        let err = hold(&path).err().unwrap();
        assert!(err.contains("on_battery"), "{err}");
        let dir = tempfile::tempdir().unwrap();
        assert!(hold(&dir.path().join("none.sock")).is_err());
    }
}
