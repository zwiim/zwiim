//! `zwiim-mcp --connector`: the agent-facing leg of a connector (r3 §3.1,
//! leg A). One launch serves the tools of the one connector its token is
//! bound to. The Host decides everything (which tools, policy, approvals,
//! the upstream call); this side only speaks MCP to the agent, in both eras,
//! and shapes tools and results for the agent's protocol version.
//!
//! ```text
//! agent ──MCP──▶ ConnectorProxy ──connector_tools / connector_call──▶ Host
//!   ▲                 │  ◀──────── connector_watch (long-poll) ─────────┘
//!   └─ tools/list_changed (legacy) or on subscriptions/listen (modern)
//! ```

use std::io;
use std::sync::{Arc, Mutex};

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData, Implementation,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig,
    SubscriptionFilter, Tool,
};
use rmcp::service::{
    NotificationContext, Peer, RequestContext, RoleServer, ServerInitializeError,
    SubscriptionContext,
};
use rmcp::{ServerHandler, ServiceExt};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;

use crate::acp::delegation::transport::{
    client_connector_call_round_trip, client_connector_tools_round_trip,
    client_connector_watch_round_trip, BrokerConnectorCallRequest, BrokerConnectorToolsRequest,
    BrokerConnectorWatchRequest, BrokerResponse, ConnectorRevision, ConnectorTools,
};
use crate::connectors::upstream::{CallOutput, ToolDefinition};

/// The first version whose tools carry `title`, `outputSchema` and whose
/// results carry `structuredContent`.
const STRUCTURED_SINCE: &str = "2025-06-18";

/// Serves the token's connector on `read` / `write` until the agent closes
/// the stream.
pub async fn serve_connector<R, W>(
    socket_path: String,
    token: String,
    read: R,
    write: W,
) -> io::Result<()>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let host = Host { socket_path, token };
    let first = host.tools().await.unwrap_or_else(|note| {
        tracing::warn!("[connector] no tools: {note}");
        ConnectorTools {
            revision: 0,
            tools: Vec::new(),
        }
    });
    let (tools_tx, tools) = watch::channel(first);
    let legacy_peer = Arc::new(Mutex::new(None));
    let watcher = tokio::spawn(watch_tools(host.clone(), tools_tx, legacy_peer.clone()));
    let proxy = ConnectorProxy {
        host,
        tools,
        legacy_peer,
    };
    let served = match proxy.serve((read, write)).await {
        Ok(running) => {
            let _ = running.waiting().await;
            Ok(())
        }
        Err(ServerInitializeError::ConnectionClosed(_)) => Ok(()),
        Err(e) => Err(io::Error::other(format!("MCP handshake: {e}"))),
    };
    watcher.abort();
    served
}

/// The Host end of the delegation socket, for this proxy's token.
#[derive(Clone)]
struct Host {
    socket_path: String,
    token: String,
}

impl Host {
    async fn tools(&self) -> Result<ConnectorTools, String> {
        let req = BrokerConnectorToolsRequest {
            token: self.token.clone(),
        };
        answer(client_connector_tools_round_trip(&self.socket_path, &req).await)
    }

    async fn changed_since(&self, revision: u64) -> Result<u64, String> {
        let req = BrokerConnectorWatchRequest {
            token: self.token.clone(),
            revision,
        };
        let changed: ConnectorRevision =
            answer(client_connector_watch_round_trip(&self.socket_path, &req).await)?;
        Ok(changed.revision)
    }

    async fn call(&self, tool: String, arguments: Map<String, Value>) -> CallOutput {
        let req = BrokerConnectorCallRequest {
            token: self.token.clone(),
            tool,
            arguments,
        };
        answer(client_connector_call_round_trip(&self.socket_path, &req).await).unwrap_or_else(
            |note| CallOutput {
                content: vec![json!({ "type": "text", "text": note })],
                structured_content: None,
                is_error: true,
            },
        )
    }
}

/// A frame's outcome, or the line to tell the agent: the Host's refusal note,
/// or why the Host couldn't be reached.
fn answer<T: serde::de::DeserializeOwned>(
    response: io::Result<BrokerResponse>,
) -> Result<T, String> {
    let outcome = response
        .map_err(|e| format!("Zwiim couldn't reach the Host for this connector: {e}"))?
        .outcome;
    if outcome.get("error").is_some() {
        return Err(outcome["note"]
            .as_str()
            .unwrap_or("Zwiim refused this connector request.")
            .to_string());
    }
    serde_json::from_value(outcome)
        .map_err(|e| format!("Zwiim sent a connector answer this proxy couldn't read: {e}"))
}

