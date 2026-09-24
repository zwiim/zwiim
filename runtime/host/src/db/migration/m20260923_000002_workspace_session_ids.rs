//! Zwiim-owned: host-owned opaque `workspace_id` (per user-facing Folder row)
//! and `session_id` (per client-facing Conversation row).
//!
//! Both are nullable: Chat scratch folders, and Delegate / Loop conversations,
//! are internal and carry no client-facing identity. New rows are minted by
//! the entities' `before_save` hook; this migration backfills existing rows —
//! soft-deleted ones included, since a revive/restore must keep its identity.

use sea_orm::{ConnectionTrait, Statement};
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
                    .add_column(ColumnDef::new(Folder::WorkspaceId).string().null())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_folder_workspace_id")
                    .table(Folder::Table)
                    .col(Folder::WorkspaceId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .add_column(ColumnDef::new(Conversation::SessionId).string().null())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_conversation_session_id")
                    .table(Conversation::Table)
                    .col(Conversation::SessionId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        backfill(
            manager,
            "SELECT id FROM folder WHERE kind = 'regular' AND workspace_id IS NULL",
            "UPDATE folder SET workspace_id = ? WHERE id = ?",
        )
        .await?;
        backfill(
            manager,
            "SELECT id FROM conversation \
             WHERE kind IN ('regular', 'chat') AND session_id IS NULL",
            "UPDATE conversation SET session_id = ? WHERE id = ?",
        )
        .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_conversation_session_id")
                    .table(Conversation::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .drop_column(Conversation::SessionId)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx_folder_workspace_id")
                    .table(Folder::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Folder::Table)
                    .drop_column(Folder::WorkspaceId)
                    .to_owned(),
            )
            .await
    }
}

/// Give every row selected by `select_ids` its own fresh UUID via `update`.
async fn backfill(manager: &SchemaManager<'_>, select_ids: &str, update: &str) -> Result<(), DbErr> {
    let db = manager.get_connection();
    let backend = db.get_database_backend();
    let rows = db
        .query_all(Statement::from_string(backend, select_ids.to_owned()))
        .await?;
    for row in rows {
        let id: i32 = row.try_get("", "id")?;
        db.execute(Statement::from_sql_and_values(
            backend,
            update,
            [uuid::Uuid::new_v4().to_string().into(), id.into()],
        ))
        .await?;
    }
    Ok(())
}

#[derive(DeriveIden)]
enum Folder {
    Table,
    WorkspaceId,
}

#[derive(DeriveIden)]
enum Conversation {
    Table,
    SessionId,
}
