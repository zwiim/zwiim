use sea_orm::entity::prelude::*;

/// Zwiim-owned: a device paired with this host (Identity V1). See
/// `crate::identity::trusted_devices`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "trusted_devices")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(unique)]
    pub device_id: String,
    pub device_name: String,
    /// `"trusted"` or `"revoked"`.
    pub status: String,
    /// SHA-256 (hex) of the device's bearer credential. `None` once revoked.
    #[sea_orm(unique)]
    pub credential_hash: Option<String>,
    /// The device's X25519 static public key (URL-safe base64). `None` for
    /// a device paired with a bearer credential.
    #[sea_orm(unique)]
    pub device_pubkey: Option<String>,
    pub paired_at: DateTimeUtc,
    pub revoked_at: Option<DateTimeUtc>,
    /// When the device last authenticated.
    pub last_seen_at: Option<DateTimeUtc>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
