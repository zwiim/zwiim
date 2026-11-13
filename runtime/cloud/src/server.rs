//! The Cloud service: HTTP endpoints, the host control socket and the relay.
//!
//! The Cloud authenticates accounts (Supabase tokens) and hosts (Ed25519
//! signatures), tracks presence, hands out single-use tickets and joins a
//! device socket to a host socket. It then moves binary messages between the
//! two without interpreting them: they are the Noise tunnel, end-to-end
//! encrypted between device and host. Whether a device may use a host is
//! decided by the host alone, inside the tunnel.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::body::Bytes;
use axum::{Json, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::{Signature, VerifyingKey};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch, Semaphore};

use crate::auth::{AuthConfig, AuthError, Verifier};
use crate::hub::{self, OneTime, PerMinute, Presence, Rendezvous};
use crate::proto::*;
use crate::registry::{HostRecord, Registry};
use crate::usage::{Store, Usage};

const MAX_BODY: usize = 16 * 1024;
const CONTROL_QUEUE: usize = 64;
const CLOSE_LINGER: Duration = Duration::from_secs(1);
/// WebSocket "try again later": the relay limit is reached.
const CLOSE_TRY_AGAIN: u16 = 1013;
/// How long a ticket waits for an account's usage on its first ticket after
/// a Cloud start; past this the account is not limited until the next sync.
const USAGE_LOAD_TIMEOUT: Duration = Duration::from_secs(2);

/// Deadlines; tests shorten them.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// A host with no heartbeat for this long is dropped.
    pub presence_timeout: Duration,
    pub enrollment_ttl: Duration,
    pub ticket_ttl: Duration,
    /// How long a device waits for its host to dial in.
    pub dial_timeout: Duration,
    /// How long a new socket has to send its first message.
    pub hello_timeout: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            presence_timeout: PRESENCE_TIMEOUT,
            enrollment_ttl: Duration::from_secs(10 * 60),
            ticket_ttl: Duration::from_secs(30),
            dial_timeout: Duration::from_secs(10),
            hello_timeout: Duration::from_secs(10),
        }
    }
}

/// Resource bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Open connections of every kind; more are closed as they arrive.
    pub max_connections: usize,
    /// Concurrent device relays; more are refused with close code 1013.
    pub max_relays: usize,
    /// New connections per client address per minute; more are closed.
    pub max_connects_per_ip: usize,
    /// Authenticated requests per account per minute; more get a 429.
    pub max_requests_per_account: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_connections: 1024,
            max_relays: 256,
            max_connects_per_ip: 120,
            max_requests_per_account: 300,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    DeviceToHost,
    HostToDevice,
}

/// Observes every relayed message. Tests use it to prove the relay carries
/// ciphertext only; production never sets it.
pub type RelayTap = Arc<dyn Fn(Direction, &[u8]) + Send + Sync>;

pub struct Config {
    /// The origin hosts and devices use to reach this Cloud; hosts bind it
    /// into their signatures.
    pub public_url: String,
    pub data_dir: PathBuf,
    pub auth: AuthConfig,
    pub timing: Timing,
    pub limits: Limits,
    pub relay_tap: Option<RelayTap>,
    /// Where relay usage is kept; without one it is counted in memory only
    /// and nothing is limited.
    pub usage_store: Option<Arc<dyn Store>>,
}

struct Enrollment {
    user_id: String,
}

struct Ticket {
    host_id: String,
    /// The account the relay's bytes count against.
    user_id: String,
    channel: Channel,
}

pub struct Cloud {
    origin: String,
    verifier: Verifier,
    registry: Registry,
    timing: Timing,
    tap: Option<RelayTap>,
    enrollments: OneTime<Enrollment>,
    tickets: OneTime<Ticket>,
    presence: Presence,
    rendezvous: Rendezvous<WebSocket>,
    relays: Arc<Semaphore>,
    accounts: PerMinute<String>,
    usage: Arc<Usage>,
    next_conn: AtomicU64,
    shutdown: watch::Receiver<bool>,
}

