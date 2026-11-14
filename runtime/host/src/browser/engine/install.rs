//! Getting the engine onto this machine (design 5.2).
//!
//! Zwiim pins one `chrome-headless-shell` version and the SHA-256 of each
//! platform's archive. The Host downloads it from Google's Chrome for Testing
//! bucket on first use, checks the hash, unpacks it into its own directory
//! and only then marks it complete:
//!
//! ```text
//! <data>/browser/engine/
//!   <version>/              chrome-headless-shell and its files
//!   <version>/.zwiim-engine.json
//!   .staging-*  .trash-*    in-flight work, swept on the next install
//! ```

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::adapters::install::{Fetch, FetchError};

/// The pinned Chrome for Testing version.
pub const VERSION: &str = "157.0.8103.78";
const BASE_URL: &str = "https://storage.googleapis.com/chrome-for-testing-public";
const MARKER: &str = ".zwiim-engine.json";
const STAGING_PREFIX: &str = ".staging-";
const BINARY: &str = "chrome-headless-shell";

/// One platform's archive of the pinned version. The release process
/// downloads it once, checks it against the bucket's MD5 (Google publishes
/// no SHA-256) and records its SHA-256 and size here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin {
    pub version: &'static str,
    /// Chrome for Testing's platform name.
    pub platform: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

const MAC_ARM64: Pin = Pin {
    version: VERSION,
    platform: "mac-arm64",
    sha256: "17bf66d53c1b1104dcb641c5b331cd4ac5d200c778f5feed0934a0b92deaf8d6",
    size: 104_822_985,
};

const MAC_X64: Pin = Pin {
    version: VERSION,
    platform: "mac-x64",
    sha256: "f1a6a9561eaf1f409849538de663695d36d72fbf1ebca4d42e47c7b5f2a157f3",
    size: 110_132_547,
};

/// This machine's pin. Only macOS has one: the memory governor and priority
/// are macOS code (design 11.5).
pub fn pin() -> Option<Pin> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some(MAC_ARM64)
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some(MAC_X64)
    } else {
        None
    }
}

impl Pin {
    pub fn url(&self) -> String {
        format!(
            "{BASE_URL}/{v}/{p}/{BINARY}-{p}.zip",
            v = self.version,
            p = self.platform
        )
    }
}

/// Why the engine could not be set up. [`SetupError::code`] is what clients
/// show a reason for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SetupError {
    #[error("the browser engine needs a network connection to download ({0})")]
    Offline(String),
    #[error("the browser engine download failed: {0}")]
    Download(String),
    #[error("the browser engine download does not match its pinned hash")]
    Checksum,
    #[error("not enough disk space for the browser engine")]
    DiskFull,
    #[error("the browser engine is not available for this platform")]
    Unsupported,
    #[error("{0}")]
    Io(String),
}

impl SetupError {
    pub fn code(&self) -> &'static str {
        match self {
            SetupError::Offline(_) => "download_offline",
            SetupError::Download(_) | SetupError::Io(_) => "setup_failed",
            SetupError::Checksum => "checksum_mismatch",
            SetupError::DiskFull => "disk_full",
            SetupError::Unsupported => "unsupported_platform",
        }
    }
}

impl From<io::Error> for SetupError {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::StorageFull {
            SetupError::DiskFull
        } else {
            SetupError::Io(e.to_string())
        }
    }
}

impl From<FetchError> for SetupError {
    fn from(e: FetchError) -> Self {
        match e {
            FetchError::Offline(m) => SetupError::Offline(m),
            FetchError::Failed(m) if m.contains("No space left") => SetupError::DiskFull,
            FetchError::Failed(m) => SetupError::Download(m),
        }
    }
}

/// Written into a version directory once it is complete.
#[derive(Debug, Serialize, Deserialize)]
struct Marker {
    version: String,
    platform: String,
    sha256: String,
}

/// An installed engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub version: String,
    pub binary: PathBuf,
}

/// Serializes installs: two tabs opening at once download once.
static INSTALL_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub struct Installer<'a> {
    pub root: &'a Path,
    pub pin: Pin,
    pub fetch: &'a dyn Fetch,
    /// Where the download is listed while it runs.
    pub downloads: Option<&'a std::sync::Arc<crate::downloads::Downloads>>,
}

