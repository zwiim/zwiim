//! Connector records in the Host database: definitions, connections, the
//! endpoints each connection may reach and each connection's tool snapshot. Credentials are never here; they
//! live in the OS keychain.

use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use url::Url;

use super::definition::{ConnectorDefinition, DefinitionError, Purpose, Runtime, Trust};
use super::egress::{EndpointRule, NetworkClass};
use super::upstream::ToolDefinition;
use crate::db::entities::{connection, connection_endpoint, connection_tool, connector_definition};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Definition(#[from] DefinitionError),
    #[error("no connector definition {0}")]
    DefinitionNotFound(String),
    #[error("no connection {0}")]
    ConnectionNotFound(String),
    #[error("a label is 1-40 characters")]
    InvalidLabel,
    #[error("this connector already has a connection labelled {0:?}")]
    LabelTaken(String),
    #[error("a {from} connection can't become {to}")]
    InvalidTransition {
        from: ConnectionStatus,
        to: ConnectionStatus,
    },
    #[error("stored connection data is unreadable: {0}")]
    Corrupt(String),
    #[error("database: {0}")]
    Db(#[from] DbErr),
}

/// Stores `definition`, replacing an earlier one with the same id. Only a
/// definition that passes [`ConnectorDefinition::validate`] is written.
pub async fn save_definition(
    conn: &DatabaseConnection,
    definition: &ConnectorDefinition,
) -> Result<(), StoreError> {
    definition.validate()?;
    let json = serde_json::to_string(definition)
        .map_err(|e| DefinitionError::Invalid(format!("can't encode the definition: {e}")))?;
    let now = Utc::now();
    let existing = connector_definition::Entity::find_by_id(definition.id.clone())
        .one(conn)
        .await?;
    let mut row = connector_definition::ActiveModel {
        id: Set(definition.id.clone()),
        zcd_version: Set(definition.zcd_version as i32),
        version: Set(definition.version.clone()),
        trust: Set(trust(definition.trust).into()),
        runtime_kind: Set(runtime_kind(&definition.runtime).into()),
        definition_json: Set(json),
        hash: Set(definition.hash()),
        created_at: Set(now),
        updated_at: Set(now),
    };
    match existing {
        Some(existing) => {
            row.created_at = Set(existing.created_at);
            row.update(conn).await?;
        }
        None => {
            row.insert(conn).await?;
        }
    }
    Ok(())
}

/// How stored definition `id` authenticates, read without validating the
/// rest, so a connection whose definition no longer passes the rules can
/// still have its secrets deleted.
pub async fn definition_auth(
    conn: &DatabaseConnection,
    id: &str,
) -> Result<Option<super::definition::Auth>, StoreError> {
    let Some(row) = connector_definition::Entity::find_by_id(id.to_string())
        .one(conn)
        .await?
    else {
        return Ok(None);
    };
    let record: serde_json::Value = serde_json::from_str(&row.definition_json)
        .map_err(|e| StoreError::Corrupt(format!("definition: {e}")))?;
    serde_json::from_value(record["auth"].clone())
        .map(Some)
        .map_err(|e| StoreError::Corrupt(format!("definition auth: {e}")))
}

/// The stored definition `id`, validated again on the way out: a record
/// written before a rule existed is refused, not used.
pub async fn definition(
    conn: &DatabaseConnection,
    id: &str,
) -> Result<Option<ConnectorDefinition>, StoreError> {
    let Some(row) = connector_definition::Entity::find_by_id(id.to_string())
        .one(conn)
        .await?
    else {
        return Ok(None);
    };
    Ok(Some(ConnectorDefinition::parse(&row.definition_json)?))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    /// Created; the first listing hasn't succeeded yet.
    Validating,
    Active,
    NeedsReauth,
    Error,
    /// Being revoked: unusable from the first step on (design r3 §4.4).
    Revoking,
    Revoked,
}

impl ConnectionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Validating => "validating",
            Self::Active => "active",
            Self::NeedsReauth => "needs_reauth",
            Self::Error => "error",
            Self::Revoking => "revoking",
            Self::Revoked => "revoked",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(value.into())).ok()
    }

    /// Revocation only moves forward: once it starts, the connection never
    /// becomes usable again.
    fn can_become(self, next: Self) -> bool {
        match self {
            Self::Revoking => next == Self::Revoked,
            Self::Revoked => false,
            _ => true,
        }
    }
}

impl std::fmt::Display for ConnectionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a connection's credentials are kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStore {
    Keychain,
    /// A headless Host's plaintext `tokens.json`; shown as unencrypted.
    File,
}