impl Cloud {
    pub fn new(config: Config, shutdown: watch::Receiver<bool>) -> Result<Arc<Self>, String> {
        let origin = normalize_cloud_url(&config.public_url).map_err(|e| format!("public URL: {e}"))?;
        Ok(Arc::new(Self {
            origin,
            verifier: Verifier::new(config.auth)?,
            registry: Registry::open(&config.data_dir).map_err(|e| format!("registry: {e}"))?,
            timing: config.timing,
            tap: config.relay_tap,
            enrollments: OneTime::default(),
            tickets: OneTime::default(),
            presence: Presence::default(),
            rendezvous: Rendezvous::default(),
            relays: Arc::new(Semaphore::new(config.limits.max_relays)),
            accounts: PerMinute::new(config.limits.max_requests_per_account),
            usage: Arc::new(Usage::new(config.usage_store)),
            next_conn: AtomicU64::new(1),
            shutdown,
        }))
    }

    pub fn usage(&self) -> Arc<Usage> {
        self.usage.clone()
    }

    async fn user(&self, headers: &HeaderMap) -> Result<String, ApiError> {
        let header = headers.get("authorization").and_then(|v| v.to_str().ok());
        let user_id = self.verifier.verify_header(header).await.map_err(|e| match e {
            AuthError::Missing => ApiError::unauthenticated("An account token is required"),
            AuthError::Invalid => ApiError::unauthenticated("The account token is invalid or expired"),
        })?;
        if !self.accounts.allow(user_id.clone()) {
            return Err(ApiError::rate_limited());
        }
        Ok(user_id)
    }

    fn host_signed(&self, record: &HostRecord, message: &[u8], signature: &str) -> bool {
        let key = URL_SAFE_NO_PAD
            .decode(&record.sign_pubkey)
            .ok()
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
            .and_then(|b| VerifyingKey::from_bytes(&b).ok());
        let sig = URL_SAFE_NO_PAD
            .decode(signature)
            .ok()
            .and_then(|b| <[u8; 64]>::try_from(b).ok())
            .map(|b| Signature::from_bytes(&b));
        match (key, sig) {
            (Some(key), Some(sig)) => key.verify_strict(message, &sig).is_ok(),
            _ => false,
        }
    }
}

/// The Cloud's routes. There is nothing else: no host operations, no `/api`.
pub fn router(cloud: Arc<Cloud>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route(ENROLLMENTS_PATH, post(create_enrollment))
        .route(ENROLL_PATH, post(enroll))
        .route(HOSTS_PATH, get(list_hosts))
        .route("/v1/hosts/{host_id}/tickets", post(create_ticket))
        .route(CONTROL_PATH, get(control_socket))
        .route(RELAY_DEVICE_PATH, get(device_socket))
        .route(RELAY_HOST_PATH, get(host_socket))
        .fallback(|| async { ApiError::new(StatusCode::NOT_FOUND, "not_found", "Not found") })
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(cloud)
}

// ─── errors ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    quota: Option<QuotaExceeded>,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into(), quota: None }
    }
    fn quota_exceeded(quota: QuotaExceeded) -> Self {
        let message = match quota.channel {
            QuotaLimit::Browser => "The account reached its remote browser limit for this period",
            QuotaLimit::Total => "The account reached its relay limit for this period",
        };
        Self { quota: Some(quota), ..Self::new(StatusCode::TOO_MANY_REQUESTS, "relay_quota_exceeded", message) }
    }
    fn unauthenticated(message: &str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthenticated", message)
    }
    fn invalid(message: &str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }
    fn rate_limited() -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", "Too many requests; try again shortly")
    }
    fn busy() -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", "Try again later")
    }
    fn internal() -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Internal error")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody { error: ErrorDetail { code: self.code.into(), message: self.message, quota: self.quota } };
        (self.status, Json(body)).into_response()
    }
}

// ─── HTTP ────────────────────────────────────────────────────────────────

