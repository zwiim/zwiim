//! Showing a folder of this computer in its file manager.

use std::path::Path;

/// Why `path` cannot be shown: only an existing folder, named in full, is.
fn refusal(path: &Path) -> Option<&'static str> {
    if !path.is_absolute() {
        Some("Only a full folder path can be shown")
    } else if !path.is_dir() {
        Some("That folder does not exist on this computer")
    } else {
        None
    }
}

/// Show folder `path` in the file manager. A folder only, never opened with
/// an app: on macOS it is selected in its parent rather than opened, since
/// opening a folder that is an app bundle would launch it.
#[tauri::command]
pub fn reveal_folder(path: String) -> Result<(), String> {
    let path = Path::new(&path);
    if let Some(reason) = refusal(path) {
        return Err(reason.into());
    }
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg("-R");
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = std::process::Command::new("explorer");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Cannot show the folder: {e}"))
}

#[cfg(test)]
mod tests {
    use super::refusal;

    #[test]
    fn shows_only_an_existing_folder_named_in_full() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(refusal(dir.path()), None);
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(refusal(&file).is_some());
        assert!(refusal(&dir.path().join("gone")).is_some());
        assert!(refusal(std::path::Path::new("relative/dir")).is_some());
    }
}
