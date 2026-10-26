//! Noticing that the computer slept: the wall clock jumps ahead while the
//! monotonic clock (which stops during sleep) does not. On a wake the host
//! redials the Cloud at once instead of waiting out a dead socket.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crate::app_state::AppState;

/// How often the clocks are compared.
const TICK: Duration = Duration::from_secs(5);
/// Wall time beyond monotonic time that counts as a sleep, not drift.
const GAP: Duration = Duration::from_secs(20);

pub struct WakeDetector {
    wall: SystemTime,
    mono: Instant,
}

impl WakeDetector {
    pub fn new(wall: SystemTime, mono: Instant) -> Self {
        Self { wall, mono }
    }

    /// How long the computer slept since the last observation, if it did.
    pub fn observe(&mut self, wall: SystemTime, mono: Instant) -> Option<Duration> {
        let walked = wall.duration_since(self.wall).ok();
        let ticked = mono.saturating_duration_since(self.mono);
        (self.wall, self.mono) = (wall, mono);
        walked?.checked_sub(ticked).filter(|slept| *slept > GAP)
    }
}

/// Watch for wakes for as long as the host runs.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut detector = WakeDetector::new(SystemTime::now(), Instant::now());
        let mut tick = tokio::time::interval(TICK);
        loop {
            tick.tick().await;
            if let Some(slept) = detector.observe(SystemTime::now(), Instant::now()) {
                tracing::info!("[power] woke after {}s asleep", slept.as_secs());
                state.cloud.nudge();
                super::nudge();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start() -> (WakeDetector, SystemTime, Instant) {
        let (wall, mono) = (SystemTime::now(), Instant::now());
        (WakeDetector::new(wall, mono), wall, mono)
    }

    #[test]
    fn a_wall_jump_fires_once() {
        let (mut d, wall, mono) = start();
        let slept = d
            .observe(
                wall + Duration::from_secs(30),
                mono + Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(slept, Duration::from_secs(25));
        assert_eq!(
            d.observe(
                wall + Duration::from_secs(35),
                mono + Duration::from_secs(10)
            ),
            None
        );
    }

    #[test]
    fn normal_drift_does_not_fire() {
        let (mut d, wall, mono) = start();
        assert_eq!(
            d.observe(
                wall + Duration::from_millis(5400),
                mono + Duration::from_secs(5)
            ),
            None
        );
        assert_eq!(
            d.observe(
                wall + Duration::from_millis(29400),
                mono + Duration::from_secs(10)
            ),
            None
        );
    }

    #[test]
    fn a_clock_set_backwards_does_not_fire() {
        let (mut d, wall, mono) = start();
        assert_eq!(
            d.observe(
                wall - Duration::from_secs(3600),
                mono + Duration::from_secs(5)
            ),
            None
        );
        // Re-based on the earlier wall time: a normal tick after it is quiet.
        let wall = wall - Duration::from_secs(3600);
        assert_eq!(
            d.observe(
                wall + Duration::from_secs(5),
                mono + Duration::from_secs(10)
            ),
            None
        );
    }
}
