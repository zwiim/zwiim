//! Zwiim-owned: each connection's tool snapshot (connector design r3 §9).
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
                    .table(ConnectionTools::Table)
                    .col(
                        ColumnDef::new(ConnectionTools::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConnectionTools::ConnectionId)
                            .string()
                            .not_null(),
                    )
                    // The service's own name for the tool.
                    .col(ColumnDef::new(ConnectionTools::Name).string().not_null())
                    .col(ColumnDef::new(ConnectionTools::Title).string().null())
                    .col(ColumnDef::new(ConnectionTools::Description).text().null())
                    .col(
                        ColumnDef::new(ConnectionTools::InputSchemaJson)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectionTools::OutputSchemaJson)
                            .text()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConnectionTools::AnnotationsJson)
                            .text()
                            .null(),
                    )
                    // `sha256:…` over every field.
                    .col(
                        ColumnDef::new(ConnectionTools::SchemaHash)
                            .string()
                            .not_null(),
                    )
                    // `active` or `removed`.
                    .col(ColumnDef::new(ConnectionTools::Status).string().not_null())
                    .col(
                        ColumnDef::new(ConnectionTools::FirstSeenAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectionTools::LastSeenAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    // When the hash last changed; null if never.
                    .col(
                        ColumnDef::new(ConnectionTools::ChangedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_connection_tools_connection_id")
                            .from(ConnectionTools::Table, ConnectionTools::ConnectionId)
                            .to(Connections::Table, Connections::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_connection_tools_name")
                    .table(ConnectionTools::Table)
                    .col(ConnectionTools::ConnectionId)
                    .col(ConnectionTools::Name)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ConnectionTools::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ConnectionTools {
    Table,
    Id,
    ConnectionId,
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
enum Connections {
    Table,
    Id,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::sea_orm::Database;

    #[tokio::test]
    async fn one_row_per_tool_name_per_connection_and_down_drops_them() {
        let conn = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        let manager = SchemaManager::new(&conn);
        // The foreign key's target, minimal.
        conn.execute_unprepared(
            "CREATE TABLE connections (id TEXT PRIMARY KEY); \
             INSERT INTO connections (id) VALUES ('c1'), ('c2')",
        )
        .await
        .unwrap();
        Migration.up(&manager).await.expect("run migration up");

        let insert = |connection: &str| {
            format!(
                "INSERT INTO connection_tools (connection_id, name, input_schema_json, \
                 schema_hash, status, first_seen_at, last_seen_at) VALUES ('{connection}', \
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
            .expect("same name, another connection");

        Migration.down(&manager).await.expect("run migration down");
        assert!(!manager.has_table("connection_tools").await.unwrap());
    }
}
