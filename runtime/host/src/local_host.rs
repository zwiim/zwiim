//! Local host discovery: `<data_dir>/local-host.json`.
//!
//! Written by the running host after it binds, so a desktop client on the same
//! machine can find it and authenticate without reading host SQLite. The file
//! is owner-only (`0600` on unix) and carries the loopback URL, the host's
//! access token, its `host_id` and its `pid`. Every start rewrites it; a file
//! left behind by a host that died is recognised as stale by its `pid` no
//! longer running and by `host.hello` failing at `url`.
//!
//! This is V1 local discovery, not device security: the token is the host's
//! existing access token (Identity V1), protected by file permissions.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "local-host.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalHostInfo {
    pub host_id: String,
    /// Loopback base URL, e.g. `http://127.0.0.1:3080`.
    pub url: String,
    pub token: String,
    pub pid: u32,
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

/// The loopback URL a local client should use for a listener bound to
/// `bind_host:port`. A wildcard bind is reached over `127.0.0.1`.
pub fn loopback_url(bind_host: &str, port: u16) -> String {
    let host = bind_host.trim_start_matches('[').trim_end_matches(']');
    match host {
        "" | "0.0.0.0" | "::" => format!("http://127.0.0.1:{port}"),
        h if h.contains(':') => format!("http://[{h}]:{port}"),
        h => format!("http://{h}:{port}"),
    }
}

/// Atomically (re)write the discovery file with owner-only permissions.
pub fn write(data_dir: &Path, info: &LocalHostInfo) -> io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let json = serde_json::to_vec_pretty(info)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let target = path(data_dir);
    let tmp = target.with_extension("json.tmp");
    // A leftover temp file may carry other permissions; start fresh.
    let _ = std::fs::remove_file(&tmp);
    crate::identity::device::write_private(&tmp, &json)?;
    std::fs::rename(&tmp, &target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(pid: u32) -> LocalHostInfo {
        LocalHostInfo {
            host_id: "host-1".into(),
            url: "http://127.0.0.1:3080".into(),
            token: "secret".into(),
            pid,
        }
    }

    #[test]
    fn write_then_rewrite_replaces_contents() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &info(1)).unwrap();
        write(dir.path(), &info(2)).unwrap();
        let read: LocalHostInfo =
            serde_json::from_slice(&std::fs::read(path(dir.path())).unwrap()).unwrap();
        assert_eq!(read, info(2));
        assert!(!dir.path().join("local-host.json.tmp").exists());
    }

    #[cfg(unix)]
    #[test]
    fn file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &info(1)).unwrap();
        let mode = std::fs::metadata(path(dir.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn loopback_url_for_bind_hosts() {
        assert_eq!(loopback_url("0.0.0.0", 3080), "http://127.0.0.1:3080");
        assert_eq!(loopback_url("::", 1), "http://127.0.0.1:1");
        assert_eq!(loopback_url("127.0.0.1", 9), "http://127.0.0.1:9");
        assert_eq!(loopback_url("[::1]", 9), "http://[::1]:9");
        assert_eq!(loopback_url("::1", 9), "http://[::1]:9");
    }
}
