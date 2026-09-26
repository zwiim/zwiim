//! The host's cryptographic identity: two long-lived keypairs with separate
//! purposes, minted once and reused across restarts.
//!
//! - **Signing key** (Ed25519): proves `host_id` to the Cloud control plane.
//! - **Static key** (X25519): the host's static key for the device tunnel.
//!   Devices pin its public half from the pairing QR.
//!
//! The private halves live in the OS keychain (service `zwiim`, one entry per
//! `host_id`). A headless host without a usable keychain opts into a private
//! file instead with `ZWIIM_HOST_KEY_STORE=file` (`<data_dir>/host-keys.json`,
//! mode `0600`). The public halves are recorded in `app_metadata`, so a
//! missing or different secret is reported instead of silently minting a new
//! identity that paired devices would no longer recognize.

use std::fmt;
use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::SigningKey;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::db::error::DbError;
use crate::db::service::app_metadata_service;

/// Selects the key store: `keychain` (default) or `file`.
pub const KEY_STORE_ENV: &str = "ZWIIM_HOST_KEY_STORE";
const KEYCHAIN_SERVICE: &str = "zwiim";
const KEY_FILE: &str = "host-keys.json";
const SIGN_PUBKEY_KEY: &str = "host_sign_pubkey";
const STATIC_PUBKEY_KEY: &str = "host_static_pubkey";
const STORED_VERSION: u32 = 1;

pub struct HostKeys {
    signing: SigningKey,
    static_secret: StaticSecret,
}

impl HostKeys {
    pub fn generate() -> Self {
        let mut rng = rand::rngs::OsRng;
        Self {
            signing: SigningKey::generate(&mut rng),
            static_secret: StaticSecret::random_from_rng(rng),
        }
    }

    /// Ed25519 public key, URL-safe base64 (no padding).
    pub fn sign_pubkey(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.signing.verifying_key().as_bytes())
    }

    /// X25519 public key, URL-safe base64 (no padding).
    pub fn static_pubkey(&self) -> String {
        URL_SAFE_NO_PAD.encode(PublicKey::from(&self.static_secret).as_bytes())
    }

    /// Sign with the Ed25519 identity key. The key authenticates the host to
    /// the Cloud control plane (`crate::cloud`) and nothing else.
    pub(crate) fn sign(&self, message: &[u8]) -> ed25519_dalek::Signature {
        use ed25519_dalek::Signer;
        self.signing.sign(message)
    }

    /// The X25519 static secret, for the tunnel handshake only.
    pub(crate) fn static_secret_bytes(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.static_secret.to_bytes())
    }

    fn to_stored(&self) -> Zeroizing<String> {
        let stored = StoredKeys {
            v: STORED_VERSION,
            sign_secret: URL_SAFE_NO_PAD.encode(self.signing.to_bytes()),
            static_secret: URL_SAFE_NO_PAD.encode(self.static_secret.to_bytes()),
        };
        Zeroizing::new(serde_json::to_string(&stored).expect("serializable"))
    }

    fn from_stored(raw: &str) -> Result<Self, HostKeyError> {
        let stored: StoredKeys = serde_json::from_str(raw).map_err(|_| HostKeyError::Corrupt)?;
        if stored.v != STORED_VERSION {
            return Err(HostKeyError::Corrupt);
        }
        Ok(Self {
            signing: SigningKey::from_bytes(&*secret_bytes(&stored.sign_secret)?),
            static_secret: StaticSecret::from(*secret_bytes(&stored.static_secret)?),
        })
    }
}

/// Only the public halves; the secrets never reach a log line.
impl fmt::Debug for HostKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostKeys")
            .field("sign_pubkey", &self.sign_pubkey())
            .field("static_pubkey", &self.static_pubkey())
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct StoredKeys {
    v: u32,
    sign_secret: String,
    static_secret: String,
}

fn secret_bytes(encoded: &str) -> Result<Zeroizing<[u8; 32]>, HostKeyError> {
    let decoded = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| HostKeyError::Corrupt)?,
    );
    let mut bytes = Zeroizing::new([0u8; 32]);
    if decoded.len() != bytes.len() {
        return Err(HostKeyError::Corrupt);
    }
    bytes.copy_from_slice(&decoded);
    Ok(bytes)
}

