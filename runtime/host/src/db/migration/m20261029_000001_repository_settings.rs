//! Zwiim-owned: what the user set for one repository, such as the scripts run
//! when its worktrees are created and removed.
//!
//! One row per root folder, made on the first save. `settings` is a JSON
//! object of the keys set (`repository_settings::KEYS`); an unset key is
//! absent. A repository removed and added again revives its folder row, so its
//! settings come back with it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(RepositorySettings::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RepositorySettings::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(RepositorySettings::FolderId)
                            .integer()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(RepositorySettings::Settings).text().not_null())
                    .col(
                        ColumnDef::new(RepositorySettings::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(RepositorySettings::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum RepositorySettings {
    Table,
    Id,
    FolderId,
    Settings,
    UpdatedAt,
}
