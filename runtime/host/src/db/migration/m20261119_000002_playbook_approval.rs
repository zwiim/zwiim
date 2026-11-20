//! Zwiim-owned: engine-owned approval requests (automation-engine spec 8.5).
//!
//! One row per pending outward action. `state` is `pending` until the person
//! answers (`approved` / `rejected` / `stopped`); the first answer wins by a
//! conditional UPDATE, so a Mac and a phone racing produce one outcome.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PlaybookApproval::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PlaybookApproval::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(PlaybookApproval::RunId).integer().not_null())
                    .col(
                        ColumnDef::new(PlaybookApproval::ItemId)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(PlaybookApproval::Kind).string().not_null())
                    // JSON of the declaration: submit_target + declared fields.
                    .col(ColumnDef::new(PlaybookApproval::Payload).text().not_null())
                    // The whitespace-insensitive binding hash (spec 8.5.3).
                    .col(ColumnDef::new(PlaybookApproval::Hash).string().not_null())
                    // `pending` | `approved` | `rejected` | `stopped`
                    .col(ColumnDef::new(PlaybookApproval::State).string().not_null())
                    // How an approved item continues; null until approved.
                    .col(
                        ColumnDef::new(PlaybookApproval::ConsumedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(PlaybookApproval::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_playbook_approval_item_pending")
                    .table(PlaybookApproval::Table)
                    .col(PlaybookApproval::ItemId)
                    .col(PlaybookApproval::State)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PlaybookApproval::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum PlaybookApproval {
    Table,
    Id,
    RunId,
    ItemId,
    Kind,
    Payload,
    Hash,
    State,
    ConsumedAt,
    CreatedAt,
}
