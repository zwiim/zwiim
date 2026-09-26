//! Host-side trusted-device registry. The host is the final authority on
//! which devices it trusts.
//!
//! A device becomes trusted by pairing (see `pairing`). Key pairing registers
//! the device's X25519 static public key, which the device tunnel
//! authenticates against ([`authenticate_key`]). The legacy `/api` pairing
//! instead issues a random bearer credential, of which the host stores only a
//! SHA-256. Revocation flips the status (and clears any credential hash), so
//! the device stops authenticating immediately, and is announced on
//! [`Revocations`] so its open connections are closed.

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
use tokio::sync::broadcast;

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

/// A device as the Zwiim protocol lists it (`device.list`).
#[derive(Clone, Debug, Serialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    /// `trusted` or `revoked`.
    pub status: String,
    /// The device's X25519 static public key; `null` for a device paired with
    /// a legacy bearer credential.
    pub device_pubkey: Option<String>,
    pub paired_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_seen_at: Option<DateTime<Utc>>,
}

impl From<trusted_device::Model> for DeviceInfo {
    fn from(m: trusted_device::Model) -> Self {
        Self {
            device_id: m.device_id,
            device_name: m.device_name,
            status: m.status,
            device_pubkey: m.device_pubkey,
            paired_at: m.paired_at,
            revoked_at: m.revoked_at,
            last_seen_at: m.last_seen_at,
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

/// Parse a device's X25519 static public key (32 bytes, URL-safe base64 without
/// padding) into its canonical encoding. The all-zero key is refused.
pub fn parse_device_pubkey(encoded: &str) -> Option<String> {
    let bytes = URL_SAFE_NO_PAD.decode(encoded.trim()).ok()?;
    if bytes.len() != 32 || bytes.iter().all(|b| *b == 0) {
        return None;
    }
    Some(URL_SAFE_NO_PAD.encode(bytes))
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
        device_pubkey: Set(None),
        paired_at: Set(now),
        revoked_at: Set(None),
        last_seen_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .on_conflict(
        OnConflict::column(trusted_device::Column::DeviceId)
            .update_columns([
                trusted_device::Column::DeviceName,
                trusted_device::Column::Status,
                trusted_device::Column::CredentialHash,
                trusted_device::Column::DevicePubkey,
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

/// Trust `device_id` by its static public key `device_pubkey` (canonical, see
/// [`parse_device_pubkey`]). Re-pairing a known device — trusted, revoked, or
/// paired with a bearer credential — re-trusts it under the new key and drops
/// any bearer credential it held.
pub async fn register_key(
    conn: &DatabaseConnection,
    device_id: &str,
    device_name: &str,
    device_pubkey: &str,
) -> Result<(), DbError> {
    let now = Utc::now();
    trusted_device::Entity::insert(trusted_device::ActiveModel {
        id: NotSet,
        device_id: Set(device_id.to_string()),
        device_name: Set(device_name.to_string()),
        status: Set(STATUS_TRUSTED.to_string()),
        credential_hash: Set(None),
        device_pubkey: Set(Some(device_pubkey.to_string())),
        paired_at: Set(now),
        revoked_at: Set(None),
        last_seen_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .on_conflict(
        OnConflict::column(trusted_device::Column::DeviceId)
            .update_columns([
                trusted_device::Column::DeviceName,
                trusted_device::Column::Status,
                trusted_device::Column::CredentialHash,
                trusted_device::Column::DevicePubkey,
                trusted_device::Column::PairedAt,
                trusted_device::Column::RevokedAt,
                trusted_device::Column::UpdatedAt,
            ])
            .to_owned(),
    )
    .exec(conn)
    .await?;
    Ok(())
}

/// The device, if any, that `device_pubkey` is registered to.
pub async fn device_with_key(
    conn: &DatabaseConnection,
    device_pubkey: &str,
) -> Result<Option<String>, DbError> {
    Ok(trusted_device::Entity::find()
        .filter(trusted_device::Column::DevicePubkey.eq(device_pubkey))
        .one(conn)
        .await?
        .map(|d| d.device_id))
}

/// Resolve a device's static public key to the trusted device it belongs to.
pub async fn authenticate_key(
    conn: &DatabaseConnection,
    device_pubkey: &str,
) -> Result<Option<String>, DbError> {
    let Some(device_pubkey) = parse_device_pubkey(device_pubkey) else {
        return Ok(None);
    };
    let device = trusted_device::Entity::find()
        .filter(trusted_device::Column::DevicePubkey.eq(device_pubkey))
        .filter(trusted_device::Column::Status.eq(STATUS_TRUSTED))
        .one(conn)
        .await?;
    Ok(device.map(|d| d.device_id))
}

pub async fn is_trusted(conn: &DatabaseConnection, device_id: &str) -> Result<bool, DbError> {
    Ok(trusted_device::Entity::find()
        .filter(trusted_device::Column::DeviceId.eq(device_id))
        .filter(trusted_device::Column::Status.eq(STATUS_TRUSTED))
        .one(conn)
        .await?
        .is_some())
}

/// How stale `last_seen_at` may get before an authentication refreshes it,
/// so a busy device is not a write per request.
const LAST_SEEN_GRANULARITY_SECS: i64 = 60;

/// Record that `device_id` just authenticated.
pub async fn mark_seen(conn: &DatabaseConnection, device_id: &str) -> Result<(), DbError> {
    use sea_orm::sea_query::Condition;
    let now = Utc::now();
    let stale = now - chrono::Duration::seconds(LAST_SEEN_GRANULARITY_SECS);
    trusted_device::Entity::update_many()
        .col_expr(
            trusted_device::Column::LastSeenAt,
            sea_orm::sea_query::Expr::value(now),
        )
        .filter(trusted_device::Column::DeviceId.eq(device_id))
        .filter(
            Condition::any()
                .add(trusted_device::Column::LastSeenAt.is_null())
                .add(trusted_device::Column::LastSeenAt.lt(stale)),
        )
        .exec(conn)
        .await?;
    Ok(())
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

pub async fn list<T: From<trusted_device::Model>>(
    conn: &DatabaseConnection,
) -> Result<Vec<T>, DbError> {
    Ok(trusted_device::Entity::find()
        .order_by_desc(trusted_device::Column::PairedAt)
        .all(conn)
        .await?
        .into_iter()
        .map(T::from)
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

/// Announces revoked devices to the connections they hold open, so a
/// revocation takes effect on live sockets, not only on the next request.
#[derive(Clone)]
pub struct Revocations {
    tx: broadcast::Sender<String>,
}

impl Default for Revocations {
    fn default() -> Self {
        Self {
            tx: broadcast::channel(64).0,
        }
    }
}

impl Revocations {
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    pub fn announce(&self, device_id: &str) {
        let _ = self.tx.send(device_id.to_string());
    }
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

        let devices = list::<TrustedDeviceInfo>(&db.conn).await.unwrap();
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

        let devices = list::<TrustedDeviceInfo>(&db.conn).await.unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].status, STATUS_TRUSTED);
        assert_eq!(devices[0].device_name, "Phone 2");
        assert_eq!(devices[0].revoked_at, None);
    }

    fn pubkey(byte: u8) -> String {
        URL_SAFE_NO_PAD.encode([byte; 32])
    }

    #[tokio::test]
    async fn key_registered_device_authenticates_until_revoked() {
        let db = fresh_in_memory_db().await;
        register_key(&db.conn, "dev-1", "Phone", &pubkey(1)).await.unwrap();

        assert_eq!(
            authenticate_key(&db.conn, &pubkey(1)).await.unwrap().as_deref(),
            Some("dev-1")
        );
        assert!(is_trusted(&db.conn, "dev-1").await.unwrap());
        // Unknown and malformed keys do not authenticate.
        assert_eq!(authenticate_key(&db.conn, &pubkey(2)).await.unwrap(), None);
        assert_eq!(authenticate_key(&db.conn, "junk").await.unwrap(), None);
        assert!(!is_trusted(&db.conn, "dev-unknown").await.unwrap());

        assert!(revoke(&db.conn, "dev-1").await.unwrap());
        assert_eq!(authenticate_key(&db.conn, &pubkey(1)).await.unwrap(), None);
        assert!(!is_trusted(&db.conn, "dev-1").await.unwrap());

        let devices = list::<DeviceInfo>(&db.conn).await.unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].status, STATUS_REVOKED);
        assert_eq!(devices[0].device_pubkey.as_deref(), Some(pubkey(1).as_str()));
        assert!(devices[0].revoked_at.is_some());

        // Revocation is persistent: re-trusting takes a new pairing.
        register_key(&db.conn, "dev-1", "Phone", &pubkey(3)).await.unwrap();
        assert_eq!(authenticate_key(&db.conn, &pubkey(1)).await.unwrap(), None);
        assert_eq!(
            authenticate_key(&db.conn, &pubkey(3)).await.unwrap().as_deref(),
            Some("dev-1")
        );
    }

    #[tokio::test]
    async fn key_pairing_replaces_a_bearer_credential() {
        let db = fresh_in_memory_db().await;
        let credential = register(&db.conn, "dev-1", "Phone").await.unwrap();
        register_key(&db.conn, "dev-1", "Phone", &pubkey(1)).await.unwrap();
        assert_eq!(authenticate(&db.conn, &credential).await.unwrap(), None);
        assert_eq!(
            authenticate_key(&db.conn, &pubkey(1)).await.unwrap().as_deref(),
            Some("dev-1")
        );
        assert_eq!(
            device_with_key(&db.conn, &pubkey(1)).await.unwrap().as_deref(),
            Some("dev-1")
        );
    }

    #[tokio::test]
    async fn last_seen_is_recorded() {
        let db = fresh_in_memory_db().await;
        register_key(&db.conn, "dev-1", "Phone", &pubkey(1)).await.unwrap();
        assert_eq!(list::<DeviceInfo>(&db.conn).await.unwrap()[0].last_seen_at, None);
        mark_seen(&db.conn, "dev-1").await.unwrap();
        let seen = list::<DeviceInfo>(&db.conn).await.unwrap()[0].last_seen_at;
        assert!(seen.is_some());
        // Within the granularity window the timestamp is left alone.
        mark_seen(&db.conn, "dev-1").await.unwrap();
        assert_eq!(list::<DeviceInfo>(&db.conn).await.unwrap()[0].last_seen_at, seen);
    }

    #[test]
    fn device_pubkey_validation() {
        assert_eq!(parse_device_pubkey(&pubkey(7)), Some(pubkey(7)));
        assert_eq!(parse_device_pubkey(&format!(" {} ", pubkey(7))), Some(pubkey(7)));
        assert_eq!(parse_device_pubkey(&pubkey(0)), None);
        assert_eq!(parse_device_pubkey(&URL_SAFE_NO_PAD.encode([1u8; 31])), None);
        assert_eq!(parse_device_pubkey("not base64!"), None);
    }

    #[tokio::test]
    async fn revocations_reach_every_subscriber() {
        let revocations = Revocations::default();
        let (mut a, mut b) = (revocations.subscribe(), revocations.subscribe());
        revocations.announce("dev-1");
        assert_eq!(a.recv().await.unwrap(), "dev-1");
        assert_eq!(b.recv().await.unwrap(), "dev-1");
    }

    #[test]
    fn device_id_validation() {
        assert!(is_valid_device_id(&uuid::Uuid::new_v4().to_string()));
        assert!(!is_valid_device_id(""));
        assert!(!is_valid_device_id("has space"));
        assert!(!is_valid_device_id(&"a".repeat(MAX_DEVICE_ID_LEN + 1)));
    }
}
