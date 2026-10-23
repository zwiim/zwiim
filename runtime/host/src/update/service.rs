//! The Host's update engine: discover, verify, download, stage; then apply
//! on a restart and prove the new version in a trial.
//!
//! - **Discover.** On GitHub Releases, the stable channel's newest release
//!   is `<releases>/latest/download/release.json` (+ `.sig`); the beta
//!   channel's is `<releases>/download/beta/release.json`, a release the
//!   release tooling moves to the newest beta or stable version. The channel
//!   is the Host's own setting (`host.update.set_channel`). Checked a few
//!   minutes after start, then every 6 hours, and on `host.update.check`.
//!   Offline is not an error anyone must act on.
//! - **Verify.** The manifest's signature against the release key compiled
//!   into this build (a build without one never updates), then the policy in
//!   `manifest::offer`: newer only, no beta on stable, not blocked, not
//!   changed.
//! - **Download and stage.** The Host archive is fetched and staged by
//!   `install::stage_archive` (hash, platform signature, self-reported
//!   version). A version the Desktop bundle already provided is not
//!   downloaded again.
//! - **Apply.** `host.update.apply` restarts through the existing drain
//!   (`host_lifecycle`); on the way out the staged version becomes current
//!   and starts its trial. Only a supervised Host can apply.
//! - **Trial.** The new worker checks its own health over its API
//!   (`host.hello`), then commits after `ZWIIM_UPGRADE_TRIAL_SECS`. The
//!   supervisor rolls back anything that does not get there.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use semver::Version;
use serde::Serialize;
use serde_json::{json, Value};

use super::install::{self, LastResult, PlatformCheck, StageSource};
use super::manifest::{self, Channel, Offer, Refusal};
use super::{runtime, signature};
use crate::adapters::install::Fetch;

/// Where releases are published. Overridable at build time and, for
/// rehearsals, at run time: the signature, not the origin, is the trust.
const DEFAULT_RELEASES_URL: &str = "https://github.com/Ashish1101/zwiim-releases/releases";
const RELEASES_URL_ENV: &str = "ZWIIM_RELEASE_URL";
/// `off` disables the periodic check (tests, air-gapped installs).
const CHECK_ENV: &str = "ZWIIM_UPDATE_CHECK";
/// The Apple Team ID release binaries are signed by, compiled in by the
/// release build. Without it the platform signature is not enforced.
const TEAM_ID: Option<&str> = option_env!("ZWIIM_APPLE_TEAM_ID");

/// A compile-time setting, where set-but-empty (an unset CI variable) means
/// unset: an empty Team ID would refuse every update.
pub(super) fn build_setting(value: Option<&'static str>) -> Option<&'static str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

fn team_id() -> Option<&'static str> {
    build_setting(TEAM_ID)
}

/// The Host's macOS code-signing identifier, embedded in the executable by
/// `build.rs` so every signer produces it. The Keychain ties the Host's
/// items to this identifier and the Team ID: an update that changed either
/// could no longer read them without prompting.
pub const HOST_SIGNING_IDENTIFIER: &str = "com.zwiim.host";

const FIRST_CHECK_DELAY: Duration = Duration::from_secs(120);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(30);

pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is semver")
}

/// The channel of the running version: a beta install follows beta.
pub(super) fn own_channel() -> Channel {
    if current_version().pre.is_empty() {
        Channel::Stable
    } else {
        Channel::Beta
    }
}

/// The channel this Host follows: the one saved, else the running
/// version's.
fn channel(root: &Path) -> Channel {
    install::channel(root).unwrap_or_else(own_channel)
}

/// [`channel`], saved on first check so a beta tester stays on beta once a
/// stable release replaces the beta they started with.
pub(super) fn follow(root: &Path) -> Channel {
    let c = channel(root);
    if install::channel(root).is_none() {
        if let Err(e) = install::set_channel(root, c) {
            tracing::warn!("[update] could not save the update channel: {e}");
        }
    }
    c
}