async fn create_enrollment(
    State(cloud): State<Arc<Cloud>>,
    headers: HeaderMap,
) -> Result<Json<EnrollmentTokenResponse>, ApiError> {
    let user_id = cloud.user(&headers).await?;
    let ttl = cloud.timing.enrollment_ttl;
    let token = cloud.enrollments.issue(Enrollment { user_id }, ttl).ok_or_else(ApiError::busy)?;
    Ok(Json(EnrollmentTokenResponse { enrollment_token: token, expires_at: hub::unix_now() + ttl.as_secs() }))
}

fn decode_key(raw: &str) -> Option<[u8; 32]> {
    URL_SAFE_NO_PAD.decode(raw).ok()?.try_into().ok()
}

async fn enroll(
    State(cloud): State<Arc<Cloud>>,
    body: Result<Json<EnrollRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<EnrollResponse>, ApiError> {
    let Json(req) = body.map_err(|rejection| match rejection.status() {
        StatusCode::PAYLOAD_TOO_LARGE => ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "too_large", "Request too large"),
        _ => ApiError::invalid("Malformed enrollment request"),
    })?;
    // Single use: redeemed before anything else is checked.
    let enrollment = cloud
        .enrollments
        .take(&req.enrollment_token)
        .ok_or_else(|| ApiError::unauthenticated("The enrollment token is invalid, used or expired"))?;
    let name = req.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    if !valid_host_id(&req.host_id)
        || decode_key(&req.sign_pubkey).is_none()
        || decode_key(&req.static_pubkey).is_none()
        || name.as_ref().is_some_and(|n| n.len() > 128 || n.chars().any(char::is_control))
    {
        return Err(ApiError::invalid("Malformed enrollment request"));
    }
    let record = HostRecord {
        host_id: req.host_id.clone(),
        user_id: enrollment.user_id,
        name,
        sign_pubkey: req.sign_pubkey.clone(),
        static_pubkey: req.static_pubkey.clone(),
        enrolled_at: hub::unix_now(),
    };
    let message = enroll_message(
        &cloud.origin,
        &req.host_id,
        &req.sign_pubkey,
        &req.static_pubkey,
        &req.enrollment_token,
    );
    if !cloud.host_signed(&record, &message, &req.signature) {
        return Err(ApiError::unauthenticated("The enrollment signature is invalid"));
    }
    // The host already belongs to an account: only that account may enroll it.
    if req.expected_user_id.as_ref().is_some_and(|u| *u != record.user_id) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "host_owned",
            "The host belongs to another account",
        ));
    }
    // A host id is bound to the key that first enrolled it.
    if let Some(existing) = cloud.registry.get(&req.host_id) {
        if existing.sign_pubkey != record.sign_pubkey {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "already_exists",
                "This host id is enrolled with a different key",
            ));
        }
    }
    cloud.registry.put(record.clone()).map_err(|e| {
        tracing::error!("[cloud] registry write failed: {e}");
        ApiError::internal()
    })?;
    tracing::info!(host_id = %record.host_id, "[cloud] host enrolled");
    Ok(Json(EnrollResponse { host_id: record.host_id, user_id: record.user_id }))
}

async fn list_hosts(State(cloud): State<Arc<Cloud>>, headers: HeaderMap) -> Result<Json<Vec<HostInfo>>, ApiError> {
    let user_id = cloud.user(&headers).await?;
    Ok(Json(
        cloud
            .registry
            .for_user(&user_id)
            .into_iter()
            .map(|h| HostInfo {
                online: cloud.presence.is_online(&h.host_id),
                last_seen_at: cloud.presence.last_seen(&h.host_id),
                host_id: h.host_id,
                name: h.name,
                static_pubkey: h.static_pubkey,
            })
            .collect(),
    ))
}

