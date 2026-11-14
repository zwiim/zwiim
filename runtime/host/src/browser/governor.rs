//! Zwiim-owned: the Host browser's memory and priority policy (browser
//! design 11). [`plan`] decides from what was measured; [`sys`] measures the
//! engine's process tree and sets its priority, on macOS only.
//!
//! The ladder, from cheapest to strongest (11.3):
//!
//! 1. Screencasts nobody watches are stopped by the stream hub, always.
//! 2. From [`FREEZE_AT`] of the budget, or on a pressure warning, background
//!    tabs unseen and idle for [`FREEZE_AFTER`] are frozen.
//! 3. From [`DISCARD_AT`], or on a warning, the least recently used tab is
//!    discarded, one per check, until usage falls back.
//! 4. From the whole budget, new tabs are refused.
//! 5. On a critical pressure event every tab not in use is discarded.
//!
//! A tab in use (viewed, driven by an agent, or downloading) is never frozen
//! or discarded.

use std::time::Duration;

use tokio::time::Instant;

/// How often the governor measures.
pub const INTERVAL: Duration = Duration::from_secs(5);
/// Unseen and idle this long, a background tab may be frozen.
pub const FREEZE_AFTER: Duration = Duration::from_secs(120);
/// Driven by an agent this recently, a tab is in use.
pub const AGENT_IN_USE: Duration = Duration::from_secs(120);
/// Nobody viewed a tab or used the browser tools for this long: the engine
/// runs in the background.
pub const IDLE_AFTER: Duration = Duration::from_secs(30);
/// Shares of the budget, in percent.
pub const FREEZE_AT: u64 = 70;
pub const DISCARD_AT: u64 = 85;
pub const REFUSE_AT: u64 = 100;
/// The budget's ceiling, and its share of the Mac's memory (decision 4).
pub const MAX_BUDGET: u64 = 6 << 30;
pub const BUDGET_SHARE: u64 = 25;
/// About one renderer per this much budget, at least [`MIN_RENDERERS`].
pub const RENDERER_BYTES: u64 = 300 << 20;
pub const MIN_RENDERERS: u32 = 4;
/// Tabs in all. A session's own cap is the `browser.session_tabs` setting.
pub const MAX_TABS: usize = 30;

/// The system's memory pressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pressure {
    Normal,
    Warn,
    Critical,
}

/// A loaded tab, as the governor sees it.
#[derive(Debug, Clone)]
pub struct TabUse {
    pub tab_id: String,
    /// Viewed, driven by an agent, or downloading.
    pub in_use: bool,
    pub frozen: bool,
    /// When it was last viewed or driven.
    pub last_used: Instant,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub freeze: Vec<String>,
    pub discard: Vec<String>,
    /// New tabs are refused until usage falls back.
    pub refuse_new: bool,
}

/// The default budget for a Mac with `ram` bytes of memory.
pub fn default_budget(ram: u64) -> u64 {
    (ram.saturating_mul(BUDGET_SHARE) / 100).min(MAX_BUDGET)
}

/// The Host's budget: the default for this Mac, or [`MAX_BUDGET`] where its
/// memory cannot be read.
pub fn budget() -> u64 {
    #[cfg(target_os = "macos")]
    let ram = sys::ram();
    #[cfg(not(target_os = "macos"))]
    let ram = 0;
    if ram == 0 {
        MAX_BUDGET
    } else {
        default_budget(ram)
    }
}

/// `--renderer-process-limit` for `budget`.
pub fn renderer_limit(budget: u64) -> u32 {
    u32::try_from(budget / RENDERER_BYTES)
        .unwrap_or(u32::MAX)
        .max(MIN_RENDERERS)
}

