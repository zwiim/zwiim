//! The fake over a byte stream with stdio framing: one JSON-RPC message per
//! line. Requests are handled concurrently, so a hanging call doesn't block
//! the next one, as with a real server.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Notify};
use tokio::task::{JoinHandle, JoinSet};

use super::{FakeMcp, Reply, Session};

/// A fake serving one stream. Dropping it closes the stream.
pub struct StreamServer {
    task: JoinHandle<()>,
}

impl StreamServer {
    /// Closes the stream as if the server process exited.
    pub fn close(&self) {
        self.task.abort();
    }
}

impl Drop for StreamServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeMcp {
    /// Serves this fake on `io` until the peer closes it, a call behaves as
    /// [`super::CallBehaviour::Drop`], or the returned handle is dropped.
    pub fn serve_stream<S>(&self, io: S) -> StreamServer
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let fake = self.clone();
        let task = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(io);
            let (out, mut outgoing) = mpsc::unbounded_channel::<Value>();
            let session = Arc::new(Mutex::new(Session::default()));
            let dropped = Arc::new(Notify::new());
            let mut tasks = JoinSet::new();

            tasks.spawn(async move {
                while let Some(message) = outgoing.recv().await {
                    let line = format!("{message}\n");
                    if write.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                }
            });
            let mut changes = fake.tool_changes();
            let (notify_session, notify_out) = (session.clone(), out.clone());
            tasks.spawn(async move {
                while changes.recv().await.is_ok() {
                    let note = notify_session.lock().unwrap().list_changed();
                    if let Some(note) = note {
                        let _ = notify_out.send(note);
                    }
                }
            });

