//! Supported providers, live: a real Host with its managed adapters talks to
//! the real vendor. Each test checks, over the Zwiim protocol only, that the
//! agent advertises its models, starts a session, streams a reply, changes
//! effort, cancels a turn, reports errors, resumes after a reconnect and after
//! a Host restart.
//!
//! Ignored by default. Run with dedicated CI-only credentials, never personal
//! ones and never committed:
//!
//! ```text
//! ZWIIM_LIVE_PROVIDERS=claude,codex \
//! ANTHROPIC_API_KEY=… OPENAI_API_KEY=… \
//! [ZWIIM_LIVE_EXPECTED_MODELS_CLAUDE=a,b] [ZWIIM_LIVE_EXPECTED_MODELS_CODEX=c,d] \
//! cargo test -p zwiim-host --test provider_live -- --ignored --nocapture --test-threads=1
//! ```
//!
//! The Host runs in a temporary home with the vendors' config directories
//! inside it, so a developer machine's own logins are never used.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use zwiim_host::local_host::LocalHostInfo;

const TURN_LIMIT: Duration = Duration::from_secs(240);

struct Provider {
    name: &'static str,
    agent: &'static str,
    credential: &'static str,
}

const CLAUDE: Provider = Provider {
    name: "claude",
    agent: "claude_code",
    credential: "ANTHROPIC_API_KEY",
};
const CODEX: Provider = Provider {
    name: "codex",
    agent: "codex",
    credential: "OPENAI_API_KEY",
};

/// Whether this run covers `p`. Listed without its credential is a failure,
/// not a skip: a release must not pass on a provider nobody tested.
fn selected(p: &Provider) -> bool {
    let listed = std::env::var("ZWIIM_LIVE_PROVIDERS")
        .unwrap_or_default()
        .split(',')
        .any(|n| n.trim() == p.name);
    if listed {
        assert!(
            std::env::var(p.credential).is_ok_and(|v| !v.trim().is_empty()),
            "{} is listed in ZWIIM_LIVE_PROVIDERS but {} is not set",
            p.name,
            p.credential
        );
    } else {
        eprintln!("{}: not in ZWIIM_LIVE_PROVIDERS, skipped", p.name);
    }
    listed
}

struct Sandbox {
    _dir: tempfile::TempDir,
    home: PathBuf,
    data: PathBuf,
    repo: PathBuf,
    port: u16,
}

fn sandbox() -> Sandbox {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let data = dir.path().join("data");
    let repo = home.join("project");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "# Live provider check\n").unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["add", "."],
        &[
            "-c",
            "user.name=Zwiim CI",
            "-c",
            "user.email=ci@zwiim.invalid",
            "commit",
            "-qm",
            "init",
        ],
    ] {
        let ok = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(ok.success(), "git {args:?}");
    }
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    Sandbox {
        _dir: dir,
        home,
        data,
        repo,
        port,
    }
}

fn start_host(s: &Sandbox) -> Child {
    Command::new(env!("CARGO_BIN_EXE_zwiim-host"))
        .env("HOME", &s.home)
        .env("CLAUDE_CONFIG_DIR", s.home.join(".claude"))
        .env("CODEX_HOME", s.home.join(".codex"))
        .env("ZWIIM_DATA_DIR", &s.data)
        .env("ZWIIM_PORT", s.port.to_string())
        .env("ZWIIM_HOST", "127.0.0.1")
        .env("ZWIIM_HOST_KEY_STORE", "file")
        .env("ZWIIM_WORKTREES_DIR", s.home.join("worktrees"))
        .env_remove("ZWIIM_ADAPTER_SOURCE")
        .env_remove("ZWIIM_ADAPTER_SETUP")
        .env_remove("ZWIIM_HOME")
        .env_remove("ZWIIM_TOKEN")
        .env_remove("ZWIIM_SUPERVISED")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap()
}

struct Host {
    info: LocalHostInfo,
    http: reqwest::Client,
}

