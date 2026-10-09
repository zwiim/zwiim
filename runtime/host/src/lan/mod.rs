//! Direct LAN access for paired devices.
//!
//! ```text
//! device ──TCP (same network)── host
//!    └──────── Noise IK tunnel ────────┘ → /zwiim/v1 router
//! ```
//!
//! Off until the host's operator turns it on (`lan.set`, local host
//! authority). While on, the host:
//!
//! - accepts device tunnels on a TCP port on every IPv4 interface. The port
//!   carries exactly what the Cloud relay carries: the Noise tunnel that
//!   `tunnel::host::serve` accepts. There is no HTTP on it, so no `/api`, no
//!   bearer token and no pairing endpoint of its own. The trusted-device
//!   check, pairing by one-time code and revocation are the tunnel's: being
//!   on the same network grants nothing.
//! - advertises itself over mDNS/DNS-SD as `<host_id>._zwiim._tcp.local.`
//!   with only what a paired device needs to dial it: the host id, the port
//!   and addresses, and the host's static public key (TXT `v`, `id`, `pk`).
//!   No name, path, session state or credential.
//!
//! - tells a paired device where to dial it (`host.lan_endpoint`, over a
//!   connection the device already has): the port and the addresses of the
//!   machine's network interfaces. A phone keeps it for the next connect,
//!   without discovery.
//!
//! A device checks the advertised key against the one it pinned at pairing
//! and the Noise handshake proves the host holds it, so a spoofed
//! advertisement can at worst fail a connection, never capture one.

#[cfg(test)]
mod tests;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use axum::Router;
use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use crate::app_state::AppState;
use crate::db::error::DbError;
use crate::db::service::app_metadata_service;
use crate::protocol::error::ZwiimError;
use crate::web::shutdown::ShutdownSignal;

/// The DNS-SD service type hosts advertise.
pub const SERVICE_TYPE: &str = "_zwiim._tcp.local.";
/// TXT `v`: the advertisement format.
pub const TXT_VERSION: &str = "1";
const CONFIG_KEY: &str = "lan_config";
/// Concurrent LAN tunnels, handshakes included. Further connections are
/// closed on arrival.
const MAX_CONNECTIONS: usize = 32;

/// Whether the operator allows LAN access. Stored in `app_metadata`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanConfig {
    pub enabled: bool,
}

pub async fn config(conn: &DatabaseConnection) -> Result<LanConfig, DbError> {
    Ok(app_metadata_service::get_value(conn, CONFIG_KEY)
        .await?
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default())
}

async fn save_config(conn: &DatabaseConnection, config: &LanConfig) -> Result<(), DbError> {
    let raw = serde_json::to_string(config).map_err(|e| DbError::Validation(e.to_string()))?;
    app_metadata_service::upsert_value(conn, CONFIG_KEY, &raw).await
}

/// Where and how the host listens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// TCP port; 0 picks a free one each time LAN access starts.
    pub port: u16,
    /// Advertise over mDNS (tests turn it off).
    pub advertise: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { port: 0, advertise: true }
    }
}

impl Settings {
    /// `ZWIIM_LAN_PORT` (optional).
    pub fn from_env() -> Result<Self, String> {
        let port = match std::env::var("ZWIIM_LAN_PORT").ok().filter(|s| !s.is_empty()) {
            None => 0,
            Some(raw) => raw
                .parse::<u16>()
                .map_err(|_| format!("ZWIIM_LAN_PORT must be a port number, got {raw:?}"))?,
        };
        Ok(Self { port, advertise: true })
    }
}

/// What LAN access is doing now. Public facts only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Listening {
    pub port: Option<u16>,
    pub advertised: bool,
    /// Why LAN access could not start. Never holds a credential.
    pub last_error: Option<String>,
}

struct Runtime {
    router: Router,
    shutdown: Arc<ShutdownSignal>,
}

/// One period of LAN access, from enabling to disabling.
struct Active {
    /// Stops the listener and ends its tunnels.
    stop: Arc<ShutdownSignal>,
    task: JoinHandle<()>,
}

/// The host's LAN access state (`AppState::lan`).
pub struct LanLink {
    settings: RwLock<Settings>,
    runtime: OnceLock<Runtime>,
    active: Mutex<Option<Active>>,
    listening: Arc<Mutex<Listening>>,
    connections: Arc<Semaphore>,
    /// Serializes `lan.set` and startup.
    apply_lock: tokio::sync::Mutex<()>,
}

impl Default for LanLink {
    fn default() -> Self {
        Self::new(Settings::default())
    }
}

