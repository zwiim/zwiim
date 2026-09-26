//! The Cloud link end to end, over real sockets: a host enrolled with an
//! in-process Zwiim Cloud, and a remote device reaching it through the
//! relay — account token → hosts → ticket → relay → Noise → `/zwiim/v1`.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::task::JoinHandle;
use zwiim_cloud::auth::AuthConfig;
use zwiim_cloud::serve::{self, Running};
use zwiim_cloud::server::{self as cloud_server, Direction};

use super::device::CloudAccount;
use super::transport::{CloudError, Tls};
use super::Timing;
use crate::acp::connection::ConnectionCommand;
use crate::identity::device::DeviceIdentity;
use crate::identity::pairing::{KeyPairingGrant, KeyPairingInvitation};
use crate::protocol::tests::{delta, host, Host, HOST_TOKEN};
use crate::tunnel::client::TunnelClient;
use crate::tunnel::noise::TunnelError;
use crate::tunnel::tests::{
    call, direct, events, frame, frame_where, new_device, ok, paired_device, paired_device_at, WAIT,
};
use crate::web::shutdown::ShutdownSignal;

const SECRET: &[u8] = b"test-jwt-secret-0123456789abcdef";

/// An account token, as the identity provider would mint it.
fn jwt(user: &str) -> String {
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 600;
    let claims = json!({"sub": user, "aud": "authenticated", "exp": exp, "role": "authenticated"});
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(SECRET),
    )
    .unwrap()
}

fn cloud_timing() -> cloud_server::Timing {
    cloud_server::Timing {
        presence_timeout: Duration::from_secs(1),
        enrollment_ttl: Duration::from_secs(60),
        ticket_ttl: Duration::from_secs(30),
        dial_timeout: Duration::from_secs(2),
        hello_timeout: Duration::from_secs(2),
    }
}

fn host_timing() -> Timing {
    Timing {
        heartbeat: Duration::from_millis(200),
        presence_timeout: Duration::from_secs(1),
        backoff_min: Duration::from_millis(100),
        backoff_max: Duration::from_millis(400),
    }
}

/// Everything the relay carried, per direction.
#[derive(Clone, Default)]
struct Taps {
    up: Arc<Mutex<Vec<u8>>>,
    down: Arc<Mutex<Vec<u8>>>,
}

impl Taps {
    fn seen(&self, needle: &[u8]) -> bool {
        [&self.up, &self.down]
            .iter()
            .any(|dir| dir.lock().unwrap().windows(needle.len()).any(|w| w == needle))
    }
}

/// A Zwiim Cloud on loopback, restartable on the same address and data.
struct TestCloud {
    dir: tempfile::TempDir,
    addr: SocketAddr,
    url: String,
    /// Certificate and key PEM, when serving TLS.
    tls: Option<(String, String)>,
    taps: Taps,
    running: Option<Running>,
}

impl TestCloud {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        Self::start_on(listener, format!("http://{addr}"), None)
    }

    async fn start_tls(cert: &str, key: &str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let tls = Some((cert.to_string(), key.to_string()));
        Self::start_on(listener, format!("https://localhost:{port}"), tls)
    }

    fn start_on(listener: tokio::net::TcpListener, url: String, tls: Option<(String, String)>) -> Self {
        let mut cloud = Self {
            dir: tempfile::tempdir().unwrap(),
            addr: listener.local_addr().unwrap(),
            url,
            tls,
            taps: Taps::default(),
            running: None,
        };
        cloud.serve(listener);
        cloud
    }

    fn serve(&mut self, listener: tokio::net::TcpListener) {
        let taps = self.taps.clone();
        let config = cloud_server::Config {
            public_url: self.url.clone(),
            data_dir: self.dir.path().join("cloud"),
            auth: AuthConfig {
                hs256_secret: Some(SECRET.to_vec()),
                audience: "authenticated".into(),
                ..Default::default()
            },
            timing: cloud_timing(),
            limits: Default::default(),
            relay_tap: Some(Arc::new(move |d, bytes: &[u8]| {
                let dir = match d {
                    Direction::DeviceToHost => &taps.up,
                    Direction::HostToDevice => &taps.down,
                };
                dir.lock().unwrap().extend_from_slice(bytes);
            })),
        };
        let tls = self
            .tls
            .as_ref()
            .map(|(cert, key)| serve::tls_config(cert.as_bytes(), key.as_bytes()).unwrap());
        self.running = Some(serve::start(config, listener, tls).unwrap());
    }

    async fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            running.stop().await;
        }
    }

    async fn restart(&mut self) {
        self.stop().await;
        let listener = tokio::net::TcpListener::bind(self.addr).await.unwrap();
        self.serve(listener);
    }

    fn account(&self, user: &str) -> CloudAccount {
        CloudAccount::new(&self.url, &jwt(user), Tls::default()).unwrap()
    }
}

