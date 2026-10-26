//! The one task that holds the computer awake: it looks again whenever a
//! turn starts or ends, the setting changes, the computer wakes, or every
//! minute, and makes the holds match [`decide`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{watch, Notify};

use super::backend::{HoldGuard, PowerBackend, PowerSnapshot};
use super::config::{self, KeepAwakeConfig};
use super::policy::{decide, Blocker, Decision, Inputs, Mode};
use crate::acp::types::{AcpEvent, EventEnvelope};
use crate::app_state::AppState;

/// How often to look again with nothing else happening: the battery drains,
/// and a hold that vanished is replaced.
const RECHECK: Duration = Duration::from_secs(60);

/// The longest [`Supervisor::active_now`] waits for the loop to catch up.
const CATCH_UP: Duration = Duration::from_secs(1);

pub struct Supervisor {
    backend: Arc<dyn PowerBackend>,
    changed: Notify,
    hold: Mutex<Option<Box<dyn HoldGuard>>>,
    /// Requests to look again, counted; each pass publishes the count it saw
    /// on `passes`, so a caller can wait for a pass that began after asking.
    asked: AtomicU64,
    passes: watch::Sender<u64>,
    recheck: Duration,
}

impl Supervisor {
    pub fn new(backend: Arc<dyn PowerBackend>) -> Arc<Self> {
        Self::with_recheck(backend, RECHECK)
    }

    pub fn with_recheck(backend: Arc<dyn PowerBackend>, recheck: Duration) -> Arc<Self> {
        Arc::new(Self {
            backend,
            changed: Notify::new(),
            hold: Mutex::new(None),
            asked: AtomicU64::new(0),
            passes: watch::Sender::new(0),
            recheck,
        })
    }

    /// Look again now.
    pub fn nudge(&self) {
        self.changed.notify_one();
    }

    /// Whether a hold is in place right now.
    pub fn active(&self) -> bool {
        let mut hold = self.hold.lock().unwrap_or_else(|e| e.into_inner());
        hold.as_mut().is_some_and(|h| h.is_alive())
    }

    /// Whether a hold is in place; when that is not what is `wanted`, the
    /// loop looks again first (up to [`CATCH_UP`]), so a status read right
    /// after a change or an unplug tells the truth.
    pub async fn active_now(&self, wanted: bool) -> bool {
        if self.active() != wanted {
            let asked = self.asked.fetch_add(1, Ordering::SeqCst) + 1;
            let mut passes = self.passes.subscribe();
            self.changed.notify_one();
            let _ = tokio::time::timeout(CATCH_UP, passes.wait_for(|seen| *seen >= asked)).await;
        }
        self.active()
    }

    /// Hold the computer as [`decide`] says, for as long as the host runs.
    pub fn run(self: Arc<Self>, state: Arc<AppState>) {
        let mut rx = state.acp_event_bus.subscribe();
        tokio::spawn(async move {
            loop {
                let seen = self.asked.load(Ordering::SeqCst);
                let (config, _, decision) = evaluate(self.backend.as_ref(), &state).await;
                {
                    let mut hold = self.hold.lock().unwrap_or_else(|e| e.into_inner());
                    if hold.as_mut().is_some_and(|h| !h.is_alive()) {
                        tracing::warn!("[power] hold ended on its own; replacing it");
                        *hold = None;
                    }
                    if decision.hold_idle && hold.is_none() {
                        match self.backend.hold_idle("keep this host reachable") {
                            Ok(guard) => {
                                tracing::info!("[power] holding");
                                *hold = Some(guard);
                            }
                            Err(e) => tracing::warn!("[power] could not hold: {e}"),
                        }
                    } else if !decision.hold_idle && hold.take().is_some() {
                        tracing::info!("[power] released ({})", release_reason(&config, &decision));
                    }
                }
                self.passes.send_replace(seen);

                tokio::select! {
                    closed = turn_started_or_ended(&mut rx) => if closed { return },
                    _ = self.changed.notified() => {}
                    _ = tokio::time::sleep(self.recheck) => {}
                }
            }
        });
    }
}

