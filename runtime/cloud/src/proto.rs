//! The Cloud wire contract, shared by the Cloud service, the host connector
//! and device clients.
//!
//! Nothing here carries Zwiim protocol content. The relay moves the bytes of
//! the Noise tunnel (`zwiim_host::tunnel`) and never sees what is inside.
//!
//! ```text
//! device ──WSS /v1/relay/device── Cloud ──WSS /v1/relay/host── host
//!                                   │
//!                     host ──WSS /v1/hosts/control (presence, dial)
//! ```
//!
//! Keys and signatures are URL-safe base64 without padding, as everywhere
//! else in Zwiim identity.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use url::{Host, Url};

// ─── endpoints ───────────────────────────────────────────────────────────

/// `POST` with an account token: a single-use host enrollment token.
pub const ENROLLMENTS_PATH: &str = "/v1/enrollments";
/// `POST` [`EnrollRequest`], authorized by the enrollment token it carries.
pub const ENROLL_PATH: &str = "/v1/hosts/enroll";
/// `GET` with an account token: the account's hosts.
pub const HOSTS_PATH: &str = "/v1/hosts";
/// `GET` with an account token: the account's plan and relay usage
/// ([`UsageResponse`]).
pub const USAGE_PATH: &str = "/v1/usage";
/// The host's control socket.
pub const CONTROL_PATH: &str = "/v1/hosts/control";
/// The device's relay socket.
pub const RELAY_DEVICE_PATH: &str = "/v1/relay/device";
/// The host's relay socket, one per dial.
pub const RELAY_HOST_PATH: &str = "/v1/relay/host";

/// `POST` [`TicketRequest`] with an account token: a connection ticket for
/// `host_id`. An empty body asks for a `control` ticket.
pub fn tickets_path(host_id: &str) -> String {
    format!("/v1/hosts/{host_id}/tickets")
}

// ─── limits and timing ───────────────────────────────────────────────────

/// Largest relayed WebSocket message. A Noise frame is at most 64 KiB.
pub const MAX_RELAY_MESSAGE: usize = 128 * 1024;
/// Largest control or hello message.
pub const MAX_CONTROL_MESSAGE: usize = 8 * 1024;
/// How often a host sends a heartbeat on its control socket.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
/// A host silent for this long is offline.
pub const PRESENCE_TIMEOUT: Duration = Duration::from_secs(45);

// ─── WebSocket close codes ───────────────────────────────────────────────

/// Malformed or unexpected message.
pub const CLOSE_PROTOCOL: u16 = 4400;
/// Authentication failed: bad signature, unknown host, or an invalid,
/// expired or already used ticket.
pub const CLOSE_UNAUTHENTICATED: u16 = 4401;
/// A newer control connection from the same host replaced this one.
pub const CLOSE_REPLACED: u16 = 4409;
/// The account reached a relay limit of its plan ([`QuotaExceeded`]).
pub const CLOSE_QUOTA_EXCEEDED: u16 = 4429;
/// The host is not connected, or did not answer the dial in time.
pub const CLOSE_HOST_OFFLINE: u16 = 4503;

// ─── relay channels ──────────────────────────────────────────────────────

/// The kind of traffic a relay connection carries. Each connection has one,
/// chosen with its ticket; the Cloud counts and limits them separately.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// The Zwiim protocol. What clients that name no channel get.
    #[default]
    Control,
    /// Agent View and Live App.
    Browser,
}

impl Channel {
    pub const ALL: [Channel; 2] = [Channel::Control, Channel::Browser];

    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Control => "control",
            Channel::Browser => "browser",
        }
    }
}

// ─── control socket ──────────────────────────────────────────────────────

/// Cloud → host, JSON text frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlFromCloud {
    /// First message: the host signs [`auth_message`] over `nonce`.
    Challenge { nonce: String },
    /// Authenticated; the host is online.
    Ready,
    /// A device holds a ticket for this host: open a relay socket and
    /// present `rendezvous` + `token` (single use, short-lived). Clouds
    /// before relay channels send no `channel`: `control`.
    Dial {
        rendezvous: String,
        token: String,
        #[serde(default)]
        channel: Channel,
    },
    HeartbeatAck,
}

/// Host → Cloud, JSON text frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlFromHost {
    Auth { host_id: String, signature: String },
    Heartbeat,
}

// ─── relay sockets ───────────────────────────────────────────────────────

/// The first (text) message on a relay socket. Everything after the Cloud's
/// [`RelayReady`] is binary and opaque.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelayHello {
    Device { ticket: String },
    Host { rendezvous: String, token: String },
}

