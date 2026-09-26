//! The tunnel end to end, in memory: a device paired by key, the Noise
//! tunnel, the host adapter and the `/zwiim/v1` router. Every operation and
//! event is checked against the same call made directly to the host.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::task::JoinHandle;

use super::client::{EventStream, StreamEvent, TunnelClient};
use super::noise::TunnelError;
use crate::acp::connection::ConnectionCommand;
use crate::identity::device::DeviceIdentity;
use crate::identity::host_keys::HostKeys;
use crate::identity::pairing::{KeyPairingGrant, KeyPairingInvitation};
use crate::protocol::events::tests::assert_no_internal_keys;
use crate::protocol::tests::{delta, host, Host, CONN, HOST_TOKEN};
use crate::web::shutdown::ShutdownSignal;

pub(crate) const WAIT: Duration = Duration::from_secs(5);

/// A device that paired with `h` by key, the way the QR flow does.
pub(crate) async fn paired_device(h: &Host) -> (DeviceIdentity, String) {
    let (device, _, host_id) = paired_device_at(h).await;
    (device, host_id)
}

pub(crate) async fn paired_device_at(h: &Host) -> (DeviceIdentity, std::path::PathBuf, String) {
    let (mut device, path) = new_device(h);
    let invite = h.ok("device.pair_invite", json!({})).await;
    let res = h
        .server
        .post("/zwiim/v1/device.pair")
        .json(&json!({
            "code": invite["code"],
            "device_id": device.device_id,
            "device_name": "Phone",
            "device_pubkey": device.static_public().unwrap(),
        }))
        .await;
    assert_eq!(res.status_code(), StatusCode::OK);
    let grant: KeyPairingGrant = res.json();
    let invitation: KeyPairingInvitation = serde_json::from_value(invite).unwrap();
    device.record_key_pairing(&path, &invitation, &grant).unwrap();
    (device, path, invitation.host_id)
}

pub(crate) fn new_device(h: &Host) -> (DeviceIdentity, std::path::PathBuf) {
    let path = h._dir.path().join(format!("device-{}.json", uuid::Uuid::new_v4()));
    (DeviceIdentity::load_or_create(&path, "Phone").unwrap(), path)
}

/// One tunnel being served by the host.
struct Served {
    shutdown: Arc<ShutdownSignal>,
    task: JoinHandle<()>,
}

impl Served {
    /// The host has let go of the tunnel.
    async fn ended(self) {
        tokio::time::timeout(WAIT, self.task)
            .await
            .expect("the host ends the tunnel")
            .unwrap();
    }
}

/// Serve a tunnel on the host; returns the device's end of the stream.
fn serve(h: &Host) -> (DuplexStream, Served) {
    let (device_io, host_io) = tokio::io::duplex(1 << 20);
    let shutdown = Arc::new(ShutdownSignal::new());
    let router = super::host::router(h.state.clone(), HOST_TOKEN.to_string(), shutdown.clone());
    let task = tokio::spawn(super::host::serve(h.state.clone(), router, shutdown.clone(), host_io));
    (device_io, Served { shutdown, task })
}

async fn connect(h: &Host, device: &DeviceIdentity, host_id: &str) -> (TunnelClient, Served) {
    let (io, served) = serve(h);
    let tunnel = device.connect_tunnel(io, host_id).await.unwrap();
    (TunnelClient::start(tunnel), served)
}

/// A wire tap between the device and the host: records both directions and
/// can flip one byte of the device's next frame.
#[derive(Clone)]
struct Tap {
    up: Arc<Mutex<Vec<u8>>>,
    down: Arc<Mutex<Vec<u8>>>,
    sent: Arc<AtomicUsize>,
    flip_at: Arc<AtomicUsize>,
}

