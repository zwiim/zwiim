//! The MCP protocol side of `zwiim-mcp`, on rmcp's server. rmcp speaks MCP:
//! both eras (`initialize` for 2024-11-05 … 2025-11-25, per-request `_meta`
//! and `server/discover` for 2026-07-28), version negotiation, `ping` and
//! cancellation. Every tool still runs through [`dispatch_line`], so tool
//! gating, schemas, argument checks, the broker round trips and the rendered
//! results are the hand-written companion's, unchanged.
//!
//! rmcp types stay in this module: the binary only calls [`serve`], or
//! [`serve_connector`] for a connector proxy launch.
//!
//! ```text
//! agent stdin ──▶ rmcp ──▶ McpServer::call_tool ──▶ dispatch_line("tools/call")
//!                   │                                 └─▶ broker round trip
//! agent stdout ◀── DeliveryWriter ◀── rmcp ◀── reply ◀──┘
//!                   └─ reply flushed ─▶ post-reply action (feedback commit)
//! ```

use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{ready, Context, Poll};

use futures_util::future::BoxFuture;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CancelledNotificationParam, ErrorCode,
    ErrorData, Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities,
    ServerConfig, Tool,
};
use rmcp::service::{NotificationContext, RequestContext, RoleServer, ServerInitializeError};
use rmcp::{ServerHandler, ServiceExt};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncRead, AsyncWrite};

use super::companion::{
    dispatch_line, CompanionContext, InflightCalls, JsonRpcResponse, LineAction,
};

mod connector;

pub use connector::serve_connector;

/// Serves MCP on `read` / `write` until the agent closes the stream or the
/// handshake fails. In-flight calls are left to the caller's
/// [`super::companion::drain_and_cancel_all`].
pub async fn serve<R, W>(
    ctx: CompanionContext,
    inflight: Arc<InflightCalls>,
    read: R,
    write: W,
) -> io::Result<()>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let tools = tools(&ctx, &inflight).await?;
    let delivery = Delivery::default();
    let server = McpServer {
        ctx,
        inflight,
        tools,
        delivery: delivery.clone(),
    };
    let running = match server
        .serve((read, DeliveryWriter::new(write, delivery)))
        .await
    {
        Ok(running) => running,
        // The agent went away before the handshake: nothing to serve.
        Err(ServerInitializeError::ConnectionClosed(_)) => return Ok(()),
        Err(e) => return Err(io::Error::other(format!("MCP handshake: {e}"))),
    };
    let _ = running.waiting().await;
    Ok(())
}

/// The tools this launch exposes, from the companion's own `tools/list`, so
/// feature gating and the `delegate_to_agent` agent enum stay the same.
async fn tools(ctx: &CompanionContext, inflight: &Arc<InflightCalls>) -> io::Result<Vec<Tool>> {
    let line = r#"{"jsonrpc":"2.0","id":0,"method":"tools/list"}"#;
    let LineAction::Respond(JsonRpcResponse {
        result: Some(result),
        ..
    }) = dispatch_line(ctx, inflight.clone(), line).await
    else {
        return Err(io::Error::other("the companion's tools/list failed"));
    };
    serde_json::from_value(result["tools"].clone())
        .map_err(|e| io::Error::other(format!("tool schema: {e}")))
}

