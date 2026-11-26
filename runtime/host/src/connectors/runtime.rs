//! Live connectors (design r3 §4.1, §4.2). [`Connectors`] holds one
//! [`ConnectorRuntime`] per connector in use and opens upstreams through
//! an [`UpstreamFactory`], the only place that matches on a definition's
//! runtime kind. Nothing here is persisted: MCP sessions don't survive the
//! Host.
//!
//! Only the connector scope exists so far. Session-scoped upstreams (legacy
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
use super::store::{self, ConnectorStatus, RevocationStep, SnapshotChanges, StoreError};
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

/// Remote MCP over Streamable HTTP, through the connector's egress rules.
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

/// Everything live for one connector.
#[derive(Default)]
pub struct ConnectorRuntime {
    shared: tokio::sync::Mutex<Option<Arc<dyn Upstream>>>,
    /// Set by revocation and Host exit: nothing new starts from here on.
    blocked: AtomicBool,
    /// Bumped whenever what agents see of the connector may have changed:
    /// its tools or whether it is usable. Proxies wait on it.
    revision: tokio::sync::watch::Sender<u64>,
}

impl ConnectorRuntime {
    /// Tells waiting proxies to list the connector's tools again.
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

/// Why validating a connector failed. `code()` is the stable name clients
/// see.
#[derive(Debug, thiserror::Error)]
pub enum ValidateError {
    #[error("no connector {0}")]
    NotFound(String),
    #[error("the connector is {0}")]
    Unavailable(ConnectorStatus),
    #[error("the connector's definition changed; confirm it again")]
    DefinitionChanged,
    #[error(transparent)]
    Credential(CredentialError),
    /// The service couldn't be reached or listed; recorded on the
    /// connector as `error`.
    #[error("{0}")]
    Failed(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl ValidateError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "connector_not_found",
            Self::Unavailable(_) => "connector_unavailable",
            Self::DefinitionChanged => "definition_changed",
            Self::Credential(e) => e.code(),
            Self::Failed(_) => "connection_failed",
            Self::Store(_) => "internal",
        }
    }
}

/// Why a connector couldn't be created. Nothing of it is left behind.
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

/// A new connector and how its first validation went. A validation that
/// failed leaves the connector in place, recorded as `error`.
pub struct Created {
    pub id: String,
    pub validation: Result<SnapshotChanges, ValidateError>,
}

/// Why a revocation hasn't finished. The connector is unusable whatever
/// this says; only the cleanup is outstanding.
#[derive(Debug, thiserror::Error)]
pub enum RevokeError {
    #[error("no connector {0}")]
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
            Self::NotFound(_) => "connector_not_found",
            Self::WaitingForStore(_) => "credential_store_unavailable",
            Self::Store(_) => "internal",
        }
    }
}

/// Shown on a connector whose secrets couldn't be deleted yet.
const WAITING_FOR_STORE: &str = "Waiting for the keychain to remove credentials";

/// The Host's live connectors.
pub struct Connectors {
    db: DatabaseConnection,
    factory: Arc<dyn UpstreamFactory>,
    secrets: Arc<dyn SecretStore>,
    runtimes: Mutex<HashMap<String, Arc<ConnectorRuntime>>>,
    /// Each call's record as it starts and as it ends.
    executions: tokio::sync::broadcast::Sender<Execution>,
    /// Calls waiting on the user, and what sessions allowed.
    asking: Arc<ask::Asking>,
}