fn tap(host_end: DuplexStream) -> (DuplexStream, Tap) {
    let tap = Tap {
        up: Default::default(),
        down: Default::default(),
        sent: Default::default(),
        flip_at: Arc::new(AtomicUsize::new(usize::MAX)),
    };
    let (device_end, relay_end) = tokio::io::duplex(1 << 20);
    let (mut from_device, mut to_device) = tokio::io::split(relay_end);
    let (mut from_host, mut to_host) = tokio::io::split(host_end);
    let t = tap.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 16 * 1024];
        while let Ok(n @ 1..) = from_device.read(&mut buf).await {
            let start = t.sent.fetch_add(n, Ordering::SeqCst);
            let flip = t.flip_at.load(Ordering::SeqCst);
            if (start..start + n).contains(&flip) {
                buf[flip - start] ^= 0x01;
            }
            t.up.lock().unwrap().extend_from_slice(&buf[..n]);
            if to_host.write_all(&buf[..n]).await.is_err() {
                break;
            }
        }
        let _ = to_host.shutdown().await;
    });
    let t = tap.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 16 * 1024];
        while let Ok(n @ 1..) = from_host.read(&mut buf).await {
            t.down.lock().unwrap().extend_from_slice(&buf[..n]);
            if to_device.write_all(&buf[..n]).await.is_err() {
                break;
            }
        }
        let _ = to_device.shutdown().await;
    });
    (device_end, tap)
}

impl Tap {
    /// Corrupt the device's next frame, past its length prefix.
    fn tamper_next(&self) {
        self.flip_at
            .store(self.sent.load(Ordering::SeqCst) + 8, Ordering::SeqCst);
    }

    fn seen(&self, needle: &[u8]) -> bool {
        [&self.up, &self.down]
            .iter()
            .any(|dir| dir.lock().unwrap().windows(needle.len()).any(|w| w == needle))
    }
}

/// No host path or connection id in `text`.
pub(crate) fn assert_clean(h: &Host, text: &str) {
    let tmp = h._dir.path();
    assert!(
        !text.contains(&h.root)
            && !text.contains(tmp.to_str().unwrap())
            && !text.contains(tmp.canonicalize().unwrap().to_str().unwrap())
            && !text.contains(CONN),
        "leaked a host path or connection id: {text}"
    );
}

pub(crate) async fn call(h: &Host, c: &TunnelClient, op: &str, body: Value) -> (u16, Value) {
    let (status, body) = tokio::time::timeout(WAIT, c.request(op, body))
        .await
        .expect("response within 5s")
        .unwrap();
    assert_clean(h, &body.to_string());
    assert_no_internal_keys(&body);
    (status, body)
}

pub(crate) async fn ok(h: &Host, c: &TunnelClient, op: &str, body: Value) -> Value {
    let (status, body) = call(h, c, op, body).await;
    assert_eq!(status, 200, "{op}: {body}");
    body
}

/// The same operation made directly to the host with its own token.
pub(crate) async fn direct(h: &Host, op: &str, body: Value) -> (u16, Value) {
    let res = h
        .server
        .post(&format!("/zwiim/v1/{op}"))
        .authorization_bearer(HOST_TOKEN)
        .json(&body)
        .await;
    (res.status_code().as_u16(), res.json())
}

pub(crate) async fn events(c: &TunnelClient) -> EventStream {
    c.open_events().await.unwrap().expect("the event socket opens")
}

pub(crate) async fn frame(h: &Host, stream: &mut EventStream) -> Value {
    match tokio::time::timeout(WAIT, stream.recv()).await.expect("frame within 5s") {
        Some(StreamEvent::Frame(text)) => {
            assert_clean(h, &text);
            let frame: Value = serde_json::from_str(&text).unwrap();
            assert_no_internal_keys(&frame);
            frame
        }
        other => panic!("expected a frame, got {other:?}"),
    }
}

/// Skip frames until `pred` matches one.
pub(crate) async fn frame_where(h: &Host, stream: &mut EventStream, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..200 {
        let frame = frame(h, stream).await;
        if pred(&frame) {
            return frame;
        }
    }
    panic!("no {what}");
}

pub(crate) async fn closed(stream: &mut EventStream) -> (Option<u16>, Option<String>) {
    loop {
        match tokio::time::timeout(WAIT, stream.recv()).await.expect("close within 5s") {
            Some(StreamEvent::Closed { code, reason }) => return (code, reason),
            Some(StreamEvent::Frame(_)) => {}
            None => panic!("stream ended without a close"),
        }
    }
}