fn releases_url() -> String {
    std::env::var(RELEASES_URL_ENV)
        .ok()
        .filter(|u| !u.trim().is_empty())
        .or_else(|| build_setting(option_env!("ZWIIM_RELEASE_URL")).map(str::to_string))
        .unwrap_or_else(|| DEFAULT_RELEASES_URL.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn data_dir() -> Option<PathBuf> {
    std::env::var_os("ZWIIM_DATA_DIR").map(PathBuf::from)
}

/// Why this Host cannot update itself, if it cannot.
fn unsupported_reason() -> Option<&'static str> {
    match signature::release_key() {
        None => return Some("unsigned_build"),
        Some(Err(_)) => return Some("invalid_release_key"),
        Some(Ok(_)) => {}
    }
    if manifest::host_target().is_none() {
        return Some("unsupported_platform");
    }
    if !runtime::is_supervised() || !install::versioned_installs() {
        return Some("unsupervised");
    }
    None
}

/// What the engine is doing now. Persistent facts live in `install::State`.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Activity {
    #[default]
    Idle,
    Checking,
    Downloading {
        version: String,
    },
    Failed {
        reason: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Available {
    pub version: String,
    pub released_at: String,
    pub notes: String,
}

#[derive(Default)]
struct Engine {
    activity: Activity,
    available: Option<Available>,
    last_checked: Option<String>,
}

static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);
static CHECKING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
/// Set by `host.update.apply`: the next restart switches to the staged version.
static APPLY_ON_RESTART: AtomicBool = AtomicBool::new(false);

fn with_engine<R>(f: impl FnOnce(&mut Engine) -> R) -> R {
    let mut guard = ENGINE.lock().unwrap_or_else(|p| p.into_inner());
    f(guard.get_or_insert_with(Engine::default))
}

/// Where a channel's newest release manifest is published.
pub(super) fn discovery_url(releases: &str, channel: Channel) -> String {
    match channel {
        Channel::Stable => format!("{releases}/latest/download"),
        Channel::Beta => format!("{releases}/download/beta"),
    }
}

/// `host.update.status`.
pub fn status() -> Value {
    let root = data_dir().map(|d| install::root(&d));
    let state = root.as_deref().map(install::read).unwrap_or_default();
    let channel = root.as_deref().map(channel).unwrap_or_else(own_channel);
    let (activity, available, last_checked) = with_engine(|e| {
        (
            e.activity.clone(),
            e.available.clone(),
            e.last_checked.clone(),
        )
    });
    let unsupported = unsupported_reason();
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "channel": channel.as_str(),
        "supported": unsupported.is_none(),
        "unsupported_reason": unsupported,
        "activity": activity,
        "available": available,
        "staged": state.staged,
        "restart_required": state.staged.is_some(),
        "trial": state.trial.as_ref().map(|t| json!({ "version": t.version, "from": t.from })),
        "previous": state.previous,
        "last_result": state.last.as_ref().map(last_result),
        "last_checked": last_checked,
        "platform_signature": if team_id().is_some() { "enforced" } else { "not_enforced" },
        "bundled_with_desktop": bundled_with_desktop(),
    })
}

fn last_result(r: &LastResult) -> Value {
    json!({ "outcome": r.outcome, "version": r.version, "from": r.from, "reason": r.reason, "at": r.at })
}