/// What to do now, with the engine's tree at `usage` bytes of `budget`.
pub fn plan(now: Instant, usage: u64, budget: u64, pressure: Pressure, tabs: &[TabUse]) -> Plan {
    let share = usage.saturating_mul(100) / budget.max(1);
    let mut out = Plan {
        refuse_new: share >= REFUSE_AT || pressure == Pressure::Critical,
        ..Plan::default()
    };
    let mut idle: Vec<&TabUse> = tabs.iter().filter(|t| !t.in_use).collect();
    idle.sort_by_key(|t| t.last_used);
    if pressure == Pressure::Critical {
        out.discard = idle.iter().map(|t| t.tab_id.clone()).collect();
        return out;
    }
    if share >= DISCARD_AT || pressure == Pressure::Warn {
        if let Some(oldest) = idle.first() {
            out.discard.push(oldest.tab_id.clone());
        }
    }
    if share >= FREEZE_AT || pressure == Pressure::Warn {
        out.freeze = idle
            .iter()
            .filter(|t| !t.frozen && now.duration_since(t.last_used) >= FREEZE_AFTER)
            .filter(|t| !out.discard.contains(&t.tab_id))
            .map(|t| t.tab_id.clone())
            .collect();
    }
    out
}

/// The engine's process tree and the system, on macOS.
#[cfg(target_os = "macos")]
pub mod sys {
    use std::ffi::c_void;
    use std::mem::{size_of, MaybeUninit};

    use super::Pressure;

    /// `PROC_PGRP_ONLY` from `<sys/proc_info.h>`.
    const PROC_PGRP_ONLY: u32 = 2;
    const MAX_PIDS: usize = 4096;

    /// The processes in the engine's group: the engine starts in a group of
    /// its own (`launch::spawn`), and its renderers, GPU and utility
    /// processes stay in it.
    pub fn tree(engine: u32) -> Vec<u32> {
        let mut pids = vec![0 as libc::pid_t; MAX_PIDS];
        let bytes = i32::try_from(pids.len() * size_of::<libc::pid_t>()).unwrap_or(i32::MAX);
        // SAFETY: the buffer holds `bytes` bytes, and the call writes at most that.
        let n = unsafe {
            libc::proc_listpids(
                PROC_PGRP_ONLY,
                engine,
                pids.as_mut_ptr().cast::<c_void>(),
                bytes,
            )
        };
        let Ok(n) = usize::try_from(n) else {
            return Vec::new();
        };
        pids.truncate(n / size_of::<libc::pid_t>());
        pids.into_iter()
            .filter_map(|p| u32::try_from(p).ok().filter(|p| *p > 0))
            .collect()
    }

    /// `pid`'s resident memory, in bytes.
    pub fn resident(pid: u32) -> Option<u64> {
        let mut info = MaybeUninit::<libc::proc_taskinfo>::zeroed();
        let size = i32::try_from(size_of::<libc::proc_taskinfo>()).ok()?;
        // SAFETY: `info` is a `proc_taskinfo`, the size the call is given.
        let n = unsafe {
            libc::proc_pidinfo(
                i32::try_from(pid).ok()?,
                libc::PROC_PIDTASKINFO,
                0,
                info.as_mut_ptr().cast::<c_void>(),
                size,
            )
        };
        // SAFETY: a full answer filled it.
        (n == size).then(|| unsafe { info.assume_init() }.pti_resident_size)
    }

    /// The tree's resident memory, in bytes, and how many processes it has.
    pub fn usage(engine: u32) -> (u64, usize) {
        let pids = tree(engine);
        (pids.iter().filter_map(|p| resident(*p)).sum(), pids.len())
    }