/// Start the host's Cloud connector the way `zwiim-host` does.
fn start_link(h: &Host, tls: Tls) -> Arc<ShutdownSignal> {
    let shutdown = Arc::new(ShutdownSignal::new());
    h.state.cloud.configure(tls, host_timing());
    let router = crate::tunnel::host::router(h.state.clone(), HOST_TOKEN.to_string(), shutdown.clone());
    super::start(h.state.clone(), router, shutdown.clone());
    shutdown
}

async fn wait_for(what: &str, mut check: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !check().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_connection(h: &Host, connection: &str) {
    wait_for(connection, async || h.ok("cloud.status", json!({})).await["connection"] == connection).await;
}

/// A fresh host, enrolled into `user`'s account and online.
async fn enrolled(cloud: &TestCloud, user: &str) -> (Host, Arc<ShutdownSignal>) {
    let h = host(false).await;
    let shutdown = start_link(&h, Tls::default());
    let token = cloud.account(user).enrollment_token().await.unwrap().enrollment_token;
    h.ok("cloud.enroll", json!({"cloud_url": cloud.url, "enrollment_token": token, "name": "Studio"}))
        .await;
    wait_connection(&h, "online").await;
    (h, shutdown)
}

/// A remote device's tunnel to `host_id`, through the Cloud relay.
async fn relayed(account: &CloudAccount, device: &DeviceIdentity, host_id: &str) -> TunnelClient {
    let io = account.connect(host_id).await.unwrap();
    TunnelClient::start(device.connect_tunnel(io, host_id).await.unwrap())
}

#[tokio::test]
async fn enrollment_registers_the_host_and_keeps_it_present() {
    let cloud = TestCloud::start().await;
    let h = host(false).await;
    let _shutdown = start_link(&h, Tls::default());
    let status = h.ok("cloud.status", json!({})).await;
    assert_eq!(status["enrolled"], false);
    assert_eq!(status["connection"], "not_enrolled");

    let alice = cloud.account("u-alice");
    let token = alice.enrollment_token().await.unwrap().enrollment_token;
    // Refused before anything is sent: an insecure Cloud URL, a bad token.
    for (body, code) in [
        (json!({"cloud_url": "http://cloud.example.com", "enrollment_token": token}), "invalid_input"),
        (json!({"cloud_url": "https://cloud.example.com/v1", "enrollment_token": token}), "invalid_input"),
        (json!({"cloud_url": cloud.url, "enrollment_token": ""}), "invalid_input"),
        (json!({"cloud_url": cloud.url, "enrollment_token": "not-a-token"}), "invalid_input"),
        (json!({"cloud_url": cloud.url, "enrollment_token": token, "extra": 1}), "invalid_request"),
    ] {
        let (status, body) = direct(&h, "cloud.enroll", body).await;
        assert_eq!((status, body["error"]["code"].as_str()), (400, Some(code)), "{body}");
    }

    let enrolled = h.ok("cloud.enroll", json!({"cloud_url": cloud.url, "enrollment_token": token})).await;
    assert_eq!(enrolled["enrolled"], true);
    assert_eq!(enrolled["cloud_url"], cloud.url.as_str());
    assert_eq!(enrolled["user_id"], "u-alice");
    assert!(!enrolled.to_string().contains(&token));
    // The enrolling account now owns the host.
    let hello = h.ok("host.hello", json!({})).await;
    assert_eq!(hello["owner"]["user_id"], "u-alice");
    wait_connection(&h, "online").await;

    // The account sees the host, its public key and presence…
    let hosts = alice.hosts().await.unwrap();
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].host_id, hello["host_id"].as_str().unwrap());
    assert_eq!(hosts[0].static_pubkey, h.state.host_keys.get().unwrap().static_pubkey());
    assert!(hosts[0].online);
    // …which heartbeats keep alive well past the Cloud's presence timeout.
    tokio::time::sleep(cloud_timing().presence_timeout * 3).await;
    assert!(alice.hosts().await.unwrap()[0].online);
    let status = h.ok("cloud.status", json!({})).await;
    assert_eq!(status["connection"], "online");
    assert!(status["connected_since"].is_string() && status["last_error"].is_null(), "{status}");

    // Another account neither sees nor can take the host.
    let bob = cloud.account("u-bob");
    assert!(bob.hosts().await.unwrap().is_empty());
    let host_id = hosts[0].host_id.clone();
    assert!(matches!(bob.ticket(&host_id).await, Err(CloudError::Refused { status: 404, .. })));
    let token = bob.enrollment_token().await.unwrap().enrollment_token;
    let (status, body) = direct(&h, "cloud.enroll", json!({"cloud_url": cloud.url, "enrollment_token": token})).await;
    assert_eq!((status, body["error"]["code"].as_str()), (409, Some("host_owned")), "{body}");
    assert_eq!(h.ok("cloud.status", json!({})).await["user_id"], "u-alice");

    // The owner may enroll again (a new token each time).
    let token = alice.enrollment_token().await.unwrap().enrollment_token;
    h.ok("cloud.enroll", json!({"cloud_url": cloud.url, "enrollment_token": token})).await;
    wait_connection(&h, "online").await;
    assert_eq!(alice.hosts().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_remote_device_uses_the_host_through_the_relay() {
    let cloud = TestCloud::start().await;
    let (h, _shutdown) = enrolled(&cloud, "u-alice").await;
    let (device, host_id) = paired_device(&h).await;
    let alice = cloud.account("u-alice");
    let hosts = alice.hosts().await.unwrap();
    assert_eq!((hosts[0].host_id.as_str(), hosts[0].online), (host_id.as_str(), true));

    let c = relayed(&alice, &device, &host_id).await;
    let wid = h.workspace_id.clone();
    let marker = "zwiim-relay-marker-9d2b";
    std::fs::write(format!("{}/notes.txt", h.root), marker).unwrap();

    // Operations are the host's own, byte for byte.
    for (op, body) in [
        ("host.hello", json!({})),
        ("workspace.list", json!({})),
        ("workspace.tree", json!({"workspace_id": wid})),
        ("files.read", json!({"workspace_id": wid, "path": "notes.txt"})),
        ("git.status", json!({"workspace_id": wid, "show_all_untracked": true})),
        ("files.read", json!({"workspace_id": wid, "path": "../../etc/passwd"})),
    ] {
        assert_eq!(call(&h, &c, op, body.clone()).await, direct(&h, op, body).await, "{op}");
    }
    let read = ok(&h, &c, "files.read", json!({"workspace_id": wid, "path": "notes.txt"})).await;
    assert_eq!(read["content"], marker);
    ok(
        &h,
        &c,
        "files.write",
        json!({"workspace_id": wid, "path": "notes.txt", "content": "changed\n", "etag": read["etag"]}),
    )
    .await;
    assert_eq!(std::fs::read_to_string(format!("{}/notes.txt", h.root)).unwrap(), "changed\n");

    // Sessions: send and history.
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
    assert_eq!((200, history), direct(&h, "session.history", json!({"session_id": sid})).await);

    // A terminal, with its output streamed back.
    let tid = ok(&h, &c, "terminal.spawn", json!({"workspace_id": wid, "shell": "/bin/sh"})).await["terminal_id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut stream = events(&c).await;
    stream.send(&json!({"op": "terminal.subscribe", "terminal_id": tid})).unwrap();
    stream.send(&json!({"op": "ping"})).unwrap();
    frame_where(&h, &mut stream, "pong", |f| f["type"] == "pong").await;
    ok(&h, &c, "terminal.write", json!({"terminal_id": tid, "data": "echo zw-$((40+2)); exit\n"})).await;
    let mut output = String::new();
    loop {
        let f = frame(&h, &mut stream).await;
        match f["event"]["type"].as_str().unwrap() {
            "terminal.output" => output.push_str(f["event"]["payload"]["data"].as_str().unwrap()),
            "terminal.exit" => break,
            other => panic!("unexpected {other}"),
        }
    }
    assert!(output.contains("zw-42"), "{output}");

    // The device is still only a device: no host authority, no legacy API.
    for (op, body) in [
        ("cloud.enroll", json!({"cloud_url": cloud.url, "enrollment_token": "x"})),
        ("cloud.status", json!({})),
        ("device.list", json!({})),
    ] {
        let (status, body) = call(&h, &c, op, body).await;
        assert_eq!((status, body["error"]["code"].as_str()), (403, Some("permission_denied")), "{op}");
    }
    for op in ["../../api/list_folders", "../api/trusted_device_list"] {
        let (status, body) = call(&h, &c, op, json!({})).await;
        assert_eq!((status, body["error"]["code"].as_str()), (404, Some("unknown_operation")), "{op}");
    }

    // The Cloud carried all of it and could read none of it.
    let device_key = base64::Engine::decode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        device.static_public().unwrap(),
    )
    .unwrap();
    for needle in [
        marker.as_bytes(),
        b"changed",
        b"notes.txt",
        b"files.read",
        b"files.write",
        b"session.send",
        b"session.history",
        b"terminal.spawn",
        b"terminal.subscribe",
        b"zw-42",
        b"workspace_id",
        b"\"t\":",
        wid.as_bytes(),
        sid.as_bytes(),
        tid.as_bytes(),
        host_id.as_bytes(),
        device.device_id.as_bytes(),
        h.root.as_bytes(),
        &device_key,
    ] {
        assert!(!cloud.taps.seen(needle), "{} crossed the relay in the clear", String::from_utf8_lossy(needle));
    }
    assert!(cloud.taps.up.lock().unwrap().len() > 500 && cloud.taps.down.lock().unwrap().len() > 500);
}

#[tokio::test]
async fn the_host_decides_who_gets_in() {
    let cloud = TestCloud::start().await;
    let (h, _shutdown) = enrolled(&cloud, "u-alice").await;
    let alice = cloud.account("u-alice");
    let invitation: KeyPairingInvitation =
        serde_json::from_value(h.ok("device.pair_invite", json!({})).await).unwrap();
    let host_id = invitation.host_id.clone();

    // A device that knows the host's key but never paired: the account can
    // reach the host, the host refuses the device.
    let (mut stranger, path) = new_device(&h);
    let grant = KeyPairingGrant { host_id: host_id.clone(), device_id: stranger.device_id.clone() };
    stranger.record_key_pairing(&path, &invitation, &grant).unwrap();
    let io = alice.connect(&host_id).await.unwrap();
    assert!(matches!(stranger.connect_tunnel(io, &host_id).await, Err(TunnelError::Refused)));

    // A paired device works until it is revoked; then its tunnel ends and
    // no new one opens.
    let (device, _, _) = paired_device_at(&h).await;
    let c = relayed(&alice, &device, &host_id).await;
    ok(&h, &c, "host.hello", json!({})).await;
    h.ok("device.revoke", json!({"device_id": device.device_id})).await;
    wait_for("the revoked tunnel to end", async || {
        matches!(c.request("host.hello", json!({})).await, Err(TunnelError::Closed))
    })
    .await;
    let io = alice.connect(&host_id).await.unwrap();
    assert!(matches!(device.connect_tunnel(io, &host_id).await, Err(TunnelError::Refused)));
    let devices = h.ok("device.list", json!({})).await["devices"].clone();
    assert!(devices.as_array().unwrap().iter().all(|d| d["device_id"] != stranger.device_id.as_str()));

    // A ticket is single-use: presenting it twice gets nowhere.
    let ticket = alice.ticket(&host_id).await.unwrap().ticket;
    let io = super::device::dial(alice.origin(), &ticket, &Tls::default()).await.unwrap();
    drop(io);
    assert!(matches!(
        super::device::dial(alice.origin(), &ticket, &Tls::default()).await,
        Err(CloudError::Closed(4401))
    ));
}

#[tokio::test]
async fn an_unreachable_host_is_reported_offline() {
    let cloud = TestCloud::start().await;
    let (h, shutdown) = enrolled(&cloud, "u-alice").await;
    let (device, host_id) = paired_device(&h).await;
    let alice = cloud.account("u-alice");

    // The host stops: the Cloud notices at once and says so.
    shutdown.trigger();
    wait_for("the host to go offline", async || !alice.hosts().await.unwrap()[0].online).await;
    assert!(alice.hosts().await.unwrap()[0].last_seen_at.is_some());
    assert!(matches!(alice.ticket(&host_id).await, Err(CloudError::HostOffline)));
    assert!(matches!(alice.connect(&host_id).await, Err(CloudError::HostOffline)));
    drop(device);
}

#[tokio::test]
async fn the_host_outlives_the_cloud_and_reconnects() {
    let mut cloud = TestCloud::start().await;
    let (h, _shutdown) = enrolled(&cloud, "u-alice").await;
    let (device, host_id) = paired_device(&h).await;
    let alice = cloud.account("u-alice");
    let c = relayed(&alice, &device, &host_id).await;
    ok(&h, &c, "host.hello", json!({})).await;

    // The Cloud goes away: remote tunnels end, the host goes offline…
    cloud.stop().await;
    wait_connection(&h, "offline").await;
    wait_for("the relayed tunnel to end", async || {
        matches!(c.request("host.hello", json!({})).await, Err(TunnelError::Closed))
    })
    .await;
    let status = h.ok("cloud.status", json!({})).await;
    assert!(status["last_error"].is_string() && status["enrolled"] == true, "{status}");

    // …and keeps working locally.
    std::fs::write(format!("{}/local.txt", h.root), "still here").unwrap();
    let read = h
        .ok("files.read", json!({"workspace_id": h.workspace_id, "path": "local.txt"}))
        .await;
    assert_eq!(read["content"], "still here");
    let (sid, mut rx) = h.live_session().await;
    h.ok(
        "session.send",
        json!({"session_id": sid, "blocks": [{"type": "text", "text": "offline"}], "client_message_id": "m1"}),
    )
    .await;
    assert!(rx.recv().await.is_some());

    // The Cloud comes back (same registry): the host reconnects by itself
    // and remote devices get in again.
    cloud.restart().await;
    wait_connection(&h, "online").await;
    assert!(alice.hosts().await.unwrap()[0].online);
    let c = relayed(&alice, &device, &host_id).await;
    let read = ok(&h, &c, "files.read", json!({"workspace_id": h.workspace_id, "path": "local.txt"})).await;
    assert_eq!(read["content"], "still here");
}

/// A TCP proxy in front of the Cloud whose connections can all be cut.
struct Proxy {
    url: String,
    conns: Arc<Mutex<Vec<JoinHandle<()>>>>,
    task: JoinHandle<()>,
}

impl Proxy {
    async fn start(target: SocketAddr) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let conns: Arc<Mutex<Vec<JoinHandle<()>>>> = Default::default();
        let tracked = conns.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut inbound, _)) = listener.accept().await {
                let conn = tokio::spawn(async move {
                    if let Ok(mut outbound) = tokio::net::TcpStream::connect(target).await {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                    }
                });
                tracked.lock().unwrap().push(conn);
            }
        });
        Self { url, conns, task }
    }

    /// Drop every open connection, as a network failure would.
    fn cut(&self) {
        for conn in self.conns.lock().unwrap().drain(..) {
            conn.abort();
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
        self.cut();
    }
}

