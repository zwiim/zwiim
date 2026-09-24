//! Device-side identity: the persistent `device_id` a client presents when
//! pairing, plus the per-host credentials it was granted.
//!
//! Stored as a small JSON file owned by the client (mode `0600` on unix). This
//! is the client half of Identity V1 and lives here until a client crate
//! exists; the host never reads it.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::pairing::PairingGrant;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIdentity {
    pub device_id: String,
    pub device_name: String,
    #[serde(default)]
    pub hosts: Vec<HostCredential>,
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
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let identity = Self {
                    device_id: uuid::Uuid::new_v4().to_string(),
                    device_name: device_name.to_string(),
                    hosts: Vec::new(),
                };
                identity.save(path)?;
                Ok(identity)
            }
            Err(e) => Err(e),
        }
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
