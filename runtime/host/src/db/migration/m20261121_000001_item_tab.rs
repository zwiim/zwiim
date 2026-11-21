//! Zwiim-owned: the browser tab an item works in or is parked on, so a
//! client can show it (automations canvas). Null when the item has no tab;
//! cleared at Host start, since tabs do not survive the Host.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(PlaybookItem::Table)
                    .add_column(ColumnDef::new(PlaybookItem::TabId).string().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(PlaybookItem::Table)
                    .drop_column(PlaybookItem::TabId)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum PlaybookItem {
    Table,
    TabId,
}
