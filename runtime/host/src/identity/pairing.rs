//! One-time QR pairing.
//!
//! Key pairing (QR v2, `device.pair_invite` / `device.pair`):
//!
//! ```text
//! Host creates invitation ─► QR (host_id, host_static_pubkey, one-time code)
//!   ─► device scans, pins the host's static key
//!   ─► device pairs with its device_id + X25519 static public key
//!   ─► host marks the device trusted under that key; no secret is issued
//! ```
//!
//! The v2 QR carries no network address: remote devices reach the host through
//! the Cloud relay, and pair in the tunnel's handshake (`tunnel::host`).
//!
//! Legacy pairing (QR v1, `/api/pairing_*`) returns a per-device bearer
//! credential instead. It stays for existing callers only; key pairing is the
//! trust model the device tunnel builds on. An invitation is good for the
//! pairing it was created for and no other.
//!
//! Invitations live only in host memory, expire after [`INVITATION_TTL_SECS`],
//! and are consumed by the first claim attempt — so a QR is never a reusable
//! credential (and a host restart voids any outstanding ones).

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use super::trusted_devices;
use crate::app_error::AppCommandError;

pub const INVITATION_TTL_SECS: i64 = 300;
pub const PAIRING_PAYLOAD_VERSION: u32 = 1;
pub const KEY_PAIRING_PAYLOAD_VERSION: u32 = 2;

/// What a v2 QR encodes (as JSON).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyPairingInvitation {
    pub v: u32,
    pub host_id: String,
    /// The host's X25519 static public key, which the device pins.
    pub host_static_pubkey: String,
    pub code: String,
    pub expires_at: DateTime<Utc>,
}

/// `device.pair` parameters.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyPairingClaim {
    pub code: String,
    pub device_id: String,
    #[serde(default)]
    pub device_name: Option<String>,
    /// The device's X25519 static public key (URL-safe base64, no padding).
    pub device_pubkey: String,
}

/// `device.pair` result. The device's trust anchor is its own key, so nothing
/// secret is returned.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyPairingGrant {
    pub host_id: String,
    pub device_id: String,
}

/// What a v1 QR encodes (as JSON).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingInvitation {
    pub v: u32,
    pub host_id: String,
    pub code: String,
    /// Base URLs the host is reachable at, when known.
    pub addresses: Vec<String>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingClaim {
    pub code: String,
    pub device_id: String,
    #[serde(default)]
    pub device_name: Option<String>,
}

/// Returned to the device once, on a successful claim.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingGrant {
    pub host_id: String,
    pub device_id: String,
    /// Per-device bearer credential for the existing auth mechanism.
    pub credential: String,
}

/// The pairing an invitation was created for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InvitationKind {
    /// Legacy: the device receives a bearer credential.
    Credential,
    /// QR v2: the device registers its static public key.
    Key,
}

/// Outstanding invitations: one-time code → (kind, expiry).
#[derive(Default)]
pub struct PairingInvitations {
    pending: Mutex<HashMap<String, (InvitationKind, DateTime<Utc>)>>,
}

impl PairingInvitations {
    pub fn create(&self, host_id: String, addresses: Vec<String>) -> PairingInvitation {
        self.create_at(host_id, addresses, Utc::now())
    }

    fn create_at(
        &self,
        host_id: String,
        addresses: Vec<String>,
        now: DateTime<Utc>,
    ) -> PairingInvitation {
        let (code, expires_at) = self.insert(InvitationKind::Credential, now);
        PairingInvitation {
            v: PAIRING_PAYLOAD_VERSION,
            host_id,
            code,
            addresses,
            expires_at,
        }
    }

    pub fn create_for_key(&self, host_id: String, host_static_pubkey: String) -> KeyPairingInvitation {
        self.create_for_key_at(host_id, host_static_pubkey, Utc::now())
    }

