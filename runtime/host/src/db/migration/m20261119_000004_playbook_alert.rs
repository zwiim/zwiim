//! Zwiim-owned: the alerts the Host sent for playbook runs (automation-engine
//! spec 11.4.2-3). One row per (kind, subject), so a resume or a re-read
//! never sends the same alert twice.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PlaybookAlert::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PlaybookAlert::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    // `run_done` | `item_waiting_person` |
                    // `item_retries_exhausted` | `approval_waiting`
                    .col(ColumnDef::new(PlaybookAlert::Kind).string().not_null())
                    // What the alert is about: the run, the item or the
                    // approval, by the kind.
                    .col(ColumnDef::new(PlaybookAlert::Subject).integer().not_null())
                    .col(ColumnDef::new(PlaybookAlert::RunId).integer().not_null())
                    .col(ColumnDef::new(PlaybookAlert::ItemId).integer().null())
                    .col(ColumnDef::new(PlaybookAlert::Text).string().not_null())
                    .col(
                        ColumnDef::new(PlaybookAlert::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_playbook_alert_once")
                    .table(PlaybookAlert::Table)
                    .col(PlaybookAlert::Kind)
                    .col(PlaybookAlert::Subject)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PlaybookAlert::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum PlaybookAlert {
    Table,
    Id,
    Kind,
    Subject,
    RunId,
    ItemId,
    Text,
    CreatedAt,
}
