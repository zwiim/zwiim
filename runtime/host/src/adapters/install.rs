//! Download → verify → atomic install → probe → activate.
//!
//! Layout under the adapters root (`<data dir>/adapters`):
//!
//! ```text
//! node/<version>/bin/node          the Node runtime adapters run on
//! <adapter>/<version>/             one complete package tree per version
//! <adapter>/<version>/.zwiim-install.json   written last: the tree is whole
//! <adapter>/state.json             {active, previous}
//! <adapter>/.staging-*             an install in progress (never run)
//! ```
//!
//! Nothing is executed until every artifact has matched the lock. A version
//! directory appears by one rename, only when complete; it becomes the one
//! the Host launches only after it answered the health probe. The previously
//! active version stays on disk as the rollback target, and a failed install
//! leaves the active one untouched.

use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::lock::{AdapterLock, Lock, Platform};
use super::probe::{self, ProbeInfo};
use super::verify::{self, VerifyError};

pub const STATE_FILE: &str = "state.json";
pub const MARKER: &str = ".zwiim-install.json";
const NODE_DIR: &str = "node";
const STAGING_PREFIX: &str = ".staging-";
const TRASH_PREFIX: &str = ".trash-";
/// What is kept from a Node release: the runtime and its license.
const NODE_FILES: &[&str] = &["bin/node", "LICENSE"];

