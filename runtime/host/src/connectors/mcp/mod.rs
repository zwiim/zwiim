//! [`Upstream`] for MCP servers, built on rmcp. rmcp speaks the protocol:
//! JSON-RPC, version negotiation and the transport framing. Zwiim keeps
//! everything else: every HTTP byte goes through the egress client, rmcp
//! retries nothing and has no timeouts of its own, and only Zwiim's types leave
//! this module.

mod convert;
mod http_client;
#[cfg(any(test, feature = "test-utils"))]
mod stdio;

use std::sync::Arc;

use async_trait::async_trait;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ClientCapabilities, ClientConfig, ClientRequest,
    DiscoverRequest, DiscoverRequestParams, Implementation, PingRequest, ProtocolVersion,
    ServerNotification, SubscriptionFilter,
};
use rmcp::service::{
    ClientInitializeError, NotificationContext, Peer, RoleClient, RunningService, ServiceError,
};
use rmcp::transport::common::client_side_sse::NeverRetry;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{IntoTransport, StreamableHttpClientTransport};
use rmcp::{ClientHandler, ClientLifecycleMode, ClientServiceExt};
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;
use url::Url;

use crate::connectors::egress::{Credential, EgressHttpClient};
use crate::connectors::upstream::{
    CallFailure, CallOutput, ConnectorError, DispatchMark, ShutdownReason, ToolListing,
    UnknownCause, Upstream, UpstreamCall,
};
use http_client::McpHttpClient;

/// The 2026-07-28 revision, tried first.
const MODERN: ProtocolVersion = ProtocolVersion::V_2026_07_28;
/// Revisions with the `initialize` handshake, newest first.
const LEGACY: [ProtocolVersion; 4] = [
    ProtocolVersion::V_2025_11_25,
    ProtocolVersion::V_2025_06_18,
    ProtocolVersion::V_2025_03_26,
    ProtocolVersion::V_2024_11_05,
];

/// What rmcp calls back into: the client's identity and legacy tool-change
/// notifications. Modern servers report changes on a `subscriptions/listen`
/// stream instead (see [`listen_for_tool_changes`]).
struct ZwiimClient {
    /// Sent in `initialize`.
    version: ProtocolVersion,
    changes: watch::Sender<u64>,
}

impl ClientHandler for ZwiimClient {
    fn get_info(&self) -> ClientConfig {
        let mut config = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("zwiim", env!("CARGO_PKG_VERSION")),
        );
        config.protocol_version = self.version.clone();
        config
    }

    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.changes.send_modify(|n| *n += 1);
    }
}

/// One live MCP endpoint.
pub struct McpUpstream {
    peer: Peer<RoleClient>,
    modern: bool,
    changes: watch::Receiver<u64>,
    /// Taken by [`Upstream::shutdown`].
    running: Mutex<Option<Running>>,
}

struct Running {
    service: RunningService<RoleClient, ZwiimClient>,
    listener: Option<JoinHandle<()>>,
}

impl McpUpstream {
    /// Connects to a Streamable HTTP endpoint and negotiates a version.
    pub async fn connect_http(
        url: &Url,
        egress: Arc<EgressHttpClient>,
        credential: Option<Credential>,
    ) -> Result<Self, ConnectorError> {
        let transport = || {
            let mut config = StreamableHttpClientTransportConfig::with_uri(url.as_str());
            // rmcp would otherwise reconnect dropped streams and re-initialize
            // expired sessions on its own, re-sending what may already have run.
            config.retry_config = Arc::new(NeverRetry::default());
            config.reinit_on_expired_session = false;
            StreamableHttpClientTransport::with_client(
                McpHttpClient::new(egress.clone(), credential.clone()),
                config,
            )
        };
        Self::connect(transport).await
    }

    /// `transport` makes a fresh transport for each attempt: a server that
    /// only speaks a legacy version but answers `server/discover` with
    /// "unsupported version" (rather than "unknown method") needs a second
    /// connection, since rmcp doesn't fall back from that answer.
    async fn connect<T, E, A>(transport: impl Fn() -> T) -> Result<Self, ConnectorError>
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let (changes_tx, changes) = watch::channel(0);
        let client = |version: &ProtocolVersion| ZwiimClient {
            version: version.clone(),
            changes: changes_tx.clone(),
        };
        let auto = ClientLifecycleMode::Auto {
            preferred_versions: vec![MODERN],
            legacy_version: Some(LEGACY[0].clone()),
        };
        let service = match client(&LEGACY[0])
            .serve_with_lifecycle(transport(), auto)
            .await
        {
            Ok(service) => service,
            Err(ClientInitializeError::NoCompatibleProtocolVersion {
                server_supported, ..
            }) => {
                let Some(version) = LEGACY.iter().find(|v| server_supported.contains(v)) else {
                    return Err(convert::init_error(
                        ClientInitializeError::NoCompatibleProtocolVersion {
                            client_supported: vec![MODERN],
                            server_supported,
                        },
                    ));
                };
                client(version)
                    .serve_with_lifecycle(transport(), ClientLifecycleMode::Initialize)
                    .await
                    .map_err(convert::init_error)?
            }
            Err(error) => return Err(convert::init_error(error)),
        };
        let info = service
            .peer()
            .peer_info()
            .ok_or_else(|| ConnectorError::Protocol("no server info after connecting".into()))?;
        let modern = info.protocol_version == MODERN;
        let announces_changes = info
            .capabilities
            .tools
            .as_ref()
            .and_then(|tools| tools.list_changed)
            .unwrap_or(false);
        let peer = service.peer().clone();
        let listener = (modern && announces_changes)
            .then(|| tokio::spawn(listen_for_tool_changes(peer.clone(), changes_tx)));
        Ok(Self {
            peer,
            modern,
            changes,
            running: Mutex::new(Some(Running { service, listener })),
        })
    }

    /// The negotiated protocol version.
    pub fn protocol_version(&self) -> Option<String> {
        self.peer
            .peer_info()
            .map(|info| info.protocol_version.as_str().to_string())
    }
}