    fn create_for_key_at(
        &self,
        host_id: String,
        host_static_pubkey: String,
        now: DateTime<Utc>,
    ) -> KeyPairingInvitation {
        let (code, expires_at) = self.insert(InvitationKind::Key, now);
        KeyPairingInvitation {
            v: KEY_PAIRING_PAYLOAD_VERSION,
            host_id,
            host_static_pubkey,
            code,
            expires_at,
        }
    }

    fn insert(&self, kind: InvitationKind, now: DateTime<Utc>) -> (String, DateTime<Utc>) {
        let code = trusted_devices::generate_secret();
        let expires_at = now + Duration::seconds(INVITATION_TTL_SECS);
        let mut pending = self.pending.lock().unwrap();
        pending.retain(|_, (_, exp)| *exp > now);
        pending.insert(code.clone(), (kind, expires_at));
        (code, expires_at)
    }

    /// Consume `code`. True only for a known, unexpired invitation of `kind`;
    /// the invitation is removed either way.
    fn consume_at(&self, code: &str, kind: InvitationKind, now: DateTime<Utc>) -> bool {
        self.pending
            .lock()
            .unwrap()
            .remove(code)
            .is_some_and(|(k, expires_at)| k == kind && now < expires_at)
    }
}

const INVITATION_REJECTED: &str = "Pairing invitation is invalid, expired, or already used";

/// Redeem an invitation: validate, consume it, and trust the device.
pub async fn claim(
    conn: &DatabaseConnection,
    invitations: &PairingInvitations,
    host_id: String,
    claim: PairingClaim,
) -> Result<PairingGrant, AppCommandError> {
    claim_at(conn, invitations, host_id, claim, Utc::now()).await
}

async fn claim_at(
    conn: &DatabaseConnection,
    invitations: &PairingInvitations,
    host_id: String,
    claim: PairingClaim,
    now: DateTime<Utc>,
) -> Result<PairingGrant, AppCommandError> {
    if !trusted_devices::is_valid_device_id(&claim.device_id) {
        return Err(AppCommandError::invalid_input("Invalid device id"));
    }
    if !invitations.consume_at(&claim.code, InvitationKind::Credential, now) {
        return Err(AppCommandError::permission_denied(INVITATION_REJECTED));
    }
    let device_name = trusted_devices::normalize_device_name(claim.device_name.as_deref());
    let credential = trusted_devices::register(conn, &claim.device_id, &device_name).await?;
    tracing::info!("[identity] device {} paired", claim.device_id);
    Ok(PairingGrant {
        host_id,
        device_id: claim.device_id,
        credential,
    })
}

/// Redeem a v2 invitation: validate, consume it, and trust the device under
/// its static public key.
pub async fn pair_device(
    conn: &DatabaseConnection,
    invitations: &PairingInvitations,
    host_id: String,
    claim: KeyPairingClaim,
) -> Result<KeyPairingGrant, AppCommandError> {
    pair_device_at(conn, invitations, host_id, claim, Utc::now()).await
}

