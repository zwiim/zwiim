//! Host-side trusted-device registry.
//!
//! A device becomes trusted by pairing (see `pairing`), which issues it a
//! random bearer credential. The host stores only a SHA-256 of that credential.
//! Revocation flips the status and clears the hash, so the credential stops
//! authenticating immediately.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use rand::RngCore;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, QueryFilter, QueryOrder, Set,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::db::entities::trusted_device;
use crate::db::error::DbError;

pub const STATUS_TRUSTED: &str = "trusted";
pub const STATUS_REVOKED: &str = "revoked";

const MAX_DEVICE_ID_LEN: usize = 64;
const MAX_DEVICE_NAME_LEN: usize = 100;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedDeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub status: String,
    pub paired_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl From<trusted_device::Model> for TrustedDeviceInfo {
    fn from(m: trusted_device::Model) -> Self {
        Self {
            device_id: m.device_id,
            device_name: m.device_name,
            status: m.status,
            paired_at: m.paired_at,
            revoked_at: m.revoked_at,
        }
    }
}

/// Device ids are minted by the device (a UUID in practice) and treated as
/// opaque; accept a bounded set of URL-safe characters.
pub fn is_valid_device_id(device_id: &str) -> bool {
    !device_id.is_empty()
        && device_id.len() <= MAX_DEVICE_ID_LEN
        && device_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn normalize_device_name(name: Option<&str>) -> String {
    name.unwrap_or_default()
        .trim()
        .chars()
        .take(MAX_DEVICE_NAME_LEN)
        .collect()
}

/// 256 bits from the OS RNG, URL-safe base64 (no padding).
pub(crate) fn generate_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn hash_credential(credential: &str) -> String {
    Sha256::digest(credential.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Mark `device_id` trusted and issue it a fresh bearer credential (returned
/// once, never stored). Re-pairing a known device — trusted or revoked —
/// re-trusts it and rotates its credential.
pub async fn register(
    conn: &DatabaseConnection,
    device_id: &str,
    device_name: &str,
) -> Result<String, DbError> {
    let credential = generate_secret();
    let now = Utc::now();
    trusted_device::Entity::insert(trusted_device::ActiveModel {
        id: NotSet,
        device_id: Set(device_id.to_string()),
        device_name: Set(device_name.to_string()),
        status: Set(STATUS_TRUSTED.to_string()),
        credential_hash: Set(Some(hash_credential(&credential))),
        paired_at: Set(now),
        revoked_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .on_conflict(
        OnConflict::column(trusted_device::Column::DeviceId)
            .update_columns([
                trusted_device::Column::DeviceName,
                trusted_device::Column::Status,
                trusted_device::Column::CredentialHash,
                trusted_device::Column::PairedAt,
                trusted_device::Column::RevokedAt,
                trusted_device::Column::UpdatedAt,
            ])
            .to_owned(),
    )
    .exec(conn)
    .await?;
    Ok(credential)
}

/// Resolve a presented bearer credential to the trusted device it belongs to.
pub async fn authenticate(
    conn: &DatabaseConnection,
    credential: &str,
) -> Result<Option<String>, DbError> {
    if credential.is_empty() {
        return Ok(None);
    }
    let device = trusted_device::Entity::find()
        .filter(trusted_device::Column::CredentialHash.eq(hash_credential(credential)))
        .filter(trusted_device::Column::Status.eq(STATUS_TRUSTED))
        .one(conn)
        .await?;
    Ok(device.map(|d| d.device_id))
}

pub async fn list(conn: &DatabaseConnection) -> Result<Vec<TrustedDeviceInfo>, DbError> {
    Ok(trusted_device::Entity::find()
        .order_by_desc(trusted_device::Column::PairedAt)
        .all(conn)
        .await?
        .into_iter()
        .map(TrustedDeviceInfo::from)
        .collect())
}

/// Revoke `device_id`. Returns `false` when the device is unknown.
pub async fn revoke(conn: &DatabaseConnection, device_id: &str) -> Result<bool, DbError> {
    let Some(device) = trusted_device::Entity::find()
        .filter(trusted_device::Column::DeviceId.eq(device_id))
        .one(conn)
        .await?
    else {
        return Ok(false);
    };
    let now = Utc::now();
    let mut active = device.into_active_model();
    active.status = Set(STATUS_REVOKED.to_string());
    active.credential_hash = Set(None);
    active.revoked_at = Set(Some(now));
    active.updated_at = Set(now);
    active.update(conn).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::fresh_in_memory_db;

    #[tokio::test]
    async fn registered_device_authenticates_until_revoked() {
        let db = fresh_in_memory_db().await;
        let credential = register(&db.conn, "dev-1", "Phone").await.unwrap();

        assert_eq!(
            authenticate(&db.conn, &credential)
                .await
                .unwrap()
                .as_deref(),
            Some("dev-1")
        );
        assert_eq!(authenticate(&db.conn, "wrong").await.unwrap(), None);
        assert_eq!(authenticate(&db.conn, "").await.unwrap(), None);

        assert!(revoke(&db.conn, "dev-1").await.unwrap());
        assert_eq!(authenticate(&db.conn, &credential).await.unwrap(), None);

        let devices = list(&db.conn).await.unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].status, STATUS_REVOKED);
        assert!(devices[0].revoked_at.is_some());
    }

    #[tokio::test]
    async fn revoking_an_unknown_device_reports_not_found() {
        let db = fresh_in_memory_db().await;
        assert!(!revoke(&db.conn, "nope").await.unwrap());
    }

    #[tokio::test]
    async fn re_pairing_rotates_the_credential_and_re_trusts() {
        let db = fresh_in_memory_db().await;
        let old = register(&db.conn, "dev-1", "Phone").await.unwrap();
        revoke(&db.conn, "dev-1").await.unwrap();

        let new = register(&db.conn, "dev-1", "Phone 2").await.unwrap();
        assert_ne!(old, new);
        assert_eq!(authenticate(&db.conn, &old).await.unwrap(), None);
        assert_eq!(
            authenticate(&db.conn, &new).await.unwrap().as_deref(),
            Some("dev-1")
        );

        let devices = list(&db.conn).await.unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].status, STATUS_TRUSTED);
        assert_eq!(devices[0].device_name, "Phone 2");
        assert_eq!(devices[0].revoked_at, None);
    }

    #[test]
    fn device_id_validation() {
        assert!(is_valid_device_id(&uuid::Uuid::new_v4().to_string()));
        assert!(!is_valid_device_id(""));
        assert!(!is_valid_device_id("has space"));
        assert!(!is_valid_device_id(&"a".repeat(MAX_DEVICE_ID_LEN + 1)));
    }
}
