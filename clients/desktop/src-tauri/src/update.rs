//! Desktop self-update through `tauri-plugin-updater`.
//!
//! Upstream-derived: the shape follows upstream `src-tauri/src/commands/app_update.rs`
//! (v0.31.1; see NOTICE): the download is driven from
//! Rust, so its progress survives the page that started it, and the UI only
//! reads a snapshot. Changed: the key and the endpoint are Zwiim's, compiled
//! in by the release build; a build without the key never updates.
//!
//! Desktop downloads Desktop only. The Host it bundles is handed to the Host's
//! own installer (`zwiim-host --seed`) as soon as the update is installed, and
//! again on the next launch, which stages it without a second download; the
//! UI's one Update then asks the Host to switch once idle and relaunches.
//!
//! Channels: stable reads `<releases>/latest/download/latest.json`; beta
//! reads `<releases>/download/beta/latest.json`, a release the release
//! tooling moves to the newest beta or stable version. Until someone
//! chooses, the app follows its own version's channel (a pre-release is
//! beta), saved on first use so a beta install stays on beta after a stable
//! release ships. The choice is saved in the app's config directory; on this
//! computer, Settings sets the Host's channel with it, so the Host Desktop
//! brings matches. A channel with nothing published reads as up to date.
//!
//! Trust: `latest.json` is published next to `release.json` on the GitHub
//! Release. The archive must carry a signature from the release key, and the
//! signature must name the announced version (`requireSignedVersion` in
//! `tauri.conf.json`), so a tampered `latest.json` cannot pair a new version
//! number with an old release. The plugin never installs an older version.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri_plugin_updater::UpdaterExt;

/// The release public key (Tauri form), compiled in by the release build.
pub const RELEASE_PUBKEY: Option<&str> = option_env!("ZWIIM_RELEASE_PUBKEY");
const DEFAULT_RELEASES_URL: &str = "https://github.com/zwiim/zwiim/releases";
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

fn release_key() -> Option<&'static str> {
    RELEASE_PUBKEY.filter(|k| !k.trim().is_empty())
}

