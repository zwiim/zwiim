//! Host end of the device tunnel.
//!
//! A tunnel is accepted only from a device whose static key belongs to a
//! trusted device. Its messages are routed in-process through the same
//! `/zwiim/v1` router local clients use — requests as the operation's POST,
//! event streams as a `/zwiim/v1/ws` socket upgraded over an in-memory HTTP
//! connection — with a [`TunnelPeer`] attached, so authentication and
//! authorization (`web::auth`) treat it as that device. Nothing outside
//! `/zwiim/v1` is reachable. Revoking the device ends the tunnel.
//!
//! A device that is not yet trusted may pair as it connects: its first
//! handshake message carries a [`DeviceHello`] with the one-time code from the
//! host's v2 QR. The host redeems it exactly as `device.pair` would, for the
//! static key the device has just proved it holds, and the tunnel opens as
//! the newly trusted device. This is how a remote device pairs: the Cloud
//! relay only reaches the host through the tunnel.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::{body::Body, extract::Extension, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, protocol::CloseFrame, Message};
use tower::ServiceExt;

use super::envelope::Envelope;
use super::noise::{self, MAX_MESSAGE_LEN};
use crate::app_state::AppState;
use crate::identity::pairing::{self, KeyPairingClaim};
use crate::identity::trusted_devices;
use crate::protocol::error::ZwiimError;
use crate::web::auth::{device_revoked, Principal, TunnelPeer, WS_CLOSE_DEVICE_REVOKED};
use crate::web::shutdown::ShutdownSignal;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const OUTBOUND_CAPACITY: usize = 256;
const STREAM_INPUT_CAPACITY: usize = 64;
/// In-memory pipe between an event stream and its `/zwiim/v1/ws` socket.
const STREAM_PIPE_BYTES: usize = 256 * 1024;
const EVENTS_PATH: &str = "ws://zwiim.tunnel/zwiim/v1/ws";

/// The first handshake payload of a device that pairs as it connects.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceHello {
    pub pair: PairingHello,
}

/// `device.pair`'s claim, minus the key: the handshake supplies that.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairingHello {
    pub code: String,
    pub device_id: String,
    #[serde(default)]
    pub device_name: Option<String>,
}

/// The `/zwiim/v1` router alone, with the extensions its handlers need.
pub fn router(state: Arc<AppState>, token: String, shutdown: Arc<ShutdownSignal>) -> Router {
    Router::new()
        .nest("/zwiim/v1", crate::protocol::router(token, state.db.conn.clone()))
        .layer(Extension(state))
        .layer(Extension(shutdown))
}

/// Serve one tunnel on `io` until either end closes it, the device is
/// revoked, or the host shuts down.
pub async fn serve<S>(state: Arc<AppState>, router: Router, shutdown: Arc<ShutdownSignal>, io: S)
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let Some(keys) = state.host_keys.get() else {
        tracing::warn!("[tunnel] refused a tunnel: host identity not loaded");
        return;
    };
    let host_id = match crate::identity::host::ensure_host_id(&state.db.conn).await {
        Ok(host_id) => host_id,
        Err(e) => return tracing::warn!("[tunnel] refused a tunnel: {e}"),
    };
    let db = state.db.conn.clone();
    let invitations = state.pairing_invitations.clone();
    let pairing_host_id = host_id.clone();
    let authorize = |device_key: [u8; 32], hello: Vec<u8>| async move {
        let key = URL_SAFE_NO_PAD.encode(device_key);
        if hello.is_empty() {
            return match trusted_devices::authenticate_key(&db, &key).await {
                Ok(device) => device,
                Err(e) => {
                    tracing::warn!("[tunnel] trusted-device lookup failed: {e}");
                    None
                }
            };
        }
        let Ok(DeviceHello { pair }) = serde_json::from_slice::<DeviceHello>(&hello) else {
            tracing::info!("[tunnel] refused a malformed device hello");
            return None;
        };
        let claim = KeyPairingClaim {
            code: pair.code,
            device_id: pair.device_id,
            device_name: pair.device_name,
            device_pubkey: key,
        };
        match pairing::pair_device(&db, &invitations, pairing_host_id, claim).await {
            Ok(grant) => Some(grant.device_id),
            Err(e) => {
                tracing::info!("[tunnel] pairing refused: {}", e.message);
                None
            }
        }
    };
    let secret = keys.static_secret_bytes();
    let accepted = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        noise::accept(io, &host_id, &secret, authorize),
    )
    .await;
    drop(secret);
    let (tunnel, device_id) = match accepted {
        Ok(Ok(accepted)) => accepted,
        Ok(Err(e)) => return tracing::info!("[tunnel] handshake refused: {e}"),
        Err(_) => return tracing::info!("[tunnel] handshake timed out"),
    };

    tracing::info!("[tunnel] device {device_id} connected");
    let router = router.layer(Extension(TunnelPeer { device_id: device_id.clone() }));
    let revoked = device_revoked(
        Some(Principal::Device(device_id.clone())),
        state.device_revocations.clone(),
        state.db.conn.clone(),
    );
    run(tunnel, router, revoked, &shutdown).await;
    tracing::info!("[tunnel] device {device_id} disconnected");
}

