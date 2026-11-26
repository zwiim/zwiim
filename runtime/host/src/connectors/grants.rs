//! Grants (connector design r3 §5.1): which sessions may use a connector,
//! and how each of its tools is treated there. Only rows the user made can
//! allow anything; nothing a server or an agent sends is read here.
//!
//! A grant is one connector for a [`Scope`]. Tools with no row fall to the
//! grant's `default_policy`, or to `new_tools` when first seen after the
//! grant was made; both are `ask` or `deny`. An `allow` row is pinned to the
//! tool's hash when it was given, and stops applying once the tool changes.

use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};

use crate::db::entities::{connector, connector_grant, connector_tool, grant_tool_policy};

/// How one tool is treated under a grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicy {
    Allow,
    Ask,
    Deny,
}

/// What a grant does with a tool it has no row for. Never `allow`: allowing
/// every current tool writes one pinned row each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fallback {
    Ask,
    Deny,
}

/// Which sessions a grant covers. `None` is every workspace, or any agent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    /// A root workspace; its worktrees are covered with it.
    pub workspace_id: Option<String>,
    /// The protocol's agent name.
    pub agent: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Grant {
    pub id: i32,
    pub connector_id: String,
    pub scope: Scope,
    pub default_policy: Fallback,
    pub new_tools: Fallback,
    pub created_at: DateTime<Utc>,
    /// By tool name.
    pub tools: Vec<ToolRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRow {
    pub tool: String,
    pub policy: ToolPolicy,
    /// The hash an `allow` was given for.
    pub schema_hash: Option<String>,
}

impl Grant {
    pub fn row(&self, tool: &str) -> Option<&ToolRow> {
        self.tools.iter().find(|row| row.tool == tool)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GrantError {
    #[error("no connector {0}")]
    ConnectorNotFound(String),
    #[error("no grant {0}")]
    GrantNotFound(i32),
    #[error("the connector has no tool {0:?}")]
    ToolNotFound(String),
    #[error("stored grant data is unreadable: {0}")]
    Corrupt(String),
    #[error("database: {0}")]
    Db(#[from] DbErr),
}

const TOOL_ACTIVE: &str = "active";

fn name_of<T: Serialize>(value: T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        _ => unreachable!("unit variants serialize as strings"),
    }
}

fn parse<T: for<'de> Deserialize<'de>>(name: &str) -> Result<T, GrantError> {
    serde_json::from_value(serde_json::Value::String(name.into()))
        .map_err(|_| GrantError::Corrupt(format!("policy {name:?}")))
}

/// Grants connector `connector_id` to `scope`, or changes the fallbacks
/// of the grant it already has there. Its tool rows are kept.
pub async fn set_grant(
    conn: &DatabaseConnection,
    connector_id: &str,
    scope: Scope,
    default_policy: Fallback,
    new_tools: Fallback,
) -> Result<Grant, GrantError> {
    connector::Entity::find_by_id(connector_id.to_string())
        .one(conn)
        .await?
        .ok_or_else(|| GrantError::ConnectorNotFound(connector_id.into()))?;
    let now = Utc::now();
    let existing = rows(conn, connector_id)
        .await?
        .into_iter()
        .find(|row| row.workspace_id == scope.workspace_id && row.agent == scope.agent);
    let id = match existing {
        Some(row) => {
            connector_grant::ActiveModel {
                id: Set(row.id),
                default_policy: Set(name_of(default_policy)),
                new_tools: Set(name_of(new_tools)),
                updated_at: Set(now),
                ..Default::default()
            }
            .update(conn)
            .await?
            .id
        }
        None => {
            connector_grant::ActiveModel {
                connector_id: Set(connector_id.into()),
                workspace_id: Set(scope.workspace_id),
                agent: Set(scope.agent),
                default_policy: Set(name_of(default_policy)),
                new_tools: Set(name_of(new_tools)),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            }
            .insert(conn)
            .await?
            .id
        }
    };
    grant(conn, id).await?.ok_or(GrantError::GrantNotFound(id))
}

async fn rows(
    conn: &DatabaseConnection,
    connector_id: &str,
) -> Result<Vec<connector_grant::Model>, DbErr> {
    connector_grant::Entity::find()
        .filter(connector_grant::Column::ConnectorId.eq(connector_id))
        .order_by_asc(connector_grant::Column::Id)
        .all(conn)
        .await
}

async fn to_grant(
    conn: &DatabaseConnection,
    row: connector_grant::Model,
) -> Result<Grant, GrantError> {
    let tools = grant_tool_policy::Entity::find()
        .filter(grant_tool_policy::Column::GrantId.eq(row.id))
        .order_by_asc(grant_tool_policy::Column::ToolName)
        .all(conn)
        .await?
        .into_iter()
        .map(|tool| {
            Ok(ToolRow {
                policy: parse(&tool.policy)?,
                tool: tool.tool_name,
                schema_hash: tool.schema_hash,
            })
        })
        .collect::<Result<_, GrantError>>()?;
    Ok(Grant {
        id: row.id,
        connector_id: row.connector_id,
        scope: Scope {
            workspace_id: row.workspace_id,
            agent: row.agent,
        },
        default_policy: parse(&row.default_policy)?,
        new_tools: parse(&row.new_tools)?,
        created_at: row.created_at,
        tools,
    })
}

pub async fn grant(conn: &DatabaseConnection, id: i32) -> Result<Option<Grant>, GrantError> {
    match connector_grant::Entity::find_by_id(id).one(conn).await? {
        Some(row) => Ok(Some(to_grant(conn, row).await?)),
        None => Ok(None),
    }
}

/// Connector `connector_id`'s grants, oldest first.
pub async fn grants(
    conn: &DatabaseConnection,
    connector_id: &str,
) -> Result<Vec<Grant>, GrantError> {
    let mut grants = Vec::new();
    for row in rows(conn, connector_id).await? {
        grants.push(to_grant(conn, row).await?);
    }
    Ok(grants)
}

/// The grant that applies to a session in root workspace `workspace_id`
/// run by `agent`: the most specific of (workspace, agent), (workspace,
/// any), (every, agent) and (every, any). `None` is no grant: deny.
pub async fn grant_for(
    conn: &DatabaseConnection,
    connector_id: &str,
    workspace_id: Option<&str>,
    agent: &str,
) -> Result<Option<Grant>, GrantError> {
    let rows = rows(conn, connector_id).await?;
    let fits = |row: &&connector_grant::Model| {
        row.workspace_id
            .as_deref()
            .is_none_or(|w| Some(w) == workspace_id)
            && row.agent.as_deref().is_none_or(|a| a == agent)
    };
    let best = rows
        .iter()
        .filter(fits)
        .max_by_key(|row| (row.workspace_id.is_some(), row.agent.is_some()));
    match best {
        Some(row) => Ok(Some(to_grant(conn, row.clone()).await?)),
        None => Ok(None),
    }
}

/// Removes grant `id` with its tool rows.
pub async fn remove_grant(conn: &DatabaseConnection, id: i32) -> Result<(), GrantError> {
    let removed = connector_grant::Entity::delete_by_id(id).exec(conn).await?;
    if removed.rows_affected == 0 {
        return Err(GrantError::GrantNotFound(id));
    }
    Ok(())
}

/// The hash of connector `connector_id`'s tool `tool` as it is listed now.
async fn current_hash(
    conn: &DatabaseConnection,
    connector_id: &str,
    tool: &str,
) -> Result<String, GrantError> {
    connector_tool::Entity::find()
        .filter(connector_tool::Column::ConnectorId.eq(connector_id))
        .filter(connector_tool::Column::Name.eq(tool))
        .filter(connector_tool::Column::Status.eq(TOOL_ACTIVE))
        .one(conn)
        .await?
        .map(|row| row.schema_hash)
        .ok_or_else(|| GrantError::ToolNotFound(tool.into()))
}

/// Sets how grant `grant_id` treats `tool`, a tool its connector lists
/// now; `None` removes the row so the grant's fallback applies. An `allow`
/// is pinned to the tool's current hash.
pub async fn set_tool_policy(
    conn: &DatabaseConnection,
    grant_id: i32,
    tool: &str,
    policy: Option<ToolPolicy>,
) -> Result<Grant, GrantError> {
    let current = grant(conn, grant_id)
        .await?
        .ok_or(GrantError::GrantNotFound(grant_id))?;
    let hash = current_hash(conn, &current.connector_id, tool).await?;
    let txn = conn.begin().await?;
    grant_tool_policy::Entity::delete_many()
        .filter(grant_tool_policy::Column::GrantId.eq(grant_id))
        .filter(grant_tool_policy::Column::ToolName.eq(tool))
        .exec(&txn)
        .await?;
    if let Some(policy) = policy {
        grant_tool_policy::ActiveModel {
            grant_id: Set(grant_id),
            tool_name: Set(tool.into()),
            policy: Set(name_of(policy)),
            schema_hash: Set((policy == ToolPolicy::Allow).then_some(hash)),
            updated_at: Set(Utc::now()),
        }
        .insert(&txn)
        .await?;
    }
    txn.commit().await?;
    grant(conn, grant_id)
        .await?
        .ok_or(GrantError::GrantNotFound(grant_id))
}

/// Allows every tool the connector lists now, each pinned to its current
/// hash. A tool the user denied stays denied.
pub async fn allow_all(conn: &DatabaseConnection, grant_id: i32) -> Result<Grant, GrantError> {
    let current = grant(conn, grant_id)
        .await?
        .ok_or(GrantError::GrantNotFound(grant_id))?;
    let tools = connector_tool::Entity::find()
        .filter(connector_tool::Column::ConnectorId.eq(&current.connector_id))
        .filter(connector_tool::Column::Status.eq(TOOL_ACTIVE))
        .all(conn)
        .await?;
    let now = Utc::now();
    let txn = conn.begin().await?;
    for tool in tools {
        if current
            .row(&tool.name)
            .is_some_and(|row| row.policy == ToolPolicy::Deny)
        {
            continue;
        }
        grant_tool_policy::Entity::delete_many()
            .filter(grant_tool_policy::Column::GrantId.eq(grant_id))
            .filter(grant_tool_policy::Column::ToolName.eq(&tool.name))
            .exec(&txn)
            .await?;
        grant_tool_policy::ActiveModel {
            grant_id: Set(grant_id),
            tool_name: Set(tool.name),
            policy: Set(name_of(ToolPolicy::Allow)),
            schema_hash: Set(Some(tool.schema_hash)),
            updated_at: Set(now),
        }
        .insert(&txn)
        .await?;
    }
    txn.commit().await?;
    grant(conn, grant_id)
        .await?
        .ok_or(GrantError::GrantNotFound(grant_id))
}

#[cfg(test)]
mod tests;
