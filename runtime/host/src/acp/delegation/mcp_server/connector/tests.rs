//! The connector proxy over in-memory stdio, against a fake Host on a real
//! Unix socket. Raw JSON-RPC lines, so the wire is what's tested.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::net::UnixListener;
use tokio::sync::watch;

use super::serve_connector;
use crate::acp::delegation::mcp_server::tests::{modern, Agent, LEGACY, MODERN, WAIT};
use crate::acp::delegation::transport::{read_frame, write_frame};

/// The Host's side of the proxy's socket. Serves `tools` at `revision`,
/// answers `connector_watch` once the revision moves, and answers calls from
/// `results` by tool name; a call to `slow` stays open until the proxy hangs
/// up. With `refuse`, every frame gets the listener's refusal.
#[derive(Clone)]
struct Host {
    seen: Arc<Mutex<Vec<Value>>>,
    tools: Arc<watch::Sender<(u64, Value)>>,
    results: Arc<Mutex<serde_json::Map<String, Value>>>,
    refuse: bool,
}

impl Host {
    fn start(path: &std::path::Path, tools: Value, refuse: bool) -> Self {
        let listener = UnixListener::bind(path).unwrap();
        let host = Self {
            seen: Arc::default(),
            tools: Arc::new(watch::channel((1, tools)).0),
            results: Arc::default(),
            refuse,
        };
        let shared = host.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let host = shared.clone();
                tokio::spawn(async move {
                    let Ok(message) = read_frame::<_, Value>(&mut stream).await else {
                        return;
                    };
                    host.seen.lock().unwrap().push(message.clone());
                    let outcome = if host.refuse {
                        json!({ "error": "no_connector", "note": "No connector here." })
                    } else {
                        match message["kind"].as_str().unwrap_or("") {
                            "connector_tools" => {
                                let (revision, tools) = host.tools.borrow().clone();
                                json!({ "revision": revision, "tools": tools })
                            }
                            "connector_watch" => {
                                let mut tools = host.tools.subscribe();
                                let since = message["revision"].as_u64().unwrap();
                                let mut probe = [0u8; 1];
                                let revision = tokio::select! {
                                    changed = tools.wait_for(|(r, _)| *r != since) => {
                                        let current = changed.unwrap();
                                        current.0
                                    }
                                    _ = stream.read(&mut probe) => return,
                                };
                                json!({ "revision": revision })
                            }
                            "connector_call" if message["tool"] == "slow" => {
                                let _ = stream.read(&mut [0u8; 1]).await;
                                host.record("connector_call closed");
                                return;
                            }
                            "connector_call" => {
                                let tool = message["tool"].as_str().unwrap();
                                host.results.lock().unwrap()[tool].clone()
                            }
                            _ => Value::Null,
                        }
                    };
                    let _ = write_frame(&mut stream, &json!({ "outcome": outcome })).await;
                });
            }
        });
        host
    }

    fn answer(&self, tool: &str, result: Value) {
        self.results.lock().unwrap().insert(tool.into(), result);
    }

    fn change_tools(&self, tools: Value) {
        self.tools.send_modify(|(revision, current)| {
            *revision += 1;
            *current = tools;
        });
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

struct Setup {
    host: Host,
    socket: String,
    _dir: tempfile::TempDir,
}

fn setup(tools: Value) -> Setup {
    setup_with(tools, false)
}

fn setup_with(tools: Value, refuse: bool) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("host.sock");
    let host = Host::start(&path, tools, refuse);
    Setup {
        host,
        socket: path.to_str().unwrap().into(),
        _dir: dir,
    }
}

impl Setup {
    fn agent(&self) -> Agent {
        let socket = self.socket.clone();
        Agent::over(|read, write| serve_connector(socket, "pr0xy".into(), read, write))
    }
}

fn issue_tool() -> Value {
    json!({
        "name": "create_issue",
        "title": "Create issue",
        "description": "Creates an issue",
        "inputSchema": { "type": "object", "properties": { "title": { "type": "string" } } },
        "outputSchema": { "type": "object", "properties": { "id": { "type": "string" } } },
        "annotations": { "destructiveHint": false },
    })
}

fn tool(name: &str) -> Value {
    json!({ "name": name, "inputSchema": { "type": "object" } })
}

fn call(name: &str) -> Value {
    json!({ "name": name, "arguments": { "title": "Bug" } })
}

#[tokio::test]
async fn each_version_gets_the_tool_fields_it_knows() {
    let setup = setup(json!([issue_tool()]));
    for version in LEGACY {
        let mut agent = setup.agent();
        let init = agent.initialize(version).await;
        assert_eq!(init["result"]["protocolVersion"], version);
        assert_eq!(init["result"]["capabilities"]["tools"]["listChanged"], true);
        let listed = agent.request("tools/list", json!({})).await;
        let tool = &listed["result"]["tools"][0];
        assert_eq!(tool["name"], "create_issue");
        assert_eq!(tool["description"], "Creates an issue");
        assert_eq!(tool["inputSchema"], issue_tool()["inputSchema"]);
        assert_eq!(tool["annotations"], issue_tool()["annotations"]);
        let structured = version >= "2025-06-18";
        assert_eq!(tool.get("title").is_some(), structured, "{version}");
        assert_eq!(tool.get("outputSchema").is_some(), structured, "{version}");
    }
}