/// Start the periodic check, if this Host can update itself.
pub fn start_background_checks() {
    if std::env::var(CHECK_ENV).is_ok_and(|v| v == "off") {
        return;
    }
    if let Some(reason) = unsupported_reason() {
        tracing::info!("[update] self-update is off for this Host ({reason})");
        return;
    }
    tokio::spawn(async {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            check_with(false).await;
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

/// A Host that came in the Desktop app gets its next version from Desktop's
/// own update; it downloads one itself only when someone asks.
fn bundled_with_desktop() -> bool {
    std::env::var(install::ENV_BUNDLED).is_ok_and(|v| v == "1")
}

/// `host.update.check`: look for a newer release and stage it.
pub async fn check() -> Value {
    check_with(true).await
}

async fn check_with(explicit: bool) -> Value {
    if unsupported_reason().is_some() {
        return status();
    }
    let Ok(_guard) = CHECKING.try_lock() else {
        return status();
    };
    with_engine(|e| e.activity = Activity::Checking);
    let result = check_inner(explicit || !bundled_with_desktop()).await;
    with_engine(|e| {
        e.last_checked = Some(chrono::Utc::now().to_rfc3339());
        e.activity = match result {
            Ok(()) => Activity::Idle,
            Err((reason, message)) => {
                tracing::warn!("[update] {reason}: {message}");
                Activity::Failed {
                    reason: reason.to_string(),
                    message,
                }
            }
        };
    });
    status()
}

type CheckError = (&'static str, String);

async fn check_inner(download: bool) -> Result<(), CheckError> {
    let key = match signature::release_key() {
        Some(Ok(key)) => key,
        _ => return Err(("unsigned_build", "this build trusts no release key".into())),
    };
    let data_dir = data_dir().ok_or(("install", "no data directory".to_string()))?;
    let root = install::root(&data_dir);
    let channel = follow(&root);
    let base = discovery_url(&releases_url(), channel);
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("zwiim-host/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| ("network", e.to_string()))?;
    let fetch = |name: &'static str| {
        let client = client.clone();
        let url = format!("{base}/{name}");
        async move {
            let res = client
                .get(&url)
                .send()
                .await
                .map_err(|e| ("network", e.to_string()))?;
            if res.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok(None);
            }
            if !res.status().is_success() {
                return Err(("network", format!("{url}: HTTP {}", res.status())));
            }
            res.bytes()
                .await
                .map(Some)
                .map_err(|e| ("network", e.to_string()))
        }
    };
    let Some(bytes) = fetch(manifest::MANIFEST_ASSET).await? else {
        // Nothing published on this channel yet.
        with_engine(|e| e.available = None);
        return Ok(());
    };
    let sig = fetch(manifest::SIGNATURE_ASSET)
        .await?
        .ok_or(("verification", "the release has no signature".to_string()))?;
    let sig = String::from_utf8_lossy(&sig).to_string();
    let verified = manifest::verify(&key, &bytes, &sig).map_err(|e| match e {
        manifest::ManifestError::UnsupportedSchema(_) => ("unsupported_manifest", e.to_string()),
        _ => ("verification", e.to_string()),
    })?;

    let state = install::read(&root);
    let version = verified.manifest.version.clone();
    let offer = manifest::offer(
        &verified,
        &current_version(),
        channel,
        state.seen.get(&version).map(String::as_str),
        state.blocked.contains(&version),
    );
    match offer {
        Offer::UpToDate => {
            with_engine(|e| e.available = None);
            return Ok(());
        }
        Offer::Refused(Refusal::Blocked) => {
            // Already failed here once: not offered, not an error.
            with_engine(|e| e.available = None);
            return Ok(());
        }
        Offer::Refused(refusal) => {
            let reason = match refusal {
                Refusal::Changed => "release_changed",
                Refusal::NeedsManualInstall { .. } => "needs_manual_install",
                Refusal::Channel => "verification",
                Refusal::NoArtifact => "unsupported_platform",
                Refusal::Blocked => unreachable!(),
            };
            return Err((reason, format!("release {version} was not accepted")));
        }
        Offer::Update => {}
    }
    install::remember(&root, &version, &verified.digest).map_err(|e| ("install", e.to_string()))?;
    let m = &verified.manifest;
    with_engine(|e| {
        e.available = Some(Available {
            version: version.clone(),
            released_at: m.released_at.clone(),
            notes: m.notes.clone(),
        })
    });

    if state.staged.as_ref().is_some_and(|s| s.version == version) {
        return Ok(());
    }
    if install::version_dir(&root, &version)
        .join(install::HOST_BIN)
        .is_file()
    {
        // Provided by the Desktop bundle: no second download.
        let mut state = install::read(&root);
        state.staged = Some(install::Staged {
            version,
            source: StageSource::Bundle,
        });
        install::write(&root, &state).map_err(|e| ("install", e.to_string()))?;
        return Ok(());
    }

    if !download {
        tracing::info!("[update] Host {version} is available; it arrives with the Desktop update");
        return Ok(());
    }
    let artifact = m
        .host_artifact()
        .expect("offer checked the artifact")
        .clone();
    with_engine(|e| {
        e.activity = Activity::Downloading {
            version: version.clone(),
        }
    });
    let downloads = root.join("downloads");
    std::fs::create_dir_all(&downloads).map_err(|e| ("install", e.to_string()))?;
    let archive = downloads.join(format!("{}.part", artifact.name));
    let fetched = crate::adapters::HttpFetch::new()
        .fetch(&artifact.url, &archive)
        .await;
    let staged = match fetched {
        Ok(()) => install::stage_archive(
            &root,
            &archive,
            &artifact,
            &version,
            &verified.digest,
            &Codesign,
        )
        .map_err(|e| match e {
            install::StageError::Hash
            | install::StageError::Platform(_)
            | install::StageError::Version(_) => ("verification", e.to_string()),
            install::StageError::Io(m) => ("install", m),
        }),
        Err(e) => Err(("network", e.to_string())),
    };
    let _ = std::fs::remove_file(&archive);
    staged?;
    tracing::info!(
        "[update] Host {version} is verified and staged; it applies on the next update restart"
    );
    Ok(())
}

/// `host.update.set_channel`: follow `channel` from the next check. What
/// the old channel offered is forgotten; nothing installed changes, and
/// leaving beta keeps the running version until a newer stable one ships.
pub fn set_channel(channel: Channel) -> std::io::Result<Value> {
    let root =
        install::root(&data_dir().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no data directory")
        })?);
    let before = self::channel(&root);
    if install::channel(&root) != Some(channel) {
        install::set_channel(&root, channel)?;
    }
    if before != channel {
        with_engine(|e| {
            e.available = None;
            if matches!(e.activity, Activity::Failed { .. }) {
                e.activity = Activity::Idle;
            }
        });
    }
    Ok(status())
}

/// The platform signature check: on macOS, a valid code signature from the
/// Zwiim Team ID, when this build names one, and for the Host, its
/// identifier.
pub struct Codesign;

impl PlatformCheck for Codesign {
    fn check(&self, binary: &Path) -> Result<(), String> {
        let Some(team) = team_id() else {
            tracing::warn!("[update] platform signature not enforced: this build names no Team ID");
            return Ok(());
        };
        if !cfg!(target_os = "macos") {
            return Ok(());
        }
        let codesign = "/usr/bin/codesign";
        let verify = std::process::Command::new(codesign)
            .args(["--verify", "--strict"])
            .arg(binary)
            .output()
            .map_err(|e| e.to_string())?;
        if !verify.status.success() {
            return Err(String::from_utf8_lossy(&verify.stderr).trim().to_string());
        }
        let info = std::process::Command::new(codesign)
            .args(["-d", "--verbose=2"])
            .arg(binary)
            .output()
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&info.stderr);
        let is_host = binary.file_name() == Some(std::ffi::OsStr::new(install::HOST_BIN));
        signed_by(&text, team, is_host.then_some(HOST_SIGNING_IDENTIFIER))
    }
}

