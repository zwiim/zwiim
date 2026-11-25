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
use super::definition::{self, Auth, ConnectorDefinition, DefinitionError, Runtime, UserConnector};
use super::egress::{Credential, EgressGuard, EgressHttpClient, EndpointRule, Resolver};
use super::executions::Execution;
use super::mcp::McpUpstream;
use super::store::{self, ConnectionStatus, RevocationStep, SnapshotChanges, StoreError};
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
    /// Bumped whenever what agents see of the connection may have changed:
    /// its tools or whether it is usable. Proxies wait on it.
    revision: tokio::sync::watch::Sender<u64>,
}

impl ConnectionRuntime {
    /// Tells waiting proxies to list the connection's tools again.
    fn changed(&self) {
        self.revision.send_modify(|r| *r += 1);
    }

    pub fn revision(&self) -> u64 {
        *self.revision.borrow()
    }

    /// The current revision once it differs from `seen`.
    pub async fn changed_since(&self, seen: u64) -> u64 {
        let mut rx = self.revision.subscribe();
        // The sender lives as long as `self`, so this never fails.
        let _ = rx.wait_for(|r| *r != seen).await;
        let r = *rx.borrow();
        r
    }

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

    fn block(&self) {
        self.blocked.store(true, Ordering::SeqCst);
    }

    pub fn is_blocked(&self) -> bool {
        self.blocked.load(Ordering::SeqCst)
    }

    /// Refuses everything new, then closes what is open. Idempotent.
    pub async fn shutdown_all(&self, reason: ShutdownReason) {
        self.block();
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

/// Why a connection couldn't be created. Nothing of it is left behind.
#[derive(Debug, thiserror::Error)]
pub enum CreateError {
    #[error(transparent)]
    Definition(#[from] DefinitionError),
    #[error(transparent)]
    Credential(CredentialError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl CreateError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Definition(e) | Self::Store(StoreError::Definition(e)) => e.code(),
            Self::Credential(e) => e.code(),
            Self::Store(StoreError::InvalidLabel) => "invalid_label",
            Self::Store(_) => "internal",
        }
    }
}

/// A new connection and how its first validation went. A validation that
/// failed leaves the connection in place, recorded as `error`.
pub struct Created {
    pub id: String,
    pub validation: Result<SnapshotChanges, ValidateError>,
}

/// Why a revocation hasn't finished. The connection is unusable whatever
/// this says; only the cleanup is outstanding.
#[derive(Debug, thiserror::Error)]
pub enum RevokeError {
    #[error("no connection {0}")]
    NotFound(String),
    /// Retried on the next start, or the next revoke.
    #[error("{WAITING_FOR_STORE}: {0}")]
    WaitingForStore(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl RevokeError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "connection_not_found",
            Self::WaitingForStore(_) => "credential_store_unavailable",
            Self::Store(_) => "internal",
        }
    }
}

/// Shown on a connection whose secrets couldn't be deleted yet.
const WAITING_FOR_STORE: &str = "Waiting for the keychain to remove credentials";

/// The Host's live connections.
pub struct Connectors {
    db: DatabaseConnection,
    factory: Arc<dyn UpstreamFactory>,
    secrets: Arc<dyn SecretStore>,
    runtimes: Mutex<HashMap<String, Arc<ConnectionRuntime>>>,
    /// Each call's record as it starts and as it ends.
    executions: tokio::sync::broadcast::Sender<Execution>,
}

/// Records a slow listener may fall behind by; it re-lists after that.
const EXECUTION_EVENTS: usize = 256;

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
            executions: tokio::sync::broadcast::channel(EXECUTION_EVENTS).0,
        }
    }

    /// Each call's record as it starts and as it ends.
    pub fn subscribe_executions(&self) -> tokio::sync::broadcast::Receiver<Execution> {
        self.executions.subscribe()
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
        let upstream = match self.open(&connection).await {
            Ok(upstream) => upstream,
            Err(e) => {
                // A credential store that won't open may only be locked.
                let recorded = match &e {
                    ValidateError::Credential(CredentialError::StoreUnavailable(_)) => None,
                    ValidateError::Credential(c) => Some(c.to_string()),
                    ValidateError::Failed(message) => Some(message.clone()),
                    _ => None,
                };
                if let Some(message) = recorded {
                    self.set_status(&connection, ConnectionStatus::Error, Some(&message))
                        .await?;
                }
                return Err(e);
            }
        };
        let listing = match timeout(upstream.list_tools()).await {
            Ok(listing) => listing,
            Err(e) => {
                let _ = upstream.shutdown(ShutdownReason::Idle).await;
                let message = self.redact(&connection, &e.to_string()).await?;
                self.set_status(&connection, ConnectionStatus::Error, Some(&message))
                    .await?;
                return Err(ValidateError::Failed(message));
            }
        };
        let changes = store::save_tool_snapshot(&self.db, id, &listing.tools).await?;
        self.set_status(&connection, ConnectionStatus::Active, None)
            .await?;
        runtime
            .replace(Some(upstream.clone()), ShutdownReason::Idle)
            .await;
        self.follow(id, &upstream);
        if !changes.is_empty() {
            runtime.changed();
        }
        Ok(changes)
    }

    /// Keeps connection `id`'s snapshot in step with what `upstream`'s
    /// service reports (`list_changed`), while it is the shared upstream:
    /// each report lists the tools again, and a changed list moves the
    /// revision. A listing that fails keeps the snapshot as it was.
    fn follow(&self, id: &str, upstream: &Arc<dyn Upstream>) {
        let db = self.db.clone();
        let id = id.to_string();
        let runtime = self.runtime(&id);
        let mut reports = upstream.tool_changes();
        let upstream = Arc::downgrade(upstream);
        tokio::spawn(async move {
            while reports.changed().await.is_ok() {
                let Some(current) = upstream.upgrade() else {
                    return;
                };
                let shared = runtime.shared.lock().await.clone();
                if runtime.is_blocked() || !shared.is_some_and(|s| Arc::ptr_eq(&s, &current)) {
                    return;
                }
                let listing = match timeout(current.list_tools()).await {
                    Ok(listing) => listing,
                    Err(e) => {
                        tracing::warn!("[connectors] couldn't list {id}'s changed tools: {e}");
                        continue;
                    }
                };
                drop(current);
                match store::save_tool_snapshot(&db, &id, &listing.tools).await {
                    Ok(changes) if !changes.is_empty() => runtime.changed(),
                    Ok(_) => {}
                    Err(e) => tracing::warn!("[connectors] couldn't record {id}'s tools: {e}"),
                }
            }
        });
    }

    /// Opens a new upstream for `connection` as the user confirmed it: its
    /// definition unchanged, its credential, its endpoints. Changes nothing.
    async fn open(
        &self,
        connection: &store::Connection,
    ) -> Result<Arc<dyn Upstream>, ValidateError> {
        let definition = store::definition(&self.db, &connection.definition_id)
            .await?
            .ok_or_else(|| ValidateError::Failed("the connector's definition is gone".into()))?;
        if definition.hash() != connection.definition_hash {
            return Err(ValidateError::DefinitionChanged);
        }
        let credential = credentials::credential(&*self.secrets, &connection.id, &definition.auth)
            .map_err(ValidateError::Credential)?;
        let endpoints = store::endpoint_rules(&self.db, &connection.id).await?;
        timeout(self.factory.open(&definition, endpoints, credential))
            .await
            .map_err(|e| {
                ValidateError::Failed(credentials::redact(
                    &*self.secrets,
                    &connection.id,
                    &definition.auth,
                    &e.to_string(),
                ))
            })
    }

    /// `text` without `connection`'s secrets.
    async fn redact(
        &self,
        connection: &store::Connection,
        text: &str,
    ) -> Result<String, StoreError> {
        let auth = store::definition_auth(&self.db, &connection.definition_id)
            .await?
            .ok_or_else(|| StoreError::DefinitionNotFound(connection.definition_id.clone()))?;
        Ok(credentials::redact(
            &*self.secrets,
            &connection.id,
            &auth,
            text,
        ))
    }

    /// Moves `connection` to `status`, telling proxies when that changes
    /// whether it is usable.
    async fn set_status(
        &self,
        connection: &store::Connection,
        status: ConnectionStatus,
        last_error: Option<&str>,
    ) -> Result<(), StoreError> {
        store::set_status(&self.db, &connection.id, status, last_error).await?;
        if (connection.status == ConnectionStatus::Active) != (status == ConnectionStatus::Active) {
            self.runtime(&connection.id).changed();
        }
        Ok(())
    }
}

