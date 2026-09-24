//! One-time QR pairing.
//!
//! ```text
//! Host creates invitation ─► QR payload (host_id, one-time code, addresses)
//!   ─► device scans ─► device claims with its device_id
//!   ─► host marks device trusted, returns a per-device bearer credential
//! ```
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

/// What the QR encodes (as JSON).
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

/// Outstanding invitations: one-time code → expiry.
#[derive(Default)]
pub struct PairingInvitations {
    pending: Mutex<HashMap<String, DateTime<Utc>>>,
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
        let code = trusted_devices::generate_secret();
        let expires_at = now + Duration::seconds(INVITATION_TTL_SECS);
        let mut pending = self.pending.lock().unwrap();
        pending.retain(|_, exp| *exp > now);
        pending.insert(code.clone(), expires_at);
        PairingInvitation {
            v: PAIRING_PAYLOAD_VERSION,
            host_id,
            code,
            addresses,
            expires_at,
        }
    }

    /// Consume `code`. True only for a known, unexpired invitation; the
    /// invitation is removed either way.
    fn consume_at(&self, code: &str, now: DateTime<Utc>) -> bool {
        self.pending
            .lock()
            .unwrap()
            .remove(code)
            .is_some_and(|expires_at| now < expires_at)
    }
}

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
    if !invitations.consume_at(&claim.code, now) {
        return Err(AppCommandError::permission_denied(
            "Pairing invitation is invalid, expired, or already used",
        ));
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
