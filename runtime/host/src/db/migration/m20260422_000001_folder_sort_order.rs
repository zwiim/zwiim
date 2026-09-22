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
                    .table(Folder::Table)
                    .add_column(
                        ColumnDef::new(Folder::SortOrder)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;

        // Backfill sort_order by current last_opened_at DESC so existing users
        // see the same order after migration. We use a correlated subquery that
        // works on SQLite without ROW_NUMBER/window functions.
        let conn = manager.get_connection();
        let sql = "UPDATE folder SET sort_order = (\
            SELECT COUNT(*) FROM folder AS inner_f \
            WHERE inner_f.last_opened_at > folder.last_opened_at \
               OR (inner_f.last_opened_at = folder.last_opened_at AND inner_f.id < folder.id) \
        ) + 1";
        conn.execute(Statement::from_string(DbBackend::Sqlite, sql.to_string()))
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_folder_sort_order")
                    .table(Folder::Table)
                    .col(Folder::SortOrder)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_folder_sort_order")
                    .table(Folder::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Folder::Table)
                    .drop_column(Folder::SortOrder)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Folder {
    Table,
    SortOrder,
}
