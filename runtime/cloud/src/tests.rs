//! The Cloud end to end over real sockets: enrollment, presence, tickets,
//! rendezvous, relay, restart and TLS. Hosts and devices here are minimal
//! stand-ins speaking the wire contract in `proto`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::{Signer, SigningKey};
use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::auth::tests::{jwt, SECRET};
use crate::auth::AuthConfig;
use crate::proto::*;
use crate::serve::{self, Running};
use crate::server::{Config, Direction, Limits, Timing};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
type Taps = Arc<Mutex<Vec<(Direction, Vec<u8>)>>>;

const WAIT: Duration = Duration::from_secs(5);

fn fast() -> Timing {
    Timing {
        presence_timeout: Duration::from_secs(5),
        enrollment_ttl: Duration::from_secs(60),
        ticket_ttl: Duration::from_secs(30),
        dial_timeout: Duration::from_millis(500),
        hello_timeout: Duration::from_secs(2),
    }
}

struct TestCloud {
    dir: tempfile::TempDir,
    running: Option<Running>,
    url: String,
    taps: Taps,
    http: reqwest::Client,
}

fn config(url: &str, dir: &std::path::Path, timing: Timing, taps: &Taps) -> Config {
    config_limited(url, dir, timing, Limits::default(), taps)
}

fn config_limited(url: &str, dir: &std::path::Path, timing: Timing, limits: Limits, taps: &Taps) -> Config {
    let taps = taps.clone();
    Config {
        public_url: url.to_string(),
        data_dir: dir.join("cloud"),
        auth: AuthConfig {
            hs256_secret: Some(SECRET.to_vec()),
            audience: "authenticated".into(),
            ..Default::default()
        },
        timing,
        limits,
        relay_tap: Some(Arc::new(move |d, bytes: &[u8]| taps.lock().unwrap().push((d, bytes.to_vec())))),
    }
}

impl TestCloud {
    async fn start(timing: Timing) -> Self {
        Self::start_limited(timing, Limits::default()).await
    }

    async fn start_limited(timing: Timing, limits: Limits) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let taps = Taps::default();
        let running = serve::start(config_limited(&url, dir.path(), timing, limits, &taps), listener, None).unwrap();
        Self { dir, running: Some(running), url, taps, http: reqwest::Client::new() }
    }

    async fn stop(&mut self) {
        self.running.take().unwrap().stop().await;
    }

    /// Start again on the same address and data directory.
    async fn restart(&mut self, timing: Timing) {
        if let Some(running) = self.running.take() {
            running.stop().await;
        }
        let addr = self.url.strip_prefix("http://").unwrap();
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let running = serve::start(config(&self.url, self.dir.path(), timing, &self.taps), listener, None).unwrap();
        self.running = Some(running);
    }

    async fn post(&self, path: &str, bearer: Option<&str>, body: Value) -> (u16, Value) {
        let mut req = self.http.post(format!("{}{path}", self.url)).json(&body);
        if let Some(b) = bearer {
            req = req.bearer_auth(b);
        }
        let res = req.send().await.unwrap();
        (res.status().as_u16(), res.json().await.unwrap_or(Value::Null))
    }

    async fn get(&self, path: &str, bearer: Option<&str>) -> (u16, Value) {
        let mut req = self.http.get(format!("{}{path}", self.url));
        if let Some(b) = bearer {
            req = req.bearer_auth(b);
        }
        let res = req.send().await.unwrap();
        (res.status().as_u16(), res.json().await.unwrap_or(Value::Null))
    }

    async fn enrollment_token(&self, user: &str) -> String {
        let (status, body) = self.post(ENROLLMENTS_PATH, Some(&jwt(user)), json!({})).await;
        assert_eq!(status, 200, "{body}");
        body["enrollment_token"].as_str().unwrap().to_string()
    }

    async fn ws(&self, path: &str) -> Ws {
        tokio_tungstenite::connect_async(ws_url(&self.url, path)).await.unwrap().0
    }

    async fn hosts(&self, user: &str) -> Vec<HostInfo> {
        let (status, body) = self.get(HOSTS_PATH, Some(&jwt(user))).await;
        assert_eq!(status, 200, "{body}");
        serde_json::from_value(body).unwrap()
    }

    async fn ticket(&self, user: &str, host_id: &str) -> (u16, Value) {
        self.post(&tickets_path(host_id), Some(&jwt(user)), json!({})).await
    }
}

