//! Zwiim Desktop native shell.
//!
//! Deliberately thin: it finds the local host through its discovery file,
//! starts the host when none is running, and hosts the web UI (plus the
//! native folder picker via the dialog plugin, and the optional account
//! sign-in whose session stays in the OS keychain — see `account`). It never
//! speaks the Zwiim protocol, never reads host SQLite, and never touches
//! project files — the UI talks to the host over localhost HTTP/WS through
//! the TypeScript SDK.
//!
//! For hosts on other computers it keeps this device's identity and pairings
//! in the keychain (`device`) and makes the account's Zwiim Cloud requests
//! (`cloud`); the UI reaches those hosts through the Cloud relay, over the
//! SDK's end-to-end encrypted tunnel. On the same local network it can
//! reach them directly instead (`lan`): mDNS discovery and a TCP byte pipe
//! to a discovered host, carrying the same tunnel.

pub mod account;
pub mod cloud;
pub mod device;
pub mod download;
pub mod fonts;
pub mod lan;
pub mod lid_helper;
pub mod login;
pub mod notify;
pub mod preview;
pub mod reveal;
pub mod update;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Manager as _;

/// Mirrors the host's `<data_dir>/local-host.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalHostInfo {
    pub host_id: String,
    pub url: String,
    pub token: String,
    pub pid: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiscoveredHost {
    #[serde(flatten)]
    pub info: LocalHostInfo,
    /// The process that wrote the file is still running. `false` means the
    /// file is left over from a host that exited.
    pub running: bool,
}

/// The host's data directory: `ZWIIM_DATA_DIR`, else the platform data dir
/// + `zwiim` (the host's own default).
fn host_data_dir() -> PathBuf {
    std::env::var_os("ZWIIM_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_dir()
                .map(|d| d.join("zwiim"))
                .unwrap_or_else(|| PathBuf::from(".zwiim-data"))
        })
}

pub fn read_discovery(data_dir: &Path) -> Result<Option<LocalHostInfo>, String> {
    let path = data_dir.join("local-host.json");
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("Invalid host discovery file: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Cannot read host discovery file: {e}")),
    }
}

#[cfg(unix)]
fn pid_running(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // Signal 0 checks existence only. EPERM means it exists under another user.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn pid_running(_pid: u32) -> bool {
    // Liveness is confirmed by `host.hello` instead.
    true
}

/// The host binary: `ZWIIM_HOST_BIN`, else the `zwiim-host` bundled next to
/// this executable. Development builds also fall back to `PATH`; a release
/// build never runs a Host it did not ship with.
fn host_binary() -> PathBuf {
    if let Some(bin) = std::env::var_os("ZWIIM_HOST_BIN") {
        return PathBuf::from(bin);
    }
    let name = if cfg!(windows) { "zwiim-host.exe" } else { "zwiim-host" };
    let bundled = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)));
    match bundled {
        Some(p) if p.is_file() || !cfg!(debug_assertions) => p,
        _ => PathBuf::from(name),
    }
}