impl CredentialStore {
    fn as_str(self) -> &'static str {
        match self {
            Self::Keychain => "keychain",
            Self::File => "file",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Connection {
    pub id: String,
    pub definition_id: String,
    /// The definition hash the user confirmed when connecting.
    pub definition_hash: String,
    pub label: String,
    /// The MCP server name agents see, e.g. `linear_work`.
    pub slug: String,
    pub status: ConnectionStatus,
    pub store: CredentialStore,
    pub last_error: Option<String>,
    pub last_validated_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// The revocation steps finished so far, in order.
    pub revocation: Vec<RevocationStep>,
}

/// One step of revoking a connection (design r3 §4.4). Each is recorded once
/// done, so a revocation interrupted by a Host exit resumes after the last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevocationStep {
    /// Marked `revoking`: refused from here on.
    Blocked,
    /// Its live upstreams are closed.
    Stopped,
    /// The service was told, where it has a way to be (OAuth, later).
    UpstreamRevoked,
    /// Its secrets are gone from the credential store.
    SecretsDeleted,
}

#[derive(Serialize, Deserialize)]
struct Revocation {
    done: Vec<RevocationStep>,
}

const MAX_LABEL: usize = 40;
const MAX_SLUG: usize = 32;

/// A new connection to the stored definition `definition_id`, in
/// `validating`, with the definition's endpoints as its own. The definition
/// is confirmed as it is now: its hash is recorded with the connection.
pub async fn create_connection(
    conn: &DatabaseConnection,
    definition_id: &str,
    label: &str,
    store: CredentialStore,
) -> Result<Connection, StoreError> {
    let definition = definition(conn, definition_id)
        .await?
        .ok_or_else(|| StoreError::DefinitionNotFound(definition_id.into()))?;
    let label = label.trim();
    if label.is_empty() || label.chars().count() > MAX_LABEL {
        return Err(StoreError::InvalidLabel);
    }
    let txn = conn.begin().await?;
    let taken = connection::Entity::find()
        .filter(connection::Column::DefinitionId.eq(definition_id))
        .filter(connection::Column::Label.eq(label))
        .one(&txn)
        .await?;
    if taken.is_some() {
        return Err(StoreError::LabelTaken(label.into()));
    }
    let base = slug(&format!("{} {label}", definition.display.name));
    let mut slug = base.clone();
    for n in 2.. {
        let used = connection::Entity::find()
            .filter(connection::Column::Slug.eq(&slug))
            .one(&txn)
            .await?;
        if used.is_none() {
            break;
        }
        let suffix = format!("_{n}");
        slug = format!("{}{suffix}", truncate(&base, MAX_SLUG - suffix.len()));
    }
    let now = Utc::now();
    let row = connection::ActiveModel {
        id: Set(uuid::Uuid::new_v4().simple().to_string()),
        definition_id: Set(definition_id.into()),
        definition_hash: Set(definition.hash()),
        label: Set(label.into()),
        slug: Set(slug),
        status: Set(ConnectionStatus::Validating.as_str().into()),
        store: Set(store.as_str().into()),
        revocation_json: Set(None),
        last_error: Set(None),
        last_validated_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&txn)
    .await?;
    for endpoint in &definition.security.endpoints {
        let origin = Url::parse(&endpoint.origin)
            .map(|url| url.origin().ascii_serialization())
            .map_err(|_| StoreError::Corrupt(format!("endpoint {:?}", endpoint.origin)))?;
        connection_endpoint::ActiveModel {
            connection_id: Set(row.id.clone()),
            origin: Set(origin),
            purpose: Set(name_of(&endpoint.purpose)?),
            credentials: Set(endpoint.credentials),
            network_class: Set(name_of(&definition.security.network_class)?),
            source: Set("definition".into()),
            confirmed_at: Set(now),
            ..Default::default()
        }
        .insert(&txn)
        .await?;
    }
    txn.commit().await?;
    to_connection(row)
}

pub async fn connection(
    conn: &DatabaseConnection,
    id: &str,
) -> Result<Option<Connection>, StoreError> {
    connection::Entity::find_by_id(id.to_string())
        .one(conn)
        .await?
        .map(to_connection)
        .transpose()
}

/// Every connection, oldest first.
pub async fn connections(conn: &DatabaseConnection) -> Result<Vec<Connection>, StoreError> {
    connection::Entity::find()
        .order_by_asc(connection::Column::CreatedAt)
        .order_by_asc(connection::Column::Id)
        .all(conn)
        .await?
        .into_iter()
        .map(to_connection)
        .collect()
}

/// Deletes definition `definition_id` with its connections, their endpoints
/// and tool snapshots. Only for undoing a connection that failed while it
/// was being created: nothing has used it yet.
pub async fn discard_definition(
    conn: &DatabaseConnection,
    definition_id: &str,
) -> Result<(), StoreError> {
    let txn = conn.begin().await?;
    connection::Entity::delete_many()
        .filter(connection::Column::DefinitionId.eq(definition_id))
        .exec(&txn)
        .await?;
    connector_definition::Entity::delete_by_id(definition_id.to_string())
        .exec(&txn)
        .await?;
    txn.commit().await?;
    Ok(())
}

/// Moves connection `id` to `status`. `last_error` must already be
/// redacted; it is cleared when the connection becomes active.
pub async fn set_status(
    conn: &DatabaseConnection,
    id: &str,
    status: ConnectionStatus,
    last_error: Option<&str>,
) -> Result<Connection, StoreError> {
    let current = connection(conn, id)
        .await?
        .ok_or_else(|| StoreError::ConnectionNotFound(id.into()))?;
    if !current.status.can_become(status) {
        return Err(StoreError::InvalidTransition {
            from: current.status,
            to: status,
        });
    }
    let now = Utc::now();
    let mut row = connection::ActiveModel {
        id: Set(id.into()),
        status: Set(status.as_str().into()),
        last_error: Set(last_error.map(str::to_string)),
        updated_at: Set(now),
        ..Default::default()
    };
    if status == ConnectionStatus::Active {
        row.last_validated_at = Set(Some(now));
    }
    to_connection(row.update(conn).await?)
}

/// The egress rules for connection `id`'s endpoints.
pub async fn endpoint_rules(
    conn: &DatabaseConnection,
    id: &str,
) -> Result<Vec<EndpointRule>, StoreError> {
    let rows = connection_endpoint::Entity::find()
        .filter(connection_endpoint::Column::ConnectionId.eq(id))
        .order_by_asc(connection_endpoint::Column::Id)
        .all(conn)
        .await?;
    rows.into_iter()
        .map(|row| {
            let class: NetworkClass = parse_name(&row.network_class)?;
            let _: Purpose = parse_name(&row.purpose)?;
            EndpointRule::new(&row.origin, row.credentials, class)
                .map_err(|e| StoreError::Corrupt(e.to_string()))
        })
        .collect()
}

/// What a new listing changed in a connection's snapshot, by tool name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotChanges {
    /// New, or listed again after being removed.
    pub added: Vec<String>,
    /// Listed with a different hash.
    pub changed: Vec<String>,
    /// No longer listed.
    pub removed: Vec<String>,
}

