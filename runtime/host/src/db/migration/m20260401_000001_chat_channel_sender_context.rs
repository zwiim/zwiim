use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ChatChannelSenderContext::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ChatChannelSenderContext::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelSenderContext::ChannelId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelSenderContext::SenderId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelSenderContext::CurrentFolderId)
                            .integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelSenderContext::CurrentAgentType)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelSenderContext::CurrentConversationId)
                            .integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelSenderContext::CurrentConnectionId)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelSenderContext::AutoApprove)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(ChatChannelSenderContext::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelSenderContext::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_ccsc_channel_id")
                            .from(
                                ChatChannelSenderContext::Table,
                                ChatChannelSenderContext::ChannelId,
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
                    .name("idx_ccsc_channel_sender")
                    .table(ChatChannelSenderContext::Table)
                    .col(ChatChannelSenderContext::ChannelId)
                    .col(ChatChannelSenderContext::SenderId)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(ChatChannelSenderContext::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ChatChannelSenderContext {
    Table,
    Id,
    ChannelId,
    SenderId,
    CurrentFolderId,
    CurrentAgentType,
    CurrentConversationId,
    CurrentConnectionId,
    AutoApprove,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ChatChannel {
    Table,
    Id,
}
