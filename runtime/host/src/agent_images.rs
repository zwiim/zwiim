//! Agent images become files in the session's workspace. The client gets a
//! workspace-relative path, never the bytes: an image read from a file inside
//! the workspace is shown from that file; any other (a capture in `/tmp`, a
//! generated image, a browser screenshot) is copied to `.zwiim/images/`,
//! named by content hash so replaying history writes nothing new.

use std::path::{Component, Path};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::commands::folder_links;
use crate::models::message::ImageData;
use crate::workspace::zwiim_dir::IMAGES;

const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "avif", "tif", "tiff", "heic", "heif",
];

/// Larger images are omitted rather than decoded and written.
pub const AGENT_IMAGE_MAX_BYTES: usize = 25 * 1024 * 1024;

pub(crate) fn ext(mime_type: &str) -> &'static str {
    match mime_type.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "image/bmp" | "image/x-ms-bmp" => "bmp",
        "image/avif" => "avif",
        "image/tiff" => "tiff",
        "image/heic" => "heic",
        "image/heif" => "heif",
        _ => "bin",
    }
}

/// Projection runs on async ws/http tasks: decoding, hashing and writing an
/// image (and the one-time `git` call) move the task off its worker thread.
fn off_async<T>(f: impl FnOnce() -> T) -> T {
    use tokio::runtime::{Handle, RuntimeFlavor};
    match Handle::try_current() {
        Ok(h) if h.runtime_flavor() == RuntimeFlavor::MultiThread => tokio::task::block_in_place(f),
        _ => f(),
    }
}

/// Save one image under `<root>/.zwiim/images/` and return
/// `{name, mime_type, path}`, `path` relative to `root`. `None` (logged) when
/// the data isn't base64, is over `AGENT_IMAGE_MAX_BYTES`, or the write fails.
pub fn save(root: &Path, data_b64: &str, mime_type: &str) -> Option<Value> {
    let data_b64 = data_b64.trim();
    if data_b64.len() / 4 * 3 > AGENT_IMAGE_MAX_BYTES {
        warn!("[agent-images] image omitted: over {AGENT_IMAGE_MAX_BYTES} bytes");
        return None;
    }
    off_async(|| save_bytes(root, data_b64, mime_type))
}

fn save_bytes(root: &Path, data_b64: &str, mime_type: &str) -> Option<Value> {
    let bytes = match STANDARD.decode(data_b64) {
        Ok(b) => b,
        Err(e) => {
            warn!("[agent-images] image omitted: not base64 ({e})");
            return None;
        }
    };
    let hash = Sha256::digest(&bytes);
    let hex: String = hash[..8].iter().map(|b| format!("{b:02x}")).collect();
    let name = format!("{hex}.{}", ext(mime_type));
    let path = format!("{IMAGES}/{name}");
    let dest = root.join(&path);
    if !dest.is_file() {
        if let Err(e) = write(root, &dest, &name, &bytes) {
            warn!("[agent-images] image omitted: could not write {}: {e}", dest.display());
            return None;
        }
    }
    Some(json!({ "name": name, "mime_type": mime_type, "path": path }))
}

fn write(root: &Path, dest: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    // A worktree made before `.zwiim/` existed: exclude it before creating it.
    if !root.join(".zwiim").exists() {
        exclude_zwiim(root);
    }
    let dir = dest.parent().expect("images dir");
    std::fs::create_dir_all(dir)?;
    // Each subscriber projects on its own, so two may save the same image at
    // once: a temp name per writer; whoever renames last wins with equal bytes.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let part = dir.join(format!(".{name}.{}-{seq}.part", std::process::id()));
    let written = std::fs::write(&part, bytes).and_then(|()| std::fs::rename(&part, dest));
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
        if dest.is_file() {
            return Ok(()); // another writer put it in place
        }
    }
    written
}

fn exclude_zwiim(root: &Path) {
    let Ok(out) = crate::process::std_command("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir", "--show-toplevel"])
        .current_dir(root)
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return; // not a repository
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    if let (Some(git_dir), Some(top)) = (lines.next(), lines.next()) {
        folder_links::append_exclude(Path::new(git_dir), Path::new(top), root, ".zwiim/");
    }
}

/// `path` (absolute, or relative to `root`) as a workspace-relative path when
/// it names an image file really inside `root`; symlinks are resolved first.
pub fn source_in(root: &Path, path: &str) -> Option<String> {
    let p = Path::new(path);
    let rel = if p.is_absolute() { p.strip_prefix(root).ok()? } else { p };
    if rel.as_os_str().is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let ext = rel.extension()?.to_str()?.to_ascii_lowercase();
    if !IMAGE_EXTS.contains(&ext.as_str()) {
        return None;
    }
    let real = root.join(rel).canonicalize().ok()?;
    if !real.starts_with(root.canonicalize().ok()?) || !real.is_file() {
        return None;
    }
    Some(rel.to_str()?.replace('\\', "/"))
}

/// One image as a block: from its source file when that is in the
/// workspace, else a copy.
pub fn one(root: Option<&str>, data_b64: &str, mime_type: &str, source: Option<&str>) -> Option<Value> {
    let Some(root) = root else {
        warn!("[agent-images] image omitted: session has no workspace");
        return None;
    };
    let root = Path::new(root);
    if let Some(path) = source.and_then(|s| source_in(root, s)) {
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        return Some(json!({ "name": name, "mime_type": mime_type, "path": path }));
    }
    save(root, data_b64, mime_type)
}

