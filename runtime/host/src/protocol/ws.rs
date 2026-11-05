//! Zwiim protocol event stream (`GET /zwiim/v1/ws`).
//!
//! Per-session subscriptions over the existing attach/replay machinery
//! (`web::ws_attach`): the client names a `session_id`, the host resolves it
//! to the live agent connection, and every frame is projected onto the Zwiim
//! vocabulary on the way out. Unlike the legacy `/ws/events` socket there is
//! no global firehose — a client receives only the sessions and terminals it
//! subscribed to.
//!
//! `host.subscribe` opts the socket into the host state feed
//! (`crate::host_feed`): a `host.snapshot` of every workspace, session and
//! terminal, then each change as a `host.event`. The feed has no replay; a
//! socket that falls behind gets a fresh snapshot.
//!
//! Replay cursors are `session_id + epoch + seq`. `seq` counts events on the
//! session's current agent connection, and `epoch` is an opaque token for
//! that connection. A cursor from an earlier connection (the agent was
//! resumed since) carries a stale epoch and gets a fresh snapshot.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use axum::{
    extract::{Extension, WebSocketUpgrade},
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::error::ZwiimError;
use super::events::{epoch_of, project_envelope, project_snapshot, project_terminal};
use crate::app_state::AppState;
use crate::host_feed::{HostEvent, HostSnapshot};
use crate::session;
use crate::web::auth::{device_revoked, Principal, WS_CLOSE_DEVICE_REVOKED};
use crate::web::shutdown::ShutdownSignal;
use crate::web::ws_attach::{self, DetachReason, ServerMsg, OUTBOUND_CAPACITY};

/// Subprotocol the Zwiim event socket negotiates.
pub const ZWIIM_EVENTS_PROTOCOL: &str = "zwiim-events";

/// How often a subscribed session's agent connection is marked active, so a
/// watched-but-quiet session outlives the idle sweep: a third of the host's
/// configured idle timeout, at most 30 s.
fn keepalive_interval() -> Duration {
    let max = Duration::from_secs(30);
    crate::acp::idle_sweep::idle_timeout_from_env().map_or(max, |timeout| {
        (timeout / 3).clamp(Duration::from_secs(1), max)
    })
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", deny_unknown_fields)]
enum ClientMsg {
    #[serde(rename = "session.subscribe")]
    SessionSubscribe {
        subscription_id: String,
        session_id: String,
        #[serde(default)]
        since_seq: Option<u64>,
        #[serde(default)]
        epoch: Option<String>,
    },
    #[serde(rename = "session.detach")]
    SessionDetach { subscription_id: String },
    #[serde(rename = "terminal.subscribe")]
    TerminalSubscribe { terminal_id: String },
    #[serde(rename = "terminal.unsubscribe")]
    TerminalUnsubscribe { terminal_id: String },
    #[serde(rename = "host.subscribe")]
    HostSubscribe {},
    #[serde(rename = "ping")]
    Ping,
}

struct Subscription {
    session_id: String,
    /// Internal agent connection. Host-side only; never serialized.
    connection_id: String,
    /// The session's workspace directory, for workspace-relative paths.
    root: Option<String>,
    handle: JoinHandle<()>,
    /// Per-socket spawn counter guarding forwarder cleanup signals (see
    /// `web::ws`); unrelated to the client-facing replay `epoch`.
    spawn_epoch: u64,
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Extension(state): Extension<Arc<AppState>>,
    Extension(shutdown_signal): Extension<Arc<ShutdownSignal>>,
    principal: Option<Extension<Principal>>,
) -> impl IntoResponse {
    let principal = principal.map(|Extension(p)| p);
    ws.protocols([ZWIIM_EVENTS_PROTOCOL])
        .on_upgrade(|socket| handle_socket(socket, state, shutdown_signal, principal))
}

fn detach_reason(reason: DetachReason) -> &'static str {
    match reason {
        DetachReason::ConnectionGone => "session_not_live",
        DetachReason::Lagged => "lagged",
        DetachReason::ServerShutdown => "server_shutdown",
    }
}

fn error_frame(subscription_id: Option<&str>, e: &ZwiimError) -> Value {
    json!({
        "type": "error",
        "subscription_id": subscription_id,
        "error": { "code": e.code, "message": e.message },
    })
}