/// A host stand-in: an id and an Ed25519 key.
struct FakeHost {
    host_id: String,
    key: SigningKey,
    static_pubkey: String,
}

fn random32() -> [u8; 32] {
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b
}

impl FakeHost {
    fn new() -> Self {
        Self {
            host_id: uuid(),
            key: SigningKey::from_bytes(&random32()),
            static_pubkey: URL_SAFE_NO_PAD.encode(random32()),
        }
    }

    fn sign_pubkey(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.key.verifying_key().as_bytes())
    }

    fn enroll_request(&self, cloud: &TestCloud, token: &str) -> Value {
        let msg = enroll_message(&cloud.url, &self.host_id, &self.sign_pubkey(), &self.static_pubkey, token);
        json!({
            "enrollment_token": token,
            "host_id": self.host_id,
            "name": "Studio Mac",
            "sign_pubkey": self.sign_pubkey(),
            "static_pubkey": self.static_pubkey,
            "signature": URL_SAFE_NO_PAD.encode(self.key.sign(&msg).to_bytes()),
        })
    }

    async fn enroll(&self, cloud: &TestCloud, user: &str) {
        let token = cloud.enrollment_token(user).await;
        let (status, body) = cloud.post(ENROLL_PATH, None, self.enroll_request(cloud, &token)).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body, json!({"host_id": self.host_id, "user_id": user}));
    }

    /// Open and authenticate a control connection.
    async fn control(&self, cloud: &TestCloud) -> Ws {
        let mut ws = cloud.ws(CONTROL_PATH).await;
        let ControlFromCloud::Challenge { nonce } = recv_json(&mut ws).await else { panic!("no challenge") };
        let sig = self.key.sign(&auth_message(&cloud.url, &self.host_id, &nonce));
        send_json(
            &mut ws,
            &ControlFromHost::Auth { host_id: self.host_id.clone(), signature: URL_SAFE_NO_PAD.encode(sig.to_bytes()) },
        )
        .await;
        assert_eq!(recv_json::<ControlFromCloud>(&mut ws).await, ControlFromCloud::Ready);
        ws
    }

    /// Answer the next dial on `control` with a relay socket.
    async fn answer(&self, cloud: &TestCloud, control: &mut Ws) -> Ws {
        let ControlFromCloud::Dial { rendezvous, token } = recv_json(control).await else { panic!("no dial") };
        let mut ws = cloud.ws(RELAY_HOST_PATH).await;
        send_json(&mut ws, &RelayHello::Host { rendezvous, token }).await;
        ws
    }
}

fn uuid() -> String {
    let b = random32();
    URL_SAFE_NO_PAD.encode(&b[..16])
}

async fn send_json<T: serde::Serialize>(ws: &mut Ws, msg: &T) {
    ws.send(Message::Text(serde_json::to_string(msg).unwrap().into())).await.unwrap();
}

async fn recv_json<T: serde::de::DeserializeOwned>(ws: &mut Ws) -> T {
    loop {
        match tokio::time::timeout(WAIT, ws.next()).await.expect("a message within 5s") {
            Some(Ok(Message::Text(t))) => return serde_json::from_str(&t).unwrap(),
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            other => panic!("expected a text message, got {other:?}"),
        }
    }
}

/// The close code the peer ends with.
async fn closed(ws: &mut Ws) -> Option<u16> {
    loop {
        match tokio::time::timeout(WAIT, ws.next()).await.expect("a close within 5s") {
            Some(Ok(Message::Close(frame))) => return frame.map(|f| u16::from(f.code)),
            Some(Ok(_)) => continue,
            Some(Err(_)) | None => return None,
        }
    }
}

