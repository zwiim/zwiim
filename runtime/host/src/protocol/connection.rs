//! Zwiim-owned: the connector platform's `/zwiim/v1` operations (connector
//! design r3 §10, phase 1). `connection.list`, `connection.create`,
//! `connection.validate`, `connection.revoke` and `connection.tools`.
//!
//! A connection is named by `connection_uid` (`connection_id` is an agent
//! connection's, which never crosses the boundary). It is answered as a
//! [`ConnectionView`], never its rows: the
//! definition, the credential store's secret names and the connector's
//! credential field names stay in the Host. Secrets go in once, with
//! `connection.create`, and never come back out.

use std::sync::Arc;

use axum::{extract::Extension, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use crate::app_state::AppState;
use crate::connectors::credentials::CredentialError;
use crate::connectors::definition::{Auth, Runtime, UserAuth, UserConnector};
use crate::connectors::egress::NetworkClass;
use crate::connectors::runtime::{CreateError, RevokeError, ValidateError};
use crate::connectors::store::{self, Connection, CredentialStore, SnapshotChanges, StoreError};
use crate::connectors::upstream::ToolDefinition;
use crate::protocol::error::{ZJson, ZwiimError};
use crate::web::auth::Principal;

type ZResult<T> = Result<Json<T>, ZwiimError>;

// ── shapes ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionView {
    pub connection_uid: String,
    /// The connector's name, e.g. `Linear`.
    pub name: String,
    /// The user's name for this connection, e.g. `Work`.
    pub label: String,
    /// The server name agents see, e.g. `linear_work`.
    pub slug: String,
    pub url: String,
    /// `validating`, `active`, `needs_reauth`, `error`, `revoking` or `revoked`.
    pub status: String,
    /// Why the connection is `error`, or what its revocation waits for.
    pub error: Option<String>,
    pub last_validated_at: Option<String>,
    pub created_at: String,
    /// Where its key is kept, `keychain` or `file` (unencrypted, on a
    /// headless Host); null when the server takes no key.
    pub credentials: Option<String>,
    /// The origins it may reach and whether its key goes to each.
    pub endpoints: Vec<EndpointView>,
    /// Tools its server listed at the last validation.
    pub tool_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointView {
    pub origin: String,
    pub credentials: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolView {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
    pub output_schema: Option<serde_json::Value>,
    /// The server's own hints (`readOnlyHint`, …), as it sent them.
    pub annotations: Option<serde_json::Value>,
}

impl From<ToolDefinition> for ToolView {
    fn from(tool: ToolDefinition) -> Self {
        Self {
            name: tool.name,
            title: tool.title,
            description: tool.description,
            input_schema: tool.input_schema,
            output_schema: tool.output_schema,
            annotations: tool.annotations,
        }
    }
}

/// What a validation changed in the connection's tools, by name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangesView {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

impl From<SnapshotChanges> for ChangesView {
    fn from(changes: SnapshotChanges) -> Self {
        Self {
            added: changes.added,
            changed: changes.changed,
            removed: changes.removed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProblemView {
    pub code: String,
    pub message: String,
}

impl From<ZwiimError> for ProblemView {
    fn from(e: ZwiimError) -> Self {
        Self {
            code: e.code.into(),
            message: e.message,
        }
    }
}

async fn view(state: &AppState, connection: Connection) -> Result<ConnectionView, ZwiimError> {
    let conn = &state.db.conn;
    let definition = store::definition(conn, &connection.definition_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal("a connection's definition is gone"))?;
    let tool_count = store::tools(conn, &connection.id)
        .await
        .map_err(internal)?
        .len();
    let url = match &definition.runtime {
        Runtime::McpHttp { url, .. } => url.clone(),
        Runtime::McpStdio { .. } => String::new(),
    };
    let credentials = match &definition.auth {
        Auth::Static { .. } => Some(
            match connection.store {
                CredentialStore::Keychain => "keychain",
                CredentialStore::File => "file",
            }
            .to_string(),
        ),
        Auth::None | Auth::Oauth { .. } => None,
    };
    Ok(ConnectionView {
        connection_uid: connection.id,
        name: definition.display.name,
        label: connection.label,
        slug: connection.slug,
        url,
        status: connection.status.as_str().into(),
        error: connection.last_error,
        last_validated_at: connection.last_validated_at.map(|t| t.to_rfc3339()),
        created_at: connection.created_at.to_rfc3339(),
        credentials,
        endpoints: definition
            .security
            .endpoints
            .into_iter()
            .map(|e| EndpointView {
                origin: e.origin,
                credentials: e.credentials,
            })
            .collect(),
        tool_count,
    })
}

async fn view_of(state: &AppState, id: &str) -> Result<ConnectionView, ZwiimError> {
    let connection = store::connection(&state.db.conn, id)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    view(state, connection).await
}

// ── errors ────────────────────────────────────────────────────────────────

fn internal(e: impl std::fmt::Display) -> ZwiimError {
    tracing::warn!(error = %e, "[zwiim] connector error translated to `internal`");
    ZwiimError::internal()
}

fn not_found() -> ZwiimError {
    ZwiimError::not_found("No such connection")
}

fn credential_error(e: &CredentialError) -> ZwiimError {
    match e {
        CredentialError::Missing(_) => ZwiimError::invalid_input("Enter the server's key"),
        CredentialError::Unknown(_) => ZwiimError::invalid_input("This server takes no key"),
        CredentialError::Invalid(_) => {
            ZwiimError::invalid_input("The key can't be sent in a header")
        }
        CredentialError::StoreUnavailable(cause) => {
            tracing::warn!("[connectors] credential store unavailable: {cause}");
            ZwiimError::keychain_unavailable()
        }
    }
}

fn validate_error(e: ValidateError) -> ZwiimError {
    match e {
        ValidateError::NotFound(_) => not_found(),
        ValidateError::Unavailable(status) => ZwiimError::new(
            StatusCode::CONFLICT,
            "connection_unavailable",
            format!("The connection is {status}"),
        ),
        ValidateError::DefinitionChanged => ZwiimError::new(
            StatusCode::CONFLICT,
            "definition_changed",
            "The connector changed since it was connected; connect it again",
        ),
        ValidateError::Credential(e) => credential_error(&e),
        // Already redacted, and recorded on the connection.
        ValidateError::Failed(message) => {
            ZwiimError::new(StatusCode::BAD_GATEWAY, "connection_failed", message)
        }
        ValidateError::Store(e) => internal(e),
    }
}

fn create_error(e: CreateError) -> ZwiimError {
    match e {
        CreateError::Definition(e) | CreateError::Store(StoreError::Definition(e)) => {
            ZwiimError::invalid_input(e.to_string())
        }
        CreateError::Store(StoreError::InvalidLabel) => {
            ZwiimError::invalid_input("A label is 1 to 40 characters")
        }
        CreateError::Credential(e) => credential_error(&e),
        CreateError::Store(e) => internal(e),
    }
}

// ── connection.list ───────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListParams {}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListResult {
    pub connections: Vec<ConnectionView>,
}

/// `connection.list {}` → `{connections}`, oldest first.
pub async fn connection_list(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(_): ZJson<ListParams>,
) -> ZResult<ListResult> {
    let mut connections = Vec::new();
    for connection in store::connections(&state.db.conn).await.map_err(internal)? {
        connections.push(view(&state, connection).await?);
    }
    Ok(Json(ListResult { connections }))
}

// ── connection.create ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthParams {
    None,
    /// `Authorization: Bearer <secret>`.
    Bearer,
    /// The secret as the whole value of `header`.
    Header {
        header: String,
    },
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkParams {
    #[default]
    Public,
    /// A server on the user's own machine or network.
    Local,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateParams {
    pub name: String,
    pub url: String,
    pub auth: AuthParams,
    #[serde(default)]
    pub network: NetworkParams,
    pub label: String,
    #[serde(default)]
    pub secret: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateResult {
    pub connection: ConnectionView,
    /// Why its first validation failed; null when it succeeded. The
    /// connection is kept either way.
    pub validation_error: Option<ProblemView>,
}

/// `connection.create {name, url, auth, network?, label, secret?}` →
/// `{connection, validation_error}`. Only on the Host itself: a key is
/// entered where it is kept.
pub async fn connection_create(
    Extension(state): Extension<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    ZJson(p): ZJson<CreateParams>,
) -> ZResult<CreateResult> {
    if let Some(Extension(Principal::Device(_))) = principal {
        return Err(ZwiimError::permission_denied(
            "Connections are added on the Host's computer",
        ));
    }
    let server = UserConnector {
        name: p.name,
        url: p.url,
        auth: match p.auth {
            AuthParams::None => UserAuth::None,
            AuthParams::Bearer => UserAuth::Bearer,
            AuthParams::Header { header } => UserAuth::Header { header },
        },
        network: match p.network {
            NetworkParams::Public => NetworkClass::Public,
            NetworkParams::Local => NetworkClass::Local,
        },
    };
    let created = state
        .connectors()
        .await
        .create(server, &p.label, p.secret.as_deref())
        .await
        .map_err(create_error)?;
    let validation_error = created.validation.err().map(|e| validate_error(e).into());
    Ok(Json(CreateResult {
        connection: view_of(&state, &created.id).await?,
        validation_error,
    }))
}

// ── connection.validate ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionParams {
    pub connection_uid: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ValidateResult {
    pub connection: ConnectionView,
    pub changes: ChangesView,
}

/// `connection.validate {connection_uid}` → `{connection, changes}`.
/// Connects afresh and lists the server's tools.
pub async fn connection_validate(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<ConnectionParams>,
) -> ZResult<ValidateResult> {
    let changes = state
        .connectors()
        .await
        .validate(&p.connection_uid)
        .await
        .map_err(validate_error)?;
    Ok(Json(ValidateResult {
        connection: view_of(&state, &p.connection_uid).await?,
        changes: changes.into(),
    }))
}

// ── connection.revoke ─────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct ConnectionResult {
    pub connection: ConnectionView,
}

/// `connection.revoke {connection_uid}` → `{connection}`. The connection is
/// unusable at once; one still `revoking` waits for the keychain to delete
/// its key, and revoking it again retries.
pub async fn connection_revoke(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<ConnectionParams>,
) -> ZResult<ConnectionResult> {
    match state.connectors().await.revoke(&p.connection_uid).await {
        Ok(()) | Err(RevokeError::WaitingForStore(_)) => {}
        Err(RevokeError::NotFound(_)) => return Err(not_found()),
        Err(RevokeError::Store(e)) => return Err(internal(e)),
    }
    Ok(Json(ConnectionResult {
        connection: view_of(&state, &p.connection_uid).await?,
    }))
}

// ── connection.tools ──────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct ToolsResult {
    pub tools: Vec<ToolView>,
}

/// `connection.tools {connection_uid}` → `{tools}`: what its server listed at
/// the last validation, by name.
pub async fn connection_tools(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<ConnectionParams>,
) -> ZResult<ToolsResult> {
    store::connection(&state.db.conn, &p.connection_uid)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let tools = store::tools(&state.db.conn, &p.connection_uid)
        .await
        .map_err(internal)?;
    Ok(Json(ToolsResult {
        tools: tools.into_iter().map(ToolView::from).collect(),
    }))
}