impl Connectors {
    /// Connects a server the user added by URL: its definition, a connection
    /// labelled `label` confirming it, and `secret` stored for its one
    /// credential field. Then validated at once. Everything is checked before
    /// anything is written, and a failure while writing undoes the rest.
    pub async fn create(
        &self,
        server: UserConnector,
        label: &str,
        secret: Option<&str>,
    ) -> Result<Created, CreateError> {
        let definition = definition::from_user(server, chrono::Utc::now())?;
        let values = secret_values(&definition.auth, secret);
        credentials::check_secrets(&definition.auth, &values).map_err(CreateError::Credential)?;
        store::save_definition(&self.db, &definition).await?;
        let connection =
            match store::create_connection(&self.db, &definition.id, label, self.secrets.kind())
                .await
            {
                Ok(connection) => connection,
                Err(e) => {
                    self.discard(&definition.id, None, &definition.auth).await;
                    return Err(e.into());
                }
            };
        if let Err(e) =
            credentials::save_secrets(&*self.secrets, &connection.id, &definition.auth, &values)
        {
            self.discard(&definition.id, Some(&connection.id), &definition.auth)
                .await;
            return Err(CreateError::Credential(e));
        }
        let validation = self.validate(&connection.id).await;
        Ok(Created {
            id: connection.id,
            validation,
        })
    }