async fn ready(ws: &mut Ws) {
    assert_eq!(recv_json::<RelayReady>(ws).await, RelayReady::Ready);
}

async fn binary(ws: &mut Ws) -> Vec<u8> {
    loop {
        match tokio::time::timeout(WAIT, ws.next()).await.expect("a message within 5s") {
            Some(Ok(Message::Binary(b))) => return b.to_vec(),
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            other => panic!("expected a binary message, got {other:?}"),
        }
    }
}

/// A device relay socket that has presented a ticket.
async fn device(cloud: &TestCloud, ticket: &str) -> Ws {
    let mut ws = cloud.ws(RELAY_DEVICE_PATH).await;
    send_json(&mut ws, &RelayHello::Device { ticket: ticket.into() }).await;
    ws
}

async fn wait_until(what: &str, mut cond: impl AsyncFnMut() -> bool) {
    for _ in 0..100 {
        if cond().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

// ─── enrollment ──────────────────────────────────────────────────────────

#[tokio::test]
async fn enrollment_needs_an_account_a_fresh_token_and_the_hosts_key() {
    let cloud = TestCloud::start(fast()).await;
    let host = FakeHost::new();

    // Minting an enrollment token needs a valid account token.
    assert_eq!(cloud.post(ENROLLMENTS_PATH, None, json!({})).await.0, 401);
    assert_eq!(cloud.post(ENROLLMENTS_PATH, Some("garbage"), json!({})).await.0, 401);

    host.enroll(&cloud, "alice").await;
    let hosts = cloud.hosts("alice").await;
    assert_eq!(hosts.len(), 1);
    assert_eq!(
        (hosts[0].host_id.as_str(), hosts[0].name.as_deref(), hosts[0].online, &hosts[0].static_pubkey),
        (host.host_id.as_str(), Some("Studio Mac"), false, &host.static_pubkey)
    );
    assert!(cloud.hosts("bob").await.is_empty());

    // Tokens are single use.
    let token = cloud.enrollment_token("alice").await;
    let req = host.enroll_request(&cloud, &token);
    assert_eq!(cloud.post(ENROLL_PATH, None, req.clone()).await.0, 200);
    let (status, body) = cloud.post(ENROLL_PATH, None, req).await;
    assert_eq!((status, &body["error"]["code"]), (401, &json!("unauthenticated")));

    // The signature must be the host's, over this Cloud's origin and token.
    let token = cloud.enrollment_token("alice").await;
    let mut forged = host.enroll_request(&cloud, &token);
    forged["signature"] = json!(URL_SAFE_NO_PAD.encode(SigningKey::from_bytes(&random32()).sign(b"x").to_bytes()));
    assert_eq!(cloud.post(ENROLL_PATH, None, forged).await.0, 401);
    let token = cloud.enrollment_token("alice").await;
    let msg = enroll_message("https://other.example", &host.host_id, &host.sign_pubkey(), &host.static_pubkey, &token);
    let mut elsewhere = host.enroll_request(&cloud, &token);
    elsewhere["signature"] = json!(URL_SAFE_NO_PAD.encode(host.key.sign(&msg).to_bytes()));
    assert_eq!(cloud.post(ENROLL_PATH, None, elsewhere).await.0, 401);

    // A host id stays bound to its first key.
    let squatter = FakeHost { host_id: host.host_id.clone(), ..FakeHost::new() };
    let token = cloud.enrollment_token("mallory").await;
    let (status, body) = cloud.post(ENROLL_PATH, None, squatter.enroll_request(&cloud, &token)).await;
    assert_eq!((status, &body["error"]["code"]), (409, &json!("already_exists")));
    assert!(cloud.hosts("mallory").await.is_empty());

    // A host that already belongs to alice refuses bob's token.
    let token = cloud.enrollment_token("bob").await;
    let mut req = host.enroll_request(&cloud, &token);
    req["expected_user_id"] = json!("alice");
    let (status, body) = cloud.post(ENROLL_PATH, None, req).await;
    assert_eq!((status, &body["error"]["code"]), (409, &json!("host_owned")));
    assert_eq!(cloud.hosts("alice").await.len(), 1);

    // Malformed requests.
    let token = cloud.enrollment_token("alice").await;
    let mut bad = host.enroll_request(&cloud, &token);
    bad["host_id"] = json!("../etc");
    assert_eq!(cloud.post(ENROLL_PATH, None, bad).await.0, 400);
    assert_eq!(cloud.post(ENROLL_PATH, None, json!({"nope": 1})).await.0, 400);
}

#[tokio::test]
async fn enrollment_tokens_expire() {
    let cloud = TestCloud::start(Timing { enrollment_ttl: Duration::from_millis(200), ..fast() }).await;
    let host = FakeHost::new();
    let token = cloud.enrollment_token("alice").await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(cloud.post(ENROLL_PATH, None, host.enroll_request(&cloud, &token)).await.0, 401);
}

// ─── presence ────────────────────────────────────────────────────────────

#[tokio::test]
async fn hosts_authenticate_their_control_connection_with_their_key() {
    let cloud = TestCloud::start(fast()).await;
    let host = FakeHost::new();
    host.enroll(&cloud, "alice").await;

    // Not enrolled, and enrolled but signing with another key.
    for impostor in [FakeHost::new(), FakeHost { host_id: host.host_id.clone(), ..FakeHost::new() }] {
        let mut ws = cloud.ws(CONTROL_PATH).await;
        let ControlFromCloud::Challenge { nonce } = recv_json(&mut ws).await else { panic!() };
        let sig = impostor.key.sign(&auth_message(&cloud.url, &impostor.host_id, &nonce));
        let auth = ControlFromHost::Auth {
            host_id: impostor.host_id.clone(),
            signature: URL_SAFE_NO_PAD.encode(sig.to_bytes()),
        };
        send_json(&mut ws, &auth).await;
        assert_eq!(closed(&mut ws).await, Some(CLOSE_UNAUTHENTICATED));
    }
    // A signature over another nonce (a replay) is refused.
    let mut ws = cloud.ws(CONTROL_PATH).await;
    let _: ControlFromCloud = recv_json(&mut ws).await;
    let sig = host.key.sign(&auth_message(&cloud.url, &host.host_id, "old-nonce"));
    let auth = ControlFromHost::Auth { host_id: host.host_id.clone(), signature: URL_SAFE_NO_PAD.encode(sig.to_bytes()) };
    send_json(&mut ws, &auth).await;
    assert_eq!(closed(&mut ws).await, Some(CLOSE_UNAUTHENTICATED));
    // Garbage instead of an answer.
    let mut ws = cloud.ws(CONTROL_PATH).await;
    let _: ControlFromCloud = recv_json(&mut ws).await;
    ws.send(Message::Text("{\"type\":\"heartbeat\"}".into())).await.unwrap();
    assert_eq!(closed(&mut ws).await, Some(CLOSE_PROTOCOL));
    assert!(!cloud.hosts("alice").await[0].online);

    let mut control = host.control(&cloud).await;
    let info = &cloud.hosts("alice").await[0];
    assert!(info.online && info.last_seen_at.is_some());
    send_json(&mut control, &ControlFromHost::Heartbeat).await;
    assert_eq!(recv_json::<ControlFromCloud>(&mut control).await, ControlFromCloud::HeartbeatAck);

    // The host closing takes it offline.
    control.close(None).await.unwrap();
    wait_until("offline", async || !cloud.hosts("alice").await[0].online).await;
}

#[tokio::test]
async fn a_newer_control_connection_replaces_the_old_one() {
    let cloud = TestCloud::start(fast()).await;
    let host = FakeHost::new();
    host.enroll(&cloud, "alice").await;
    let mut first = host.control(&cloud).await;
    let mut second = host.control(&cloud).await;
    assert_eq!(closed(&mut first).await, Some(CLOSE_REPLACED));
    // The replaced connection going away leaves the host online.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(cloud.hosts("alice").await[0].online);
    send_json(&mut second, &ControlFromHost::Heartbeat).await;
    assert_eq!(recv_json::<ControlFromCloud>(&mut second).await, ControlFromCloud::HeartbeatAck);
}

#[tokio::test]
async fn a_silent_host_goes_offline() {
    let cloud = TestCloud::start(Timing { presence_timeout: Duration::from_millis(400), ..fast() }).await;
    let host = FakeHost::new();
    host.enroll(&cloud, "alice").await;
    let mut control = host.control(&cloud).await;

    // Heartbeats keep it online past the timeout…
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        send_json(&mut control, &ControlFromHost::Heartbeat).await;
        assert_eq!(recv_json::<ControlFromCloud>(&mut control).await, ControlFromCloud::HeartbeatAck);
    }
    assert!(cloud.hosts("alice").await[0].online);
    // …silence ends it.
    assert_eq!(closed(&mut control).await, Some(1001));
    assert!(!cloud.hosts("alice").await[0].online);
    assert_eq!(cloud.ticket("alice", &host.host_id).await.1["error"]["code"], "host_offline");
}

// ─── tickets and relay ───────────────────────────────────────────────────

#[tokio::test]
async fn tickets_are_for_the_accounts_online_hosts() {
    let cloud = TestCloud::start(fast()).await;
    let host = FakeHost::new();
    host.enroll(&cloud, "alice").await;

    let (status, body) = cloud.ticket("alice", &host.host_id).await;
    assert_eq!((status, &body["error"]["code"]), (503, &json!("host_offline")));
    let _control = host.control(&cloud).await;
    for (user, id) in [("bob", host.host_id.as_str()), ("alice", "no-such-host")] {
        let (status, body) = cloud.ticket(user, id).await;
        assert_eq!((status, &body["error"]["code"]), (404, &json!("not_found")), "{user} {id}");
    }
    assert_eq!(cloud.post(&tickets_path(&host.host_id), None, json!({})).await.0, 401);
    let (status, body) = cloud.ticket("alice", &host.host_id).await;
    assert_eq!(status, 200, "{body}");
    assert!(body["ticket"].as_str().unwrap().len() >= 43 && body["expires_at"].is_u64());
}

#[tokio::test]
async fn the_relay_joins_device_and_host_and_moves_opaque_bytes() {
    let cloud = TestCloud::start(fast()).await;
    let host = FakeHost::new();
    host.enroll(&cloud, "alice").await;
    let mut control = host.control(&cloud).await;

    let ticket = cloud.ticket("alice", &host.host_id).await.1["ticket"].as_str().unwrap().to_string();
    let mut dev = device(&cloud, &ticket).await;
    let mut relayed = host.answer(&cloud, &mut control).await;
    ready(&mut relayed).await;
    ready(&mut dev).await;

    let up: Vec<u8> = (0..=255u8).cycle().take(70_000).collect();
    dev.send(Message::Binary(up.clone().into())).await.unwrap();
    assert_eq!(binary(&mut relayed).await, up);
    relayed.send(Message::Binary(vec![9, 8, 7].into())).await.unwrap();
    assert_eq!(binary(&mut dev).await, [9, 8, 7]);
    // Heartbeats carry on beside the relay.
    send_json(&mut control, &ControlFromHost::Heartbeat).await;
    assert_eq!(recv_json::<ControlFromCloud>(&mut control).await, ControlFromCloud::HeartbeatAck);

    let taps = cloud.taps.lock().unwrap().clone();
    assert_eq!(taps, [(Direction::DeviceToHost, up), (Direction::HostToDevice, vec![9, 8, 7])]);

    // Either side closing closes the other.
    dev.close(None).await.unwrap();
    assert_eq!(closed(&mut relayed).await, Some(1000));

    // The ticket was single use.
    let mut again = device(&cloud, &ticket).await;
    assert_eq!(closed(&mut again).await, Some(CLOSE_UNAUTHENTICATED));
}

#[tokio::test]
async fn relay_failures_are_explicit() {
    let cloud = TestCloud::start(Timing { ticket_ttl: Duration::from_millis(300), ..fast() }).await;
    let host = FakeHost::new();
    host.enroll(&cloud, "alice").await;
    let mut control = host.control(&cloud).await;
    let ticket = async || cloud.ticket("alice", &host.host_id).await.1["ticket"].as_str().unwrap().to_string();

    // Unknown and expired tickets.
    let mut ws = device(&cloud, "not-a-ticket").await;
    assert_eq!(closed(&mut ws).await, Some(CLOSE_UNAUTHENTICATED));
    let stale = ticket().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut ws = device(&cloud, &stale).await;
    assert_eq!(closed(&mut ws).await, Some(CLOSE_UNAUTHENTICATED));

    // The host never dials back: host_offline, and the dial is dead.
    let mut ws = device(&cloud, &ticket().await).await;
    let ControlFromCloud::Dial { rendezvous, token } = recv_json(&mut control).await else { panic!() };
    assert_eq!(closed(&mut ws).await, Some(CLOSE_HOST_OFFLINE));
    let mut late = cloud.ws(RELAY_HOST_PATH).await;
    send_json(&mut late, &RelayHello::Host { rendezvous, token }).await;
    assert_eq!(closed(&mut late).await, Some(CLOSE_UNAUTHENTICATED));

    // A host socket with the wrong token is refused, and burns the dial.
    let mut ws = device(&cloud, &ticket().await).await;
    let ControlFromCloud::Dial { rendezvous, token } = recv_json(&mut control).await else { panic!() };
    let mut wrong = cloud.ws(RELAY_HOST_PATH).await;
    send_json(&mut wrong, &RelayHello::Host { rendezvous: rendezvous.clone(), token: "guess".into() }).await;
    assert_eq!(closed(&mut wrong).await, Some(CLOSE_UNAUTHENTICATED));
    let mut right = cloud.ws(RELAY_HOST_PATH).await;
    send_json(&mut right, &RelayHello::Host { rendezvous, token }).await;
    assert_eq!(closed(&mut right).await, Some(CLOSE_UNAUTHENTICATED));
    assert_eq!(closed(&mut ws).await, Some(CLOSE_HOST_OFFLINE));

    // A host that is not connected.
    let t = ticket().await;
    control.close(None).await.unwrap();
    wait_until("offline", async || !cloud.hosts("alice").await[0].online).await;
    let mut ws = device(&cloud, &t).await;
    assert_eq!(closed(&mut ws).await, Some(CLOSE_HOST_OFFLINE));

    // Hello messages must be the right shape.
    let mut ws = cloud.ws(RELAY_DEVICE_PATH).await;
    ws.send(Message::Binary(vec![1, 2, 3].into())).await.unwrap();
    assert_eq!(closed(&mut ws).await, Some(CLOSE_PROTOCOL));
    let mut ws = cloud.ws(RELAY_HOST_PATH).await;
    send_json(&mut ws, &RelayHello::Device { ticket: "x".into() }).await;
    assert_eq!(closed(&mut ws).await, Some(CLOSE_PROTOCOL));
}

#[tokio::test]
async fn relayed_messages_are_binary_and_bounded() {
    let cloud = TestCloud::start(fast()).await;
    let host = FakeHost::new();
    host.enroll(&cloud, "alice").await;
    let mut control = host.control(&cloud).await;

    let open = async |control: &mut Ws| {
        let ticket = cloud.ticket("alice", &host.host_id).await.1["ticket"].as_str().unwrap().to_string();
        let mut dev = device(&cloud, &ticket).await;
        let mut relayed = host.answer(&cloud, control).await;
        ready(&mut relayed).await;
        ready(&mut dev).await;
        (dev, relayed)
    };

    // Text after the handshake is a protocol violation.
    let (mut dev, mut relayed) = open(&mut control).await;
    dev.send(Message::Text("{\"op\":\"files.read\"}".into())).await.unwrap();
    assert_eq!(closed(&mut relayed).await, Some(CLOSE_PROTOCOL));

    // Oversized messages end the relay.
    let (mut dev, mut relayed) = open(&mut control).await;
    let _ = dev.send(Message::Binary(vec![0u8; MAX_RELAY_MESSAGE + 1].into())).await;
    assert_eq!(closed(&mut relayed).await, Some(1000));
    assert!(cloud.taps.lock().unwrap().iter().all(|(_, b)| b.len() <= MAX_RELAY_MESSAGE));
}

// ─── restart ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_restart_keeps_the_registry_and_drops_everything_live() {
    let mut cloud = TestCloud::start(fast()).await;
    let host = FakeHost::new();
    host.enroll(&cloud, "alice").await;
    let mut control = host.control(&cloud).await;
    let ticket = cloud.ticket("alice", &host.host_id).await.1["ticket"].as_str().unwrap().to_string();
    let spare = cloud.ticket("alice", &host.host_id).await.1["ticket"].as_str().unwrap().to_string();
    let mut dev = device(&cloud, &ticket).await;
    let mut relayed = host.answer(&cloud, &mut control).await;
    ready(&mut relayed).await;
    ready(&mut dev).await;

    cloud.stop().await;
    assert_eq!(closed(&mut control).await, Some(1012));
    assert!(matches!(closed(&mut dev).await, None | Some(1000)));
    assert!(tokio_tungstenite::connect_async(ws_url(&cloud.url, CONTROL_PATH)).await.is_err());

    cloud.restart(fast()).await;
    // Enrollment survived; presence and tickets did not.
    let hosts = cloud.hosts("alice").await;
    assert_eq!((hosts.len(), hosts[0].online), (1, false));
    let _control = host.control(&cloud).await;
    assert!(cloud.hosts("alice").await[0].online);
    let mut ws = device(&cloud, &spare).await;
    assert_eq!(closed(&mut ws).await, Some(CLOSE_UNAUTHENTICATED));
}

