//! The fake over Streamable HTTP on a loopback port, path `/mcp`.
//!
//! ```text
//! POST request       -> 200 JSON reply (or SSE for subscriptions/listen)
//! POST notification  -> 202
//! GET                -> 405 (no server-initiated stream for legacy clients)
//! DELETE             -> 200, ends the legacy session
//! redirect set       -> 307 to that location, for every request
//! ```
//!
//! Legacy `initialize` gets an `Mcp-Session-Id`; later requests carrying it
//! share that session. Modern requests are stateless. A call that behaves as
//! [`super::CallBehaviour::Drop`] closes its TCP connection without a response.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;
use axum::Router;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::Notify;
use tokio::task::{JoinHandle, JoinSet};

use super::{FakeMcp, Reply, Session};

const SESSION_HEADER: &str = "mcp-session-id";
const BODY_LIMIT: usize = 4 << 20;

/// A fake listening on loopback. Dropping it stops the listener and closes
/// every open connection.
pub struct HttpServer {
    addr: SocketAddr,
    redirect: Arc<Mutex<Option<String>>>,
    task: JoinHandle<()>,
}

impl HttpServer {
    /// The MCP endpoint, `http://127.0.0.1:<port>/mcp`.
    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Answers every request with a 307 to `location`, or stops when `None`.
    pub fn redirect_to(&self, location: Option<String>) {
        *self.redirect.lock().unwrap() = location;
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Clone)]
struct Shared {
    fake: FakeMcp,
    redirect: Arc<Mutex<Option<String>>>,
    sessions: Arc<Mutex<HashMap<String, Arc<Mutex<Session>>>>>,
    next_session: Arc<AtomicU64>,
}

/// Closes the TCP connection a request arrived on.
#[derive(Clone)]
struct CloseConnection(Arc<Notify>);

impl FakeMcp {
    /// Serves this fake over Streamable HTTP on a free loopback port.
    pub async fn serve_http(&self) -> HttpServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let redirect = Arc::new(Mutex::new(None));
        let router = Router::new()
            .route("/mcp", axum::routing::any(handle))
            .with_state(Shared {
                fake: self.clone(),
                redirect: redirect.clone(),
                sessions: Arc::default(),
                next_session: Arc::default(),
            });
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            while let Ok((stream, _)) = listener.accept().await {
                let close = Arc::new(Notify::new());
                let router = router
                    .clone()
                    .layer(Extension(CloseConnection(close.clone())));
                connections.spawn(async move {
                    let service = hyper_util::service::TowerToHyperService::new(router);
                    let connection = hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(stream), service);
                    tokio::select! {
                        _ = connection => {}
                        () = close.notified() => {}
                    }
                });
            }
        });
        HttpServer {
            addr,
            redirect,
            task,
        }
    }
}

async fn handle(
    State(shared): State<Shared>,
    Extension(CloseConnection(close)): Extension<CloseConnection>,
    method: Method,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if let Some(location) = shared.redirect.lock().unwrap().clone() {
        return status(StatusCode::TEMPORARY_REDIRECT)
            .header("location", location)
            .body(Body::empty())
            .unwrap();
    }
    let header_list: Vec<(String, String)> = headers
        .iter()
        .map(|(n, v)| (n.to_string(), v.to_str().unwrap_or_default().to_owned()))
        .collect();
    let session_id = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    match method {
        Method::POST => {}
        Method::DELETE => {
            if let Some(id) = &session_id {
                shared.sessions.lock().unwrap().remove(id);
            }
            shared.fake.record("HTTP DELETE", header_list);
            return status(StatusCode::OK).body(Body::empty()).unwrap();
        }
        _ => {
            shared.fake.record(&format!("HTTP {method}"), header_list);
            return status(StatusCode::METHOD_NOT_ALLOWED)
                .body(Body::empty())
                .unwrap();
        }
    }

    let Ok(bytes) = axum::body::to_bytes(body, BODY_LIMIT).await else {
        return status(StatusCode::BAD_REQUEST).body(Body::empty()).unwrap();
    };
    let Ok(message) = serde_json::from_slice::<Value>(&bytes) else {
        return status(StatusCode::BAD_REQUEST).body(Body::empty()).unwrap();
    };
    let is_initialize = message["method"] == "initialize";
    let is_listen = message["method"] == "subscriptions/listen";
    let session = session_id
        .as_ref()
        .and_then(|id| shared.sessions.lock().unwrap().get(id).cloned())
        .unwrap_or_default();

    match shared.fake.handle(message, header_list, &session).await {
        Reply::Nothing if is_listen => status(StatusCode::OK).body(Body::empty()).unwrap(),
        Reply::Nothing => status(StatusCode::ACCEPTED).body(Body::empty()).unwrap(),
        Reply::Drop => {
            close.notify_one();
            std::future::pending().await
        }
        Reply::Send(ack) if is_listen && ack.get("method").is_some() => {
            listen_stream(&shared.fake, ack, session)
        }
        Reply::Send(reply) => {
            let mut response = status(StatusCode::OK).header("content-type", "application/json");
            if is_initialize && reply.get("result").is_some() {
                let id = format!(
                    "fake-session-{}",
                    shared.next_session.fetch_add(1, Ordering::Relaxed)
                );
                shared
                    .sessions
                    .lock()
                    .unwrap()
                    .insert(id.clone(), session.clone());
                response = response.header(SESSION_HEADER, id);
            }
            response.body(Body::from(reply.to_string())).unwrap()
        }
    }
}

