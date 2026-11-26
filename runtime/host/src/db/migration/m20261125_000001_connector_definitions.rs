//! Zwiim-owned: connector definitions (connector design r3 §4.5, §9). One row
//! per Zwiim Connector Definition, the stored record a connector is made
//! from. The record itself is `definition_json`; the other columns are kept
//! next to it for queries and for spotting a changed definition.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ConnectorDefinitions::Table)
                    .col(
                        ColumnDef::new(ConnectorDefinitions::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConnectorDefinitions::ZcdVersion)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorDefinitions::Version)
                            .string()
                            .not_null(),
                    )
                    // `catalog_verified`, `registry` or `user`.
                    .col(
                        ColumnDef::new(ConnectorDefinitions::Trust)
                            .string()
                            .not_null(),
                    )
                    // `mcp_http` for now.
                    .col(
                        ColumnDef::new(ConnectorDefinitions::RuntimeKind)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorDefinitions::DefinitionJson)
                            .text()
                            .not_null(),
                    )
                    // `sha256:…` over runtime, auth and security.
                    .col(
                        ColumnDef::new(ConnectorDefinitions::Hash)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorDefinitions::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorDefinitions::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ConnectorDefinitions::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ConnectorDefinitions {
    Table,
    Id,
    ZcdVersion,
    Version,
    Trust,
    RuntimeKind,
    DefinitionJson,
    Hash,
    CreatedAt,
    UpdatedAt,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::sea_orm::Database;

    #[tokio::test]
    async fn one_row_per_definition_id_and_down_drops_them() {
        let conn = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        let manager = SchemaManager::new(&conn);
        Migration.up(&manager).await.expect("run migration up");

        let insert = "INSERT INTO connector_definitions (id, zcd_version, version, trust, \
            runtime_kind, definition_json, hash, created_at, updated_at) VALUES ('linear', 1, \
            '1', 'user', 'mcp_http', '{}', 'sha256:0', '2026-11-25T00:00:00Z', \
            '2026-11-25T00:00:00Z')";
        conn.execute_unprepared(insert).await.expect("first row");
        assert!(conn.execute_unprepared(insert).await.is_err());

        Migration.down(&manager).await.expect("run migration down");
        assert!(!manager
            .has_table("connector_definitions")
            .await
            .expect("has table"));
    }
}
