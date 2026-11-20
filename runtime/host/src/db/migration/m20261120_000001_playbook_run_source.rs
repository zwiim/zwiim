//! Zwiim-owned: how far discovery got with each source of a run
//! (automation-engine spec 6.2.8). A Host restart during `discovering` reruns
//! the sources still `pending`; items already queued are kept.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PlaybookRunSource::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PlaybookRunSource::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(PlaybookRunSource::RunId)
                            .integer()
                            .not_null(),
                    )
                    // The source's place in the pinned playbook's `sources`.
                    .col(
                        ColumnDef::new(PlaybookRunSource::SourceIndex)
                            .integer()
                            .not_null(),
                    )
                    // `pending` | `done`
                    .col(
                        ColumnDef::new(PlaybookRunSource::Status)
                            .string()
                            .not_null(),
                    )
                    // The run note: what was found, and a limit that stopped it.
                    .col(ColumnDef::new(PlaybookRunSource::Note).string().null())
                    .col(
                        ColumnDef::new(PlaybookRunSource::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_playbook_run_source_once")
                    .table(PlaybookRunSource::Table)
                    .col(PlaybookRunSource::RunId)
                    .col(PlaybookRunSource::SourceIndex)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PlaybookRunSource::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum PlaybookRunSource {
    Table,
    Id,
    RunId,
    SourceIndex,
    Status,
    Note,
    UpdatedAt,
}