#[tokio::test]
async fn operations_through_the_tunnel_are_the_hosts_own() {
    let h = host(false).await;
    let (device, host_id) = paired_device(&h).await;
    let (c, _served) = connect(&h, &device, &host_id).await;
    let wid = h.workspace_id.clone();
    std::fs::write(format!("{}/notes.txt", h.root), "hello tunnel\n").unwrap();

    for (op, body) in [
        ("host.hello", json!({})),
        ("workspace.list", json!({})),
        ("workspace.open", json!({"workspace_id": wid})),
        ("workspace.tree", json!({"workspace_id": wid})),
        ("files.read", json!({"workspace_id": wid, "path": "notes.txt"})),
        ("git.status", json!({"workspace_id": wid, "show_all_untracked": true})),
        ("git.log", json!({"workspace_id": wid, "limit": 5})),
        ("git.branches", json!({"workspace_id": wid})),
        ("session.list", json!({"workspace_id": wid})),
        // Errors keep their status and shape.
        ("files.read", json!({"workspace_id": wid, "path": "../../etc/passwd"})),
        ("workspace.open", json!({"workspace_id": "nope"})),
        ("session.start", json!({"workspace_id": wid, "agent": "claude_code", "blocks": []})),
        ("session.history", json!({"session_id": "nope"})),
        ("git.log", json!({"workspace_id": wid, "bogus": 1})),
        ("nope.nope", json!({})),
    ] {
        assert_eq!(call(&h, &c, op, body.clone()).await, direct(&h, op, body).await, "{op}");
    }
    assert_eq!(ok(&h, &c, "host.hello", json!({})).await["host_id"], host_id.as_str());
    let read = ok(&h, &c, "files.read", json!({"workspace_id": wid, "path": "notes.txt"})).await;
    assert_eq!(read["content"], "hello tunnel\n");

    // Mutations land on the host.
    ok(
        &h,
        &c,
        "files.write",
        json!({"workspace_id": wid, "path": "notes.txt", "content": "changed\n", "etag": read["etag"]}),
    )
    .await;
    assert_eq!(std::fs::read_to_string(format!("{}/notes.txt", h.root)).unwrap(), "changed\n");
    let wt = ok(&h, &c, "workspace.create_worktree", json!({"workspace_id": wid})).await;
    assert!(direct(&h, "workspace.list", json!({})).await.1.to_string().contains(wt["workspace_id"].as_str().unwrap()));

    // Only `/zwiim/v1` operations are reachable.
    for op in ["../../api/list_folders", "../api/trusted_device_list", "Host.Hello", "", "host.hello?x=1"] {
        let (status, body) = call(&h, &c, op, json!({})).await;
        assert_eq!((status, &body["error"]["code"]), (404, &json!("unknown_operation")), "{op:?}");
    }
}

#[tokio::test]
async fn the_device_is_authorized_as_itself() {
    let h = host(false).await;
    let (device, host_id) = paired_device(&h).await;
    let (c, _served) = connect(&h, &device, &host_id).await;

    for (op, body) in [
        ("device.pair_invite", json!({})),
        ("device.list", json!({})),
        ("device.revoke", json!({"device_id": device.device_id})),
        ("host.claim", json!({"user_id": "u-mallory"})),
    ] {
        assert_eq!(
            call(&h, &c, op, body).await,
            (
                403,
                json!({"error": {"code": "permission_denied", "message": "This operation requires local host authority"}})
            ),
            "{op}"
        );
    }
    let devices = h.ok("device.list", json!({})).await;
    assert_eq!(devices["devices"][0]["status"], "trusted");
    assert!(devices["devices"][0]["last_seen_at"].is_string(), "{devices}");
    assert_eq!(h.ok("host.hello", json!({})).await["owner"], Value::Null);
}