impl Host {
    async fn wait(data: &Path, child: &mut Child, since: Option<std::time::SystemTime>) -> Host {
        let http = reqwest::Client::new();
        let deadline = Instant::now() + Duration::from_secs(90);
        let path = zwiim_host::local_host::path(data);
        loop {
            assert!(Instant::now() < deadline, "host never became ready");
            if let Some(status) = child.try_wait().unwrap() {
                panic!("host exited: {status}");
            }
            let fresh = since.is_none_or(|t| {
                std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .is_ok_and(|m| m > t)
            });
            if fresh {
                if let Some(info) = std::fs::read(&path)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<LocalHostInfo>(&b).ok())
                {
                    let host = Host {
                        info,
                        http: http.clone(),
                    };
                    if host.call("host.hello", json!({})).await.is_ok() {
                        return host;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    async fn call(&self, op: &str, body: Value) -> Result<Value, Value> {
        let res = self
            .http
            .post(format!("{}/zwiim/v1/{op}", self.info.url))
            .bearer_auth(&self.info.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| json!({"error": {"code": "network", "message": e.to_string()}}))?;
        let ok = res.status().is_success();
        let body: Value = res.json().await.unwrap_or(Value::Null);
        if ok {
            Ok(body)
        } else {
            Err(body)
        }
    }

    async fn ok(&self, op: &str, body: Value) -> Value {
        match self.call(op, body.clone()).await {
            Ok(v) => v,
            Err(e) => panic!("{op} {body} failed: {e}"),
        }
    }

    async fn events(&self) -> Events {
        let url = format!("{}/zwiim/v1/ws", self.info.url.replacen("http", "ws", 1));
        let mut req = url.into_client_request().unwrap();
        req.headers_mut().insert(
            "authorization",
            format!("Bearer {}", self.info.token).parse().unwrap(),
        );
        let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
        Events { ws, last_seq: 0 }
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Events {
    ws: Socket,
    last_seq: u64,
}

/// What one turn produced on the event channel.
#[derive(Default, Debug)]
struct Turn {
    text: String,
    deltas: usize,
    stop_reason: Option<String>,
    errors: Vec<Value>,
}

impl Events {
    async fn subscribe(&mut self, session_id: &str, since: Option<(u64, &str)>) -> Value {
        let mut msg =
            json!({"op": "session.subscribe", "subscription_id": "live", "session_id": session_id});
        if let Some((seq, epoch)) = since {
            msg["since_seq"] = json!(seq);
            msg["epoch"] = json!(epoch);
        }
        self.ws.send(Message::text(msg.to_string())).await.unwrap();
        // The first frame is the snapshot or the replay.
        loop {
            let frame = self.next(Duration::from_secs(30)).await;
            match frame["type"].as_str() {
                Some("snapshot") => {
                    self.last_seq = frame["event_seq"].as_u64().unwrap_or(0);
                    return frame;
                }
                Some("replay") => {
                    self.last_seq = frame["high_water_seq"].as_u64().unwrap_or(0);
                    return frame;
                }
                Some("error") => panic!("subscribe failed: {frame}"),
                _ => {}
            }
        }
    }

    async fn next(&mut self, limit: Duration) -> Value {
        let msg = tokio::time::timeout(limit, self.ws.next())
            .await
            .expect("no frame in time")
            .expect("event channel closed")
            .unwrap();
        match msg {
            Message::Text(t) => serde_json::from_str(&t).unwrap(),
            _ => Value::Null,
        }
    }

    /// Read events until the turn completes. `on_delta` runs once, at the
    /// first streamed text.
    async fn turn<F: std::future::Future<Output = ()>>(&mut self, on_delta: Option<F>) -> Turn {
        let mut turn = Turn::default();
        let mut on_delta = on_delta.map(Box::pin);
        let deadline = Instant::now() + TURN_LIMIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "turn did not complete: {turn:?}");
            let frame = self.next(left).await;
            if frame["type"] == "detached" {
                panic!("detached mid-turn: {frame}");
            }
            if frame["type"] != "event" {
                continue;
            }
            let event = &frame["event"];
            self.last_seq = event["seq"].as_u64().unwrap_or(self.last_seq);
            match event["type"].as_str() {
                Some("agent.message_delta") => {
                    turn.deltas += 1;
                    turn.text += event["payload"]["text"].as_str().unwrap_or_default();
                    if let Some(f) = on_delta.take() {
                        f.await;
                    }
                }
                Some("agent.error") => turn.errors.push(event["payload"].clone()),
                Some("agent.turn_complete") => {
                    turn.stop_reason = event["payload"]["stop_reason"].as_str().map(String::from);
                    return turn;
                }
                _ => {}
            }
        }
    }
}

fn text(t: &str) -> Value {
    json!([{"type": "text", "text": t}])
}

async fn wait_available(host: &Host, p: &Provider) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20 * 60);
    loop {
        let agents = host.ok("agent.list", json!({})).await;
        let a = agents
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["agent"] == p.agent)
            .unwrap_or_else(|| panic!("{} is not listed: {agents}", p.agent))
            .clone();
        assert_eq!(a["support"], "supported", "{a}");
        if a["available"] == true {
            return a;
        }
        let reason = a["reason"].as_str().unwrap_or_default();
        assert!(
            matches!(reason, "installing" | "needs_network" | "not_installed"),
            "{} cannot run: {a}",
            p.name
        );
        if reason != "installing" {
            let _ = host.call("agent.setup", json!({"agent": p.agent})).await;
        }
        assert!(
            Instant::now() < deadline,
            "{} never became available: {a}",
            p.name
        );
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn run(p: &Provider) {
    let s = sandbox();
    let mut child = start_host(&s);
    let host = Host::wait(&s.data, &mut child, None).await;
    wait_available(&host, p).await;
    let workspace = host
        .ok("workspace.add", json!({"path": s.repo.to_string_lossy()}))
        .await;
    let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();

    // Session start and streaming.
    let mut events = host.events().await;
    let started = host
        .ok(
            "session.start",
            json!({"workspace_id": workspace_id, "agent": p.agent,
                   "blocks": text("Reply with exactly one word: pong. Do not use tools.")}),
        )
        .await;
    let session_id = started["session_id"].as_str().unwrap().to_string();
    let snapshot = events.subscribe(&session_id, None).await;
    let epoch = snapshot["epoch"].as_str().unwrap_or_default().to_string();
    let first = events.turn(None::<std::future::Ready<()>>).await;
    eprintln!("{}: first turn {first:?}", p.name);
    assert!(first.errors.is_empty(), "{first:?}");
    assert!(first.deltas >= 1, "no streamed text: {first:?}");
    assert!(first.text.to_lowercase().contains("pong"), "{first:?}");

    // Advertised models, expected against actual.
    let options = host
        .ok("session.options", json!({"session_id": session_id}))
        .await["options"]
        .clone();
    let models: Vec<String> = options["model"]["values"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|m| m["value"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    eprintln!("{}: advertised models {models:?}", p.name);
    assert!(!models.is_empty(), "no models advertised: {options}");
    let expected_var = format!("ZWIIM_LIVE_EXPECTED_MODELS_{}", p.name.to_uppercase());
    if let Ok(expected) = std::env::var(&expected_var) {
        let missing: Vec<&str> = expected
            .split(',')
            .map(str::trim)
            .filter(|m| !m.is_empty() && !models.iter().any(|x| x == m))
            .collect();
        assert!(
            missing.is_empty(),
            "{expected_var}: not advertised {missing:?}; actual {models:?}"
        );
    }

    // Effort, where the model offers it.
    if let Some(values) = options["effort"]["values"].as_array() {
        let current = options["effort"]["current"].as_str().unwrap_or_default();
        if let Some(other) = values
            .iter()
            .filter_map(|v| v["value"].as_str())
            .find(|v| *v != current)
        {
            let set = host
                .ok(
                    "session.set_options",
                    json!({"session_id": session_id, "effort": other}),
                )
                .await;
            assert_eq!(set["options"]["effort"]["current"], other, "{set}");
        }
    }

    // A refused option is a Zwiim error, and changes nothing.
    let refused = host
        .call(
            "session.set_options",
            json!({"session_id": session_id, "model": "zwiim-no-such-model"}),
        )
        .await
        .expect_err("an unknown model was accepted");
    assert_eq!(refused["error"]["code"], "option_unavailable", "{refused}");

    // Cancel a turn once it is streaming.
    host.ok(
        "session.send",
        json!({"session_id": session_id,
               "blocks": text("Count from 1 to 400, one number per line. Do not use tools.")}),
    )
    .await;
    let cancel = async {
        host.ok("session.cancel", json!({"session_id": session_id}))
            .await;
    };
    let cancelled = events.turn(Some(cancel)).await;
    eprintln!(
        "{}: cancelled turn stop_reason={:?}",
        p.name, cancelled.stop_reason
    );
    assert_eq!(
        cancelled.stop_reason.as_deref(),
        Some("cancelled"),
        "{cancelled:?}"
    );

    // Reconnect: a new event channel resumes from the last seen event.
    let seen = events.last_seq;
    drop(events);
    let mut events = host.events().await;
    let resumed = events.subscribe(&session_id, Some((seen, &epoch))).await;
    assert!(
        matches!(resumed["type"].as_str(), Some("replay" | "snapshot")),
        "{resumed}"
    );
    host.ok(
        "session.send",
        json!({"session_id": session_id, "blocks": text("Reply with exactly one word: again.")}),
    )
    .await;
    let again = events.turn(None::<std::future::Ready<()>>).await;
    assert!(again.text.to_lowercase().contains("again"), "{again:?}");
    drop(events);

    // Host restart: the session and its history survive, and it continues.
    let written = std::fs::metadata(zwiim_host::local_host::path(&s.data))
        .and_then(|m| m.modified())
        .ok();
    let restart = host.ok("host.restart", json!({"mode": "when_idle"})).await;
    assert_eq!(restart["state"], "restarting", "{restart}");
    let host = Host::wait(&s.data, &mut child, written).await;
    let history = host
        .ok("session.history", json!({"session_id": session_id}))
        .await;
    let turns = history["turns"].as_array().map(Vec::len).unwrap_or(0);
    assert!(turns >= 3, "history lost across the restart: {history}");
    wait_available(&host, p).await;
    let mut events = host.events().await;
    events.subscribe(&session_id, None).await;
    host.ok(
        "session.send",
        json!({"session_id": session_id, "blocks": text("Reply with exactly one word: back.")}),
    )
    .await;
    let back = events.turn(None::<std::future::Ready<()>>).await;
    assert!(back.errors.is_empty(), "{back:?}");
    assert!(back.text.to_lowercase().contains("back"), "{back:?}");

    // SAFETY: signalling our own child process.
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let _ = child.wait();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live provider: needs CI-only credentials (see module docs)"]
async fn claude_live() {
    if selected(&CLAUDE) {
        run(&CLAUDE).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live provider: needs CI-only credentials (see module docs)"]
async fn codex_live() {
    if selected(&CODEX) {
        run(&CODEX).await;
    }
}
