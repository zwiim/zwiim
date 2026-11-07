// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned Host installer: versioned installs, a trial, and rollback.
//!
//! Replaces the upstream in-place self-updater. The Host never overwrites the
//! binary it runs. Each version lives in its own directory under the data
//! directory, and one state file says which one runs:
//!
//! ```text
//! <data>/runtime/
//!   state.json            current, previous, staged, trial, blocked, seen, last
//!   channel.json          the release channel this Host follows
//!   versions/<v>/         zwiim-host, zwiim-mcp
//!   bin/gh                link to the running version's gh, for child processes
//!   .staging-*  .trash-*  in-flight work, swept on the next run
//! ```
//!
//! - **Stage.** A new version gets its own directory only after its archive
//!   matched the signed manifest, its binaries passed the platform signature
//!   check and it reported its own version ([`stage_archive`]). Desktop's
//!   bundled Host is staged the same way without a download ([`seed`]).
//! - **Activate.** On a restart the user asked to apply, the staged version
//!   becomes current and starts its **trial**; the old one is kept as
//!   previous ([`activate_staged`]).
//! - **Trial.** The supervisor starts `versions/<current>`. The new worker
//!   records whether it migrated the database ([`note_migration`]), proves
//!   its health, and commits after the trial window ([`commit`]).
//! - **Rollback.** A worker that exits (or hangs) before committing is
//!   rolled back by the supervisor: the pre-migration snapshot goes back if
//!   the trial migrated, previous becomes current again, and the failed
//!   version is blocked from being offered again ([`rollback`]).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::app_error::{AppCommandError, AppErrorCode};
use crate::db::schema_guard::MigrationOutcome;

use super::manifest::{Artifact, Channel};

pub const ROOT: &str = "runtime";
const STATE_FILE: &str = "state.json";
/// Its own file: `state.json` is rewritten by whichever version runs, and
/// one older than the channel setting would drop it.
const CHANNEL_FILE: &str = "channel.json";
const VERSIONS: &str = "versions";
const STAGING_PREFIX: &str = ".staging-";