async fn create_ticket(
    State(cloud): State<Arc<Cloud>>,
    Path(host_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<TicketResponse>, ApiError> {
    let user_id = cloud.user(&headers).await?;
    // Clients before relay channels send no body.
    let TicketRequest { channel } = match body.is_empty() {
        true => TicketRequest::default(),
        false => serde_json::from_slice(&body).map_err(|_| ApiError::invalid("Malformed ticket request"))?,
    };
    // Someone else's host is indistinguishable from no host.
    if !cloud.registry.get(&host_id).is_some_and(|h| h.user_id == user_id) {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "not_found", "Host not found"));
    }
    if !cloud.presence.is_online(&host_id) {
        return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "host_offline", "The host is offline"));
    }
    let _ = tokio::time::timeout(USAGE_LOAD_TIMEOUT, cloud.usage.load(&user_id)).await;
    if let Some(quota) = cloud.usage.blocked(&user_id, channel) {
        return Err(ApiError::quota_exceeded(quota));
    }
    let ttl = cloud.timing.ticket_ttl;
    let ticket = cloud.tickets.issue(Ticket { host_id, user_id, channel }, ttl).ok_or_else(ApiError::busy)?;
    Ok(Json(TicketResponse { ticket, expires_at: hub::unix_now() + ttl.as_secs() }))
}

// ─── sockets ─────────────────────────────────────────────────────────────

async fn close(mut ws: WebSocket, code: u16, reason: &'static str) {
    let _ = ws.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
    // Let the peer's close arrive so ours is not lost to a reset.
    let _ = tokio::time::timeout(CLOSE_LINGER, async { while let Some(Ok(_)) = ws.recv().await {} }).await;
}

async fn send_json<T: serde::Serialize>(ws: &mut WebSocket, msg: &T) -> bool {
    let text = serde_json::to_string(msg).expect("serializable");
    ws.send(Message::Text(text.into())).await.is_ok()
}

/// The first message on a socket: text, small, within the hello timeout.
async fn hello<T: serde::de::DeserializeOwned>(cloud: &Cloud, ws: &mut WebSocket) -> Result<T, (u16, &'static str)> {
    let first = tokio::time::timeout(cloud.timing.hello_timeout, async {
        loop {
            match ws.recv().await {
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                other => return other,
            }
        }
    })
    .await;
    match first {
        Ok(Some(Ok(Message::Text(text)))) if text.len() <= MAX_CONTROL_MESSAGE => {
            serde_json::from_str(&text).map_err(|_| (CLOSE_PROTOCOL, "malformed"))
        }
        Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => Err((1000, "")),
        Ok(Some(Ok(_))) => Err((CLOSE_PROTOCOL, "malformed")),
        Err(_) => Err((CLOSE_PROTOCOL, "timeout")),
    }
}

async fn control_socket(State(cloud): State<Arc<Cloud>>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(MAX_CONTROL_MESSAGE)
        .max_frame_size(MAX_CONTROL_MESSAGE)
        .on_upgrade(move |ws| control(cloud, ws))
}

/// A host's control connection: challenge, signed answer, then heartbeats
/// and dial requests until it drops, goes silent, or is replaced.
async fn control(cloud: Arc<Cloud>, mut ws: WebSocket) {
    let nonce = hub::random_token(32);
    if !send_json(&mut ws, &ControlFromCloud::Challenge { nonce: nonce.clone() }).await {
        return;
    }
    let host_id = match hello(&cloud, &mut ws).await {
        Ok(ControlFromHost::Auth { host_id, signature }) => {
            let authentic = valid_host_id(&host_id)
                && cloud.registry.get(&host_id).is_some_and(|record| {
                    cloud.host_signed(&record, &auth_message(&cloud.origin, &host_id, &nonce), &signature)
                });
            if !authentic {
                return close(ws, CLOSE_UNAUTHENTICATED, "unauthenticated").await;
            }
            host_id
        }
        Ok(_) => return close(ws, CLOSE_PROTOCOL, "malformed").await,
        Err((code, reason)) => return close(ws, code, reason).await,
    };

    let conn = cloud.next_conn.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::channel(CONTROL_QUEUE);
    cloud.presence.connect(&host_id, conn, tx);
    tracing::info!(host_id = %host_id, "[cloud] host online");
    let mut shutdown = cloud.shutdown.clone();
    let (code, reason) = if !send_json(&mut ws, &ControlFromCloud::Ready).await {
        (1000, "")
    } else {
        let mut deadline = tokio::time::Instant::now() + cloud.timing.presence_timeout;
        loop {
            tokio::select! {
                msg = ws.recv() => match msg {
                    Some(Ok(Message::Text(text))) => match serde_json::from_str(&text) {
                        Ok(ControlFromHost::Heartbeat) => {
                            deadline = tokio::time::Instant::now() + cloud.timing.presence_timeout;
                            cloud.presence.touch(&host_id);
                            if !send_json(&mut ws, &ControlFromCloud::HeartbeatAck).await {
                                break (1000, "");
                            }
                        }
                        _ => break (CLOSE_PROTOCOL, "malformed"),
                    },
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    Some(Ok(Message::Binary(_))) => break (CLOSE_PROTOCOL, "malformed"),
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break (1000, ""),
                },
                out = rx.recv() => match out {
                    Some(msg) => {
                        if !send_json(&mut ws, &msg).await {
                            break (1000, "");
                        }
                    }
                    // A newer connection from this host took over.
                    None => break (CLOSE_REPLACED, "replaced"),
                },
                _ = tokio::time::sleep_until(deadline) => break (1001, "heartbeat timeout"),
                _ = crate::hub::stopped(&mut shutdown) => break (1012, "restarting"),
            }
        }
    };
    cloud.presence.disconnect(&host_id, conn);
    tracing::info!(host_id = %host_id, reason, "[cloud] host offline");
    close(ws, code, reason).await;
}

