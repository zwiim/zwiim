//! Zwiim-owned: the sites the Host browser's site policy keeps (browser
//! design 13.2): each session's strict site list, and the sites a person
//! always allows agents to open.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(BrowserSites::Table)
                    // `strict` (a session's strict site list) or `approved`
                    // (always allowed, every session).
                    .col(ColumnDef::new(BrowserSites::Kind).string().not_null())
                    // The public session id; empty for `approved`.
                    .col(ColumnDef::new(BrowserSites::SessionId).string().not_null())
                    // A site pattern: `example.com`, `*.example.com`, `host:port`.
                    .col(ColumnDef::new(BrowserSites::Pattern).string().not_null())
                    .col(
                        ColumnDef::new(BrowserSites::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(BrowserSites::Kind)
                            .col(BrowserSites::SessionId)
                            .col(BrowserSites::Pattern),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(BrowserSites::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum BrowserSites {
    Table,
    Kind,
    SessionId,
    Pattern,
    CreatedAt,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::sea_orm::Database;

    #[tokio::test]
    async fn a_site_is_kept_once_per_kind_and_session_and_down_drops_them() {
        let conn = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        let manager = SchemaManager::new(&conn);
        Migration.up(&manager).await.expect("run migration up");

        let insert = "INSERT INTO browser_sites (kind, session_id, pattern, created_at) \
            VALUES ('strict', 's1', 'github.com', '2026-11-13T00:00:00Z')";
        conn.execute_unprepared(insert).await.expect("first site");
        assert!(conn.execute_unprepared(insert).await.is_err());
        let other = "INSERT INTO browser_sites (kind, session_id, pattern, created_at) \
            VALUES ('strict', 's2', 'github.com', '2026-11-13T00:00:00Z')";
        conn.execute_unprepared(other)
            .await
            .expect("another session");

        Migration.down(&manager).await.expect("run migration down");
        assert!(!manager.has_table("browser_sites").await.expect("has table"));
    }
}
