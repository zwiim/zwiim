//! In-memory, short-lived Cloud state: one-time tokens (enrollment tokens,
//! connection tickets, rendezvous tokens) and host presence. Nothing here
//! survives a restart, by design — hosts reconnect and devices ask again.

use std::collections::HashMap;
use std::hash::Hash;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::proto::ControlFromCloud;

/// Outstanding one-time tokens per kind; beyond it issuing fails until some
/// expire.
pub const MAX_OUTSTANDING: usize = 10_000;

/// Keys a [`PerMinute`] tracks; a new key beyond it is refused until old
/// windows lapse.
const MAX_TRACKED: usize = 100_000;
const MINUTE: Duration = Duration::from_secs(60);

/// Allows `limit` events per key per minute, in fixed windows.
pub struct PerMinute<K> {
    limit: usize,
    seen: Mutex<HashMap<K, (Instant, usize)>>,
}

impl<K: Eq + Hash> PerMinute<K> {
    pub fn new(limit: usize) -> Self {
        Self { limit, seen: Mutex::default() }
    }

    pub fn allow(&self, key: K) -> bool {
        let now = Instant::now();
        let mut seen = self.seen.lock().unwrap();
        if seen.len() >= MAX_TRACKED && !seen.contains_key(&key) {
            seen.retain(|_, (started, _)| now.duration_since(*started) < MINUTE);
            if seen.len() >= MAX_TRACKED {
                return false;
            }
        }
        let (started, count) = seen.entry(key).or_insert((now, 0));
        if now.duration_since(*started) >= MINUTE {
            (*started, *count) = (now, 0);
        }
        *count += 1;
        *count <= self.limit
    }
}

/// The address a client is limited under, if it can be told apart: an IPv6
/// client controls a whole /64, so that is one client. A loopback peer is a
/// tunnel or proxy on this machine carrying every client at once, so it is
/// not limited as one.
pub fn client_key(ip: IpAddr) -> Option<IpAddr> {
    let ip = ip.to_canonical();
    if ip.is_loopback() {
        return None;
    }
    Some(match ip {
        IpAddr::V6(v6) => {
            let [a, b, c, d, ..] = v6.segments();
            IpAddr::V6(Ipv6Addr::new(a, b, c, d, 0, 0, 0, 0))
        }
        v4 => v4,
    })
}

pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Resolves once `shutdown` is set (or its sender is gone).
pub async fn stopped(shutdown: &mut tokio::sync::watch::Receiver<bool>) {
    let _ = shutdown.wait_for(|stop| *stop).await;
}

fn digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// Single-use bearer values with a deadline. Only their SHA-256 is kept, so
/// the table itself is useless to whoever reads it.
pub struct OneTime<T> {
    entries: Mutex<HashMap<[u8; 32], (T, Instant)>>,
}

impl<T> Default for OneTime<T> {
    fn default() -> Self {
        Self { entries: Mutex::default() }
    }
}

impl<T> OneTime<T> {
    /// Issue a token for `value`, valid for `ttl`. `None` when too many are
    /// outstanding.
    pub fn issue(&self, value: T, ttl: Duration) -> Option<String> {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();
        if entries.len() >= MAX_OUTSTANDING {
            entries.retain(|_, (_, deadline)| *deadline > now);
            if entries.len() >= MAX_OUTSTANDING {
                return None;
            }
        }
        let token = random_token(32);
        entries.insert(digest(&token), (value, now + ttl));
        Some(token)
    }

