//! The CDP connection to the engine (design 5.4): NUL-delimited JSON over
//! the `--remote-debugging-pipe` fds, with flattened sessions, so one reader
//! task serves every tab.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

/// Larger messages end the connection instead of growing the buffer without
/// bound. Screencast frames and screenshots are the big ones: a few MB.
const MAX_MESSAGE: usize = 64 * 1024 * 1024;
/// Events buffered per subscriber before it starts missing them.
const EVENT_BUFFER: usize = 1024;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CdpError {
    #[error("the browser engine is not running")]
    Closed,
    #[error("{method} timed out")]
    Timeout { method: String },
    #[error("{method}: {message} ({code})")]
    Protocol {
        method: String,
        code: i64,
        message: String,
    },
}

/// An event from the engine. `session_id` names the tab's session, or is
/// `None` for browser-level events.
#[derive(Debug, Clone)]
pub struct Event {
    pub session_id: Option<String>,
    pub method: String,
    pub params: Arc<Value>,
}

type Reply = Result<Value, (i64, String)>;

struct Shared {
    pending: Mutex<HashMap<u64, oneshot::Sender<Reply>>>,
    next_id: AtomicU64,
    events: broadcast::Sender<Event>,
    closed: watch::Sender<bool>,
}

/// A handle to the engine's CDP pipe. Cheap to clone.
#[derive(Clone)]
pub struct Cdp {
    shared: Arc<Shared>,
    writer: mpsc::UnboundedSender<Vec<u8>>,
}

impl std::fmt::Debug for Cdp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cdp")
            .field("closed", &self.is_closed())
            .finish()
    }
}

#[derive(Deserialize)]
struct Incoming {
    id: Option<u64>,
    method: Option<String>,
    #[serde(default)]
    params: Value,
    result: Option<Value>,
    error: Option<WireError>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct WireError {
    code: i64,
    message: String,
}

impl Cdp {
    /// Start the reader and writer tasks over the engine's pipe ends.
    pub fn new<R, W>(reader: R, writer: W) -> Cdp
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            events,
            closed: watch::channel(false).0,
        });
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(write_loop(writer, rx, shared.clone()));
        tokio::spawn(read_loop(reader, shared.clone()));
        Cdp { shared, writer: tx }
    }

    /// Send `method` and wait for its result.
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value, CdpError> {
        self.call_with_timeout(method, params, session_id, DEFAULT_TIMEOUT)
            .await
    }

    pub async fn call_with_timeout(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, CdpError> {
        self.request(method, params, session_id, timeout).await
    }

    /// Send `method` now; the returned future waits for its result. For a
    /// call whose result can arrive much later (`Page.navigate` waits for
    /// the server), so the caller need not hold anything while it waits.
    pub fn request(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
        timeout: Duration,
    ) -> impl std::future::Future<Output = Result<Value, CdpError>> + Send + 'static {
        let sent = self.enqueue(method, params, session_id);
        let shared = self.shared.clone();
        let method = method.to_string();
        async move {
            let (id, reply_rx) = sent?;
            let _forget = Forget {
                shared: &shared,
                id,
            };
            match tokio::time::timeout(timeout, reply_rx).await {
                Err(_) => Err(CdpError::Timeout { method }),
                Ok(Err(_)) => Err(CdpError::Closed),
                Ok(Ok(Ok(result))) => Ok(result),
                Ok(Ok(Err((code, message)))) => Err(CdpError::Protocol {
                    method,
                    code,
                    message,
                }),
            }
        }
    }

    fn enqueue(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<(u64, oneshot::Receiver<Reply>), CdpError> {
        if self.is_closed() {
            return Err(CdpError::Closed);
        }
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let mut message = serde_json::json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session_id {
            message["sessionId"] = Value::String(session.to_string());
        }
        let mut bytes = serde_json::to_vec(&message).expect("a JSON value serializes");
        bytes.push(0);

        let (reply_tx, reply_rx) = oneshot::channel();
        lock(&self.shared.pending).insert(id, reply_tx);
        if self.writer.send(bytes).is_err() {
            lock(&self.shared.pending).remove(&id);
            return Err(CdpError::Closed);
        }
        Ok((id, reply_rx))
    }

    /// Every event from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.shared.events.subscribe()
    }

    pub fn is_closed(&self) -> bool {
        *self.shared.closed.borrow()
    }

    /// Resolves once the pipe has closed.
    pub async fn closed(&self) {
        let mut rx = self.shared.closed.subscribe();
        let _ = rx.wait_for(|closed| *closed).await;
    }
}

