// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: the host's trusted-device registry (Identity V1).
//!
//! One row per device that has paired with this host. `credential_hash` is a
//! SHA-256 of the device's bearer credential (never the credential itself) and
//! is cleared on revocation, so a revoked device can no longer authenticate.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(TrustedDevices::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(TrustedDevices::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(TrustedDevices::DeviceId)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(TrustedDevices::DeviceName)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(ColumnDef::new(TrustedDevices::Status).string().not_null())
                    .col(
                        ColumnDef::new(TrustedDevices::CredentialHash)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(TrustedDevices::PairedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TrustedDevices::RevokedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(TrustedDevices::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TrustedDevices::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_trusted_devices_credential_hash")
                    .table(TrustedDevices::Table)
                    .col(TrustedDevices::CredentialHash)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(TrustedDevices::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum TrustedDevices {
    Table,
    Id,
    DeviceId,
    DeviceName,
    Status,
    CredentialHash,
    PairedAt,
    RevokedAt,
    CreatedAt,
    UpdatedAt,
}
