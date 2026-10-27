//! Who holds the lid, and what `disablesleep` should be. Pure: the power
//! command is injected, so every case is tested without touching the Mac.

use std::collections::BTreeSet;

/// Below this the lid is let go: a closed Mac that is barely charged should
/// sleep. Matches the Host's `LID_MIN_BATTERY_PERCENT`.
pub const MIN_BATTERY_PERCENT: u8 = 20;

/// How often a stopping daemon tries to put sleep back. launchd waits 20 s
/// before killing it, and each try may take up to 5 s.
pub const SHUTDOWN_ATTEMPTS: usize = 3;

/// Sets `disablesleep`; true when it was applied.
pub trait Pmset {
    fn set_disablesleep(&mut self, on: bool) -> bool;
}

/// What the guard rails look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Power {
    /// `None` when the power source could not be read.
    pub on_ac: Option<bool>,
    /// `None` on a Mac without a battery.
    pub battery_percent: Option<u8>,
    pub hot: bool,
}

/// Whether a closed Mac may be kept awake now; the reason it may not.
pub fn guard(p: &Power) -> Result<(), &'static str> {
    match p.on_ac {
        None => return Err("power_unknown"),
        Some(false) => return Err("on_battery"),
        Some(true) => {}
    }
    if p.battery_percent.is_some_and(|b| b < MIN_BATTERY_PERCENT) {
        return Err("low_battery");
    }
    if p.hot {
        return Err("too_hot");
    }
    Ok(())
}

/// The connections holding the lid. `disablesleep` is on exactly while
/// there is one; whenever it should be off and turning it off failed, every
/// tick tries again.
pub struct Core<P: Pmset> {
    pmset: P,
    holders: BTreeSet<u64>,
    needs_reset: bool,
    /// Set by `shutdown`: no hold is taken after it.
    stopping: bool,
}

impl<P: Pmset> Core<P> {
    /// Whatever a previous run left behind is undone first.
    pub fn new(pmset: P) -> Self {
        let mut core = Self {
            pmset,
            holders: BTreeSet::new(),
            needs_reset: true,
            stopping: false,
        };
        core.try_reset();
        core
    }

    pub fn hold(&mut self, conn: u64, power: &Power) -> Result<(), String> {
        if self.stopping {
            return Err("stopping".into());
        }
        guard(power).map_err(str::to_string)?;
        if self.holders.contains(&conn) {
            return Ok(());
        }
        if self.holders.is_empty() {
            if !self.pmset.set_disablesleep(true) {
                self.needs_reset = true;
                self.try_reset();
                return Err("pmset_failed".into());
            }
            self.needs_reset = false;
        }
        self.holders.insert(conn);
        Ok(())
    }

    pub fn release(&mut self, conn: u64) {
        if self.holders.remove(&conn) && self.holders.is_empty() {
            self.needs_reset = true;
            self.try_reset();
        }
    }

    /// Re-checks the guard rails for everyone holding, and retries a reset
    /// that failed. Returns the connections dropped by a guard rail.
    pub fn tick(&mut self, power: &Power) -> Vec<u64> {
        let mut dropped = Vec::new();
        if !self.holders.is_empty() && guard(power).is_err() {
            dropped = std::mem::take(&mut self.holders).into_iter().collect();
            self.needs_reset = true;
        }
        self.try_reset();
        dropped
    }

    /// The daemon is stopping: let the Mac sleep again, and take no hold
    /// after this. False when sleep could not be put back.
    pub fn shutdown(&mut self) -> bool {
        self.stopping = true;
        self.holders.clear();
        self.needs_reset = true;
        for _ in 0..SHUTDOWN_ATTEMPTS {
            self.try_reset();
            if !self.needs_reset {
                return true;
            }
        }
        false
    }

    #[cfg(test)]
    pub fn held(&self) -> bool {
        !self.holders.is_empty()
    }

    fn try_reset(&mut self) {
        if self.needs_reset && self.holders.is_empty() && self.pmset.set_disablesleep(false) {
            self.needs_reset = false;
        }
    }
}