async fn pair_device_at(
    conn: &DatabaseConnection,
    invitations: &PairingInvitations,
    host_id: String,
    claim: KeyPairingClaim,
    now: DateTime<Utc>,
) -> Result<KeyPairingGrant, AppCommandError> {
    // Malformed input is refused before the invitation is consumed.
    if !trusted_devices::is_valid_device_id(&claim.device_id) {
        return Err(AppCommandError::invalid_input("Invalid device id"));
    }
    let Some(device_pubkey) = trusted_devices::parse_device_pubkey(&claim.device_pubkey) else {
        return Err(AppCommandError::invalid_input("Invalid device public key"));
    };
    if let Some(owner) = trusted_devices::device_with_key(conn, &device_pubkey).await? {
        if owner != claim.device_id {
            return Err(AppCommandError::already_exists(
                "This device key is already paired as another device",
            ));
        }
    }
    if !invitations.consume_at(&claim.code, InvitationKind::Key, now) {
        return Err(AppCommandError::permission_denied(INVITATION_REJECTED));
    }
    let device_name = trusted_devices::normalize_device_name(claim.device_name.as_deref());
    trusted_devices::register_key(conn, &claim.device_id, &device_name, &device_pubkey).await?;
    tracing::info!("[identity] device {} paired by key", claim.device_id);
    Ok(KeyPairingGrant {
        host_id,
        device_id: claim.device_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_error::AppErrorCode;
    use crate::db::test_helpers::fresh_in_memory_db;

    fn claim_for(invitation: &PairingInvitation, device_id: &str) -> PairingClaim {
        PairingClaim {
            code: invitation.code.clone(),
            device_id: device_id.to_string(),
            device_name: Some("Phone".to_string()),
        }
    }

    fn key_claim(inv: &KeyPairingInvitation, device_id: &str, key: u8) -> KeyPairingClaim {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        KeyPairingClaim {
            code: inv.code.clone(),
            device_id: device_id.to_string(),
            device_name: Some("Phone".to_string()),
            device_pubkey: URL_SAFE_NO_PAD.encode([key; 32]),
        }
    }

    fn key_invitation(invitations: &PairingInvitations) -> KeyPairingInvitation {
        invitations.create_for_key("host".into(), "host-static".into())
    }

    #[tokio::test]
    async fn key_pairing_trusts_the_device_key_once() {
        let db = fresh_in_memory_db().await;
        let invitations = PairingInvitations::default();
        let inv = key_invitation(&invitations);
        assert_eq!(inv.v, KEY_PAIRING_PAYLOAD_VERSION);
        assert_eq!(inv.host_static_pubkey, "host-static");
        let payload = serde_json::to_value(&inv).unwrap();
        let mut fields: Vec<_> = payload.as_object().unwrap().keys().cloned().collect();
        fields.sort();
        assert_eq!(fields, ["code", "expires_at", "host_id", "host_static_pubkey", "v"]);

        let claim = key_claim(&inv, "dev-1", 1);
        let key = claim.device_pubkey.clone();
        let grant = pair_device(&db.conn, &invitations, "host".into(), claim).await.unwrap();
        assert_eq!((grant.host_id.as_str(), grant.device_id.as_str()), ("host", "dev-1"));
        assert_eq!(
            trusted_devices::authenticate_key(&db.conn, &key).await.unwrap().as_deref(),
            Some("dev-1")
        );

        // Reused code.
        let err = pair_device(&db.conn, &invitations, "host".into(), key_claim(&inv, "dev-2", 2))
            .await
            .unwrap_err();
        assert!(matches!(err.code, AppErrorCode::PermissionDenied));
        assert_eq!(
            trusted_devices::authenticate_key(&db.conn, &key_claim(&inv, "dev-2", 2).device_pubkey)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn expired_key_invitation_is_rejected() {
        let db = fresh_in_memory_db().await;
        let invitations = PairingInvitations::default();
        let now = Utc::now();
        let inv = invitations.create_for_key_at("host".into(), "k".into(), now);
        let later = now + Duration::seconds(INVITATION_TTL_SECS);
        let err = pair_device_at(&db.conn, &invitations, "host".into(), key_claim(&inv, "dev-1", 1), later)
            .await
            .unwrap_err();
        assert!(matches!(err.code, AppErrorCode::PermissionDenied));
    }

    #[tokio::test]
    async fn invitations_only_serve_their_own_pairing() {
        let db = fresh_in_memory_db().await;
        let invitations = PairingInvitations::default();

        // A v2 code cannot mint a bearer credential…
        let v2 = key_invitation(&invitations);
        let err = claim(&db.conn, &invitations, "host".into(), PairingClaim {
            code: v2.code.clone(),
            device_id: "dev-1".into(),
            device_name: None,
        })
        .await
        .unwrap_err();
        assert!(matches!(err.code, AppErrorCode::PermissionDenied));

        // …and a v1 code cannot register a key.
        let v1 = invitations.create("host".into(), vec![]);
        let mut via_v1 = key_claim(&v2, "dev-1", 1);
        via_v1.code = v1.code;
        let err = pair_device(&db.conn, &invitations, "host".into(), via_v1).await.unwrap_err();
        assert!(matches!(err.code, AppErrorCode::PermissionDenied));
    }

    #[tokio::test]
    async fn malformed_key_claims_keep_the_invitation() {
        let db = fresh_in_memory_db().await;
        let invitations = PairingInvitations::default();
        let inv = key_invitation(&invitations);

        let mut bad_key = key_claim(&inv, "dev-1", 1);
        bad_key.device_pubkey = "short".into();
        let err = pair_device(&db.conn, &invitations, "host".into(), bad_key).await.unwrap_err();
        assert!(matches!(err.code, AppErrorCode::InvalidInput));
        let err = pair_device(&db.conn, &invitations, "host".into(), key_claim(&inv, "bad id", 1))
            .await
            .unwrap_err();
        assert!(matches!(err.code, AppErrorCode::InvalidInput));

        // A key already paired as another device is refused, also without
        // burning the invitation.
        trusted_devices::register_key(&db.conn, "dev-0", "Old", &key_claim(&inv, "x", 1).device_pubkey)
            .await
            .unwrap();
        let err = pair_device(&db.conn, &invitations, "host".into(), key_claim(&inv, "dev-1", 1))
            .await
            .unwrap_err();
        assert!(matches!(err.code, AppErrorCode::AlreadyExists));

        pair_device(&db.conn, &invitations, "host".into(), key_claim(&inv, "dev-1", 2))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn invitation_is_single_use() {
        let db = fresh_in_memory_db().await;
        let invitations = PairingInvitations::default();
        let inv = invitations.create("host".into(), vec![]);

        let grant = claim(
            &db.conn,
            &invitations,
            "host".into(),
            claim_for(&inv, "dev-1"),
        )
        .await
        .unwrap();
        assert_eq!(grant.device_id, "dev-1");
        assert_eq!(grant.host_id, "host");

        let err = claim(
            &db.conn,
            &invitations,
            "host".into(),
            claim_for(&inv, "dev-2"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err.code, AppErrorCode::PermissionDenied));
    }

    #[tokio::test]
    async fn expired_invitation_is_rejected() {
        let db = fresh_in_memory_db().await;
        let invitations = PairingInvitations::default();
        let now = Utc::now();
        let inv = invitations.create_at("host".into(), vec![], now);

        let later = now + Duration::seconds(INVITATION_TTL_SECS);
        let err = claim_at(
            &db.conn,
            &invitations,
            "host".into(),
            claim_for(&inv, "dev-1"),
            later,
        )
        .await
        .unwrap_err();
        assert!(matches!(err.code, AppErrorCode::PermissionDenied));
    }

    #[tokio::test]
    async fn unknown_code_and_bad_device_id_are_rejected() {
        let db = fresh_in_memory_db().await;
        let invitations = PairingInvitations::default();
        let inv = invitations.create("host".into(), vec![]);

        let mut bogus = claim_for(&inv, "dev-1");
        bogus.code = "not-a-code".into();
        let err = claim(&db.conn, &invitations, "host".into(), bogus)
            .await
            .unwrap_err();
        assert!(matches!(err.code, AppErrorCode::PermissionDenied));

        // A malformed device id is rejected without burning the invitation.
        let err = claim(
            &db.conn,
            &invitations,
            "host".into(),
            claim_for(&inv, "bad id"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err.code, AppErrorCode::InvalidInput));
        claim(
            &db.conn,
            &invitations,
            "host".into(),
            claim_for(&inv, "dev-1"),
        )
        .await
        .unwrap();
    }
}
