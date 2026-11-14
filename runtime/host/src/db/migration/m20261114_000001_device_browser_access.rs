//! Zwiim-owned: what a paired device may do with the Host browser (browser
//! design 7.6): `none`, `view` (see its sessions' tabs) or `control` (steer
//! them, and Live App). Devices paired before this get `view`, as a newly
//! paired one does.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(TrustedDevices::Table)
                    .add_column(
                        ColumnDef::new(TrustedDevices::BrowserAccess)
                            .string()
                            .not_null()
                            .default("view"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(TrustedDevices::Table)
                    .drop_column(TrustedDevices::BrowserAccess)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum TrustedDevices {
    Table,
    BrowserAccess,
}