#[tokio::test]
async fn structured_results_reach_older_agents_as_text() {
    let setup = setup(json!([issue_tool(), tool("echo")]));
    setup.host.answer(
        "create_issue",
        json!({ "content": [], "structuredContent": { "id": "ISS-1" } }),
    );
    // Already carries the JSON as text, as the spec asks servers to.
    setup.host.answer(
        "echo",
        json!({ "content": [{ "type": "text", "text": "{\"id\":\"ISS-1\"}" }],
                "structuredContent": { "id": "ISS-1" } }),
    );

    let mut old = setup.agent();
    old.initialize("2025-03-26").await;
    let reply = old.request("tools/call", call("create_issue")).await;
    assert_eq!(
        reply["result"]["content"],
        json!([{ "type": "text", "text": "{\"id\":\"ISS-1\"}" }])
    );
    assert!(reply["result"].get("structuredContent").is_none());
    assert_eq!(reply["result"]["isError"], false);
    let reply = old.request("tools/call", call("echo")).await;
    assert_eq!(reply["result"]["content"].as_array().unwrap().len(), 1);

    let mut new = setup.agent();
    new.initialize("2025-06-18").await;
    let reply = new.request("tools/call", call("create_issue")).await;
    assert_eq!(
        reply["result"]["structuredContent"],
        json!({ "id": "ISS-1" })
    );
    assert_eq!(reply["result"]["content"], json!([]));

    let sent = setup.host.find("connector_call").unwrap();
    assert_eq!(sent["token"], "pr0xy");
    assert_eq!(sent["tool"], "create_issue");
    assert_eq!(sent["arguments"], json!({ "title": "Bug" }));
}

#[tokio::test]
async fn modern_agents_work_without_initialize() {
    let setup = setup(json!([issue_tool()]));
    setup.host.answer(
        "create_issue",
        json!({ "content": [{ "type": "text", "text": "done" }] }),
    );
    let mut agent = setup.agent();
    let discover = agent.request("server/discover", modern(json!({}))).await;
    let supported = discover["result"]["supportedVersions"].as_array().unwrap();
    assert!(supported.iter().any(|v| v == MODERN));
    let listed = agent.request("tools/list", modern(json!({}))).await;
    assert_eq!(listed["result"]["tools"][0]["title"], "Create issue");
    let reply = agent
        .request("tools/call", modern(call("create_issue")))
        .await;
    assert_eq!(reply["result"]["content"][0]["text"], "done");
}

#[tokio::test]
async fn a_tool_change_reaches_a_legacy_agent() {
    let setup = setup(json!([tool("one")]));
    let mut agent = setup.agent();
    agent.initialize("2025-11-25").await;
    assert!(setup.host.saw("connector_watch").await);

    setup.host.change_tools(json!([tool("one"), tool("two")]));
    let changed = agent
        .notification("notifications/tools/list_changed", WAIT)
        .await;
    assert!(changed.is_some(), "no list_changed");
    let listed = agent.request("tools/list", json!({})).await;
    assert_eq!(listed["result"]["tools"][1]["name"], "two");
}

#[tokio::test]
async fn a_tool_change_reaches_a_modern_agent_on_its_listen_stream() {
    let setup = setup(json!([tool("one")]));
    let mut agent = setup.agent();
    let listen = agent
        .start_request(
            "subscriptions/listen",
            modern(json!({ "notifications": { "toolsListChanged": true } })),
        )
        .await;
    let acknowledged = agent
        .notification("notifications/subscriptions/acknowledged", WAIT)
        .await
        .expect("no acknowledgement");
    assert_eq!(
        acknowledged["params"]["notifications"]["toolsListChanged"],
        true
    );
    assert!(setup.host.saw("connector_watch").await);

    setup.host.change_tools(json!([tool("two")]));
    let changed = agent
        .notification("notifications/tools/list_changed", WAIT)
        .await
        .expect("no list_changed");
    assert_eq!(
        changed["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        listen
    );
    let listed = agent.request("tools/list", modern(json!({}))).await;
    assert_eq!(listed["result"]["tools"][0]["name"], "two");
}

#[tokio::test]
async fn a_cancelled_call_gets_no_reply_and_hangs_up_on_the_host() {
    let setup = setup(json!([tool("slow")]));
    let mut agent = setup.agent();
    agent.initialize("2025-11-25").await;
    let id = agent.start_request("tools/call", call("slow")).await;
    assert!(setup.host.saw("connector_call").await);
    agent
        .notify(
            "notifications/cancelled",
            json!({ "requestId": id, "reason": "user stopped" }),
        )
        .await;
    assert!(setup.host.saw("connector_call closed").await);
    assert!(agent.reply(id, Duration::from_millis(300)).await.is_none());
}

#[tokio::test]
async fn host_refusals_reach_the_agent_as_tool_errors() {
    let setup = setup_with(json!([]), true);
    let mut agent = setup.agent();
    agent.initialize("2025-11-25").await;
    let listed = agent.request("tools/list", json!({})).await;
    assert_eq!(listed["result"]["tools"], json!([]));
    let reply = agent.request("tools/call", call("create_issue")).await;
    assert_eq!(reply["result"]["isError"], true);
    assert_eq!(reply["result"]["content"][0]["text"], "No connector here.");
}

#[tokio::test]
async fn a_tool_whose_input_schema_is_not_an_object_is_not_offered() {
    let setup = setup(json!([
        { "name": "broken", "inputSchema": "not a schema" },
        tool("fine"),
    ]));
    let mut agent = setup.agent();
    agent.initialize("2025-11-25").await;
    let listed = agent.request("tools/list", json!({})).await;
    assert_eq!(listed["result"]["tools"], json!([tool("fine")]));
}
