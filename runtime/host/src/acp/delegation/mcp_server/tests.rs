//! The companion's MCP server over in-memory stdio, against a fake broker on
//! a real Unix socket. Raw JSON-RPC lines, so the wire is what's tested.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines};
use tokio::net::UnixListener;

use super::serve;
use crate::acp::delegation::companion::{
    dispatch_line, CompanionContext, CompanionFeatures, InflightCalls, LineAction,
};
use crate::acp::delegation::transport::{read_frame, write_frame};

pub(super) const LEGACY: [&str; 4] = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
pub(super) const MODERN: &str = "2026-07-28";
pub(super) const WAIT: Duration = Duration::from_secs(5);

/// The Host's side of the companion socket. Records every message; answers
/// at once except `ask` and `call`, which stay open until the companion
/// hangs up, and `feedback`, which waits `feedback_delay` first.
#[derive(Clone)]
struct Broker {
    seen: Arc<Mutex<Vec<Value>>>,
    feedback_delay: Arc<Mutex<Duration>>,
}

impl Broker {
    fn start(path: &std::path::Path) -> Self {
        let listener = UnixListener::bind(path).unwrap();
        let broker = Self {
            seen: Arc::default(),
            feedback_delay: Arc::default(),
        };
        let shared = broker.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let broker = shared.clone();
                tokio::spawn(async move {
                    let Ok(message) = read_frame::<_, Value>(&mut stream).await else {
                        return;
                    };
                    let kind = message["kind"].as_str().unwrap_or("").to_string();
                    broker.seen.lock().unwrap().push(message);
                    let outcome = match kind.as_str() {
                        "ask" | "call" => {
                            let _ = stream.read(&mut [0u8; 1]).await;
                            broker.record(&format!("{kind} closed"));
                            return;
                        }
                        "update_progress" => json!({ "recorded": true }),
                        "feedback" => {
                            let delay = *broker.feedback_delay.lock().unwrap();
                            tokio::time::sleep(delay).await;
                            json!({ "count": 1, "feedback": [{ "text": "use tabs" }],
                                    "_commit_ids": ["n1"] })
                        }
                        _ => Value::Null,
                    };
                    let _ = write_frame(&mut stream, &json!({ "outcome": outcome })).await;
                });
            }
        });
        broker
    }

    fn record(&self, kind: &str) {
        self.seen.lock().unwrap().push(json!({ "kind": kind }));
    }

    fn find(&self, kind: &str) -> Option<Value> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .find(|m| m["kind"] == kind)
            .cloned()
    }

    async fn saw(&self, kind: &str) -> bool {
        tokio::time::timeout(WAIT, async {
            while self.find(kind).is_none() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .is_ok()
    }
}

/// An agent talking to one companion: its stdin and stdout are separate
/// streams, as with a real child process. Shared with the connector proxy's
/// tests.
pub(super) struct Agent {
    stdin: DuplexStream,
    stdout: Lines<BufReader<DuplexStream>>,
    served: tokio::task::JoinHandle<std::io::Result<()>>,
    next_id: u64,
}

struct Setup {
    ctx: CompanionContext,
    broker: Broker,
    _dir: tempfile::TempDir,
}

fn setup(features: &str) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broker.sock");
    let broker = Broker::start(&path);
    let ctx = CompanionContext {
        parent_connection_id: "parent".into(),
        socket_path: path.to_str().unwrap().into(),
        token: "t0ken".into(),
        features: CompanionFeatures::parse(Some(features)),
        custom_agents: Vec::new(),
        disabled_agents: Vec::new(),
        only_tools: None,
    };
    Setup {
        ctx,
        broker,
        _dir: dir,
    }
}

impl Agent {
    fn start(ctx: &CompanionContext) -> Self {
        let ctx = ctx.clone();
        Self::over(|read, write| serve(ctx, Arc::new(InflightCalls::new()), read, write))
    }

    /// An agent on the server `serve` runs over the agent's stdin / stdout.
    pub(super) fn over<F>(serve: impl FnOnce(DuplexStream, DuplexStream) -> F) -> Self
    where
        F: std::future::Future<Output = std::io::Result<()>> + Send + 'static,
    {
        let (stdin, server_in) = tokio::io::duplex(1 << 20);
        let (server_out, stdout) = tokio::io::duplex(1 << 20);
        let served = tokio::spawn(serve(server_in, server_out));
        Self {
            stdin,
            stdout: BufReader::new(stdout).lines(),
            served,
            next_id: 1,
        }
    }

    pub(super) async fn send(&mut self, message: Value) {
        let line = format!("{message}\n");
        self.stdin.write_all(line.as_bytes()).await.unwrap();
    }

