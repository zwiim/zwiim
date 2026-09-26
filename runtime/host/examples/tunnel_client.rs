//! Minimal device-side test client for the device tunnel.
//!
//! Pairs a device by key (first run only), opens the Noise tunnel to a host —
//! directly on its loopback test listener, or through Zwiim Cloud's relay —
//! and exercises real `/zwiim/v1` operations and the event socket through it,
//! including a reconnect that resumes from cursors.
//!
//! ```text
//! ZWIIM_HOST_URL=http://127.0.0.1:38180  # pairing only: device.pair_invite + device.pair
//! ZWIIM_TOKEN=…                          # pairing only: stands in for scanning the QR
//! ZWIIM_TUNNEL_ADDR=127.0.0.1:38190      # the host's ZWIIM_TUNNEL_TEST_ADDR
//! ZWIIM_DEVICE_FILE=/tmp/device.json     # this device's identity (created if absent)
//! ZWIIM_SMOKE_AGENT=claude_code          # optional: run a real agent session
//!
//! # Relay mode, instead of ZWIIM_TUNNEL_ADDR:
//! ZWIIM_CLOUD_URL=https://cloud.example  # the Cloud the host is enrolled with
//! ZWIIM_CLOUD_JWT=…                      # the account's access token (Supabase)
//! ZWIIM_CLOUD_CA_CERT=/tmp/ca.pem        # optional: trust a private CA
//!
//! cargo run -p zwiim-host --example tunnel_client
//! ```

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use zwiim_host::cloud::device::CloudAccount;
use zwiim_host::cloud::transport::Tls;
use zwiim_host::identity::device::DeviceIdentity;
use zwiim_host::identity::pairing::{KeyPairingGrant, KeyPairingInvitation};
use zwiim_host::tunnel::client::{EventStream, StreamEvent, TunnelClient};

type Error = Box<dyn std::error::Error>;

fn env(name: &str) -> Result<String, Error> {
    std::env::var(name).map_err(|_| format!("{name} is not set").into())
}

#[derive(Default)]
struct Report {
    passed: usize,
    failed: usize,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: impl std::fmt::Display) {
        if ok {
            self.passed += 1;
            println!("PASS  {name}");
        } else {
            self.failed += 1;
            println!("FAIL  {name}: {detail}");
        }
    }
}

/// Host paths never reach a device.
fn leaks_a_path(text: &str) -> bool {
    ["/Users/", "/private/", "/tmp/", "/home/", "/var/"]
        .iter()
        .any(|p| text.contains(p))
}