#[derive(Debug, thiserror::Error)]
pub enum HostKeyError {
    #[error("host key store: {0}")]
    Store(String),
    #[error("the stored host keys are unreadable")]
    Corrupt,
    #[error(
        "the host keys are missing from the {0} store, but this host already has an \
         identity; refusing to mint a new one"
    )]
    Missing(&'static str),
    #[error("the {0} store holds keys that do not match this host's recorded identity")]
    Mismatch(&'static str),
    #[error(transparent)]
    Db(#[from] DbError),
}

/// Where the private halves are kept.
#[derive(Clone, Debug)]
pub enum KeyStore {
    Keychain { account: String },
    File(PathBuf),
}

impl KeyStore {
    /// The store `ZWIIM_HOST_KEY_STORE` selects for host `host_id`.
    pub fn from_env(data_dir: &Path, host_id: &str) -> Result<Self, HostKeyError> {
        match std::env::var(KEY_STORE_ENV).ok().as_deref().map(str::trim) {
            None | Some("") | Some("keychain") => Ok(Self::keychain(host_id)),
            Some("file") => Ok(Self::File(data_dir.join(KEY_FILE))),
            Some(other) => Err(HostKeyError::Store(format!(
                "unknown {KEY_STORE_ENV} value `{other}` (expected `keychain` or `file`)"
            ))),
        }
    }

    pub fn keychain(host_id: &str) -> Self {
        Self::Keychain {
            account: format!("host-identity:{host_id}"),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Keychain { .. } => "keychain",
            Self::File(_) => "file",
        }
    }

    fn load(&self) -> Result<Option<Zeroizing<String>>, HostKeyError> {
        match self {
            Self::Keychain { account } => match keychain_entry(account)?.get_password() {
                Ok(raw) => Ok(Some(Zeroizing::new(raw))),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(HostKeyError::Store(e.to_string())),
            },
            Self::File(path) => match std::fs::read_to_string(path) {
                Ok(raw) => Ok(Some(Zeroizing::new(raw))),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(HostKeyError::Store(e.to_string())),
            },
        }
    }

    fn save(&self, raw: &str) -> Result<(), HostKeyError> {
        match self {
            Self::Keychain { account } => keychain_entry(account)?
                .set_password(raw)
                .map_err(|e| HostKeyError::Store(e.to_string())),
            Self::File(path) => {
                // Write-then-rename so a crash never leaves a truncated key file.
                let tmp = path.with_extension("tmp");
                super::device::write_private(&tmp, raw.as_bytes())
                    .and_then(|()| std::fs::rename(&tmp, path))
                    .map_err(|e| HostKeyError::Store(e.to_string()))
            }
        }
    }
}

fn keychain_entry(account: &str) -> Result<keyring::Entry, HostKeyError> {
    keyring::Entry::new(KEYCHAIN_SERVICE, account).map_err(|e| HostKeyError::Store(e.to_string()))
}

/// Keychain calls block (and may wait on a system prompt): run them off the
/// async workers.
async fn blocking<T: Send + 'static>(
    store: &KeyStore,
    f: impl FnOnce(&KeyStore) -> Result<T, HostKeyError> + Send + 'static,
) -> Result<T, HostKeyError> {
    let store = store.clone();
    tokio::task::spawn_blocking(move || f(&store))
        .await
        .map_err(|e| HostKeyError::Store(e.to_string()))?
}

/// Load this host's keys from `store`, minting and persisting them on first
/// boot only. A host whose identity is already recorded never mints again: a
/// missing or different secret is an error, not a new identity.
pub async fn load_or_create(
    conn: &DatabaseConnection,
    store: &KeyStore,
) -> Result<HostKeys, HostKeyError> {
    let recorded_sign = app_metadata_service::get_value(conn, SIGN_PUBKEY_KEY).await?;
    let recorded_static = app_metadata_service::get_value(conn, STATIC_PUBKEY_KEY).await?;
    let keys = match blocking(store, KeyStore::load).await? {
        Some(raw) => HostKeys::from_stored(&raw)?,
        None if recorded_sign.is_some() || recorded_static.is_some() => {
            return Err(HostKeyError::Missing(store.name()));
        }
        None => {
            let keys = HostKeys::generate();
            let raw = keys.to_stored();
            blocking(store, move |s| s.save(&raw)).await?;
            keys
        }
    };
    for (key, recorded, actual) in [
        (SIGN_PUBKEY_KEY, recorded_sign, keys.sign_pubkey()),
        (STATIC_PUBKEY_KEY, recorded_static, keys.static_pubkey()),
    ] {
        match recorded {
            Some(recorded) if recorded != actual => {
                return Err(HostKeyError::Mismatch(store.name()));
            }
            Some(_) => {}
            None => app_metadata_service::upsert_value(conn, key, &actual).await?,
        }
    }
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::fresh_in_memory_db;

    #[test]
    fn keys_have_separate_purposes_and_round_trip() {
        let keys = HostKeys::generate();
        assert_eq!(URL_SAFE_NO_PAD.decode(keys.sign_pubkey()).unwrap().len(), 32);
        assert_eq!(URL_SAFE_NO_PAD.decode(keys.static_pubkey()).unwrap().len(), 32);
        assert_ne!(keys.sign_pubkey(), keys.static_pubkey());
        assert_ne!(
            keys.signing.to_bytes(),
            keys.static_secret.to_bytes(),
            "one secret must not serve both purposes"
        );

        let restored = HostKeys::from_stored(&keys.to_stored()).unwrap();
        assert_eq!(restored.sign_pubkey(), keys.sign_pubkey());
        assert_eq!(restored.static_pubkey(), keys.static_pubkey());
        assert_ne!(HostKeys::generate().static_pubkey(), keys.static_pubkey());
    }

    #[test]
    fn debug_never_prints_a_secret() {
        let keys = HostKeys::generate();
        let debug = format!("{keys:?}");
        let stored: serde_json::Value = serde_json::from_str(&keys.to_stored()).unwrap();
        for secret in ["sign_secret", "static_secret"] {
            assert!(!debug.contains(stored[secret].as_str().unwrap()));
        }
        assert!(debug.contains(&keys.static_pubkey()));
    }

    #[test]
    fn corrupt_stores_are_rejected() {
        for raw in [
            "not json",
            r#"{"v":1,"sign_secret":"AAAA","static_secret":"AAAA"}"#,
            r#"{"v":2,"sign_secret":"","static_secret":""}"#,
        ] {
            assert!(matches!(HostKeys::from_stored(raw), Err(HostKeyError::Corrupt)));
        }
    }

    #[tokio::test]
    async fn keys_are_minted_once_and_reused_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let store = KeyStore::File(dir.path().join(KEY_FILE));
        let first = {
            let db = crate::db::init_database(dir.path(), "test").await.unwrap();
            let keys = load_or_create(&db.conn, &store).await.unwrap();
            db.conn.close().await.unwrap();
            keys
        };
        let db = crate::db::init_database(dir.path(), "test").await.unwrap();
        let again = load_or_create(&db.conn, &store).await.unwrap();
        assert_eq!(again.sign_pubkey(), first.sign_pubkey());
        assert_eq!(again.static_pubkey(), first.static_pubkey());
        assert_eq!(
            app_metadata_service::get_value(&db.conn, STATIC_PUBKEY_KEY)
                .await
                .unwrap(),
            Some(first.static_pubkey())
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn file_store_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let db = fresh_in_memory_db().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(KEY_FILE);
        load_or_create(&db.conn, &KeyStore::File(path.clone()))
            .await
            .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[tokio::test]
    async fn a_lost_secret_is_an_error_not_a_new_identity() {
        let db = fresh_in_memory_db().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(KEY_FILE);
        let store = KeyStore::File(path.clone());
        let keys = load_or_create(&db.conn, &store).await.unwrap();

        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            load_or_create(&db.conn, &store).await,
            Err(HostKeyError::Missing("file"))
        ));
        assert!(!path.exists(), "no replacement identity was minted");

        // A different secret in the store is refused too.
        store.save(&HostKeys::generate().to_stored()).unwrap();
        assert!(matches!(
            load_or_create(&db.conn, &store).await,
            Err(HostKeyError::Mismatch("file"))
        ));

        store.save(&keys.to_stored()).unwrap();
        let restored = load_or_create(&db.conn, &store).await.unwrap();
        assert_eq!(restored.static_pubkey(), keys.static_pubkey());
    }

    #[test]
    fn store_selection() {
        let dir = Path::new("/data");
        temp_env::with_var(KEY_STORE_ENV, None::<&str>, || {
            assert_eq!(KeyStore::from_env(dir, "h1").unwrap().name(), "keychain");
        });
        temp_env::with_var(KEY_STORE_ENV, Some("file"), || {
            match KeyStore::from_env(dir, "h1").unwrap() {
                KeyStore::File(path) => assert_eq!(path, dir.join(KEY_FILE)),
                other => panic!("{other:?}"),
            }
        });
        temp_env::with_var(KEY_STORE_ENV, Some("vault"), || {
            assert!(KeyStore::from_env(dir, "h1").is_err());
        });
        match KeyStore::keychain("h1") {
            KeyStore::Keychain { account } => assert_eq!(account, "host-identity:h1"),
            other => panic!("{other:?}"),
        }
    }

    /// Exercises the real OS keychain; run by hand (`--ignored`), since it
    /// writes to the user's login keychain.
    #[tokio::test]
    #[ignore]
    async fn keychain_store_round_trips() {
        let db = fresh_in_memory_db().await;
        let store = KeyStore::keychain(&format!("test-{}", uuid::Uuid::new_v4()));
        let keys = load_or_create(&db.conn, &store).await.unwrap();
        let again = load_or_create(&db.conn, &store).await.unwrap();
        assert_eq!(again.static_pubkey(), keys.static_pubkey());
        if let KeyStore::Keychain { account } = &store {
            keychain_entry(account).unwrap().delete_credential().unwrap();
        }
    }
}
