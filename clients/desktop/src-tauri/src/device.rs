//! This desktop's device identity, for reaching hosts through Zwiim Cloud.
//!
//! One X25519 static key per installation, created on first use, and the
//! hosts it has paired with — each pinned to the static key its pairing QR
//! carried. All of it lives in one OS keychain item; never in SQLite, web
//! storage, config files or logs.
//!
//! The web UI runs the device's end of the encrypted tunnel (the SDK's
//! Noise handshake), so it is handed the secret key in memory when it opens
//! one; it never stores it. Account sign-in is separate: signing out keeps
//! this identity and its pairings.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

const KEYCHAIN_SERVICE: &str = "dev.zwiim.desktop";
const KEYCHAIN_ACCOUNT: &str = "device-identity";

/// A host this device has paired with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedHost {
    pub host_id: String,
    /// The key the pairing QR carried: the only key this device accepts from
    /// the host.
    pub host_static_pubkey: String,
    pub name: Option<String>,
    /// Unix seconds.
    pub paired_at: u64,
}

/// The identity as kept in the keychain.
#[derive(Clone, Serialize, Deserialize)]
pub struct DeviceIdentity {
    pub device_id: String,
    pub device_name: String,
    /// The X25519 static secret, unpadded base64url.
    pub secret: String,
    #[serde(default)]
    pub hosts: Vec<PairedHost>,
}

/// Never prints the secret.
impl std::fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceIdentity")
            .field("device_id", &self.device_id)
            .field("device_name", &self.device_name)
            .field("hosts", &self.hosts)
            .finish_non_exhaustive()
    }
}

/// Where the identity lives: the OS keychain in the app, memory in tests.
pub trait DeviceStore: Send + Sync {
    fn load(&self) -> Result<Option<DeviceIdentity>, String>;
    fn save(&self, identity: &DeviceIdentity) -> Result<(), String>;
}

pub struct Keychain;

impl Keychain {
    fn entry() -> Result<keyring::Entry, String> {
        keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).map_err(|e| format!("Keychain unavailable: {e}"))
    }
}

impl DeviceStore for Keychain {
    fn load(&self) -> Result<Option<DeviceIdentity>, String> {
        match Self::entry()?.get_password() {
            // Unreadable is an error, not a new identity: that would orphan
            // every pairing.
            Ok(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|_| "The device identity in the keychain is damaged".to_string()),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("Cannot read the keychain: {e}")),
        }
    }
    fn save(&self, identity: &DeviceIdentity) -> Result<(), String> {
        let raw = serde_json::to_string(identity).map_err(|e| e.to_string())?;
        Self::entry()?
            .set_password(&raw)
            .map_err(|e| format!("Cannot write the keychain: {e}"))
    }
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut bytes);
    bytes
}

/// This computer's name, as the host lists the device.
fn computer_name() -> String {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0 {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            let name = String::from_utf8_lossy(&buf[..end]);
            let name = name.trim().trim_end_matches(".local");
            if !name.is_empty() {
                return name.chars().take(64).collect();
            }
        }
    }
    "Zwiim Desktop".into()
}

fn valid_key(key: &str) -> bool {
    key.len() == 43 && URL_SAFE_NO_PAD.decode(key).is_ok_and(|b| b.len() == 32)
}

fn valid_host_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The device identity, created and kept on first use.
pub fn identity(store: &dyn DeviceStore) -> Result<DeviceIdentity, String> {
    if let Some(identity) = store.load()? {
        return Ok(identity);
    }
    let id: [u8; 8] = random_bytes();
    let identity = DeviceIdentity {
        device_id: format!("desktop-{}", id.iter().map(|b| format!("{b:02x}")).collect::<String>()),
        device_name: computer_name(),
        secret: URL_SAFE_NO_PAD.encode(random_bytes::<32>()),
        hosts: Vec::new(),
    };
    store.save(&identity)?;
    Ok(identity)
}

/// The hosts this device has paired with.
pub fn paired_hosts(store: &dyn DeviceStore) -> Result<Vec<PairedHost>, String> {
    Ok(store.load()?.map(|i| i.hosts).unwrap_or_default())
}