    /// Redeem a token: its value if it was issued, unused and unexpired.
    /// Redeeming consumes it either way.
    pub fn take(&self, token: &str) -> Option<T> {
        let (value, deadline) = self.entries.lock().unwrap().remove(&digest(token))?;
        (Instant::now() < deadline).then_some(value)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

/// A host's live control connection.
struct Online {
    conn: u64,
    control: mpsc::Sender<ControlFromCloud>,
}

/// Which hosts are connected, and when each was last heard from.
#[derive(Default)]
pub struct Presence {
    online: Mutex<HashMap<String, Online>>,
    last_seen: Mutex<HashMap<String, u64>>,
}

impl Presence {
    /// Mark `host_id` online on connection `conn`, replacing (and thereby
    /// closing) any previous connection.
    pub fn connect(&self, host_id: &str, conn: u64, control: mpsc::Sender<ControlFromCloud>) {
        self.online
            .lock()
            .unwrap()
            .insert(host_id.to_string(), Online { conn, control });
        self.touch(host_id);
    }

    pub fn touch(&self, host_id: &str) {
        self.last_seen
            .lock()
            .unwrap()
            .insert(host_id.to_string(), unix_now());
    }

    /// Mark `host_id` offline, unless a newer connection replaced `conn`.
    pub fn disconnect(&self, host_id: &str, conn: u64) {
        let mut online = self.online.lock().unwrap();
        if online.get(host_id).is_some_and(|o| o.conn == conn) {
            online.remove(host_id);
        }
    }

    pub fn control(&self, host_id: &str) -> Option<mpsc::Sender<ControlFromCloud>> {
        self.online
            .lock()
            .unwrap()
            .get(host_id)
            .map(|o| o.control.clone())
    }

    pub fn is_online(&self, host_id: &str) -> bool {
        self.online.lock().unwrap().contains_key(host_id)
    }

    pub fn last_seen(&self, host_id: &str) -> Option<u64> {
        self.last_seen.lock().unwrap().get(host_id).copied()
    }
}

/// A device waiting for its host to dial in.
pub struct Pending<S> {
    pub host_id: String,
    token: [u8; 32],
    deadline: Instant,
    pub deliver: tokio::sync::oneshot::Sender<S>,
}

/// Rendezvous between a device socket and the host socket answering its
/// dial. Ids are random and only ever sent to the host on its authenticated
/// control connection; each carries its own one-time token.
pub struct Rendezvous<S> {
    pending: Mutex<HashMap<String, Pending<S>>>,
}

impl<S> Default for Rendezvous<S> {
    fn default() -> Self {
        Self { pending: Mutex::default() }
    }
}

impl<S> Rendezvous<S> {
    /// Open a rendezvous for `host_id`: its id, its token, and where the
    /// host's socket arrives.
    pub fn open(
        &self,
        host_id: &str,
        ttl: Duration,
    ) -> Option<(String, String, tokio::sync::oneshot::Receiver<S>)> {
        let now = Instant::now();
        let mut pending = self.pending.lock().unwrap();
        if pending.len() >= MAX_OUTSTANDING {
            pending.retain(|_, p| p.deadline > now && !p.deliver.is_closed());
            if pending.len() >= MAX_OUTSTANDING {
                return None;
            }
        }
        let (id, token) = (random_token(16), random_token(32));
        let (deliver, arrived) = tokio::sync::oneshot::channel();
        pending.insert(
            id.clone(),
            Pending { host_id: host_id.to_string(), token: digest(&token), deadline: now + ttl, deliver },
        );
        Some((id, token, arrived))
    }

    pub fn cancel(&self, id: &str) {
        self.pending.lock().unwrap().remove(id);
    }

    /// Join rendezvous `id` presenting `token`. Consumed whether or not the
    /// token matches, so a wrong guess burns it.
    pub fn join(&self, id: &str, token: &str) -> Option<Pending<S>> {
        use subtle::ConstantTimeEq;
        let pending = self.pending.lock().unwrap().remove(id)?;
        let valid = bool::from(pending.token.ct_eq(&digest(token))) && Instant::now() < pending.deadline;
        valid.then_some(pending)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_key_over_its_limit_is_refused_without_touching_others() {
        let limit = super::PerMinute::new(2);
        assert!(limit.allow("a"));
        assert!(limit.allow("a"));
        assert!(!limit.allow("a"));
        assert!(limit.allow("b"));
    }

    #[test]
    fn ipv6_clients_are_limited_by_their_network() {
        use std::net::IpAddr;
        let key = |raw: &str| super::client_key(raw.parse::<IpAddr>().unwrap());
        assert_eq!(key("2001:db8:1:2::1"), key("2001:db8:1:2:ffff::9"));
        assert_ne!(key("2001:db8:1:2::1"), key("2001:db8:1:3::1"));
        assert_eq!(key("::ffff:192.0.2.7"), key("192.0.2.7"));
    }

    #[test]
    fn a_local_tunnel_is_not_limited_as_one_client() {
        use std::net::IpAddr;
        let key = |raw: &str| super::client_key(raw.parse::<IpAddr>().unwrap());
        assert_eq!(key("127.0.0.1"), None);
        assert_eq!(key("::1"), None);
        assert_eq!(key("::ffff:127.0.0.1"), None);
        assert!(key("192.0.2.7").is_some());
    }

    use super::*;

    #[test]
    fn one_time_tokens_are_single_use_and_expire() {
        let store = OneTime::default();
        let token = store.issue("a", Duration::from_secs(60)).unwrap();
        assert_eq!(store.take("wrong"), None);
        assert_eq!(store.take(&token), Some("a"));
        assert_eq!(store.take(&token), None);

        let expired = store.issue("b", Duration::ZERO).unwrap();
        assert_eq!(store.take(&expired), None);
        assert_eq!(store.len(), 0, "an expired token is consumed too");
    }

    #[test]
    fn issuing_is_bounded() {
        let store = OneTime::default();
        for _ in 0..MAX_OUTSTANDING {
            store.issue((), Duration::from_secs(60)).unwrap();
        }
        assert!(store.issue((), Duration::from_secs(60)).is_none());

        let expiring = OneTime::default();
        for _ in 0..MAX_OUTSTANDING {
            expiring.issue((), Duration::ZERO).unwrap();
        }
        assert!(expiring.issue((), Duration::from_secs(60)).is_some(), "expired entries make room");
    }

    #[test]
    fn a_replaced_connection_does_not_take_its_successor_offline() {
        let presence = Presence::default();
        let (a, _ra) = mpsc::channel(1);
        let (b, _rb) = mpsc::channel(1);
        presence.connect("h", 1, a);
        presence.connect("h", 2, b);
        presence.disconnect("h", 1);
        assert!(presence.is_online("h"));
        presence.disconnect("h", 2);
        assert!(!presence.is_online("h"));
        assert!(presence.last_seen("h").is_some());
    }

    #[test]
    fn a_rendezvous_needs_its_token_and_is_single_use() {
        let rv = Rendezvous::<u8>::default();
        let (id, token, _rx) = rv.open("h", Duration::from_secs(10)).unwrap();
        assert!(rv.join(&id, "wrong").is_none());
        assert!(rv.join(&id, &token).is_none(), "a wrong guess burned it");

        let (id, token, mut rx) = rv.open("h", Duration::from_secs(10)).unwrap();
        let pending = rv.join(&id, &token).unwrap();
        assert_eq!(pending.host_id, "h");
        pending.deliver.send(7).unwrap();
        assert_eq!(rx.try_recv(), Ok(7));
        assert!(rv.join(&id, &token).is_none());

        let (id, token, _rx) = rv.open("h", Duration::ZERO).unwrap();
        assert!(rv.join(&id, &token).is_none(), "expired");
    }
}