    /// The system's memory pressure (`kern.memorystatus_vm_pressure_level`).
    pub fn pressure() -> Pressure {
        let mut level: i32 = 0;
        let mut size = size_of::<i32>();
        // SAFETY: the name is NUL-terminated and `level` is the size given.
        let ok = unsafe {
            libc::sysctlbyname(
                c"kern.memorystatus_vm_pressure_level".as_ptr(),
                (&mut level as *mut i32).cast::<c_void>(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } == 0;
        match level {
            4 if ok => Pressure::Critical,
            2 if ok => Pressure::Warn,
            _ => Pressure::Normal,
        }
    }

    /// The Mac's memory, in bytes.
    pub fn ram() -> u64 {
        let mut ram: u64 = 0;
        let mut size = size_of::<u64>();
        // SAFETY: the name is NUL-terminated and `ram` is the size given.
        let ok = unsafe {
            libc::sysctlbyname(
                c"hw.memsize".as_ptr(),
                (&mut ram as *mut u64).cast::<c_void>(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } == 0;
        if ok {
            ram
        } else {
            0
        }
    }

    /// Give every process of the engine's tree `nice` and, with
    /// `background`, `PRIO_DARWIN_BG` (CPU, disk and network throttled).
    /// A process that went meanwhile is skipped.
    pub fn set_priority(engine: u32, nice: i32, background: bool) {
        let darwin = if background { libc::PRIO_DARWIN_BG } else { 0 };
        for pid in tree(engine) {
            // SAFETY: plain syscalls on a pid; a stale one fails harmlessly.
            unsafe {
                libc::setpriority(libc::PRIO_PROCESS, pid, nice);
                libc::setpriority(libc::PRIO_DARWIN_PROCESS, pid, darwin);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    fn tab(id: &str, in_use: bool, frozen: bool, age: u64, now: Instant) -> TabUse {
        TabUse {
            tab_id: id.into(),
            in_use,
            frozen,
            last_used: now - Duration::from_secs(age),
        }
    }

    #[test]
    fn the_budget_is_a_quarter_of_memory_at_most_six_gigabytes() {
        assert_eq!(default_budget(16 * GB), 4 * GB);
        assert_eq!(default_budget(64 * GB), 6 * GB);
        assert_eq!(renderer_limit(4 * GB), 13);
        assert_eq!(renderer_limit(GB / 2), MIN_RENDERERS);
    }

    #[test]
    fn the_ladder_climbs_with_usage_and_never_touches_tabs_in_use() {
        tokio_test_now(|now| {
            let tabs = [
                tab("viewed", true, false, 600, now),
                tab("old", false, false, 900, now),
                tab("older", false, false, 1200, now),
                tab("fresh", false, false, 10, now),
                tab("frozen", false, true, 1500, now),
            ];
            let at = |share: u64, pressure| plan(now, share * 1000, 100_000, pressure, &tabs);

            assert_eq!(at(50, Pressure::Normal), Plan::default());
            let freeze = at(70, Pressure::Normal);
            assert_eq!(freeze.freeze, vec!["older".to_string(), "old".into()]);
            assert!(freeze.discard.is_empty() && !freeze.refuse_new);

            // The least recently used goes first, frozen or not.
            let discard = at(85, Pressure::Normal);
            assert_eq!(discard.discard, vec!["frozen".to_string()]);
            assert_eq!(discard.freeze, vec!["older".to_string(), "old".into()]);
            assert!(!discard.refuse_new);
            assert_eq!(at(70, Pressure::Warn).discard, vec!["frozen".to_string()]);

            assert!(at(100, Pressure::Normal).refuse_new);
            let critical = at(10, Pressure::Critical);
            assert!(critical.refuse_new);
            assert_eq!(critical.discard, vec!["frozen", "older", "old", "fresh"]);
            assert!(critical.freeze.is_empty());
        });
    }

    fn tokio_test_now(f: impl FnOnce(Instant)) {
        f(Instant::now() + Duration::from_secs(3600));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn this_process_is_measured() {
        let me = std::process::id();
        assert!(sys::resident(me).unwrap() > 1 << 20);
        assert!(sys::ram() > GB);
        // Its own group holds at least the test process when it leads one,
        // and nothing breaks when it does not.
        let _ = sys::tree(me);
        let _ = sys::pressure();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_process_group_is_one_tree_and_takes_its_priority() {
        use std::os::unix::process::CommandExt;
        // A shell leading its own group, with a child in it.
        let mut leader = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30 & wait"])
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = leader.id();
        let mut tree = Vec::new();
        for _ in 0..100 {
            tree = sys::tree(pid);
            if tree.len() >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(tree.contains(&pid) && tree.len() >= 2, "{tree:?}");
        let (bytes, count) = sys::usage(pid);
        assert!(bytes > 0 && count >= 2);

        sys::set_priority(pid, 5, true);
        for p in &tree {
            // SAFETY: reading a priority.
            assert_eq!(unsafe { libc::getpriority(libc::PRIO_PROCESS, *p) }, 5);
        }
        unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        let _ = leader.wait();
    }
}
