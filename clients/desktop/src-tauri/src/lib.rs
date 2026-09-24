//! Zwiim Desktop native shell.
//!
//! Deliberately thin: it finds the local host through its discovery file,
//! starts the host when none is running, and hosts the web UI (plus the
//! native folder picker via the dialog plugin, and the optional account
//! sign-in whose session stays in the OS keychain — see `account`). It never
//! speaks the Zwiim protocol, never reads host SQLite, and never touches
//! project files — the UI talks to the host over localhost HTTP/WS through
//! the TypeScript SDK.

pub mod account;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

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

/// The host binary: `ZWIIM_HOST_BIN`, else `zwiim-host` next to this
/// executable, else `zwiim-host` on `PATH`.
fn host_binary() -> PathBuf {
    if let Some(bin) = std::env::var_os("ZWIIM_HOST_BIN") {
        return PathBuf::from(bin);
    }
    let name = if cfg!(windows) { "zwiim-host.exe" } else { "zwiim-host" };
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
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
#[tauri::command]
fn start_host() -> Result<(), String> {
    let bin = host_binary();
    let mut cmd = Command::new(&bin);
    cmd.stdin(Stdio::null())
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
    /// This build can sign in (it was built with Supabase configuration).
    available: bool,
    account: Option<account::Account>,
}

#[tauri::command]
fn account_status() -> Result<AccountStatus, String> {
    Ok(AccountStatus {
        available: account::AuthConfig::from_build().is_some(),
        account: account::current(&account::Keychain)?,
    })
}

#[tauri::command]
async fn account_sign_in(state: tauri::State<'_, SignIn>) -> Result<account::Account, String> {
    let config = account::AuthConfig::from_build().ok_or("Sign-in is not available in this build")?;
    account::sign_in(&config, &account::Keychain, account::open_in_browser, &state.cancel).await
}

#[tauri::command]
fn account_cancel_sign_in(state: tauri::State<'_, SignIn>) {
    state.cancel.notify_waiters();
}

#[tauri::command]
async fn account_sign_out() -> Result<(), String> {
    account::sign_out(account::AuthConfig::from_build().as_ref(), &account::Keychain).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(SignIn::default())
        .invoke_handler(tauri::generate_handler![
            discover_host,
            start_host,
            account_status,
            account_sign_in,
            account_cancel_sign_in,
            account_sign_out
        ])
        .run(tauri::generate_context!())
        .expect("error while running Zwiim Desktop");
}

#[cfg(test)]
mod tests {
    use super::*;

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
