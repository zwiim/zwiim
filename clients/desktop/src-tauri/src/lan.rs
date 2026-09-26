//! Hosts on this computer's local network, reached directly.
//!
//! The webview can do neither of the two things this needs: browse mDNS, or
//! open a plain TCP connection. [`Discovery`] browses `_zwiim._tcp.local.`
//! in the background and keeps the advertisements it sees; [`Links`] opens
//! a byte pipe to an endpoint discovery reported, and to nothing else, and
//! moves bytes both ways.
//!
//! Nothing here trusts a host. The UI's SDK checks each advertisement
//! against the host key pinned at pairing and runs the Noise handshake over
//! the pipe, so a host is only ever reached as its pinned key says, and the
//! shell never sees a decrypted byte. Only an advertisement's `v`, `id` and
//! `pk` are passed on.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::TcpStream;

/// The DNS-SD service type Zwiim hosts advertise.
pub const SERVICE_TYPE: &str = "_zwiim._tcp.local.";
/// Open links at once: a device needs one per host it shows.
const MAX_LINKS: usize = 8;
const MAX_ADDRESSES: usize = 8;
/// The TXT keys passed on; anything else a host (or impostor) adds is dropped.
const TXT_KEYS: [&str; 3] = ["v", "id", "pk"];

/// One host's advertisement, in the shape the SDK's `LanDiscover` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Advertisement {
    /// The DNS-SD instance name: the host id.
    pub instance: String,
    pub port: u16,
    pub addresses: Vec<String>,
    pub txt: BTreeMap<String, String>,
}

/// `<instance>._zwiim._tcp.local.` → `<instance>`.
fn instance_of(fullname: &str) -> Option<&str> {
    fullname.strip_suffix(SERVICE_TYPE)?.strip_suffix('.').filter(|i| !i.is_empty())
}

/// An advertisement from what mDNS resolved: IPv4 only (hosts listen on
/// IPv4), non-loopback addresses first.
pub fn advertisement<'a>(
    fullname: &str,
    port: u16,
    addresses: impl IntoIterator<Item = IpAddr>,
    txt: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<Advertisement> {
    let instance = instance_of(fullname)?.to_string();
    let mut addresses: Vec<IpAddr> = addresses.into_iter().filter(IpAddr::is_ipv4).collect();
    addresses.sort_by_key(|a| (a.is_loopback(), *a));
    addresses.dedup();
    addresses.truncate(MAX_ADDRESSES);
    let txt = txt
        .into_iter()
        .filter(|(k, _)| TXT_KEYS.contains(k))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    (!addresses.is_empty()).then(|| Advertisement {
        instance,
        port,
        addresses: addresses.iter().map(IpAddr::to_string).collect(),
        txt,
    })
}

type Seen = Arc<Mutex<HashMap<String, Advertisement>>>;

/// A background mDNS browse, started on first use, and what it has seen.
#[derive(Default)]
pub struct Discovery {
    daemon: Mutex<Option<ServiceDaemon>>,
    seen: Seen,
}