/// Drops a call's pending entry however the call ends (timeout, cancel).
struct Forget<'a> {
    shared: &'a Shared,
    id: u64,
}

impl Drop for Forget<'_> {
    fn drop(&mut self) {
        lock(&self.shared.pending).remove(&self.id);
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
    shared: Arc<Shared>,
) {
    while let Some(bytes) = rx.recv().await {
        if writer.write_all(&bytes).await.is_err() || writer.flush().await.is_err() {
            break;
        }
    }
    close(&shared);
}

async fn read_loop<R: AsyncRead + Unpin>(mut reader: R, shared: Arc<Shared>) {
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut chunk = vec![0u8; 256 * 1024];
    'read: loop {
        let n = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let scanned = buf.len();
        buf.extend_from_slice(&chunk[..n]);
        let mut start = 0;
        let mut from = scanned;
        while let Some(end) = buf[from..].iter().position(|b| *b == 0).map(|i| from + i) {
            dispatch(&shared, &buf[start..end]);
            start = end + 1;
            from = start;
        }
        buf.drain(..start);
        if buf.len() > MAX_MESSAGE {
            tracing::warn!(
                bytes = buf.len(),
                "browser engine sent an oversized CDP message"
            );
            break 'read;
        }
    }
    close(&shared);
}

fn dispatch(shared: &Shared, bytes: &[u8]) {
    let msg: Incoming = match serde_json::from_slice(bytes) {
        Ok(msg) => msg,
        Err(e) => {
            tracing::warn!(error = %e, "browser engine sent unreadable CDP");
            return;
        }
    };
    if let Some(id) = msg.id {
        let reply = match msg.error {
            Some(e) => Err((e.code, e.message)),
            None => Ok(msg.result.unwrap_or(Value::Null)),
        };
        if let Some(tx) = lock(&shared.pending).remove(&id) {
            let _ = tx.send(reply);
        }
    } else if let Some(method) = msg.method {
        let _ = shared.events.send(Event {
            session_id: msg.session_id,
            method,
            params: Arc::new(msg.params),
        });
    }
}