/// Same resolution as the Host: run-time `ZWIIM_RELEASE_URL` (rehearsals),
/// else the build's, else the project's GitHub Releases.
fn releases_url() -> String {
    std::env::var("ZWIIM_RELEASE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
        .or_else(|| {
            option_env!("ZWIIM_RELEASE_URL")
                .filter(|u| !u.trim().is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| DEFAULT_RELEASES_URL.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// The updater plugin, keyed by the release key when this build has one.
pub fn plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R, tauri_plugin_updater::Config> {
    let builder = tauri_plugin_updater::Builder::new();
    match release_key() {
        Some(key) => builder.pubkey(key),
        None => builder,
    }
    .build()
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Activity {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available {
        version: String,
        notes: Option<String>,
    },
    Downloading {
        version: String,
        downloaded: u64,
        total: Option<u64>,
    },
    /// Installed; the app restarts into it.
    Ready {
        version: String,
    },
    Failed {
        message: String,
    },
}

/// Which releases this app updates to. Beta takes stable releases too.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    #[default]
    Stable,
    Beta,
}

#[derive(Serialize, Deserialize)]
struct ChannelChoice {
    channel: Channel,
}

fn channel_path(config_dir: &Path) -> PathBuf {
    config_dir.join("update-channel.json")
}

/// The saved choice, if any.
fn read_channel(config_dir: &Path) -> Option<Channel> {
    std::fs::read(channel_path(config_dir))
        .ok()
        .and_then(|b| serde_json::from_slice::<ChannelChoice>(&b).ok())
        .map(|c| c.channel)
}

/// The channel a `version` belongs to: a pre-release is beta.
fn own_channel(version: &str) -> Channel {
    if version.split('+').next().unwrap_or(version).contains('-') {
        Channel::Beta
    } else {
        Channel::Stable
    }
}

fn write_channel(config_dir: &Path, channel: Channel) -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir)?;
    std::fs::write(
        channel_path(config_dir),
        serde_json::to_vec(&ChannelChoice { channel })?,
    )
}

fn config_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    app.path().app_config_dir().map_err(|e| e.to_string())
}

/// Where a channel's `latest.json` is published.
fn endpoint(releases: &str, channel: Channel) -> String {
    match channel {
        Channel::Stable => format!("{releases}/latest/download/latest.json"),
        Channel::Beta => format!("{releases}/download/beta/latest.json"),
    }
}

#[derive(Default)]
pub struct DesktopUpdate {
    activity: Mutex<Activity>,
    /// Read from the config directory on first use.
    channel: Mutex<Option<Channel>>,
}

impl DesktopUpdate {
    fn get(&self) -> Activity {
        self.activity
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    fn set(&self, a: Activity) {
        *self.activity.lock().unwrap_or_else(|p| p.into_inner()) = a;
    }
    fn channel<R: tauri::Runtime>(&self, app: &tauri::AppHandle<R>) -> Channel {
        *self
            .channel
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_or_insert_with(|| {
                let dir = config_dir(app).ok();
                if let Some(saved) = dir.as_deref().and_then(read_channel) {
                    return saved;
                }
                let own = own_channel(env!("CARGO_PKG_VERSION"));
                if let Some(dir) = dir {
                    if let Err(e) = write_channel(&dir, own) {
                        eprintln!("zwiim: could not save the update channel: {e}");
                    }
                }
                own
            })
    }
}

#[derive(Serialize)]
pub struct Status {
    version: &'static str,
    supported: bool,
    /// `unsigned_build`: made outside the release pipeline, never updates.
    unsupported_reason: Option<&'static str>,
    channel: Channel,
    activity: Activity,
}

fn status_of(state: &DesktopUpdate, channel: Channel) -> Status {
    let supported = release_key().is_some();
    Status {
        version: env!("CARGO_PKG_VERSION"),
        supported,
        unsupported_reason: (!supported).then_some("unsigned_build"),
        channel,
        activity: state.get(),
    }
}

fn updater<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<tauri_plugin_updater::Updater, String> {
    let channel = app.state::<DesktopUpdate>().channel(app);
    let url = url::Url::parse(&endpoint(&releases_url(), channel)).map_err(|e| e.to_string())?;
    app.updater_builder()
        .endpoints(vec![url])
        .map_err(|e| e.to_string())?
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())
}

async fn find<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Option<tauri_plugin_updater::Update>, String> {
    match tokio::time::timeout(CHECK_TIMEOUT, updater(app)?.check())
        .await
        .map_err(|_| "The update check timed out".to_string())?
    {
        // The channel has nothing published yet.
        Err(tauri_plugin_updater::Error::ReleaseNotFound) => Ok(None),
        result => result.map_err(|e| e.to_string()),
    }
}

#[tauri::command]
pub fn desktop_update_status(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopUpdate>,
) -> Status {
    status_of(&state, state.channel(&app))
}

/// Follow `channel` from the next check. What the old channel offered is
/// forgotten; an update already downloading or installed stays.
#[tauri::command]
pub fn desktop_update_set_channel(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopUpdate>,
    channel: Channel,
) -> Result<Status, String> {
    let before = state.channel(&app);
    let dir = config_dir(&app)?;
    if read_channel(&dir) != Some(channel) {
        write_channel(&dir, channel).map_err(|e| e.to_string())?;
    }
    *state.channel.lock().unwrap_or_else(|p| p.into_inner()) = Some(channel);
    if before != channel {
        if matches!(
            state.get(),
            Activity::UpToDate | Activity::Available { .. } | Activity::Failed { .. }
        ) {
            state.set(Activity::Idle);
        }
    }
    Ok(status_of(&state, channel))
}

/// Look for a newer Desktop. Never downloads.
#[tauri::command]
pub async fn desktop_update_check(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopUpdate>,
) -> Result<Status, String> {
    if release_key().is_none() {
        return Ok(status_of(&state, state.channel(&app)));
    }
    if matches!(
        state.get(),
        Activity::Downloading { .. } | Activity::Ready { .. }
    ) {
        return Ok(status_of(&state, state.channel(&app)));
    }
    state.set(Activity::Checking);
    state.set(match find(&app).await {
        Ok(Some(update)) => Activity::Available {
            version: update.version,
            notes: update.body,
        },
        Ok(None) => Activity::UpToDate,
        Err(message) => Activity::Failed { message },
    });
    Ok(status_of(&state, state.channel(&app)))
}

/// Download, verify and install the available update; the UI then offers
/// the restart. Returns at once: progress is in `desktop_update_status`.
#[tauri::command]
pub async fn desktop_update_install(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopUpdate>,
) -> Result<Status, String> {
    let Activity::Available { version, .. } = state.get() else {
        return Err("No Desktop update is available".into());
    };
    state.set(Activity::Downloading {
        version: version.clone(),
        downloaded: 0,
        total: None,
    });
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = handle.state::<DesktopUpdate>();
        let result = async {
            // Re-check: the release could have changed since the check.
            let update = find(&handle)
                .await?
                .ok_or("No Desktop update is available")?;
            let version = update.version.clone();
            let mut downloaded = 0u64;
            update
                .download_and_install(
                    |chunk, total| {
                        downloaded += chunk as u64;
                        handle.state::<DesktopUpdate>().set(Activity::Downloading {
                            version: version.clone(),
                            downloaded,
                            total,
                        });
                    },
                    || {},
                )
                .await
                .map_err(|e| e.to_string())?;
            // The installed bundle carries the next Host: stage it now, so
            // the restart that applies this update can switch the Host too.
            let _ = tauri::async_runtime::spawn_blocking(crate::seed_host).await;
            Ok::<_, String>(update.version)
        }
        .await;
        state.set(match result {
            Ok(version) => Activity::Ready { version },
            Err(message) => Activity::Failed { message },
        });
    });
    Ok(status_of(&state, state.channel(&app)))
}

