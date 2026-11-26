//! Zwiim-owned: connectors and the endpoints each may reach (connector
//! design r3 §4.4, §5.3, §9). A connector is one account on one connector
//! definition; its credentials live in the OS keychain, never here.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Connectors::Table)
                    .col(
                        ColumnDef::new(Connectors::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Connectors::DefinitionId).string().not_null())
                    // The definition hash the user confirmed.
                    .col(
                        ColumnDef::new(Connectors::DefinitionHash)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Connectors::Label).string().not_null())
                    // The MCP server name agents see, e.g. `linear_work`.
                    .col(
                        ColumnDef::new(Connectors::Slug)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    // `validating`, `active`, `needs_reauth`, `error`,
                    // `revoking` or `revoked`.
                    .col(ColumnDef::new(Connectors::Status).string().not_null())
                    // `keychain`, or `file` for a headless Host's plaintext store.
                    .col(ColumnDef::new(Connectors::Store).string().not_null())
                    // The revocation steps finished so far.
                    .col(ColumnDef::new(Connectors::RevocationJson).text().null())
                    // Redacted.
                    .col(ColumnDef::new(Connectors::LastError).text().null())
                    .col(
                        ColumnDef::new(Connectors::LastValidatedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Connectors::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Connectors::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_connectors_definition_id")
                            .from(Connectors::Table, Connectors::DefinitionId)
                            .to(ConnectorDefinitions::Table, ConnectorDefinitions::Id)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_connectors_definition_label")
                    .table(Connectors::Table)
                    .col(Connectors::DefinitionId)
                    .col(Connectors::Label)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_connectors_status")
                    .table(Connectors::Table)
                    .col(Connectors::Status)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ConnectorEndpoints::Table)
                    .col(
                        ColumnDef::new(ConnectorEndpoints::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConnectorEndpoints::ConnectorId)
                            .string()
                            .not_null(),
                    )
                    // `scheme://host:port`, normalized.
                    .col(
                        ColumnDef::new(ConnectorEndpoints::Origin)
                            .string()
                            .not_null(),
                    )
                    // `mcp`, `oauth_metadata`, `oauth_token`, `authorize`, `api`.
                    .col(
                        ColumnDef::new(ConnectorEndpoints::Purpose)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorEndpoints::Credentials)
                            .boolean()
                            .not_null(),
                    )
                    // `public` or `local`.
                    .col(
                        ColumnDef::new(ConnectorEndpoints::NetworkClass)
                            .string()
                            .not_null(),
                    )
                    // `definition`, `discovered` or `user`.
                    .col(
                        ColumnDef::new(ConnectorEndpoints::Source)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorEndpoints::ConfirmedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_connector_endpoints_connector_id")
                            .from(ConnectorEndpoints::Table, ConnectorEndpoints::ConnectorId)
                            .to(Connectors::Table, Connectors::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_connector_endpoints_origin_purpose")
                    .table(ConnectorEndpoints::Table)
                    .col(ConnectorEndpoints::ConnectorId)
                    .col(ConnectorEndpoints::Origin)
                    .col(ConnectorEndpoints::Purpose)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ConnectorEndpoints::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Connectors::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Connectors {
    Table,
    Id,
    DefinitionId,
    DefinitionHash,
    Label,
    Slug,
    Status,
    Store,
    RevocationJson,
    LastError,
    LastValidatedAt,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ConnectorEndpoints {
    Table,
    Id,
    ConnectorId,
    Origin,
    Purpose,
    Credentials,
    NetworkClass,
    Source,
    ConfirmedAt,
}

#[derive(DeriveIden)]
enum ConnectorDefinitions {
    Table,
    Id,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::sea_orm::{ConnectionTrait, Database, DbBackend, Statement};

    const AT: &str = "'2026-11-25T00:00:00Z'";

    fn connector(id: &str, label: &str, slug: &str) -> String {
        format!(
            "INSERT INTO connectors (id, definition_id, definition_hash, label, slug, status, \
             store, created_at, updated_at) VALUES ('{id}', 'linear', 'sha256:0', '{label}', \
             '{slug}', 'active', 'keychain', {AT}, {AT})"
        )
    }

    fn endpoint(connector: &str, origin: &str) -> String {
        format!(
            "INSERT INTO connector_endpoints (connector_id, origin, purpose, credentials, \
             network_class, source, confirmed_at) VALUES ('{connector}', '{origin}', 'mcp', 1, \
             'public', 'definition', {AT})"
        )
    }

    #[tokio::test]
    async fn connectors_keep_their_definition_and_endpoints_follow_their_connector() {
        let conn = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        conn.execute(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA foreign_keys=ON;".to_owned(),
        ))
        .await
        .unwrap();
        let manager = SchemaManager::new(&conn);
        super::super::m20261125_000001_connector_definitions::Migration
            .up(&manager)
            .await
            .unwrap();
        Migration.up(&manager).await.expect("run migration up");
        conn.execute_unprepared(&format!(
            "INSERT INTO connector_definitions (id, zcd_version, version, trust, runtime_kind, \
             definition_json, hash, created_at, updated_at) VALUES ('linear', 1, '1', 'user', \
             'mcp_http', '{{}}', 'sha256:0', {AT}, {AT})"
        ))
        .await
        .unwrap();

        conn.execute_unprepared(&connector("c1", "Work", "linear_work"))
            .await
            .expect("first connector");
        // Same label on the same definition, or the same slug: refused.
        assert!(conn
            .execute_unprepared(&connector("c2", "Work", "linear_work_2"))
            .await
            .is_err());
        assert!(conn
            .execute_unprepared(&connector("c2", "Home", "linear_work"))
            .await
            .is_err());

        conn.execute_unprepared(&endpoint("c1", "https://mcp.linear.app:443"))
            .await
            .expect("endpoint");
        assert!(conn
            .execute_unprepared(&endpoint("c1", "https://mcp.linear.app:443"))
            .await
            .is_err());
        assert!(conn
            .execute_unprepared(&endpoint("missing", "https://mcp.linear.app:443"))
            .await
            .is_err());
        // A definition in use can't go; deleting the connector takes its
        // endpoints with it.
        assert!(conn
            .execute_unprepared("DELETE FROM connector_definitions WHERE id = 'linear'")
            .await
            .is_err());
        conn.execute_unprepared("DELETE FROM connectors WHERE id = 'c1'")
            .await
            .unwrap();
        let left = conn
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT COUNT(*) AS n FROM connector_endpoints".to_owned(),
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "n")
            .unwrap();
        assert_eq!(left, 0);

        Migration.down(&manager).await.expect("run migration down");
        assert!(!manager.has_table("connectors").await.unwrap());
        assert!(!manager.has_table("connector_endpoints").await.unwrap());
    }
}