/// Translate an internal attach frame into its Zwiim frame. `None` drops it:
/// events the vocabulary does not cover, and frames left in the queue by a
/// forwarder whose subscription has since been replaced or detached.
fn project_server_msg(msg: ServerMsg, subs: &HashMap<String, Subscription>) -> Option<Value> {
    let sub_for = |sid: &str, conn: &str| subs.get(sid).filter(|s| s.connection_id == conn);
    match msg {
        ServerMsg::Snapshot {
            subscription_id,
            connection_id,
            snapshot,
            event_seq,
        } => {
            let sub = sub_for(&subscription_id, &connection_id)?;
            Some(json!({
                "type": "snapshot",
                "subscription_id": subscription_id,
                "session_id": sub.session_id,
                "epoch": epoch_of(&connection_id),
                "snapshot": project_snapshot(&snapshot, sub.root.as_deref()),
                "event_seq": event_seq,
            }))
        }
        ServerMsg::Replay {
            subscription_id,
            connection_id,
            events,
            high_water_seq,
        } => {
            let sub = sub_for(&subscription_id, &connection_id)?;
            let events: Vec<_> = events
                .iter()
                .filter_map(|e| project_envelope(&sub.session_id, sub.root.as_deref(), e))
                .collect();
            Some(json!({
                "type": "replay",
                "subscription_id": subscription_id,
                "session_id": sub.session_id,
                "epoch": epoch_of(&connection_id),
                "events": events,
                "high_water_seq": high_water_seq,
            }))
        }
        ServerMsg::Event {
            subscription_id,
            envelope,
        } => {
            let sub = sub_for(&subscription_id, &envelope.connection_id)?;
            let event = project_envelope(&sub.session_id, sub.root.as_deref(), &envelope)?;
            Some(json!({
                "type": "event",
                "subscription_id": subscription_id,
                "event": event,
            }))
        }
        ServerMsg::Detached {
            subscription_id,
            reason,
        } => Some(json!({
            "type": "detached",
            "subscription_id": subscription_id,
            "reason": detach_reason(reason),
        })),
        ServerMsg::Pong => Some(json!({ "type": "pong" })),
    }
}

/// `msg`, unless it says its session's history was rewound: then a fresh
/// snapshot of the session in its place, which clients take as "reload the
/// transcript". Events already queued behind it carry `seq`s the snapshot
/// covers, so clients drop them.
async fn resnapshot_rewound(
    state: &AppState,
    msg: ServerMsg,
    subs: &HashMap<String, Subscription>,
) -> ServerMsg {
    let ServerMsg::Event {
        subscription_id,
        envelope,
    } = &msg
    else {
        return msg;
    };
    if !ws_attach::rewinds_history(envelope) {
        return msg;
    }
    let Some(sub) = subs
        .get(subscription_id)
        .filter(|s| s.connection_id == envelope.connection_id)
    else {
        return msg;
    };
    let fresh = ws_attach::snapshot(
        &state.connection_manager,
        subscription_id.clone(),
        sub.connection_id.clone(),
    )
    .await;
    fresh.unwrap_or(msg)
}

fn host_snapshot_frame(snapshot: HostSnapshot) -> Value {
    json!({
        "type": "host.snapshot",
        "epoch": snapshot.epoch,
        "seq": snapshot.seq,
        "workspaces": snapshot.workspaces,
        "layout": snapshot.layout,
        "sessions": snapshot.sessions,
        "terminals": snapshot.terminals,
        "prs": snapshot.prs,
        "keep_awake": snapshot.keep_awake,
    })
}

type FeedReceiver = tokio::sync::broadcast::Receiver<Arc<HostEvent>>;

