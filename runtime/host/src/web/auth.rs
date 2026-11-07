// Modified from upstream Apache-2.0 source; see NOTICE.
//! Request authentication and the one authorization distinction the host
//! makes: local host authority versus a paired device.

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sea_orm::DatabaseConnection;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::broadcast::error::RecvError;

use crate::identity::trusted_devices::{self, Revocations};

pub const WS_EVENT_PROTOCOL: &str = "zwiim-events";
/// Token subprotocol of the event sockets (`/ws/events`, `/zwiim/v1/ws`).
const WS_TOKEN_PROTOCOL_PREFIX: &str = "zwiim-token.";

fn token_from_ws_protocols(value: &str) -> Option<String> {
    value
        .split(',')
        .map(str::trim)
        .find_map(|protocol| protocol.strip_prefix(WS_TOKEN_PROTOCOL_PREFIX))
        .and_then(|encoded| URL_SAFE_NO_PAD.decode(encoded).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// Bearer credentials presented on the request: the `Authorization: Bearer`
/// header and/or the WS token subprotocol.
fn presented_tokens(request: &Request) -> Vec<String> {
    let mut tokens = Vec::new();
    if let Some(auth_str) = request
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
    {
        if let Some(t) = auth_str.strip_prefix("Bearer ") {
            tokens.push(t.to_string());
        }
    }
    if let Some(protocols) = request
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|h| h.to_str().ok())
    {
        if let Some(t) = token_from_ws_protocols(protocols) {
            tokens.push(t);
        }
    }
    tokens
}

/// Who an authenticated request acts for. `require_token` attaches it as a
/// request extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Principal {
    /// The host's own access token: the local desktop or operator.
    Host,
    /// A paired device, by `device_id`.
    Device(String),
}

/// A device authenticated by the tunnel (`crate::tunnel::host`), attached to
/// the requests it routes in-process. Only the tunnel adapter inserts it; no
/// network request can carry an extension.
#[derive(Clone, Debug)]
pub struct TunnelPeer {
    pub device_id: String,
}

/// How a request reached the Host. The tunnel adapter attaches it to the
/// requests it routes; plain HTTP requests carry none and are `Local`.
/// Size limits that exist for the Cloud relay's bandwidth key off it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionPath {
    Local,
    Lan,
    Relay,
}

/// Constant-time token equality. Both sides are hashed first so neither the
/// comparison nor its length check depends on how much of the token matches.
fn token_matches(presented: &str, expected: &str) -> bool {
    let presented = Sha256::digest(presented.as_bytes());
    let expected = Sha256::digest(expected.as_bytes());
    presented.ct_eq(&expected).into()
}