// ─── surface ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn nothing_but_the_cloud_api_is_served() {
    let cloud = TestCloud::start(fast()).await;
    for path in ["/api/list_folders", "/zwiim/v1/host.hello", "/v1/hosts/x/files", "/"] {
        assert_eq!(cloud.post(path, Some(&jwt("alice")), json!({})).await.0, 404, "{path}");
        assert_eq!(cloud.get(path, Some(&jwt("alice"))).await.0, 404, "{path}");
    }
    let res = cloud.http.get(format!("{}/healthz", cloud.url)).send().await.unwrap();
    assert_eq!(res.text().await.unwrap(), "ok");
    // Bodies are bounded.
    let big = json!({"enrollment_token": "x".repeat(32 * 1024)});
    assert_eq!(cloud.post(ENROLL_PATH, None, big).await.0, 413);
}

#[tokio::test]
async fn plain_http_is_for_loopback_only() {
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let cfg = config("https://cloud.example", dir.path(), fast(), &Taps::default());
    assert!(serve::start(cfg, listener, None).is_err());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cfg = config("http://cloud.example", dir.path(), fast(), &Taps::default());
    assert!(serve::start(cfg, listener, None).is_err(), "a public http URL is refused");
}

/// A CA and a `localhost` server certificate it issued.
pub(crate) fn test_pki() -> (String, String, String) {
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
    leaf.not_before = rcgen::date_time_ymd(2026, 1, 1);
    leaf.not_after = rcgen::date_time_ymd(2027, 1, 1);
    let leaf_cert = leaf.signed_by(&leaf_key, &ca_cert, &ca_key).unwrap();
    (ca_cert.pem(), leaf_cert.pem(), leaf_key.serialize_pem())
}