/// Pair with the host by key, as scanning its QR would.
async fn pair(device: &mut DeviceIdentity, path: &PathBuf) -> Result<String, Error> {
    let url = env("ZWIIM_HOST_URL")?;
    let token = env("ZWIIM_TOKEN")?;
    let http = reqwest::Client::new();
    let invite: KeyPairingInvitation = http
        .post(format!("{url}/zwiim/v1/device.pair_invite"))
        .bearer_auth(token)
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let grant: KeyPairingGrant = http
        .post(format!("{url}/zwiim/v1/device.pair"))
        .json(&json!({
            "code": invite.code,
            "device_id": device.device_id,
            "device_name": device.device_name,
            "device_pubkey": device.static_public()?,
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    device.record_key_pairing(path, &invite, &grant)?;
    println!("paired as device {} with host {}", device.device_id, invite.host_id);
    Ok(invite.host_id)
}

/// The account, when the tunnel goes through Zwiim Cloud.
fn cloud() -> Result<Option<CloudAccount>, Error> {
    let Ok(url) = std::env::var("ZWIIM_CLOUD_URL") else { return Ok(None) };
    let tls = Tls::from_env()?;
    Ok(Some(CloudAccount::new(&url, &env("ZWIIM_CLOUD_JWT")?, tls)?))
}

async fn connect(device: &DeviceIdentity, host_id: &str) -> Result<TunnelClient, Error> {
    if let Some(account) = cloud()? {
        let io = account.connect(host_id).await?;
        return Ok(TunnelClient::start(device.connect_tunnel(io, host_id).await?));
    }
    let stream = tokio::net::TcpStream::connect(env("ZWIIM_TUNNEL_ADDR")?).await?;
    Ok(TunnelClient::start(device.connect_tunnel(stream, host_id).await?))
}

struct Session<'a> {
    client: &'a TunnelClient,
    report: &'a mut Report,
}

impl Session<'_> {
    async fn call(&mut self, op: &str, body: Value) -> Result<(u16, Value), Error> {
        let (status, body) = self.client.request(op, body).await?;
        let text = body.to_string();
        if leaks_a_path(&text) {
            self.report.check(&format!("{op} leaks no host path"), false, &text);
        }
        Ok((status, body))
    }

    async fn ok(&mut self, op: &str, body: Value) -> Result<Value, Error> {
        let (status, body) = self.call(op, body).await?;
        self.report.check(op, status == 200, format!("{status} {body}"));
        Ok(body)
    }
}

async fn next_frame(stream: &mut EventStream, report: &mut Report, secs: u64) -> Result<Value, Error> {
    match tokio::time::timeout(Duration::from_secs(secs), stream.recv()).await {
        Ok(Some(StreamEvent::Frame(text))) => {
            if leaks_a_path(&text) {
                report.check("event frame leaks no host path", false, &text);
            }
            Ok(serde_json::from_str(&text)?)
        }
        Ok(Some(StreamEvent::Closed { code, reason })) => {
            Err(format!("event socket closed: {code:?} {reason:?}").into())
        }
        Ok(None) => Err("event socket ended".into()),
        Err(_) => Err("no event frame in time".into()),
    }
}

/// Frames until the agent's turn completes; returns the text and last seq.
async fn run_turn(stream: &mut EventStream, report: &mut Report) -> Result<(String, u64, String), Error> {
    let (mut text, mut seq) = (String::new(), 0);
    loop {
        let frame = next_frame(stream, report, 120).await?;
        if frame["type"] != "event" {
            continue;
        }
        let event = &frame["event"];
        seq = event["seq"].as_u64().unwrap_or(seq);
        match event["type"].as_str() {
            Some("agent.message_delta") => text.push_str(event["payload"]["text"].as_str().unwrap_or("")),
            Some("agent.turn_complete") => {
                let stop = event["payload"]["stop_reason"].as_str().unwrap_or("").to_string();
                return Ok((text, seq, stop));
            }
            Some("agent.error") => return Err(format!("agent error: {event}").into()),
            _ => {}
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let path = PathBuf::from(env("ZWIIM_DEVICE_FILE")?);
    let mut device = DeviceIdentity::load_or_create(&path, "Tunnel test client")?;
    let host_id = match device.paired_hosts.first() {
        Some(host) => host.host_id.clone(),
        None => pair(&mut device, &path).await?,
    };
    let mut report = Report::default();

    if let Some(account) = cloud()? {
        let hosts = account.hosts().await?;
        let found = hosts.iter().find(|h| h.host_id == host_id);
        report.check(
            "cloud lists the host online",
            found.is_some_and(|h| h.online),
            format!("{hosts:?}"),
        );
    }
    let client = connect(&device, &host_id).await?;
    report.check("noise handshake", true, "");
    let mut s = Session { client: &client, report: &mut report };

    let hello = s.ok("host.hello", json!({})).await?;
    s.report.check("host.hello host_id", hello["host_id"] == host_id.as_str(), &hello);
    let workspaces = s.ok("workspace.list", json!({})).await?;
    let wid = workspaces[0]["workspace_id"].as_str().ok_or("no workspace")?.to_string();
    s.ok("workspace.open", json!({"workspace_id": wid})).await?;
    let tree = s.ok("workspace.tree", json!({"workspace_id": wid})).await?;
    let file = tree
        .as_array()
        .and_then(|t| t.iter().find(|e| e["kind"] == "file"))
        .and_then(|e| e["path"].as_str())
        .map(str::to_string);
    if let Some(file) = file {
        let read = s.ok("files.read", json!({"workspace_id": wid, "path": file})).await?;
        s.report.check("files.read content", read["content"].is_string(), &read);
    }
    s.ok("git.status", json!({"workspace_id": wid})).await?;
    let log = s.ok("git.log", json!({"workspace_id": wid, "limit": 3})).await?;
    s.report.check("git.log entries", log["entries"].is_array(), &log);
    let (status, denied) = s.call("device.list", json!({})).await?;
    s.report.check("device.list needs local host authority", status == 403, format!("{status} {denied}"));
    let (status, unknown) = s.call("../../api/list_folders", json!({})).await?;
    s.report.check("legacy /api unreachable", status == 404, format!("{status} {unknown}"));

    // Host feed + terminal events.
    let mut feed = client.open_events().await?.map_err(|(s, b)| format!("{s} {b}"))?;
    feed.send(&json!({"op": "host.subscribe"}))?;
    let snapshot = next_frame(&mut feed, s.report, 10).await?;
    s.report.check("host.snapshot", snapshot["type"] == "host.snapshot", &snapshot);
    let tid = s
        .ok("terminal.spawn", json!({"workspace_id": wid, "shell": "/bin/sh"}))
        .await?["terminal_id"]
        .as_str()
        .ok_or("no terminal_id")?
        .to_string();
    loop {
        let f = next_frame(&mut feed, s.report, 10).await?;
        if f["event"]["type"] == "terminal.changed" && f["event"]["payload"]["terminal"]["terminal_id"] == tid.as_str() {
            s.report.check("host.event terminal.changed", true, "");
            break;
        }
    }
    feed.send(&json!({"op": "terminal.subscribe", "terminal_id": tid}))?;
    feed.send(&json!({"op": "ping"}))?;
    while next_frame(&mut feed, s.report, 10).await?["type"] != "pong" {}
    s.ok("terminal.write", json!({"terminal_id": tid, "data": "echo zw-$((40+2)); exit\n"})).await?;
    let mut output = String::new();
    loop {
        let f = next_frame(&mut feed, s.report, 10).await?;
        match f["event"]["type"].as_str() {
            Some("terminal.output") if f["type"] == "terminal" => {
                output.push_str(f["event"]["payload"]["data"].as_str().unwrap_or(""))
            }
            Some("terminal.exit") => break,
            _ => {}
        }
    }
    s.report.check("terminal output through the tunnel", output.contains("zw-42"), &output);
    feed.close();

    // A real agent session, then a reconnect that resumes from its cursors.
    if let Ok(agent) = std::env::var("ZWIIM_SMOKE_AGENT") {
        let start = s
            .ok(
                "session.start",
                json!({"workspace_id": wid, "agent": agent, "blocks": [{"type": "text", "text": "Reply with exactly the single word: pong"}]}),
            )
            .await?;
        let sid = start["session_id"].as_str().ok_or("no session_id")?.to_string();
        let mut events = client.open_events().await?.map_err(|(s, b)| format!("{s} {b}"))?;
        events.send(&json!({"op": "session.subscribe", "subscription_id": "a", "session_id": sid}))?;
        let snap = next_frame(&mut events, s.report, 10).await?;
        s.report.check("session snapshot", snap["type"] == "snapshot", &snap);
        let epoch = snap["epoch"].as_str().unwrap_or("").to_string();
        let (text, _, stop) = run_turn(&mut events, s.report).await?;
        s.report.check("session.start turn", text.to_lowercase().contains("pong"), format!("{stop} {text:?}"));

        s.ok("session.send", json!({"session_id": sid, "blocks": [{"type": "text", "text": "Reply with exactly the single word: again"}]}))
            .await?;
        let (text, seq, stop) = run_turn(&mut events, s.report).await?;
        s.report.check("session.send turn", text.to_lowercase().contains("again"), format!("{stop} {text:?}"));
        let history = s.ok("session.history", json!({"session_id": sid})).await?;
        s.report.check(
            "session.history",
            history["turns"].as_array().is_some_and(|t| t.len() >= 2),
            history["turns"].as_array().map_or(0, |t| t.len()),
        );

        // Drop the whole tunnel, then resume from the cursors on a new one.
        drop(events);
        client.close().await;
        let client = connect(&device, &host_id).await?;
        let mut events = client.open_events().await?.map_err(|(s, b)| format!("{s} {b}"))?;
        let since = seq.saturating_sub(3);
        events.send(&json!({"op": "session.subscribe", "subscription_id": "b", "session_id": sid, "since_seq": since, "epoch": epoch}))?;
        let replay = next_frame(&mut events, &mut report, 10).await?;
        let replayed = replay["events"].as_array().map_or(0, |e| e.len());
        report.check(
            "reconnect resumes with a replay",
            replay["type"] == "replay" && replay["epoch"] == epoch.as_str() && replayed >= 1,
            format!("{} {} events", replay["type"], replayed),
        );
        events.send(&json!({"op": "session.subscribe", "subscription_id": "c", "session_id": sid, "since_seq": since, "epoch": "0000000000000000"}))?;
        let stale = next_frame(&mut events, &mut report, 10).await?;
        report.check("stale epoch falls back to a snapshot", stale["type"] == "snapshot", &stale["type"]);
        events.close();
        client.close().await;
    } else {
        // Without an agent: still prove a reconnect on a fresh tunnel.
        client.close().await;
        let client = connect(&device, &host_id).await?;
        let (status, _) = client.request("host.hello", json!({})).await?;
        report.check("reconnect", status == 200, status);
        client.close().await;
    }

    println!("{} passed, {} failed", report.passed, report.failed);
    if report.failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