fn release_reason(config: &KeepAwakeConfig, decision: &Decision) -> &'static str {
    match (decision.blocked_by, config.mode) {
        (Some(Blocker::LowBattery), _) => "battery low",
        (Some(Blocker::OnBattery), _) => "on battery",
        (Some(Blocker::Thermal), _) => "too hot",
        (None, Mode::Off) => "turned off",
        (None, Mode::WhileWorking) => "no agent working",
        (None, Mode::Always) => "not supported",
    }
}

/// The setting, the power state, and what to hold, read fresh.
pub async fn evaluate(
    backend: &dyn PowerBackend,
    state: &AppState,
) -> (KeepAwakeConfig, PowerSnapshot, Decision) {
    let config = config::load(&state.db.conn).await.unwrap_or_default();
    let power = backend.snapshot().await;
    let turn_in_flight = turn_in_flight(state).await;
    let decision = decide(&Inputs {
        config,
        turn_in_flight,
        power,
        caps: backend.capabilities(),
    });
    (config, power, decision)
}

/// Waits for an event that can start or end a turn; true once the bus closed.
async fn turn_started_or_ended(
    rx: &mut tokio::sync::broadcast::Receiver<Arc<EventEnvelope>>,
) -> bool {
    loop {
        match rx.recv().await {
            Ok(e)
                if matches!(
                    e.payload,
                    AcpEvent::StatusChanged { .. } | AcpEvent::TurnComplete { .. }
                ) =>
            {
                return false
            }
            Ok(_) => {}
            // Missed events may have included one: look again.
            Err(RecvError::Lagged(_)) => return false,
            Err(RecvError::Closed) => return true,
        }
    }
}

