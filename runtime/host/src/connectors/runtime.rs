//! Live connections (design r3 §4.1, §4.2). [`Connectors`] holds one
//! [`ConnectionRuntime`] per connection in use and opens upstreams through
//! an [`UpstreamFactory`], the only place that matches on a definition's
//! runtime kind. Nothing here is persisted: MCP sessions don't survive the
//! Host.
//!
//! Only the connection scope exists so far. Session-scoped upstreams (legacy
//! MCP sessions per Zwiim session) come with the gateway that serves agents.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use url::Url;

use super::credentials::{self, CredentialError, SecretStore};
use super::definition::{ConnectorDefinition, Runtime};
use super::egress::{Credential, EgressGuard, EgressHttpClient, EndpointRule, Resolver};
use super::mcp::McpUpstream;
use super::store::{self, ConnectionStatus, SnapshotChanges, StoreError};
use super::upstream::{ConnectorError, ShutdownReason, Upstream};

/// How long connecting to a service, or one listing, may take.
const SERVICE_TIMEOUT: Duration = Duration::from_secs(30);

/// Opens upstreams. The only place that matches on [`Runtime`].
#[async_trait]
pub trait UpstreamFactory: Send + Sync {
    async fn open(
        &self,
        definition: &ConnectorDefinition,
        endpoints: Vec<EndpointRule>,
        credential: Option<Credential>,
    ) -> Result<Arc<dyn Upstream>, ConnectorError>;
}

/// Remote MCP over Streamable HTTP, through the connection's egress rules.
/// Local (stdio) servers are refused: there is no sandbox to run them in.
pub struct McpFactory {
    resolver: Arc<dyn Resolver>,
}

impl McpFactory {
    pub fn new(resolver: Arc<dyn Resolver>) -> Self {
        Self { resolver }
    }
}

#[async_trait]
impl UpstreamFactory for McpFactory {
    async fn open(
        &self,
        definition: &ConnectorDefinition,
        endpoints: Vec<EndpointRule>,
        credential: Option<Credential>,
    ) -> Result<Arc<dyn Upstream>, ConnectorError> {
        match &definition.runtime {
            Runtime::McpHttp { url, .. } => {
                let url = Url::parse(url).map_err(|e| ConnectorError::Protocol(e.to_string()))?;
                let guard = Arc::new(EgressGuard::new(endpoints, self.resolver.clone()));
                let egress = EgressHttpClient::new(guard)
                    .map_err(|e| ConnectorError::Transport(e.to_string()))?;
                let upstream =
                    McpUpstream::connect_http(&url, Arc::new(egress), credential).await?;
                Ok(Arc::new(upstream))
            }
            Runtime::McpStdio { .. } => Err(ConnectorError::Protocol(
                "local connectors need Zwiim's sandbox, which isn't available yet".into(),
            )),
        }
    }
}

/// Everything live for one connection.
#[derive(Default)]
pub struct ConnectionRuntime {
    shared: tokio::sync::Mutex<Option<Arc<dyn Upstream>>>,
    /// Set by revocation and Host exit: nothing new starts from here on.
    blocked: AtomicBool,
}

impl ConnectionRuntime {
    /// Replaces the shared upstream, closing the one it had. Once blocked,
    /// a new upstream is closed instead of kept.
    async fn replace(&self, upstream: Option<Arc<dyn Upstream>>, reason: ShutdownReason) {
        let mut shared = self.shared.lock().await;
        let (keep, close) = if self.is_blocked() {
            (None, [shared.take(), upstream])
        } else {
            let old = shared.take();
            (upstream, [old, None])
        };
        *shared = keep;
        for upstream in close.into_iter().flatten() {
            let _ = upstream.shutdown(reason).await;
        }
    }

    pub fn is_blocked(&self) -> bool {
        self.blocked.load(Ordering::SeqCst)
    }

    /// Refuses everything new, then closes what is open. Idempotent.
    pub async fn shutdown_all(&self, reason: ShutdownReason) {
        self.blocked.store(true, Ordering::SeqCst);
        self.replace(None, reason).await;
    }
}

