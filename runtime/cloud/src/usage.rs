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

// ─── Postgres store ──────────────────────────────────────────────────────

const STORE_TIMEOUT: Duration = Duration::from_secs(10);

const ADD_SQL: &str = "select user_id::text, plan_id, plan_name, \
    extract(epoch from period_start)::bigint, extract(epoch from resets_at)::bigint, \
    control_used, browser_used, browser_limit, total_limit, control_rate \
    from public.add_relay_usage($1::jsonb)";

/// The usage store in Postgres (the hosted Cloud's Supabase project),
/// reached as the `zwiim_cloud` role, which may only call
/// `add_relay_usage` (`runtime/cloud/supabase/migrations/`). One connection,
/// opened on first use and again after any failure.
pub struct PgStore {
    config: tokio_postgres::Config,
    tls: tokio_postgres_rustls::MakeRustlsConnect,
    client: tokio::sync::Mutex<Option<tokio_postgres::Client>>,
}

impl PgStore {
    /// TLS is required unless the database is on this machine. Server
    /// certificates are checked against `ca` (a PEM file) when given, else
    /// the system's roots.
    pub fn new(mut config: tokio_postgres::Config, ca: Option<&std::path::Path>) -> Result<Self, String> {
        use rustls_pki_types::pem::PemObject;
        use rustls_pki_types::CertificateDer;
        use tokio_postgres::config::{Host, SslMode};

        let local = config.get_hosts().iter().all(|host| match host {
            Host::Tcp(name) => matches!(name.as_str(), "localhost" | "127.0.0.1" | "::1"),
            Host::Unix(_) => true,
        });
        if !local {
            config.ssl_mode(SslMode::Require);
        }
        let mut roots = rustls::RootCertStore::empty();
        match ca {
            Some(path) => {
                for cert in CertificateDer::pem_file_iter(path).map_err(|e| format!("{}: {e}", path.display()))? {
                    let cert = cert.map_err(|e| format!("{}: {e}", path.display()))?;
                    roots.add(cert).map_err(|e| format!("{}: {e}", path.display()))?;
                }
            }
            None => {
                for cert in rustls_native_certs::load_native_certs().certs {
                    let _ = roots.add(cert);
                }
            }
        }
        if roots.is_empty() && !local {
            return Err("no CA certificates to check the usage database with".into());
        }
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(Self {
            config,
            tls: tokio_postgres_rustls::MakeRustlsConnect::new(tls),
            client: tokio::sync::Mutex::default(),
        })
    }

    async fn query(&self, batch: &serde_json::Value) -> Result<Vec<tokio_postgres::Row>, String> {
        let mut client = self.client.lock().await;
        let result = tokio::time::timeout(STORE_TIMEOUT, async {
            if client.as_ref().is_none_or(|c| c.is_closed()) {
                let (fresh, connection) = self.config.connect(self.tls.clone()).await?;
                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        tracing::warn!("[cloud] usage database connection ended: {e}");
                    }
                });
                *client = Some(fresh);
            }
            client.as_ref().expect("connected").query(ADD_SQL, &[batch]).await
        })
        .await;
        match result {
            Ok(Ok(rows)) => Ok(rows),
            Ok(Err(e)) => {
                *client = None;
                Err(e.to_string())
            }
            Err(_) => {
                *client = None;
                Err("timed out".into())
            }
        }
    }
}

fn account_from_row(row: &tokio_postgres::Row) -> Result<Account, String> {
    let count = |i: usize| -> Result<u64, String> {
        let n: i64 = row.try_get(i).map_err(|e| e.to_string())?;
        u64::try_from(n).map_err(|_| format!("negative value in column {i}"))
    };
    let limit = |i: usize| -> Result<Option<u64>, String> {
        let n: Option<i64> = row.try_get(i).map_err(|e| e.to_string())?;
        n.map(|n| u64::try_from(n).map_err(|_| format!("negative value in column {i}"))).transpose()
    };
    let text = |i: usize| -> Result<String, String> { row.try_get(i).map_err(|e| e.to_string()) };
    Ok(Account {
        user_id: text(0)?,
        plan_id: text(1)?,
        plan_name: text(2)?,
        period_start: count(3)?,
        resets_at: count(4)?,
        used: [count(5)?, count(6)?],
        limits: PlanLimits { browser: limit(7)?, total: limit(8)?, control_rate: limit(9)? },
    })
}

impl Store for PgStore {
    fn add(&self, batch: Vec<Entry>) -> BoxFuture<'_, Result<Vec<Account>, String>> {
        Box::pin(async move {
            let batch = serde_json::Value::Array(
                batch
                    .iter()
                    .map(|e| serde_json::json!({ "user_id": e.user_id, "channel": e.channel.as_str(), "bytes": e.bytes }))
                    .collect(),
            );
            self.query(&batch).await?.iter().map(account_from_row).collect()
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A store in memory: knows `users`, fails while `down` is set.
    #[derive(Default)]
    pub(crate) struct Fake {
        users: Vec<String>,
        bytes: Mutex<HashMap<(String, Channel), u64>>,
        calls: Mutex<Vec<Vec<Entry>>>,
        down: AtomicBool,
        resets_at: u64,
    }

    impl Fake {
        pub(crate) fn knowing(users: &[&str]) -> Arc<Self> {
            Self::resetting(users, hub::unix_now() + 3600)
        }
        fn resetting(users: &[&str], resets_at: u64) -> Arc<Self> {
            Arc::new(Self { users: users.iter().map(|u| u.to_string()).collect(), resets_at, ..Default::default() })
        }
        pub(crate) fn stored(&self, user: &str, channel: Channel) -> u64 {
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

    /// Against a database with the migration applied, as the `zwiim_cloud`
    /// role: `ZWIIM_CLOUD_TEST_DATABASE_URL` and an existing account in
    /// `ZWIIM_CLOUD_TEST_USER_ID`. Adds 5 control bytes to that account.
    #[tokio::test]
    #[ignore = "needs a Postgres with the usage migration"]
    async fn pg_store_adds_and_reads_back() {
        let url = std::env::var("ZWIIM_CLOUD_TEST_DATABASE_URL").expect("ZWIIM_CLOUD_TEST_DATABASE_URL");
        let user = std::env::var("ZWIIM_CLOUD_TEST_USER_ID").expect("ZWIIM_CLOUD_TEST_USER_ID");
        let store = PgStore::new(url.parse().expect("database URL"), None).unwrap();
        let read = |bytes| vec![Entry { user_id: user.clone(), channel: Channel::Control, bytes }];
        let before = store.add(read(0)).await.unwrap();
        assert_eq!(before.len(), 1);
        let after = store.add(read(5)).await.unwrap();
        assert_eq!(after[0].used(Channel::Control), before[0].used(Channel::Control) + 5);
        assert_eq!(after[0].plan_id, "free");
        assert!(after[0].resets_at > after[0].period_start);
        let stranger = Entry { user_id: "not-a-uuid".into(), channel: Channel::Browser, bytes: 1 };
        assert!(store.add(vec![stranger]).await.unwrap().is_empty());
    }
}
