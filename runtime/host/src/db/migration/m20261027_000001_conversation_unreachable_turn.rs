// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: the turn that failed because the agent couldn't reach its
//! provider, saved for `session.retry`.
//!
//! `unreachable_turn` is JSON (`session_retry::UnreachableTurn`: the error
//! message and the failed prompt's blocks). NULL means nothing to retry, which
//! is every row before this migration and every row whose next prompt went
//! out.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .add_column(ColumnDef::new(Conversation::UnreachableTurn).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .drop_column(Conversation::UnreachableTurn)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Conversation {
    Table,
    UnreachableTurn,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::sea_orm::{ConnectionTrait, Database, DbBackend, Statement};

    async fn columns(conn: &sea_orm_migration::sea_orm::DatabaseConnection) -> Vec<String> {
        conn.query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT name FROM pragma_table_info('conversation')".to_owned(),
        ))
        .await
        .expect("table info")
        .iter()
        .map(|r| r.try_get::<String>("", "name").expect("name"))
        .collect()
    }

    /// `up` adds a nullable `unreachable_turn` that existing rows read as NULL;
    /// `down` drops it again.
    #[tokio::test]
    async fn unreachable_turn_column_is_nullable_and_down_drops_it() {
        let conn = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        conn.execute_unprepared(
            "CREATE TABLE conversation (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT)",
        )
        .await
        .expect("create stub table");
        conn.execute_unprepared("INSERT INTO conversation (title) VALUES ('x')")
            .await
            .expect("insert row");

        let manager = SchemaManager::new(&conn);
        Migration.up(&manager).await.expect("run migration up");
        let rows = conn
            .query_all(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT unreachable_turn FROM conversation".to_owned(),
            ))
            .await
            .expect("query rows");
        assert_eq!(rows.len(), 1);
        let saved: Option<String> = rows[0]
            .try_get("", "unreachable_turn")
            .expect("unreachable_turn col");
        assert!(saved.is_none(), "existing rows have nothing to retry");

        Migration.down(&manager).await.expect("run migration down");
        assert!(!columns(&conn)
            .await
            .contains(&"unreachable_turn".to_string()));
        assert!(columns(&conn).await.contains(&"title".to_string()));
    }
}
