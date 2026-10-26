//! What to hold, decided from the setting, whether an agent works, and the
//! power state. Pure: no OS calls, so every case is tested anywhere.

use serde::{Deserialize, Serialize};

use super::backend::{Capabilities, LidSupport, PowerSnapshot, PowerSource, Thermal};
use super::config::KeepAwakeConfig;

/// On battery below this, nothing is held whatever the setting.
pub const MIN_BATTERY_PERCENT: u8 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Off,
    WhileWorking,
    Always,
}

/// Why nothing is held although the setting asks for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Blocker {
    OnBattery,
    LowBattery,
    Thermal,
}

/// Held, but something will still sleep the computer or cost the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Warning {
    LidCloseWillSleep,
    OnBatteryWillDrain,
}

#[derive(Debug, Clone, Copy)]
pub struct Inputs {
    pub config: KeepAwakeConfig,
    pub turn_in_flight: bool,
    pub power: PowerSnapshot,
    pub caps: Capabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Decision {
    pub hold_idle: bool,
    pub hold_lid: bool,
    pub blocked_by: Option<Blocker>,
    pub warnings: Vec<Warning>,
}

pub fn decide(i: &Inputs) -> Decision {
    let want = match i.config.mode {
        Mode::Off => false,
        Mode::WhileWorking => i.turn_in_flight,
        Mode::Always => true,
    };
    if !want {
        return Decision::default();
    }
    let on_battery = i.power.source == PowerSource::Battery;
    let blocked_by = if on_battery
        && i.power
            .battery_percent
            .is_some_and(|p| p < MIN_BATTERY_PERCENT)
    {
        Some(Blocker::LowBattery)
    } else if on_battery && !i.config.on_battery {
        Some(Blocker::OnBattery)
    } else if matches!(i.power.thermal, Some(Thermal::Serious | Thermal::Critical)) {
        Some(Blocker::Thermal)
    } else {
        None
    };
    if blocked_by.is_some() {
        return Decision {
            blocked_by,
            ..Decision::default()
        };
    }
    let hold_idle = i.caps.idle;
    let hold_lid = hold_idle && i.config.lid_closed && i.caps.lid == LidSupport::Available;
    let mut warnings = Vec::new();
    if hold_idle && i.caps.lid != LidSupport::NoLid && !hold_lid {
        warnings.push(Warning::LidCloseWillSleep);
    }
    if hold_idle && on_battery {
        warnings.push(Warning::OnBatteryWillDrain);
    }
    Decision {
        hold_idle,
        hold_lid,
        blocked_by: None,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AC: PowerSnapshot = PowerSnapshot {
        source: PowerSource::Ac,
        battery_percent: Some(80),
        lid_closed: None,
        thermal: None,
    };
    const DESKTOP: Capabilities = Capabilities {
        idle: true,
        lid: LidSupport::NoLid,
        battery: true,
    };
    const LAPTOP: Capabilities = Capabilities {
        idle: true,
        lid: LidSupport::Unavailable,
        battery: true,
    };

    fn battery(percent: u8) -> PowerSnapshot {
        PowerSnapshot {
            source: PowerSource::Battery,
            battery_percent: Some(percent),
            ..AC
        }
    }

    fn config(mode: Mode, on_battery: bool) -> KeepAwakeConfig {
        KeepAwakeConfig {
            mode,
            on_battery,
            lid_closed: false,
        }
    }

    fn run(
        config: KeepAwakeConfig,
        turn_in_flight: bool,
        power: PowerSnapshot,
        caps: Capabilities,
    ) -> Decision {
        decide(&Inputs {
            config,
            turn_in_flight,
            power,
            caps,
        })
    }

    fn holding(warnings: Vec<Warning>) -> Decision {
        Decision {
            hold_idle: true,
            hold_lid: false,
            blocked_by: None,
            warnings,
        }
    }

    fn blocked(by: Blocker) -> Decision {
        Decision {
            blocked_by: Some(by),
            ..Decision::default()
        }
    }

    #[test]
    fn off_holds_nothing() {
        for turn in [false, true] {
            assert_eq!(
                run(config(Mode::Off, true), turn, AC, LAPTOP),
                Decision::default()
            );
        }
    }

    #[test]
    fn while_working_holds_only_during_a_turn() {
        let c = config(Mode::WhileWorking, false);
        assert_eq!(run(c, false, AC, DESKTOP), Decision::default());
        assert_eq!(run(c, true, AC, DESKTOP), holding(vec![]));
        // Not asked, so not blocked either.
        assert_eq!(run(c, false, battery(50), DESKTOP), Decision::default());
    }

    #[test]
    fn always_holds_on_power_with_no_turn() {
        assert_eq!(
            run(config(Mode::Always, false), false, AC, DESKTOP),
            holding(vec![])
        );
    }

    #[test]
    fn battery_blocks_unless_allowed() {
        assert_eq!(
            run(config(Mode::Always, false), false, battery(50), DESKTOP),
            blocked(Blocker::OnBattery)
        );
        assert_eq!(
            run(config(Mode::Always, true), false, battery(50), DESKTOP),
            holding(vec![Warning::OnBatteryWillDrain])
        );
    }

    #[test]
    fn low_battery_blocks_whatever_the_setting() {
        let c = config(Mode::Always, true);
        assert_eq!(
            run(c, false, battery(9), DESKTOP),
            blocked(Blocker::LowBattery)
        );
        assert_eq!(
            run(config(Mode::Always, false), false, battery(9), DESKTOP),
            blocked(Blocker::LowBattery)
        );
        assert_eq!(
            run(c, false, battery(10), DESKTOP),
            holding(vec![Warning::OnBatteryWillDrain])
        );
        // Charging at 3% is on power.
        let charging = PowerSnapshot {
            battery_percent: Some(3),
            ..AC
        };
        assert_eq!(run(c, false, charging, DESKTOP), holding(vec![]));
        // On battery with the level unknown: not low.
        let unknown = PowerSnapshot {
            battery_percent: None,
            ..battery(0)
        };
        assert_eq!(
            run(c, false, unknown, DESKTOP),
            holding(vec![Warning::OnBatteryWillDrain])
        );
    }

    #[test]
    fn thermal_serious_blocks_and_fair_does_not() {
        let c = config(Mode::Always, false);
        for t in [Thermal::Serious, Thermal::Critical] {
            let hot = PowerSnapshot {
                thermal: Some(t),
                ..AC
            };
            assert_eq!(run(c, false, hot, DESKTOP), blocked(Blocker::Thermal));
        }
        for t in [Thermal::Nominal, Thermal::Fair] {
            let fine = PowerSnapshot {
                thermal: Some(t),
                ..AC
            };
            assert_eq!(run(c, false, fine, DESKTOP), holding(vec![]));
        }
    }

    #[test]
    fn a_laptop_lid_warns_and_a_desktop_does_not() {
        let c = config(Mode::Always, false);
        assert_eq!(
            run(c, false, AC, LAPTOP),
            holding(vec![Warning::LidCloseWillSleep])
        );
        assert_eq!(run(c, false, AC, DESKTOP), holding(vec![]));
        let needs_setup = Capabilities {
            lid: LidSupport::NeedsSetup,
            ..LAPTOP
        };
        assert_eq!(
            run(c, false, AC, needs_setup),
            holding(vec![Warning::LidCloseWillSleep])
        );
    }

    #[test]
    fn lid_hold_only_when_available_and_enabled() {
        let available = Capabilities {
            lid: LidSupport::Available,
            ..LAPTOP
        };
        let lid_on = KeepAwakeConfig {
            lid_closed: true,
            ..config(Mode::Always, false)
        };
        assert_eq!(
            run(lid_on, false, AC, available),
            Decision {
                hold_idle: true,
                hold_lid: true,
                blocked_by: None,
                warnings: vec![]
            }
        );
        // Available but not enabled: the lid will still sleep it.
        assert_eq!(
            run(config(Mode::Always, false), false, AC, available),
            holding(vec![Warning::LidCloseWillSleep])
        );
        // Enabled but not available.
        assert_eq!(
            run(lid_on, false, AC, LAPTOP),
            holding(vec![Warning::LidCloseWillSleep])
        );
    }

    #[test]
    fn unsupported_backend_holds_nothing() {
        let none = Capabilities {
            idle: false,
            lid: LidSupport::NoLid,
            battery: false,
        };
        assert_eq!(
            run(
                config(Mode::Always, true),
                true,
                PowerSnapshot::unknown(),
                none
            ),
            Decision::default()
        );
    }
}