/// Accepts the host's own access token (local desktop / operator), the bearer
/// credential of a currently trusted paired device, or a [`TunnelPeer`] that
/// is still trusted, and records which as the request's [`Principal`].
pub async fn require_token(
    mut request: Request,
    next: Next,
    token: String,
    db: DatabaseConnection,
) -> Response {
    // Fail closed on a misconfigured empty token: otherwise `Bearer ` (an empty
    // bearer value) would match it and silently disable authentication.
    if token.is_empty() {
        return (StatusCode::UNAUTHORIZED, "Server token is not configured").into_response();
    }

    // A tunnel peer is a device, whatever else the request carries. Trust is
    // re-checked per request, so a revocation applies to the next one.
    if let Some(TunnelPeer { device_id }) = request.extensions().get::<TunnelPeer>().cloned() {
        return match trusted_devices::is_trusted(&db, &device_id).await {
            Ok(true) => {
                if let Err(e) = trusted_devices::mark_seen(&db, &device_id).await {
                    tracing::warn!("[auth][WARN] recording device activity failed: {e}");
                }
                request.extensions_mut().insert(Principal::Device(device_id));
                next.run(request).await
            }
            Ok(false) => (StatusCode::UNAUTHORIZED, "Invalid or missing token").into_response(),
            Err(e) => {
                tracing::warn!("[auth][WARN] trusted-device lookup failed: {e}");
                (StatusCode::UNAUTHORIZED, "Invalid or missing token").into_response()
            }
        };
    }

    let presented = presented_tokens(&request);
    if presented.iter().any(|t| token_matches(t, &token)) {
        request.extensions_mut().insert(Principal::Host);
        return next.run(request).await;
    }

    for candidate in &presented {
        match trusted_devices::authenticate(&db, candidate).await {
            Ok(Some(device_id)) => {
                if let Err(e) = trusted_devices::mark_seen(&db, &device_id).await {
                    tracing::warn!("[auth][WARN] recording device activity failed: {e}");
                }
                request.extensions_mut().insert(Principal::Device(device_id));
                return next.run(request).await;
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("[auth][WARN] trusted-device lookup failed: {e}"),
        }
    }

    (StatusCode::UNAUTHORIZED, "Invalid or missing token").into_response()
}

/// Admits only requests made with local host authority. Device management
/// (pairing, listing, revocation) and host claiming stay with the host's
/// operator; a paired device may drive the host but not change who trusts it.
/// Runs inside `require_token`.
pub async fn require_local_authority(request: Request, next: Next) -> Response {
    match request.extensions().get::<Principal>() {
        Some(Principal::Host) => next.run(request).await,
        _ => (StatusCode::FORBIDDEN, "Requires local host authority").into_response(),
    }
}

/// WebSocket close code sent when the socket's device is revoked.
pub const WS_CLOSE_DEVICE_REVOKED: u16 = 4403;

/// Resolves once `principal`'s device is no longer trusted; never for the
/// host itself. Event sockets select on it so revocation ends an open
/// connection instead of waiting for it to reconnect.
pub async fn device_revoked(principal: Option<Principal>, revocations: Revocations, db: DatabaseConnection) {
    let Some(Principal::Device(device_id)) = principal else {
        return std::future::pending().await;
    };
    // Subscribe before checking: a revocation between authentication and
    // here is then either persisted already or still to be announced.
    let mut rx = revocations.subscribe();
    loop {
        if !trusted_devices::is_trusted(&db, &device_id).await.unwrap_or(false) {
            return;
        }
        loop {
            match rx.recv().await {
                Ok(revoked) if revoked == device_id => return,
                Ok(_) => {}
                // Missed announcements: re-check the persisted state.
                Err(RecvError::Lagged(_)) => break,
                Err(RecvError::Closed) => return std::future::pending().await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    #[test]
    fn parses_token_from_ws_protocols() {
        let token = "secret/token+value";
        let encoded = URL_SAFE_NO_PAD.encode(token);
        assert_eq!(
            token_from_ws_protocols(&format!("zwiim-events, zwiim-token.{encoded}")).as_deref(),
            Some(token)
        );
    }

    #[test]
    fn ignores_invalid_ws_protocol_token() {
        assert!(token_from_ws_protocols("zwiim-events, zwiim-token.not-valid-@@@@").is_none());
    }

    #[test]
    fn token_comparison() {
        assert!(token_matches("smoke-token-123", "smoke-token-123"));
        assert!(!token_matches("smoke-token-124", "smoke-token-123"));
        assert!(!token_matches("smoke-token-12", "smoke-token-123"));
        assert!(!token_matches("smoke-token-1234", "smoke-token-123"));
        assert!(!token_matches("", "smoke-token-123"));
    }

    #[tokio::test]
    async fn revocation_resolves_only_for_the_revoked_device() {
        use crate::db::test_helpers::fresh_in_memory_db;
        use std::time::Duration;

        let db = fresh_in_memory_db().await;
        trusted_devices::register(&db.conn, "dev-1", "One").await.unwrap();
        trusted_devices::register(&db.conn, "dev-2", "Two").await.unwrap();
        let revocations = Revocations::default();
        let watch = |principal| {
            tokio::spawn(device_revoked(principal, revocations.clone(), db.conn.clone()))
        };
        let host = watch(Some(Principal::Host));
        let one = watch(Some(Principal::Device("dev-1".into())));
        let two = watch(Some(Principal::Device("dev-2".into())));
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(trusted_devices::revoke(&db.conn, "dev-1").await.unwrap());
        revocations.announce("dev-1");
        tokio::time::timeout(Duration::from_secs(5), one).await.unwrap().unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!two.is_finished());
        assert!(!host.is_finished());

        // Already revoked before watching (the reconnect race): resolves at once.
        tokio::time::timeout(
            Duration::from_secs(5),
            device_revoked(Some(Principal::Device("dev-1".into())), revocations, db.conn.clone()),
        )
        .await
        .unwrap();
        two.abort();
        host.abort();
    }
}