/// A tool's images. The source file stands for its image only when there is
/// exactly one (a multi-page PDF read returns one image per page).
pub fn project(root: Option<&str>, images: &[ImageData], source: Option<&str>) -> Vec<Value> {
    let source = if images.len() == 1 { source } else { None };
    images.iter().filter_map(|i| one(root, &i.data, &i.mime_type, source)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// 1×1 PNG header bytes are enough; nothing decodes the image.
    const PNG: &str = "iVBORw0KGgo=";

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn same_bytes_same_path_no_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let a = save(dir.path(), PNG, "image/png").expect("saved");
        let path = a["path"].as_str().unwrap().to_string();
        assert!(path.starts_with(".zwiim/images/") && path.ends_with(".png"), "{path}");
        assert_eq!(a["name"].as_str().unwrap().len(), 16 + 4);
        assert_eq!(a["mime_type"], "image/png");
        let file = dir.path().join(&path);
        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let b = save(dir.path(), PNG, "image/png").expect("saved again");
        assert_eq!(a, b);
        assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), before, "not rewritten");
        // No .part left behind.
        let names: Vec<_> = std::fs::read_dir(file.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }

    #[test]
    fn concurrent_saves_of_one_image_all_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let first = save(dir.path(), PNG, "image/png").unwrap();
        let file = dir.path().join(first["path"].as_str().unwrap());
        std::fs::remove_file(&file).unwrap();
        // Another subscriber's in-flight temp file sits where a shared name would go.
        std::fs::create_dir(file.with_file_name(format!(".{}.part", first["name"].as_str().unwrap()))).unwrap();
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let root = dir.path().to_path_buf();
                std::thread::spawn(move || save(&root, PNG, "image/png"))
            })
            .collect();
        for t in threads {
            assert_eq!(t.join().unwrap().as_ref(), Some(&first));
        }
        assert_eq!(std::fs::read(&file).unwrap(), base64::engine::general_purpose::STANDARD.decode(PNG).unwrap());
    }

    #[test]
    fn an_image_over_the_cap_is_omitted_unwritten() {
        let dir = tempfile::tempdir().unwrap();
        let over = "A".repeat((AGENT_IMAGE_MAX_BYTES / 3 + 1) * 4);
        assert!(save(dir.path(), &over, "image/png").is_none());
        assert!(!dir.path().join(IMAGES).exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn saving_on_a_runtime_worker_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let v = tokio::spawn(async move { save(&root, PNG, "image/png") }).await.unwrap();
        assert!(v.is_some());
    }

    #[test]
    fn invalid_base64_or_no_root_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(save(dir.path(), "not base64!!", "image/png").is_none());
        assert!(one(None, PNG, "image/png", None).is_none());
    }

    #[test]
    fn extension_from_mime() {
        for (mime, ext) in [
            ("image/png", "png"),
            ("image/jpeg", "jpg"),
            ("image/jpg", "jpg"),
            ("image/gif", "gif"),
            ("image/webp", "webp"),
            ("image/svg+xml", "svg"),
            ("image/bmp", "bmp"),
            ("image/avif", "avif"),
            ("image/tiff", "tiff"),
            ("image/heic", "heic"),
            ("application/octet-stream", "bin"),
        ] {
            assert_eq!(super::ext(mime), ext, "{mime}");
        }
    }

    #[test]
    fn a_file_inside_the_worktree_is_used_in_place() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("shots")).unwrap();
        std::fs::write(dir.path().join("shots/a.png"), b"x").unwrap();
        let abs = dir.path().join("shots/a.png");
        let root = dir.path().to_str().unwrap();
        let v = one(Some(root), PNG, "image/png", abs.to_str()).unwrap();
        assert_eq!(v, serde_json::json!({"name": "a.png", "mime_type": "image/png", "path": "shots/a.png"}));
        assert!(!dir.path().join(".zwiim/images").exists(), "no copy");
        // The same read from outside the worktree is copied.
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("b.png"), b"x").unwrap();
        let v = one(Some(root), PNG, "image/png", outside.path().join("b.png").to_str()).unwrap();
        assert!(v["path"].as_str().unwrap().starts_with(".zwiim/images/"));
    }

    #[test]
    fn source_in_rejects_escapes_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("o.png"), b"x").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path().join("o.png"), dir.path().join("link.png")).unwrap();
        let root = dir.path();
        assert_eq!(source_in(root, "../o.png"), None);
        assert_eq!(source_in(root, outside.path().join("o.png").to_str().unwrap()), None);
        assert_eq!(source_in(root, "link.png"), None, "symlink out of the worktree");
        assert_eq!(source_in(root, "notes.txt"), None, "not an image");
        assert_eq!(source_in(root, "absent.png"), None, "not a file");
    }

    #[test]
    fn first_save_in_old_worktree_keeps_git_status_clean() {
        // A worktree made before `.zwiim/` existed: no folder, no exclude rule.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
        let wt = dir.path().join("wt");
        git(&repo, &["worktree", "add", "-q", "-b", "w", wt.to_str().unwrap()]);
        save(&wt, PNG, "image/png").expect("saved");
        assert_eq!(git(&wt, &["status", "--porcelain", "--untracked-files=all"]), "");
    }
}