#[tokio::test]
async fn responses_are_correlated_with_their_requests() {
    let h = host(false).await;
    let (device, host_id) = paired_device(&h).await;
    let (c, _served) = connect(&h, &device, &host_id).await;
    let wid = h.workspace_id.clone();
    for i in 0..24 {
        std::fs::write(format!("{}/f{i}.txt", h.root), format!("content-{i}")).unwrap();
    }
    let git_status = direct(&h, "git.status", json!({"workspace_id": wid})).await.1;

    let calls = (0..24).map(|i| {
        let (c, wid) = (&c, wid.clone());
        async move {
            // Interleave slow and fast operations so responses can overtake.
            let (op, body) = match i % 3 {
                0 => ("git.status", json!({"workspace_id": wid})),
                1 => ("host.hello", json!({})),
                _ => ("files.read", json!({"workspace_id": wid, "path": format!("f{i}.txt")})),
            };
            (i, c.request(op, body).await.unwrap())
        }
    });
    for (i, (status, body)) in futures_util::future::join_all(calls).await {
        assert_eq!(status, 200, "{i}: {body}");
        match i % 3 {
            0 => assert_eq!(body, git_status, "{i}"),
            1 => assert_eq!(body["host_id"], host_id.as_str(), "{i}"),
            _ => assert_eq!(body["content"], format!("content-{i}"), "{i}"),
        }
    }
}

#[tokio::test]
async fn sessions_send_and_history_through_the_tunnel() {
    let h = host(false).await;
    let (device, host_id) = paired_device(&h).await;
    let (c, _served) = connect(&h, &device, &host_id).await;
    let (sid, mut rx) = h.live_session().await;

    let sent = ok(
        &h,
        &c,
        "session.send",
        json!({"session_id": sid, "blocks": [{"type": "text", "text": "hi"}], "client_message_id": "m1"}),
    )
    .await;
    assert_eq!(sent, json!({"session_id": sid}));
    assert!(matches!(
        tokio::time::timeout(WAIT, rx.recv()).await.unwrap(),
        Some(ConnectionCommand::Prompt { .. })
    ));

    let history = ok(&h, &c, "session.history", json!({"session_id": sid})).await;
    assert_eq!(history["session_id"], sid.as_str());
    assert_eq!(
        (200, history),
        direct(&h, "session.history", json!({"session_id": sid})).await
    );
    let list = ok(&h, &c, "session.list", json!({"workspace_id": h.workspace_id})).await;
    assert!(list.as_array().unwrap().iter().any(|s| s["session_id"] == sid.as_str() && s["live"] == true));
}

#[tokio::test]
async fn terminals_through_the_tunnel() {
    let h = host(false).await;
    let (device, host_id) = paired_device(&h).await;
    let (c, _served) = connect(&h, &device, &host_id).await;

    let spawned = ok(
        &h,
        &c,
        "terminal.spawn",
        json!({"workspace_id": h.workspace_id, "shell": "/bin/sh"}),
    )
    .await;
    let tid = spawned["terminal_id"].as_str().unwrap().to_string();
    let list = ok(&h, &c, "terminal.list", json!({})).await;
    assert!(list.as_array().unwrap().iter().any(|t| t["terminal_id"] == tid.as_str()));

    let mut stream = events(&c).await;
    stream.send(&json!({"op": "terminal.subscribe", "terminal_id": tid})).unwrap();
    stream.send(&json!({"op": "ping"})).unwrap();
    frame_where(&h, &mut stream, "pong", |f| f["type"] == "pong").await;
    ok(
        &h,
        &c,
        "terminal.write",
        json!({"terminal_id": tid, "data": "echo zw-$((40+2)); exit\n"}),
    )
    .await;

    let mut output = String::new();
    loop {
        let f = frame(&h, &mut stream).await;
        assert_eq!(f["type"], "terminal", "{f}");
        let ev = &f["event"];
        assert_eq!(ev["terminal_id"], tid.as_str());
        match ev["type"].as_str().unwrap() {
            "terminal.output" => {
                assert!(ev["seq"].is_u64());
                output.push_str(ev["payload"]["data"].as_str().unwrap());
            }
            "terminal.exit" => break,
            other => panic!("unexpected {other}"),
        }
    }
    assert!(output.contains("zw-42"), "{output}");

    let other = ok(
        &h,
        &c,
        "terminal.spawn",
        json!({"workspace_id": h.workspace_id, "shell": "/bin/sh"}),
    )
    .await["terminal_id"]
        .clone();
    ok(&h, &c, "terminal.kill", json!({"terminal_id": other})).await;
    stream.close();
}

