// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: the base a managed worktree workspace was created from.
//!
//! `base_branch` is the root workspace's branch at creation (NULL when its
//! HEAD was detached) and `base_commit` the commit the worktree branch started
//! at. Both are NULL for root workspaces and for worktrees registered before
//! this migration; the workspace diff falls back to the worktree's HEAD then.

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
                    .add_column(ColumnDef::new(Folder::BaseBranch).string().null())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Folder::Table)
                    .add_column(ColumnDef::new(Folder::BaseCommit).string().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Folder::Table)
                    .drop_column(Folder::BaseCommit)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Folder::Table)
                    .drop_column(Folder::BaseBranch)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Folder {
    Table,
    BaseBranch,
    BaseCommit,
}