    pub(super) async fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await;
    }

    pub(super) async fn start_request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        id
    }

    /// The reply to `id`, skipping anything else; `None` if none in `wait`.
    pub(super) async fn reply(&mut self, id: u64, wait: Duration) -> Option<Value> {
        tokio::time::timeout(wait, async {
            loop {
                let line = self.stdout.next_line().await.ok()??;
                let message: Value = serde_json::from_str(&line).ok()?;
                if message["id"] == id {
                    return Some(message);
                }
            }
        })
        .await
        .ok()
        .flatten()
    }

    /// The next notification named `method`, skipping anything else; `None`
    /// if none in `wait`.
    pub(super) async fn notification(&mut self, method: &str, wait: Duration) -> Option<Value> {
        tokio::time::timeout(wait, async {
            loop {
                let line = self.stdout.next_line().await.ok()??;
                let message: Value = serde_json::from_str(&line).ok()?;
                if message.get("id").is_none() && message["method"] == method {
                    return Some(message);
                }
            }
        })
        .await
        .ok()
        .flatten()
    }

    pub(super) async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.start_request(method, params).await;
        self.reply(id, WAIT)
            .await
            .unwrap_or_else(|| panic!("no reply to {method}"))
    }

    pub(super) async fn initialize(&mut self, version: &str) -> Value {
        let init = self
            .request(
                "initialize",
                json!({ "protocolVersion": version, "capabilities": {},
                        "clientInfo": { "name": "agent", "version": "1" } }),
            )
            .await;
        self.notify("notifications/initialized", json!({})).await;
        init
    }
}

pub(super) fn modern(mut params: Value) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientInfo": { "name": "agent", "version": "1" },
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    params
}

fn progress() -> Value {
    json!({ "name": "update_progress",
            "arguments": { "steps": [{ "title": "Step", "status": "in_progress" }] } })
}

fn text(reply: &Value) -> &str {
    reply["result"]["content"][0]["text"].as_str().unwrap_or("")
}

/// The companion's own `tools/list` result, for comparison.
async fn companion_tools(ctx: &CompanionContext) -> Value {
    let line = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let LineAction::Respond(response) =
        dispatch_line(ctx, Arc::new(InflightCalls::new()), line).await
    else {
        panic!("tools/list spawned");
    };
    response.result.unwrap()["tools"].clone()
}

const ALL: &str = "delegation,feedback,ask,sessions,tasks,automations,taskboard,\
                   browser,browser_eval,git,progress";

#[tokio::test]
async fn every_legacy_version_is_negotiated_and_answers_ping() {
    let setup = setup("progress");
    for version in LEGACY {
        let mut agent = Agent::start(&setup.ctx);
        let init = agent.initialize(version).await;
        assert_eq!(init["result"]["protocolVersion"], version);
        assert_eq!(init["result"]["serverInfo"]["name"], "zwiim-mcp");
        let ping = agent.request("ping", json!({})).await;
        assert_eq!(ping["result"], json!({}), "{version}");
        let tools = agent.request("tools/list", json!({})).await;
        assert_eq!(tools["result"]["tools"][0]["name"], "update_progress");
        let call = agent.request("tools/call", progress()).await;
        assert_eq!(text(&call), "Recorded.", "{version}");
    }
}

#[tokio::test]
async fn modern_requests_work_without_initialize() {
    let setup = setup("progress");
    let mut agent = Agent::start(&setup.ctx);
    let discover = agent.request("server/discover", modern(json!({}))).await;
    let supported = discover["result"]["supportedVersions"].as_array().unwrap();
    assert!(supported.iter().any(|v| v == MODERN));
    assert!(supported.iter().any(|v| v == "2024-11-05"));
    let tools = agent.request("tools/list", modern(json!({}))).await;
    assert_eq!(tools["result"]["tools"][0]["name"], "update_progress");
    let call = agent.request("tools/call", modern(progress())).await;
    assert_eq!(text(&call), "Recorded.");
    let future = agent
        .request(
            "tools/list",
            json!({ "_meta": { "io.modelcontextprotocol/protocolVersion": "2099-01-01" } }),
        )
        .await;
    assert_eq!(future["error"]["code"], -32022);
}

/// Same tools and schemas as the companion's own listing, with every group
/// on: each one must also parse as an MCP tool, or the companion won't start.
#[tokio::test]
async fn the_listing_is_the_companions() {
    let setup = setup(ALL);
    let mut agent = Agent::start(&setup.ctx);
    agent.initialize("2025-06-18").await;
    let listed = agent.request("tools/list", json!({})).await["result"]["tools"].clone();
    let expected = companion_tools(&setup.ctx).await;
    let pick = |tools: &Value| -> Vec<(Value, Value, Value)> {
        tools
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                (
                    t["name"].clone(),
                    t["description"].clone(),
                    t["inputSchema"].clone(),
                )
            })
            .collect()
    };
    assert!(pick(&expected).len() > 40);
    assert_eq!(pick(&listed), pick(&expected));
}