#[cfg(windows)]
pub const HOST_BIN: &str = "zwiim-host.exe";
#[cfg(not(windows))]
pub const HOST_BIN: &str = "zwiim-host";
#[cfg(windows)]
pub const MCP_BIN: &str = "zwiim-mcp.exe";
#[cfg(not(windows))]
pub const MCP_BIN: &str = "zwiim-mcp";
/// The GitHub CLI shipped with the Host (see `git_harness::find_gh`).
#[cfg(windows)]
pub const GH_BIN: &str = "gh.exe";
#[cfg(not(windows))]
pub const GH_BIN: &str = "gh";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// The version the supervisor starts.
    #[serde(default)]
    pub current: Option<String>,
    /// The last known-good version before `current`, kept on disk.
    #[serde(default)]
    pub previous: Option<String>,
    /// Verified and waiting for a restart.
    #[serde(default)]
    pub staged: Option<Staged>,
    /// `current` has not proven itself yet.
    #[serde(default)]
    pub trial: Option<Trial>,
    /// Versions that failed their trial here; never offered again.
    #[serde(default)]
    pub blocked: Vec<String>,
    /// Digest of the manifest accepted for each version.
    #[serde(default)]
    pub seen: BTreeMap<String, String>,
    #[serde(default)]
    pub last: Option<LastResult>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageSource {
    /// Downloaded from the release and verified against its manifest.
    Download,
    /// The Host bundled in the Desktop app: no download needed.
    Bundle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Staged {
    pub version: String,
    pub source: StageSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trial {
    pub version: String,
    pub from: String,
    /// `previous` before this trial began, restored on rollback.
    #[serde(default)]
    pub previous_before: Option<String>,
    pub started_at: String,
    /// The trial's worker migrated the database; this snapshot predates it.
    #[serde(default)]
    pub migrated: bool,
    #[serde(default)]
    pub snapshot: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Applied,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastResult {
    pub outcome: Outcome,
    pub version: String,
    pub from: String,
    #[serde(default)]
    pub reason: Option<String>,
    pub at: String,
}

pub fn root(data_dir: &Path) -> PathBuf {
    data_dir.join(ROOT)
}

pub fn version_dir(root: &Path, version: &str) -> PathBuf {
    root.join(VERSIONS).join(version)
}

pub fn read(root: &Path) -> State {
    std::fs::read(root.join(STATE_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Write the state durably: a complete file replaces the old by rename.
pub fn write(root: &Path, state: &State) -> io::Result<()> {
    std::fs::create_dir_all(root)?;
    let tmp = root.join(format!("{STATE_FILE}.tmp"));
    {
        let mut file = std::fs::File::create(&tmp)?;
        io::Write::write_all(&mut file, &serde_json::to_vec_pretty(state)?)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, root.join(STATE_FILE))?;
    if let Ok(dir) = std::fs::File::open(root) {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct ChannelChoice {
    channel: Channel,
}

/// The release channel saved for this Host, if any.
pub fn channel(root: &Path) -> Option<Channel> {
    std::fs::read(root.join(CHANNEL_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<ChannelChoice>(&b).ok())
        .map(|c| c.channel)
}

pub fn set_channel(root: &Path, channel: Channel) -> io::Result<()> {
    std::fs::create_dir_all(root)?;
    let tmp = root.join(format!("{CHANNEL_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(&ChannelChoice { channel })?)?;
    std::fs::rename(&tmp, root.join(CHANNEL_FILE))
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn parse(v: &str) -> Option<Version> {
    Version::parse(v).ok()
}

/// The Host binary the supervisor should start, if a version is installed.
pub fn worker_exe(root: &Path) -> Option<PathBuf> {
    let current = read(root).current?;
    let exe = version_dir(root, &current).join(HOST_BIN);
    exe.is_file().then_some(exe)
}

/// Copy a complete install (the Host and, when present, the MCP bridge and
/// `gh` next to it) from `from_dir` into `versions/<version>` by rename.
fn install_from_dir(root: &Path, from_dir: &Path, version: &str) -> io::Result<()> {
    let versions = root.join(VERSIONS);
    std::fs::create_dir_all(&versions)?;
    let staging = new_staging(&versions)?;
    let result = (|| {
        for bin in [HOST_BIN, MCP_BIN, GH_BIN] {
            let src = from_dir.join(bin);
            if src.is_file() {
                std::fs::copy(&src, staging.join(bin))?;
            } else if bin == HOST_BIN {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{} missing", src.display()),
                ));
            }
        }
        crate::adapters::install::replace_dir(&staging, &version_dir(root, version))
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

fn new_staging(parent: &Path) -> io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let dir = parent.join(format!("{STAGING_PREFIX}{}-{nanos}", std::process::id()));
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

/// What [`seed`] did with the bundled Host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seeded {
    /// First install: the bundled Host is now current.
    Installed(String),
    /// Newer than current: staged for the next applied restart.
    Staged(String),
    /// Nothing to do (already there, or older than current).
    Unchanged,
}

/// Offer the Host shipped in the Desktop bundle (`exe`, version `version`)
/// to this data directory without downloading it again. The first install
/// runs it; a newer one is staged; an older one is ignored (never a
/// downgrade).
pub fn seed(root: &Path, exe: &Path, version: &str) -> io::Result<Seeded> {
    let Some(bundled) = parse(version) else {
        return Ok(Seeded::Unchanged);
    };
    let mut state = read(root);
    if state.blocked.iter().any(|b| b == version) {
        return Ok(Seeded::Unchanged);
    }
    let dir = version_dir(root, version);
    let install = || -> io::Result<()> {
        if !dir.join(HOST_BIN).is_file() {
            let from = exe.parent().unwrap_or(Path::new("."));
            install_from_dir(root, from, version)?;
        }
        Ok(())
    };
    let Some(current) = state.current.clone() else {
        install()?;
        state.current = Some(version.to_string());
        write(root, &state)?;
        return Ok(Seeded::Installed(version.to_string()));
    };
    if state.trial.is_some() || parse(&current).is_some_and(|c| c >= bundled) {
        return Ok(Seeded::Unchanged);
    }
    if let Some(staged) = &state.staged {
        if parse(&staged.version).is_some_and(|s| s >= bundled) {
            return Ok(Seeded::Unchanged);
        }
    }
    install()?;
    state.staged = Some(Staged {
        version: version.to_string(),
        source: StageSource::Bundle,
    });
    write(root, &state)?;
    Ok(Seeded::Staged(version.to_string()))
}

/// Why an archive was not staged.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StageError {
    #[error("the download does not match the signed release")]
    Hash,
    #[error("the Host binary failed its platform signature check: {0}")]
    Platform(String),
    #[error("the Host binary is not the version the release names ({0})")]
    Version(String),
    #[error("{0}")]
    Io(String),
}

impl From<io::Error> for StageError {
    fn from(e: io::Error) -> Self {
        StageError::Io(e.to_string())
    }
}

/// Checks a Host binary must pass before it is run at all.
pub trait PlatformCheck {
    fn check(&self, binary: &Path) -> Result<(), String>;
}

/// Stage a downloaded Host archive for `version`: its hash and size match
/// the signed manifest, every binary passes the platform check, and the Host
/// reports `version` itself. Only then does it get `versions/<version>`.
pub fn stage_archive(
    root: &Path,
    archive: &Path,
    artifact: &Artifact,
    version: &str,
    digest: &str,
    platform: &dyn PlatformCheck,
) -> Result<(), StageError> {
    let size = std::fs::metadata(archive)?.len();
    if size != artifact.size {
        return Err(StageError::Hash);
    }
    crate::adapters::verify::sha256(archive, &artifact.sha256, &artifact.name)
        .map_err(|_| StageError::Hash)?;

    let versions = root.join(VERSIONS);
    std::fs::create_dir_all(&versions)?;
    let staging = new_staging(&versions)?;
    let result = (|| {
        crate::adapters::install::extract(archive, &staging, Some(&[HOST_BIN, MCP_BIN, GH_BIN]))
            .map_err(|e| StageError::Io(e.to_string()))?;
        let host = staging.join(HOST_BIN);
        if !host.is_file() {
            return Err(StageError::Io("the archive has no Host binary".into()));
        }
        for bin in [HOST_BIN, MCP_BIN, GH_BIN] {
            let path = staging.join(bin);
            if path.is_file() {
                platform.check(&path).map_err(StageError::Platform)?;
            }
        }
        // Verified by hash and signature: now it may run, to say its version.
        let reported = reported_version(&host)?;
        if reported != version {
            return Err(StageError::Version(reported));
        }
        crate::adapters::install::replace_dir(&staging, &version_dir(root, version))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
        return result;
    }
    let mut state = read(root);
    state.seen.insert(version.to_string(), digest.to_string());
    state.staged = Some(Staged {
        version: version.to_string(),
        source: StageSource::Download,
    });
    write(root, &state)?;
    Ok(())
}

fn reported_version(host: &Path) -> Result<String, StageError> {
    let out = std::process::Command::new(host)
        .arg("--version")
        .env_remove("ZWIIM_SUPERVISED")
        .output()?;
    if !out.status.success() {
        return Err(StageError::Version(format!("exit {}", out.status)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Remember the manifest digest accepted for `version` (see `manifest::offer`).
pub fn remember(root: &Path, version: &str, digest: &str) -> io::Result<()> {
    let mut state = read(root);
    if state.seen.get(version).map(String::as_str) != Some(digest) {
        state.seen.insert(version.to_string(), digest.to_string());
        write(root, &state)?;
    }
    Ok(())
}

/// Make the staged version current and start its trial. Returns it, or
/// `None` when nothing is staged or its install went missing.
pub fn activate_staged(root: &Path) -> io::Result<Option<String>> {
    let mut state = read(root);
    let (Some(staged), Some(current)) = (state.staged.clone(), state.current.clone()) else {
        return Ok(None);
    };
    if state.trial.is_some() || !version_dir(root, &staged.version).join(HOST_BIN).is_file() {
        return Ok(None);
    }
    state.trial = Some(Trial {
        version: staged.version.clone(),
        from: current.clone(),
        previous_before: state.previous.take(),
        started_at: now(),
        migrated: false,
        snapshot: None,
    });
    state.previous = Some(current);
    state.current = Some(staged.version.clone());
    state.staged = None;
    write(root, &state)?;
    Ok(Some(staged.version))
}

/// The trial's worker records what its database start did, so a rollback
/// knows whether to restore the snapshot.
pub fn note_migration(root: &Path, running: &str, outcome: &MigrationOutcome) -> io::Result<()> {
    let mut state = read(root);
    let Some(trial) = state.trial.as_mut().filter(|t| t.version == running) else {
        return Ok(());
    };
    if outcome.applied.is_empty() {
        return Ok(());
    }
    trial.migrated = true;
    trial.snapshot = outcome.snapshot.clone();
    write(root, &state)
}

/// The trial passed: `current` is known-good. Older installs beyond
/// `previous` are removed.
pub fn commit(root: &Path, running: &str) -> io::Result<Option<String>> {
    let mut state = read(root);
    if state.trial.as_ref().is_none_or(|t| t.version != running) {
        return Ok(None);
    }
    let Some(trial) = state.trial.take() else {
        return Ok(None);
    };
    state.last = Some(LastResult {
        outcome: Outcome::Applied,
        version: trial.version.clone(),
        from: trial.from,
        reason: None,
        at: now(),
    });
    write(root, &state)?;
    prune(root, &state);
    Ok(Some(trial.version))
}

/// Undo an unproven trial: restore the database if the trial migrated it,
/// return to the version it replaced, and block the failed one. Call only
/// while no worker runs.
pub fn rollback(root: &Path, data_dir: &Path, reason: &str) -> Result<String, String> {
    let mut state = read(root);
    let trial = state
        .trial
        .clone()
        .ok_or_else(|| "no update is on trial".to_string())?;
    if !version_dir(root, &trial.from).join(HOST_BIN).is_file() {
        return Err(format!("version {} is no longer installed", trial.from));
    }
    // A worker can die between migrating and recording it: any
    // pre-migration snapshot taken during the trial counts.
    match trial
        .snapshot
        .clone()
        .or_else(|| trial_snapshot(data_dir, &trial))
    {
        Some(snapshot) => {
            crate::db::schema_guard::restore(data_dir, crate::db::database_file_name(), &snapshot)
                .map_err(|e| format!("restoring the database failed: {e}"))?;
        }
        None if trial.migrated => {
            return Err("the trial migrated the database without a snapshot".to_string());
        }
        None => {}
    }
    state.current = Some(trial.from.clone());
    state.previous = trial.previous_before.clone();
    state.trial = None;
    if !state.blocked.contains(&trial.version) {
        state.blocked.push(trial.version.clone());
    }
    state.last = Some(LastResult {
        outcome: Outcome::RolledBack,
        version: trial.version,
        from: trial.from.clone(),
        reason: Some(reason.to_string()),
        at: now(),
    });
    write(root, &state).map_err(|e| e.to_string())?;
    Ok(trial.from)
}

/// The newest pre-migration snapshot written after `trial` began.
fn trial_snapshot(data_dir: &Path, trial: &Trial) -> Option<PathBuf> {
    let started = chrono::DateTime::parse_from_rfc3339(&trial.started_at).ok()?;
    crate::db::schema_guard::snapshots(data_dir)
        .into_iter()
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            let Some(stamp) = name.strip_suffix("-pre-migration.db") else {
                return false;
            };
            chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%S%.3fZ")
                .is_ok_and(|t| t.and_utc() >= started)
        })
        .next_back()
}

/// Remove every install other than current, previous and staged, and any
/// leftover staging directories.
fn prune(root: &Path, state: &State) {
    let keep: Vec<&str> = [
        state.current.as_deref(),
        state.previous.as_deref(),
        state.staged.as_ref().map(|s| s.version.as_str()),
    ]
    .into_iter()
    .flatten()
    .collect();
    let Ok(entries) = std::fs::read_dir(root.join(VERSIONS)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // Dot directories are work in flight (a download being staged).
        if !name.starts_with('.') && !keep.contains(&name.as_str()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

// ─── Process-level entry points (supervisor and worker) ──────────────────
//
// `ZWIIM_DATA_DIR` is resolved and pinned by `main` before either runs.

/// Whether this Host runs from versioned installs. Release builds do; a
/// development build keeps running the binary just built (set
/// `ZWIIM_VERSIONED_INSTALL=1` to rehearse updates with one).
pub fn versioned_installs() -> bool {
    !cfg!(debug_assertions) || std::env::var("ZWIIM_VERSIONED_INSTALL").is_ok_and(|v| v == "1")
}

/// Set by the supervisor for a worker whose Host came in the Desktop app:
/// Desktop's own update then brings the next Host, so it is not downloaded
/// separately unless someone asks.
pub const ENV_BUNDLED: &str = "ZWIIM_BUNDLED_HOST";

/// Whether `exe` sits inside a macOS app bundle.
pub fn in_app_bundle(exe: &Path) -> bool {
    exe.to_string_lossy().contains(".app/Contents/MacOS/")
}

fn env_data_dir() -> Option<PathBuf> {
    std::env::var_os("ZWIIM_DATA_DIR").map(PathBuf::from)
}

/// Whether the current version is on trial (the supervisor watches it).
pub fn upgrade_staged() -> bool {
    env_data_dir().is_some_and(|d| read(&root(&d)).trial.is_some())
}

/// Commit the trial of the running version. Returns whether one was open.
pub fn take_upgrade_staged() -> bool {
    env_data_dir()
        .and_then(|d| commit(&root(&d), env!("CARGO_PKG_VERSION")).ok().flatten())
        .inspect(|v| {
            tracing::info!("[update] {v} passed its trial and is now the known-good version")
        })
        .is_some()
}

/// Supervisor: roll an unproven version back.
pub fn rollback_current(reason: &str) -> Result<(), AppCommandError> {
    let data_dir = env_data_dir().ok_or_else(|| {
        AppCommandError::new(AppErrorCode::DependencyMissing, "no data directory")
    })?;
    rollback(&root(&data_dir), &data_dir, reason)
        .map(|restored| tracing::info!("[update] rolled back to {restored}: {reason}"))
        .map_err(|e| AppCommandError::new(AppErrorCode::DependencyMissing, e))
}

/// Worker: record the database start of a trial.
pub fn note_database_start(outcome: &MigrationOutcome) {
    if let Some(d) = env_data_dir() {
        if let Err(e) = note_migration(&root(&d), env!("CARGO_PKG_VERSION"), outcome) {
            tracing::error!("[update] could not record the trial's migration: {e}");
        }
    }
}