pub async fn turn_in_flight(state: &AppState) -> bool {
    for info in state.connection_manager.list_connections().await {
        if let Some(s) = state.connection_manager.get_state(&info.id).await {
            if s.read().await.turn_in_flight {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Mutex;
    use std::time::Instant;

    use futures::future::BoxFuture;

    use super::*;
    use crate::acp::types::{AcpEvent, EventEnvelope};
    use crate::models::agent::AgentType;
    use crate::power::backend::{
        Capabilities, HoldGuard, LidSupport, PowerError, PowerSnapshot, PowerSource,
    };
    use crate::power::config;
    use crate::power::policy::Mode;
    use crate::web::event_bridge::EventEmitter;

    const AC: PowerSnapshot = PowerSnapshot {
        source: PowerSource::Ac,
        battery_percent: Some(80),
        lid_closed: None,
        thermal: None,
    };
    const FAST: Duration = Duration::from_millis(100);

    struct FakeBackend {
        snapshot: Mutex<PowerSnapshot>,
        /// Guards alive right now.
        live: Arc<AtomicUsize>,
        /// Guards ever handed out.
        acquired: AtomicUsize,
        /// Each guard's switch: false makes it report dead.
        guards: Mutex<Vec<Arc<AtomicBool>>>,
    }

    struct FakeGuard {
        live: Arc<AtomicUsize>,
        alive: Arc<AtomicBool>,
    }

    impl HoldGuard for FakeGuard {
        fn is_alive(&mut self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }
    }

    impl Drop for FakeGuard {
        fn drop(&mut self) {
            self.live.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl FakeBackend {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                snapshot: Mutex::new(AC),
                live: Arc::new(AtomicUsize::new(0)),
                acquired: AtomicUsize::new(0),
                guards: Mutex::new(Vec::new()),
            })
        }

        fn set_power(&self, snapshot: PowerSnapshot) {
            *self.snapshot.lock().unwrap() = snapshot;
        }

        fn kill_guards(&self) {
            for g in self.guards.lock().unwrap().iter() {
                g.store(false, Ordering::SeqCst);
            }
        }
    }

    impl PowerBackend for FakeBackend {
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                idle: true,
                lid: LidSupport::NoLid,
                battery: true,
            }
        }

        fn snapshot(&self) -> BoxFuture<'_, PowerSnapshot> {
            let s = *self.snapshot.lock().unwrap();
            Box::pin(async move { s })
        }

        fn hold_idle(&self, _reason: &str) -> Result<Box<dyn HoldGuard>, PowerError> {
            let alive = Arc::new(AtomicBool::new(true));
            self.guards.lock().unwrap().push(alive.clone());
            self.live.fetch_add(1, Ordering::SeqCst);
            self.acquired.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeGuard {
                live: self.live.clone(),
                alive,
            }))
        }

        fn hold_lid(&self) -> Result<Box<dyn HoldGuard>, PowerError> {
            Err(PowerError("no".into()))
        }
    }

    /// Waits up to 5 s for exactly `n` live holds.
    async fn held(fake: &FakeBackend, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let live = fake.live.load(Ordering::SeqCst);
            if live == n {
                return;
            }
            assert!(Instant::now() < deadline, "live holds: {live}, want {n}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    struct Rig {
        _dir: tempfile::TempDir,
        state: Arc<AppState>,
    }

    impl Rig {
        async fn new(config: KeepAwakeConfig) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db = crate::db::test_helpers::fresh_in_memory_db().await;
            let state = Arc::new(AppState::new_for_test(db, dir.path().to_path_buf()));
            state
                .connection_manager
                .insert_test_connection("c1", AgentType::Codex, None, EventEmitter::Noop)
                .await;
            config::save(&state.db.conn, &config).await.unwrap();
            Self { _dir: dir, state }
        }

        async fn turn(&self, running: bool) {
            let s = self.state.connection_manager.get_state("c1").await.unwrap();
            s.write().await.turn_in_flight = running;
            let payload = AcpEvent::TurnComplete {
                session_id: "s1".into(),
                stop_reason: "end_turn".into(),
                agent_type: "codex".into(),
            };
            let _ = self.state.acp_event_bus.send(Arc::new(EventEnvelope {
                seq: 1,
                connection_id: "c1".into(),
                payload,
            }));
        }
    }

    fn cfg(mode: Mode, on_battery: bool) -> KeepAwakeConfig {
        KeepAwakeConfig {
            mode,
            on_battery,
            lid_closed: false,
        }
    }

    #[tokio::test]
    async fn while_working_holds_during_a_turn_and_releases_after() {
        let rig = Rig::new(cfg(Mode::WhileWorking, false)).await;
        let fake = FakeBackend::new();
        let sup = Supervisor::with_recheck(fake.clone(), Duration::from_secs(60));
        sup.clone().run(rig.state.clone());
        held(&fake, 0).await;
        rig.turn(true).await;
        held(&fake, 1).await;
        rig.turn(false).await;
        held(&fake, 0).await;
        assert!(!sup.active());
    }

    #[tokio::test]
    async fn always_holds_with_no_turn() {
        let rig = Rig::new(cfg(Mode::Always, false)).await;
        let fake = FakeBackend::new();
        let sup = Supervisor::with_recheck(fake.clone(), FAST);
        sup.clone().run(rig.state.clone());
        held(&fake, 1).await;
        tokio::time::sleep(FAST * 3).await;
        assert!(sup.active());
        assert_eq!(fake.acquired.load(Ordering::SeqCst), 1, "one hold, kept");
    }

    #[tokio::test]
    async fn unplugging_releases_and_replugging_holds_again() {
        let rig = Rig::new(cfg(Mode::Always, false)).await;
        let fake = FakeBackend::new();
        Supervisor::with_recheck(fake.clone(), FAST).run(rig.state.clone());
        held(&fake, 1).await;
        fake.set_power(PowerSnapshot {
            source: PowerSource::Battery,
            battery_percent: Some(50),
            ..AC
        });
        held(&fake, 0).await;
        fake.set_power(AC);
        held(&fake, 1).await;
    }

    #[tokio::test]
    async fn a_dead_hold_is_replaced() {
        let rig = Rig::new(cfg(Mode::Always, false)).await;
        let fake = FakeBackend::new();
        Supervisor::with_recheck(fake.clone(), FAST).run(rig.state.clone());
        held(&fake, 1).await;
        fake.kill_guards();
        let deadline = Instant::now() + Duration::from_secs(5);
        while fake.acquired.load(Ordering::SeqCst) < 2 {
            assert!(Instant::now() < deadline, "the dead hold was not replaced");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        held(&fake, 1).await;
    }

    #[tokio::test]
    async fn turning_it_off_releases_at_once() {
        let rig = Rig::new(cfg(Mode::Always, false)).await;
        let fake = FakeBackend::new();
        let sup = Supervisor::with_recheck(fake.clone(), Duration::from_secs(60));
        sup.clone().run(rig.state.clone());
        held(&fake, 1).await;
        config::save(&rig.state.db.conn, &cfg(Mode::Off, false))
            .await
            .unwrap();
        sup.nudge();
        held(&fake, 0).await;
        assert!(!sup.active());
    }

    #[tokio::test]
    async fn a_killed_hold_is_not_reported_active() {
        let rig = Rig::new(cfg(Mode::Always, false)).await;
        let fake = FakeBackend::new();
        let sup = Supervisor::with_recheck(fake.clone(), Duration::from_secs(60));
        sup.clone().run(rig.state.clone());
        held(&fake, 1).await;
        fake.kill_guards();
        assert!(!sup.active(), "nothing holds the computer any more");
    }

    #[tokio::test]
    async fn active_now_lets_the_loop_catch_up_first() {
        let rig = Rig::new(cfg(Mode::Always, false)).await;
        let fake = FakeBackend::new();
        let sup = Supervisor::with_recheck(fake.clone(), Duration::from_secs(60));
        sup.clone().run(rig.state.clone());
        held(&fake, 1).await;
        config::save(&rig.state.db.conn, &cfg(Mode::Off, false))
            .await
            .unwrap();
        assert!(!sup.active_now(false).await);
        assert_eq!(fake.live.load(Ordering::SeqCst), 0);
        config::save(&rig.state.db.conn, &cfg(Mode::Always, false))
            .await
            .unwrap();
        assert!(sup.active_now(true).await);
    }

    /// A real `caffeinate`, held for exactly as long as a turn runs.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn the_mac_is_kept_awake_while_a_turn_runs_and_released_after() {
        async fn held(sup: &Supervisor, want: bool) {
            let pattern = format!("caffeinate -i -s -w {}", std::process::id());
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let running = std::process::Command::new("pgrep")
                    .args(["-f", &pattern])
                    .output()
                    .unwrap()
                    .status
                    .success();
                if running == want && sup.active() == want {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "caffeinate running: {running}, want {want}"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }

        let _turn = crate::power::macos::CAFFEINATE_TEST_LOCK.lock().await;
        let rig = Rig::new(cfg(Mode::WhileWorking, true)).await;
        let sup = Supervisor::new(crate::power::backend().await);
        sup.clone().run(rig.state.clone());
        held(&sup, false).await;
        rig.turn(true).await;
        held(&sup, true).await;
        rig.turn(false).await;
        held(&sup, false).await;

        // Turned off, a running turn does not hold the Mac.
        config::save(&rig.state.db.conn, &cfg(Mode::Off, true))
            .await
            .unwrap();
        sup.nudge();
        rig.turn(true).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        held(&sup, false).await;
        config::save(&rig.state.db.conn, &cfg(Mode::WhileWorking, true))
            .await
            .unwrap();
        sup.nudge();
        held(&sup, true).await;
        rig.turn(false).await;
        held(&sup, false).await;
    }
}