impl SnapshotChanges {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }
}

const TOOL_ACTIVE: &str = "active";
const TOOL_REMOVED: &str = "removed";

/// Records `tools` as connection `id`'s current listing, in one
/// transaction. A tool no longer listed stays as `removed`, so a grant
/// pinned to it isn't silently lost.
pub async fn save_tool_snapshot(
    conn: &DatabaseConnection,
    id: &str,
    tools: &[ToolDefinition],
) -> Result<SnapshotChanges, StoreError> {
    let txn = conn.begin().await?;
    let existing = connection_tool::Entity::find()
        .filter(connection_tool::Column::ConnectionId.eq(id))
        .all(&txn)
        .await?;
    let now = Utc::now();
    let mut changes = SnapshotChanges::default();
    for tool in tools {
        let hash = tool.hash();
        let mut row = connection_tool::ActiveModel {
            connection_id: Set(id.into()),
            name: Set(tool.name.clone()),
            title: Set(tool.title.clone()),
            description: Set(tool.description.clone()),
            input_schema_json: Set(tool.input_schema.to_string()),
            output_schema_json: Set(tool.output_schema.as_ref().map(|v| v.to_string())),
            annotations_json: Set(tool.annotations.as_ref().map(|v| v.to_string())),
            schema_hash: Set(hash.clone()),
            status: Set(TOOL_ACTIVE.into()),
            last_seen_at: Set(now),
            ..Default::default()
        };
        match existing.iter().find(|r| r.name == tool.name) {
            None => {
                row.first_seen_at = Set(now);
                row.insert(&txn).await?;
                changes.added.push(tool.name.clone());
            }
            Some(old) => {
                if old.status == TOOL_REMOVED {
                    changes.added.push(tool.name.clone());
                }
                if old.schema_hash != hash {
                    row.changed_at = Set(Some(now));
                    if old.status != TOOL_REMOVED {
                        changes.changed.push(tool.name.clone());
                    }
                }
                row.id = Set(old.id);
                row.update(&txn).await?;
            }
        }
    }
    for old in &existing {
        if old.status != TOOL_REMOVED && !tools.iter().any(|t| t.name == old.name) {
            connection_tool::ActiveModel {
                id: Set(old.id),
                status: Set(TOOL_REMOVED.into()),
                ..Default::default()
            }
            .update(&txn)
            .await?;
            changes.removed.push(old.name.clone());
        }
    }
    txn.commit().await?;
    Ok(changes)
}

