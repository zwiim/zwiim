//! Zwiim-owned: what a playbook knows per site beyond its text
//! (automation-engine spec 5.4, 7.3, 7.4): answers the person gave to form
//! questions, and tips item agents proposed. An answer row starts `open`
//! when an item asks and becomes `answered`; a tip is `proposed` until the
//! person approves or rejects it, and only approved tips reach an agent.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PlaybookKnowledge::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PlaybookKnowledge::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(PlaybookKnowledge::PlaybookId)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(PlaybookKnowledge::Site).string().not_null())
                    // `answer` | `tip`
                    .col(ColumnDef::new(PlaybookKnowledge::Kind).string().not_null())
                    // The question, or the tip.
                    .col(ColumnDef::new(PlaybookKnowledge::Text).string().not_null())
                    // The person's answer; null while the question is open.
                    .col(ColumnDef::new(PlaybookKnowledge::Value).string().null())
                    // Answers: `open` | `answered`. Tips: `proposed` |
                    // `approved` | `rejected`.
                    .col(ColumnDef::new(PlaybookKnowledge::State).string().not_null())
                    // The item that asked or proposed it.
                    .col(ColumnDef::new(PlaybookKnowledge::ItemId).integer().null())
                    .col(
                        ColumnDef::new(PlaybookKnowledge::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PlaybookKnowledge::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_playbook_knowledge_once")
                    .table(PlaybookKnowledge::Table)
                    .col(PlaybookKnowledge::PlaybookId)
                    .col(PlaybookKnowledge::Site)
                    .col(PlaybookKnowledge::Kind)
                    .col(PlaybookKnowledge::Text)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PlaybookKnowledge::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum PlaybookKnowledge {
    Table,
    Id,
    PlaybookId,
    Site,
    Kind,
    Text,
    Value,
    State,
    ItemId,
    CreatedAt,
    UpdatedAt,
}
