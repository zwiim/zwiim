//! Zwiim-owned: the user's on/off switch for a whole connector. One that is
//! off keeps its key, tools and grants, but agents are shown none of its
//! tools and its calls are refused. Existing connectors stay on.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Connectors::Table)
                    .add_column(
                        ColumnDef::new(Connectors::Enabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Connectors::Table)
                    .drop_column(Connectors::Enabled)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Connectors {
    Table,
    Enabled,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::sea_orm::{ConnectionTrait, Database};

    #[tokio::test]
    async fn existing_connectors_stay_on() {
        let conn = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        let manager = SchemaManager::new(&conn);
        conn.execute_unprepared(
            "CREATE TABLE connectors (id TEXT PRIMARY KEY); \
             INSERT INTO connectors (id) VALUES ('c1')",
        )
        .await
        .unwrap();
        Migration.up(&manager).await.expect("run migration up");

        let row = conn
            .query_one(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                "SELECT enabled FROM connectors",
            ))
            .await
            .unwrap()
            .unwrap();
        assert!(row.try_get::<bool>("", "enabled").unwrap(), "on");

        Migration.down(&manager).await.expect("run migration down");
        assert!(!manager.has_column("connectors", "enabled").await.unwrap());
    }
}
