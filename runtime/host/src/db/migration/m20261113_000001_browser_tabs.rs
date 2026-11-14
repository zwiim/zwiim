//! Zwiim-owned: the Host browser's tabs, so they come back after an engine
//! restart, a Host restart or a discard (browser design 6.4). Page content
//! is never stored; a tab is its owner, address and title.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(BrowserTabs::Table)
                    .col(
                        ColumnDef::new(BrowserTabs::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    // `session` or `personal`.
                    .col(ColumnDef::new(BrowserTabs::OwnerKind).string().not_null())
                    // The public session id; null for personal tabs.
                    .col(ColumnDef::new(BrowserTabs::SessionId).string().null())
                    .col(ColumnDef::new(BrowserTabs::Url).string().not_null())
                    .col(ColumnDef::new(BrowserTabs::Title).string().not_null())
                    .col(ColumnDef::new(BrowserTabs::Position).integer().not_null())
                    .col(
                        ColumnDef::new(BrowserTabs::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(BrowserTabs::LastActiveAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(BrowserTabs::Discarded)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_browser_tabs_session")
                    .table(BrowserTabs::Table)
                    .col(BrowserTabs::SessionId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(BrowserTabs::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum BrowserTabs {
    Table,
    Id,
    OwnerKind,
    SessionId,
    Url,
    Title,
    Position,
    CreatedAt,
    LastActiveAt,
    Discarded,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::sea_orm::Database;

    #[tokio::test]
    async fn tabs_are_keyed_by_id_and_down_drops_them() {
        let conn = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        let manager = SchemaManager::new(&conn);
        Migration.up(&manager).await.expect("run migration up");

        let insert = "INSERT INTO browser_tabs \
            (id, owner_kind, session_id, url, title, position, created_at, last_active_at) \
            VALUES ('t1', 'session', 's1', 'http://localhost:5173/', 'App', 0, \
            '2026-11-13T00:00:00Z', '2026-11-13T00:00:00Z')";
        conn.execute_unprepared(insert).await.expect("first tab");
        assert!(conn.execute_unprepared(insert).await.is_err());

        Migration.down(&manager).await.expect("run migration down");
        assert!(!manager.has_table("browser_tabs").await.expect("has table"));
    }
}