struct McpServer {
    ctx: CompanionContext,
    inflight: Arc<InflightCalls>,
    tools: Vec<Tool>,
    delivery: Delivery,
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerConfig {
        let mut config = ServerConfig::default();
        config.capabilities = ServerCapabilities::builder().enable_tools().build();
        config.server_info = Implementation::new("zwiim-mcp", env!("CARGO_PKG_VERSION"));
        // Told to every agent that connects, not only those that take
        // Zwiim's session instructions, so any of them prefers the browser
        // the user can watch.
        if self.ctx.features.browser {
            config.instructions = Some(crate::harness::BROWSER_LINES.join("\n"));
        }
        config
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.tools.clone()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // The agent's own id, so `notifications/cancelled` finds the call in
        // the companion's in-flight table.
        let id = serde_json::to_value(&context.id).map_err(internal)?;
        let mut params = Map::new();
        params.insert("name".into(), json!(request.name));
        if let Some(arguments) = request.arguments {
            params.insert("arguments".into(), Value::Object(arguments));
        }
        // `_meta.tool_use_id` binds a delegation to the agent's tool call.
        let meta = serde_json::to_value(&context.meta).map_err(internal)?;
        if meta.as_object().is_some_and(|m| !m.is_empty()) {
            params.insert("_meta".into(), meta);
        }
        let line = json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": params })
            .to_string();
        let response = match dispatch_line(&self.ctx, self.inflight.clone(), &line).await {
            LineAction::Respond(response) => response,
            LineAction::Spawn(spawned) => {
                // Cancellation arrives through `on_cancelled`, which wakes this
                // future; it then has no response, and rmcp sends none.
                let done = spawned.future.await;
                if let (Some(_), Some(after)) = (&done.response, done.after_relay) {
                    self.delivery.after_reply(&id, after);
                }
                match done.response {
                    Some(response) => response,
                    None => return Err(ErrorData::internal_error("cancelled", None)),
                }
            }
            LineAction::Silent => return Err(ErrorData::internal_error("no response", None)),
        };
        if let Some(error) = response.error {
            return Err(ErrorData::new(
                ErrorCode(i32::try_from(error.code).unwrap_or(ErrorCode::INTERNAL_ERROR.0)),
                error.message,
                error.data,
            ));
        }
        let result: CallToolResult =
            serde_json::from_value(response.result.unwrap_or(Value::Null)).map_err(internal)?;
        Ok(CallToolResponse::Complete(result))
    }

    async fn on_cancelled(
        &self,
        notification: CancelledNotificationParam,
        _context: NotificationContext<RoleServer>,
    ) {
        // Through the companion's own handler, reason included, so the
        // broker-side cancel of a delegation is the same as before.
        let Ok(params) = serde_json::to_value(&notification) else {
            return;
        };
        let line =
            json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": params })
                .to_string();
        dispatch_line(&self.ctx, self.inflight.clone(), &line).await;
    }
}

fn internal(error: serde_json::Error) -> ErrorData {
    ErrorData::internal_error(error.to_string(), None)
}

/// Post-reply actions waiting for their reply, keyed by the JSON text of the
/// request id.
#[derive(Clone, Default)]
struct Delivery(Arc<Mutex<HashMap<String, BoxFuture<'static, ()>>>>);

impl Delivery {
    fn after_reply(&self, id: &Value, action: BoxFuture<'static, ()>) {
        self.0.lock().unwrap().insert(id.to_string(), action);
    }

    fn is_waiting(&self) -> bool {
        !self.0.lock().unwrap().is_empty()
    }

    fn take(&self, id: &Value) -> Option<BoxFuture<'static, ()>> {
        self.0.lock().unwrap().remove(&id.to_string())
    }
}

/// The agent's stdout. A call's post-reply action (the `check_user_feedback`
/// delivery commit) runs only once that call's reply line was written and
/// flushed: if the agent is gone and the write fails, the notes stay pending
/// for the next check (at-least-once), as before rmcp.
///
/// Lines are only looked at while an action is waiting, so large replies
/// (page snapshots, screenshots) aren't copied.
struct DeliveryWriter<W> {
    inner: W,
    delivery: Delivery,
    /// The line being written, when it is being looked at.
    line: Option<Vec<u8>>,
    at_line_start: bool,
    /// Actions whose reply was written, to run after the next flush.
    written: Vec<BoxFuture<'static, ()>>,
}

impl<W> DeliveryWriter<W> {
    fn new(inner: W, delivery: Delivery) -> Self {
        Self {
            inner,
            delivery,
            line: None,
            at_line_start: true,
            written: Vec::new(),
        }
    }

    fn observe(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.at_line_start {
                self.at_line_start = false;
                self.line = self.delivery.is_waiting().then(Vec::new);
            }
            if byte == b'\n' {
                self.at_line_start = true;
                let Some(line) = self.line.take() else {
                    continue;
                };
                let id = serde_json::from_slice::<Value>(&line)
                    .ok()
                    .and_then(|message| message.get("id").cloned());
                if let Some(action) = id.and_then(|id| self.delivery.take(&id)) {
                    self.written.push(action);
                }
            } else if let Some(line) = &mut self.line {
                line.push(byte);
            }
        }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for DeliveryWriter<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let n = ready!(Pin::new(&mut this.inner).poll_write(cx, buf))?;
        this.observe(&buf[..n]);
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(Pin::new(&mut this.inner).poll_flush(cx))?;
        for action in this.written.drain(..) {
            tokio::spawn(action);
        }
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(all(test, unix))]
mod tests;
