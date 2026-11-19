//! Zwiim-owned: the cap slot an item's dispatch took (automation-engine
//! spec 6.6.1, 6.6.5), so "Not sent, redo" can give back that day's slot
//! even after local midnight has passed.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite adds one column per ALTER TABLE.
        for column in [
            PlaybookItem::CapSite,
            PlaybookItem::CapKind,
            PlaybookItem::CapDay,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(PlaybookItem::Table)
                        .add_column(ColumnDef::new(column).string().null())
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            PlaybookItem::CapSite,
            PlaybookItem::CapKind,
            PlaybookItem::CapDay,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(PlaybookItem::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
enum PlaybookItem {
    Table,
    CapSite,
    CapKind,
    CapDay,
}