impl LanLink {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings: RwLock::new(settings),
            runtime: OnceLock::new(),
            active: Mutex::new(None),
            listening: Default::default(),
            connections: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
            apply_lock: Default::default(),
        }
    }

    pub fn from_env() -> Self {
        let settings = Settings::from_env().unwrap_or_else(|e| {
            tracing::error!("[lan] {e}; using a free port");
            Settings::default()
        });
        Self::new(settings)
    }

    /// Replace the settings (tests; takes effect the next time LAN access
    /// starts).
    pub fn configure(&self, settings: Settings) {
        *self.settings.write().unwrap() = settings;
    }

    pub fn listening(&self) -> Listening {
        self.listening.lock().unwrap().clone()
    }

    /// Stop the listener and end its tunnels; the returned task finishes
    /// once the port is released and the advertisement withdrawn.
    fn stop(&self) -> Option<JoinHandle<()>> {
        let active = self.active.lock().unwrap().take()?;
        active.stop.trigger();
        Some(active.task)
    }
}

impl Drop for LanLink {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Start LAN access if the operator turned it on, once the host's tunnel
/// router exists. `lan.set` starts and stops it later.
pub fn start(state: Arc<AppState>, router: Router, shutdown: Arc<ShutdownSignal>) {
    if state.lan.runtime.set(Runtime { router, shutdown }).is_err() {
        return tracing::warn!("[lan] already started");
    }
    tokio::spawn(async move {
        if let Err(e) = apply(&state).await {
            tracing::warn!("[lan] not started: {e:?}");
        }
    });
}

/// Make the running listener match the stored configuration.
async fn apply(state: &Arc<AppState>) -> Result<(), ZwiimError> {
    let _guard = state.lan.apply_lock.lock().await;
    let enabled = config(&state.db.conn).await.map_err(|_| ZwiimError::internal())?.enabled;
    let link = &state.lan;
    if let Some(stopping) = link.stop() {
        let _ = tokio::time::timeout(Duration::from_secs(5), stopping).await;
    }
    *link.listening.lock().unwrap() = Listening::default();
    let Some(runtime) = link.runtime.get() else { return Ok(()) };
    if !enabled {
        return Ok(());
    }
    let stop = Arc::new(ShutdownSignal::new());
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(run(
        state.clone(),
        runtime.router.clone(),
        runtime.shutdown.clone(),
        stop.clone(),
        *link.settings.read().unwrap(),
        ready_tx,
    ));
    *link.active.lock().unwrap() = Some(Active { stop, task });
    // Report the outcome of binding (or keep waiting for the host identity
    // in the background).
    let _ = tokio::time::timeout(Duration::from_secs(5), ready_rx).await;
    Ok(())
}

async fn run(
    state: Arc<AppState>,
    router: Router,
    host_shutdown: Arc<ShutdownSignal>,
    stop: Arc<ShutdownSignal>,
    settings: Settings,
    ready: tokio::sync::oneshot::Sender<()>,
) {
    let listening = state.lan.listening.clone();
    let fail = |e: String| {
        tracing::warn!("[lan] {e}");
        listening.lock().unwrap().last_error = Some(e);
    };
    // The host's tunnel key loads in the background after startup.
    let (host_id, static_pubkey) = loop {
        if let Some(keys) = state.host_keys.get() {
            match crate::identity::host::ensure_host_id(&state.db.conn).await {
                Ok(host_id) => break (host_id, keys.static_pubkey()),
                Err(e) => return fail(format!("host identity unavailable: {e}")),
            }
        }
        tokio::select! {
            _ = stop.wait() => return,
            _ = host_shutdown.wait() => return,
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
    };
    let listener = match tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, settings.port)).await {
        Ok(listener) => listener,
        Err(e) => return fail(format!("cannot listen on port {}: {e}", settings.port)),
    };
    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(e) => return fail(format!("cannot listen: {e}")),
    };
    let _ = crate::web::socket_inherit::mark_listener_non_inheritable(&listener);
    let advertised = if settings.advertise {
        match Advertisement::register(&host_id, &static_pubkey, port) {
            Ok(ad) => Some(ad),
            Err(e) => {
                fail(format!("cannot advertise over mDNS: {e}"));
                None
            }
        }
    } else {
        None
    };
    {
        let mut l = listening.lock().unwrap();
        l.port = Some(port);
        l.advertised = advertised.is_some();
    }
    tracing::info!("[lan] accepting device tunnels on port {port}");
    let _ = ready.send(());

    // Host shutdown ends LAN tunnels too.
    let ends = {
        let (stop, host_shutdown) = (stop.clone(), host_shutdown.clone());
        tokio::spawn(async move {
            host_shutdown.wait().await;
            stop.trigger();
        })
    };
    loop {
        tokio::select! {
            _ = stop.wait() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let Ok(permit) = state.lan.connections.clone().try_acquire_owned() else {
                        tracing::debug!("[lan] connection limit reached; closing a new connection");
                        continue;
                    };
                    let _ = stream.set_nodelay(true);
                    let (state, router, stop) = (state.clone(), router.clone(), stop.clone());
                    tokio::spawn(async move {
                        crate::tunnel::host::serve(state, router, stop, stream, crate::web::auth::ConnectionPath::Lan).await;
                        drop(permit);
                    });
                }
                Err(e) => {
                    tracing::warn!("[lan] accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
        }
    }
    ends.abort();
    drop(advertised);
    let mut l = listening.lock().unwrap();
    l.port = None;
    l.advertised = false;
    tracing::info!("[lan] stopped");
}