#[tokio::test]
async fn the_host_feed_through_the_tunnel_converges_with_local_clients() {
    let h = host(false).await;
    let (device, host_id) = paired_device(&h).await;
    let (c, _served) = connect(&h, &device, &host_id).await;

    let mut stream = events(&c).await;
    stream.send(&json!({"op": "host.subscribe"})).unwrap();
    let snapshot = frame(&h, &mut stream).await;
    assert_eq!(snapshot["type"], "host.snapshot", "{snapshot}");
    assert_eq!(snapshot["workspaces"], h.ok("workspace.list", json!({})).await);
    assert_eq!(snapshot["terminals"], h.ok("terminal.list", json!({})).await);
    assert!(snapshot["epoch"].is_string() && snapshot["seq"].is_u64());

    // A change made through the tunnel…
    let wt = ok(&h, &c, "workspace.create_worktree", json!({"workspace_id": h.workspace_id})).await;
    let wid = wt["workspace_id"].clone();
    let changed = frame_where(&h, &mut stream, "workspace.changed", |f| {
        f["type"] == "host.event"
            && f["event"]["type"] == "workspace.changed"
            && f["event"]["payload"]["workspace"]["workspace_id"] == wid
    })
    .await;
    assert_eq!(changed["epoch"], snapshot["epoch"]);
    assert!(changed["event"]["seq"].as_u64().unwrap() > snapshot["seq"].as_u64().unwrap());
    assert_eq!(changed["event"]["payload"]["workspace"], wt);

    // …and one made by the local desktop reach the device alike.
    let tid = h
        .ok(
            "terminal.spawn",
            json!({"workspace_id": h.workspace_id, "shell": "/bin/sh"}),
        )
        .await["terminal_id"]
        .clone();
    frame_where(&h, &mut stream, "terminal.changed", |f| {
        f["event"]["type"] == "terminal.changed" && f["event"]["payload"]["terminal"]["terminal_id"] == tid
    })
    .await;
    h.ok("terminal.kill", json!({"terminal_id": tid})).await;
    frame_where(&h, &mut stream, "terminal.removed", |f| {
        f["event"]["type"] == "terminal.removed" && f["event"]["payload"]["terminal_id"] == tid
    })
    .await;
}