fn relay_socket(ws: WebSocketUpgrade) -> WebSocketUpgrade {
    ws.max_message_size(MAX_RELAY_MESSAGE).max_frame_size(MAX_RELAY_MESSAGE)
}

async fn device_socket(State(cloud): State<Arc<Cloud>>, ws: WebSocketUpgrade) -> Response {
    relay_socket(ws).on_upgrade(move |ws| device_relay(cloud, ws))
}

async fn host_socket(State(cloud): State<Arc<Cloud>>, ws: WebSocketUpgrade) -> Response {
    relay_socket(ws).on_upgrade(move |ws| host_relay(cloud, ws))
}

/// A device's relay socket: redeem the ticket, ask the host to dial, wait
/// for its socket, then relay.
async fn device_relay(cloud: Arc<Cloud>, mut ws: WebSocket) {
    let ticket = match hello(&cloud, &mut ws).await {
        Ok(RelayHello::Device { ticket }) => ticket,
        Ok(_) => return close(ws, CLOSE_PROTOCOL, "malformed").await,
        Err((code, reason)) => return close(ws, code, reason).await,
    };
    let Some(Ticket { host_id, user_id, channel }) = cloud.tickets.take(&ticket) else {
        return close(ws, CLOSE_UNAUTHENTICATED, "unauthenticated").await;
    };
    // Held for the relay's life.
    let Ok(_slot) = cloud.relays.clone().try_acquire_owned() else {
        tracing::warn!(host_id = %host_id, "[cloud] relay refused: at the relay limit");
        return close(ws, CLOSE_TRY_AGAIN, "busy").await;
    };
    let Some(control) = cloud.presence.control(&host_id) else {
        return close(ws, CLOSE_HOST_OFFLINE, "host_offline").await;
    };
    let Some((id, token, arrived)) = cloud.rendezvous.open(&host_id, cloud.timing.dial_timeout) else {
        return close(ws, CLOSE_HOST_OFFLINE, "host_offline").await;
    };
    if control.try_send(ControlFromCloud::Dial { rendezvous: id.clone(), token, channel }).is_err() {
        cloud.rendezvous.cancel(&id);
        return close(ws, CLOSE_HOST_OFFLINE, "host_offline").await;
    }
    let mut shutdown = cloud.shutdown.clone();
    let host = tokio::select! {
        joined = tokio::time::timeout(cloud.timing.dial_timeout, arrived) => match joined {
            Ok(Ok(host)) => host,
            _ => {
                cloud.rendezvous.cancel(&id);
                tracing::info!(host_id = %host_id, "[cloud] host did not answer a dial");
                return close(ws, CLOSE_HOST_OFFLINE, "host_offline").await;
            }
        },
        _ = crate::hub::stopped(&mut shutdown) => {
            cloud.rendezvous.cancel(&id);
            return close(ws, 1012, "restarting").await;
        }
    };
    tracing::info!(relay = %id, host_id = %host_id, channel = channel.as_str(), "[cloud] relay open");
    pipe(&cloud, ws, host, &user_id, channel).await;
    tracing::info!(relay = %id, host_id = %host_id, "[cloud] relay closed");
}

