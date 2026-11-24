//! A scriptable MCP server for tests, written by hand so the connector code is
//! never tested against the same SDK it is built on.
//!
//! One [`FakeMcp`] holds the server's tools, how each tool call behaves, and a
//! log of everything it received. It can be served over a byte stream (stdio
//! framing, [`FakeMcp::serve_stream`]) or Streamable HTTP
//! ([`FakeMcp::serve_http`]), and plays one protocol [`Era`]:
//!
//! ```text
//!               initialize            server/discover          request _meta version
//! Modern        unknown method        2026-07-28               required, must be 2026-07-28
//! DualEra       echoes legacy ver.    all versions             optional
//! Legacy(v)     always answers v      unknown method           ignored
//! LegacyAware(v) always answers v     -32022, supported [v]    ignored
//! ```
//!
//! `Legacy` is what a pre-2026 server does; `LegacyAware` is a server built on
//! a 2026-aware SDK but limited to one legacy version, which some clients fail
//! to fall back from.

mod http;
mod stream;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::broadcast;

pub use http::HttpServer;
pub use stream::StreamServer;

/// The 2026-07-28 revision: no `initialize`, no `ping`, per-request metadata.
pub const MODERN: &str = "2026-07-28";
/// Revisions that use the `initialize` handshake, oldest first.
pub const LEGACY: [&str; 4] = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_SUBSCRIPTION: &str = "io.modelcontextprotocol/subscriptionId";
const METHOD_NOT_FOUND: i64 = -32601;
const UNSUPPORTED_VERSION: i64 = -32022;

/// Which protocol revisions the fake speaks, and how it refuses the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Era {
    Modern,
    DualEra,
    Legacy(&'static str),
    LegacyAware(&'static str),
}

/// What a `tools/call` for one tool does.
#[derive(Clone, Debug)]
pub enum CallBehaviour {
    /// Returns the `text` argument as a text block.
    Echo,
    /// Echoes after the delay.
    Delay(Duration),
    /// Never answers; the request stays open until the connection ends.
    Hang,
    /// Closes the connection without answering.
    Drop,
    /// Answers with `isError: true` and this text.
    ToolError(String),
    /// Answers with a JSON-RPC error.
    RpcError { code: i64, message: String },
}

/// One message the fake received, with the HTTP headers it came with (empty
/// over a stream).
#[derive(Clone, Debug)]
pub struct Received {
    pub method: String,
    pub id: Option<Value>,
    pub params: Value,
    pub headers: Vec<(String, String)>,
}

impl Received {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// What the transport does with a message the fake handled.
#[derive(Debug)]
pub(crate) enum Reply {
    Send(Value),
    /// A notification, or a request the fake leaves unanswered on purpose.
    Nothing,
    /// Close the connection without answering.
    Drop,
}

/// Per-connection protocol state.
#[derive(Debug, Default)]
pub(crate) struct Session {
    /// Legacy `notifications/initialized` seen: plain `list_changed` may be sent.
    initialized: bool,
    /// Modern `subscriptions/listen` request id that asked for tool changes.
    listen_id: Option<Value>,
}

impl Session {
    /// The `list_changed` notification this session should get, if any.
    pub(crate) fn list_changed(&self) -> Option<Value> {
        if let Some(id) = &self.listen_id {
            return Some(json!({
                "jsonrpc": "2.0",
                "method": "notifications/tools/list_changed",
                "params": { "_meta": { META_SUBSCRIPTION: id } },
            }));
        }
        self.initialized
            .then(|| json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }))
    }
}

struct State {
    era: Era,
    tools: Mutex<Vec<Value>>,
    behaviours: Mutex<HashMap<String, CallBehaviour>>,
    received: Mutex<Vec<Received>>,
    tools_changed: broadcast::Sender<()>,
}

/// A scriptable MCP server. Clones share one server.
#[derive(Clone)]
pub struct FakeMcp {
    state: Arc<State>,
}

