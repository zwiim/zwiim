//! Zwiim-owned: an opaque `public_id` for each playbook, run, item, approval
//! and alert. Clients address them by it, never by the integer primary key
//! (protocol/mod.rs). Mirrors `m20261104_000001_workspace_group_ids`:
//! nullable, minted on insert, backfilled here.

use sea_orm::{ConnectionTrait, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const TABLES: [&str; 5] = [
    "playbook",
    "playbook_run",
    "playbook_item",
    "playbook_approval",
    "playbook_alert",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        let backend = db.get_database_backend();
        for table in TABLES {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new(table))
                        .add_column(ColumnDef::new(Alias::new("public_id")).string().null())
                        .to_owned(),
                )
                .await?;
            manager
                .create_index(
                    Index::create()
                        .name(format!("idx_{table}_public_id"))
                        .table(Alias::new(table))
                        .col(Alias::new("public_id"))
                        .unique()
                        .to_owned(),
                )
                .await?;
            let rows = db
                .query_all(Statement::from_string(
                    backend,
                    format!("SELECT id FROM {table} WHERE public_id IS NULL"),
                ))
                .await?;
            for row in rows {
                let id: i32 = row.try_get("", "id")?;
                db.execute(Statement::from_sql_and_values(
                    backend,
                    format!("UPDATE {table} SET public_id = ? WHERE id = ?"),
                    [uuid::Uuid::new_v4().to_string().into(), id.into()],
                ))
                .await?;
            }
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in TABLES {
            manager
                .drop_index(
                    Index::drop()
                        .name(format!("idx_{table}_public_id"))
                        .table(Alias::new(table))
                        .to_owned(),
                )
                .await?;
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new(table))
                        .drop_column(Alias::new("public_id"))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
    use sea_orm_migration::MigratorTrait;

    use crate::db::migration::Migrator;

    /// A playbook written before this migration gets a public id.
    #[tokio::test]
    async fn backfills_a_public_id() {
        let conn = Database::connect("sqlite::memory:").await.expect("db");
        let migrations = <Migrator as MigratorTrait>::migrations();
        let at = migrations
            .iter()
            .position(|m| m.name().contains("playbook_public_ids"))
            .expect("registered");
        Migrator::up(&conn, Some(at as u32)).await.expect("earlier");
        conn.execute(Statement::from_string(
            DbBackend::Sqlite,
            "INSERT INTO playbook (id, name, source, format, version, show_me, \
             freeze_learning, created_at, updated_at) VALUES \
             (1, 'Apply', 'goal', 1, 1, 1, 0, '2026-11-19 00:00:00', '2026-11-19 00:00:00')",
        ))
        .await
        .expect("seed");
        Migrator::up(&conn, None).await.expect("rest");
        let row = conn
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT public_id FROM playbook WHERE id = 1",
            ))
            .await
            .expect("query")
            .expect("row");
        let id: String = row.try_get("", "public_id").expect("backfilled");
        assert!(uuid::Uuid::parse_str(&id).is_ok(), "{id}");
    }
}
