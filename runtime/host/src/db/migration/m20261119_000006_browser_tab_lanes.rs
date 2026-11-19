//! Zwiim-owned: a browser tab may belong to a lane, one (site, account)
//! login shared by every run on that site (automation spec 14.1). Lane tabs
//! carry `owner_kind = 'lane'` and these two columns; others leave them null.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [BrowserTabs::LaneSite, BrowserTabs::LaneAccount] {
            manager
                .alter_table(
                    Table::alter()
                        .table(BrowserTabs::Table)
                        .add_column(ColumnDef::new(column).string().null())
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [BrowserTabs::LaneSite, BrowserTabs::LaneAccount] {
            manager
                .alter_table(
                    Table::alter()
                        .table(BrowserTabs::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
enum BrowserTabs {
    Table,
    LaneSite,
    LaneAccount,
}
