//! Device-side identity: the persistent `device_id` and X25519 static keypair
//! a client presents when pairing, the hosts it pinned by key, and any legacy
//! per-host bearer credentials.
//!
//! Stored as a small JSON file owned by the client (mode `0600` on unix). This
//! is the client half of the identity model and lives here until a client
//! crate exists; the host never reads it. A client with platform secure
//! storage (the mobile keystore) keeps the static secret there instead.

use std::fmt;
use std::io;
use std::path::Path;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::pairing::{KeyPairingGrant, KeyPairingInvitation, PairingGrant};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIdentity {
    pub device_id: String,
    pub device_name: String,
    /// X25519 static secret (URL-safe base64). Minted on first load, including
    /// for an identity file written before device keys existed.
    #[serde(default)]
    static_secret: Option<DeviceSecret>,
    /// Hosts paired by key, each pinned to its static public key.
    #[serde(default)]
    pub paired_hosts: Vec<PairedHost>,
    /// Legacy bearer credentials.
    #[serde(default)]
    pub hosts: Vec<HostCredential>,
}

#[derive(Clone, PartialEq, Zeroize, ZeroizeOnDrop, Serialize, Deserialize)]
#[serde(transparent)]
struct DeviceSecret(String);

/// A host this device paired with by key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedHost {
    pub host_id: String,
    /// The host's X25519 static public key from the QR it was paired through.
    pub host_static_pubkey: String,
}