#[tokio::test]
async fn tls_serves_https_and_wss() {
    let (ca, cert, key) = test_pki();
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("https://localhost:{port}");
    let tls = serve::tls_config(cert.as_bytes(), key.as_bytes()).unwrap();
    let running = serve::start(config(&url, dir.path(), fast(), &Taps::default()), listener, Some(tls)).unwrap();

    let http = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .build()
        .unwrap();
    assert_eq!(http.get(format!("{url}/healthz")).send().await.unwrap().text().await.unwrap(), "ok");
    // Untrusted without the CA.
    assert!(reqwest::get(format!("{url}/healthz")).await.is_err());

    let ca = native_tls::Certificate::from_pem(ca.as_bytes()).unwrap();
    let connector = native_tls::TlsConnector::builder().add_root_certificate(ca).build().unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async_tls_with_config(
        ws_url(&url, CONTROL_PATH),
        None,
        false,
        Some(tokio_tungstenite::Connector::NativeTls(connector)),
    )
    .await
    .unwrap();
    let ControlFromCloud::Challenge { nonce } = recv_json(&mut ws).await else { panic!() };
    assert!(!nonce.is_empty());
    running.stop().await;
}

// ─── limits ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn relays_beyond_the_limit_are_refused_until_one_ends() {
    let cloud = TestCloud::start_limited(fast(), Limits { max_relays: 1, ..Limits::default() }).await;
    let host = FakeHost::new();
    host.enroll(&cloud, "alice").await;
    let mut control = host.control(&cloud).await;

    let ticket = |body: Value| body["ticket"].as_str().unwrap().to_string();
    let mut first = device(&cloud, &ticket(cloud.ticket("alice", &host.host_id).await.1)).await;
    let mut first_host = host.answer(&cloud, &mut control).await;
    ready(&mut first).await;
    ready(&mut first_host).await;

    // A second relay while the first is open: "try again later".
    let mut second = device(&cloud, &ticket(cloud.ticket("alice", &host.host_id).await.1)).await;
    assert_eq!(closed(&mut second).await, Some(1013));

    // The first ends; its slot is free again.
    first.close(None).await.unwrap();
    assert!(closed(&mut first_host).await.is_some());
    let mut third = None;
    for _ in 0..50 {
        let mut ws = device(&cloud, &ticket(cloud.ticket("alice", &host.host_id).await.1)).await;
        match tokio::time::timeout(Duration::from_millis(300), recv_json::<ControlFromCloud>(&mut control)).await {
            Ok(ControlFromCloud::Dial { rendezvous, token }) => {
                let mut host_ws = cloud.ws(RELAY_HOST_PATH).await;
                send_json(&mut host_ws, &RelayHello::Host { rendezvous, token }).await;
                ready(&mut ws).await;
                third = Some((ws, host_ws));
                break;
            }
            _ => assert_eq!(closed(&mut ws).await, Some(1013)),
        }
    }
    assert!(third.is_some(), "a relay after the first ended");
}