/// The Cloud's answer once both ends are joined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelayReady {
    Ready,
}

// ─── HTTP bodies ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentTokenResponse {
    pub enrollment_token: String,
    /// Unix seconds.
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollRequest {
    pub enrollment_token: String,
    pub host_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Ed25519: authenticates the host's control connection.
    pub sign_pubkey: String,
    /// X25519: the key devices pin for the tunnel (metadata only here).
    pub static_pubkey: String,
    /// The account the host already belongs to, if any. The Cloud refuses an
    /// enrollment token from any other account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_user_id: Option<String>,
    /// Ed25519 over [`enroll_message`]: proof the host holds `sign_pubkey`.
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollResponse {
    pub host_id: String,
    pub user_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfo {
    pub host_id: String,
    pub name: Option<String>,
    pub static_pubkey: String,
    pub online: bool,
    /// Unix seconds of the last heartbeat since the Cloud started.
    pub last_seen_at: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TicketRequest {
    #[serde(default)]
    pub channel: Channel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TicketResponse {
    pub ticket: String,
    /// Unix seconds.
    pub expires_at: u64,
}

/// The account's plan and its relay usage this period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageResponse {
    /// `None` when this Cloud keeps no plan for the account: nothing is
    /// limited, and the period fields are absent.
    pub plan: Option<PlanInfo>,
    #[serde(default, flatten, skip_serializing_if = "Option::is_none")]
    pub period: Option<UsagePeriod>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsagePeriod {
    /// Unix seconds.
    pub period_start: u64,
    /// Unix seconds.
    pub resets_at: u64,
    pub channels: UsageChannels,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageChannels {
    pub browser: ChannelUsage,
    /// Every channel together.
    pub total: ChannelUsage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelUsage {
    pub used: u64,
    /// `None` is unlimited.
    pub limit: Option<u64>,
}

/// Every HTTP error: `{"error": {"code", "message"}}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
    /// With `relay_quota_exceeded` only.
    #[serde(default, flatten, skip_serializing_if = "Option::is_none")]
    pub quota: Option<QuotaExceeded>,
}

/// Which limit a ticket or relay ran into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaLimit {
    /// Browser bytes this period.
    Browser,
    /// Bytes this period over every channel.
    Total,
}

/// A refused ticket (`429 relay_quota_exceeded`) says which limit, and when
/// it lifts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaExceeded {
    pub channel: QuotaLimit,
    /// The plan id.
    pub plan: String,
    pub used_bytes: u64,
    pub limit_bytes: u64,
    /// Unix seconds.
    pub resets_at: u64,
}

// ─── signed messages ─────────────────────────────────────────────────────
//
// Both bind the Cloud's origin, so a signature made for one Cloud is useless
// at another.

/// What a host signs to enroll.
pub fn enroll_message(
    cloud_origin: &str,
    host_id: &str,
    sign_pubkey: &str,
    static_pubkey: &str,
    enrollment_token: &str,
) -> Vec<u8> {
    signed(&[
        "zwiim-cloud-enroll/1",
        cloud_origin,
        host_id,
        sign_pubkey,
        static_pubkey,
        enrollment_token,
    ])
}

/// What a host signs to authenticate its control connection.
pub fn auth_message(cloud_origin: &str, host_id: &str, nonce: &str) -> Vec<u8> {
    signed(&["zwiim-cloud-auth/1", cloud_origin, host_id, nonce])
}

/// Fields are NUL-separated; none can contain NUL (origins are URLs, host ids
/// are [`valid_host_id`], keys and tokens are base64).
fn signed(parts: &[&str]) -> Vec<u8> {
    parts.join("\0").into_bytes()
}

// ─── URLs ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CloudUrlError {
    #[error("not a valid Cloud URL")]
    Invalid,
    /// Only loopback Clouds may be reached without TLS.
    #[error("the Cloud URL must use https")]
    Insecure,
}

/// The canonical origin of a Cloud URL: `https://host[:port]`, lowercase,
/// no path. `http` is accepted for loopback hosts only.
pub fn normalize_cloud_url(raw: &str) -> Result<String, CloudUrlError> {
    let url = Url::parse(raw.trim()).map_err(|_| CloudUrlError::Invalid)?;
    let host = url.host().ok_or(CloudUrlError::Invalid)?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(CloudUrlError::Invalid);
    }
    match url.scheme() {
        "https" => {}
        "http" if is_loopback(&host) => {}
        "http" => return Err(CloudUrlError::Insecure),
        _ => return Err(CloudUrlError::Invalid),
    }
    Ok(url.origin().ascii_serialization())
}

/// The WebSocket URL of `path` on a normalized Cloud origin.
pub fn ws_url(origin: &str, path: &str) -> String {
    let rest = origin
        .strip_prefix("https://")
        .map(|r| format!("wss://{r}"))
        .or_else(|| origin.strip_prefix("http://").map(|r| format!("ws://{r}")))
        .unwrap_or_else(|| origin.to_string());
    format!("{rest}{path}")
}

pub fn is_loopback(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(d) => d.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
    }
}

