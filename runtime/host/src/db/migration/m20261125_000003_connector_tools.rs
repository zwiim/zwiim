//! Zwiim-owned: each connector's tool snapshot (connector design r3 §9).
//! The tools a service last listed, with the hash Zwiim computed for each,
//! so grants can be pinned to a hash and a changed tool can be noticed.
//! A tool the service stops listing is kept as `removed`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ConnectorTools::Table)
                    .col(
                        ColumnDef::new(ConnectorTools::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConnectorTools::ConnectorId)
                            .string()
                            .not_null(),
                    )
                    // The service's own name for the tool.
                    .col(ColumnDef::new(ConnectorTools::Name).string().not_null())
                    .col(ColumnDef::new(ConnectorTools::Title).string().null())
                    .col(ColumnDef::new(ConnectorTools::Description).text().null())
                    .col(
                        ColumnDef::new(ConnectorTools::InputSchemaJson)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorTools::OutputSchemaJson)
                            .text()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorTools::AnnotationsJson)
                            .text()
                            .null(),
                    )
                    // `sha256:…` over every field.
                    .col(
                        ColumnDef::new(ConnectorTools::SchemaHash)
                            .string()
                            .not_null(),
                    )
                    // `active` or `removed`.
                    .col(ColumnDef::new(ConnectorTools::Status).string().not_null())
                    .col(
                        ColumnDef::new(ConnectorTools::FirstSeenAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorTools::LastSeenAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    // When the hash last changed; null if never.
                    .col(
                        ColumnDef::new(ConnectorTools::ChangedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_connector_tools_connector_id")
                            .from(ConnectorTools::Table, ConnectorTools::ConnectorId)
                            .to(Connectors::Table, Connectors::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_connector_tools_name")
                    .table(ConnectorTools::Table)
                    .col(ConnectorTools::ConnectorId)
                    .col(ConnectorTools::Name)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ConnectorTools::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ConnectorTools {
    Table,
    Id,
    ConnectorId,
    Name,
    Title,
    Description,
    InputSchemaJson,
    OutputSchemaJson,
    AnnotationsJson,
    SchemaHash,
    Status,
    FirstSeenAt,
    LastSeenAt,
    ChangedAt,
}

#[derive(DeriveIden)]
enum Connectors {
    Table,
    Id,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::sea_orm::Database;

    #[tokio::test]
    async fn one_row_per_tool_name_per_connector_and_down_drops_them() {
        let conn = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        let manager = SchemaManager::new(&conn);
        // The foreign key's target, minimal.
        conn.execute_unprepared(
            "CREATE TABLE connectors (id TEXT PRIMARY KEY); \
             INSERT INTO connectors (id) VALUES ('c1'), ('c2')",
        )
        .await
        .unwrap();
        Migration.up(&manager).await.expect("run migration up");

        let insert = |connector: &str| {
            format!(
                "INSERT INTO connector_tools (connector_id, name, input_schema_json, \
                 schema_hash, status, first_seen_at, last_seen_at) VALUES ('{connector}', \
                 'echo', '{{}}', 'sha256:0', 'active', '2026-11-25T00:00:00Z', \
                 '2026-11-25T00:00:00Z')"
            )
        };
        conn.execute_unprepared(&insert("c1"))
            .await
            .expect("first tool");
        assert!(conn.execute_unprepared(&insert("c1")).await.is_err());
        conn.execute_unprepared(&insert("c2"))
            .await
            .expect("same name, another connector");

        Migration.down(&manager).await.expect("run migration down");
        assert!(!manager.has_table("connector_tools").await.unwrap());
    }
}
