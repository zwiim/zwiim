//! The outbound control connection: authenticate with the host's Ed25519
//! key, heartbeat, and answer dials with relayed tunnels. Reconnects with
//! exponential backoff and jitter for as long as the host runs.

use std::sync::Arc;

use axum::Router;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use futures_util::StreamExt;
use rand::Rng;
use tokio_tungstenite::tungstenite::Message;
use zwiim_cloud::proto::{
    self, ControlFromCloud, ControlFromHost, RelayHello, RelayReady, CLOSE_UNAUTHENTICATED, CONTROL_PATH,
    MAX_CONTROL_MESSAGE, MAX_RELAY_MESSAGE, RELAY_HOST_PATH,
};

use super::transport::{self, CloudError, CloudSocket};
use super::{config, CloudConfig, Connection, HELLO_TIMEOUT};
use crate::app_state::AppState;
use crate::web::shutdown::ShutdownSignal;

pub(super) async fn run(state: Arc<AppState>, router: Router, shutdown: Arc<ShutdownSignal>) {
    let cloud = &state.cloud;
    let mut backoff = cloud.timing().backoff_min;
    loop {
        let config = match config(&state.db.conn).await {
            Ok(Some(config)) => config,
            Ok(None) => return cloud.set_status(Connection::NotEnrolled, None),
            Err(e) => {
                tracing::warn!("[cloud] reading the Cloud config failed: {e}");
                return cloud.set_status(Connection::Offline, Some("host database unavailable".into()));
            }
        };
        cloud.set_status(Connection::Connecting, None);
        let (was_online, error) = tokio::select! {
            ended = session(&state, &router, &shutdown, &config) => ended,
            _ = shutdown.wait() => return,
        };
        tracing::info!("[cloud] disconnected from {}: {error}", config.url);
        cloud.set_status(Connection::Offline, Some(error));

        let timing = cloud.timing();
        if was_online {
            backoff = timing.backoff_min;
        }
        let half = backoff / 2;
        let delay = half + half.mul_f64(rand::thread_rng().gen::<f64>());
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shutdown.wait() => return,
        }
        backoff = (backoff * 2).min(timing.backoff_max);
    }
}

/// One control connection, until it fails: whether it got online, and why
/// it ended.
async fn session(
    state: &Arc<AppState>,
    router: &Router,
    shutdown: &Arc<ShutdownSignal>,
    config: &CloudConfig,
) -> (bool, String) {
    let cloud = &state.cloud;
    let Some(keys) = state.host_keys.get() else {
        return (false, "host identity not loaded".into());
    };
    let host_id = match crate::identity::host::ensure_host_id(&state.db.conn).await {
        Ok(host_id) => host_id,
        Err(_) => return (false, "host database unavailable".into()),
    };
    let tls = cloud.tls();
    let timing = cloud.timing();
    let mut ws = match transport::connect(&config.url, CONTROL_PATH, MAX_CONTROL_MESSAGE, &tls).await {
        Ok(ws) => ws,
        Err(e) => return (false, e.to_string()),
    };

    // Challenge, signed answer, ready.
    let authenticated = async {
        let nonce = match serde_json::from_str(&transport::recv_text(&mut ws, HELLO_TIMEOUT).await?) {
            Ok(ControlFromCloud::Challenge { nonce }) => nonce,
            _ => return Err(CloudError::Protocol),
        };
        let signature = keys.sign(&proto::auth_message(&config.url, &host_id, &nonce));
        let auth = ControlFromHost::Auth { host_id: host_id.clone(), signature: URL_SAFE_NO_PAD.encode(signature.to_bytes()) };
        transport::send_json(&mut ws, &auth).await?;
        match serde_json::from_str(&transport::recv_text(&mut ws, HELLO_TIMEOUT).await?) {
            Ok(ControlFromCloud::Ready) => Ok(()),
            _ => Err(CloudError::Protocol),
        }
    };
    match authenticated.await {
        Ok(()) => {}
        Err(CloudError::Closed(CLOSE_UNAUTHENTICATED)) => {
            return (false, "the Cloud does not recognize this host; enroll it again".into())
        }
        Err(e) => return (false, e.to_string()),
    }
    cloud.set_status(Connection::Online, None);
    tracing::info!("[cloud] online at {}", config.url);

    let mut heartbeat = tokio::time::interval(timing.heartbeat);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut deadline = tokio::time::Instant::now() + timing.presence_timeout;
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if transport::send_json(&mut ws, &ControlFromHost::Heartbeat).await.is_err() {
                    return (true, "connection lost".into());
                }
            }
            msg = ws.next() => match msg {
                Some(Ok(Message::Text(text))) => match serde_json::from_str(&text) {
                    Ok(ControlFromCloud::HeartbeatAck) => {
                        deadline = tokio::time::Instant::now() + timing.presence_timeout;
                    }
                    Ok(ControlFromCloud::Dial { rendezvous, token }) => {
                        dial(state, router, shutdown, &config.url, rendezvous, token);
                    }
                    _ => return (true, CloudError::Protocol.to_string()),
                },
                Some(Ok(Message::Close(frame))) => {
                    let code = frame.map(|f| u16::from(f.code)).unwrap_or(1005);
                    return (true, CloudError::Closed(code).to_string());
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return (true, "connection lost".into()),
            },
            _ = tokio::time::sleep_until(deadline) => return (true, "heartbeat timed out".into()),
        }
    }
}

/// Answer a dial: an outbound relay socket, served as a device tunnel. The
/// device is authenticated inside the tunnel like any other.
fn dial(state: &Arc<AppState>, router: &Router, shutdown: &Arc<ShutdownSignal>, url: &str, rendezvous: String, token: String) {
    let Ok(permit) = state.cloud.relays.clone().try_acquire_owned() else {
        return tracing::warn!("[cloud] refusing a dial: too many relayed tunnels");
    };
    let (state, router, shutdown, url) = (state.clone(), router.clone(), shutdown.clone(), url.to_string());
    tokio::spawn(async move {
        let _permit = permit;
        let tls = state.cloud.tls();
        match relay(&url, rendezvous, token, &tls).await {
            Ok(ws) => {
                let io = crate::tunnel::relay::stream(ws);
                crate::tunnel::host::serve(state, router, shutdown, io, crate::web::auth::ConnectionPath::Relay).await;
            }
            Err(e) => tracing::info!("[cloud] dial failed: {e}"),
        }
    });
}

async fn relay(url: &str, rendezvous: String, token: String, tls: &transport::Tls) -> Result<CloudSocket, CloudError> {
    let mut ws = transport::connect(url, RELAY_HOST_PATH, MAX_RELAY_MESSAGE, tls).await?;
    transport::send_json(&mut ws, &RelayHello::Host { rendezvous, token }).await?;
    match serde_json::from_str(&transport::recv_text(&mut ws, HELLO_TIMEOUT).await?) {
        Ok(RelayReady::Ready) => Ok(ws),
        Err(_) => Err(CloudError::Protocol),
    }
}
