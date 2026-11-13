//! Relay usage per account. The relay adds every message's size to its
//! account's counter here; once a minute the counters go to the usage store
//! in one batch, and the store answers with each account's plan, its usage
//! this period and its limits. Only byte counts leave this module: never
//! content, addresses or URLs.
//!
//! Fail-open: when the store is unreachable the bytes stay pending and are
//! sent with the next batch, and limits are judged on the last known totals.
//! An account the store has never answered for has no limits, which is also
//! what a Cloud without a store gives every account.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;

use crate::hub;

/// How often pending bytes go to the store.
pub const SYNC_INTERVAL: Duration = Duration::from_secs(60);

/// The kind of traffic a relay connection carries, counted and limited
/// separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    Control,
    Browser,
}

impl Channel {
    pub const ALL: [Channel; 2] = [Channel::Control, Channel::Browser];

    fn index(self) -> usize {
        self as usize
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Control => "control",
            Channel::Browser => "browser",
        }
    }
}

/// A plan's limits; `None` is unlimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlanLimits {
    /// Browser bytes per period.
    pub browser: Option<u64>,
    /// Bytes per period over every channel.
    pub total: Option<u64>,
    /// Sustained `control` bytes per second.
    pub control_rate: Option<u64>,
}

/// One account as the store reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub user_id: String,
    pub plan_id: String,
    pub plan_name: String,
    /// Unix seconds.
    pub period_start: u64,
    /// Unix seconds; the period's usage stops counting from here.
    pub resets_at: u64,
    /// Bytes this period, indexed like [`Channel::ALL`].
    pub used: [u64; 2],
    pub limits: PlanLimits,
}

impl Account {
    pub fn used(&self, channel: Channel) -> u64 {
        self.used[channel.index()]
    }

    pub fn total(&self) -> u64 {
        self.used.iter().fold(0, |sum, n| sum.saturating_add(*n))
    }
}

/// Bytes to add for one account and channel; zero only reads the totals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub user_id: String,
    pub channel: Channel,
    pub bytes: u64,
}

/// Where usage is kept across restarts and Cloud instances.
pub trait Store: Send + Sync {
    /// Add a batch and return every account in it the store knows, with its
    /// totals after the batch.
    fn add(&self, batch: Vec<Entry>) -> BoxFuture<'_, Result<Vec<Account>, String>>;
}

#[derive(Default)]
struct Counter {
    /// Not yet accepted by the store.
    pending: [u64; 2],
    /// The store's last answer.
    known: Option<Account>,
}

pub struct Usage {
    store: Option<Arc<dyn Store>>,
    accounts: Mutex<HashMap<String, Counter>>,
}

impl Usage {
    pub fn new(store: Option<Arc<dyn Store>>) -> Self {
        Self { store, accounts: Mutex::default() }
    }

    /// Count `bytes` relayed for `user_id` on `channel`.
    pub fn add(&self, user_id: &str, channel: Channel, bytes: u64) {
        let mut accounts = self.accounts.lock().unwrap();
        let counter = match accounts.get_mut(user_id) {
            Some(counter) => counter,
            None => accounts.entry(user_id.to_string()).or_default(),
        };
        let pending = &mut counter.pending[channel.index()];
        *pending = pending.saturating_add(bytes);
    }

    /// The account's plan and usage now, including bytes not yet synced.
    /// `None` when the store has never answered for it: no limits apply.
    pub fn account(&self, user_id: &str) -> Option<Account> {
        let accounts = self.accounts.lock().unwrap();
        let counter = accounts.get(user_id)?;
        let mut account = counter.known.clone()?;
        // The period ended: its usage no longer counts, so a block lifts
        // without waiting for the next sync to report the new period.
        if hub::unix_now() >= account.resets_at {
            account.used = [0; 2];
        }
        for channel in Channel::ALL {
            let i = channel.index();
            account.used[i] = account.used[i].saturating_add(counter.pending[i]);
        }
        Some(account)
    }