/// A host id the Cloud accepts: 1-128 of `[A-Za-z0-9_-]`, so it is safe in a
/// URL path and a log line.
pub fn valid_host_id(host_id: &str) -> bool {
    (1..=128).contains(&host_id.len())
        && host_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_urls_normalize_to_an_origin() {
        for (raw, origin) in [
            ("https://Cloud.Example.com", "https://cloud.example.com"),
            ("https://cloud.example.com/", "https://cloud.example.com"),
            ("https://cloud.example.com:443", "https://cloud.example.com"),
            ("https://cloud.example.com:8443/", "https://cloud.example.com:8443"),
            ("http://127.0.0.1:9000", "http://127.0.0.1:9000"),
            ("http://localhost:9000", "http://localhost:9000"),
            ("http://[::1]:9000", "http://[::1]:9000"),
        ] {
            assert_eq!(normalize_cloud_url(raw).as_deref(), Ok(origin), "{raw}");
        }
        for raw in ["http://cloud.example.com", "http://10.0.0.2:9000", "http://0.0.0.0:9000"] {
            assert_eq!(normalize_cloud_url(raw), Err(CloudUrlError::Insecure), "{raw}");
        }
        for raw in [
            "",
            "cloud.example.com",
            "ftp://cloud.example.com",
            "https://cloud.example.com/api",
            "https://u:p@cloud.example.com",
            "https://cloud.example.com/?x=1",
            "wss://cloud.example.com",
        ] {
            assert_eq!(normalize_cloud_url(raw), Err(CloudUrlError::Invalid), "{raw}");
        }
    }

    #[test]
    fn websocket_urls_follow_the_scheme() {
        assert_eq!(ws_url("https://c.example", CONTROL_PATH), "wss://c.example/v1/hosts/control");
        assert_eq!(ws_url("http://127.0.0.1:1", RELAY_HOST_PATH), "ws://127.0.0.1:1/v1/relay/host");
    }

    #[test]
    fn signed_messages_bind_every_field() {
        let base = enroll_message("https://a", "h", "s", "x", "t");
        for other in [
            enroll_message("https://b", "h", "s", "x", "t"),
            enroll_message("https://a", "h2", "s", "x", "t"),
            enroll_message("https://a", "h", "s2", "x", "t"),
            enroll_message("https://a", "h", "s", "x2", "t"),
            enroll_message("https://a", "h", "s", "x", "t2"),
        ] {
            assert_ne!(base, other);
        }
        assert_ne!(auth_message("https://a", "h", "n"), auth_message("https://b", "h", "n"));
        assert_ne!(auth_message("https://a", "h", "n"), base);
    }

    #[test]
    fn host_ids_are_path_safe() {
        assert!(valid_host_id("0b7c2f1e-2d3a-4c1b-9f0e-1a2b3c4d5e6f"));
        for bad in ["", "a/b", "a b", "..", "a%2f", &"x".repeat(129)] {
            assert!(!valid_host_id(bad), "{bad}");
        }
    }

    #[test]
    fn control_messages_are_tagged() {
        let dial = ControlFromCloud::Dial { rendezvous: "r".into(), token: "t".into(), channel: Channel::Browser };
        assert_eq!(
            serde_json::to_value(&dial).unwrap(),
            serde_json::json!({"type": "dial", "rendezvous": "r", "token": "t", "channel": "browser"})
        );
        // From a Cloud before relay channels.
        let old: ControlFromCloud = serde_json::from_str(r#"{"type":"dial","rendezvous":"r","token":"t"}"#).unwrap();
        assert!(matches!(old, ControlFromCloud::Dial { channel: Channel::Control, .. }));
        let hello: RelayHello = serde_json::from_str(r#"{"type":"device","ticket":"k"}"#).unwrap();
        assert_eq!(hello, RelayHello::Device { ticket: "k".into() });
        assert!(serde_json::from_str::<ControlFromHost>(r#"{"type":"dial"}"#).is_err());
    }
}
