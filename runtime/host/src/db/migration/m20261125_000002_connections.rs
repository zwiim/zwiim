//! Zwiim-owned: connections and the endpoints each may reach (connector
//! design r3 §4.4, §5.3, §9). A connection is one account on one connector
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
                    .table(Connections::Table)
                    .col(
                        ColumnDef::new(Connections::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Connections::DefinitionId)
                            .string()
                            .not_null(),
                    )
                    // The definition hash the user confirmed.
                    .col(
                        ColumnDef::new(Connections::DefinitionHash)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Connections::Label).string().not_null())
                    // The MCP server name agents see, e.g. `linear_work`.
                    .col(
                        ColumnDef::new(Connections::Slug)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    // `validating`, `active`, `needs_reauth`, `error`,
                    // `revoking` or `revoked`.
                    .col(ColumnDef::new(Connections::Status).string().not_null())
                    // `keychain`, or `file` for a headless Host's plaintext store.
                    .col(ColumnDef::new(Connections::Store).string().not_null())
                    // The revocation steps finished so far.
                    .col(ColumnDef::new(Connections::RevocationJson).text().null())
                    // Redacted.
                    .col(ColumnDef::new(Connections::LastError).text().null())
                    .col(
                        ColumnDef::new(Connections::LastValidatedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Connections::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Connections::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_connections_definition_id")
                            .from(Connections::Table, Connections::DefinitionId)
                            .to(ConnectorDefinitions::Table, ConnectorDefinitions::Id)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_connections_definition_label")
                    .table(Connections::Table)
                    .col(Connections::DefinitionId)
                    .col(Connections::Label)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_connections_status")
                    .table(Connections::Table)
                    .col(Connections::Status)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ConnectionEndpoints::Table)
                    .col(
                        ColumnDef::new(ConnectionEndpoints::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConnectionEndpoints::ConnectionId)
                            .string()
                            .not_null(),
                    )
                    // `scheme://host:port`, normalized.
                    .col(
                        ColumnDef::new(ConnectionEndpoints::Origin)
                            .string()
                            .not_null(),
                    )
                    // `mcp`, `oauth_metadata`, `oauth_token`, `authorize`, `api`.
                    .col(
                        ColumnDef::new(ConnectionEndpoints::Purpose)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectionEndpoints::Credentials)
                            .boolean()
                            .not_null(),
                    )
                    // `public` or `local`.
                    .col(
                        ColumnDef::new(ConnectionEndpoints::NetworkClass)
                            .string()
                            .not_null(),
                    )
                    // `definition`, `discovered` or `user`.
                    .col(
                        ColumnDef::new(ConnectionEndpoints::Source)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectionEndpoints::ConfirmedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_connection_endpoints_connection_id")
                            .from(
                                ConnectionEndpoints::Table,
                                ConnectionEndpoints::ConnectionId,
                            )
                            .to(Connections::Table, Connections::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_connection_endpoints_origin_purpose")
                    .table(ConnectionEndpoints::Table)
                    .col(ConnectionEndpoints::ConnectionId)
                    .col(ConnectionEndpoints::Origin)
                    .col(ConnectionEndpoints::Purpose)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ConnectionEndpoints::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Connections::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Connections {
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
enum ConnectionEndpoints {
    Table,
    Id,
    ConnectionId,
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

    fn connection(id: &str, label: &str, slug: &str) -> String {
        format!(
            "INSERT INTO connections (id, definition_id, definition_hash, label, slug, status, \
             store, created_at, updated_at) VALUES ('{id}', 'linear', 'sha256:0', '{label}', \
             '{slug}', 'active', 'keychain', {AT}, {AT})"
        )
    }

    fn endpoint(connection: &str, origin: &str) -> String {
        format!(
            "INSERT INTO connection_endpoints (connection_id, origin, purpose, credentials, \
             network_class, source, confirmed_at) VALUES ('{connection}', '{origin}', 'mcp', 1, \
             'public', 'definition', {AT})"
        )
    }

    #[tokio::test]
    async fn connections_keep_their_definition_and_endpoints_follow_their_connection() {
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

        conn.execute_unprepared(&connection("c1", "Work", "linear_work"))
            .await
            .expect("first connection");
        // Same label on the same definition, or the same slug: refused.
        assert!(conn
            .execute_unprepared(&connection("c2", "Work", "linear_work_2"))
            .await
            .is_err());
        assert!(conn
            .execute_unprepared(&connection("c2", "Home", "linear_work"))
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
        // A definition in use can't go; deleting the connection takes its
        // endpoints with it.
        assert!(conn
            .execute_unprepared("DELETE FROM connector_definitions WHERE id = 'linear'")
            .await
            .is_err());
        conn.execute_unprepared("DELETE FROM connections WHERE id = 'c1'")
            .await
            .unwrap();
        let left = conn
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT COUNT(*) AS n FROM connection_endpoints".to_owned(),
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "n")
            .unwrap();
        assert_eq!(left, 0);

        Migration.down(&manager).await.expect("run migration down");
        assert!(!manager.has_table("connections").await.unwrap());
        assert!(!manager.has_table("connection_endpoints").await.unwrap());
    }
}