    /// Fetch the account's totals from the store if it has none yet, as on
    /// its first ticket after a Cloud start. A failure leaves it unlimited.
    pub async fn load(&self, user_id: &str) {
        let Some(store) = &self.store else { return };
        let loaded = self.accounts.lock().unwrap().get(user_id).is_some_and(|c| c.known.is_some());
        if loaded {
            return;
        }
        let read = Entry { user_id: user_id.to_string(), channel: Channel::Control, bytes: 0 };
        match store.add(vec![read]).await {
            Ok(rows) => self.apply(rows),
            Err(e) => tracing::warn!("[cloud] usage store unavailable, account not limited: {e}"),
        }
    }

    /// Send pending bytes to the store. On failure they stay pending.
    pub async fn sync(&self) {
        let Some(store) = &self.store else { return };
        let batch: Vec<Entry> = {
            let mut accounts = self.accounts.lock().unwrap();
            accounts.retain(|_, c| c.known.is_some() || c.pending.iter().any(|n| *n > 0));
            accounts
                .iter_mut()
                .flat_map(|(user_id, counter)| {
                    let pending = std::mem::take(&mut counter.pending);
                    Channel::ALL.into_iter().filter(move |c| pending[c.index()] > 0).map(move |channel| Entry {
                        user_id: user_id.clone(),
                        channel,
                        bytes: pending[channel.index()],
                    })
                })
                .collect()
        };
        if batch.is_empty() {
            return;
        }
        match store.add(batch.clone()).await {
            Ok(rows) => self.apply(rows),
            Err(e) => {
                tracing::warn!("[cloud] usage sync failed, retrying next minute: {e}");
                for entry in batch {
                    self.add(&entry.user_id, entry.channel, entry.bytes);
                }
            }
        }
    }

    fn apply(&self, rows: Vec<Account>) {
        let mut accounts = self.accounts.lock().unwrap();
        for row in rows {
            let counter = accounts.entry(row.user_id.clone()).or_default();
            counter.known = Some(row);
        }
    }