/// Next host feed event; pending forever until the socket subscribed.
async fn next_feed_event(
    feed: &mut Option<FeedReceiver>,
) -> Result<Arc<HostEvent>, tokio::sync::broadcast::error::RecvError> {
    match feed {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Subscribe (or re-subscribe) the socket to the host feed and send its
/// snapshot. False when the socket is gone.
async fn subscribe_host(
    state: &Arc<AppState>,
    feed: &mut Option<FeedReceiver>,
    socket: &mut WebSocket,
) -> bool {
    match state.host_feed.subscribe(state).await {
        Ok((snapshot, rx)) => {
            *feed = Some(rx);
            send_json(socket, &host_snapshot_frame(snapshot)).await
        }
        Err(e) => {
            *feed = None;
            send_json(socket, &error_frame(None, &ZwiimError::from(e))).await
        }
    }
}

async fn send_json(socket: &mut WebSocket, frame: &Value) -> bool {
    socket
        .send(Message::Text(frame.to_string().into()))
        .await
        .is_ok()
}

async fn handle_socket(
    mut socket: WebSocket,
    state: Arc<AppState>,
    shutdown_signal: Arc<ShutdownSignal>,
    principal: Option<Principal>,
) {
    if shutdown_signal.is_triggered() {
        let _ = socket.send(Message::Close(None)).await;
        return;
    }

    // Terminal output rides the host's JSON broadcaster; only channels for
    // terminals this socket subscribed to are projected and forwarded.
    let mut terminal_rx = state.event_broadcaster.subscribe();
    let mut terminals: HashSet<String> = HashSet::new();
    let mut feed: Option<FeedReceiver> = None;

    let (outbound_tx, mut outbound_rx) = mpsc::channel::<ServerMsg>(OUTBOUND_CAPACITY);
    let (cleanup_tx, mut cleanup_rx) = mpsc::channel::<(String, u64)>(OUTBOUND_CAPACITY);
    let mut subs: HashMap<String, Subscription> = HashMap::new();
    let mut next_spawn_epoch: u64 = 0;

    let mut keepalive = tokio::time::interval(keepalive_interval());
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // A revoked device loses this socket at once, not at its next reconnect.
    let revoked = device_revoked(principal, state.device_revocations.clone(), state.db.conn.clone());
    tokio::pin!(revoked);

    loop {
        tokio::select! {
            _ = &mut revoked => {
                let _ = socket
                    .send(Message::Close(Some(CloseFrame {
                        code: WS_CLOSE_DEVICE_REVOKED,
                        reason: "device_revoked".into(),
                    })))
                    .await;
                break;
            }
            _ = shutdown_signal.wait() => {
                for sub_id in subs.keys() {
                    let frame = project_server_msg(
                        ServerMsg::Detached {
                            subscription_id: sub_id.clone(),
                            reason: DetachReason::ServerShutdown,
                        },
                        &subs,
                    );
                    if let Some(frame) = frame {
                        let _ = send_json(&mut socket, &frame).await;
                    }
                }
                let _ = socket.send(Message::Close(None)).await;
                break;
            }

            _ = keepalive.tick() => {
                for sub in subs.values() {
                    state.connection_manager.touch(&sub.connection_id).await;
                }
            }

            cleanup = cleanup_rx.recv() => {
                if let Some((sub_id, spawn_epoch)) = cleanup {
                    if subs.get(&sub_id).is_some_and(|s| s.spawn_epoch == spawn_epoch) {
                        subs.remove(&sub_id);
                    }
                }
            }

            outgoing = outbound_rx.recv() => {
                let Some(msg) = outgoing else { break };
                let msg = resnapshot_rewound(&state, msg, &subs).await;
                if let Some(frame) = project_server_msg(msg, &subs) {
                    if !send_json(&mut socket, &frame).await {
                        break;
                    }
                }
            }

            result = terminal_rx.recv() => {
                match result {
                    Ok(event) => {
                        let Some(tev) = project_terminal(&event.channel, &event.payload) else {
                            continue;
                        };
                        if !terminals.contains(&tev.terminal_id) {
                            continue;
                        }
                        let frame = json!({ "type": "terminal", "event": tev });
                        if !send_json(&mut socket, &frame).await {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("[zwiim ws] terminal receiver lagged: skipped {n} events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }

            result = next_feed_event(&mut feed) => {
                match result {
                    Ok(event) => {
                        let frame = json!({
                            "type": "host.event",
                            "epoch": state.host_feed.epoch(),
                            "event": event,
                        });
                        if !send_json(&mut socket, &frame).await {
                            break;
                        }
                    }
                    // Fell behind: start over from a fresh snapshot.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if !subscribe_host(&state, &mut feed, &mut socket).await {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => feed = None,
                }
            }

            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMsg>(&text) {
                            Ok(cmsg) => {
                                handle_client_msg(
                                    cmsg,
                                    &state,
                                    &outbound_tx,
                                    &cleanup_tx,
                                    &mut subs,
                                    &mut terminals,
                                    &mut feed,
                                    &mut next_spawn_epoch,
                                    &mut socket,
                                )
                                .await;
                            }
                            Err(e) => {
                                let err = ZwiimError::invalid_request(format!("Invalid message: {e}"));
                                if !send_json(&mut socket, &error_frame(None, &err)).await {
                                    break;
                                }
                            }
                        }
                    }
                    Some(Ok(_)) => {}
                    _ => break,
                }
            }
        }
    }

    for (_, sub) in subs.drain() {
        sub.handle.abort();
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_client_msg(
    msg: ClientMsg,
    state: &Arc<AppState>,
    outbound_tx: &mpsc::Sender<ServerMsg>,
    cleanup_tx: &mpsc::Sender<(String, u64)>,
    subs: &mut HashMap<String, Subscription>,
    terminals: &mut HashSet<String>,
    feed: &mut Option<FeedReceiver>,
    next_spawn_epoch: &mut u64,
    socket: &mut WebSocket,
) {
    match msg {
        ClientMsg::SessionSubscribe {
            subscription_id,
            session_id,
            since_seq,
            epoch,
        } => {
            if let Some(old) = subs.remove(&subscription_id) {
                old.handle.abort();
            }
            let connection_id = match session::live_connection_id(
                &state.db.conn,
                &state.connection_manager,
                &session_id,
            )
            .await
            {
                Ok(Some(id)) => id,
                Ok(None) => {
                    let _ = outbound_tx
                        .send(ServerMsg::Detached {
                            subscription_id,
                            reason: DetachReason::ConnectionGone,
                        })
                        .await;
                    return;
                }
                Err(e) => {
                    let err = ZwiimError::from(e);
                    let _ = send_json(socket, &error_frame(Some(&subscription_id), &err)).await;
                    return;
                }
            };

            let root = session::workspace_dir(&state.db.conn, &session_id).await;

            // A cursor is only meaningful against the connection it was
            // taken on; anything else gets a fresh snapshot.
            let cursor = match (since_seq, epoch) {
                (Some(seq), Some(epoch)) if epoch == epoch_of(&connection_id) => Some(seq),
                _ => None,
            };

            match ws_attach::handle_attach(
                &state.connection_manager,
                state.acp_event_bus.metrics(),
                subscription_id.clone(),
                connection_id.clone(),
                cursor,
            )
            .await
            {
                Ok(outcome) => {
                    state.connection_manager.touch(&connection_id).await;
                    *next_spawn_epoch = next_spawn_epoch.wrapping_add(1);
                    let spawn_epoch = *next_spawn_epoch;
                    // Register before queueing the initial frame so the
                    // outbound projection can resolve the subscription.
                    let handle = ws_attach::spawn_forwarder(
                        subscription_id.clone(),
                        spawn_epoch,
                        state.acp_event_bus.metrics().clone(),
                        outcome.receiver,
                        outbound_tx.clone(),
                        cleanup_tx.clone(),
                    );
                    subs.insert(
                        subscription_id,
                        Subscription {
                            session_id,
                            connection_id,
                            root,
                            handle,
                            spawn_epoch,
                        },
                    );
                    // The forwarder and this send share one FIFO queue, but
                    // the forwarder may enqueue first; send the initial frame
                    // straight to the socket so it always precedes events.
                    if let Some(frame) = project_server_msg(outcome.initial_msg, subs) {
                        let _ = send_json(socket, &frame).await;
                    }
                }
                Err(reason) => {
                    let _ = outbound_tx
                        .send(ServerMsg::Detached {
                            subscription_id,
                            reason,
                        })
                        .await;
                }
            }
        }
        ClientMsg::SessionDetach { subscription_id } => {
            if let Some(sub) = subs.remove(&subscription_id) {
                sub.handle.abort();
            }
        }
        ClientMsg::TerminalSubscribe { terminal_id } => {
            terminals.insert(terminal_id);
        }
        ClientMsg::TerminalUnsubscribe { terminal_id } => {
            terminals.remove(&terminal_id);
        }
        ClientMsg::HostSubscribe {} => {
            subscribe_host(state, feed, socket).await;
        }
        ClientMsg::Ping => {
            for sub in subs.values() {
                state.connection_manager.touch(&sub.connection_id).await;
            }
            let _ = send_json(socket, &json!({ "type": "pong" })).await;
        }
    }
}