impl FakeMcp {
    /// A server with a single `echo` tool.
    pub fn new(era: Era) -> Self {
        let (tools_changed, _) = broadcast::channel(16);
        Self {
            state: Arc::new(State {
                era,
                tools: Mutex::new(vec![echo_tool()]),
                behaviours: Mutex::new(HashMap::new()),
                received: Mutex::new(Vec::new()),
                tools_changed,
            }),
        }
    }

    pub fn era(&self) -> Era {
        self.state.era
    }

    /// Replaces the tool list and tells connected clients it changed.
    pub fn set_tools(&self, tools: Vec<Value>) {
        *self.state.tools.lock().unwrap() = tools;
        let _ = self.state.tools_changed.send(());
    }

    /// Sets how calls to `tool` behave (default: [`CallBehaviour::Echo`]).
    pub fn on_call(&self, tool: &str, behaviour: CallBehaviour) {
        self.state
            .behaviours
            .lock()
            .unwrap()
            .insert(tool.to_owned(), behaviour);
    }

    /// Everything received so far, in order.
    pub fn received(&self) -> Vec<Received> {
        self.state.received.lock().unwrap().clone()
    }

    /// The method of everything received so far, in order.
    pub fn received_methods(&self) -> Vec<String> {
        self.received().into_iter().map(|r| r.method).collect()
    }

    /// Records a transport-level event (an HTTP method with no JSON-RPC body).
    pub(crate) fn record(&self, method: &str, headers: Vec<(String, String)>) {
        self.state.received.lock().unwrap().push(Received {
            method: method.to_owned(),
            id: None,
            params: Value::Null,
            headers,
        });
    }

    pub(crate) fn tool_changes(&self) -> broadcast::Receiver<()> {
        self.state.tools_changed.subscribe()
    }