/// Whether `codesign -dv` output names `team` and, when given, `identifier`.
pub(super) fn signed_by(details: &str, team: &str, identifier: Option<&str>) -> Result<(), String> {
    let has = |want: String| details.lines().any(|l| l.trim() == want);
    if !has(format!("TeamIdentifier={team}")) {
        return Err(format!("not signed by Team ID {team}"));
    }
    match identifier {
        Some(id) if !has(format!("Identifier={id}")) => Err(format!("not signed as {id}")),
        _ => Ok(()),
    }
}

/// Why `host.update.apply` cannot run now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    Unsupervised,
    NothingStaged,
}

/// Arm the switch to the staged version for the next restart.
pub fn arm_apply() -> Result<String, ApplyError> {
    if !runtime::is_supervised() || !install::versioned_installs() {
        return Err(ApplyError::Unsupervised);
    }
    let staged = data_dir()
        .map(|d| install::read(&install::root(&d)))
        .and_then(|s| s.staged)
        .ok_or(ApplyError::NothingStaged)?;
    APPLY_ON_RESTART.store(true, Ordering::SeqCst);
    Ok(staged.version)
}

/// On a supervised restart: switch to the staged version if the restart was
/// asked to apply it. Returns the version the supervisor will start.
pub fn activate_if_armed() -> Option<String> {
    if !APPLY_ON_RESTART.swap(false, Ordering::SeqCst) {
        return None;
    }
    let root = install::root(&data_dir()?);
    match install::activate_staged(&root) {
        Ok(Some(version)) => {
            tracing::info!("[update] switching to Host {version}; it starts on trial");
            Some(version)
        }
        Ok(None) => None,
        Err(e) => {
            tracing::error!("[update] could not switch to the staged version: {e}");
            None
        }
    }
}