    /// Replaces connection `id`'s key with `secret`, for a key that was
    /// wrong, expired or rotated. Validate it afterwards: the connection is
    /// the same one, its tools and label kept. Refused once it is revoking.
    pub async fn set_secret(&self, id: &str, secret: &str) -> Result<(), ValidateError> {
        let connection = store::connection(&self.db, id)
            .await?
            .ok_or_else(|| ValidateError::NotFound(id.into()))?;
        if matches!(
            connection.status,
            ConnectionStatus::Revoking | ConnectionStatus::Revoked
        ) || self.runtime(id).is_blocked()
        {
            return Err(ValidateError::Unavailable(connection.status));
        }
        let auth = store::definition_auth(&self.db, &connection.definition_id)
            .await?
            .ok_or_else(|| StoreError::DefinitionNotFound(connection.definition_id.clone()))?;
        let values = secret_values(&auth, Some(secret));
        credentials::save_secrets(&*self.secrets, id, &auth, &values)
            .map_err(ValidateError::Credential)
    }

    /// Undoes a creation that failed part way.
    async fn discard(&self, definition_id: &str, connection_id: Option<&str>, auth: &Auth) {
        if let Some(id) = connection_id {
            let _ = credentials::delete_secrets(&*self.secrets, id, auth);
        }
        if let Err(e) = store::discard_definition(&self.db, definition_id).await {
            tracing::warn!("[connectors] couldn't discard definition {definition_id}: {e}");
        }
    }
}

/// `secret` as the value of `auth`'s one credential field.
fn secret_values(auth: &Auth, secret: Option<&str>) -> HashMap<String, String> {
    let Some(secret) = secret else {
        return HashMap::new();
    };
    let name = match auth {
        Auth::Static { fields } => fields.first().map(|f| f.name.clone()),
        Auth::None | Auth::Oauth { .. } => None,
    };
    // A secret for a server that takes none is refused by name.
    HashMap::from([(name.unwrap_or_else(|| "secret".into()), secret.into())])
}

impl Connectors {
    /// Revokes connection `id` (design r3 §4.4): refused at once, then its
    /// upstreams closed and its secrets deleted, each step recorded so an
    /// interrupted revocation resumes where it stopped. Running it again is
    /// fine. A credential store that won't open leaves the connection
    /// `revoking`, waiting for it.
    pub async fn revoke(&self, id: &str) -> Result<(), RevokeError> {
        let connection = store::connection(&self.db, id)
            .await?
            .ok_or_else(|| RevokeError::NotFound(id.into()))?;
        if connection.status == ConnectionStatus::Revoked {
            return Ok(());
        }
        let runtime = self.runtime(id);
        // In memory first, so nothing in flight can start using it again.
        runtime.block();
        runtime.changed();
        let mut done = connection.revocation;
        if !done.contains(&RevocationStep::Blocked) {
            if connection.status != ConnectionStatus::Revoking {
                store::set_status(&self.db, id, ConnectionStatus::Revoking, None).await?;
            }
            self.finished(id, &mut done, RevocationStep::Blocked)
                .await?;
        }
        if !done.contains(&RevocationStep::Stopped) {
            runtime.shutdown_all(ShutdownReason::Revoked).await;
            self.finished(id, &mut done, RevocationStep::Stopped)
                .await?;
        }
        if !done.contains(&RevocationStep::UpstreamRevoked) {
            // Static keys have nothing to revoke at the service; OAuth
            // token revocation (RFC 7009) comes with OAuth.
            self.finished(id, &mut done, RevocationStep::UpstreamRevoked)
                .await?;
        }
        if !done.contains(&RevocationStep::SecretsDeleted) {
            let auth = store::definition_auth(&self.db, &connection.definition_id)
                .await?
                .ok_or_else(|| StoreError::DefinitionNotFound(connection.definition_id.clone()))?;
            if let Err(e) = credentials::delete_secrets(&*self.secrets, id, &auth) {
                store::record_revocation(&self.db, id, &done, Some(WAITING_FOR_STORE)).await?;
                return Err(RevokeError::WaitingForStore(e.to_string()));
            }
            self.finished(id, &mut done, RevocationStep::SecretsDeleted)
                .await?;
        }
        store::set_status(&self.db, id, ConnectionStatus::Revoked, None).await?;
        Ok(())
    }

    async fn finished(
        &self,
        id: &str,
        done: &mut Vec<RevocationStep>,
        step: RevocationStep,
    ) -> Result<(), StoreError> {
        done.push(step);
        store::record_revocation(&self.db, id, done, None).await
    }

    /// Finishes every revocation a Host exit interrupted. Run at start; one
    /// that still waits for the credential store stays `revoking`.
    pub async fn resume_revocations(&self) -> Result<(), StoreError> {
        for connection in store::connections(&self.db).await? {
            if connection.status != ConnectionStatus::Revoking {
                continue;
            }
            if let Err(e) = self.revoke(&connection.id).await {
                tracing::warn!("[connectors] revocation of {} waits: {e}", connection.id);
            }
        }
        Ok(())
    }
}

async fn timeout<T>(
    work: impl std::future::Future<Output = Result<T, ConnectorError>>,
) -> Result<T, ConnectorError> {
    tokio::time::timeout(SERVICE_TIMEOUT, work)
        .await
        .map_err(|_| ConnectorError::Transport("the service didn't answer in time".into()))?
}

mod gateway;

#[cfg(test)]
mod tests;