#[tokio::test]
async fn a_cut_relay_resumes_from_the_devices_cursors_without_duplicates() {
    let cloud = TestCloud::start().await;
    let (h, _shutdown) = enrolled(&cloud, "u-alice").await;
    let (device, host_id) = paired_device(&h).await;
    let proxy = Proxy::start(cloud.addr).await;
    let alice = CloudAccount::new(&proxy.url, &jwt("u-alice"), Tls::default()).unwrap();
    let (sid, mut rx) = h.live_session().await;

    let c = relayed(&alice, &device, &host_id).await;
    let mut stream = events(&c).await;
    stream
        .send(&json!({"op": "session.subscribe", "subscription_id": "a", "session_id": sid}))
        .unwrap();
    let snap = frame(&h, &mut stream).await;
    assert_eq!(snap["type"], "snapshot", "{snap}");
    let epoch = snap["epoch"].as_str().unwrap().to_string();
    h.emit(delta("one")).await;
    let one = frame(&h, &mut stream).await;
    assert_eq!(one["event"]["payload"]["text"], "one");
    let cursor = one["event"]["seq"].as_u64().unwrap();

    // The network drops mid-mutation. The outcome is unknown to the device,
    // and nothing retries it.
    let send = json!({"session_id": sid, "blocks": [{"type": "text", "text": "once"}], "client_message_id": "m1"});
    let (sent, ()) = tokio::join!(c.request("session.send", send), async { proxy.cut() });
    assert!(matches!(sent, Ok((200, _)) | Err(TunnelError::Closed)), "{sent:?}");
    wait_for("the tunnel to end", async || {
        matches!(c.request("host.hello", json!({})).await, Err(TunnelError::Closed))
    })
    .await;
    // Events keep happening on the host meanwhile; it stayed online.
    h.emit(delta("two")).await;
    h.emit(delta("three")).await;
    assert_eq!(h.ok("cloud.status", json!({})).await["connection"], "online");

    // A new relayed tunnel resumes from the cursor: every missed event,
    // once, in order.
    let c = relayed(&alice, &device, &host_id).await;
    let mut stream = events(&c).await;
    stream
        .send(&json!({"op": "session.subscribe", "subscription_id": "b", "session_id": sid, "since_seq": cursor, "epoch": epoch}))
        .unwrap();
    let replay = frame(&h, &mut stream).await;
    assert_eq!(replay["type"], "replay", "{replay}");
    assert_eq!(replay["epoch"], epoch.as_str());
    let events: Vec<_> = replay["events"].as_array().unwrap().clone();
    let texts: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "agent.message_delta")
        .map(|e| e["payload"]["text"].clone())
        .collect();
    assert_eq!(texts, [json!("two"), json!("three")]);
    let seqs: Vec<u64> = events.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert!(seqs.first().is_some_and(|s| *s > cursor) && seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");

    // Live again: the next event arrives once.
    h.emit(delta("four")).await;
    let four = frame(&h, &mut stream).await;
    assert_eq!((four["type"].as_str(), &four["event"]["payload"]["text"]), (Some("event"), &json!("four")));
    stream.send(&json!({"op": "ping"})).unwrap();
    assert_eq!(frame(&h, &mut stream).await["type"], "pong");

    // The interrupted send reached the agent at most once.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut prompts = 0;
    while let Ok(cmd) = rx.try_recv() {
        prompts += matches!(cmd, ConnectionCommand::Prompt { .. }) as usize;
    }
    assert!(prompts <= 1, "{prompts} prompts");
    if sent.is_ok() {
        assert_eq!(prompts, 1);
    }
}