impl Discovery {
    fn ensure_browsing(&self) -> Result<(), String> {
        let mut daemon = self.daemon.lock().unwrap();
        if daemon.is_some() {
            return Ok(());
        }
        let d = ServiceDaemon::new().map_err(|e| format!("Local network discovery is unavailable: {e}"))?;
        let events = d
            .browse(SERVICE_TYPE)
            .map_err(|e| format!("Local network discovery is unavailable: {e}"))?;
        let seen = self.seen.clone();
        std::thread::Builder::new()
            .name("zwiim-lan-browse".into())
            .spawn(move || {
                while let Ok(event) = events.recv() {
                    match event {
                        ServiceEvent::ServiceResolved(s) => {
                            let addresses = s.addresses.iter().map(|a| a.to_ip_addr());
                            let txt = s.txt_properties.iter().map(|p| (p.key(), p.val_str()));
                            if let Some(ad) = advertisement(&s.fullname, s.port, addresses, txt) {
                                seen.lock().unwrap().insert(ad.instance.clone(), ad);
                            }
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => {
                            if let Some(instance) = instance_of(&fullname) {
                                seen.lock().unwrap().remove(instance);
                            }
                        }
                        ServiceEvent::SearchStopped(_) => break,
                        _ => {}
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        *daemon = Some(d);
        Ok(())
    }

    /// `host_id`'s advertisement, waiting up to `timeout` for it to be seen.
    /// Empty when the host is not advertising on this network.
    pub async fn discover(&self, host_id: &str, timeout: Duration) -> Result<Vec<Advertisement>, String> {
        self.ensure_browsing()?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(ad) = self.seen.lock().unwrap().get(host_id) {
                return Ok(vec![ad.clone()]);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(Vec::new());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Whether discovery reported `address:port`: the only endpoints a link
    /// may be opened to.
    pub fn reported(&self, address: IpAddr, port: u16) -> bool {
        let address = address.to_string();
        self.seen
            .lock()
            .unwrap()
            .values()
            .any(|ad| ad.port == port && ad.addresses.contains(&address))
    }

    #[cfg(test)]
    fn insert(&self, ad: Advertisement) {
        self.seen.lock().unwrap().insert(ad.instance.clone(), ad);
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        if let Some(d) = self.daemon.lock().unwrap().take() {
            let _ = d.shutdown();
        }
    }
}

/// What a link reports to the UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LinkEvent {
    /// Bytes from the host, base64.
    Data { data: String },
    /// The connection ended; reported once.
    Closed,
}

type Sink = Arc<dyn Fn(LinkEvent) + Send + Sync>;

struct Link {
    write: Arc<tokio::sync::Mutex<OwnedWriteHalf>>,
    reader: tokio::task::JoinHandle<()>,
    sink: Sink,
}

/// Open TCP links to hosts, by id.
#[derive(Default)]
pub struct Links {
    next: AtomicU32,
    open: Arc<Mutex<HashMap<u32, Link>>>,
}

impl Links {
    /// Connect to `address:port`, which discovery must have reported. Bytes
    /// the host sends, and the end of the connection, go to `sink`.
    pub async fn connect(
        &self,
        discovery: &Discovery,
        address: &str,
        port: u16,
        timeout: Duration,
        sink: impl Fn(LinkEvent) + Send + Sync + 'static,
    ) -> Result<u32, String> {
        let ip: IpAddr = address.parse().map_err(|_| "Not an address".to_string())?;
        if !discovery.reported(ip, port) {
            return Err("Not a host found on this network".into());
        }
        if self.open.lock().unwrap().len() >= MAX_LINKS {
            return Err("Too many local connections".into());
        }
        let stream = tokio::time::timeout(timeout, TcpStream::connect(SocketAddr::new(ip, port)))
            .await
            .map_err(|_| "Connection timed out".to_string())?
            .map_err(|e| format!("Cannot connect: {e}"))?;
        let _ = stream.set_nodelay(true);
        let (mut read, write) = stream.into_split();
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let sink: Sink = Arc::new(sink);
        let open = self.open.clone();
        let events = sink.clone();
        // Held until the link is registered, so a connection that ends at
        // once is still reported (and removed) after it.
        let registered = Arc::new(tokio::sync::Notify::new());
        let ready = registered.clone();
        let reader = tokio::spawn(async move {
            ready.notified().await;
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                match read.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => events(LinkEvent::Data {
                        data: base64::engine::general_purpose::STANDARD.encode(&buf[..n]),
                    }),
                }
            }
            if open.lock().unwrap().remove(&id).is_some() {
                events(LinkEvent::Closed);
            }
        });
        self.open.lock().unwrap().insert(
            id,
            Link {
                write: Arc::new(tokio::sync::Mutex::new(write)),
                reader,
                sink,
            },
        );
        registered.notify_one();
        Ok(id)
    }

    /// Send bytes (base64) to the host.
    pub async fn send(&self, id: u32, data: &str) -> Result<(), String> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| "Not base64".to_string())?;
        let write = self.open.lock().unwrap().get(&id).map(|l| l.write.clone());
        let Some(write) = write else {
            return Err("Connection closed".into());
        };
        let result = write.lock().await.write_all(&bytes).await;
        if result.is_err() {
            self.close(id).await;
            return Err("Connection closed".into());
        }
        Ok(())
    }

    /// End a link. Reported as `Closed` if it was still open.
    pub async fn close(&self, id: u32) {
        let link = self.open.lock().unwrap().remove(&id);
        if let Some(link) = link {
            link.reader.abort();
            let _ = link.write.lock().await.shutdown().await;
            (link.sink)(LinkEvent::Closed);
        }
    }

    pub fn len(&self) -> usize {
        self.open.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use tokio::net::TcpListener;

    fn ad(instance: &str, port: u16, addresses: &[&str]) -> Advertisement {
        Advertisement {
            instance: instance.into(),
            port,
            addresses: addresses.iter().map(|a| a.to_string()).collect(),
            txt: BTreeMap::new(),
        }
    }

    #[test]
    fn advertisements_keep_ipv4_and_the_three_txt_keys_only() {
        let ips: Vec<IpAddr> = ["127.0.0.1", "fe80::1", "192.168.1.20", "10.0.0.2", "192.168.1.20"]
            .iter()
            .map(|a| a.parse().unwrap())
            .collect();
        let found = advertisement(
            "host-1._zwiim._tcp.local.",
            4000,
            ips,
            [("v", "1"), ("id", "host-1"), ("pk", "key"), ("name", "Studio"), ("path", "/Users/me")],
        )
        .unwrap();
        assert_eq!(found.instance, "host-1");
        assert_eq!(found.port, 4000);
        assert_eq!(found.addresses, ["10.0.0.2", "192.168.1.20", "127.0.0.1"]);
        assert_eq!(
            found.txt,
            BTreeMap::from([("id".into(), "host-1".into()), ("pk".into(), "key".into()), ("v".into(), "1".into())])
        );
        let none: [(&str, &str); 0] = [];
        assert_eq!(advertisement("host-1._zwiim._tcp.local.", 1, ["fe80::1".parse().unwrap()], none), None);
        assert_eq!(advertisement("host-1._other._tcp.local.", 1, [IpAddr::V4(Ipv4Addr::LOCALHOST)], none), None);
        assert_eq!(advertisement("._zwiim._tcp.local.", 1, [IpAddr::V4(Ipv4Addr::LOCALHOST)], none), None);
    }

    async fn listener() -> (TcpListener, u16) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        (l, port)
    }

    fn recorder() -> (Arc<Mutex<Vec<LinkEvent>>>, impl Fn(LinkEvent) + Send + Sync + 'static) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let e = events.clone();
        (events, move |ev| e.lock().unwrap().push(ev))
    }

    async fn until(cond: impl Fn() -> bool) {
        for _ in 0..200 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out");
    }

    #[tokio::test]
    async fn links_open_only_to_what_discovery_reported() {
        let discovery = Discovery::default();
        let links = Links::default();
        let (l, port) = listener().await;
        let secs = Duration::from_secs(2);
        // Not reported: nothing is dialed.
        let err = links.connect(&discovery, "127.0.0.1", port, secs, |_| {}).await.unwrap_err();
        assert_eq!(err, "Not a host found on this network");
        // Reported, but on another port; or not an address at all.
        discovery.insert(ad("host-1", port + 1, &["127.0.0.1"]));
        assert!(links.connect(&discovery, "127.0.0.1", port, secs, |_| {}).await.is_err());
        assert!(links.connect(&discovery, "localhost", port + 1, secs, |_| {}).await.is_err());
        let accepted = tokio::time::timeout(Duration::from_millis(100), l.accept()).await;
        assert!(accepted.is_err(), "nothing was dialed");
        assert_eq!(links.len(), 0);
    }

    #[tokio::test]
    async fn a_link_moves_bytes_both_ways_and_reports_its_end_once() {
        let discovery = Discovery::default();
        let links = Links::default();
        let (l, port) = listener().await;
        discovery.insert(ad("host-1", port, &["127.0.0.1"]));
        let (events, sink) = recorder();
        let id = links
            .connect(&discovery, "127.0.0.1", port, Duration::from_secs(2), sink)
            .await
            .unwrap();
        let (mut peer, _) = l.accept().await.unwrap();

        links.send(id, &base64::engine::general_purpose::STANDARD.encode(b"hello")).await.unwrap();
        let mut buf = [0u8; 5];
        peer.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");

        peer.write_all(b"world").await.unwrap();
        until(|| !events.lock().unwrap().is_empty()).await;
        assert_eq!(
            events.lock().unwrap()[0],
            LinkEvent::Data { data: base64::engine::general_purpose::STANDARD.encode(b"world") }
        );

        // The host ends it: reported once, and the link is gone.
        drop(peer);
        until(|| events.lock().unwrap().last() == Some(&LinkEvent::Closed)).await;
        assert_eq!(links.len(), 0);
        assert!(links.send(id, "AA==").await.is_err());
        links.close(id).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(events.lock().unwrap().iter().filter(|e| **e == LinkEvent::Closed).count(), 1);
    }

    #[tokio::test]
    async fn closing_a_link_ends_the_connection() {
        let discovery = Discovery::default();
        let links = Links::default();
        let (l, port) = listener().await;
        discovery.insert(ad("host-1", port, &["127.0.0.1"]));
        let (events, sink) = recorder();
        let id = links
            .connect(&discovery, "127.0.0.1", port, Duration::from_secs(2), sink)
            .await
            .unwrap();
        let (mut peer, _) = l.accept().await.unwrap();
        links.close(id).await;
        assert_eq!(*events.lock().unwrap(), [LinkEvent::Closed]);
        let mut buf = [0u8; 1];
        assert_eq!(peer.read(&mut buf).await.unwrap(), 0);
        assert_eq!(links.len(), 0);
    }

    #[tokio::test]
    async fn links_are_limited() {
        let discovery = Discovery::default();
        let links = Links::default();
        let (l, port) = listener().await;
        discovery.insert(ad("host-1", port, &["127.0.0.1"]));
        let secs = Duration::from_secs(2);
        let mut peers = Vec::new();
        for _ in 0..MAX_LINKS {
            links.connect(&discovery, "127.0.0.1", port, secs, |_| {}).await.unwrap();
            peers.push(l.accept().await.unwrap());
        }
        let err = links.connect(&discovery, "127.0.0.1", port, secs, |_| {}).await.unwrap_err();
        assert_eq!(err, "Too many local connections");
    }

    /// A real advertisement, as a host publishes it, found over mDNS.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_host_advertising_on_the_network_is_discovered() {
        let host_id = format!("lan-test-{}", std::process::id());
        let publisher = ServiceDaemon::new().unwrap();
        let info = mdns_sd::ServiceInfo::new(
            SERVICE_TYPE,
            &host_id,
            &format!("{host_id}.local."),
            (),
            40123,
            &[("v", "1"), ("id", host_id.as_str()), ("pk", "key"), ("extra", "dropped")][..],
        )
        .unwrap()
        .enable_addr_auto();
        publisher.register(info).unwrap();

        let discovery = Discovery::default();
        assert_eq!(discovery.discover("someone-else", Duration::from_millis(200)).await.unwrap(), []);
        let found = discovery.discover(&host_id, Duration::from_secs(10)).await.unwrap();
        let _ = publisher.shutdown();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].instance, host_id);
        assert_eq!(found[0].port, 40123);
        assert!(!found[0].addresses.is_empty());
        assert_eq!(found[0].txt.keys().collect::<Vec<_>>(), ["id", "pk", "v"]);
        let ip: IpAddr = found[0].addresses[0].parse().unwrap();
        assert!(discovery.reported(ip, 40123));
        assert!(!discovery.reported(ip, 40124));
    }
}