/// Connection `id`'s currently listed tools, by name.
pub async fn tools(conn: &DatabaseConnection, id: &str) -> Result<Vec<ToolDefinition>, StoreError> {
    let rows = connection_tool::Entity::find()
        .filter(connection_tool::Column::ConnectionId.eq(id))
        .filter(connection_tool::Column::Status.eq(TOOL_ACTIVE))
        .order_by_asc(connection_tool::Column::Name)
        .all(conn)
        .await?;
    let json = |text: &str| {
        serde_json::from_str(text).map_err(|e| StoreError::Corrupt(format!("tool schema: {e}")))
    };
    rows.into_iter()
        .map(|row| {
            Ok(ToolDefinition {
                input_schema: json(&row.input_schema_json)?,
                output_schema: row.output_schema_json.as_deref().map(json).transpose()?,
                annotations: row.annotations_json.as_deref().map(json).transpose()?,
                name: row.name,
                title: row.title,
                description: row.description,
            })
        })
        .collect()
}

/// Records the revocation steps connection `id` has finished, and why the
/// next one waits, if it does. The status stays as it is.
pub async fn record_revocation(
    conn: &DatabaseConnection,
    id: &str,
    done: &[RevocationStep],
    waiting: Option<&str>,
) -> Result<(), StoreError> {
    let json = serde_json::to_string(&Revocation {
        done: done.to_vec(),
    })
    .map_err(|e| StoreError::Corrupt(e.to_string()))?;
    connection::ActiveModel {
        id: Set(id.into()),
        revocation_json: Set(Some(json)),
        last_error: Set(waiting.map(str::to_string)),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(conn)
    .await?;
    Ok(())
}

fn to_connection(row: connection::Model) -> Result<Connection, StoreError> {
    let status = ConnectionStatus::parse(&row.status)
        .ok_or_else(|| StoreError::Corrupt(format!("status {:?}", row.status)))?;
    let revocation = match row.revocation_json.as_deref() {
        None => Vec::new(),
        Some(json) => {
            serde_json::from_str::<Revocation>(json)
                .map_err(|e| StoreError::Corrupt(format!("revocation: {e}")))?
                .done
        }
    };
    Ok(Connection {
        revocation,
        status,
        store: parse_name(&row.store)?,
        id: row.id,
        definition_id: row.definition_id,
        definition_hash: row.definition_hash,
        label: row.label,
        slug: row.slug,
        last_error: row.last_error,
        last_validated_at: row.last_validated_at,
        created_at: row.created_at,
    })
}

/// The serde name of a unit enum value (`Purpose::OauthToken` → `oauth_token`).
fn name_of<T: Serialize>(value: &T) -> Result<String, StoreError> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => Ok(name),
        _ => Err(StoreError::Corrupt("not a named value".into())),
    }
}

fn parse_name<T: serde::de::DeserializeOwned>(name: &str) -> Result<T, StoreError> {
    serde_json::from_value(serde_json::Value::String(name.into()))
        .map_err(|_| StoreError::Corrupt(format!("value {name:?}")))
}

/// `Linear Work!` → `linear_work`: lowercase ASCII letters, digits and
/// single underscores, at most 32 characters, cut at a word, never empty.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    let full = out.trim_end_matches('_');
    let mut out = truncate(full, MAX_SLUG);
    // Cut at a word, not inside one, when there is a word to cut at.
    if out.len() < full.len() && !full[out.len()..].starts_with('_') {
        if let Some(end) = out.rfind('_') {
            out.truncate(end);
        }
    }
    if out.is_empty() {
        "connector".into()
    } else {
        out
    }
}

fn truncate(text: &str, max: usize) -> String {
    text.chars()
        .take(max)
        .collect::<String>()
        .trim_end_matches('_')
        .to_string()
}

fn trust(trust: Trust) -> &'static str {
    match trust {
        Trust::CatalogVerified => "catalog_verified",
        Trust::Registry => "registry",
        Trust::User => "user",
    }
}

fn runtime_kind(runtime: &Runtime) -> &'static str {
    match runtime {
        Runtime::McpHttp { .. } => "mcp_http",
        Runtime::McpStdio { .. } => "mcp_stdio",
    }
}

#[cfg(test)]
mod tests;