    /// Handles one JSON-RPC message from a client.
    pub(crate) async fn handle(
        &self,
        message: Value,
        headers: Vec<(String, String)>,
        session: &Mutex<Session>,
    ) -> Reply {
        let method = message["method"].as_str().unwrap_or_default().to_owned();
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        self.state.received.lock().unwrap().push(Received {
            method: method.clone(),
            id: id.clone(),
            params: params.clone(),
            headers,
        });

        let Some(id) = id else {
            if method == "notifications/initialized" {
                session.lock().unwrap().initialized = true;
            }
            return Reply::Nothing;
        };

        let requested = params["_meta"][META_VERSION].as_str().map(str::to_owned);
        let modern = match self.check_version(&method, requested.as_deref()) {
            Ok(modern) => modern,
            Err(error) => return Reply::Send(error_reply(&id, error)),
        };

        let result = match method.as_str() {
            "initialize" => self.initialize(&params),
            "server/discover" => self.discover(&requested),
            "ping" if !modern => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": self.state.tools.lock().unwrap().clone() })),
            "tools/call" => return self.call(&id, &params, modern).await,
            "subscriptions/listen" if modern => {
                if params["notifications"]["toolsListChanged"] != json!(true) {
                    Err((METHOD_NOT_FOUND, method.clone(), None))
                } else {
                    session.lock().unwrap().listen_id = Some(id.clone());
                    return Reply::Send(json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/subscriptions/acknowledged",
                        "params": {
                            "_meta": { META_SUBSCRIPTION: id },
                            "notifications": { "toolsListChanged": true },
                        },
                    }));
                }
            }
            _ => Err((
                METHOD_NOT_FOUND,
                format!("Method not found: {method}"),
                None,
            )),
        };
        Reply::Send(match result {
            Ok(result) => result_reply(&id, result, modern),
            Err(error) => error_reply(&id, error),
        })
    }

    /// Whether a request is in the modern era, or the error refusing it.
    fn check_version(&self, method: &str, requested: Option<&str>) -> Result<bool, RpcError> {
        match self.state.era {
            Era::Legacy(_) | Era::LegacyAware(_) => Ok(false),
            Era::Modern if method == "initialize" => Ok(false),
            Era::Modern | Era::DualEra => match requested {
                Some(MODERN) => Ok(true),
                None if self.state.era == Era::DualEra => Ok(false),
                other => Err(unsupported(other, vec![MODERN])),
            },
        }
    }

    fn initialize(&self, params: &Value) -> Result<Value, RpcError> {
        let version = match self.state.era {
            Era::Modern => {
                return Err((
                    METHOD_NOT_FOUND,
                    "Method not found: initialize".into(),
                    None,
                ))
            }
            Era::Legacy(v) | Era::LegacyAware(v) => v,
            Era::DualEra => {
                let asked = params["protocolVersion"].as_str().unwrap_or_default();
                LEGACY
                    .into_iter()
                    .find(|v| *v == asked)
                    .unwrap_or(LEGACY[LEGACY.len() - 1])
            }
        };
        Ok(json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": true } },
            "serverInfo": { "name": "fake-mcp", "version": "0.0.0" },
        }))
    }

    fn discover(&self, requested: &Option<String>) -> Result<Value, RpcError> {
        let supported: Vec<&str> = match self.state.era {
            Era::Legacy(_) => {
                return Err((
                    METHOD_NOT_FOUND,
                    "Method not found: server/discover".into(),
                    None,
                ))
            }
            Era::LegacyAware(v) => return Err(unsupported(requested.as_deref(), vec![v])),
            Era::Modern => vec![MODERN],
            Era::DualEra => LEGACY.into_iter().chain([MODERN]).collect(),
        };
        Ok(json!({
            "supportedVersions": supported,
            "capabilities": { "tools": { "listChanged": true } },
            "serverInfo": { "name": "fake-mcp", "version": "0.0.0" },
        }))
    }

    async fn call(&self, id: &Value, params: &Value, modern: bool) -> Reply {
        let name = params["name"].as_str().unwrap_or_default();
        if !self
            .state
            .tools
            .lock()
            .unwrap()
            .iter()
            .any(|t| t["name"] == name)
        {
            return Reply::Send(error_reply(
                id,
                (-32602, format!("Unknown tool: {name}"), None),
            ));
        }
        let behaviour = self
            .state
            .behaviours
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or(CallBehaviour::Echo);
        let text = params["arguments"]["text"].as_str().unwrap_or_default();
        let echo = json!({ "content": [{ "type": "text", "text": text }] });
        let result = match behaviour {
            CallBehaviour::Echo => echo,
            CallBehaviour::Delay(delay) => {
                tokio::time::sleep(delay).await;
                echo
            }
            CallBehaviour::Hang => std::future::pending().await,
            CallBehaviour::Drop => return Reply::Drop,
            CallBehaviour::ToolError(text) => {
                json!({ "content": [{ "type": "text", "text": text }], "isError": true })
            }
            CallBehaviour::RpcError { code, message } => {
                return Reply::Send(error_reply(id, (code, message, None)))
            }
        };
        Reply::Send(result_reply(id, result, modern))
    }
}

type RpcError = (i64, String, Option<Value>);

fn unsupported(requested: Option<&str>, supported: Vec<&str>) -> RpcError {
    (
        UNSUPPORTED_VERSION,
        "Unsupported protocol version".into(),
        Some(json!({ "requested": requested, "supported": supported })),
    )
}

fn result_reply(id: &Value, mut result: Value, modern: bool) -> Value {
    if modern {
        result["resultType"] = json!("complete");
    }
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_reply(id: &Value, (code, message, data): RpcError) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

/// The tool every fake starts with.
pub fn echo_tool() -> Value {
    tool("echo", "Echo text back")
}

/// A tool taking one `text` string argument.
pub fn tool(name: &str, description: &str) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"],
        },
    })
}
