//! Zwiim-owned: cryptographic device identity on the trusted-device registry.
//!
//! `device_pubkey` is the device's X25519 static public key (URL-safe base64),
//! registered at pairing; it replaces the bearer credential as the device's
//! trust anchor. NULL for devices paired with a bearer credential before
//! this migration. `last_seen_at` is when the device last authenticated.

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
                    .add_column(ColumnDef::new(TrustedDevices::DevicePubkey).string().null())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(TrustedDevices::Table)
                    .add_column(
                        ColumnDef::new(TrustedDevices::LastSeenAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_trusted_devices_device_pubkey")
                    .table(TrustedDevices::Table)
                    .col(TrustedDevices::DevicePubkey)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_trusted_devices_device_pubkey")
                    .table(TrustedDevices::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(TrustedDevices::Table)
                    .drop_column(TrustedDevices::LastSeenAt)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(TrustedDevices::Table)
                    .drop_column(TrustedDevices::DevicePubkey)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum TrustedDevices {
    Table,
    DevicePubkey,
    LastSeenAt,
}