impl Installer<'_> {
    /// The pinned engine, downloading it first if it is not installed. Safe
    /// to repeat and to interrupt: a later call starts the install over.
    pub async fn ensure(&self) -> Result<Installed, SetupError> {
        if let Some(found) = installed(self.root, &self.pin) {
            return Ok(found);
        }
        let _guard = INSTALL_LOCK.lock().await;
        if let Some(found) = installed(self.root, &self.pin) {
            return Ok(found);
        }

        std::fs::create_dir_all(self.root)?;
        sweep(self.root);
        let staging = new_staging(self.root)?;
        let result = self.install_into(&staging).await;
        let _ = std::fs::remove_dir_all(&staging);
        result?;
        installed(self.root, &self.pin)
            .ok_or_else(|| SetupError::Io("the installed engine is incomplete".into()))
    }

    async fn install_into(&self, staging: &Path) -> Result<(), SetupError> {
        let archive = staging.join("engine.zip");
        let progress = match self.downloads {
            Some(downloads) => downloads.begin(format!("Browser engine {}", self.pin.version), 1),
            None => crate::downloads::Progress::none(),
        };
        self.fetch.fetch(&self.pin.url(), &archive, &progress).await?;
        drop(progress);

        let pin = self.pin;
        let staging = staging.to_path_buf();
        let root = self.root.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let size = std::fs::metadata(&archive)?.len();
            if size != pin.size
                || crate::adapters::verify::sha256(&archive, pin.sha256, "browser engine").is_err()
            {
                return Err(SetupError::Checksum);
            }
            let tree = staging.join("tree");
            extract_zip(&archive, &tree)?;
            if !tree.join(BINARY).is_file() {
                return Err(SetupError::Io("the engine archive has no binary".into()));
            }
            let marker = Marker {
                version: pin.version.to_string(),
                platform: pin.platform.to_string(),
                sha256: pin.sha256.to_string(),
            };
            std::fs::write(
                tree.join(MARKER),
                serde_json::to_vec_pretty(&marker).map_err(io::Error::from)?,
            )?;
            crate::adapters::install::replace_dir(&tree, &root.join(pin.version))?;
            Ok(())
        })
        .await
        .map_err(|e| SetupError::Io(e.to_string()))?
    }
}

/// The pinned engine, if it is completely installed under `root`.
pub fn installed(root: &Path, pin: &Pin) -> Option<Installed> {
    let dir = root.join(pin.version);
    let marker: Marker = serde_json::from_slice(&std::fs::read(dir.join(MARKER)).ok()?).ok()?;
    let binary = dir.join(BINARY);
    (marker.version == pin.version && marker.sha256 == pin.sha256 && binary.is_file()).then(|| {
        Installed {
            version: pin.version.to_string(),
            binary,
        }
    })
}

