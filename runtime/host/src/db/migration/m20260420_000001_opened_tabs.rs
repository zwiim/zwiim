use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(FolderOpenedConversation::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(OpenedTab::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OpenedTab::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(OpenedTab::FolderId).integer().not_null())
                    .col(ColumnDef::new(OpenedTab::ConversationId).integer().null())
                    .col(ColumnDef::new(OpenedTab::AgentType).string().not_null())
                    .col(ColumnDef::new(OpenedTab::Position).integer().not_null())
                    .col(
                        ColumnDef::new(OpenedTab::IsActive)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(OpenedTab::IsPinned)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(OpenedTab::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OpenedTab::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(OpenedTab::Table, OpenedTab::FolderId)
                            .to(Folder::Table, Folder::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(OpenedTab::Table, OpenedTab::ConversationId)
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_opened_tabs_folder_id")
                    .table(OpenedTab::Table)
                    .col(OpenedTab::FolderId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_opened_tabs_position")
                    .table(OpenedTab::Table)
                    .col(OpenedTab::Position)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(OpenedTab::Table).if_exists().to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum OpenedTab {
    Table,
    Id,
    FolderId,
    ConversationId,
    AgentType,
    Position,
    IsActive,
    IsPinned,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum FolderOpenedConversation {
    Table,
}

#[derive(DeriveIden)]
enum Folder {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Conversation {
    Table,
    Id,
}