/// A private CA and a `localhost` certificate it signed.
fn test_pki() -> (String, String, String) {
    use rcgen::{BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose};
    let ca_key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.distinguished_name.push(DnType::CommonName, "Zwiim Test CA");
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let ca_cert = ca.self_signed(&ca_key).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let mut leaf = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    leaf.distinguished_name.push(DnType::CommonName, "localhost");
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf_cert = leaf.signed_by(&leaf_key, &ca_cert, &ca_key).unwrap();
    (ca_cert.pem(), leaf_cert.pem(), leaf_key.serialize_pem())
}

#[tokio::test]
async fn the_cloud_is_reached_over_tls_only_when_trusted() {
    let (ca, cert, key) = test_pki();
    let cloud = TestCloud::start_tls(&cert, &key).await;
    let trusted = Tls::with_ca_pem(ca.as_bytes()).unwrap();
    let alice = CloudAccount::new(&cloud.url, &jwt("u-alice"), trusted.clone()).unwrap();
    let token = alice.enrollment_token().await.unwrap().enrollment_token;

    // A host that does not trust the Cloud's certificate does not talk to it.
    let h = host(false).await;
    let _shutdown = start_link(&h, Tls::default());
    let (status, body) = direct(&h, "cloud.enroll", json!({"cloud_url": cloud.url, "enrollment_token": token})).await;
    assert_eq!((status, body["error"]["code"].as_str()), (502, Some("cloud_unavailable")), "{body}");
    assert_eq!(h.ok("cloud.status", json!({})).await["enrolled"], false);

    // Trusted: https enrollment, a wss control connection and relay.
    h.state.cloud.configure(trusted, host_timing());
    h.ok("cloud.enroll", json!({"cloud_url": cloud.url, "enrollment_token": token})).await;
    wait_connection(&h, "online").await;
    let (device, host_id) = paired_device(&h).await;
    let c = relayed(&alice, &device, &host_id).await;
    assert_eq!(ok(&h, &c, "host.hello", json!({})).await["host_id"], host_id.as_str());
}