    /// Sync every `interval` until shutdown, then once more.
    pub async fn run(self: Arc<Self>, interval: Duration, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        if self.store.is_none() {
            return;
        }
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => self.sync().await,
                _ = hub::stopped(&mut shutdown) => break,
            }
        }
        self.sync().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A store in memory: knows `users`, fails while `down` is set.
    #[derive(Default)]
    struct Fake {
        users: Vec<String>,
        bytes: Mutex<HashMap<(String, Channel), u64>>,
        calls: Mutex<Vec<Vec<Entry>>>,
        down: AtomicBool,
        resets_at: u64,
    }

    impl Fake {
        fn knowing(users: &[&str]) -> Arc<Self> {
            Self::resetting(users, hub::unix_now() + 3600)
        }
        fn resetting(users: &[&str], resets_at: u64) -> Arc<Self> {
            Arc::new(Self { users: users.iter().map(|u| u.to_string()).collect(), resets_at, ..Default::default() })
        }
        fn stored(&self, user: &str, channel: Channel) -> u64 {
            *self.bytes.lock().unwrap().get(&(user.to_string(), channel)).unwrap_or(&0)
        }
    }

    impl Store for Fake {
        fn add(&self, batch: Vec<Entry>) -> BoxFuture<'_, Result<Vec<Account>, String>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(batch.clone());
                if self.down.load(Ordering::SeqCst) {
                    return Err("down".into());
                }
                let mut bytes = self.bytes.lock().unwrap();
                let mut users: Vec<String> = Vec::new();
                for e in batch.into_iter().filter(|e| self.users.contains(&e.user_id)) {
                    *bytes.entry((e.user_id.clone(), e.channel)).or_default() += e.bytes;
                    if !users.contains(&e.user_id) {
                        users.push(e.user_id);
                    }
                }
                Ok(users
                    .into_iter()
                    .map(|user_id| Account {
                        used: Channel::ALL.map(|c| *bytes.get(&(user_id.clone(), c)).unwrap_or(&0)),
                        user_id,
                        plan_id: "free".into(),
                        plan_name: "Free".into(),
                        period_start: 0,
                        resets_at: self.resets_at,
                        limits: PlanLimits { browser: Some(5), total: Some(10), control_rate: Some(1) },
                    })
                    .collect())
            })
        }
    }

    fn usage(store: &Arc<Fake>) -> Usage {
        Usage::new(Some(store.clone() as Arc<dyn Store>))
    }

    #[tokio::test]
    async fn bytes_reach_the_store_in_one_batch_and_add_to_known_totals() {
        let store = Fake::knowing(&["a"]);
        let usage = usage(&store);
        usage.add("a", Channel::Browser, 100);
        usage.add("a", Channel::Browser, 50);
        usage.add("a", Channel::Control, 7);
        assert_eq!(usage.account("a"), None, "unknown until the store answers");

        usage.sync().await;
        assert_eq!(store.calls.lock().unwrap().len(), 1);
        assert_eq!(store.stored("a", Channel::Browser), 150);

        usage.add("a", Channel::Browser, 1);
        let account = usage.account("a").unwrap();
        assert_eq!(account.used(Channel::Browser), 151, "synced plus pending");
        assert_eq!(account.total(), 158);
        assert_eq!(account.limits.browser, Some(5));

        usage.sync().await;
        assert_eq!(store.stored("a", Channel::Browser), 151, "pending is sent once");
    }

    #[tokio::test]
    async fn a_failed_sync_keeps_the_bytes_for_the_next_one() {
        let store = Fake::knowing(&["a"]);
        let usage = usage(&store);
        usage.add("a", Channel::Browser, 10);
        usage.sync().await;

        store.down.store(true, Ordering::SeqCst);
        usage.add("a", Channel::Browser, 5);
        usage.sync().await;
        assert_eq!(usage.account("a").unwrap().used(Channel::Browser), 15, "last known plus pending");

        store.down.store(false, Ordering::SeqCst);
        usage.sync().await;
        assert_eq!(store.stored("a", Channel::Browser), 15);
        assert_eq!(usage.account("a").unwrap().used(Channel::Browser), 15);
    }

    #[tokio::test]
    async fn an_account_the_store_does_not_know_stays_unlimited() {
        let store = Fake::knowing(&[]);
        let usage = usage(&store);
        usage.add("stranger", Channel::Browser, 10);
        usage.sync().await;
        assert_eq!(usage.account("stranger"), None);
        usage.sync().await;
        assert_eq!(store.calls.lock().unwrap().len(), 1, "its bytes are not resent");
    }

    #[tokio::test]
    async fn load_reads_totals_once_without_adding() {
        let store = Fake::knowing(&["a"]);
        store.bytes.lock().unwrap().insert(("a".into(), Channel::Browser), 40);
        let usage = usage(&store);
        usage.load("a").await;
        usage.load("a").await;
        assert_eq!(usage.account("a").unwrap().used(Channel::Browser), 40);
        let calls = store.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0][0].bytes, 0);
    }

    #[tokio::test]
    async fn load_failing_leaves_the_account_unlimited() {
        let store = Fake::knowing(&["a"]);
        store.down.store(true, Ordering::SeqCst);
        let usage = usage(&store);
        usage.load("a").await;
        assert_eq!(usage.account("a"), None);
    }

    #[tokio::test]
    async fn usage_from_an_ended_period_stops_counting() {
        let store = Fake::resetting(&["a"], hub::unix_now() - 1);
        let usage = usage(&store);
        usage.add("a", Channel::Browser, 10);
        usage.sync().await;
        usage.add("a", Channel::Browser, 3);
        assert_eq!(usage.account("a").unwrap().used(Channel::Browser), 3, "only bytes since the reset");
    }

    #[tokio::test]
    async fn without_a_store_nothing_is_limited() {
        let usage = Usage::new(None);
        usage.add("a", Channel::Browser, 10);
        usage.sync().await;
        usage.load("a").await;
        assert_eq!(usage.account("a"), None);
    }
}