enum StreamInput {
    Frame(String),
    Close,
}

async fn run<S>(
    tunnel: noise::Tunnel<S>,
    router: Router,
    revoked: impl std::future::Future<Output = ()>,
    shutdown: &ShutdownSignal,
) where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut reader, mut writer) = tunnel.split();
    let (out, mut out_rx) = mpsc::channel::<Envelope>(OUTBOUND_CAPACITY);
    let writer = tokio::spawn(async move {
        while let Some(envelope) = out_rx.recv().await {
            if let Err(e) = writer.send(&envelope.encode()).await {
                tracing::info!("[tunnel] write failed: {e}");
                break;
            }
        }
        writer.close().await;
    });

    let mut tasks = JoinSet::new();
    let mut streams: HashMap<u64, mpsc::Sender<StreamInput>> = HashMap::new();
    tokio::pin!(revoked);
    loop {
        tokio::select! {
            _ = &mut revoked => {
                for &id in streams.keys() {
                    let _ = out
                        .send(Envelope::WsClose {
                            id,
                            code: Some(WS_CLOSE_DEVICE_REVOKED),
                            reason: Some("device_revoked".into()),
                        })
                        .await;
                }
                break;
            }
            _ = shutdown.wait() => break,
            // Reap finished tasks so a long tunnel does not accumulate them.
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            message = reader.recv() => {
                let message = match message {
                    Ok(Some(message)) => message,
                    Ok(None) => break,
                    Err(e) => {
                        tracing::info!("[tunnel] closing: {e}");
                        break;
                    }
                };
                let Some(envelope) = Envelope::decode(&message) else {
                    tracing::info!("[tunnel] closing: malformed envelope");
                    break;
                };
                match envelope {
                    Envelope::Req { id, op, body } => {
                        tasks.spawn(request(router.clone(), id, op, body, out.clone()));
                    }
                    Envelope::WsOpen { id, .. } => {
                        if streams.get(&id).is_some_and(|tx| !tx.is_closed()) {
                            tracing::info!("[tunnel] closing: stream id reused");
                            break;
                        }
                        // Forget streams whose socket has already ended.
                        streams.retain(|_, tx| !tx.is_closed());
                        let (tx, rx) = mpsc::channel(STREAM_INPUT_CAPACITY);
                        streams.insert(id, tx);
                        tasks.spawn(event_stream(router.clone(), id, rx, out.clone()));
                    }
                    Envelope::WsMsg { id, data } => {
                        if let Some(tx) = streams.get(&id) {
                            if tx.send(StreamInput::Frame(data)).await.is_err() {
                                streams.remove(&id);
                            }
                        }
                    }
                    Envelope::WsClose { id, .. } => {
                        if let Some(tx) = streams.remove(&id) {
                            let _ = tx.send(StreamInput::Close).await;
                        }
                    }
                    Envelope::Res { .. } => {
                        tracing::info!("[tunnel] closing: unexpected response envelope");
                        break;
                    }
                }
            }
        }
    }

    // Requests in flight are abandoned; event sockets close with their pipes.
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    drop(streams);
    drop(out);
    // Flush what is queued (the revocation closes) before ending the stream.
    let _ = tokio::time::timeout(Duration::from_secs(5), writer).await;
}

/// Operations are `[a-z0-9_.]` names; anything else is not one.
fn is_operation(op: &str) -> bool {
    !op.is_empty()
        && op.len() <= 64
        && op.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
}

fn error_body(e: ZwiimError) -> (u16, Value) {
    (
        e.status.as_u16(),
        json!({ "error": { "code": e.code, "message": e.message } }),
    )
}

