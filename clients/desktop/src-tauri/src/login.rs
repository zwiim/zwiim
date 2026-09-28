//! Starting the Host when the user logs in (macOS): a per-user LaunchAgent
//! that runs the Host this app bundles, under its supervisor. On by default
//! for an installed app; the user turns it off in About & updates. Only the
//! next login is affected: turning it on or off never starts or stops the
//! running Host. Development builds (no bundled Host) leave it alone.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::Manager;

pub const LABEL: &str = "dev.zwiim.host";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Status {
    pub supported: bool,
    pub enabled: bool,
}

#[derive(Serialize, Deserialize)]
struct Choice {
    enabled: bool,
}

fn agent_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

fn choice_path(config_dir: &Path) -> PathBuf {
    config_dir.join("host-login.json")
}

/// Only a Host inside an installed app bundle, which keeps its path.
fn supported(host: &Path) -> bool {
    cfg!(target_os = "macos") && host.to_string_lossy().contains(".app/Contents/MacOS/")
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn plist(host: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>--supervise</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <false/>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
  <key>ProcessType</key>
  <string>Interactive</string>
</dict>
</plist>
"#,
        xml_escape(&host.to_string_lossy())
    )
}

fn enabled_choice(config_dir: &Path) -> bool {
    std::fs::read(choice_path(config_dir))
        .ok()
        .and_then(|b| serde_json::from_slice::<Choice>(&b).ok())
        .is_none_or(|c| c.enabled)
}

/// Make the LaunchAgent match the user's choice, pointing at `host` (the app
/// may have moved since it was written).
fn sync(home: &Path, config_dir: &Path, host: &Path) -> std::io::Result<Status> {
    if !supported(host) {
        return Ok(Status {
            supported: false,
            enabled: false,
        });
    }
    let enabled = enabled_choice(config_dir);
    let path = agent_path(home);
    if enabled {
        let want = plist(host);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(want.as_str()) {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let tmp = path.with_extension("plist.part");
            std::fs::write(&tmp, want)?;
            std::fs::rename(&tmp, &path)?;
        }
    } else if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(Status {
        supported: true,
        enabled,
    })
}

fn set(home: &Path, config_dir: &Path, host: &Path, enabled: bool) -> std::io::Result<Status> {
    std::fs::create_dir_all(config_dir)?;
    std::fs::write(
        choice_path(config_dir),
        serde_json::to_vec(&Choice { enabled }).unwrap(),
    )?;
    sync(home, config_dir, host)
}

fn dirs_for(app: &tauri::AppHandle) -> Result<(PathBuf, PathBuf), String> {
    let home = app.path().home_dir().map_err(|e| e.to_string())?;
    let config = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok((home, config))
}

/// At launch: keep the LaunchAgent in line with the choice and this app's path.
pub fn sync_at_launch(app: &tauri::AppHandle) {
    let result = dirs_for(app).and_then(|(home, config)| {
        sync(&home, &config, &super::host_binary()).map_err(|e| e.to_string())
    });
    if let Err(e) = result {
        eprintln!("zwiim: could not update the login item: {e}");
    }
}

#[tauri::command]
pub fn host_login_status(app: tauri::AppHandle) -> Result<Status, String> {
    let (home, config) = dirs_for(&app)?;
    sync(&home, &config, &super::host_binary()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn host_login_set(app: tauri::AppHandle, enabled: bool) -> Result<Status, String> {
    let (home, config) = dirs_for(&app)?;
    set(&home, &config, &super::host_binary(), enabled).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLED: &str = "/Applications/Zwiim & Co.app/Contents/MacOS/zwiim-host";

    #[test]
    fn the_agent_runs_the_bundled_host_under_its_supervisor() {
        let p = plist(Path::new(BUNDLED));
        assert!(p.contains(
            "<string>/Applications/Zwiim &amp; Co.app/Contents/MacOS/zwiim-host</string>"
        ));
        assert!(p.contains("<string>--supervise</string>"));
        assert!(p.contains("<key>RunAtLoad</key>\n  <true/>"));
    }

    #[test]
    fn on_by_default_off_when_chosen_and_never_for_a_development_build() {
        let home = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let agent = agent_path(home.path());
        if cfg!(target_os = "macos") {
            let s = sync(home.path(), config.path(), Path::new(BUNDLED)).unwrap();
            assert_eq!(
                s,
                Status {
                    supported: true,
                    enabled: true
                }
            );
            assert!(std::fs::read_to_string(&agent)
                .unwrap()
                .contains("--supervise"));
            let s = set(home.path(), config.path(), Path::new(BUNDLED), false).unwrap();
            assert_eq!(
                s,
                Status {
                    supported: true,
                    enabled: false
                }
            );
            assert!(!agent.exists());
            // The choice outlives the launch.
            let s = sync(home.path(), config.path(), Path::new(BUNDLED)).unwrap();
            assert!(!s.enabled && !agent.exists());
        }
        let dev = sync(
            home.path(),
            config.path(),
            Path::new("/repo/target/debug/zwiim-host"),
        )
        .unwrap();
        assert_eq!(
            dev,
            Status {
                supported: false,
                enabled: false
            }
        );
    }
}
