//! Device end of the tunnel: `/zwiim/v1` operations and event sockets over an
//! established [`Tunnel`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::envelope::Envelope;
use super::noise::{Tunnel, TunnelError};

/// A `/zwiim/v1` response: HTTP status and JSON body.
pub type Response = (u16, Value);

/// What an event stream receives.
#[derive(Debug, PartialEq)]
pub enum StreamEvent {
    /// A text frame from `/zwiim/v1/ws`, verbatim.
    Frame(String),
    /// The socket closed. `code` is absent when the tunnel itself ended.
    Closed { code: Option<u16>, reason: Option<String> },
}

#[derive(Default)]
struct Routes {
    pending: HashMap<u64, oneshot::Sender<Response>>,
    streams: HashMap<u64, mpsc::UnboundedSender<Inbound>>,
    closed: bool,
}

enum Inbound {
    Opened { status: u16, body: Option<Value> },
    Event(StreamEvent),
}

/// Dropping the client ends the tunnel at once; [`TunnelClient::close`]
/// first sends what is queued.
pub struct TunnelClient {
    out: Option<mpsc::UnboundedSender<Envelope>>,
    routes: Arc<Mutex<Routes>>,
    next_id: AtomicU64,
    reader: JoinHandle<()>,
    writer: Option<JoinHandle<()>>,
    closer: Option<oneshot::Sender<()>>,
}

impl TunnelClient {
    pub fn start<S>(tunnel: Tunnel<S>) -> Self
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (mut reader, mut writer) = tunnel.split();
        let (out, mut out_rx) = mpsc::unbounded_channel::<Envelope>();
        let routes = Arc::new(Mutex::new(Routes::default()));

        let (closer, mut close_rx) = oneshot::channel::<()>();
        let writer = tokio::spawn(async move {
            loop {
                tokio::select! {
                    envelope = out_rx.recv() => {
                        let Some(envelope) = envelope else { break };
                        if writer.send(&envelope.encode()).await.is_err() {
                            break;
                        }
                    }
                    // Closing: send what is queued, then end the stream.
                    _ = &mut close_rx => {
                        out_rx.close();
                        while let Some(envelope) = out_rx.recv().await {
                            if writer.send(&envelope.encode()).await.is_err() {
                                break;
                            }
                        }
                        break;
                    }
                }
            }
            writer.close().await;
        });

        let reader_routes = routes.clone();
        let reader = tokio::spawn(async move {
            while let Ok(Some(message)) = reader.recv().await {
                let Some(envelope) = Envelope::decode(&message) else { break };
                let mut routes = reader_routes.lock().unwrap();
                match envelope {
                    Envelope::Res { id, status, body } => {
                        if let Some(tx) = routes.pending.remove(&id) {
                            let _ = tx.send((status, body));
                        }
                    }
                    Envelope::WsOpen { id, status, body } => {
                        if let Some(tx) = routes.streams.get(&id) {
                            let _ = tx.send(Inbound::Opened { status: status.unwrap_or(0), body });
                        }
                    }
                    Envelope::WsMsg { id, data } => {
                        if let Some(tx) = routes.streams.get(&id) {
                            let _ = tx.send(Inbound::Event(StreamEvent::Frame(data)));
                        }
                    }
                    Envelope::WsClose { id, code, reason } => {
                        if let Some(tx) = routes.streams.remove(&id) {
                            let _ = tx.send(Inbound::Event(StreamEvent::Closed { code, reason }));
                        }
                    }
                    Envelope::Req { .. } => break,
                }
            }
            // Tunnel over: fail what is pending, end every stream.
            let mut routes = reader_routes.lock().unwrap();
            routes.closed = true;
            routes.pending.clear();
            for (_, tx) in routes.streams.drain() {
                let _ = tx.send(Inbound::Event(StreamEvent::Closed { code: None, reason: None }));
            }
        });

        Self {
            out: Some(out),
            routes,
            next_id: AtomicU64::new(1),
            reader,
            writer: Some(writer),
            closer: Some(closer),
        }
    }

    fn send(&self, envelope: Envelope) -> Result<(), TunnelError> {
        self.out
            .as_ref()
            .and_then(|out| out.send(envelope).ok())
            .ok_or(TunnelError::Closed)
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Call a `/zwiim/v1` operation.
    pub async fn request(&self, op: &str, body: Value) -> Result<Response, TunnelError> {
        let id = self.next_id();
        let (tx, rx) = oneshot::channel();
        {
            let mut routes = self.routes.lock().unwrap();
            if routes.closed {
                return Err(TunnelError::Closed);
            }
            routes.pending.insert(id, tx);
        }
        self.send(Envelope::Req { id, op: op.to_string(), body })?;
        rx.await.map_err(|_| TunnelError::Closed)
    }

    /// Open a `/zwiim/v1/ws` event socket. A refusal returns its response.
    pub async fn open_events(&self) -> Result<Result<EventStream, Response>, TunnelError> {
        let id = self.next_id();
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let mut routes = self.routes.lock().unwrap();
            if routes.closed {
                return Err(TunnelError::Closed);
            }
            routes.streams.insert(id, tx);
        }
        self.send(Envelope::WsOpen { id, status: None, body: None })?;
        match rx.recv().await {
            Some(Inbound::Opened { status: 101, .. }) => Ok(Ok(EventStream {
                id,
                out: self.out.clone().ok_or(TunnelError::Closed)?,
                routes: self.routes.clone(),
                rx,
            })),
            Some(Inbound::Opened { status, body }) => {
                self.routes.lock().unwrap().streams.remove(&id);
                Ok(Err((status, body.unwrap_or(Value::Null))))
            }
            _ => Err(TunnelError::Closed),
        }
    }

    /// Whether the tunnel has ended.
    pub fn is_closed(&self) -> bool {
        self.routes.lock().unwrap().closed
    }

    /// End the tunnel.
    pub async fn close(mut self) {
        self.out.take();
        if let Some(closer) = self.closer.take() {
            let _ = closer.send(());
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.await;
        }
    }
}

impl Drop for TunnelClient {
    fn drop(&mut self) {
        self.reader.abort();
        if let Some(writer) = &self.writer {
            writer.abort();
        }
        // Streams outlive the client; tell them the tunnel is gone.
        let mut routes = self.routes.lock().unwrap();
        routes.closed = true;
        routes.pending.clear();
        for (_, tx) in routes.streams.drain() {
            let _ = tx.send(Inbound::Event(StreamEvent::Closed { code: None, reason: None }));
        }
    }
}

/// A `/zwiim/v1/ws` socket through the tunnel.
pub struct EventStream {
    id: u64,
    out: mpsc::UnboundedSender<Envelope>,
    routes: Arc<Mutex<Routes>>,
    rx: mpsc::UnboundedReceiver<Inbound>,
}

impl EventStream {
    /// Send one text frame (a `/zwiim/v1/ws` client message).
    pub fn send(&self, frame: &Value) -> Result<(), TunnelError> {
        self.out
            .send(Envelope::WsMsg { id: self.id, data: frame.to_string() })
            .map_err(|_| TunnelError::Closed)
    }

    /// The next frame or the close; `None` after the close was delivered.
    pub async fn recv(&mut self) -> Option<StreamEvent> {
        loop {
            match self.rx.recv().await? {
                Inbound::Event(event) => return Some(event),
                Inbound::Opened { .. } => {}
            }
        }
    }

    pub fn close(self) {
        self.routes.lock().unwrap().streams.remove(&self.id);
        let _ = self.out.send(Envelope::WsClose { id: self.id, code: None, reason: None });
    }
}