/// The SSE response to an accepted `subscriptions/listen`: the
/// acknowledgment, then one `list_changed` per tool change.
fn listen_stream(fake: &FakeMcp, ack: Value, session: Arc<Mutex<Session>>) -> Response {
    let changes = futures::stream::unfold(fake.tool_changes(), |mut changes| async move {
        let changed = changes.recv().await.is_ok();
        changed.then_some(((), changes))
    })
    .filter_map(move |()| std::future::ready(session.lock().unwrap().list_changed()));
    let events = futures::stream::once(std::future::ready(ack))
        .chain(changes)
        .map(|message| Ok::<_, std::convert::Infallible>(format!("data: {message}\n\n")));
    status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(events))
        .unwrap()
}

fn status(code: StatusCode) -> axum::http::response::Builder {
    Response::builder().status(code)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{json, Value};

    use super::super::{tool, CallBehaviour, Era, FakeMcp, MODERN};

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    async fn post(url: &str, session: Option<&str>, message: Value) -> reqwest::Response {
        let mut request = client()
            .post(url)
            .header("accept", "application/json, text/event-stream")
            .header("mcp-method", message["method"].as_str().unwrap_or_default())
            .json(&message);
        if let Some(session) = session {
            request = request.header("mcp-session-id", session);
        }
        request.send().await.unwrap()
    }

    fn modern(method: &str, id: u64, mut params: Value) -> Value {
        params["_meta"] = json!({ "io.modelcontextprotocol/protocolVersion": MODERN });
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
    }

    #[tokio::test]
    async fn legacy_session_lifecycle_and_headers() {
        let fake = FakeMcp::new(Era::Legacy("2025-06-18"));
        let server = fake.serve_http().await;
        let url = server.url();
        let init = post(
            &url,
            None,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-11-25", "capabilities": {},
                "clientInfo": { "name": "t", "version": "1" } } }),
        )
        .await;
        let session = init.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned();
        let body: Value = init.json().await.unwrap();
        assert_eq!(body["result"]["protocolVersion"], "2025-06-18");

        let note = post(
            &url,
            Some(&session),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        )
        .await;
        assert_eq!(note.status(), 202);
        let list: Value = post(
            &url,
            Some(&session),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(list["result"]["tools"][0]["name"], "echo");

        let get = client().get(&url).send().await.unwrap();
        assert_eq!(get.status(), 405);
        let delete = client()
            .delete(&url)
            .header("mcp-session-id", &session)
            .send()
            .await
            .unwrap();
        assert_eq!(delete.status(), 200);

        let received = fake.received();
        let listed = received.iter().find(|r| r.method == "tools/list").unwrap();
        assert_eq!(listed.header("mcp-session-id"), Some(session.as_str()));
        assert_eq!(listed.header("mcp-method"), Some("tools/list"));
        assert!(received.iter().any(|r| r.method == "HTTP DELETE"));
    }

    #[tokio::test]
    async fn modern_is_stateless_and_versioned() {
        let fake = FakeMcp::new(Era::Modern);
        let server = fake.serve_http().await;
        let response = post(
            &server.url(),
            None,
            modern(
                "tools/call",
                1,
                json!({
            "name": "echo", "arguments": { "text": "hi" } }),
            ),
        )
        .await;
        assert!(response.headers().get("mcp-session-id").is_none());
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["result"]["content"][0]["text"], "hi");
        assert_eq!(body["result"]["resultType"], "complete");
    }

    #[tokio::test]
    async fn redirect_answers_307() {
        let fake = FakeMcp::new(Era::Modern);
        let server = fake.serve_http().await;
        server.redirect_to(Some("http://203.0.113.9/mcp".into()));
        let response = post(&server.url(), None, modern("tools/list", 1, json!({}))).await;
        assert_eq!(response.status(), 307);
        assert_eq!(response.headers()["location"], "http://203.0.113.9/mcp");
        assert!(fake.received().is_empty());
        server.redirect_to(None);
        let response = post(&server.url(), None, modern("tools/list", 2, json!({}))).await;
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn drop_closes_the_connection_without_a_response() {
        let fake = FakeMcp::new(Era::Modern);
        fake.on_call("echo", CallBehaviour::Drop);
        let server = fake.serve_http().await;
        let result = client()
            .post(server.url())
            .json(&modern(
                "tools/call",
                1,
                json!({ "name": "echo", "arguments": {} }),
            ))
            .send()
            .await;
        let error = result.expect_err("connection closed");
        assert!(!error.is_timeout(), "{error}");
    }

    #[tokio::test]
    async fn listen_streams_ack_then_tool_changes() {
        let fake = FakeMcp::new(Era::Modern);
        let server = fake.serve_http().await;
        let response = post(
            &server.url(),
            None,
            modern(
                "subscriptions/listen",
                5,
                json!({
                "notifications": { "toolsListChanged": true } }),
            ),
        )
        .await;
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        let mut body = response.bytes_stream();
        let mut text = String::new();
        let mut changed = false;
        while let Ok(Some(chunk)) =
            tokio::time::timeout(Duration::from_secs(5), futures::StreamExt::next(&mut body)).await
        {
            text.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap());
            if text.contains("subscriptions/acknowledged") && !changed {
                fake.set_tools(vec![tool("next", "n")]);
                changed = true;
            }
            if text.contains("notifications/tools/list_changed") {
                assert!(text.contains("\"io.modelcontextprotocol/subscriptionId\":5"));
                return;
            }
        }
        panic!("no list_changed event: {text}");
    }
}