/// Why validating a connection failed. `code()` is the stable name clients
/// see.
#[derive(Debug, thiserror::Error)]
pub enum ValidateError {
    #[error("no connection {0}")]
    NotFound(String),
    #[error("the connection is {0}")]
    Unavailable(ConnectionStatus),
    #[error("the connector's definition changed; confirm it again")]
    DefinitionChanged,
    #[error(transparent)]
    Credential(CredentialError),
    /// The service couldn't be reached or listed; recorded on the
    /// connection as `error`.
    #[error("{0}")]
    Failed(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl ValidateError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "connection_not_found",
            Self::Unavailable(_) => "connection_unavailable",
            Self::DefinitionChanged => "definition_changed",
            Self::Credential(e) => e.code(),
            Self::Failed(_) => "connection_failed",
            Self::Store(_) => "internal",
        }
    }
}

/// The Host's live connections.
pub struct Connectors {
    db: DatabaseConnection,
    factory: Arc<dyn UpstreamFactory>,
    secrets: Arc<dyn SecretStore>,
    runtimes: Mutex<HashMap<String, Arc<ConnectionRuntime>>>,
}

impl Connectors {
    pub fn new(
        db: DatabaseConnection,
        factory: Arc<dyn UpstreamFactory>,
        secrets: Arc<dyn SecretStore>,
    ) -> Self {
        Self {
            db,
            factory,
            secrets,
            runtimes: Mutex::default(),
        }
    }

    /// Connection `id`'s runtime, made on first use.
    pub fn runtime(&self, id: &str) -> Arc<ConnectionRuntime> {
        self.runtimes
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    /// Connects to connection `id`'s service afresh, lists its tools and
    /// records them. Success makes the connection `active`; a service that
    /// can't be reached, refuses the credential or can't be listed makes it
    /// `error`. A credential store that won't open changes nothing: the
    /// keychain may only be locked.
    pub async fn validate(&self, id: &str) -> Result<SnapshotChanges, ValidateError> {
        let connection = store::connection(&self.db, id)
            .await?
            .ok_or_else(|| ValidateError::NotFound(id.into()))?;
        let runtime = self.runtime(id);
        if matches!(
            connection.status,
            ConnectionStatus::Revoking | ConnectionStatus::Revoked
        ) || runtime.is_blocked()
        {
            return Err(ValidateError::Unavailable(connection.status));
        }
        let definition = store::definition(&self.db, &connection.definition_id)
            .await?
            .ok_or_else(|| ValidateError::Failed("the connector's definition is gone".into()))?;
        if definition.hash() != connection.definition_hash {
            return Err(ValidateError::DefinitionChanged);
        }
        let credential = match credentials::credential(&*self.secrets, id, &definition.auth) {
            Ok(credential) => credential,
            Err(e @ CredentialError::StoreUnavailable(_)) => {
                return Err(ValidateError::Credential(e))
            }
            Err(e) => {
                store::set_status(&self.db, id, ConnectionStatus::Error, Some(&e.to_string()))
                    .await?;
                return Err(ValidateError::Credential(e));
            }
        };
        let endpoints = store::endpoint_rules(&self.db, id).await?;

        let listed = async {
            let upstream = self.factory.open(&definition, endpoints, credential);
            let upstream = timeout(upstream).await?;
            match timeout(upstream.list_tools()).await {
                Ok(listing) => Ok((upstream, listing)),
                Err(e) => {
                    let _ = upstream.shutdown(ShutdownReason::Idle).await;
                    Err(e)
                }
            }
        };
        let (upstream, listing) = match listed.await {
            Ok(listed) => listed,
            Err(e) => {
                let message =
                    credentials::redact(&*self.secrets, id, &definition.auth, &e.to_string());
                store::set_status(&self.db, id, ConnectionStatus::Error, Some(&message)).await?;
                return Err(ValidateError::Failed(message));
            }
        };
        let changes = store::save_tool_snapshot(&self.db, id, &listing.tools).await?;
        store::set_status(&self.db, id, ConnectionStatus::Active, None).await?;
        runtime.replace(Some(upstream), ShutdownReason::Idle).await;
        Ok(changes)
    }
}

async fn timeout<T>(
    work: impl std::future::Future<Output = Result<T, ConnectorError>>,
) -> Result<T, ConnectorError> {
    tokio::time::timeout(SERVICE_TIMEOUT, work)
        .await
        .map_err(|_| ConnectorError::Transport("the service didn't answer in time".into()))?
}

#[cfg(test)]
mod tests;
