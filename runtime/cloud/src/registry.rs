//! The account ↔ host registry: which account each enrolled host belongs to
//! and its public keys. Public metadata only — no secrets, no host state.
//!
//! Single-node V1: one JSON file in the data directory, rewritten atomically
//! on every change (enrollments are rare).

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

const FILE: &str = "registry.json";
const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRecord {
    pub host_id: String,
    pub user_id: String,
    pub name: Option<String>,
    /// Ed25519, authenticates the host's control connection.
    pub sign_pubkey: String,
    /// X25519, the tunnel key devices pin.
    pub static_pubkey: String,
    /// Unix seconds.
    pub enrolled_at: u64,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    v: u32,
    hosts: Vec<HostRecord>,
}

pub struct Registry {
    path: PathBuf,
    hosts: Mutex<HashMap<String, HostRecord>>,
}

impl Registry {
    pub fn open(dir: &Path) -> io::Result<Self> {
        create_private_dir(dir)?;
        let path = dir.join(FILE);
        let hosts = match fs::read(&path) {
            Ok(raw) => {
                let stored: Stored = serde_json::from_slice(&raw)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                if stored.v != VERSION {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "unknown registry version"));
                }
                stored.hosts.into_iter().map(|h| (h.host_id.clone(), h)).collect()
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e),
        };
        Ok(Self { path, hosts: Mutex::new(hosts) })
    }

    pub fn get(&self, host_id: &str) -> Option<HostRecord> {
        self.hosts.lock().unwrap().get(host_id).cloned()
    }

    pub fn for_user(&self, user_id: &str) -> Vec<HostRecord> {
        let mut hosts: Vec<_> = self
            .hosts
            .lock()
            .unwrap()
            .values()
            .filter(|h| h.user_id == user_id)
            .cloned()
            .collect();
        hosts.sort_by(|a, b| a.host_id.cmp(&b.host_id));
        hosts
    }

    /// Insert or replace a record, persisting before it takes effect.
    pub fn put(&self, record: HostRecord) -> io::Result<()> {
        let mut hosts = self.hosts.lock().unwrap();
        let mut next = hosts.clone();
        next.insert(record.host_id.clone(), record);
        let mut list: Vec<_> = next.values().cloned().collect();
        list.sort_by(|a, b| a.host_id.cmp(&b.host_id));
        let raw = serde_json::to_vec_pretty(&Stored { v: VERSION, hosts: list })?;
        write_atomic(&self.path, &raw)?;
        *hosts = next;
        Ok(())
    }
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
    }
    #[cfg(not(unix))]
    fs::create_dir_all(dir)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(host_id: &str, user_id: &str) -> HostRecord {
        HostRecord {
            host_id: host_id.into(),
            user_id: user_id.into(),
            name: None,
            sign_pubkey: format!("sign-{host_id}"),
            static_pubkey: format!("static-{host_id}"),
            enrolled_at: 1,
        }
    }

    #[test]
    fn records_survive_a_restart_and_are_per_account() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("cloud");
        let registry = Registry::open(&data).unwrap();
        registry.put(record("h2", "alice")).unwrap();
        registry.put(record("h1", "alice")).unwrap();
        registry.put(record("h3", "bob")).unwrap();

        let reopened = Registry::open(&data).unwrap();
        let alice: Vec<_> = reopened.for_user("alice").into_iter().map(|h| h.host_id).collect();
        assert_eq!(alice, ["h1", "h2"]);
        assert_eq!(reopened.get("h3"), Some(record("h3", "bob")));
        assert_eq!(reopened.get("h4"), None);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&data), 0o700);
            assert_eq!(mode(&data.join(FILE)), 0o600);
        }
        let raw = fs::read_to_string(data.join(FILE)).unwrap();
        assert!(!raw.contains("secret") && !raw.contains("token"), "{raw}");
    }

    #[test]
    fn a_corrupt_registry_is_an_error_not_an_empty_one() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(FILE), b"{not json").unwrap();
        assert!(Registry::open(dir.path()).is_err());
    }
}