#[tokio::test]
async fn session_snapshot_then_replay_after_reconnecting_with_cursors() {
    let h = host(true).await;
    let (device, host_id) = paired_device(&h).await;
    let (sid, _rx) = h.live_session().await;
    h.emit(delta("before ")).await;

    let (c, served) = connect(&h, &device, &host_id).await;
    let mut stream = events(&c).await;
    stream
        .send(&json!({"op": "session.subscribe", "subscription_id": "a", "session_id": sid}))
        .unwrap();
    let snap = frame(&h, &mut stream).await;
    assert_eq!(snap["type"], "snapshot", "{snap}");
    assert_eq!(snap["session_id"], sid.as_str());
    let epoch = snap["epoch"].as_str().unwrap().to_string();
    let base = snap["event_seq"].as_u64().unwrap();

    h.emit(delta("hello")).await;
    let ev = frame(&h, &mut stream).await;
    assert_eq!(ev["type"], "event");
    assert_eq!(ev["event"]["type"], "agent.message_delta");
    assert_eq!(ev["event"]["payload"], json!({"text": "hello"}));
    assert!(ev["event"]["seq"].as_u64().unwrap() > base);

    // The tunnel goes away entirely; events keep happening on the host.
    c.close().await;
    served.ended().await;
    h.emit(delta(" world")).await;

    // A new tunnel resumes from the cursors the device kept.
    let (c, _served) = connect(&h, &device, &host_id).await;
    let mut stream = events(&c).await;
    let resume = json!({"op": "session.subscribe", "subscription_id": "b", "session_id": sid, "since_seq": base, "epoch": epoch});
    stream.send(&resume).unwrap();
    let replay = frame(&h, &mut stream).await;
    assert_eq!(replay["type"], "replay", "{replay}");
    assert_eq!(replay["epoch"], epoch.as_str());
    let texts: Vec<_> = replay["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["payload"]["text"].clone())
        .collect();
    assert_eq!(texts, [json!("hello"), json!(" world")]);

    // The same cursor on a local socket replays the same events.
    let proto = format!("zwiim-events, zwiim-token.{}", URL_SAFE_NO_PAD.encode(HOST_TOKEN));
    let mut local = h
        .server
        .get_websocket("/zwiim/v1/ws")
        .add_header("sec-websocket-protocol", proto)
        .await
        .into_websocket()
        .await;
    local.send_json(&resume).await;
    let local_replay: Value = serde_json::from_str(&local.receive_text().await).unwrap();
    assert_eq!(local_replay, replay);

    // A stale epoch, or none, falls back to a snapshot.
    for extra in [json!({"epoch": "0000000000000000"}), json!({})] {
        let mut msg = json!({"op": "session.subscribe", "subscription_id": "c", "session_id": sid, "since_seq": base});
        msg.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        stream.send(&msg).unwrap();
        let f = frame(&h, &mut stream).await;
        assert_eq!(f["type"], "snapshot", "{f}");
    }
}

#[tokio::test]
async fn unknown_revoked_and_misdirected_devices_are_refused() {
    let h = host(false).await;
    let invitation: KeyPairingInvitation =
        serde_json::from_value(h.ok("device.pair_invite", json!({})).await).unwrap();
    let host_id = invitation.host_id.clone();

    // A device that knows the host's key but never paired.
    let (mut stranger, path) = new_device(&h);
    let grant = KeyPairingGrant { host_id: host_id.clone(), device_id: stranger.device_id.clone() };
    stranger.record_key_pairing(&path, &invitation, &grant).unwrap();
    let (io, served) = serve(&h);
    assert!(matches!(stranger.connect_tunnel(io, &host_id).await, Err(TunnelError::Refused)));
    served.ended().await;

    // A revoked device.
    let (device, _) = paired_device(&h).await;
    h.ok("device.revoke", json!({"device_id": device.device_id})).await;
    let (io, served) = serve(&h);
    assert!(matches!(device.connect_tunnel(io, &host_id).await, Err(TunnelError::Refused)));
    served.ended().await;

    // A paired device holding the wrong host key (another host's QR, or a
    // host impersonating this one) gets no answer at all.
    let (mut misled, path, _) = paired_device_at(&h).await;
    let wrong = KeyPairingInvitation {
        host_static_pubkey: HostKeys::generate().static_pubkey(),
        ..invitation.clone()
    };
    let grant = KeyPairingGrant { host_id: host_id.clone(), device_id: misled.device_id.clone() };
    misled.record_key_pairing(&path, &wrong, &grant).unwrap();
    let (io, served) = serve(&h);
    assert!(matches!(misled.connect_tunnel(io, &host_id).await, Err(TunnelError::Handshake)));
    served.ended().await;

    // Nobody was let in.
    let devices = h.ok("device.list", json!({})).await["devices"].clone();
    assert!(devices.as_array().unwrap().iter().all(|d| d["last_seen_at"].is_null()), "{devices}");
}

#[tokio::test]
async fn a_device_pairs_as_it_connects() {
    let h = host(false).await;
    let invitation: KeyPairingInvitation =
        serde_json::from_value(h.ok("device.pair_invite", json!({})).await).unwrap();
    let (mut device, path) = new_device(&h);
    let (io, served) = serve(&h);
    let tunnel = device.pair_tunnel(&path, io, &invitation).await.unwrap();
    let c = TunnelClient::start(tunnel);
    // The tunnel is open as the newly trusted device.
    assert_eq!(ok(&h, &c, "host.hello", json!({})).await["host_id"], invitation.host_id.as_str());
    let devices = h.ok("device.list", json!({})).await["devices"].clone();
    let listed = devices.as_array().unwrap().iter().find(|d| d["device_id"] == device.device_id.as_str()).unwrap();
    assert_eq!(listed["status"], "trusted");
    assert_eq!(listed["device_pubkey"], device.static_public().unwrap().as_str());
    // Persisted: the device reconnects without the code.
    let reloaded = DeviceIdentity::load_or_create(&path, "Phone").unwrap();
    assert_eq!(reloaded.pinned_host_key(&invitation.host_id), Some(invitation.host_static_pubkey.as_str()));
    drop(c);
    served.ended().await;
    let (c, served) = connect(&h, &reloaded, &invitation.host_id).await;
    ok(&h, &c, "workspace.list", json!({})).await;
    drop(c);
    served.ended().await;

    // The code is spent: another device presenting it is refused, and never trusted.
    let (mut other, path) = new_device(&h);
    let (io, served) = serve(&h);
    assert!(matches!(other.pair_tunnel(&path, io, &invitation).await, Err(TunnelError::Refused)));
    served.ended().await;
    let devices = h.ok("device.list", json!({})).await["devices"].clone();
    assert_eq!(devices.as_array().unwrap().len(), 1, "{devices}");
    assert_eq!(other.pinned_host_key(&invitation.host_id), None);
}

#[tokio::test]
async fn bad_pairing_hellos_are_refused() {
    let h = host(false).await;
    let invitation: KeyPairingInvitation =
        serde_json::from_value(h.ok("device.pair_invite", json!({})).await).unwrap();
    // A code the host never issued.
    let (mut device, path) = new_device(&h);
    let forged = KeyPairingInvitation { code: "not-a-code".into(), ..invitation.clone() };
    let (io, served) = serve(&h);
    assert!(matches!(device.pair_tunnel(&path, io, &forged).await, Err(TunnelError::Refused)));
    served.ended().await;
    // A hello that is not a pairing claim.
    let secret = device.static_secret_bytes_for_test();
    let host_key: [u8; 32] = URL_SAFE_NO_PAD.decode(&invitation.host_static_pubkey).unwrap().try_into().unwrap();
    let (io, served) = serve(&h);
    let r = super::noise::connect_with(io, &invitation.host_id, &secret, &host_key, b"{\"pair\":{\"code\":1}}").await;
    assert!(matches!(r, Err(TunnelError::Refused)));
    served.ended().await;
    // The genuine invitation is still good after both.
    let (io, served) = serve(&h);
    let tunnel = device.pair_tunnel(&path, io, &invitation).await.unwrap();
    drop(tunnel);
    served.ended().await;
    // A revoked device can pair again only with a fresh code.
    h.ok("device.revoke", json!({"device_id": device.device_id})).await;
    let (io, served) = serve(&h);
    assert!(matches!(device.connect_tunnel(io, &invitation.host_id).await, Err(TunnelError::Refused)));
    served.ended().await;
    let fresh: KeyPairingInvitation =
        serde_json::from_value(h.ok("device.pair_invite", json!({})).await).unwrap();
    let (io, served) = serve(&h);
    drop(device.pair_tunnel(&path, io, &fresh).await.unwrap());
    served.ended().await;
}

#[tokio::test]
async fn revoking_a_connected_device_ends_its_tunnel() {
    let h = host(false).await;
    let (device, host_id) = paired_device(&h).await;
    let (c, served) = connect(&h, &device, &host_id).await;
    let mut stream = events(&c).await;
    stream.send(&json!({"op": "host.subscribe"})).unwrap();
    assert_eq!(frame(&h, &mut stream).await["type"], "host.snapshot");

    h.ok("device.revoke", json!({"device_id": device.device_id})).await;
    assert_eq!(closed(&mut stream).await, (Some(4403), Some("device_revoked".into())));
    served.ended().await;
    assert!(matches!(c.request("host.hello", json!({})).await, Err(TunnelError::Closed)));
    assert!(c.is_closed());
}

#[tokio::test]
async fn a_tampered_message_ends_the_tunnel() {
    let h = host(false).await;
    let (device, host_id) = paired_device(&h).await;
    let (io, served) = serve(&h);
    let (io, tap) = tap(io);
    let c = TunnelClient::start(device.connect_tunnel(io, &host_id).await.unwrap());
    ok(&h, &c, "host.hello", json!({})).await;

    tap.tamper_next();
    let tampered = tokio::time::timeout(WAIT, c.request("workspace.list", json!({}))).await.unwrap();
    assert!(matches!(tampered, Err(TunnelError::Closed)), "{tampered:?}");
    served.ended().await;
    // The host itself is unaffected.
    h.ok("host.hello", json!({})).await;
}

#[tokio::test]
async fn disconnects_are_clean_on_either_side() {
    let h = host(false).await;
    let (device, host_id) = paired_device(&h).await;

    // The device closes, with an event socket open and a request in flight.
    let (c, served) = connect(&h, &device, &host_id).await;
    let mut stream = events(&c).await;
    stream.send(&json!({"op": "host.subscribe"})).unwrap();
    frame(&h, &mut stream).await;
    let in_flight = c.request("git.status", json!({"workspace_id": h.workspace_id}));
    drop(in_flight);
    c.close().await;
    served.ended().await;
    assert_eq!(closed(&mut stream).await, (None, None));

    // The device vanishes without closing.
    let (c, served) = connect(&h, &device, &host_id).await;
    ok(&h, &c, "host.hello", json!({})).await;
    drop(c);
    served.ended().await;

    // The host shuts down.
    let (c, served) = connect(&h, &device, &host_id).await;
    let mut stream = events(&c).await;
    served.shutdown.trigger();
    served.ended().await;
    assert_eq!(closed(&mut stream).await, (None, None));
    assert!(matches!(c.request("host.hello", json!({})).await, Err(TunnelError::Closed)));

    // Nothing leaked into the host: it serves the next tunnel as before.
    let (c, _served) = connect(&h, &device, &host_id).await;
    ok(&h, &c, "host.hello", json!({})).await;
    h.ok("host.hello", json!({})).await;
}

#[tokio::test]
async fn nothing_readable_crosses_the_wire() {
    let h = host(false).await;
    let (device, host_id) = paired_device(&h).await;
    let marker = "zwiim-plaintext-marker-5c1e";
    std::fs::write(format!("{}/secret.txt", h.root), marker).unwrap();

    let (io, _served) = serve(&h);
    let (io, tap) = tap(io);
    let c = TunnelClient::start(device.connect_tunnel(io, &host_id).await.unwrap());
    let read = ok(&h, &c, "files.read", json!({"workspace_id": h.workspace_id, "path": "secret.txt"})).await;
    assert_eq!(read["content"], marker);
    ok(&h, &c, "workspace.list", json!({})).await;
    let mut stream = events(&c).await;
    stream.send(&json!({"op": "host.subscribe"})).unwrap();
    assert_eq!(frame(&h, &mut stream).await["type"], "host.snapshot");

    let device_key = URL_SAFE_NO_PAD.decode(device.static_public().unwrap()).unwrap();
    for needle in [
        marker.as_bytes(),
        b"files.read",
        b"workspace.list",
        b"host.subscribe",
        b"host.snapshot",
        b"secret.txt",
        b"\"t\":",
        h.workspace_id.as_bytes(),
        host_id.as_bytes(),
        device.device_id.as_bytes(),
        // IK encrypts the device's static key: an observer cannot tell
        // which device is connecting.
        &device_key,
    ] {
        assert!(!tap.seen(needle), "{} crossed the wire in the clear", String::from_utf8_lossy(needle));
    }
    assert!(tap.up.lock().unwrap().len() > 100 && tap.down.lock().unwrap().len() > marker.len());
}

#[tokio::test]
async fn the_test_listener_is_loopback_tcp_only() {
    let h = host(false).await;
    let (device, host_id) = paired_device(&h).await;
    let shutdown = Arc::new(ShutdownSignal::new());
    let router = || super::host::router(h.state.clone(), HOST_TOKEN.to_string(), shutdown.clone());

    let refused = super::host::serve_test_listener(
        "0.0.0.0:0".parse().unwrap(),
        h.state.clone(),
        router(),
        shutdown.clone(),
    )
    .await;
    assert_eq!(refused.unwrap_err().kind(), std::io::ErrorKind::InvalidInput);

    let addr = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let listener = tokio::spawn(super::host::serve_test_listener(
        addr,
        h.state.clone(),
        router(),
        shutdown.clone(),
    ));
    let mut attempts = 0;
    let stream = loop {
        match tokio::net::TcpStream::connect(addr).await {
            Ok(stream) => break stream,
            Err(_) if attempts < 50 => {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => panic!("{e}"),
        }
    };
    let c = TunnelClient::start(device.connect_tunnel(stream, &host_id).await.unwrap());
    assert_eq!(ok(&h, &c, "host.hello", json!({})).await["host_id"], host_id.as_str());
    c.close().await;
    shutdown.trigger();
    tokio::time::timeout(WAIT, listener).await.unwrap().unwrap().unwrap();
}
