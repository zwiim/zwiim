//! Zwiim-owned: host-owned opaque `public_id` per sidebar group, and one
//! sequence for `folder.sort_order`.
//!
//! The id mirrors `m20260923_000002_workspace_session_ids`: nullable, minted by
//! the entity's `before_save` hook, backfilled here. Clients address a group by
//! it, never by the integer primary key.
//!
//! `sort_order = id` keeps the sidebar as it is on upgrade. Workspaces have
//! always been listed by id, and clients start reading this column now, so rows
//! adopted from an older database, which carry recency order, would reshuffle.

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
                    .table(FolderGroup::Table)
                    .add_column(ColumnDef::new(FolderGroup::PublicId).string().null())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_folder_group_public_id")
                    .table(FolderGroup::Table)
                    .col(FolderGroup::PublicId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        let db = manager.get_connection();
        let backend = db.get_database_backend();
        let rows = db
            .query_all(Statement::from_string(
                backend,
                "SELECT id FROM folder_group WHERE public_id IS NULL".to_owned(),
            ))
            .await?;
        for row in rows {
            let id: i32 = row.try_get("", "id")?;
            db.execute(Statement::from_sql_and_values(
                backend,
                "UPDATE folder_group SET public_id = ? WHERE id = ?",
                [uuid::Uuid::new_v4().to_string().into(), id.into()],
            ))
            .await?;
        }
        db.execute(Statement::from_string(
            backend,
            "UPDATE folder SET sort_order = id".to_owned(),
        ))
        .await?;
        Ok(())
    }

    /// `sort_order` is not restored: the overwrite is one-way.
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_folder_group_public_id")
                    .table(FolderGroup::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(FolderGroup::Table)
                    .drop_column(FolderGroup::PublicId)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum FolderGroup {
    Table,
    PublicId,
}
