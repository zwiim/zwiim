use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ─── chat_channel ───
        manager
            .create_table(
                Table::create()
                    .table(ChatChannel::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ChatChannel::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ChatChannel::Name).string().not_null())
                    .col(ColumnDef::new(ChatChannel::ChannelType).string().not_null())
                    .col(
                        ColumnDef::new(ChatChannel::Enabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(ColumnDef::new(ChatChannel::ConfigJson).text().not_null())
                    .col(ColumnDef::new(ChatChannel::EventFilterJson).text().null())
                    .col(
                        ColumnDef::new(ChatChannel::DailyReportEnabled)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(ChatChannel::DailyReportTime).string().null())
                    .col(
                        ColumnDef::new(ChatChannel::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannel::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        // ─── chat_channel_message_log ───
        manager
            .create_table(
                Table::create()
                    .table(ChatChannelMessageLog::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ChatChannelMessageLog::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelMessageLog::ChannelId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelMessageLog::Direction)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelMessageLog::MessageType)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelMessageLog::ContentPreview)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelMessageLog::Status)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelMessageLog::ErrorDetail)
                            .text()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelMessageLog::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_ccml_channel_id")
                            .from(
                                ChatChannelMessageLog::Table,
                                ChatChannelMessageLog::ChannelId,
                            )
                            .to(ChatChannel::Table, ChatChannel::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_ccml_channel_created")
                    .table(ChatChannelMessageLog::Table)
                    .col(ChatChannelMessageLog::ChannelId)
                    .col(ChatChannelMessageLog::CreatedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ChatChannelMessageLog::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ChatChannel::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ChatChannel {
    Table,
    Id,
    Name,
    ChannelType,
    Enabled,
    ConfigJson,
    EventFilterJson,
    DailyReportEnabled,
    DailyReportTime,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ChatChannelMessageLog {
    Table,
    Id,
    ChannelId,
    Direction,
    MessageType,
    ContentPreview,
    Status,
    ErrorDetail,
    CreatedAt,
}