/// Relaunch into the installed update. The Host keeps running; the UI has
/// already asked it to switch to the staged version once idle.
#[tauri::command]
pub fn desktop_update_restart(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopUpdate>,
) -> Result<(), String> {
    if !matches!(state.get(), Activity::Ready { .. }) {
        return Err("No installed Desktop update to restart into".into());
    }
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_without_the_release_key_never_updates() {
        let status = status_of(&DesktopUpdate::default(), Channel::Stable);
        assert!(!status.supported);
        assert_eq!(status.unsupported_reason, Some("unsigned_build"));
    }

    #[test]
    fn the_endpoint_is_the_channels_latest_release() {
        std::env::set_var("ZWIIM_RELEASE_URL", "http://127.0.0.1:9/releases/");
        assert_eq!(releases_url(), "http://127.0.0.1:9/releases");
        std::env::remove_var("ZWIIM_RELEASE_URL");
        let base = "https://github.com/o/r/releases";
        assert_eq!(
            endpoint(base, Channel::Stable),
            "https://github.com/o/r/releases/latest/download/latest.json"
        );
        assert_eq!(
            endpoint(base, Channel::Beta),
            "https://github.com/o/r/releases/download/beta/latest.json"
        );
    }

    #[test]
    fn the_channel_is_unset_until_chosen() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        assert_eq!(read_channel(&config), None);
        write_channel(&config, Channel::Beta).unwrap();
        assert_eq!(read_channel(&config), Some(Channel::Beta));
        std::fs::write(channel_path(&config), b"{").unwrap();
        assert_eq!(read_channel(&config), None);
    }

    #[test]
    fn a_pre_release_follows_beta() {
        assert_eq!(own_channel("0.1.0"), Channel::Stable);
        assert_eq!(own_channel("0.1.0+build-7"), Channel::Stable);
        assert_eq!(own_channel("0.1.0-rc.8"), Channel::Beta);
    }
}
