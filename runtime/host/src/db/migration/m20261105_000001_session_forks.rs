//! Zwiim-owned: which session and turn a forked session came from.
//!
//! A table rather than a file in the session's worktree, as the link crosses
//! worktrees, and rather than a column, as most sessions have none. Ids are the
//! public `session_id`s, so the link outlives a source row being renumbered,
//! and is read back with no join: a source that is gone only loses its title.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(SessionForks::Table)
                    .col(
                        ColumnDef::new(SessionForks::SessionId)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(SessionForks::SourceSessionId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SessionForks::SourceTurnId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SessionForks::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(SessionForks::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum SessionForks {
    Table,
    SessionId,
    SourceSessionId,
    SourceTurnId,
    CreatedAt,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::sea_orm::Database;

    /// `up` makes one link per forked session; `down` drops the table.
    #[tokio::test]
    async fn one_link_per_session_and_down_drops_it() {
        let conn = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        let manager = SchemaManager::new(&conn);
        Migration.up(&manager).await.expect("run migration up");

        let insert = "INSERT INTO session_forks \
            (session_id, source_session_id, source_turn_id, created_at) \
            VALUES ('s2', 's1', 'turn-3', '2026-11-05T00:00:00Z')";
        conn.execute_unprepared(insert).await.expect("first link");
        assert!(conn.execute_unprepared(insert).await.is_err());

        Migration.down(&manager).await.expect("run migration down");
        assert!(!manager.has_table("session_forks").await.expect("has table"));
    }
}
