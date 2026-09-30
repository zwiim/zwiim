//! The host's link to Zwiim Cloud: managed connectivity for remote devices.
//!
//! ```text
//! remote device ──WSS── Cloud ──WSS (outbound)── host
//!       └──────────── Noise IK tunnel ─────────────┘ → /zwiim/v1 router
//! ```
//!
//! - Enrollment (`cloud.enroll`, local host authority) registers the host's
//!   public keys with an account, using a single-use enrollment token and a
//!   signature by the host's Ed25519 identity key. Only the Cloud URL and the
//!   account id are stored (`app_metadata`); there is no Cloud credential.
//! - The [`connector`] keeps an outbound control connection, authenticated by
//!   signing the Cloud's challenge, sends heartbeats, and answers each dial
//!   with an outbound relay socket that it serves as a device tunnel
//!   (`tunnel::relay` + `tunnel::host::serve`). The host never listens on a
//!   public address.
//! - The Cloud sees ciphertext only. Which devices may use the host is
//!   decided here, by the tunnel's trusted-device check, never by the Cloud.
//! - The Cloud is never in a local path: local clients talk to the host
//!   directly, and a host with no Cloud (or an unreachable one) works as
//!   before. The connector just keeps retrying with backoff.
//!
//! [`device`] is the device half, used by tests and the `tunnel_client`
//! example until a client crate exists.

pub mod connector;
pub mod device;
pub mod transport;

#[cfg(test)]
mod tests;

use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use axum::Router;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use zwiim_cloud::proto::{self, EnrollRequest, EnrollResponse, ENROLL_PATH};

use crate::app_state::AppState;
use crate::db::error::DbError;
use crate::db::service::app_metadata_service;
use crate::identity::host::{self as host_identity, Claim, HostOwner};
use crate::protocol::error::ZwiimError;
use crate::web::shutdown::ShutdownSignal;
use transport::{read_json, unreachable, CloudError, Tls};

const CONFIG_KEY: &str = "cloud_config";
/// How long the Cloud has to answer a handshake step.
pub(crate) const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// Where the host is enrolled. Public facts only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudConfig {
    /// Normalized Cloud origin.
    pub url: String,
    /// The account the Cloud enrolled the host into.
    pub user_id: String,
    pub enrolled_at: DateTime<Utc>,
}

pub async fn config(conn: &DatabaseConnection) -> Result<Option<CloudConfig>, DbError> {
    Ok(app_metadata_service::get_value(conn, CONFIG_KEY)
        .await?
        .and_then(|raw| serde_json::from_str(&raw).ok()))
}

async fn save_config(conn: &DatabaseConnection, config: &CloudConfig) -> Result<(), DbError> {
    let raw = serde_json::to_string(config).map_err(|e| DbError::Validation(e.to_string()))?;
    app_metadata_service::upsert_value(conn, CONFIG_KEY, &raw).await
}

/// Connector timing; tests shorten it.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub heartbeat: Duration,
    /// No heartbeat answer for this long: the connection is dead.
    pub presence_timeout: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            heartbeat: proto::HEARTBEAT_INTERVAL,
            presence_timeout: proto::PRESENCE_TIMEOUT,
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Connection {
    NotEnrolled,
    Connecting,
    Online,
    Offline,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub connection: Connection,
    pub connected_since: Option<DateTime<Utc>>,
    /// Why the last connection attempt failed. Never holds a credential.
    pub last_error: Option<String>,
}

/// What the connector needs to serve relayed tunnels: the `/zwiim/v1`
/// router they route into, and the host's shutdown.
struct Runtime {
    router: Router,
    shutdown: Arc<ShutdownSignal>,
}

/// The host's Cloud connector state (`AppState::cloud`).
pub struct CloudLink {
    settings: RwLock<(Tls, Timing)>,
    status: Mutex<Status>,
    task: Mutex<Option<JoinHandle<()>>>,
    runtime: OnceLock<Runtime>,
    /// Bounds concurrent relayed tunnels.
    relays: Arc<tokio::sync::Semaphore>,
}

/// Concurrent relayed tunnels per host.
const MAX_RELAYS: usize = 32;