/// The Host's connectors, made on first use and shared with whatever
/// must see them once they are (agent launches, the delegation listener).
pub type SharedConnectors = Arc<tokio::sync::OnceCell<Arc<Connectors>>>;

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
            asking: Arc::default(),
        }
    }

    /// Each call's record as it starts and as it ends.
    pub fn subscribe_executions(&self) -> tokio::sync::broadcast::Receiver<Execution> {
        self.executions.subscribe()
    }

    /// Connector `id`'s runtime, made on first use.
    pub fn runtime(&self, id: &str) -> Arc<ConnectorRuntime> {
        self.runtimes
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    /// Connects to connector `id`'s service afresh, lists its tools and
    /// records them. Success makes the connector `active`; a service that
    /// can't be reached, refuses the credential or can't be listed makes it
    /// `error`. A credential store that won't open changes nothing: the
    /// keychain may only be locked.
    pub async fn validate(&self, id: &str) -> Result<SnapshotChanges, ValidateError> {
        let connector = store::connector(&self.db, id)
            .await?
            .ok_or_else(|| ValidateError::NotFound(id.into()))?;
        let runtime = self.runtime(id);
        if matches!(
            connector.status,
            ConnectorStatus::Revoking | ConnectorStatus::Revoked
        ) || runtime.is_blocked()
        {
            return Err(ValidateError::Unavailable(connector.status));
        }
        let upstream = match self.open(&connector).await {
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
                    self.set_status(&connector, ConnectorStatus::Error, Some(&message))
                        .await?;
                }
                return Err(e);
            }
        };
        let listing = match timeout(upstream.list_tools()).await {
            Ok(listing) => listing,
            Err(e) => {
                let _ = upstream.shutdown(ShutdownReason::Idle).await;
                let message = self.redact(&connector, &e.to_string()).await?;
                self.set_status(&connector, ConnectorStatus::Error, Some(&message))
                    .await?;
                return Err(ValidateError::Failed(message));
            }
        };
        let changes = store::save_tool_snapshot(&self.db, id, &listing.tools).await?;
        self.set_status(&connector, ConnectorStatus::Active, None)
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

    /// Keeps connector `id`'s snapshot in step with what `upstream`'s
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

    /// Opens a new upstream for `connector` as the user confirmed it: its
    /// definition unchanged, its credential, its endpoints. Changes nothing.
    async fn open(&self, connector: &store::Connector) -> Result<Arc<dyn Upstream>, ValidateError> {
        let definition = store::definition(&self.db, &connector.definition_id)
            .await?
            .ok_or_else(|| ValidateError::Failed("the connector's definition is gone".into()))?;
        if definition.hash() != connector.definition_hash {
            return Err(ValidateError::DefinitionChanged);
        }
        let credential = credentials::credential(&*self.secrets, &connector.id, &definition.auth)
            .map_err(ValidateError::Credential)?;
        let endpoints = store::endpoint_rules(&self.db, &connector.id).await?;
        timeout(self.factory.open(&definition, endpoints, credential))
            .await
            .map_err(|e| {
                ValidateError::Failed(credentials::redact(
                    &*self.secrets,
                    &connector.id,
                    &definition.auth,
                    &e.to_string(),
                ))
            })
    }

    /// `text` without `connector`'s secrets.
    async fn redact(&self, connector: &store::Connector, text: &str) -> Result<String, StoreError> {
        let auth = store::definition_auth(&self.db, &connector.definition_id)
            .await?
            .ok_or_else(|| StoreError::DefinitionNotFound(connector.definition_id.clone()))?;
        Ok(credentials::redact(
            &*self.secrets,
            &connector.id,
            &auth,
            text,
        ))
    }

    /// Moves `connector` to `status`, telling proxies when that changes
    /// whether it is usable.
    async fn set_status(
        &self,
        connector: &store::Connector,
        status: ConnectorStatus,
        last_error: Option<&str>,
    ) -> Result<(), StoreError> {
        store::set_status(&self.db, &connector.id, status, last_error).await?;
        if (connector.status == ConnectorStatus::Active) != (status == ConnectorStatus::Active) {
            self.runtime(&connector.id).changed();
        }
        Ok(())
    }
}