impl fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceIdentity")
            .field("device_id", &self.device_id)
            .field("device_name", &self.device_name)
            .field("static_public", &self.static_public().ok())
            .field("paired_hosts", &self.paired_hosts)
            .field("hosts", &self.hosts.iter().map(|h| &h.host_id).collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostCredential {
    pub host_id: String,
    pub credential: String,
    #[serde(default)]
    pub addresses: Vec<String>,
}

impl DeviceIdentity {
    /// Load the identity at `path`, or mint and persist a new `device_id`.
    pub fn load_or_create(path: &Path, device_name: &str) -> io::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let mut identity: Self = serde_json::from_slice(&bytes)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                if identity.static_secret.is_none() {
                    identity.static_secret = Some(new_static_secret());
                    identity.save(path)?;
                }
                identity.static_key()?;
                Ok(identity)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let identity = Self {
                    device_id: uuid::Uuid::new_v4().to_string(),
                    device_name: device_name.to_string(),
                    static_secret: Some(new_static_secret()),
                    paired_hosts: Vec::new(),
                    hosts: Vec::new(),
                };
                identity.save(path)?;
                Ok(identity)
            }
            Err(e) => Err(e),
        }
    }

    fn static_key(&self) -> io::Result<StaticSecret> {
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid device static key");
        let encoded = self.static_secret.as_ref().ok_or_else(invalid)?;
        let mut decoded = URL_SAFE_NO_PAD.decode(&encoded.0).map_err(|_| invalid())?;
        let bytes: Result<[u8; 32], _> = decoded.as_slice().try_into();
        decoded.zeroize();
        let mut bytes = bytes.map_err(|_| invalid())?;
        let key = StaticSecret::from(bytes);
        bytes.zeroize();
        Ok(key)
    }

    /// The raw static secret, for tests that drive the handshake directly.
    #[cfg(test)]
    pub(crate) fn static_secret_bytes_for_test(&self) -> zeroize::Zeroizing<[u8; 32]> {
        zeroize::Zeroizing::new(self.static_key().unwrap().to_bytes())
    }

    /// This device's X25519 static public key, as `device.pair` takes it.
    pub fn static_public(&self) -> io::Result<String> {
        Ok(URL_SAFE_NO_PAD.encode(PublicKey::from(&self.static_key()?).as_bytes()))
    }

    /// Remember a host paired by key, pinned to the static key its invitation
    /// carried (replacing any earlier record for the same host), and persist.
    pub fn record_key_pairing(
        &mut self,
        path: &Path,
        invitation: &KeyPairingInvitation,
        grant: &KeyPairingGrant,
    ) -> io::Result<()> {
        if grant.host_id != invitation.host_id || grant.device_id != self.device_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "pairing grant does not match the invitation",
            ));
        }
        self.paired_hosts.retain(|h| h.host_id != grant.host_id);
        self.paired_hosts.push(PairedHost {
            host_id: invitation.host_id.clone(),
            host_static_pubkey: invitation.host_static_pubkey.clone(),
        });
        self.save(path)
    }

    /// Open the device tunnel to `host_id` over `io`, authenticating with
    /// this device's static key and the host's pinned one.
    pub async fn connect_tunnel<S>(
        &self,
        io: S,
        host_id: &str,
    ) -> Result<crate::tunnel::noise::Tunnel<S>, crate::tunnel::noise::TunnelError>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let not_paired = || io::Error::new(io::ErrorKind::NotFound, "not paired with this host by key");
        let host_key: [u8; 32] = self
            .pinned_host_key(host_id)
            .and_then(|k| URL_SAFE_NO_PAD.decode(k).ok())
            .and_then(|k| k.try_into().ok())
            .ok_or_else(not_paired)?;
        let secret = zeroize::Zeroizing::new(self.static_key()?.to_bytes());
        crate::tunnel::noise::connect(io, host_id, &secret, &host_key).await
    }

    /// Pair with the host of a v2 `invitation` as the tunnel opens over `io`
    /// (the Cloud relay): the handshake carries the one-time code, and the
    /// host trusts this device's static key. On success the host's key is
    /// pinned and persisted, and the tunnel is open as this device.
    pub async fn pair_tunnel<S>(
        &mut self,
        path: &Path,
        io: S,
        invitation: &KeyPairingInvitation,
    ) -> Result<crate::tunnel::noise::Tunnel<S>, crate::tunnel::noise::TunnelError>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let host_key: [u8; 32] = URL_SAFE_NO_PAD
            .decode(&invitation.host_static_pubkey)
            .ok()
            .and_then(|k| k.try_into().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid host key in invitation"))?;
        let hello = crate::tunnel::host::DeviceHello {
            pair: crate::tunnel::host::PairingHello {
                code: invitation.code.clone(),
                device_id: self.device_id.clone(),
                device_name: Some(self.device_name.clone()),
            },
        };
        let hello = serde_json::to_vec(&hello).map_err(io::Error::other)?;
        let secret = zeroize::Zeroizing::new(self.static_key()?.to_bytes());
        let tunnel =
            crate::tunnel::noise::connect_with(io, &invitation.host_id, &secret, &host_key, &hello).await?;
        let grant = KeyPairingGrant {
            host_id: invitation.host_id.clone(),
            device_id: self.device_id.clone(),
        };
        self.record_key_pairing(path, invitation, &grant)?;
        Ok(tunnel)
    }

    /// The pinned static key of a host paired by key.
    pub fn pinned_host_key(&self, host_id: &str) -> Option<&str> {
        self.paired_hosts
            .iter()
            .find(|h| h.host_id == host_id)
            .map(|h| h.host_static_pubkey.as_str())
    }

    /// Remember the credential a host granted this device (replacing any
    /// earlier one for the same host) and persist.
    pub fn record_pairing(
        &mut self,
        path: &Path,
        grant: &PairingGrant,
        addresses: Vec<String>,
    ) -> io::Result<()> {
        self.hosts.retain(|h| h.host_id != grant.host_id);
        self.hosts.push(HostCredential {
            host_id: grant.host_id.clone(),
            credential: grant.credential.clone(),
            addresses,
        });
        self.save(path)
    }

    pub fn credential_for(&self, host_id: &str) -> Option<&str> {
        self.hosts
            .iter()
            .find(|h| h.host_id == host_id)
            .map(|h| h.credential.as_str())
    }

    fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        // Write-then-rename so a crash never leaves a truncated identity.
        let tmp = path.with_extension("tmp");
        write_private(&tmp, &json)?;
        std::fs::rename(&tmp, path)
    }
}

fn new_static_secret() -> DeviceSecret {
    DeviceSecret(URL_SAFE_NO_PAD.encode(StaticSecret::random_from_rng(OsRng).to_bytes()))
}

