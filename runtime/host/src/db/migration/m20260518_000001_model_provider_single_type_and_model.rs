use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend, Statement};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ModelProvider::Table)
                    .add_column(
                        ColumnDef::new(ModelProvider::AgentType)
                            .text()
                            .not_null()
                            .default(""),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(ModelProvider::Table)
                    .add_column(ColumnDef::new(ModelProvider::Model).text().null())
                    .to_owned(),
            )
            .await?;

        // Backfill agent_type from the first element of agent_types_json.
        let conn = manager.get_connection();
        let sql = "UPDATE model_provider \
            SET agent_type = COALESCE(json_extract(agent_types_json, '$[0]'), '')";
        conn.execute(Statement::from_string(DbBackend::Sqlite, sql.to_string()))
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ModelProvider::Table)
                    .drop_column(ModelProvider::Model)
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(ModelProvider::Table)
                    .drop_column(ModelProvider::AgentType)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ModelProvider {
    Table,
    AgentType,
    Model,
}
