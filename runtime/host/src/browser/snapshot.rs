//! Sign-ins kept between engines (design 6.3): the personal context's, and
//! each workspace's saved login that its sessions start from.
//!
//! The engine's contexts are ephemeral and the headless shell protects its
//! own cookie file with Chromium's built-in mock password, so the Host keeps
//! the cookies itself: sealed with AES-256-GCM under a per-Host key held in
//! the keychain (or the file key store a headless Host opts into), and only
//! the ciphertext written to disk.
//!
//! File layout: `ZWBS` · version 1 · 12-byte nonce · ciphertext and tag.

use std::io::Write;
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

const MAGIC: &[u8; 4] = b"ZWBS";
const VERSION: u8 = 1;
const NONCE_LEN: usize = 12;
/// The keychain secret holding the key.
const KEY_SECRET: &str = "browser-snapshot-key";
/// Binds a sealed file to what it is, so it cannot be passed off as another.
const AAD: &[u8] = b"zwiim browser personal snapshot v1";
/// The same for a workspace's login, followed by the workspace's id.
const LOGIN_AAD: &str = "zwiim browser workspace login v1 ";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotError {
    #[error("the browser snapshot key is unavailable: {0}")]
    Key(String),
    #[error("the browser snapshot cannot be read")]
    Corrupt,
    #[error("{0}")]
    Io(String),
}

impl From<std::io::Error> for SnapshotError {
    fn from(e: std::io::Error) -> Self {
        SnapshotError::Io(e.to_string())
    }
}

/// What a snapshot holds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Contents {
    /// CDP `Network.CookieParam`s, ready for `Storage.setCookies`.
    pub cookies: Vec<Value>,
    /// For a workspace login, the sites the person chose to keep.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<String>,
}

#[derive(Clone)]
enum KeySource {
    Keychain,
    #[cfg(test)]
    Fixed([u8; 32]),
}

/// A snapshot file and the key that seals it.
pub struct Snapshot {
    path: PathBuf,
    aad: Vec<u8>,
    source: KeySource,
    key: std::sync::OnceLock<Zeroizing<[u8; 32]>>,
}

impl Snapshot {
    /// The Host's snapshot: `<data>/browser/personal.snapshot`, key in the
    /// keychain.
    pub fn at(path: PathBuf) -> Self {
        Snapshot {
            path,
            aad: AAD.to_vec(),
            source: KeySource::Keychain,
            key: std::sync::OnceLock::new(),
        }
    }

    #[cfg(test)]
    pub fn with_key(path: PathBuf, key: [u8; 32]) -> Self {
        Snapshot {
            path,
            aad: AAD.to_vec(),
            source: KeySource::Fixed(key),
            key: std::sync::OnceLock::new(),
        }
    }

    /// Workspace `workspace_id`'s saved login, beside this snapshot under
    /// `logins/`, sealed by the same key and bound to that workspace. None
    /// for an id that is not a plain name.
    pub fn login(&self, workspace_id: &str) -> Option<Snapshot> {
        let plain = !workspace_id.is_empty()
            && workspace_id.len() <= 64
            && workspace_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !plain {
            return None;
        }
        let dir = self.path.parent().unwrap_or(Path::new("."));
        let key = std::sync::OnceLock::new();
        if let Some(k) = self.key.get() {
            let _ = key.set(k.clone());
        }
        Some(Snapshot {
            path: dir.join("logins").join(format!("{workspace_id}.snapshot")),
            aad: format!("{LOGIN_AAD}{workspace_id}").into_bytes(),
            source: self.source.clone(),
            key,
        })
    }

    /// Read and open the snapshot. No file is an empty snapshot.
    pub fn load(&self) -> Result<Contents, SnapshotError> {
        let sealed = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Contents::default()),
            Err(e) => return Err(e.into()),
        };
        let plain = Zeroizing::new(open(&*self.key()?, &self.aad, &sealed)?);
        serde_json::from_slice(&plain).map_err(|_| SnapshotError::Corrupt)
    }

    /// Seal and write the snapshot, replacing the old one atomically.
    pub fn save(&self, contents: &Contents) -> Result<(), SnapshotError> {
        let plain = Zeroizing::new(
            serde_json::to_vec(contents).map_err(|e| SnapshotError::Io(e.to_string()))?,
        );
        let sealed = seal(&*self.key()?, &self.aad, &plain);
        write_private(&self.path, &sealed)
    }

    /// Remove the snapshot. No file is fine.
    pub fn remove(&self) -> Result<(), SnapshotError> {
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    fn key(&self) -> Result<Zeroizing<[u8; 32]>, SnapshotError> {
        if let Some(key) = self.key.get() {
            return Ok(key.clone());
        }
        let key = match &self.source {
            KeySource::Keychain => keychain_key()?,
            #[cfg(test)]
            KeySource::Fixed(key) => Zeroizing::new(*key),
        };
        Ok(self.key.get_or_init(|| key).clone())
    }
}

/// The key from the keychain, made on first use. A keychain that will not
/// open is an error, never a new key: that would orphan the old snapshot.
fn keychain_key() -> Result<Zeroizing<[u8; 32]>, SnapshotError> {
    if let Some(stored) =
        crate::keyring_store::get_secret(KEY_SECRET).map_err(SnapshotError::Key)?
    {
        let bytes = Zeroizing::new(
            B64.decode(stored.as_bytes())
                .map_err(|_| SnapshotError::Corrupt)?,
        );
        let key: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| SnapshotError::Corrupt)?;
        return Ok(Zeroizing::new(key));
    }
    let mut key = Zeroizing::new([0u8; 32]);
    rand::rngs::OsRng.fill_bytes(key.as_mut());
    let encoded = Zeroizing::new(B64.encode(key.as_ref()));
    crate::keyring_store::set_secret(KEY_SECRET, &encoded).map_err(SnapshotError::Key)?;
    Ok(key)
}

