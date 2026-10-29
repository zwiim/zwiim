//! Zwiim-owned: a session's queued follow-ups, the prompts waiting for its
//! running turn to end.
//!
//! `follow_ups` is JSON (a list of `follow_ups::FollowUp`, first to go out
//! first). NULL means nothing queued, which is every row before this
//! migration.

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
                    .add_column(ColumnDef::new(Conversation::FollowUps).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .drop_column(Conversation::FollowUps)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Conversation {
    Table,
    FollowUps,
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

    /// `up` adds a nullable `follow_ups` that existing rows read as NULL;
    /// `down` drops it again.
    #[tokio::test]
    async fn follow_ups_column_is_nullable_and_down_drops_it() {
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
                "SELECT follow_ups FROM conversation".to_owned(),
            ))
            .await
            .expect("query rows");
        assert_eq!(rows.len(), 1);
        let saved: Option<String> = rows[0]
            .try_get("", "follow_ups")
            .expect("follow_ups col");
        assert!(saved.is_none(), "existing rows have nothing queued");

        Migration.down(&manager).await.expect("run migration down");
        assert!(!columns(&conn)
            .await
            .contains(&"follow_ups".to_string()));
        assert!(columns(&conn).await.contains(&"title".to_string()));
    }
}
