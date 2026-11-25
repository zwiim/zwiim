//! Connector records in the Host database. Definitions only for now;
//! connections, endpoints and the tool snapshot follow.

use chrono::Utc;
use sea_orm::{ActiveModelTrait, DatabaseConnection, DbErr, EntityTrait, Set};

use super::definition::{ConnectorDefinition, DefinitionError, Runtime, Trust};
use crate::db::entities::connector_definition;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Definition(#[from] DefinitionError),
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