/// Fail every waiting call and tell `closed()` waiters.
fn close(shared: &Shared) {
    shared.closed.send_replace(true);
    lock(&shared.pending).clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{duplex, AsyncBufReadExt, BufReader, DuplexStream};

    /// A fake engine on the other end of two in-memory pipes.
    fn connect() -> (Cdp, BufReader<DuplexStream>, DuplexStream) {
        let (to_engine, from_host) = duplex(1 << 20);
        let (to_host, from_engine) = duplex(1 << 20);
        (
            Cdp::new(from_engine, to_engine),
            BufReader::new(from_host),
            to_host,
        )
    }

    async fn next_request(engine: &mut BufReader<DuplexStream>) -> Value {
        let mut buf = Vec::new();
        engine.read_until(0, &mut buf).await.unwrap();
        buf.pop();
        serde_json::from_slice(&buf).unwrap()
    }

    async fn send(engine: &mut DuplexStream, msg: Value) {
        let mut bytes = serde_json::to_vec(&msg).unwrap();
        bytes.push(0);
        engine.write_all(&bytes).await.unwrap();
    }

    #[tokio::test]
    async fn calls_get_their_own_results_and_events_reach_subscribers() {
        let (cdp, mut requests, mut engine) = connect();
        let mut events = cdp.subscribe();

        let a = tokio::spawn({
            let cdp = cdp.clone();
            async move { cdp.call("Browser.getVersion", json!({}), None).await }
        });
        let b = tokio::spawn({
            let cdp = cdp.clone();
            async move {
                cdp.call("Page.navigate", json!({"url": "x"}), Some("S1"))
                    .await
            }
        });
        let first = next_request(&mut requests).await;
        let second = next_request(&mut requests).await;
        let (version, navigate) = if first["method"] == "Browser.getVersion" {
            (first, second)
        } else {
            (second, first)
        };
        assert_eq!(navigate["sessionId"], "S1");
        assert_eq!(navigate["params"]["url"], "x");
        assert!(version.get("sessionId").is_none());

        // Replies out of order, split across writes, with an event between.
        let mut bytes =
            serde_json::to_vec(&json!({"id": navigate["id"], "result": {"frameId": "F"}})).unwrap();
        bytes.push(0);
        bytes.extend(serde_json::to_vec(&json!({"method": "Page.loadEventFired", "sessionId": "S1", "params": {"timestamp": 1}})).unwrap());
        bytes.push(0);
        bytes.extend(
            serde_json::to_vec(
                &json!({"id": version["id"], "result": {"product": "HeadlessChrome/157"}}),
            )
            .unwrap(),
        );
        bytes.push(0);
        let (head, tail) = bytes.split_at(7);
        engine.write_all(head).await.unwrap();
        engine.write_all(tail).await.unwrap();

        assert_eq!(a.await.unwrap().unwrap()["product"], "HeadlessChrome/157");
        assert_eq!(b.await.unwrap().unwrap()["frameId"], "F");
        let event = events.recv().await.unwrap();
        assert_eq!(event.method, "Page.loadEventFired");
        assert_eq!(event.session_id.as_deref(), Some("S1"));
        assert_eq!(event.params["timestamp"], 1);
    }

    #[tokio::test]
    async fn protocol_errors_carry_the_engines_message() {
        let (cdp, mut requests, mut engine) = connect();
        let call = tokio::spawn({
            let cdp = cdp.clone();
            async move { cdp.call("Nope.method", json!({}), None).await }
        });
        let req = next_request(&mut requests).await;
        send(&mut engine, json!({"id": req["id"], "error": {"code": -32601, "message": "'Nope.method' wasn't found"}})).await;
        let err = call.await.unwrap().unwrap_err();
        assert_eq!(
            err,
            CdpError::Protocol {
                method: "Nope.method".into(),
                code: -32601,
                message: "'Nope.method' wasn't found".into()
            }
        );
    }

    #[tokio::test]
    async fn a_closed_pipe_fails_waiting_and_later_calls() {
        let (cdp, mut requests, engine) = connect();
        let call = tokio::spawn({
            let cdp = cdp.clone();
            async move { cdp.call("Browser.getVersion", json!({}), None).await }
        });
        next_request(&mut requests).await;
        drop(engine);
        assert_eq!(call.await.unwrap().unwrap_err(), CdpError::Closed);
        cdp.closed().await;
        assert!(cdp.is_closed());
        assert_eq!(
            cdp.call("Browser.getVersion", json!({}), None)
                .await
                .unwrap_err(),
            CdpError::Closed
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_unanswered_call_times_out_and_is_forgotten() {
        let (cdp, mut requests, _engine) = connect();
        let call = tokio::spawn({
            let cdp = cdp.clone();
            async move {
                cdp.call_with_timeout(
                    "Browser.getVersion",
                    json!({}),
                    None,
                    Duration::from_secs(3),
                )
                .await
            }
        });
        next_request(&mut requests).await;
        assert!(matches!(
            call.await.unwrap().unwrap_err(),
            CdpError::Timeout { .. }
        ));
        assert!(lock(&cdp.shared.pending).is_empty());
    }
}