/// The code requirement a caller must satisfy: the Zwiim Host signed by
/// `team`. `None` for anything but a 10-character Apple team id, and then
/// no caller is accepted.
pub fn caller_requirement(team: &str) -> Option<String> {
    let valid = team.len() == 10
        && team
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    valid.then(|| {
        format!(
            "anchor apple generic and certificate leaf[subject.OU] = \"{team}\" \
             and identifier \"com.zwiim.host\""
        )
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// Records every call; fails `true` always when asked, and the next
    /// `fail_resets` calls with `false`.
    #[derive(Clone, Default)]
    struct Fake {
        calls: Arc<Mutex<Vec<bool>>>,
        fail_set: bool,
        fail_resets: Arc<Mutex<usize>>,
    }

    impl Fake {
        fn calls(&self) -> Vec<bool> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Pmset for Fake {
        fn set_disablesleep(&mut self, on: bool) -> bool {
            self.calls.lock().unwrap().push(on);
            if on {
                return !self.fail_set;
            }
            let mut left = self.fail_resets.lock().unwrap();
            if *left > 0 {
                *left -= 1;
                return false;
            }
            true
        }
    }

    const AC: Power = Power {
        on_ac: Some(true),
        battery_percent: Some(80),
        hot: false,
    };

    fn core() -> (Core<Fake>, Fake) {
        let fake = Fake::default();
        (Core::new(fake.clone()), fake)
    }

    #[test]
    fn new_resets_to_zero() {
        let (_core, fake) = core();
        assert_eq!(fake.calls(), [false]);
    }

    #[test]
    fn first_hold_sets_and_last_release_resets() {
        let (mut core, fake) = core();
        core.hold(1, &AC).unwrap();
        core.hold(2, &AC).unwrap();
        assert_eq!(fake.calls(), [false, true]);
        assert!(core.held());
        core.release(1);
        assert_eq!(fake.calls(), [false, true]);
        core.release(2);
        assert_eq!(fake.calls(), [false, true, false]);
        assert!(!core.held());
    }

    #[test]
    fn hold_refused_by_guard() {
        let cases = [
            (
                Power {
                    on_ac: Some(false),
                    ..AC
                },
                "on_battery",
            ),
            (
                Power {
                    battery_percent: Some(MIN_BATTERY_PERCENT - 1),
                    ..AC
                },
                "low_battery",
            ),
            (Power { hot: true, ..AC }, "too_hot"),
            (Power { on_ac: None, ..AC }, "power_unknown"),
        ];
        for (power, reason) in cases {
            let (mut core, fake) = core();
            assert_eq!(core.hold(1, &power), Err(reason.to_string()));
            assert_eq!(fake.calls(), [false], "{reason}");
            assert!(!core.held());
        }
        let (mut core, _) = core();
        let charged = Power {
            battery_percent: Some(MIN_BATTERY_PERCENT),
            ..AC
        };
        assert_eq!(core.hold(1, &charged), Ok(()));
        let desktop = Power {
            battery_percent: None,
            ..AC
        };
        assert_eq!(core.hold(2, &desktop), Ok(()));
    }

    #[test]
    fn failed_set_is_not_ok() {
        let fake = Fake {
            fail_set: true,
            ..Fake::default()
        };
        let mut core = Core::new(fake.clone());
        assert!(core.hold(1, &AC).is_err());
        assert!(!core.held());
        // Whatever pmset half-did is undone.
        assert_eq!(fake.calls().last(), Some(&false));
    }

    #[test]
    fn failed_reset_retries_each_tick() {
        let fake = Fake::default();
        *fake.fail_resets.lock().unwrap() = 2;
        let mut core = Core::new(fake.clone());
        assert_eq!(fake.calls(), [false]);
        assert!(core.tick(&AC).is_empty());
        assert_eq!(fake.calls(), [false, false]);
        core.tick(&AC);
        assert_eq!(fake.calls(), [false, false, false]);
        // Reset at last: nothing more to do.
        core.tick(&AC);
        assert_eq!(fake.calls(), [false, false, false]);
    }

    #[test]
    fn hold_after_failed_reset_clears_the_retry() {
        let fake = Fake::default();
        *fake.fail_resets.lock().unwrap() = 1;
        let mut core = Core::new(fake.clone());
        core.hold(1, &AC).unwrap();
        core.tick(&AC);
        assert_eq!(fake.calls(), [false, true]);
    }

    #[test]
    fn tick_violation_drops_everyone() {
        let (mut core, fake) = core();
        core.hold(1, &AC).unwrap();
        core.hold(2, &AC).unwrap();
        assert!(core.tick(&AC).is_empty());
        let unplugged = Power {
            on_ac: Some(false),
            ..AC
        };
        let mut dropped = core.tick(&unplugged);
        dropped.sort();
        assert_eq!(dropped, [1, 2]);
        assert!(!core.held());
        assert_eq!(fake.calls(), [false, true, false]);
        // A late release from a dropped connection changes nothing.
        core.release(1);
        assert_eq!(fake.calls(), [false, true, false]);
    }

    #[test]
    fn release_of_unknown_conn_is_harmless() {
        let (mut core, fake) = core();
        core.release(7);
        assert_eq!(fake.calls(), [false]);
    }

    #[test]
    fn double_hold_same_conn_counts_once() {
        let (mut core, fake) = core();
        core.hold(1, &AC).unwrap();
        core.hold(1, &AC).unwrap();
        core.release(1);
        assert!(!core.held());
        assert_eq!(fake.calls(), [false, true, false]);
    }

    #[test]
    fn shutdown_resets() {
        let (mut core, fake) = core();
        core.hold(1, &AC).unwrap();
        assert!(core.shutdown());
        assert!(!core.held());
        assert_eq!(fake.calls(), [false, true, false]);
    }

    #[test]
    fn hold_after_shutdown_is_refused() {
        let (mut core, fake) = core();
        core.shutdown();
        assert_eq!(core.hold(1, &AC), Err("stopping".to_string()));
        assert!(!core.held());
        assert_eq!(fake.calls(), [false, false]);
    }

    #[test]
    fn shutdown_retries_a_failed_reset() {
        let (mut core, fake) = core();
        core.hold(1, &AC).unwrap();
        *fake.fail_resets.lock().unwrap() = SHUTDOWN_ATTEMPTS - 1;
        assert!(core.shutdown());
        assert_eq!(fake.calls().len(), 2 + SHUTDOWN_ATTEMPTS);
        assert_eq!(fake.calls().last(), Some(&false));
    }

    #[test]
    fn shutdown_reports_a_reset_that_kept_failing() {
        let (mut core, fake) = core();
        core.hold(1, &AC).unwrap();
        *fake.fail_resets.lock().unwrap() = SHUTDOWN_ATTEMPTS + 5;
        assert!(!core.shutdown());
        assert_eq!(fake.calls().len(), 2 + SHUTDOWN_ATTEMPTS);
    }

    #[test]
    fn requirement_needs_a_team() {
        assert_eq!(
            caller_requirement("ABCDE12345").as_deref(),
            Some(
                "anchor apple generic and certificate leaf[subject.OU] = \"ABCDE12345\" \
                 and identifier \"com.zwiim.host\""
            )
        );
        for bad in ["", "abc", "ABCDE1234!", "abcde12345", "ABCDE123456"] {
            assert_eq!(caller_requirement(bad), None, "{bad}");
        }
    }
}
