use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sea_orm::DatabaseConnection;

use crate::identity::trusted_devices;

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

/// Accepts the host's own access token (local desktop / operator) or the
/// bearer credential of a currently trusted paired device.
pub async fn require_token(
    request: Request,
    next: Next,
    token: String,
    db: DatabaseConnection,
) -> Response {
    // Fail closed on a misconfigured empty token: otherwise `Bearer ` (an empty
    // bearer value) would match it and silently disable authentication.
    if token.is_empty() {
        return (StatusCode::UNAUTHORIZED, "Server token is not configured").into_response();
    }

    let presented = presented_tokens(&request);
    if presented.iter().any(|t| *t == token) {
        return next.run(request).await;
    }

    for candidate in &presented {
        match trusted_devices::authenticate(&db, candidate).await {
            Ok(Some(_device_id)) => return next.run(request).await,
            Ok(None) => {}
            Err(e) => tracing::warn!("[auth][WARN] trusted-device lookup failed: {e}"),
        }
    }

    (StatusCode::UNAUTHORIZED, "Invalid or missing token").into_response()
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
}
