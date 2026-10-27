//! The socket: one thread per connection, a line protocol of exactly two
//! verbs, and holds that end with the connection.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::core::{Core, Pmset, Power};

/// `hold` and `release` fit many times over; anything longer is not ours.
const MAX_LINE: u64 = 64;

/// A refused caller gets this long to send its first line.
const REFUSE_TIMEOUT: Duration = Duration::from_secs(5);

type Authorize = Box<dyn Fn(&UnixStream) -> bool + Send + Sync>;
type ReadPower = Box<dyn Fn() -> Power + Send + Sync>;

pub struct Server<P: Pmset> {
    core: Mutex<Core<P>>,
    /// Live connections, so a guard rail can cut their holders off.
    conns: Mutex<HashMap<u64, UnixStream>>,
    next: AtomicU64,
    authorize: Authorize,
    power: ReadPower,
}

impl<P: Pmset + Send + 'static> Server<P> {
    pub fn new(
        core: Core<P>,
        authorize: impl Fn(&UnixStream) -> bool + Send + Sync + 'static,
        power: impl Fn() -> Power + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            core: Mutex::new(core),
            conns: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            authorize: Box::new(authorize),
            power: Box::new(power),
        })
    }

    /// Accepts connections until the listener fails.
    pub fn serve(self: &Arc<Self>, listener: UnixListener) {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let server = self.clone();
            std::thread::spawn(move || server.connection(stream));
        }
    }

    /// Re-checks the guard rails: holders are disconnected on a violation,
    /// and a reset that failed is tried again.
    pub fn tick(&self) {
        let power = (self.power)();
        let dropped = self.core().tick(&power);
        if dropped.is_empty() {
            return;
        }
        eprintln!(
            "zwiim-power-helper: guard rail tripped, released {} hold(s)",
            dropped.len()
        );
        let conns = self.conns.lock().unwrap_or_else(|e| e.into_inner());
        for conn in dropped {
            if let Some(stream) = conns.get(&conn) {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
    }

    /// The daemon is stopping: let the Mac sleep again. False when sleep
    /// could not be put back.
    pub fn shutdown(&self) -> bool {
        self.core().shutdown()
    }

    fn core(&self) -> MutexGuard<'_, Core<P>> {
        self.core.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn connection(&self, mut stream: UnixStream) {
        let Ok(reading) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(reading);
        if !(self.authorize)(&stream) {
            let _ = stream.set_read_timeout(Some(REFUSE_TIMEOUT));
            let _ = read_line(&mut reader);
            let _ = stream.write_all(b"error unauthorized\n");
            return;
        }
        let conn = self.next.fetch_add(1, Ordering::Relaxed);
        if let Ok(registered) = stream.try_clone() {
            self.conns
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(conn, registered);
        }
        while let Some(line) = read_line(&mut reader) {
            let reply = match line.as_deref() {
                Some("hold") => {
                    let power = (self.power)();
                    match self.core().hold(conn, &power) {
                        Ok(()) => "ok".to_string(),
                        Err(reason) => format!("error {reason}"),
                    }
                }
                Some("release") => {
                    self.core().release(conn);
                    "ok".to_string()
                }
                _ => "error unknown_verb".to_string(),
            };
            if stream.write_all(format!("{reply}\n").as_bytes()).is_err() || line.is_none() {
                break;
            }
        }
        self.conns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&conn);
        self.core().release(conn);
    }
}