/// Follows the Host's revision of the exposed tools, refreshes the list and
/// tells a legacy agent. Modern agents hear about it on their listen stream
/// (`ConnectorProxy::listen`), which watches the same channel. Stops when the
/// Host refuses or goes away: the proxy dies with the Host's socket anyway.
async fn watch_tools(
    host: Host,
    tools: watch::Sender<ConnectorTools>,
    legacy_peer: Arc<Mutex<Option<Peer<RoleServer>>>>,
) {
    loop {
        let revision = tools.borrow().revision;
        let changed = match host.changed_since(revision).await {
            Ok(changed) => changed,
            Err(note) => {
                tracing::warn!("[connector] stopped following tool changes: {note}");
                return;
            }
        };
        if changed == revision {
            continue;
        }
        match host.tools().await {
            Ok(fresh) => tools.send_replace(fresh),
            Err(note) => {
                tracing::warn!("[connector] stopped following tool changes: {note}");
                return;
            }
        };
        let peer = legacy_peer.lock().unwrap().clone();
        if let Some(peer) = peer {
            let _ = peer.notify_tool_list_changed().await;
        }
    }
}

struct ConnectorProxy {
    host: Host,
    tools: watch::Receiver<ConnectorTools>,
    /// Set once a legacy agent finished `initialize`; modern agents never set
    /// it, because they may only be sent notifications on a listen stream.
    legacy_peer: Arc<Mutex<Option<Peer<RoleServer>>>>,
}

impl ServerHandler for ConnectorProxy {
    fn get_info(&self) -> ServerConfig {
        let mut config = ServerConfig::default();
        config.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .build();
        config.server_info = Implementation::new("zwiim-mcp", env!("CARGO_PKG_VERSION"));
        config
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        *self.legacy_peer.lock().unwrap() = Some(context.peer);
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let version = context.protocol_version();
        let tools = self
            .tools
            .borrow()
            .tools
            .iter()
            .filter_map(|tool| agent_tool(tool, version.as_ref()))
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let call = self.host.call(
            request.name.to_string(),
            request.arguments.unwrap_or_default(),
        );
        // Dropping the call closes its socket, which cancels it on the Host.
        // rmcp sends the agent nothing for a cancelled request.
        let output = tokio::select! {
            output = call => output,
            () = context.ct.cancelled() => {
                return Err(ErrorData::internal_error("cancelled", None));
            }
        };
        let result = agent_result(output, context.protocol_version().as_ref())?;
        Ok(CallToolResponse::Complete(result))
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        let mut accepted = SubscriptionFilter::new();
        accepted.tools_list_changed = requested.tools_list_changed;
        Some(accepted)
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        let mut tools = self.tools.clone();
        tools.mark_unchanged();
        loop {
            tokio::select! {
                () = context.cancelled() => return Ok(()),
                changed = tools.changed() => {
                    if changed.is_err() {
                        // No more changes will come; keep the stream open
                        // rather than end it and have the agent listen again.
                        context.cancelled().await;
                        return Ok(());
                    }
                    if context.sink().notify_tool_list_changed().await.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }
}

fn structured(version: Option<&ProtocolVersion>) -> bool {
    version.is_some_and(|v| v.as_str() >= STRUCTURED_SINCE)
}

/// A tool as the agent's version can take it: `title` and `outputSchema`
/// only from 2025-06-18. A tool whose input schema isn't an object can't be
/// offered at all.
fn agent_tool(tool: &ToolDefinition, version: Option<&ProtocolVersion>) -> Option<Tool> {
    let mut wire = serde_json::to_value(tool).ok()?;
    if !structured(version) {
        let fields = wire.as_object_mut()?;
        fields.remove("title");
        fields.remove("outputSchema");
    }
    match serde_json::from_value(wire) {
        Ok(tool) => Some(tool),
        Err(e) => {
            tracing::warn!("[connector] not offering tool {:?}: {e}", tool.name);
            None
        }
    }
}

/// A result as the agent's version can take it. Before 2025-06-18 there is
/// no `structuredContent`, so its JSON goes into a text block unless one
/// already carries it.
fn agent_result(
    output: CallOutput,
    version: Option<&ProtocolVersion>,
) -> Result<CallToolResult, ErrorData> {
    let CallOutput {
        mut content,
        mut structured_content,
        is_error,
    } = output;
    if !structured(version) {
        if let Some(value) = structured_content.take() {
            let carried = content.iter().any(|block| {
                block["type"] == "text"
                    && block["text"]
                        .as_str()
                        .and_then(|text| serde_json::from_str::<Value>(text).ok())
                        .is_some_and(|parsed| parsed == value)
            });
            if !carried {
                content.push(json!({ "type": "text", "text": value.to_string() }));
            }
        }
    }
    let mut wire = json!({ "content": content, "isError": is_error });
    if let Some(value) = structured_content {
        wire["structuredContent"] = value;
    }
    serde_json::from_value(wire).map_err(|e| {
        ErrorData::internal_error(format!("the connector's result can't be sent: {e}"), None)
    })
}

#[cfg(all(test, unix))]
mod tests;