#[cfg(unix)]
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

#[cfg(not(unix))]
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_is_stable_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.json");

        let first = DeviceIdentity::load_or_create(&path, "Phone").unwrap();
        assert!(uuid::Uuid::parse_str(&first.device_id).is_ok());

        let reloaded = DeviceIdentity::load_or_create(&path, "ignored").unwrap();
        assert_eq!(reloaded, first);
    }

    #[test]
    fn pairing_credential_persists_and_replaces_per_host() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.json");
        let mut identity = DeviceIdentity::load_or_create(&path, "Phone").unwrap();

        let grant = |credential: &str| PairingGrant {
            host_id: "host-1".into(),
            device_id: identity.device_id.clone(),
            credential: credential.into(),
        };
        let (g1, g2) = (grant("c1"), grant("c2"));
        identity.record_pairing(&path, &g1, vec![]).unwrap();
        identity.record_pairing(&path, &g2, vec![]).unwrap();

        let reloaded = DeviceIdentity::load_or_create(&path, "Phone").unwrap();
        assert_eq!(reloaded.hosts.len(), 1);
        assert_eq!(reloaded.credential_for("host-1"), Some("c2"));
        assert_eq!(reloaded.credential_for("host-2"), None);
    }

    #[test]
    fn static_key_is_stable_and_never_printed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.json");
        let first = DeviceIdentity::load_or_create(&path, "Phone").unwrap();
        let public = first.static_public().unwrap();
        assert!(crate::identity::trusted_devices::parse_device_pubkey(&public).is_some());
        let reloaded = DeviceIdentity::load_or_create(&path, "Phone").unwrap();
        assert_eq!(reloaded.static_public().unwrap(), public);

        let secret = first.static_secret.as_ref().unwrap().0.clone();
        assert_ne!(secret, public);
        let debug = format!("{first:?}");
        assert!(debug.contains(&public) && !debug.contains(&secret), "{debug}");
    }

    #[test]
    fn identity_files_from_before_device_keys_gain_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.json");
        std::fs::write(
            &path,
            r#"{"deviceId":"dev-1","deviceName":"Phone","hosts":[{"hostId":"h","credential":"c"}]}"#,
        )
        .unwrap();
        let upgraded = DeviceIdentity::load_or_create(&path, "ignored").unwrap();
        assert_eq!(upgraded.device_id, "dev-1");
        assert_eq!(upgraded.credential_for("h"), Some("c"));
        let public = upgraded.static_public().unwrap();
        let reloaded = DeviceIdentity::load_or_create(&path, "ignored").unwrap();
        assert_eq!(reloaded.static_public().unwrap(), public);
    }

    #[test]
    fn key_pairing_pins_the_host_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.json");
        let mut identity = DeviceIdentity::load_or_create(&path, "Phone").unwrap();
        let invitation = |key: &str| KeyPairingInvitation {
            v: 2,
            host_id: "host-1".into(),
            host_static_pubkey: key.into(),
            code: "code".into(),
            expires_at: chrono::Utc::now(),
        };
        let grant = KeyPairingGrant {
            host_id: "host-1".into(),
            device_id: identity.device_id.clone(),
        };
        identity.record_key_pairing(&path, &invitation("k1"), &grant).unwrap();
        identity.record_key_pairing(&path, &invitation("k2"), &grant).unwrap();
        let wrong_host = KeyPairingGrant {
            host_id: "host-2".into(),
            device_id: identity.device_id.clone(),
        };
        assert!(identity.record_key_pairing(&path, &invitation("k3"), &wrong_host).is_err());

        let reloaded = DeviceIdentity::load_or_create(&path, "Phone").unwrap();
        assert_eq!(reloaded.paired_hosts.len(), 1);
        assert_eq!(reloaded.pinned_host_key("host-1"), Some("k2"));
        assert_eq!(reloaded.pinned_host_key("host-2"), None);
    }

    #[cfg(unix)]
    #[test]
    fn identity_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.json");
        DeviceIdentity::load_or_create(&path, "Phone").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