#[tokio::test]
async fn tool_errors_keep_the_companions_codes() {
    let setup = setup("progress");
    let mut agent = Agent::start(&setup.ctx);
    agent.initialize("2025-11-25").await;
    let bad = agent
        .request(
            "tools/call",
            json!({ "name": "update_progress", "arguments": {} }),
        )
        .await;
    assert_eq!(bad["error"]["code"], -32602);
    assert!(bad["error"]["message"].as_str().unwrap().contains("steps"));
    let hidden = agent
        .request(
            "tools/call",
            json!({ "name": "ask_user_question", "arguments": {} }),
        )
        .await;
    assert_eq!(hidden["error"]["code"], -32602);
    assert_eq!(
        hidden["error"]["message"],
        "unknown tool: ask_user_question"
    );
}

#[tokio::test]
async fn a_cancelled_call_gets_no_reply_and_releases_the_host() {
    let setup = setup("ask");
    let mut agent = Agent::start(&setup.ctx);
    agent.initialize("2025-06-18").await;
    let id = agent
        .start_request(
            "tools/call",
            json!({ "name": "ask_user_question", "arguments": { "questions": [{
                "question": "Which?", "header": "Pick", "multiSelect": false,
                "options": [{ "label": "A", "description": "a" },
                            { "label": "B", "description": "b" }],
            }] } }),
        )
        .await;
    assert!(setup.broker.saw("ask").await);
    agent
        .notify("notifications/cancelled", json!({ "requestId": id }))
        .await;
    assert!(setup.broker.saw("ask closed").await);
    assert_eq!(agent.reply(id, Duration::from_millis(300)).await, None);
    // Still serving.
    let ping = agent.request("ping", json!({})).await;
    assert_eq!(ping["result"], json!({}));
}

/// A delegation is bound to the agent's tool call, and cancelling it tells
/// the broker why, as the hand-written loop did.
#[tokio::test]
async fn a_delegation_keeps_its_tool_use_id_and_cancel_reason() {
    let setup = setup("delegation");
    let mut agent = Agent::start(&setup.ctx);
    agent.initialize("2025-06-18").await;
    let id = agent
        .start_request(
            "tools/call",
            json!({ "name": "delegate_to_agent",
                    "arguments": { "agent_type": "codex", "task": "t" },
                    "_meta": { "tool_use_id": "toolu_1" } }),
        )
        .await;
    assert!(setup.broker.saw("call").await);
    let call = setup.broker.find("call").unwrap();
    assert_eq!(call["parent_tool_use_id"], "toolu_1");
    let handle = call["external_handle"].clone();
    agent
        .notify(
            "notifications/cancelled",
            json!({ "requestId": id, "reason": "user stopped" }),
        )
        .await;
    assert!(setup.broker.saw("cancel").await);
    let cancel = setup.broker.find("cancel").unwrap();
    assert_eq!(cancel["external_handle"], handle);
    assert_eq!(cancel["reason"], "user stopped");
}

#[tokio::test]
async fn feedback_is_committed_once_the_reply_is_written() {
    let setup = setup("feedback");
    let mut agent = Agent::start(&setup.ctx);
    agent.initialize("2025-06-18").await;
    let reply = agent
        .request(
            "tools/call",
            json!({ "name": "check_user_feedback", "arguments": {} }),
        )
        .await;
    assert!(text(&reply).contains("use tabs"));
    assert!(setup.broker.saw("commit_feedback").await);
    assert_eq!(
        setup.broker.find("commit_feedback").unwrap()["ids"],
        json!(["n1"])
    );
}

/// The agent stops reading before the Host answers: the reply can't be
/// written, so the notes must stay pending for the next check.
#[tokio::test]
async fn feedback_is_not_committed_when_the_reply_cannot_be_written() {
    let setup = setup("feedback");
    *setup.broker.feedback_delay.lock().unwrap() = Duration::from_millis(200);
    let mut agent = Agent::start(&setup.ctx);
    agent.initialize("2025-06-18").await;
    agent
        .start_request(
            "tools/call",
            json!({ "name": "check_user_feedback", "arguments": {} }),
        )
        .await;
    assert!(setup.broker.saw("feedback").await);
    let Agent { stdin, stdout, .. } = agent;
    drop(stdout);
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(setup.broker.find("commit_feedback").is_none());
    drop(stdin);
}

#[tokio::test]
async fn an_agent_that_leaves_before_the_handshake_is_not_an_error() {
    let setup = setup("progress");
    let agent = Agent::start(&setup.ctx);
    let Agent { stdin, served, .. } = agent;
    drop(stdin);
    tokio::time::timeout(WAIT, served)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
