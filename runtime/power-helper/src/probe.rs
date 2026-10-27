//! The power state the guard rails need, read from `pmset`. The parsers
//! mirror the Host's (`runtime/host/src/power/macos.rs`); the helper does not
//! link the Host, so they are kept here.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::core::Power;

/// `pmset` answers in milliseconds; past this, read nothing.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// The power state now. Anything unreadable reads as unknown, which the
/// guard rails refuse.
pub fn read_power() -> Power {
    let read = |args: &[&str]| pmset(args).unwrap_or_default();
    let (on_ac, battery_percent) = parse_batt(&read(&["-g", "batt"]));
    Power {
        on_ac,
        battery_percent,
        hot: parse_hot(&read(&["-g", "therm"])),
    }
}

/// `/usr/bin/pmset`'s stdout when it succeeded; `None` if it could not run,
/// failed, or took longer than [`COMMAND_TIMEOUT`] (it is killed then).
pub fn pmset(args: &[&str]) -> Option<String> {
    let Ok(mut child) = Command::new("/usr/bin/pmset")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return None;
    };
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    status.success().then_some(out)
}

/// From `pmset -g batt`: on AC power, and the battery level.
pub fn parse_batt(pmset: &str) -> (Option<bool>, Option<u8>) {
    let on_ac = if pmset.contains("'Battery Power'") {
        Some(false)
    } else if pmset.contains("'AC Power'") {
        Some(true)
    } else {
        None
    };
    let percent = pmset.lines().find_map(|line| {
        let (before, _) = line.split_once('%')?;
        let digits = before.rsplit(|c: char| !c.is_ascii_digit()).next()?;
        digits.parse::<u8>().ok()
    });
    (on_ac, percent)
}

/// From `pmset -g therm`: throttled or warned is too hot.
pub fn parse_hot(pmset: &str) -> bool {
    let number = |s: &str| s.trim().trim_end_matches('.').parse::<u32>().ok();
    pmset.lines().any(|line| {
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "CPU_Speed_Limit" {
                return number(value).is_some_and(|n| n < 100);
            }
        }
        line.contains("warning level") && line.split_whitespace().filter_map(number).any(|n| n > 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_source_and_percent() {
        let real = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=22806627)\t85%; discharging; 6:02 remaining present: true\n";
        assert_eq!(parse_batt(real), (Some(false), Some(85)));
        let charging = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=4653155)\t3%; charging; 2:10 remaining present: true\n";
        assert_eq!(parse_batt(charging), (Some(true), Some(3)));
        assert_eq!(
            parse_batt("Now drawing from 'AC Power'\n"),
            (Some(true), None)
        );
        assert_eq!(parse_batt(""), (None, None));
    }

    #[test]
    fn thermal_levels() {
        let real = "Note: No thermal warning level has been recorded\nNote: No performance warning level has been recorded\nNote: No CPU power status has been recorded\n";
        assert!(!parse_hot(real));
        assert!(!parse_hot("CPU_Speed_Limit \t= 100\n"));
        assert!(parse_hot("CPU_Speed_Limit \t= 50\n"));
        assert!(parse_hot("Thermal warning level set to 2.\n"));
        assert!(!parse_hot("Thermal warning level set to 0.\n"));
        assert!(!parse_hot(""));
    }
}
