//! macOS: `caffeinate` holds the Mac awake; `pmset` and `ioreg` report the
//! power source, battery, lid and heat. The parsers build everywhere so
//! their tests run on any CI machine.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use super::backend::{PowerSource, Thermal};

#[cfg(target_os = "macos")]
pub use imp::MacBackend;

#[cfg(target_os = "macos")]
mod imp {
    use futures::future::BoxFuture;

    use super::super::backend::{
        Capabilities, HoldGuard, LidSupport, PowerBackend, PowerError, PowerSnapshot,
    };
    use super::{parse_batt, parse_clamshell, parse_therm};

    /// `pmset` and `ioreg` answer in milliseconds; past this, read nothing.
    const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    pub struct MacBackend {
        pub(super) has_lid: bool,
    }

    impl MacBackend {
        /// A Mac with a lid reports `AppleClamshellState`; a desktop does not.
        pub async fn detect() -> Self {
            Self {
                has_lid: parse_clamshell(&ioreg().await).is_some(),
            }
        }
    }

    impl PowerBackend for MacBackend {
        fn capabilities(&self) -> Capabilities {
            let lid = if self.has_lid {
                LidSupport::Unavailable
            } else {
                LidSupport::NoLid
            };
            Capabilities {
                idle: true,
                lid,
                battery: true,
            }
        }

        fn snapshot(&self) -> BoxFuture<'_, PowerSnapshot> {
            Box::pin(async {
                let (batt, clamshell, therm) = tokio::join!(
                    output("/usr/bin/pmset", &["-g", "batt"]),
                    ioreg(),
                    output("/usr/bin/pmset", &["-g", "therm"])
                );
                let (source, battery_percent) = parse_batt(&batt);
                // pmset that could not run says nothing about the heat.
                let thermal = (!therm.is_empty()).then(|| parse_therm(&therm));
                PowerSnapshot {
                    source,
                    battery_percent,
                    lid_closed: parse_clamshell(&clamshell),
                    thermal,
                }
            })
        }

        /// `caffeinate -i -s`: no idle sleep, and no system sleep on power.
        /// `-w` ends it with the host, however the host ends.
        fn hold_idle(&self, reason: &str) -> Result<Box<dyn HoldGuard>, PowerError> {
            let child = crate::process::tokio_command("/usr/bin/caffeinate")
                .args(["-i", "-s", "-w", &std::process::id().to_string()])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| PowerError(format!("caffeinate failed to start: {e}")))?;
            tracing::debug!("[power] caffeinate started ({reason})");
            Ok(Box::new(CaffeinateHold(child)))
        }

        fn hold_lid(&self) -> Result<Box<dyn HoldGuard>, PowerError> {
            Err(PowerError("not available".into()))
        }
    }

    /// Killed on drop.
    struct CaffeinateHold(tokio::process::Child);

    impl HoldGuard for CaffeinateHold {
        fn is_alive(&mut self) -> bool {
            matches!(self.0.try_wait(), Ok(None))
        }
    }

    async fn ioreg() -> String {
        output(
            "/usr/sbin/ioreg",
            &["-r", "-k", "AppleClamshellState", "-d", "4"],
        )
        .await
    }

    /// A command's stdout; empty if it could not run or took longer than
    /// [`COMMAND_TIMEOUT`] (it is killed then).
    pub(super) async fn output(program: &str, args: &[&str]) -> String {
        let out = crate::process::tokio_command(program)
            .args(args)
            .kill_on_drop(true)
            .output();
        match tokio::time::timeout(COMMAND_TIMEOUT, out).await {
            Ok(Ok(o)) => String::from_utf8_lossy(&o.stdout).into_owned(),
            _ => String::new(),
        }
    }
}

/// From `pmset -g batt`: where power comes from, and the battery level.
fn parse_batt(pmset: &str) -> (PowerSource, Option<u8>) {
    let source = if pmset.contains("'Battery Power'") {
        PowerSource::Battery
    } else if pmset.contains("'AC Power'") {
        PowerSource::Ac
    } else {
        PowerSource::Unknown
    };
    let percent = pmset.lines().find_map(|line| {
        let (before, _) = line.split_once('%')?;
        let digits = before.rsplit(|c: char| !c.is_ascii_digit()).next()?;
        digits.parse::<u8>().ok()
    });
    (source, percent)
}

/// From `ioreg -r -k AppleClamshellState`: whether the lid is closed, or
/// `None` without a lid.
fn parse_clamshell(ioreg: &str) -> Option<bool> {
    let line = ioreg
        .lines()
        .find(|l| l.contains("\"AppleClamshellState\""))?;
    match line.rsplit('=').next()?.trim() {
        "Yes" => Some(true),
        "No" => Some(false),
        _ => None,
    }
}