#[tokio::test]
async fn connections_beyond_the_limit_are_closed_on_arrival() {
    let cloud = TestCloud::start_limited(fast(), Limits { max_connections: 2, ..Limits::default() }).await;
    let health = || async {
        reqwest::Client::new().get(format!("{}/healthz", cloud.url)).send().await.map(|r| r.status().as_u16())
    };
    // Two idle sockets hold both slots.
    let mut a = cloud.ws(CONTROL_PATH).await;
    let _b = cloud.ws(CONTROL_PATH).await;
    assert!(health().await.is_err());
    // One goes away: new connections are served again.
    a.close(None).await.unwrap();
    drop(a);
    wait_until("a free slot", async || health().await.is_ok_and(|s| s == 200)).await;
}

#[tokio::test]
async fn an_account_over_its_request_rate_gets_a_429_and_others_do_not() {
    let cloud = TestCloud::start_limited(fast(), Limits { max_requests_per_account: 2, ..Limits::default() }).await;
    assert_eq!(cloud.get(HOSTS_PATH, Some(&jwt("alice"))).await.0, 200);
    assert_eq!(cloud.get(HOSTS_PATH, Some(&jwt("alice"))).await.0, 200);
    let (status, body) = cloud.get(HOSTS_PATH, Some(&jwt("alice"))).await;
    assert_eq!(status, 429);
    assert_eq!(body["error"]["code"], "rate_limited");
    assert_eq!(cloud.get(HOSTS_PATH, Some(&jwt("bob"))).await.0, 200);
}