/// Keeps a `subscriptions/listen` stream open and counts tool-list changes.
async fn listen_for_tool_changes(peer: Peer<RoleClient>, changes: watch::Sender<u64>) {
    let mut filter = SubscriptionFilter::default();
    filter.tools_list_changed = Some(true);
    let mut subscription = match peer.listen(filter).await {
        Ok(subscription) => subscription,
        Err(error) => {
            tracing::warn!(%error, "connector: could not listen for tool changes");
            return;
        }
    };
    while let Ok(Some(notification)) = subscription.next().await {
        if matches!(
            notification,
            ServerNotification::ToolListChangedNotification(_)
        ) {
            changes.send_modify(|n| *n += 1);
        }
    }
}

#[async_trait]
impl Upstream for McpUpstream {
    async fn list_tools(&self) -> Result<ToolListing, ConnectorError> {
        let list = self
            .peer
            .list_all_tools_with_cache_hint()
            .await
            .map_err(convert::service_error)?;
        Ok(ToolListing {
            tools: list
                .items
                .iter()
                .map(convert::tool)
                .collect::<Result<_, _>>()?,
            ttl: convert::ttl(list.ttl_ms),
        })
    }

    async fn call(
        &self,
        call: UpstreamCall,
        dispatch: &DispatchMark,
    ) -> Result<CallOutput, CallFailure> {
        let mut params = CallToolRequestParams::new(call.tool);
        params.arguments = Some(call.arguments);
        // rmcp gives no hook between queueing a request and writing it, so
        // the mark is set before the call is queued. A failure in between is
        // then reported as "may have run", the safe side.
        dispatch.mark();
        // Dropping this future (the executor's timeout or cancel) makes rmcp
        // send `notifications/cancelled`.
        match self.peer.call_tool_once(params).await {
            Ok(CallToolResponse::Complete(result)) => convert::call_output(&result)
                .map_err(|_| CallFailure::OutcomeUnknown(UnknownCause::ConnectionLost)),
            // Zwiim advertises neither tasks nor input requests. A task means
            // the work started and may still finish.
            Ok(CallToolResponse::Task(_)) => {
                Err(CallFailure::OutcomeUnknown(UnknownCause::ConnectionLost))
            }
            Ok(_) => Err(CallFailure::Rejected {
                code: -32603,
                message: "the service asked for input Zwiim can't provide".into(),
            }),
            Err(ServiceError::McpError(error)) => Err(CallFailure::Rejected {
                code: i64::from(error.code.0),
                message: error.message.to_string(),
            }),
            Err(ServiceError::TransportSend(error))
                if convert::egress_refusal(error.error.as_ref())
                    .is_some_and(|refusal| convert::never_sent(&refusal)) =>
            {
                Err(CallFailure::NotSent(convert::service_error(
                    ServiceError::TransportSend(error),
                )))
            }
            Err(_) => Err(CallFailure::OutcomeUnknown(UnknownCause::ConnectionLost)),
        }
    }

    async fn probe(&self) -> Result<(), ConnectorError> {
        // Sent directly: `Peer::discover` may answer from its cache.
        let request = if self.modern {
            ClientRequest::DiscoverRequest(DiscoverRequest::new(DiscoverRequestParams {}))
        } else {
            ClientRequest::PingRequest(PingRequest::default())
        };
        self.peer
            .send_request(request)
            .await
            .map(|_| ())
            .map_err(convert::service_error)
    }

    fn tool_changes(&self) -> watch::Receiver<u64> {
        self.changes.clone()
    }

    async fn shutdown(&self, _reason: ShutdownReason) -> Result<(), ConnectorError> {
        let Some(running) = self.running.lock().await.take() else {
            return Ok(());
        };
        if let Some(listener) = running.listener {
            listener.abort();
        }
        // Cancels what is in flight, closes the transport and, for a legacy
        // HTTP session, sends the DELETE that ends it.
        running
            .service
            .cancel()
            .await
            .map(|_| ())
            .map_err(|e| ConnectorError::Transport(e.to_string()))
    }
}

#[cfg(test)]
mod tests;