/// Held while the bundled Host is offered to the Host's installer, so a Host
/// started meanwhile sees the result.
static SEEDING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Offer the Host this app bundles to the Host's installer: installed on
/// first run, staged when newer than the installed Host (the running Host
/// then reports that a restart finishes the update), ignored otherwise.
/// A Desktop update therefore never downloads the Host a second time.
pub(crate) fn seed_host() {
    let _held = SEEDING.lock().unwrap_or_else(|p| p.into_inner());
    let bin = host_binary();
    match Command::new(&bin)
        .arg("--seed")
        .stdin(Stdio::null())
        .output()
    {
        Ok(out) if out.status.success() => {}
        Ok(out) => eprintln!(
            "zwiim: offering the bundled host failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => eprintln!("zwiim: cannot run {} --seed: {e}", bin.display()),
    }
}

#[tauri::command]
fn discover_host() -> Result<Option<DiscoveredHost>, String> {
    Ok(read_discovery(&host_data_dir())?.map(|info| DiscoveredHost {
        running: pid_running(info.pid),
        info,
    }))
}

/// Start the host as an independent process: its own process group, no
/// inherited stdio, never killed by the desktop. It outlives the window.
/// It runs under its supervisor, which restarts it, applies Host updates and
/// rolls back one that fails its trial.
#[tauri::command]
fn start_host() -> Result<(), String> {
    drop(SEEDING.lock().unwrap_or_else(|p| p.into_inner()));
    let bin = host_binary();
    let mut cmd = Command::new(&bin);
    cmd.arg("--supervise")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Cannot start host ({}): {e}", bin.display()))?;
    // Reap it if it exits while the desktop is still open.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Sign-in in progress can be cancelled from the UI.
#[derive(Default)]
struct SignIn {
    cancel: tokio::sync::Notify,
}

#[derive(Serialize)]
struct AccountStatus {
    /// Sign-in is configured (Supabase URL and anon key, at run or build time).
    available: bool,
    account: Option<account::Account>,
    /// The Zwiim Cloud remote hosts are reached through; none without
    /// sign-in or a configured Cloud.
    cloud_url: Option<String>,
}

#[tauri::command]
fn account_status() -> Result<AccountStatus, String> {
    let available = account::AuthConfig::configured().is_some();
    Ok(AccountStatus {
        available,
        account: account::current(&account::Keychain)?,
        cloud_url: cloud::configured_url().filter(|_| available),
    })
}

#[tauri::command]
async fn account_sign_in(state: tauri::State<'_, SignIn>) -> Result<account::Account, String> {
    let config = account::AuthConfig::configured().ok_or("Sign-in is not available in this build")?;
    account::sign_in(&config, &account::Keychain, account::open_in_browser, &state.cancel).await
}

#[tauri::command]
fn account_cancel_sign_in(state: tauri::State<'_, SignIn>) {
    state.cancel.notify_waiters();
}

#[tauri::command]
async fn account_sign_out() -> Result<(), String> {
    account::sign_out(account::AuthConfig::configured().as_ref(), &account::Keychain).await
}

/// Open a link from the conversation in the default browser. Agent text is
/// untrusted, so only web and mail links open; anything else (`file:`, app
/// schemes) could start a program.
#[tauri::command]
fn open_link(url: String) -> Result<(), String> {
    if !is_openable_link(&url) {
        return Err("Only web and mail links can be opened".into());
    }
    account::open_in_browser(&url)
}

/// Whether an IPC call's `Origin` is the app's own page. Everything else is
/// refused: preview frames (on Windows they get Tauri's IPC script too, and
/// Tauri counts registered schemes as local), and calls with no `Origin`
/// (the postMessage fallback, which the app's CSP never needs).
fn is_app_origin(origin: Option<&str>) -> bool {
    matches!(
        origin,
        Some("tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost")
    ) || (cfg!(debug_assertions) && matches!(origin, Some("http://localhost:1420" | "http://localhost:1421")))
}

fn is_openable_link(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    ["https://", "http://", "mailto:"].iter().any(|s| lower.starts_with(s))
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// An allowed Zwiim Cloud call, with the account's token added here.
#[tauri::command]
async fn cloud_request(method: String, path: String) -> Result<cloud::CloudReply, String> {
    let config = account::AuthConfig::configured().ok_or("Sign-in is not available in this build")?;
    let url = cloud::configured_url().ok_or("No Zwiim Cloud is configured")?;
    let token = |force| {
        let config = config.clone();
        async move { account::access_token(&config, &account::Keychain, force).await }
    };
    cloud::request(&url, token, &method, &path).await
}

/// What the UI needs to open a tunnel as this device. The secret stays in
/// memory there.
#[derive(Serialize)]
struct DeviceKey {
    device_id: String,
    device_name: String,
    secret: String,
}

#[tauri::command]
fn device_identity() -> Result<DeviceKey, String> {
    let identity = device::identity(&device::Keychain)?;
    Ok(DeviceKey { device_id: identity.device_id, device_name: identity.device_name, secret: identity.secret })
}

#[tauri::command]
fn device_paired_hosts() -> Result<Vec<device::PairedHost>, String> {
    device::paired_hosts(&device::Keychain)
}

#[tauri::command]
fn device_pin_host(host: device::PairedHost) -> Result<(), String> {
    device::pin_host(&device::Keychain, host)
}

#[tauri::command]
fn device_forget_host(host_id: String) -> Result<(), String> {
    device::forget_host(&device::Keychain, &host_id)
}

/// Hosts advertising on this network. Empty when `host_id` is not seen
/// within `timeout_ms`.
#[tauri::command]
async fn lan_discover(
    discovery: tauri::State<'_, lan::Discovery>,
    host_id: String,
    timeout_ms: u64,
) -> Result<Vec<lan::Advertisement>, String> {
    if !cloud::valid_host_id(&host_id) {
        return Err("Not a host id".into());
    }
    discovery.discover(&host_id, Duration::from_millis(timeout_ms.min(10_000))).await
}

/// A byte pipe to a discovered host's endpoint; bytes and its end arrive on
/// `on_event`.
#[tauri::command]
async fn lan_connect(
    discovery: tauri::State<'_, lan::Discovery>,
    links: tauri::State<'_, lan::Links>,
    address: String,
    port: u16,
    timeout_ms: u64,
    on_event: tauri::ipc::Channel<lan::LinkEvent>,
) -> Result<u32, String> {
    let timeout = Duration::from_millis(timeout_ms.min(10_000));
    links
        .connect(&discovery, &address, port, timeout, move |ev| {
            let _ = on_event.send(ev);
        })
        .await
}

#[tauri::command]
async fn lan_send(links: tauri::State<'_, lan::Links>, id: u32, data: String) -> Result<(), String> {
    links.send(id, &data).await
}

#[tauri::command]
async fn lan_close(links: tauri::State<'_, lan::Links>, id: u32) -> Result<(), String> {
    links.close(id).await;
    Ok(())
}

/// Zooms the whole page, as a browser does; the app remembers the level and
/// sets it again at launch.
#[tauri::command]
fn window_zoom(window: tauri::WebviewWindow, factor: f64) -> Result<(), String> {
    if !factor.is_finite() {
        return Err("Not a zoom level".into());
    }
    window.set_zoom(factor.clamp(0.5, 2.0)).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let commands: fn(tauri::ipc::Invoke) -> bool = tauri::generate_handler![
        discover_host,
        start_host,
        account_status,
        account_sign_in,
        account_cancel_sign_in,
        account_sign_out,
        open_link,
        download::download_begin,
        download::download_append,
        download::download_open,
        preview::preview_open,
        preview::preview_close,
        preview::preview_reply,
        cloud_request,
        device_identity,
        device_paired_hosts,
        device_pin_host,
        device_forget_host,
        lan_discover,
        lan_connect,
        lan_send,
        lan_close,
        update::desktop_update_status,
        update::desktop_update_check,
        update::desktop_update_install,
        update::desktop_update_restart,
        update::desktop_update_set_channel,
        login::host_login_status,
        login::host_login_set,
        lid_helper::lid_helper_setup,
        notify::desktop_notify,
        fonts::installed_fonts,
        reveal::reveal_folder,
        window_zoom
    ];
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(update::plugin())
        .register_asynchronous_uri_scheme_protocol("zwiim-preview", |ctx, request, responder| {
            let previews = ctx.app_handle().state::<preview::Previews>();
            previews.ask_with(request.method().as_str(), request.uri(), move |_, res| responder.respond(res));
        })
        .setup(|app| {
            // Built here rather than from the config so new windows (a
            // preview's `target=_blank` links) go to the system browser.
            let config = app
                .config()
                .app
                .windows
                .iter()
                .find(|w| w.label == "main")
                .cloned()
                .ok_or("no main window config")?;
            tauri::WebviewWindowBuilder::from_config(app.handle(), &config)?
                .on_new_window(|url, _| {
                    if is_openable_link(url.as_str()) {
                        let _ = account::open_in_browser(url.as_str());
                    }
                    tauri::webview::NewWindowResponse::Deny
                })
                .initialization_script_for_all_frames(preview::LINKS_JS)
                .build()?;
            std::thread::spawn(seed_host);
            login::sync_at_launch(app.handle());
            Ok(())
        })
        .manage(SignIn::default())
        .manage(update::DesktopUpdate::default())
        .manage(lan::Discovery::default())
        .manage(lan::Links::default())
        .manage(download::Downloads::default())
        .manage(preview::Previews::default())
        .invoke_handler(move |invoke| {
            let origin = invoke.message.headers().get("origin");
            if !is_app_origin(origin.and_then(|o| o.to_str().ok())) {
                invoke.resolver.reject("Not allowed");
                return true;
            }
            commands(invoke)
        })
        .run(tauri::generate_context!())
        .expect("error while running Zwiim Desktop");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_only_web_and_mail_links() {
        assert!(is_openable_link("https://example.com/a?b=c"));
        assert!(is_openable_link("HTTP://example.com"));
        assert!(is_openable_link("mailto:me@example.com"));
        assert!(!is_openable_link("file:///etc/passwd"));
        assert!(!is_openable_link("vscode://open"));
        assert!(!is_openable_link("-a Calculator"));
        assert!(!is_openable_link("https://x.com -a Terminal"));
        assert!(!is_openable_link("https://x.com\n"));
    }

    #[test]
    fn takes_commands_only_from_the_app_origin() {
        assert!(is_app_origin(Some("tauri://localhost")));
        assert!(is_app_origin(Some("http://tauri.localhost")));
        assert!(is_app_origin(Some("https://tauri.localhost")));
        assert_eq!(is_app_origin(Some("http://localhost:1420")), cfg!(debug_assertions));
        assert!(!is_app_origin(None));
        assert!(!is_app_origin(Some("null")));
        assert!(!is_app_origin(Some("zwiim-preview://0123456789abcdef0123456789abcdef")));
        assert!(!is_app_origin(Some("http://zwiim-preview.0123456789abcdef0123456789abcdef")));
        assert!(!is_app_origin(Some("tauri://localhost.evil")));
        assert!(!is_app_origin(Some("https://example.com")));
    }

    #[test]
    fn missing_discovery_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_discovery(dir.path()).unwrap(), None);
    }

    #[test]
    fn reads_the_hosts_discovery_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("local-host.json"),
            r#"{"host_id":"h","url":"http://127.0.0.1:3080","token":"t","pid":42}"#,
        )
        .unwrap();
        let info = read_discovery(dir.path()).unwrap().unwrap();
        assert_eq!(info.url, "http://127.0.0.1:3080");
        assert_eq!(info.pid, 42);
    }

    #[test]
    fn corrupt_discovery_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("local-host.json"), "{").unwrap();
        assert!(read_discovery(dir.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn pid_liveness() {
        assert!(pid_running(std::process::id()));
        assert!(!pid_running(0));
        assert!(!pid_running(u32::MAX));
    }
}
