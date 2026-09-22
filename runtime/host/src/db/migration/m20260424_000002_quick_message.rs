use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(QuickMessage::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(QuickMessage::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(QuickMessage::Title).string().not_null())
                    .col(ColumnDef::new(QuickMessage::Content).text().not_null())
                    .col(
                        ColumnDef::new(QuickMessage::SortOrder)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(QuickMessage::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(QuickMessage::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_quick_message_sort_order")
                    .table(QuickMessage::Table)
                    .col(QuickMessage::SortOrder)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(QuickMessage::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum QuickMessage {
    Table,
    Id,
    Title,
    Content,
    SortOrder,
    CreatedAt,
    UpdatedAt,
}
