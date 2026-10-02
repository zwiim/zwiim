//! Desktop self-update through `tauri-plugin-updater`.
//!
//! Upstream-derived: the shape follows upstream `src-tauri/src/commands/app_update.rs`
//! (v0.31.1; see third_party/codeg/PROVENANCE.md): the download is driven from
//! Rust, so its progress survives the page that started it, and the UI only
//! reads a snapshot. Changed: the key and the endpoint are Zwiim's, compiled
//! in by the release build; a build without the key never updates.
//!
//! Desktop updates Desktop only. The Host it bundles is handed to the Host's
//! own installer on the next launch (`zwiim-host --seed`), which stages it
//! without a second download; the Host decides when to switch to it.
//!
//! Trust: `latest.json` is published next to `release.json` on the GitHub
//! Release. The archive must carry a signature from the release key, and the
//! signature must name the announced version (`requireSignedVersion` in
//! `tauri.conf.json`), so a tampered `latest.json` cannot pair a new version
//! number with an old release. The plugin never installs an older version.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::Manager;
use tauri_plugin_updater::UpdaterExt;

/// The release public key (Tauri form), compiled in by the release build.
pub const RELEASE_PUBKEY: Option<&str> = option_env!("ZWIIM_RELEASE_PUBKEY");
const DEFAULT_RELEASES_URL: &str = "https://github.com/Ashish1101/zwiim-releases/releases";
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

#[derive(Default)]
pub struct DesktopUpdate(Mutex<Activity>);

impl DesktopUpdate {
    fn get(&self) -> Activity {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    fn set(&self, a: Activity) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = a;
    }
}

#[derive(Serialize)]
pub struct Status {
    version: &'static str,
    supported: bool,
    /// `unsigned_build`: made outside the release pipeline, never updates.
    unsupported_reason: Option<&'static str>,
    activity: Activity,
}

fn status_of(state: &DesktopUpdate) -> Status {
    let supported = release_key().is_some();
    Status {
        version: env!("CARGO_PKG_VERSION"),
        supported,
        unsupported_reason: (!supported).then_some("unsigned_build"),
        activity: state.get(),
    }
}

fn updater<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<tauri_plugin_updater::Updater, String> {
    let endpoint = format!("{}/latest/download/latest.json", releases_url());
    let url = url::Url::parse(&endpoint).map_err(|e| e.to_string())?;
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
    tokio::time::timeout(CHECK_TIMEOUT, updater(app)?.check())
        .await
        .map_err(|_| "The update check timed out".to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn desktop_update_status(state: tauri::State<'_, DesktopUpdate>) -> Status {
    status_of(&state)
}

/// Look for a newer Desktop. Never downloads.
#[tauri::command]
pub async fn desktop_update_check(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopUpdate>,
) -> Result<Status, String> {
    if release_key().is_none() {
        return Ok(status_of(&state));
    }
    if matches!(
        state.get(),
        Activity::Downloading { .. } | Activity::Ready { .. }
    ) {
        return Ok(status_of(&state));
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
    Ok(status_of(&state))
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
            Ok::<_, String>(update.version)
        }
        .await;
        state.set(match result {
            Ok(version) => Activity::Ready { version },
            Err(message) => Activity::Failed { message },
        });
    });
    Ok(status_of(&state))
}

/// Relaunch into the installed update. The Host keeps running.
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
        let status = status_of(&DesktopUpdate::default());
        assert!(!status.supported);
        assert_eq!(status.unsupported_reason, Some("unsigned_build"));
    }

    #[test]
    fn the_endpoint_is_the_latest_release() {
        std::env::set_var("ZWIIM_RELEASE_URL", "http://127.0.0.1:9/releases/");
        assert_eq!(releases_url(), "http://127.0.0.1:9/releases");
        std::env::remove_var("ZWIIM_RELEASE_URL");
    }
}