/// A host's answer to a dial: hand its socket to the waiting device.
async fn host_relay(cloud: Arc<Cloud>, mut ws: WebSocket) {
    let (id, token) = match hello(&cloud, &mut ws).await {
        Ok(RelayHello::Host { rendezvous, token }) => (rendezvous, token),
        Ok(_) => return close(ws, CLOSE_PROTOCOL, "malformed").await,
        Err((code, reason)) => return close(ws, code, reason).await,
    };
    let Some(pending) = cloud.rendezvous.join(&id, &token) else {
        return close(ws, CLOSE_UNAUTHENTICATED, "unauthenticated").await;
    };
    if let Err(ws) = pending.deliver.send(ws) {
        // The device gave up waiting.
        close(ws, CLOSE_HOST_OFFLINE, "device_gone").await;
    }
}

/// Relay binary messages both ways until either side closes, counting their
/// bytes against `user_id` on `channel`. Both ends learn the other is ready;
/// after that, any text message is a protocol violation. A relay that
/// reaches a limit of the account's plan is closed at both ends with
/// [`CLOSE_QUOTA_EXCEEDED`].
async fn pipe(cloud: &Cloud, mut device: WebSocket, mut host: WebSocket, user_id: &str, channel: Channel) {
    if !send_json(&mut host, &RelayReady::Ready).await {
        return close(device, CLOSE_HOST_OFFLINE, "host_offline").await;
    }
    if !send_json(&mut device, &RelayReady::Ready).await {
        return close(host, 1000, "").await;
    }
    let (mut device_tx, mut device_rx) = device.split();
    let (mut host_tx, mut host_rx) = host.split();
    let mut shutdown = cloud.shutdown.clone();
    let blocked = tokio::select! {
        b = forward(&mut device_rx, &mut host_tx, Direction::DeviceToHost, cloud, user_id, channel) => b,
        b = forward(&mut host_rx, &mut device_tx, Direction::HostToDevice, cloud, user_id, channel) => b,
        _ = crate::hub::stopped(&mut shutdown) => false,
    };
    if blocked {
        tracing::info!(channel = channel.as_str(), "[cloud] relay closed at the account's limit");
        for tx in [&mut device_tx, &mut host_tx] {
            let frame = CloseFrame { code: CLOSE_QUOTA_EXCEEDED, reason: "relay_quota_exceeded".into() };
            let _ = tx.send(Message::Close(Some(frame))).await;
        }
    }
}

/// Move messages from `from` to `to` until `from` closes, then pass its
/// close on. `true` when the account reached a limit instead; the caller
/// closes both ends.
async fn forward(
    from: &mut SplitStream<WebSocket>,
    to: &mut SplitSink<WebSocket, Message>,
    direction: Direction,
    cloud: &Cloud,
    user_id: &str,
    channel: Channel,
) -> bool {
    let (code, reason) = loop {
        match from.next().await {
            Some(Ok(Message::Binary(bytes))) => {
                if let Some(tap) = &cloud.tap {
                    tap(direction, &bytes);
                }
                if cloud.usage.blocked(user_id, channel).is_some() {
                    return true;
                }
                // Both directions leave the Cloud's network, so both count.
                cloud.usage.add(user_id, channel, bytes.len() as u64);
                if channel == Channel::Control {
                    let hold = cloud.usage.throttle(user_id, bytes.len() as u64);
                    if !hold.is_zero() {
                        tokio::time::sleep(hold).await;
                    }
                }
                if to.send(Message::Binary(bytes)).await.is_err() {
                    return false;
                }
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            Some(Ok(Message::Text(_))) => break (CLOSE_PROTOCOL, "malformed"),
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break (1000, ""),
        }
    };
    let _ = to.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
    false
}