/// The next line without its newline; `Some(None)` for a line that is too
/// long or not text, `None` once the connection is done.
fn read_line(reader: &mut BufReader<UnixStream>) -> Option<Option<String>> {
    let mut line = Vec::new();
    match reader.by_ref().take(MAX_LINE).read_until(b'\n', &mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) if line.last() != Some(&b'\n') => Some(None),
        Ok(_) => {
            line.pop();
            Some(String::from_utf8(line).ok())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::core::{Core, Pmset, Power};

    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<Vec<bool>>>);

    impl Fake {
        fn calls(&self) -> Vec<bool> {
            self.0.lock().unwrap().clone()
        }
    }

    impl Pmset for Fake {
        fn set_disablesleep(&mut self, on: bool) -> bool {
            self.0.lock().unwrap().push(on);
            true
        }
    }

    const AC: Power = Power {
        on_ac: Some(true),
        battery_percent: Some(80),
        hot: false,
    };

    struct Rig {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
        fake: Fake,
        power: Arc<Mutex<Power>>,
        server: Arc<Server<Fake>>,
    }

    fn rig(authorized: bool) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("helper.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let fake = Fake::default();
        let power = Arc::new(Mutex::new(AC));
        let read = power.clone();
        let server = Server::new(
            Core::new(fake.clone()),
            move |_: &UnixStream| authorized,
            move || *read.lock().unwrap(),
        );
        let serving = server.clone();
        std::thread::spawn(move || serving.serve(listener));
        Rig {
            _dir: dir,
            path,
            fake,
            power,
            server,
        }
    }

    fn connect(rig: &Rig) -> (UnixStream, BufReader<UnixStream>) {
        let stream = UnixStream::connect(&rig.path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let reader = BufReader::new(stream.try_clone().unwrap());
        (stream, reader)
    }

    fn ask(stream: &mut UnixStream, reader: &mut BufReader<UnixStream>, line: &str) -> String {
        stream.write_all(line.as_bytes()).unwrap();
        let mut reply = String::new();
        reader.read_line(&mut reply).unwrap();
        reply
    }

    fn closed(reader: &mut BufReader<UnixStream>) -> bool {
        let mut rest = Vec::new();
        matches!(reader.read_to_end(&mut rest), Ok(0))
    }

    fn eventually(what: &str, check: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !check() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn hold_then_release_replies_ok() {
        let rig = rig(true);
        let (mut s, mut r) = connect(&rig);
        assert_eq!(ask(&mut s, &mut r, "hold\n"), "ok\n");
        assert_eq!(rig.fake.calls(), [false, true]);
        assert_eq!(ask(&mut s, &mut r, "release\n"), "ok\n");
        assert_eq!(rig.fake.calls(), [false, true, false]);
    }

    #[test]
    fn guard_rail_refusal_is_an_error() {
        let rig = rig(true);
        *rig.power.lock().unwrap() = Power {
            on_ac: Some(false),
            ..AC
        };
        let (mut s, mut r) = connect(&rig);
        assert_eq!(ask(&mut s, &mut r, "hold\n"), "error on_battery\n");
        assert_eq!(rig.fake.calls(), [false]);
    }

    #[test]
    fn unknown_verb_is_an_error() {
        let rig = rig(true);
        let (mut s, mut r) = connect(&rig);
        assert_eq!(ask(&mut s, &mut r, "sleep\n"), "error unknown_verb\n");
        // Still connected.
        assert_eq!(ask(&mut s, &mut r, "release\n"), "ok\n");
    }

    #[test]
    fn overlong_line_disconnects() {
        let rig = rig(true);
        let (mut s, mut r) = connect(&rig);
        s.write_all(&[b'x'; 4096]).unwrap();
        let mut reply = String::new();
        r.read_line(&mut reply).unwrap();
        assert_eq!(reply, "error unknown_verb\n");
        assert!(closed(&mut r));
    }

    #[test]
    fn unauthorized_caller_is_refused() {
        let rig = rig(false);
        let (mut s, mut r) = connect(&rig);
        assert_eq!(ask(&mut s, &mut r, "hold\n"), "error unauthorized\n");
        assert!(closed(&mut r));
        assert_eq!(rig.fake.calls(), [false]);
    }

    #[test]
    fn dropping_the_client_releases() {
        let rig = rig(true);
        let (mut s, mut r) = connect(&rig);
        assert_eq!(ask(&mut s, &mut r, "hold\n"), "ok\n");
        drop((s, r));
        eventually("released", || rig.fake.calls() == [false, true, false]);
    }

    #[test]
    fn tick_disconnects_holders_on_violation() {
        let rig = rig(true);
        let (mut s, mut r) = connect(&rig);
        assert_eq!(ask(&mut s, &mut r, "hold\n"), "ok\n");
        let (mut idle, mut idle_r) = connect(&rig);
        assert_eq!(ask(&mut idle, &mut idle_r, "release\n"), "ok\n");
        rig.server.tick();
        assert_eq!(rig.fake.calls(), [false, true], "AC: nothing changes");

        *rig.power.lock().unwrap() = Power {
            on_ac: Some(false),
            ..AC
        };
        rig.server.tick();
        assert!(closed(&mut r));
        assert_eq!(rig.fake.calls(), [false, true, false]);
        // A connection that held nothing stays.
        assert_eq!(ask(&mut idle, &mut idle_r, "release\n"), "ok\n");
    }

    #[test]
    fn shutdown_resets() {
        let rig = rig(true);
        let (mut s, mut r) = connect(&rig);
        assert_eq!(ask(&mut s, &mut r, "hold\n"), "ok\n");
        assert!(rig.server.shutdown());
        assert_eq!(rig.fake.calls(), [false, true, false]);
        // A hold racing the stop is refused, so nothing sets it again.
        let (mut s, mut r) = connect(&rig);
        assert_eq!(ask(&mut s, &mut r, "hold\n"), "error stopping\n");
        assert_eq!(rig.fake.calls(), [false, true, false]);
    }
}