/// From `pmset -g therm`: throttled or warned is too hot.
fn parse_therm(pmset: &str) -> Thermal {
    let number = |s: &str| s.trim().trim_end_matches('.').parse::<u32>().ok();
    let hot = pmset.lines().any(|line| {
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "CPU_Speed_Limit" {
                return number(value).is_some_and(|n| n < 100);
            }
        }
        line.contains("warning level") && line.split_whitespace().filter_map(number).any(|n| n > 0)
    });
    if hot {
        Thermal::Serious
    } else {
        Thermal::Nominal
    }
}

/// Tests that look for a real `caffeinate` by its command line take turns.
#[cfg(all(test, target_os = "macos"))]
pub(crate) static CAFFEINATE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
mod tests {
    use super::*;

    const ON_BATTERY: &str = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=4653155)\t";
    const ON_POWER: &str = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=4653155)\t";

    #[test]
    fn battery_source_and_percent() {
        let real = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=22806627)\t100%; discharging; (no estimate) present: true\n";
        assert_eq!(parse_batt(real), (PowerSource::Battery, Some(100)));
        let on = |s: &str, rest: &str| parse_batt(&format!("{s}{rest}"));
        assert_eq!(
            on(
                ON_BATTERY,
                "9%; discharging; 0:21 remaining present: true\n"
            ),
            (PowerSource::Battery, Some(9))
        );
        assert_eq!(
            on(
                ON_BATTERY,
                "10%; discharging; 0:25 remaining present: true\n"
            ),
            (PowerSource::Battery, Some(10))
        );
        assert_eq!(
            on(
                ON_BATTERY,
                "85%; discharging; 6:02 remaining present: true\n"
            ),
            (PowerSource::Battery, Some(85))
        );
        assert_eq!(
            on(ON_POWER, "3%; charging; 2:10 remaining present: true\n"),
            (PowerSource::Ac, Some(3))
        );
        // A desktop Mac, or pmset unavailable.
        assert_eq!(
            parse_batt("Now drawing from 'AC Power'\n"),
            (PowerSource::Ac, None)
        );
        assert_eq!(parse_batt(""), (PowerSource::Unknown, None));
    }

    #[test]
    fn clamshell_open_closed_and_desktop() {
        let open =
            "  |   \"AppleClamshellCausesSleep\" = Yes\n  |   \"AppleClamshellState\" = No\n";
        assert_eq!(parse_clamshell(open), Some(false));
        let closed =
            "  |   \"AppleClamshellCausesSleep\" = Yes\n  |   \"AppleClamshellState\" = Yes\n";
        assert_eq!(parse_clamshell(closed), Some(true));
        assert_eq!(parse_clamshell(""), None);
    }

    #[test]
    fn thermal_levels() {
        let real = "Note: No thermal warning level has been recorded\nNote: No performance warning level has been recorded\nNote: No CPU power status has been recorded\n";
        assert_eq!(parse_therm(real), Thermal::Nominal);
        assert_eq!(parse_therm("CPU_Speed_Limit \t= 100\n"), Thermal::Nominal);
        assert_eq!(parse_therm("CPU_Speed_Limit \t= 50\n"), Thermal::Serious);
        assert_eq!(
            parse_therm("Thermal warning level set to 2.\n"),
            Thermal::Serious
        );
        assert_eq!(
            parse_therm("Thermal warning level set to 0.\n"),
            Thermal::Nominal
        );
        assert_eq!(parse_therm(""), Thermal::Nominal);
    }

    /// A real `caffeinate`, alive exactly as long as its guard.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn caffeinate_lives_as_long_as_the_guard() {
        use std::time::{Duration, Instant};

        use super::super::backend::PowerBackend;

        async fn running(want: bool) {
            let pattern = format!("caffeinate -i -s -w {}", std::process::id());
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let found = std::process::Command::new("pgrep")
                    .args(["-f", &pattern])
                    .output()
                    .unwrap()
                    .status
                    .success();
                if found == want {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "caffeinate running: {found}, want {want}"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }

        let _turn = super::CAFFEINATE_TEST_LOCK.lock().await;
        let backend = MacBackend { has_lid: true };
        let mut guard = backend.hold_idle("test").unwrap();
        running(true).await;
        assert!(guard.is_alive());
        drop(guard);
        running(false).await;
    }

    /// A stuck `pmset` or `ioreg` must not stall the supervisor for good.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn a_stuck_command_reads_as_empty() {
        let started = std::time::Instant::now();
        assert_eq!(imp::output("/bin/sleep", &["30"]).await, "");
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }
}