impl Connectors {
    /// Connects a server the user added by URL: its definition, a connector
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
        let connector =
            match store::create_connector(&self.db, &definition.id, label, self.secrets.kind())
                .await
            {
                Ok(connector) => connector,
                Err(e) => {
                    self.discard(&definition.id, None, &definition.auth).await;
                    return Err(e.into());
                }
            };
        if let Err(e) =
            credentials::save_secrets(&*self.secrets, &connector.id, &definition.auth, &values)
        {
            self.discard(&definition.id, Some(&connector.id), &definition.auth)
                .await;
            return Err(CreateError::Credential(e));
        }
        let validation = self.validate(&connector.id).await;
        Ok(Created {
            id: connector.id,
            validation,
        })
    }

    /// Replaces connector `id`'s key with `secret`, for a key that was
    /// wrong, expired or rotated. Validate it afterwards: the connector is
    /// the same one, its tools and label kept. Refused once it is revoking.
    pub async fn set_secret(&self, id: &str, secret: &str) -> Result<(), ValidateError> {
        let connector = store::connector(&self.db, id)
            .await?
            .ok_or_else(|| ValidateError::NotFound(id.into()))?;
        if matches!(
            connector.status,
            ConnectorStatus::Revoking | ConnectorStatus::Revoked
        ) || self.runtime(id).is_blocked()
        {
            return Err(ValidateError::Unavailable(connector.status));
        }
        let auth = store::definition_auth(&self.db, &connector.definition_id)
            .await?
            .ok_or_else(|| StoreError::DefinitionNotFound(connector.definition_id.clone()))?;
        let values = secret_values(&auth, Some(secret));
        credentials::save_secrets(&*self.secrets, id, &auth, &values)
            .map_err(ValidateError::Credential)
    }

    /// Undoes a creation that failed part way.
    async fn discard(&self, definition_id: &str, connector_id: Option<&str>, auth: &Auth) {
        if let Some(id) = connector_id {
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
    /// Revokes connector `id` (design r3 §4.4): refused at once, then its
    /// upstreams closed and its secrets deleted, each step recorded so an
    /// interrupted revocation resumes where it stopped. Running it again is
    /// fine. A credential store that won't open leaves the connector
    /// `revoking`, waiting for it.
    pub async fn revoke(&self, id: &str) -> Result<(), RevokeError> {
        let connector = store::connector(&self.db, id)
            .await?
            .ok_or_else(|| RevokeError::NotFound(id.into()))?;
        if connector.status == ConnectorStatus::Revoked {
            return Ok(());
        }
        let runtime = self.runtime(id);
        // In memory first, so nothing in flight can start using it again.
        runtime.block();
        runtime.changed();
        self.end_approvals_for(id).await;
        let mut done = connector.revocation;
        if !done.contains(&RevocationStep::Blocked) {
            if connector.status != ConnectorStatus::Revoking {
                store::set_status(&self.db, id, ConnectorStatus::Revoking, None).await?;
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
            let auth = store::definition_auth(&self.db, &connector.definition_id)
                .await?
                .ok_or_else(|| StoreError::DefinitionNotFound(connector.definition_id.clone()))?;
            if let Err(e) = credentials::delete_secrets(&*self.secrets, id, &auth) {
                store::record_revocation(&self.db, id, &done, Some(WAITING_FOR_STORE)).await?;
                return Err(RevokeError::WaitingForStore(e.to_string()));
            }
            self.finished(id, &mut done, RevocationStep::SecretsDeleted)
                .await?;
        }
        store::set_status(&self.db, id, ConnectorStatus::Revoked, None).await?;
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
        for connector in store::connectors(&self.db).await? {
            if connector.status != ConnectorStatus::Revoking {
                continue;
            }
            if let Err(e) = self.revoke(&connector.id).await {
                tracing::warn!("[connectors] revocation of {} waits: {e}", connector.id);
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

mod ask;
mod gateway;

pub use ask::{AnswerError, ApprovalAnswer, ApprovalCard, ApprovalCards, CallSession, NoCards};

#[cfg(test)]
mod tests;