/// The host's mDNS/DNS-SD record, withdrawn on drop.
struct Advertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Advertisement {
    fn register(host_id: &str, static_pubkey: &str, port: u16) -> Result<Self, mdns_sd::Error> {
        let daemon = ServiceDaemon::new()?;
        // The listener is IPv4 only; advertise only addresses it answers on.
        daemon.disable_interface(IfKind::IPv6)?;
        let info = service_info(host_id, static_pubkey, port)?;
        let fullname = info.get_fullname().to_string();
        daemon.register(info)?;
        Ok(Self { daemon, fullname })
    }
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// The advertised record: instance `<host_id>`, host name `<host_id>.local.`
/// (not the machine's name), the interfaces' addresses, and TXT `v`, `id`,
/// `pk`. Nothing else.
pub fn service_info(host_id: &str, static_pubkey: &str, port: u16) -> Result<ServiceInfo, mdns_sd::Error> {
    let txt = [("v", TXT_VERSION), ("id", host_id), ("pk", static_pubkey)];
    Ok(ServiceInfo::new(SERVICE_TYPE, host_id, &format!("{host_id}.local."), (), port, &txt[..])?
        .enable_addr_auto())
}

/// `lan.status`.
pub async fn status(state: &AppState) -> Result<serde_json::Value, ZwiimError> {
    let config = config(&state.db.conn).await.map_err(|_| ZwiimError::internal())?;
    let listening = state.lan.listening();
    Ok(serde_json::json!({
        "enabled": config.enabled,
        "listening": listening.port.is_some(),
        "port": listening.port,
        "advertised": listening.advertised,
        "last_error": listening.last_error,
    }))
}

/// `host.lan_endpoint`: where a paired device on this network can dial the
/// host directly, while LAN access is listening; `null` otherwise. The port
/// and addresses only.
pub fn endpoint(state: &AppState) -> serde_json::Value {
    let endpoint = state.lan.listening().port.and_then(|port| {
        let addresses = interface_addresses();
        (!addresses.is_empty()).then(|| serde_json::json!({ "addresses": addresses, "port": port }))
    });
    serde_json::json!({ "endpoint": endpoint })
}

/// Most addresses `host.lan_endpoint` reports; a device tries each in turn.
const MAX_ENDPOINT_ADDRESSES: usize = 4;
/// Interfaces no other device on the network dials: loopback, VPN and
/// tunnel, container and VM bridges, and Apple's peer-to-peer links.
const VIRTUAL_INTERFACES: &[&str] = &[
    "lo", "utun", "tun", "tap", "ppp", "ipsec", "wg", "bridge", "br-", "docker", "veth", "vmnet", "vboxnet", "virbr",
    "awdl", "llw", "anpi",
];

/// The machine's IPv4 addresses a device on the same network can dial,
/// private ranges first. A device waits out a connect timeout on each
/// address that does not answer, so virtual interfaces are left out.
fn interface_addresses() -> Vec<String> {
    let mut addresses: Vec<Ipv4Addr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !VIRTUAL_INTERFACES.iter().any(|prefix| i.name.starts_with(prefix)))
        .filter_map(|i| match i.ip() {
            IpAddr::V4(ip)
                if !ip.is_loopback()
                    && !ip.is_link_local()
                    && !ip.is_unspecified()
                    && !ip.is_multicast()
                    && !ip.is_broadcast() =>
            {
                Some(ip)
            }
            _ => None,
        })
        .collect();
    addresses.sort_by_key(|ip| (!ip.is_private(), *ip));
    addresses.dedup();
    addresses.truncate(MAX_ENDPOINT_ADDRESSES);
    addresses.into_iter().map(|ip| ip.to_string()).collect()
}

/// `lan.set`: allow or stop LAN access, and remember the choice.
pub async fn set(state: &Arc<AppState>, enabled: bool) -> Result<serde_json::Value, ZwiimError> {
    save_config(&state.db.conn, &LanConfig { enabled })
        .await
        .map_err(|_| ZwiimError::internal())?;
    tracing::info!("[lan] access {}", if enabled { "enabled" } else { "disabled" });
    apply(state).await?;
    status(state).await
}