            let mut lines = BufReader::new(read).lines();
            loop {
                tokio::select! {
                    line = lines.next_line() => {
                        let Ok(Some(line)) = line else { break };
                        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
                        let (fake, session, out, dropped) =
                            (fake.clone(), session.clone(), out.clone(), dropped.clone());
                        tasks.spawn(async move {
                            match fake.handle(message, Vec::new(), &session).await {
                                Reply::Send(reply) => { let _ = out.send(reply); }
                                Reply::Nothing => {}
                                Reply::Drop => dropped.notify_one(),
                            }
                        });
                    }
                    () = dropped.notified() => break,
                }
            }
            // Dropping `tasks` aborts the writer, so the peer sees the stream end.
        });
        StreamServer { task }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{json, Value};
    use tokio::io::{
        AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
    };

    use super::super::{tool, CallBehaviour, Era, FakeMcp, LEGACY, MODERN};
    use super::StreamServer;

    /// A raw client on the other end of the stream.
    struct Client {
        write: WriteHalf<DuplexStream>,
        lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
        _server: StreamServer,
    }

    impl Client {
        fn connect(fake: &FakeMcp) -> Self {
            let (ours, theirs) = tokio::io::duplex(1 << 16);
            let server = fake.serve_stream(theirs);
            let (read, write) = tokio::io::split(ours);
            Self {
                write,
                lines: BufReader::new(read).lines(),
                _server: server,
            }
        }

        async fn send(&mut self, message: Value) {
            let line = format!("{message}\n");
            self.write.write_all(line.as_bytes()).await.unwrap();
        }

        /// The next message, or `None` when the stream ended.
        async fn next(&mut self) -> Option<Value> {
            let line = tokio::time::timeout(Duration::from_secs(5), self.lines.next_line())
                .await
                .expect("message in time")
                .unwrap()?;
            Some(serde_json::from_str(&line).unwrap())
        }

        async fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
            self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
                .await;
            self.next().await.expect("a reply")
        }

        async fn initialize(&mut self, version: &str) -> Value {
            let reply = self
                .request(
                    0,
                    "initialize",
                    json!({
                        "protocolVersion": version,
                        "capabilities": {},
                        "clientInfo": { "name": "test", "version": "1" },
                    }),
                )
                .await;
            self.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
                .await;
            reply
        }
    }

    fn modern(params: Value) -> Value {
        let mut params = params;
        params["_meta"] = json!({ "io.modelcontextprotocol/protocolVersion": MODERN });
        params
    }

    fn call(text: &str) -> Value {
        json!({ "name": "echo", "arguments": { "text": text } })
    }

    #[tokio::test]
    async fn dual_era_echoes_each_legacy_version() {
        for version in LEGACY {
            let mut client = Client::connect(&FakeMcp::new(Era::DualEra));
            let init = client.initialize(version).await;
            assert_eq!(init["result"]["protocolVersion"], version);
            let ping = client.request(1, "ping", json!({})).await;
            assert_eq!(ping["result"], json!({}));
            let tools = client.request(2, "tools/list", json!({})).await;
            assert_eq!(tools["result"]["tools"][0]["name"], "echo");
            assert!(tools["result"].get("resultType").is_none());
            let out = client.request(3, "tools/call", call("hi")).await;
            assert_eq!(out["result"]["content"][0]["text"], "hi");
        }
    }

    #[tokio::test]
    async fn dual_era_answers_modern_requests() {
        let mut client = Client::connect(&FakeMcp::new(Era::DualEra));
        let discover = client
            .request(1, "server/discover", modern(json!({})))
            .await;
        let supported = discover["result"]["supportedVersions"].as_array().unwrap();
        assert_eq!(supported.len(), LEGACY.len() + 1);
        let out = client.request(2, "tools/call", modern(call("yo"))).await;
        assert_eq!(out["result"]["content"][0]["text"], "yo");
        assert_eq!(out["result"]["resultType"], "complete");
        let ping = client.request(3, "ping", modern(json!({}))).await;
        assert_eq!(ping["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn modern_refuses_initialize_and_other_versions() {
        let fake = FakeMcp::new(Era::Modern);
        let mut client = Client::connect(&fake);
        let init = client.initialize("2025-11-25").await;
        assert_eq!(init["error"]["code"], -32601);
        let bare = client.request(1, "tools/list", json!({})).await;
        assert_eq!(bare["error"]["code"], -32022);
        let mut params = json!({});
        params["_meta"] = json!({ "io.modelcontextprotocol/protocolVersion": "2027-01-01" });
        let future = client.request(2, "tools/list", params).await;
        assert_eq!(future["error"]["code"], -32022);
        assert_eq!(future["error"]["data"]["supported"], json!([MODERN]));
        assert_eq!(future["error"]["data"]["requested"], "2027-01-01");
    }

    #[tokio::test]
    async fn legacy_answers_its_own_version_and_not_discover() {
        let mut client = Client::connect(&FakeMcp::new(Era::Legacy("2025-03-26")));
        let discover = client
            .request(1, "server/discover", modern(json!({})))
            .await;
        assert_eq!(discover["error"]["code"], -32601);
        let init = client.initialize("2025-11-25").await;
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    }

    #[tokio::test]
    async fn legacy_aware_refuses_discover_with_its_version() {
        let mut client = Client::connect(&FakeMcp::new(Era::LegacyAware("2025-06-18")));
        let discover = client
            .request(1, "server/discover", modern(json!({})))
            .await;
        assert_eq!(discover["error"]["code"], -32022);
        assert_eq!(
            discover["error"]["data"]["supported"],
            json!(["2025-06-18"])
        );
        let init = client.initialize("2025-11-25").await;
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    }

    #[tokio::test]
    async fn call_behaviours() {
        let fake = FakeMcp::new(Era::DualEra);
        fake.set_tools(vec![
            tool("echo", "e"),
            tool("bad", "b"),
            tool("broken", "x"),
        ]);
        fake.on_call("bad", CallBehaviour::ToolError("nope".into()));
        fake.on_call(
            "broken",
            CallBehaviour::RpcError {
                code: -32000,
                message: "boom".into(),
            },
        );
        let mut client = Client::connect(&fake);
        client.initialize("2025-11-25").await;
        let bad = client
            .request(1, "tools/call", json!({ "name": "bad", "arguments": {} }))
            .await;
        assert_eq!(bad["result"]["isError"], true);
        assert_eq!(bad["result"]["content"][0]["text"], "nope");
        let broken = client
            .request(
                2,
                "tools/call",
                json!({ "name": "broken", "arguments": {} }),
            )
            .await;
        assert_eq!(broken["error"]["code"], -32000);
        let unknown = client
            .request(
                3,
                "tools/call",
                json!({ "name": "missing", "arguments": {} }),
            )
            .await;
        assert_eq!(unknown["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn hanging_call_does_not_block_others_and_cancel_is_recorded() {
        let fake = FakeMcp::new(Era::DualEra);
        fake.set_tools(vec![tool("echo", "e"), tool("slow", "s")]);
        fake.on_call("slow", CallBehaviour::Hang);
        let mut client = Client::connect(&fake);
        client.initialize("2025-11-25").await;
        client
            .send(json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                "params": { "name": "slow", "arguments": { "text": "z" } } }))
            .await;
        let out = client.request(8, "tools/call", call("fast")).await;
        assert_eq!(out["id"], 8);
        client
            .send(
                json!({ "jsonrpc": "2.0", "method": "notifications/cancelled",
                "params": { "requestId": 7 } }),
            )
            .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let cancel = fake
            .received()
            .into_iter()
            .find(|r| r.method == "notifications/cancelled")
            .expect("cancel recorded");
        assert_eq!(cancel.params["requestId"], 7);
    }

    #[tokio::test]
    async fn delay_answers_late() {
        let fake = FakeMcp::new(Era::DualEra);
        fake.on_call("echo", CallBehaviour::Delay(Duration::from_millis(100)));
        let mut client = Client::connect(&fake);
        client.initialize("2025-11-25").await;
        let started = std::time::Instant::now();
        let out = client.request(1, "tools/call", call("late")).await;
        assert_eq!(out["result"]["content"][0]["text"], "late");
        assert!(started.elapsed() >= Duration::from_millis(100));
    }

    #[tokio::test]
    async fn drop_closes_the_stream_without_answering() {
        let fake = FakeMcp::new(Era::DualEra);
        fake.on_call("echo", CallBehaviour::Drop);
        let mut client = Client::connect(&fake);
        client.initialize("2025-11-25").await;
        client
            .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": call("x") }))
            .await;
        assert_eq!(client.next().await, None);
    }

    #[tokio::test]
    async fn legacy_tool_change_notifies_after_initialized() {
        let fake = FakeMcp::new(Era::DualEra);
        let mut client = Client::connect(&fake);
        client.initialize("2025-06-18").await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        fake.set_tools(vec![tool("other", "o")]);
        let note = client.next().await.unwrap();
        assert_eq!(note["method"], "notifications/tools/list_changed");
        let tools = client.request(1, "tools/list", json!({})).await;
        assert_eq!(tools["result"]["tools"][0]["name"], "other");
    }

    #[tokio::test]
    async fn modern_tool_change_only_on_listen_stream() {
        let fake = FakeMcp::new(Era::Modern);
        let mut client = Client::connect(&fake);
        client
            .request(1, "server/discover", modern(json!({})))
            .await;
        // Not subscribed: a change sends nothing.
        fake.set_tools(vec![tool("a", "a")]);
        let ack = client
            .request(
                2,
                "subscriptions/listen",
                modern(json!({ "notifications": { "toolsListChanged": true } })),
            )
            .await;
        assert_eq!(ack["method"], "notifications/subscriptions/acknowledged");
        assert_eq!(
            ack["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
            2
        );
        fake.set_tools(vec![tool("b", "b")]);
        let note = client.next().await.unwrap();
        assert_eq!(note["method"], "notifications/tools/list_changed");
        assert_eq!(
            note["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
            2
        );
    }
}
