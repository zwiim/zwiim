// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: caps and pace (automation-engine spec 10.1).
//!
//! Caps key on (site, account, action kind) with a local-midnight day, so a
//! DST change or a time-zone move can neither merge two days nor create a
//! third (spec 6.7.7). The person may lower a cap, never raise it. Pace is a
//! per-lane clock with floors that can only slow down.

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::automation::reasons::ActionKind;

/// The default caps in spec 10.1 [est]. A playbook may lower them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps {
    /// Per (site, account), every outward action together.
    pub site_daily: i32,
    /// Per run.
    pub run_items: i32,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            site_daily: 25,
            run_items: 50,
        }
    }
}

impl Caps {
    /// The effective cap for a kind on a site: `submit_application` uses the
    /// site cap, other kinds take the smaller of the kind cap and the site
    /// cap. `None` means no per-kind cap for this kind.
    pub fn for_kind(&self, kind: ActionKind) -> Option<i32> {
        kind.daily_cap()
            .map(|kind_cap| kind_cap.min(self.site_daily))
    }

    /// Lower only: a request to raise above the defaults is clamped to them
    /// and reported (spec 10.1.4).
    pub fn lowered(self, request: Caps) -> (Caps, bool) {
        let d = Caps::default();
        let clamped = Caps {
            site_daily: request.site_daily.min(self.site_daily).min(d.site_daily),
            run_items: request.run_items.min(self.run_items).min(d.run_items),
        };
        let raised =
            clamped.site_daily != request.site_daily || clamped.run_items != request.run_items;
        (clamped, raised)
    }

    pub fn is_valid(&self) -> bool {
        self.site_daily >= 0 && self.run_items >= 1
    }
}

/// The local calendar day a moment falls in, as `YYYY-MM-DD` (spec 6.7.7).
/// Uses the Host's local zone; tested across a DST change and a zone move.
pub fn local_day(at: DateTime<Utc>, zone: &chrono_tz::Tz) -> String {
    at.with_timezone(zone).date_naive().to_string()
}

/// Same as [`local_day`] for the machine's own zone.
pub fn local_day_here(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).date_naive().to_string()
}

/// The local-midnight boundary for `day` in `zone`, as UTC.
pub fn local_midnight_utc(day: NaiveDate, zone: &chrono_tz::Tz) -> Option<DateTime<Utc>> {
    let naive = day.and_hms_opt(0, 0, 0)?;
    zone.from_local_datetime(&naive)
        .earliest()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Pace floors per site (spec 10.1.3): at least 3 s between actions, 30 s
/// between items. Can be slowed, never sped up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pace {
    pub between_actions_secs: u64,
    pub between_items_secs: u64,
}

impl Default for Pace {
    fn default() -> Self {
        Self {
            between_actions_secs: 3,
            between_items_secs: 30,
        }
    }
}

impl Pace {
    /// A requested pace clamped to the floors: slower is allowed, faster is
    /// raised back to the default.
    pub fn slower_only(self) -> Pace {
        let d = Pace::default();
        Pace {
            between_actions_secs: self.between_actions_secs.max(d.between_actions_secs),
            between_items_secs: self.between_items_secs.max(d.between_items_secs),
        }
    }

    /// How long the lane must still wait before its next action at `since`.
    pub fn wait_before_action(
        &self,
        since: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> std::time::Duration {
        wait_for(since, now, self.between_actions_secs)
    }

    pub fn wait_before_item(
        &self,
        since: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> std::time::Duration {
        wait_for(since, now, self.between_items_secs)
    }
}

fn wait_for(since: DateTime<Utc>, now: DateTime<Utc>, secs: u64) -> std::time::Duration {
    let elapsed = (now - since).num_seconds().max(0) as u64;
    std::time::Duration::from_secs(secs.saturating_sub(elapsed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn a_local_day_is_the_calendar_day_in_its_zone() {
        let ny: chrono_tz::Tz = "America/New_York".parse().unwrap();
        // 03:00Z on the 20th is 23:00 on the 19th in New York.
        assert_eq!(local_day(utc("2026-11-20T03:00:00Z"), &ny), "2026-11-19");
        // 05:00Z is midnight EST on the 20th.
        assert_eq!(local_day(utc("2026-11-20T04:59:59Z"), &ny), "2026-11-19");
        assert_eq!(local_day(utc("2026-11-20T05:00:00Z"), &ny), "2026-11-20");
    }

    #[test]
    fn a_dst_change_keeps_one_day_and_moves_its_midnight() {
        let ny: chrono_tz::Tz = "America/New_York".parse().unwrap();
        let day: NaiveDate = "2026-03-08".parse().unwrap();
        // Midnight EST = 05:00Z; the day is 23 hours long.
        let start = local_midnight_utc(day, &ny).unwrap();
        assert_eq!(start, utc("2026-03-08T05:00:00Z"));
        let end = local_midnight_utc("2026-03-09".parse().unwrap(), &ny).unwrap();
        assert_eq!((end - start).num_hours(), 23);
        // A moment just before and just after the boundary is the right day.
        assert_eq!(local_day(end - TimeDelta::seconds(1), &ny), "2026-03-08");
        assert_eq!(local_day(end, &ny), "2026-03-09");
    }

    #[test]
    fn a_time_zone_move_changes_the_day_a_moment_falls_in() {
        let ny: chrono_tz::Tz = "America/New_York".parse().unwrap();
        let tokyo: chrono_tz::Tz = "Asia/Tokyo".parse().unwrap();
        let t = utc("2026-11-20T03:00:00Z");
        assert_eq!(local_day(t, &ny), "2026-11-19");
        assert_eq!(local_day(t, &tokyo), "2026-11-20");
    }

    #[test]
    fn caps_can_be_lowered_never_raised() {
        let (lowered, raised) = Caps::default().lowered(Caps {
            site_daily: 10,
            run_items: 20,
        });
        assert_eq!(lowered.site_daily, 10);
        assert!(!raised);

        let (clamped, raised) = Caps::default().lowered(Caps {
            site_daily: 100,
            run_items: 500,
        });
        assert_eq!(clamped, Caps::default());
        assert!(raised);
    }

    #[test]
    fn per_kind_caps_take_the_smaller_of_kind_and_site() {
        let caps = Caps {
            site_daily: 2,
            run_items: 50,
        };
        // publish_post is 3 by kind, 2 by site: 2 wins.
        assert_eq!(caps.for_kind(ActionKind::PublishPost), Some(2));
        let caps = Caps {
            site_daily: 25,
            run_items: 50,
        };
        assert_eq!(caps.for_kind(ActionKind::PublishPost), Some(3));
        assert_eq!(caps.for_kind(ActionKind::SubmitApplication), None);
    }

    #[test]
    fn pace_only_slows_and_counts_down() {
        let pace = Pace {
            between_actions_secs: 1,
            between_items_secs: 1,
        }
        .slower_only();
        assert_eq!(pace.between_actions_secs, 3);
        assert_eq!(pace.between_items_secs, 30);

        let now = utc("2026-11-20T00:00:00Z");
        assert_eq!(
            pace.wait_before_action(now - TimeDelta::seconds(1), now),
            std::time::Duration::from_secs(2)
        );
        assert_eq!(
            pace.wait_before_action(now - TimeDelta::seconds(10), now),
            std::time::Duration::ZERO
        );
    }

    #[test]
    fn a_zero_site_cap_is_valid_but_no_slots() {
        let caps = Caps {
            site_daily: 0,
            run_items: 1,
        };
        assert!(caps.is_valid());
        assert_eq!(caps.for_kind(ActionKind::Comment), Some(0));
    }
}
