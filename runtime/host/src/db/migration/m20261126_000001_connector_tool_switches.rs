//! Zwiim-owned: which of a connector's tools agents are shown, and how much
//! of an agent's context the shown ones may take (connector design r3 §8.2,
//! as the user chooses it). `enabled` is the user's switch per tool;
//! `tool_budget` caps the estimated size of a connector's enabled tools, in
//! token-equivalents.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ConnectorTools::Table)
                    .add_column(
                        ColumnDef::new(ConnectorTools::Enabled)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Connectors::Table)
                    .add_column(
                        ColumnDef::new(Connectors::ToolBudget)
                            .integer()
                            .not_null()
                            .default(4000)
                            .check(Expr::cust("tool_budget BETWEEN 1000 AND 10000")),
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
                    .drop_column(Connectors::ToolBudget)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ConnectorTools::Table)
                    .drop_column(ConnectorTools::Enabled)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ConnectorTools {
    Table,
    Enabled,
}

#[derive(DeriveIden)]
enum Connectors {
    Table,
    ToolBudget,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::sea_orm::{ConnectionTrait, Database};

    #[tokio::test]
    async fn tools_start_off_and_a_budget_stays_in_range() {
        let conn = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        let manager = SchemaManager::new(&conn);
        conn.execute_unprepared(
            "CREATE TABLE connectors (id TEXT PRIMARY KEY); \
             CREATE TABLE connector_tools (id INTEGER PRIMARY KEY, name TEXT); \
             INSERT INTO connectors (id) VALUES ('c1'); \
             INSERT INTO connector_tools (name) VALUES ('echo')",
        )
        .await
        .unwrap();
        Migration.up(&manager).await.expect("run migration up");

        let row = conn
            .query_one(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                "SELECT t.enabled AS enabled, c.tool_budget AS budget \
                 FROM connector_tools t, connectors c",
            ))
            .await
            .unwrap()
            .unwrap();
        assert!(!row.try_get::<bool>("", "enabled").unwrap(), "off");
        assert_eq!(row.try_get::<i32>("", "budget").unwrap(), 4000);
        for (budget, ok) in [(1000, true), (10000, true), (999, false), (10001, false)] {
            let set = conn
                .execute_unprepared(&format!("UPDATE connectors SET tool_budget = {budget}"))
                .await;
            assert_eq!(set.is_ok(), ok, "{budget}");
        }

        Migration.down(&manager).await.expect("run migration down");
        assert!(!manager
            .has_column("connectors", "tool_budget")
            .await
            .unwrap());
        assert!(!manager
            .has_column("connector_tools", "enabled")
            .await
            .unwrap());
    }
}