/// Remove every installed version except `keep`. Called after `keep` has
/// started, so a broken new engine never costs the old one.
pub fn prune(root: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name != keep && entry.path().is_dir() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Leftovers of interrupted installs.
fn sweep(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(STAGING_PREFIX) || name.starts_with(".trash-") {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn new_staging(root: &Path) -> io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = root.join(format!("{STAGING_PREFIX}{}-{nanos}", std::process::id()));
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

/// Unpack a `.zip` into `dest`, dropping the archive's top directory. Only
/// regular files and directories are written; links are skipped, and any
/// path that would leave `dest` is refused.
fn extract_zip(archive: &Path, dest: &Path) -> Result<(), SetupError> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)
        .map_err(|e| SetupError::Io(format!("the engine archive is unreadable: {e}")))?;
    std::fs::create_dir_all(dest)?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| SetupError::Io(format!("the engine archive is unreadable: {e}")))?;
        let Some(path) = entry.enclosed_name() else {
            return Err(SetupError::Io(format!(
                "unsafe path in the engine archive: {}",
                entry.name()
            )));
        };
        let rel: PathBuf = path.components().skip(1).collect();
        if rel.as_os_str().is_empty() || entry.is_symlink() {
            continue;
        }
        let out = dest.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let executable = entry.unix_mode().is_some_and(|m| m & 0o111 != 0);
        let mut file = std::fs::File::create(&out)?;
        io::copy(&mut entry, &mut file)?;
        set_mode(&out, executable)?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, executable: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if executable { 0o755 } else { 0o644 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _executable: bool) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct FakeFetch {
        files: Mutex<HashMap<String, Vec<u8>>>,
        fetched: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl Fetch for FakeFetch {
        async fn fetch(
            &self,
            url: &str,
            dest: &Path,
            _progress: &crate::downloads::Progress,
        ) -> Result<(), FetchError> {
            let bytes = self.files.lock().unwrap().get(url).cloned();
            let bytes = bytes.ok_or_else(|| FetchError::Offline("no route to host".into()))?;
            self.fetched.fetch_add(1, Ordering::SeqCst);
            std::fs::write(dest, bytes).map_err(|e| FetchError::Failed(e.to_string()))
        }
    }

    /// A zip of `(path, mode, contents)` under one top directory.
    fn zip_of(entries: &[(&str, u32, &str)]) -> Vec<u8> {
        let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (path, mode, body) in entries {
            let options = zip::write::SimpleFileOptions::default().unix_permissions(*mode);
            out.start_file(*path, options).unwrap();
            out.write_all(body.as_bytes()).unwrap();
        }
        out.finish().unwrap().into_inner()
    }

    fn engine_zip() -> Vec<u8> {
        zip_of(&[
            (
                "chrome-headless-shell-mac-arm64/chrome-headless-shell",
                0o755,
                "#!engine",
            ),
            (
                "chrome-headless-shell-mac-arm64/resources/a/b.js",
                0o644,
                "js",
            ),
        ])
    }

    fn pin_for(bytes: &[u8]) -> Pin {
        use sha2::{Digest, Sha256};
        let sha = format!("{:x}", Sha256::digest(bytes));
        Pin {
            version: "1.2.3",
            platform: "mac-arm64",
            sha256: Box::leak(sha.into_boxed_str()),
            size: bytes.len() as u64,
        }
    }

    #[tokio::test]
    async fn installs_the_pinned_archive_once() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = engine_zip();
        let pin = pin_for(&bytes);
        let fetch = FakeFetch::default();
        fetch.files.lock().unwrap().insert(pin.url(), bytes);
        let installer = Installer {
            root: dir.path(),
            pin,
            fetch: &fetch,
            downloads: None,
        };

        let got = installer.ensure().await.unwrap();
        assert_eq!(got.version, "1.2.3");
        assert_eq!(got.binary, dir.path().join("1.2.3").join(BINARY));
        assert_eq!(std::fs::read_to_string(&got.binary).unwrap(), "#!engine");
        assert!(dir.path().join("1.2.3/resources/a/b.js").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&got.binary).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }

        assert_eq!(installer.ensure().await.unwrap(), got);
        assert_eq!(fetch.fetched.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_wrong_hash_installs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = engine_zip();
        let mut pin = pin_for(&bytes);
        pin.sha256 = "00";
        let fetch = FakeFetch::default();
        fetch.files.lock().unwrap().insert(pin.url(), bytes);
        let installer = Installer {
            root: dir.path(),
            pin,
            fetch: &fetch,
            downloads: None,
        };

        let err = installer.ensure().await.unwrap_err();
        assert_eq!(err.code(), "checksum_mismatch");
        let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert!(
            left.is_empty(),
            "staging is removed and no version is installed"
        );
    }

    #[tokio::test]
    async fn offline_is_reported_as_such() {
        let dir = tempfile::tempdir().unwrap();
        let fetch = FakeFetch::default();
        let installer = Installer {
            root: dir.path(),
            pin: pin_for(b"x"),
            fetch: &fetch,
            downloads: None,
        };
        assert_eq!(
            installer.ensure().await.unwrap_err().code(),
            "download_offline"
        );
    }

    #[test]
    fn paths_that_leave_the_install_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("evil.zip");
        std::fs::write(&archive, zip_of(&[("top/../../evil", 0o644, "x")])).unwrap();
        let err = extract_zip(&archive, &dir.path().join("out")).unwrap_err();
        assert!(err.to_string().contains("unsafe path"), "{err}");
        assert!(!dir.path().join("evil").exists());
    }

    #[test]
    fn prune_keeps_only_the_running_version() {
        let dir = tempfile::tempdir().unwrap();
        for v in ["1.0", "2.0"] {
            std::fs::create_dir_all(dir.path().join(v)).unwrap();
        }
        prune(dir.path(), "2.0");
        assert!(!dir.path().join("1.0").exists());
        assert!(dir.path().join("2.0").exists());
    }

    #[test]
    fn the_pinned_url_is_chrome_for_testing() {
        assert_eq!(
            MAC_ARM64.url(),
            "https://storage.googleapis.com/chrome-for-testing-public/157.0.8103.78/mac-arm64/chrome-headless-shell-mac-arm64.zip"
        );
    }
}