/// Record a completed pairing, replacing any earlier one with the host.
pub fn pin_host(store: &dyn DeviceStore, host: PairedHost) -> Result<(), String> {
    if !valid_host_id(&host.host_id) || !valid_key(&host.host_static_pubkey) {
        return Err("Invalid host identity".into());
    }
    let mut identity = identity(store)?;
    identity.hosts.retain(|h| h.host_id != host.host_id);
    identity.hosts.push(host);
    store.save(&identity)
}

/// Forget a pairing (the host revoked this device, or the user removed it).
pub fn forget_host(store: &dyn DeviceStore, host_id: &str) -> Result<(), String> {
    let Some(mut identity) = store.load()? else {
        return Ok(());
    };
    identity.hosts.retain(|h| h.host_id != host_id);
    store.save(&identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Memory(Mutex<Option<String>>);
    // Round-trips through JSON, as the keychain does.
    impl DeviceStore for Memory {
        fn load(&self) -> Result<Option<DeviceIdentity>, String> {
            Ok(self.0.lock().unwrap().as_deref().map(|s| serde_json::from_str(s).unwrap()))
        }
        fn save(&self, identity: &DeviceIdentity) -> Result<(), String> {
            *self.0.lock().unwrap() = Some(serde_json::to_string(identity).unwrap());
            Ok(())
        }
    }

    fn host(id: &str, key: u8) -> PairedHost {
        PairedHost {
            host_id: id.into(),
            host_static_pubkey: URL_SAFE_NO_PAD.encode([key; 32]),
            name: Some("Studio".into()),
            paired_at: 1,
        }
    }

    #[test]
    fn the_identity_is_created_once_and_kept() {
        let store = Memory::default();
        let first = identity(&store).unwrap();
        assert!(first.device_id.starts_with("desktop-") && first.device_id.len() == 24);
        assert_eq!(URL_SAFE_NO_PAD.decode(&first.secret).unwrap().len(), 32);
        assert!(!first.device_name.is_empty());
        // As after a restart: the same key, not a new one.
        let again = identity(&store).unwrap();
        assert_eq!((again.device_id, again.secret), (first.device_id, first.secret.clone()));
        // Another installation gets another key.
        assert_ne!(identity(&Memory::default()).unwrap().secret, first.secret);
    }

    #[test]
    fn pairings_are_pinned_replaced_and_forgotten() {
        let store = Memory::default();
        assert!(paired_hosts(&store).unwrap().is_empty());
        pin_host(&store, host("h1", 1)).unwrap();
        pin_host(&store, host("h2", 2)).unwrap();
        pin_host(&store, host("h1", 3)).unwrap();
        let hosts = paired_hosts(&store).unwrap();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts.iter().find(|h| h.host_id == "h1").unwrap(), &host("h1", 3));
        let secret = identity(&store).unwrap().secret;
        forget_host(&store, "h1").unwrap();
        assert_eq!(paired_hosts(&store).unwrap(), vec![host("h2", 2)]);
        // Forgetting a host keeps the device's key.
        assert_eq!(identity(&store).unwrap().secret, secret);
    }

    #[test]
    fn invalid_pins_are_refused() {
        let store = Memory::default();
        assert!(pin_host(&store, PairedHost { host_static_pubkey: "short".into(), ..host("h1", 1) }).is_err());
        assert!(pin_host(&store, host("bad id", 1)).is_err());
        assert!(paired_hosts(&store).unwrap().is_empty());
    }

    #[test]
    fn debug_never_prints_the_secret() {
        let identity = identity(&Memory::default()).unwrap();
        assert!(!format!("{identity:?}").contains(&identity.secret));
    }

    /// Against the real OS keychain; run by hand (`--ignored`).
    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let before = Keychain.load().unwrap();
        let identity = identity(&Keychain).unwrap();
        assert_eq!(Keychain.load().unwrap().unwrap().secret, identity.secret);
        if before.is_none() {
            Keychain::entry().unwrap().delete_credential().unwrap();
        }
    }
}