/// Serializes Node installs: every adapter shares `node/`.
static NODE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[async_trait::async_trait]
pub trait Fetch: Send + Sync {
    /// Download `url` into the file `dest`.
    async fn fetch(&self, url: &str, dest: &Path) -> Result<(), FetchError>;
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum FetchError {
    /// The server could not be reached.
    #[error("{0}")]
    Offline(String),
    #[error("{0}")]
    Failed(String),
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum SetupError {
    #[error("setup needs a network connection ({0})")]
    Offline(String),
    #[error("download failed: {0}")]
    Download(String),
    #[error(transparent)]
    Verify(#[from] VerifyError),
    #[error("the adapter failed its health check: {0}")]
    Unhealthy(String),
    #[error("not available for this platform")]
    Unsupported,
    #[error("{0}")]
    Io(String),
}

impl From<io::Error> for SetupError {
    fn from(e: io::Error) -> Self {
        SetupError::Io(e.to_string())
    }
}

impl From<FetchError> for SetupError {
    fn from(e: FetchError) -> Self {
        match e {
            FetchError::Offline(m) => SetupError::Offline(m),
            FetchError::Failed(m) => SetupError::Download(m),
        }
    }
}

/// Which installed version the Host launches, and which it falls back to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveState {
    pub active: Option<String>,
    pub previous: Option<String>,
}

/// Written into a version directory once it is complete.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Marker {
    adapter: String,
    version: String,
    /// [`Lock::digest`] of what was installed.
    digest: String,
    /// The Node version it was probed with.
    node: String,
    entry: String,
    installed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeMarker {
    version: String,
    sha256: String,
}

/// A complete installed version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub version: String,
    pub digest: String,
    pub node: PathBuf,
    pub entry: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The locked version was already active and healthy.
    Current(ProbeInfo),
    /// The locked version was installed and activated.
    Installed {
        version: String,
        replaced: Option<String>,
        probe: ProbeInfo,
    },
}

pub fn read_state(root: &Path, adapter: &str) -> ActiveState {
    std::fs::read(root.join(adapter).join(STATE_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_state(root: &Path, adapter: &str, state: &ActiveState) -> io::Result<()> {
    let dir = root.join(adapter);
    let tmp = dir.join(format!("{STATE_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
    std::fs::File::open(&tmp)?.sync_all()?;
    std::fs::rename(&tmp, dir.join(STATE_FILE))?;
    sync_dir(&dir);
    Ok(())
}

/// A complete install of `version`, if there is one.
pub fn installed(root: &Path, adapter: &str, version: &str) -> Option<Installed> {
    let dir = root.join(adapter).join(version);
    let marker: Marker = serde_json::from_slice(&std::fs::read(dir.join(MARKER)).ok()?).ok()?;
    if marker.adapter != adapter || marker.version != version || !safe_relative(&marker.entry) {
        return None;
    }
    let node = node_bin(root, &marker.node);
    let entry = dir.join(&marker.entry);
    (node.is_file() && entry.is_file()).then_some(Installed {
        version: marker.version,
        digest: marker.digest,
        node,
        entry,
    })
}

/// The version the Host launches.
pub fn active(root: &Path, adapter: &str) -> Option<Installed> {
    installed(root, adapter, read_state(root, adapter).active.as_deref()?)
}

/// The version kept to roll back to.
pub fn previous(root: &Path, adapter: &str) -> Option<Installed> {
    installed(
        root,
        adapter,
        read_state(root, adapter).previous.as_deref()?,
    )
}

fn node_bin(root: &Path, version: &str) -> PathBuf {
    root.join(NODE_DIR).join(version).join("bin").join("node")
}

pub struct Installer<'a> {
    pub root: &'a Path,
    pub lock: &'a Lock,
    pub platform: Platform,
    pub fetch: &'a dyn Fetch,
    pub probe_timeout: Duration,
}

impl Installer<'_> {
    /// Make the locked version of `adapter` the active one. Safe to repeat,
    /// and to interrupt: a later call starts the install over.
    pub async fn ensure(&self, adapter: &str) -> Result<Outcome, SetupError> {
        let tree = self
            .lock
            .adapters
            .get(adapter)
            .ok_or(SetupError::Unsupported)?;
        let digest = self
            .lock
            .digest(adapter, self.platform)
            .ok_or(SetupError::Unsupported)?;
        if let Some(current) = active(self.root, adapter) {
            if current.version == tree.version && current.digest == digest {
                return self.probe(&current).await.map(Outcome::Current);
            }
        }

        let node_version = self.ensure_node().await?;
        let version_dir = self
            .install_tree(adapter, tree, &digest, &node_version)
            .await?;
        let Some(fresh) = installed(self.root, adapter, &tree.version) else {
            return Err(SetupError::Io("the installed tree is incomplete".into()));
        };
        match self.probe(&fresh).await {
            Ok(probe) => {
                let replaced = activate(self.root, adapter, &tree.version)?;
                let _guard = NODE_LOCK.lock().await;
                prune(self.root, self.lock);
                Ok(Outcome::Installed {
                    version: tree.version.clone(),
                    replaced,
                    probe,
                })
            }
            Err(e) => {
                let state = read_state(self.root, adapter);
                if state.active.as_deref() == Some(tree.version.as_str()) {
                    // A broken reinstall of the active version: fall back.
                    write_state(
                        self.root,
                        adapter,
                        &ActiveState {
                            active: state.previous.clone(),
                            previous: None,
                        },
                    )?;
                }
                if state.previous.as_deref() != Some(tree.version.as_str()) {
                    let _ = std::fs::remove_dir_all(&version_dir);
                }
                Err(e)
            }
        }
    }

    /// Make the previous version active again, after it passes the probe.
    pub async fn rollback(&self, adapter: &str) -> Result<String, SetupError> {
        let Some(previous) = previous(self.root, adapter) else {
            return Err(SetupError::Io(
                "there is no previous version to roll back to".into(),
            ));
        };
        self.probe(&previous).await?;
        let state = read_state(self.root, adapter);
        write_state(
            self.root,
            adapter,
            &ActiveState {
                active: state.previous,
                previous: state.active,
            },
        )?;
        Ok(previous.version)
    }

    async fn probe(&self, installed: &Installed) -> Result<ProbeInfo, SetupError> {
        probe::initialize(&installed.node, &installed.entry, self.probe_timeout)
            .await
            .map_err(SetupError::Unhealthy)
    }

    /// The locked Node runtime, installed if missing. Returns its version.
    async fn ensure_node(&self) -> Result<String, SetupError> {
        let _guard = NODE_LOCK.lock().await;
        let node = &self.lock.node;
        let artifact = node
            .platforms
            .get(&self.platform.key())
            .ok_or(SetupError::Unsupported)?;
        let dir = self.root.join(NODE_DIR).join(&node.version);
        let marker: Option<NodeMarker> = std::fs::read(dir.join(MARKER))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        if marker.is_some_and(|m| m.sha256 == artifact.sha256) && dir.join("bin/node").is_file() {
            return Ok(node.version.clone());
        }

        let staging = new_staging(&self.root.join(NODE_DIR))?;
        let result = async {
            let archive = staging.join("node.tar.gz");
            self.fetch.fetch(&artifact.url, &archive).await?;
            verify::sha256(
                &archive,
                &artifact.sha256,
                &format!("Node {}", node.version),
            )?;
            let tree = staging.join("tree");
            extract(&archive, &tree, Some(NODE_FILES))?;
            if !tree.join("bin/node").is_file() {
                return Err(SetupError::Io("the Node archive has no bin/node".into()));
            }
            write_json(
                &tree.join(MARKER),
                &NodeMarker {
                    version: node.version.clone(),
                    sha256: artifact.sha256.clone(),
                },
            )?;
            replace_dir(&tree, &dir)?;
            Ok(node.version.clone())
        }
        .await;
        let _ = std::fs::remove_dir_all(&staging);
        result
    }

    /// Download, verify and extract every package, then publish the tree as
    /// `<adapter>/<version>` in one rename.
    async fn install_tree(
        &self,
        adapter: &str,
        tree: &AdapterLock,
        digest: &str,
        node_version: &str,
    ) -> Result<PathBuf, SetupError> {
        if !safe_relative(&tree.entry) {
            return Err(SetupError::Io(format!("unsafe entry path {}", tree.entry)));
        }
        let parent = self.root.join(adapter);
        sweep(&parent);
        let staging = new_staging(&parent)?;
        let dest = parent.join(&tree.version);
        let result = async {
            let out = staging.join("tree");
            std::fs::create_dir_all(&out)?;
            let wanted = tree.packages.iter().filter(|p| p.applies_to(self.platform));
            for (i, package) in wanted.enumerate() {
                if !safe_relative(&package.path) {
                    return Err(SetupError::Io(format!(
                        "unsafe package path {}",
                        package.path
                    )));
                }
                let archive = staging.join(format!("{i}.tgz"));
                self.fetch.fetch(&package.url, &archive).await?;
                let what = format!("{}@{}", package.name, package.version);
                verify::integrity(&archive, &package.integrity, &what)?;
                verify::registry_signature(&self.lock.registry_keys, package)?;
                extract(&archive, &out.join(&package.path), None)?;
                let _ = std::fs::remove_file(&archive);
            }
            if !out.join(&tree.entry).is_file() {
                return Err(SetupError::Io(format!("the adapter has no {}", tree.entry)));
            }
            write_json(
                &out.join(MARKER),
                &Marker {
                    adapter: adapter.to_string(),
                    version: tree.version.clone(),
                    digest: digest.to_string(),
                    node: node_version.to_string(),
                    entry: tree.entry.clone(),
                    installed_at: chrono::Utc::now().to_rfc3339(),
                },
            )?;
            replace_dir(&out, &dest)?;
            Ok(dest.clone())
        }
        .await;
        let _ = std::fs::remove_dir_all(&staging);
        result
    }
}

/// Record `version` as active, keeping the one it replaces as the rollback
/// target. Returns the replaced version.
fn activate(root: &Path, adapter: &str, version: &str) -> io::Result<Option<String>> {
    let state = read_state(root, adapter);
    let replaced = state.active.clone().filter(|v| v != version);
    let previous = replaced.clone().or(state.previous);
    write_state(
        root,
        adapter,
        &ActiveState {
            active: Some(version.to_string()),
            previous: previous.filter(|v| v != version),
        },
    )?;
    Ok(replaced)
}

/// Remove versions no adapter can launch or roll back to, and the Node
/// runtimes none of those use. Callers hold [`NODE_LOCK`].
fn prune(root: &Path, lock: &Lock) {
    let mut nodes_in_use = std::collections::HashSet::new();
    for adapter in lock.adapters.keys() {
        let state = read_state(root, adapter);
        let keep: Vec<&str> = [state.active.as_deref(), state.previous.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        for version in &keep {
            if let Some(marker) = std::fs::read(root.join(adapter).join(version).join(MARKER))
                .ok()
                .and_then(|b| serde_json::from_slice::<Marker>(&b).ok())
            {
                nodes_in_use.insert(marker.node);
            }
        }
        remove_dirs_except(&root.join(adapter), &keep);
    }
    let nodes: Vec<&str> = nodes_in_use.iter().map(String::as_str).collect();
    remove_dirs_except(&root.join(NODE_DIR), &nodes);
}

fn remove_dirs_except(parent: &Path, keep: &[&str]) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Dot-directories are installs in progress; `sweep` owns them.
        if !entry.path().is_dir() || name.starts_with('.') || keep.contains(&name.as_str()) {
            continue;
        }
        if let Err(e) = std::fs::remove_dir_all(entry.path()) {
            tracing::warn!(
                "[adapters] could not remove {}: {e}",
                entry.path().display()
            );
        }
    }
}

/// Leftovers of interrupted installs. Only one install per directory runs at
/// a time (the caller's lock and the Host's instance lock).
fn sweep(parent: &Path) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(STAGING_PREFIX) || name.starts_with(TRASH_PREFIX) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn new_staging(parent: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(parent)?;
    let dir = parent.join(format!(
        "{STAGING_PREFIX}{}-{}",
        std::process::id(),
        nanos()
    ));
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

/// Put the complete directory `src` at `dest` by rename. Whatever was at
/// `dest` (an incomplete leftover, or a tree from an older lock) goes first.
pub(crate) fn replace_dir(src: &Path, dest: &Path) -> io::Result<()> {
    let parent = dest.parent().expect("dest has a parent");
    if dest.exists() {
        let trash = parent.join(format!("{TRASH_PREFIX}{}", nanos()));
        std::fs::rename(dest, &trash)?;
        let _ = std::fs::remove_dir_all(&trash);
    }
    std::fs::rename(src, dest)?;
    sync_dir(parent);
    Ok(())
}

/// Unpack a `.tar.gz` into `dest`, dropping the archive's top directory.
/// Only regular files and directories are written; links and special files
/// are skipped, and any path that would leave `dest` is refused. `only`
/// limits extraction to those paths.
pub(crate) fn extract(
    archive: &Path,
    dest: &Path,
    only: Option<&[&str]>,
) -> Result<(), SetupError> {
    let file = std::fs::File::open(archive)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    std::fs::create_dir_all(dest)?;
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let mut rel = PathBuf::new();
        for component in path.components().skip(1) {
            match component {
                Component::Normal(part) => rel.push(part),
                Component::CurDir => {}
                _ => {
                    return Err(SetupError::Verify(VerifyError::Malformed(format!(
                        "unsafe path in archive: {}",
                        path.display()
                    ))))
                }
            }
        }
        if rel.as_os_str().is_empty() {
            continue;
        }
        if only.is_some_and(|only| !only.iter().any(|p| rel == Path::new(p))) {
            continue;
        }
        let out = dest.join(&rel);
        match entry.header().entry_type() {
            tar::EntryType::Directory => std::fs::create_dir_all(&out)?,
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let executable = entry
                    .header()
                    .mode()
                    .map(|m| m & 0o111 != 0)
                    .unwrap_or(false);
                let mut file = std::fs::File::create(&out)?;
                io::copy(&mut entry, &mut file)?;
                set_mode(&out, executable)?;
            }
            _ => {}
        }
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

/// A lock path that stays inside the install directory.
fn safe_relative(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)))
}

fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    std::fs::write(path, serde_json::to_vec_pretty(value)?)
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(unix)]
fn sync_dir(dir: &Path) {
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) {}
