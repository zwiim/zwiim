// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: site pauses and login pauses (automation-engine spec 10.2,
//! 10.3) and the token budget (10.4).
//!
//! A pause keys on (site, account), survives restarts in `site_knowledge`,
//! and carries a `cause` shown in the run record and the Sites list. Login
//! expiry is a pause with cause `login`, not a retry.

use serde::{Deserialize, Serialize};

use crate::automation::playbook_store::{PlaybookStore, SiteKnowledgeKind};

/// Why a site is paused (spec 10.2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseCause {
    Captcha,
    Blocked,
    Login,
}

impl PauseCause {
    pub fn as_str(self) -> &'static str {
        match self {
            PauseCause::Captcha => "captcha",
            PauseCause::Blocked => "blocked",
            PauseCause::Login => "login",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "captcha" => PauseCause::Captcha,
            "blocked" => PauseCause::Blocked,
            "login" => PauseCause::Login,
            _ => return None,
        })
    }
}

/// What the Host saw on a lane tab's navigation (spec 10.2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationSignal {
    /// A normal page.
    Ok,
    /// HTTP 403: one fails that item with `page_unrecognised`.
    Forbidden,
    /// HTTP 429: pause the site.
    TooManyRequests,
    /// A recognised block page.
    BlockPage,
    /// A CAPTCHA frame is present.
    Captcha,
    /// The site sent us to its login page.
    LoginRedirect,
}

/// The pause a signal raises, if any. A single 403 does not pause (it fails
/// the item), but three 403s on one site in one run do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationOutcome {
    Continue,
    FailItemPageUnrecognised,
    Pause(PauseCause),
}

/// Track the 403s per (site, account) in a run and decide what a navigation
/// means. `None` when the signal continues normally.
pub fn on_navigation(
    signal: &NavigationSignal,
    site: &str,
    account: &str,
    forbidden_count: &mut std::collections::HashMap<(String, String), u32>,
) -> NavigationOutcome {
    match signal {
        NavigationSignal::Ok => NavigationOutcome::Continue,
        NavigationSignal::Forbidden => {
            let n = forbidden_count
                .entry((site.to_string(), account.to_string()))
                .or_insert(0);
            *n += 1;
            if *n >= 3 {
                NavigationOutcome::Pause(PauseCause::Blocked)
            } else {
                NavigationOutcome::FailItemPageUnrecognised
            }
        }
        NavigationSignal::TooManyRequests | NavigationSignal::BlockPage => {
            NavigationOutcome::Pause(PauseCause::Blocked)
        }
        NavigationSignal::Captcha => NavigationOutcome::Pause(PauseCause::Captcha),
        NavigationSignal::LoginRedirect => NavigationOutcome::Pause(PauseCause::Login),
    }
}

/// Pause state as stored in `site_knowledge` (`kind = pause`); the value is
/// the cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SitePause {
    pub site: String,
    pub account: String,
    pub cause: PauseCause,
}

impl SitePause {
    /// Pause a (site, account) for every playbook until the person resumes.
    pub async fn pause(
        store: &PlaybookStore,
        site: &str,
        account: &str,
        cause: PauseCause,
    ) -> Result<SitePause, crate::db::error::DbError> {
        store
            .set_site_value(site, SiteKnowledgeKind::Pause, account, cause.as_str())
            .await?;
        Ok(SitePause {
            site: site.to_string(),
            account: account.to_string(),
            cause,
        })
    }

    /// Resume: clear the pause. `false` when it was not paused, which the
    /// protocol reports as `site_not_paused`.
    pub async fn resume(
        store: &PlaybookStore,
        site: &str,
        account: &str,
    ) -> Result<bool, crate::db::error::DbError> {
        let paused = SitePause::current(store, site, account).await?;
        if paused.is_none() {
            return Ok(false);
        }
        store
            .clear_site_value(site, SiteKnowledgeKind::Pause, account)
            .await?;
        Ok(true)
    }

