//! Zwiim-owned: the ports a managed worktree's processes may use.
//!
//! `port` is the first of a block of `workspace::ports::BLOCK` ports given to
//! the worktree's scripts, terminals and agents as `ZWIIM_PORT`, chosen the
//! first time one of them starts. NULL for roots and for worktrees nothing has
//! started in yet.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Folder::Table)
                    .add_column(ColumnDef::new(Folder::Port).integer().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(Table::alter().table(Folder::Table).drop_column(Folder::Port).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Folder {
    Table,
    Port,
}