/// One `req`: the operation's POST through the router.
async fn request(router: Router, id: u64, op: String, body: Value, out: mpsc::Sender<Envelope>) {
    let (status, body) = if is_operation(&op) {
        let request = http::Request::post(format!("/zwiim/v1/{op}"))
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .expect("valid operation request");
        let response = router.oneshot(request).await.expect("router is infallible");
        let status = response.status().as_u16();
        match axum::body::to_bytes(response.into_body(), MAX_MESSAGE_LEN).await {
            Ok(bytes) if bytes.is_empty() => (status, Value::Null),
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(body) => (status, body),
                Err(_) => error_body(ZwiimError::internal()),
            },
            Err(_) => error_body(ZwiimError::internal()),
        }
    } else {
        error_body(ZwiimError::unknown_operation())
    };
    let _ = out.send(Envelope::Res { id, status, body }).await;
}

/// One `ws.*` stream: a `/zwiim/v1/ws` socket over an in-memory connection
/// to the router, frames relayed verbatim both ways.
async fn event_stream(
    router: Router,
    id: u64,
    mut input: mpsc::Receiver<StreamInput>,
    out: mpsc::Sender<Envelope>,
) {
    let (client_io, server_io) = tokio::io::duplex(STREAM_PIPE_BYTES);
    let service = hyper_util::service::TowerToHyperService::new(router);
    let connection = tokio::spawn(async move {
        let _ = hyper::server::conn::http1::Builder::new()
            .serve_connection(hyper_util::rt::TokioIo::new(server_io), service)
            .with_upgrades()
            .await;
    });
    let _abort_connection = AbortOnDrop(connection);

    let mut upgrade = EVENTS_PATH.into_client_request().expect("valid events request");
    upgrade.headers_mut().insert(
        http::header::SEC_WEBSOCKET_PROTOCOL,
        http::HeaderValue::from_static(crate::protocol::ws::ZWIIM_EVENTS_PROTOCOL),
    );
    let socket = match tokio_tungstenite::client_async(upgrade, client_io).await {
        Ok((socket, _)) => socket,
        Err(tungstenite::Error::Http(response)) => {
            let status = response.status().as_u16();
            let body = response
                .body()
                .as_deref()
                .and_then(|b| serde_json::from_slice(b).ok());
            let _ = out.send(Envelope::WsOpen { id, status: Some(status), body }).await;
            return;
        }
        Err(e) => {
            tracing::info!("[tunnel] event stream failed to open: {e}");
            let (status, body) = error_body(ZwiimError::internal());
            let _ = out.send(Envelope::WsOpen { id, status: Some(status), body: Some(body) }).await;
            return;
        }
    };
    if out.send(Envelope::WsOpen { id, status: Some(101), body: None }).await.is_err() {
        return;
    }

    let (mut sink, mut frames) = socket.split();
    loop {
        tokio::select! {
            frame = frames.next() => match frame {
                Some(Ok(Message::Text(data))) => {
                    if out.send(Envelope::WsMsg { id, data: data.to_string() }).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(frame))) => {
                    let (code, reason) = match frame {
                        Some(f) => (Some(u16::from(f.code)), Some(f.reason.to_string())),
                        None => (None, None),
                    };
                    let _ = out.send(Envelope::WsClose { id, code, reason }).await;
                    break;
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => {
                    let _ = out.send(Envelope::WsClose { id, code: None, reason: None }).await;
                    break;
                }
            },
            input = input.recv() => match input {
                Some(StreamInput::Frame(data)) => {
                    if sink.send(Message::text(data)).await.is_err() {
                        let _ = out.send(Envelope::WsClose { id, code: None, reason: None }).await;
                        break;
                    }
                }
                Some(StreamInput::Close) | None => {
                    let _ = sink
                        .send(Message::Close(Some(CloseFrame {
                            code: tungstenite::protocol::frame::coding::CloseCode::Normal,
                            reason: "".into(),
                        })))
                        .await;
                    break;
                }
            },
        }
    }
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Accept tunnels on a loopback TCP address. For local verification only:
/// remote devices reach a host through the Cloud relay (`crate::cloud`).
pub async fn serve_test_listener(
    addr: SocketAddr,
    state: Arc<AppState>,
    router: Router,
    shutdown: Arc<ShutdownSignal>,
) -> std::io::Result<()> {
    if !addr.ip().is_loopback() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the tunnel test listener binds loopback addresses only",
        ));
    }
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("[tunnel] test listener on {addr}");
    loop {
        tokio::select! {
            _ = shutdown.wait() => return Ok(()),
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    tokio::spawn(serve(state.clone(), router.clone(), shutdown.clone(), stream));
                }
                Err(e) => {
                    tracing::warn!("[tunnel] accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
        }
    }
}