fn seal(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Vec<u8> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let sealed = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad })
        .expect("AES-GCM encryption of an in-memory buffer");
    let mut out = Vec::with_capacity(MAGIC.len() + 1 + NONCE_LEN + sealed.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    out
}

fn open(key: &[u8; 32], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, SnapshotError> {
    let header = MAGIC.len() + 1;
    if sealed.len() < header + NONCE_LEN
        || &sealed[..MAGIC.len()] != MAGIC
        || sealed[MAGIC.len()] != VERSION
    {
        return Err(SnapshotError::Corrupt);
    }
    let (nonce, body) = sealed[header..].split_at(NONCE_LEN);
    Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
        .decrypt(Nonce::from_slice(nonce), Payload { msg: body, aad })
        .map_err(|_| SnapshotError::Corrupt)
}

/// Write `bytes` to `path` readable by this user only, by rename.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), SnapshotError> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("snapshot.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// A cookie as `Storage.getCookies` reports it, made into the parameter
/// `Storage.setCookies` takes: only the fields that parameter has.
pub fn cookie_param(cookie: &Value) -> Option<Value> {
    const KEEP: &[&str] = &[
        "name",
        "value",
        "domain",
        "path",
        "secure",
        "httpOnly",
        "sameSite",
        "priority",
        "sourceScheme",
        "sourcePort",
        "partitionKey",
    ];
    let object = cookie.as_object()?;
    let mut param = serde_json::Map::new();
    for key in KEEP {
        if let Some(v) = object.get(*key) {
            param.insert((*key).to_string(), v.clone());
        }
    }
    // Session cookies have no expiry; keeping them keeps the person signed
    // in across engine restarts, as "continue where you left off" does.
    if object.get("session") != Some(&Value::Bool(true)) {
        if let Some(expires) = object.get("expires") {
            param.insert("expires".into(), expires.clone());
        }
    }
    param.contains_key("name").then_some(Value::Object(param))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn contents() -> Contents {
        Contents {
            cookies: vec![
                json!({ "name": "sid", "value": "secret-session-value", "domain": "example.com", "path": "/" }),
            ],
            sites: Vec::new(),
        }
    }

    #[test]
    fn a_workspace_login_is_its_own_file_bound_to_its_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let personal = Snapshot::with_key(dir.path().join("personal.snapshot"), [7; 32]);
        assert!(personal.login("../x").is_none());
        assert!(personal.login("").is_none());
        let login = personal.login("w1").unwrap();
        let saved = Contents {
            sites: vec!["example.com".into()],
            ..contents()
        };
        login.save(&saved).unwrap();
        assert!(dir.path().join("logins/w1.snapshot").exists());
        assert_eq!(login.load().unwrap(), saved);
        assert_eq!(personal.load().unwrap(), Contents::default());

        // Copied to another workspace's name, it does not open.
        std::fs::copy(
            dir.path().join("logins/w1.snapshot"),
            dir.path().join("logins/w2.snapshot"),
        )
        .unwrap();
        assert_eq!(
            personal.login("w2").unwrap().load().unwrap_err(),
            SnapshotError::Corrupt
        );
        login.remove().unwrap();
        login.remove().unwrap();
        assert_eq!(login.load().unwrap(), Contents::default());
    }

    #[test]
    fn a_snapshot_round_trips_and_is_not_readable_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("browser/personal.snapshot");
        let snapshot = Snapshot::with_key(path.clone(), [7; 32]);
        assert_eq!(snapshot.load().unwrap(), Contents::default());

        snapshot.save(&contents()).unwrap();
        let on_disk = std::fs::read(&path).unwrap();
        assert!(on_disk.starts_with(MAGIC));
        assert!(!String::from_utf8_lossy(&on_disk).contains("secret-session-value"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(snapshot.load().unwrap(), contents());
    }

    #[test]
    fn another_key_or_a_changed_byte_does_not_open_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("personal.snapshot");
        Snapshot::with_key(path.clone(), [7; 32])
            .save(&contents())
            .unwrap();
        assert_eq!(
            Snapshot::with_key(path.clone(), [8; 32])
                .load()
                .unwrap_err(),
            SnapshotError::Corrupt
        );

        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(
            Snapshot::with_key(path, [7; 32]).load().unwrap_err(),
            SnapshotError::Corrupt
        );
    }

    #[test]
    fn cookies_become_set_cookie_parameters() {
        let got = json!({
            "name": "sid", "value": "v", "domain": ".example.com", "path": "/",
            "expires": 1_900_000_000.0, "size": 4, "httpOnly": true, "secure": true,
            "session": false, "sameSite": "Lax", "priority": "Medium", "sameParty": false,
            "sourceScheme": "Secure", "sourcePort": 443
        });
        let param = cookie_param(&got).unwrap();
        assert_eq!(param["expires"], 1_900_000_000.0);
        assert!(param.get("size").is_none() && param.get("session").is_none());
        assert!(param.get("sameParty").is_none());

        let session = json!({ "name": "s", "value": "v", "domain": "x", "path": "/", "expires": -1, "session": true });
        assert!(cookie_param(&session).unwrap().get("expires").is_none());
    }
}
