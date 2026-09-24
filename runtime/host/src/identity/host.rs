//! Host identity: a persistent opaque `host_id`, minted on first
//! initialization and stored in `app_metadata` so it survives restarts, and
//! the account that owns the host, if one has claimed it.

use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use crate::db::error::DbError;
use crate::db::service::app_metadata_service;

const HOST_ID_KEY: &str = "host_id";
const HOST_OWNER_KEY: &str = "host_owner";

/// Return this host's `host_id`, generating and persisting it on first call.
pub async fn ensure_host_id(conn: &DatabaseConnection) -> Result<String, DbError> {
    if let Some(existing) = app_metadata_service::get_value(conn, HOST_ID_KEY).await? {
        return Ok(existing);
    }
    // Insert-if-absent then read back: concurrent first calls converge on
    // whichever id landed first instead of overwriting each other.
    let candidate = uuid::Uuid::new_v4().to_string();
    app_metadata_service::insert_if_absent(conn, HOST_ID_KEY, &candidate).await?;
    app_metadata_service::get_value(conn, HOST_ID_KEY)
        .await?
        .ok_or_else(|| DbError::NotFound(HOST_ID_KEY.to_string()))
}

/// The account a host belongs to: its identity-provider user id and email.
/// Never a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostOwner {
    pub user_id: String,
    pub email: Option<String>,
}

pub async fn owner(conn: &DatabaseConnection) -> Result<Option<HostOwner>, DbError> {
    Ok(app_metadata_service::get_value(conn, HOST_OWNER_KEY)
        .await?
        .and_then(|raw| serde_json::from_str(&raw).ok()))
}

pub enum Claim {
    /// The host now belongs to (or already belonged to) the claimant.
    Owned(HostOwner),
    /// Another account owns the host; ownership is never transferred.
    OwnedByOther(HostOwner),
}

/// Associate the host with account `claimant`. The first claim wins; the
/// owner re-claiming refreshes its email; anyone else is refused.
pub async fn claim(conn: &DatabaseConnection, claimant: HostOwner) -> Result<Claim, DbError> {
    let raw = serde_json::to_string(&claimant).map_err(|e| DbError::Validation(e.to_string()))?;
    app_metadata_service::insert_if_absent(conn, HOST_OWNER_KEY, &raw).await?;
    let current = owner(conn)
        .await?
        .ok_or_else(|| DbError::NotFound(HOST_OWNER_KEY.to_string()))?;
    if current.user_id != claimant.user_id {
        return Ok(Claim::OwnedByOther(current));
    }
    if current != claimant {
        app_metadata_service::upsert_value(conn, HOST_OWNER_KEY, &raw).await?;
    }
    Ok(Claim::Owned(claimant))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::fresh_in_memory_db;

    #[tokio::test]
    async fn host_id_is_generated_once_and_then_stable() {
        let db = fresh_in_memory_db().await;
        let first = ensure_host_id(&db.conn).await.unwrap();
        assert!(uuid::Uuid::parse_str(&first).is_ok());
        assert_eq!(ensure_host_id(&db.conn).await.unwrap(), first);
    }

    #[tokio::test]
    async fn host_id_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let first = {
            let db = crate::db::init_database(dir.path(), "test").await.unwrap();
            let id = ensure_host_id(&db.conn).await.unwrap();
            db.conn.close().await.unwrap();
            id
        };
        let db = crate::db::init_database(dir.path(), "test").await.unwrap();
        assert_eq!(ensure_host_id(&db.conn).await.unwrap(), first);
    }

    #[tokio::test]
    async fn first_claim_wins_and_is_never_transferred() {
        let db = fresh_in_memory_db().await;
        assert_eq!(owner(&db.conn).await.unwrap(), None);
        let alice = HostOwner {
            user_id: "u-alice".into(),
            email: Some("alice@example.com".into()),
        };
        assert!(matches!(claim(&db.conn, alice.clone()).await.unwrap(), Claim::Owned(_)));
        let bob = HostOwner {
            user_id: "u-bob".into(),
            email: None,
        };
        match claim(&db.conn, bob).await.unwrap() {
            Claim::OwnedByOther(o) => assert_eq!(o, alice),
            Claim::Owned(_) => panic!("ownership transferred"),
        }
        let renamed = HostOwner {
            email: Some("alice@new.example".into()),
            ..alice
        };
        assert!(matches!(claim(&db.conn, renamed.clone()).await.unwrap(), Claim::Owned(_)));
        assert_eq!(owner(&db.conn).await.unwrap(), Some(renamed));
    }
}
