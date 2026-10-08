//! Open a workspace file in the default app for its type. The Host may be on
//! another computer, so the Desktop gets the bytes (`files.download`), keeps
//! them in a temporary folder, and asks the OS to open that copy.

use std::path::{Path, PathBuf};

use base64::Engine;
use rand::{distributions::Alphanumeric, Rng};

/// Documents, media and data the OS hands to a viewer. Anything else, a
/// script or an app above all, is refused: the file came from the workspace,
/// which agents write to.
const OPENABLE: &[&str] = &[
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf", "pages", "numbers",
    "key", "csv", "tsv", "txt", "md", "log", "html", "htm", "json", "xml", "png", "jpg", "jpeg", "gif",
    "webp", "heic", "bmp", "tif", "tiff", "svg", "mp3", "wav", "m4a", "mp4", "mov", "webm", "zip",
];

/// The last component of `name`, if it is a plain file name.
fn clean_name(name: &str) -> Option<&str> {
    let last = name.rsplit(['/', '\\']).next()?.trim();
    let usable = !last.is_empty() && last != "." && last != ".." && !last.chars().any(char::is_control);
    usable.then_some(last)
}

fn is_openable(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| OPENABLE.contains(&e.to_ascii_lowercase().as_str()))
}

/// Write `data` as `<dir>/<random>/<name>`; the path written.
fn write_copy(dir: &Path, name: &str, data: &[u8]) -> Result<PathBuf, String> {
    let folder: String = rand::thread_rng().sample_iter(&Alphanumeric).take(12).map(char::from).collect();
    let folder = dir.join(folder);
    std::fs::create_dir_all(&folder).map_err(|e| format!("Cannot save a copy to open: {e}"))?;
    let path = folder.join(name);
    std::fs::write(&path, data).map_err(|e| format!("Cannot save a copy to open: {e}"))?;
    Ok(path)
}

fn open_path(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Cannot open the file: {e}"))
}

/// Open `name` (its bytes as standard base64) in the default app for its type.
#[tauri::command]
pub fn open_download(name: String, data: String) -> Result<(), String> {
    let name = clean_name(&name).ok_or("That file name cannot be opened")?;
    if !is_openable(name) {
        return Err(format!("Zwiim does not open {name} outside the app, because it could run a program."));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|_| "The file's contents were damaged on the way")?;
    let path = write_copy(&std::env::temp_dir().join("zwiim-downloads"), name, &bytes)?;
    open_path(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_file_name() {
        assert_eq!(clean_name("report.pdf"), Some("report.pdf"));
        assert_eq!(clean_name(".zwiim/attachments/ab12/report.pdf"), Some("report.pdf"));
        assert_eq!(clean_name("..\\..\\x.docx"), Some("x.docx"));
        assert_eq!(clean_name(".."), None);
        assert_eq!(clean_name("dir/"), None);
        assert_eq!(clean_name("a\nb.pdf"), None);
    }

    #[test]
    fn opens_documents_not_programs() {
        for ok in ["a.pdf", "B.XLSX", "c.docx", "d.csv", "e.png", "f.html", "g.zip"] {
            assert!(is_openable(ok), "{ok}");
        }
        for refused in ["run.sh", "x.command", "App.app", "setup.exe", "x.bat", "y.ps1", "z.jar", "Makefile", "lib.dylib"] {
            assert!(!is_openable(refused), "{refused}");
        }
    }

    #[test]
    fn writes_a_copy_under_its_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_copy(dir.path(), "r.pdf", b"%PDF").unwrap();
        let b = write_copy(dir.path(), "r.pdf", b"%PDF-2").unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read(&a).unwrap(), b"%PDF");
        assert!(a.starts_with(dir.path()) && a.ends_with("r.pdf"));
    }

    #[test]
    fn refuses_a_script_before_writing_it() {
        let err = open_download("run.command".into(), "ZWNobw==".into()).unwrap_err();
        assert!(err.contains("could run a program"), "{err}");
    }
}
