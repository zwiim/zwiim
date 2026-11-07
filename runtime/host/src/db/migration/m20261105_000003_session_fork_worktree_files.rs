// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: how many files a fork to a new worktree started with, for the
//! card its first screen shows. Null for a fork in place, and for a link from
//! before this migration.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(SessionForks::Table)
                    .add_column(ColumnDef::new(SessionForks::WorktreeFiles).big_integer().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(SessionForks::Table)
                    .drop_column(SessionForks::WorktreeFiles)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum SessionForks {
    Table,
    WorktreeFiles,
}