impl Default for CloudLink {
    fn default() -> Self {
        Self::new(Tls::default(), Timing::default())
    }
}

impl CloudLink {
    pub fn new(tls: Tls, timing: Timing) -> Self {
        Self {
            settings: RwLock::new((tls, timing)),
            status: Mutex::new(Status { connection: Connection::NotEnrolled, connected_since: None, last_error: None }),
            task: Mutex::new(None),
            runtime: OnceLock::new(),
            relays: Arc::new(tokio::sync::Semaphore::new(MAX_RELAYS)),
        }
    }

    /// Trust from the environment (`ZWIIM_CLOUD_CA_CERT`), default timing.
    pub fn from_env() -> Self {
        let tls = Tls::from_env().unwrap_or_else(|e| {
            tracing::error!("[cloud] {e}; using the system roots only");
            Tls::default()
        });
        Self::new(tls, Timing::default())
    }

    /// Replace TLS trust and timing (tests; takes effect on the next
    /// connection).
    pub fn configure(&self, tls: Tls, timing: Timing) {
        *self.settings.write().unwrap() = (tls, timing);
    }

    fn tls(&self) -> Tls {
        self.settings.read().unwrap().0.clone()
    }

    fn timing(&self) -> Timing {
        self.settings.read().unwrap().1
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    fn set_status(&self, connection: Connection, last_error: Option<String>) {
        let mut status = self.status.lock().unwrap();
        status.connected_since = match connection {
            Connection::Online => Some(Utc::now()),
            _ => None,
        };
        status.connection = connection;
        if connection == Connection::Online || last_error.is_some() {
            status.last_error = last_error;
        }
    }
}

/// Start the connector, once the host's tunnel router exists. Connects if
/// the host is enrolled; `cloud.enroll` (re)starts it later otherwise.
pub fn start(state: Arc<AppState>, router: Router, shutdown: Arc<ShutdownSignal>) {
    if state.cloud.runtime.set(Runtime { router, shutdown }).is_err() {
        return tracing::warn!("[cloud] connector already started");
    }
    restart(&state);
}

fn restart(state: &Arc<AppState>) {
    let Some(runtime) = state.cloud.runtime.get() else { return };
    let mut task = state.cloud.task.lock().unwrap();
    if let Some(old) = task.take() {
        old.abort();
    }
    *task = Some(tokio::spawn(connector::run(
        state.clone(),
        runtime.router.clone(),
        runtime.shutdown.clone(),
    )));
}

/// `cloud.status`: enrollment and connection, no secrets.
pub async fn status(state: &AppState) -> Result<serde_json::Value, ZwiimError> {
    let config = config(&state.db.conn).await.map_err(|_| ZwiimError::internal())?;
    let status = state.cloud.status();
    let connection = match (&config, status.connection) {
        (None, _) => Connection::NotEnrolled,
        (Some(_), Connection::NotEnrolled) => Connection::Offline,
        (Some(_), c) => c,
    };
    Ok(serde_json::json!({
        "enrolled": config.is_some(),
        "cloud_url": config.as_ref().map(|c| &c.url),
        "user_id": config.as_ref().map(|c| &c.user_id),
        "enrolled_at": config.as_ref().map(|c| c.enrolled_at),
        "connection": connection,
        "connected_since": status.connected_since,
        "last_error": status.last_error,
    }))
}

/// What the account's other devices call this host when enrollment names
/// none: the computer's name as the OS shows it ("Ashish’s MacBook Air" on
/// macOS), else its hostname.
fn machine_name() -> Option<String> {
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("/usr/sbin/scutil").args(["--get", "ComputerName"]).output() {
        let name = String::from_utf8_lossy(&out.stdout);
        if let Some(name) = out.status.success().then(|| clean_name(&name)).flatten() {
            return Some(name);
        }
    }
    #[cfg(unix)]
    return crate::commands::config_sync::snapshot::unix_hostname()
        .and_then(|n| clean_name(n.trim_end_matches(".local")));
    #[cfg(not(unix))]
    std::env::var("COMPUTERNAME").ok().and_then(|n| clean_name(&n))
}

/// A name the Cloud accepts: trimmed, no control characters, at most 128 bytes.
fn clean_name(name: &str) -> Option<String> {
    let mut name: String = name.trim().chars().filter(|c| !c.is_control()).collect();
    while name.len() > 128 {
        name.pop();
    }
    (!name.is_empty()).then_some(name)
}

/// `cloud.enroll`: register this host with the Cloud at `cloud_url` into the
/// account that minted `enrollment_token`, then connect.
pub async fn enroll(
    state: &Arc<AppState>,
    cloud_url: &str,
    enrollment_token: &str,
    name: Option<String>,
) -> Result<serde_json::Value, ZwiimError> {
    let origin = proto::normalize_cloud_url(cloud_url).map_err(|e| ZwiimError::invalid_input(e.to_string()))?;
    let name = name.or_else(machine_name);
    let token = enrollment_token.trim();
    if token.is_empty() || token.len() > 256 {
        return Err(ZwiimError::invalid_input("enrollment_token must be 1-256 characters"));
    }
    let keys = state.host_keys.get().ok_or_else(ZwiimError::identity_unavailable)?;
    let conn = &state.db.conn;
    let host_id = host_identity::ensure_host_id(conn).await.map_err(|_| ZwiimError::internal())?;
    let owner = host_identity::owner(conn).await.map_err(|_| ZwiimError::internal())?;

    let (sign_pubkey, static_pubkey) = (keys.sign_pubkey(), keys.static_pubkey());
    let message = proto::enroll_message(&origin, &host_id, &sign_pubkey, &static_pubkey, token);
    let request = EnrollRequest {
        enrollment_token: token.to_string(),
        host_id: host_id.clone(),
        name,
        sign_pubkey,
        static_pubkey,
        expected_user_id: owner.as_ref().map(|o| o.user_id.clone()),
        signature: URL_SAFE_NO_PAD.encode(keys.sign(&message).to_bytes()),
    };
    let http = state.cloud.tls().http().map_err(cloud_error)?;
    let sent = http
        .post(format!("{origin}{ENROLL_PATH}"))
        .json(&request)
        .send()
        .await
        .map_err(|e| cloud_error(unreachable(e)))?;
    let enrolled: EnrollResponse = match read_json(sent).await {
        Ok(enrolled) => enrolled,
        Err(CloudError::Refused { code, .. }) if code == "host_owned" => {
            return Err(ZwiimError::host_owned(owner.and_then(|o| o.email)))
        }
        Err(CloudError::Refused { code, message, .. }) if code == "already_exists" => {
            return Err(ZwiimError::already_exists(message))
        }
        Err(CloudError::Refused { status: 401, .. }) => {
            return Err(ZwiimError::invalid_input("The enrollment token is invalid, used or expired"))
        }
        Err(CloudError::Refused { status: 400, message, .. }) => return Err(ZwiimError::invalid_input(message)),
        Err(e) => return Err(cloud_error(e)),
    };
    if enrolled.host_id != host_id {
        return Err(cloud_error(CloudError::Protocol));
    }

    // The host and the Cloud agree on the account: an unclaimed host is
    // claimed by it; a claimed one was only enrolled by its owner.
    match &owner {
        None => {
            let claimant = HostOwner { user_id: enrolled.user_id.clone(), email: None };
            if let Claim::OwnedByOther(o) = host_identity::claim(conn, claimant)
                .await
                .map_err(|_| ZwiimError::internal())?
            {
                return Err(ZwiimError::host_owned(o.email));
            }
        }
        Some(o) if o.user_id != enrolled.user_id => return Err(ZwiimError::host_owned(o.email.clone())),
        Some(_) => {}
    }
    let config = CloudConfig { url: origin, user_id: enrolled.user_id, enrolled_at: Utc::now() };
    save_config(conn, &config).await.map_err(|_| ZwiimError::internal())?;
    tracing::info!("[cloud] enrolled with {}", config.url);
    state.cloud.set_status(Connection::Connecting, None);
    restart(state);
    status(state).await
}

fn cloud_error(e: CloudError) -> ZwiimError {
    ZwiimError::cloud_unavailable(e.to_string())
}