/// Worker, on trial: check health over the API just bound, then commit
/// after the trial window. An unhealthy version stops itself; the
/// supervisor then rolls it back.
pub fn run_trial(
    url: String,
    token: String,
    lifecycle: std::sync::Arc<crate::host_lifecycle::Lifecycle>,
) {
    if !runtime::is_supervised() || !install::upgrade_staged() {
        return;
    }
    tokio::spawn(async move {
        match health_check(&url, &token).await {
            Ok(()) => tracing::info!(
                "[update] health check passed; trial runs {}s",
                runtime::upgrade_trial_secs()
            ),
            Err(e) => {
                tracing::error!("[update] health check failed: {e}; stopping for rollback");
                lifecycle.request(crate::host_lifecycle::ShutdownKind::Stop);
                return;
            }
        }
        tokio::time::sleep(Duration::from_secs(runtime::upgrade_trial_secs())).await;
        if lifecycle.requested().is_none() {
            install::take_upgrade_staged();
        }
    });
}

/// The Host answers `host.hello` over its own API as the version it is,
/// with its identity: the database opened, migrations ran, the API is bound
/// and the protocol layer serves.
async fn health_check(url: &str, token: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let deadline = tokio::time::Instant::now() + HEALTH_TIMEOUT;
    let mut last = String::new();
    while tokio::time::Instant::now() < deadline {
        let res = client
            .post(format!("{url}/zwiim/v1/host.hello"))
            .bearer_auth(token)
            .json(&json!({}))
            .send()
            .await;
        match res {
            Ok(res) if res.status().is_success() => {
                let body: Value = res.json().await.map_err(|e| e.to_string())?;
                let version_ok = body["version"] == env!("CARGO_PKG_VERSION");
                let identity_ok = body["host_id"].as_str().is_some_and(|h| !h.is_empty());
                if version_ok && identity_ok {
                    return Ok(());
                }
                last = format!("unexpected hello: {body}");
            }
            Ok(res) => last = format!("HTTP {}", res.status()),
            Err(e) => last = e.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(last)
}

/// `zwiim-host --release-info`: the facts a release manifest records about
/// this build, so the manifest is generated from the binary it ships.
pub fn release_info() -> Value {
    let adapters: serde_json::Map<String, Value> = crate::adapters::ManagedAdapter::ALL
        .iter()
        .filter_map(|a| {
            Some((
                a.id().to_string(),
                json!({ "version": a.locked_version()? }),
            ))
        })
        .collect();
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "target": manifest::host_target(),
        "protocol": {
            "major": 1,
            "revision": crate::protocol::compat::PROTOCOL_REVISION,
            "min_client_revision": crate::protocol::compat::MIN_CLIENT_REVISION,
            "capabilities": crate::protocol::compat::CAPABILITIES,
        },
        "adapters": adapters,
        "release_key": signature::release_key().map(|k| k.is_ok()),
        "team_id": team_id(),
    })
}

/// `zwiim-host --verify-release`: verify a manifest file and its signature
/// with this build's release key.
pub fn verify_release_files(manifest_path: &Path, sig_path: &Path) -> Result<String, String> {
    let key = match signature::release_key() {
        Some(Ok(key)) => key,
        Some(Err(e)) => return Err(format!("invalid release key in this build: {e}")),
        None => return Err("this build has no release key (ZWIIM_RELEASE_PUBKEY)".into()),
    };
    let bytes = std::fs::read(manifest_path).map_err(|e| e.to_string())?;
    let sig = std::fs::read_to_string(sig_path).map_err(|e| e.to_string())?;
    let verified = manifest::verify(&key, &bytes, &sig).map_err(|e| e.to_string())?;
    if verified.manifest.version != env!("CARGO_PKG_VERSION") {
        return Err(format!(
            "manifest is for {}, this build is {}",
            verified.manifest.version,
            env!("CARGO_PKG_VERSION")
        ));
    }
    Ok(verified.manifest.version)
}