    /// The current pause, if any. A blank value is "none".
    pub async fn current(
        store: &PlaybookStore,
        site: &str,
        account: &str,
    ) -> Result<Option<SitePause>, crate::db::error::DbError> {
        let value = store
            .site_value(site, SiteKnowledgeKind::Pause, account)
            .await?;
        Ok(value
            .filter(|v| !v.is_empty())
            .and_then(|v| PauseCause::parse(&v))
            .map(|cause| SitePause {
                site: site.to_string(),
                account: account.to_string(),
                cause,
            }))
    }
}

/// The playbook's optional token budget, copied to each run (spec 10.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenBudget {
    pub tokens: u64,
}

impl TokenBudget {
    /// Whether the run must pause now: the budget is reached. In-flight
    /// items finish first, so a run can pass its budget by what is in flight.
    pub fn reached(&self, used: u64) -> bool {
        used >= self.tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::playbook_store::PlaybookStore;
    use crate::db::migration::Migrator;
    use sea_orm::{Database, DatabaseConnection};
    use sea_orm_migration::MigratorTrait;

    async fn store() -> PlaybookStore {
        let db: DatabaseConnection = Database::connect("sqlite::memory:").await.unwrap();
        Migrator::up(&db, None).await.unwrap();
        PlaybookStore::new(db)
    }

    #[test]
    fn one_403_fails_the_item_and_three_pause_the_site() {
        let mut seen = Default::default();
        let site = "indeed.com";
        let acct = "main";
        assert_eq!(
            on_navigation(&NavigationSignal::Forbidden, site, acct, &mut seen),
            NavigationOutcome::FailItemPageUnrecognised
        );
        assert_eq!(
            on_navigation(&NavigationSignal::Forbidden, site, acct, &mut seen),
            NavigationOutcome::FailItemPageUnrecognised
        );
        assert_eq!(
            on_navigation(&NavigationSignal::Forbidden, site, acct, &mut seen),
            NavigationOutcome::Pause(PauseCause::Blocked)
        );
        // Another site starts its own count.
        assert_eq!(
            on_navigation(&NavigationSignal::Forbidden, "x.com", acct, &mut seen),
            NavigationOutcome::FailItemPageUnrecognised
        );
    }

    #[test]
    fn signals_map_to_the_right_pause_cause() {
        let mut seen = Default::default();
        let mut f = |s| on_navigation(&s, "a.com", "main", &mut seen);
        assert_eq!(f(NavigationSignal::Ok), NavigationOutcome::Continue);
        assert_eq!(
            f(NavigationSignal::TooManyRequests),
            NavigationOutcome::Pause(PauseCause::Blocked)
        );
        assert_eq!(
            f(NavigationSignal::BlockPage),
            NavigationOutcome::Pause(PauseCause::Blocked)
        );
        assert_eq!(
            f(NavigationSignal::Captcha),
            NavigationOutcome::Pause(PauseCause::Captcha)
        );
        assert_eq!(
            f(NavigationSignal::LoginRedirect),
            NavigationOutcome::Pause(PauseCause::Login)
        );
    }

    #[tokio::test]
    async fn a_pause_survives_a_restart_and_resume_clears_it() {
        let s = store().await;
        assert!(SitePause::current(&s, "x.com", "main")
            .await
            .unwrap()
            .is_none());
        SitePause::pause(&s, "x.com", "main", PauseCause::Captcha)
            .await
            .unwrap();
        // A fresh handle against the same database still sees it.
        let again = SitePause::current(&s, "x.com", "main").await.unwrap();
        assert_eq!(again.unwrap().cause, PauseCause::Captcha);
        // Resume is per account.
        assert!(!SitePause::resume(&s, "x.com", "second").await.unwrap());
        assert!(SitePause::resume(&s, "x.com", "main").await.unwrap());
        assert!(SitePause::current(&s, "x.com", "main")
            .await
            .unwrap()
            .is_none());
        assert!(!SitePause::resume(&s, "x.com", "main").await.unwrap());
    }

    #[test]
    fn a_budget_is_reached_at_its_limit() {
        let b = TokenBudget { tokens: 1000 };
        assert!(!b.reached(999));
        assert!(b.reached(1000));
        assert!(b.reached(1001));
    }

    #[test]
    fn causes_round_trip() {
        for c in [PauseCause::Captcha, PauseCause::Blocked, PauseCause::Login] {
            assert_eq!(PauseCause::parse(c.as_str()), Some(c));
        }
    }
}
