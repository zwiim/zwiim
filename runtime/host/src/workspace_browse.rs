//! Home-only folder browsing, for adding a workspace from a paired device.
//!
//! A client names a folder by its segments under the host user's home
//! directory (`["Projects", "zwiim"]`), never by a host path. Resolution is
//! jailed: every segment is a plain, visible folder name, and the result is
//! canonicalized and must stay inside the canonical home, so `..`, absolute
//! paths and symlinks leading outside are all refused. An escape reads as
//! `not_found`, revealing nothing about what lies outside.
//!
//! One write: `create_folder` makes a single new, visible folder inside a
//! folder the same resolution reaches, so a device can make a project folder
//! to add. Nothing else here writes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::app_error::AppCommandError;

/// A listing names at most this many folders; `truncated` says there were more.
pub const BROWSE_LIMIT: usize = 1000;

/// A new folder's name is at most this many bytes (the common filesystem limit).
pub const FOLDER_NAME_MAX: usize = 255;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BrowseEntry {
    /// One path segment: never `.`, `..` or dot-prefixed.
    pub name: String,
    /// `<folder>/.git` exists (a file for a linked worktree, else a dir).
    pub is_git_repo: bool,
    /// A live workspace already points here.
    pub is_workspace: bool,
    /// Its permission bits let the host list it.
    pub is_readable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BrowseListing {
    /// The segments listed, as requested.
    pub path: Vec<String>,
    /// `~` or `~/a/b`: presentation only.
    pub display_path: String,
    /// Child folders, sorted case-insensitively.
    pub entries: Vec<BrowseEntry>,
    pub truncated: bool,
}

fn check_segment(segment: &str) -> Result<(), AppCommandError> {
    // A leading dot covers `.`, `..` and hidden folders alike.
    if segment.is_empty() || segment.starts_with('.') || segment.contains(['/', '\\', '\0']) {
        return Err(AppCommandError::invalid_input(
            "Folders are named by plain, visible folder names",
        ));
    }
    Ok(())
}

/// The folder `segments` names under `home` (canonical), canonicalized.
/// Fails with `invalid_input` for a bad segment and `not_found` when the
/// folder is missing, not a folder, or resolves outside `home`.
pub fn resolve_home_path(home: &Path, segments: &[String]) -> Result<PathBuf, AppCommandError> {
    for segment in segments {
        check_segment(segment)?;
    }
    segments
        .iter()
        .fold(home.to_path_buf(), |path, segment| path.join(segment))
        .canonicalize()
        .ok()
        .filter(|path| path.is_dir() && path.starts_with(home))
        .ok_or_else(|| AppCommandError::not_found("Folder not found"))
}

#[cfg(unix)]
fn is_readable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // Permission bits only: nothing inside the folder is opened.
    // SAFETY: `c_path` is a valid NUL-terminated string for the call's duration.
    unsafe { libc::access(c_path.as_ptr(), libc::R_OK | libc::X_OK) == 0 }
}

#[cfg(not(unix))]
fn is_readable(_path: &Path) -> bool {
    true
}

/// The folders directly inside the folder `segments` names under `home`
/// (canonical). `workspaces` holds the canonical paths of live workspaces.
pub fn browse(
    home: &Path,
    segments: &[String],
    workspaces: &HashSet<PathBuf>,
) -> Result<BrowseListing, AppCommandError> {
    let target = resolve_home_path(home, segments)?;
    let dir = std::fs::read_dir(&target).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => {
            AppCommandError::permission_denied("Folder can't be read")
        }
        std::io::ErrorKind::NotFound => AppCommandError::not_found("Folder not found"),
        _ => AppCommandError::io(e),
    })?;

    let mut folders: Vec<(String, PathBuf)> = Vec::new();
    for entry in dir.flatten() {
        // A name that isn't UTF-8 couldn't be named back as a segment.
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = if kind.is_dir() {
            entry.path()
        } else if kind.is_symlink() {
            // Followed only while it stays a folder inside home.
            match entry.path().canonicalize() {
                Ok(p) if p.is_dir() && p.starts_with(home) => p,
                _ => continue,
            }
        } else {
            continue;
        };
        folders.push((name, path));
    }
    folders.sort_by(|a, b| {
        a.0.to_lowercase()
            .cmp(&b.0.to_lowercase())
            .then_with(|| a.0.cmp(&b.0))
    });
    let truncated = folders.len() > BROWSE_LIMIT;
    folders.truncate(BROWSE_LIMIT);

    let entries = folders
        .into_iter()
        .map(|(name, path)| BrowseEntry {
            is_git_repo: path.join(".git").symlink_metadata().is_ok(),
            is_workspace: workspaces.contains(&path),
            is_readable: is_readable(&path),
            name,
        })
        .collect();
    Ok(BrowseListing {
        display_path: std::iter::once("~")
            .chain(segments.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("/"),
        path: segments.to_vec(),
        entries,
        truncated,
    })
}

/// Make the folder `name` inside the folder `segments` names under `home`
/// (canonical). Only the one folder is made: the parent must exist, and a
/// name already taken (by a folder or a file) is `already_exists`.
pub fn create_folder(
    home: &Path,
    segments: &[String],
    name: &str,
) -> Result<BrowseEntry, AppCommandError> {
    check_segment(name)?;
    if name.len() > FOLDER_NAME_MAX {
        return Err(AppCommandError::invalid_input("Folder name is too long"));
    }
    let parent = resolve_home_path(home, segments)?;
    let path = parent.join(name);
    std::fs::create_dir(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => {
            AppCommandError::already_exists("A folder or file with that name is already here")
        }
        std::io::ErrorKind::PermissionDenied => {
            AppCommandError::permission_denied("Folder can't be created here")
        }
        std::io::ErrorKind::NotFound => AppCommandError::not_found("Folder not found"),
        _ => AppCommandError::io(e),
    })?;
    Ok(BrowseEntry {
        name: name.to_string(),
        is_git_repo: false,
        is_workspace: false,
        is_readable: is_readable(&path),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_error::AppErrorCode;

    fn segs(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    /// A temp home (canonical) and the directory holding it.
    fn home() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let home = home.canonicalize().unwrap();
        (dir, home)
    }

    fn names(listing: &BrowseListing) -> Vec<&str> {
        listing.entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn lists_visible_folders_sorted_with_their_tags() {
        let (_dir, home) = home();
        for d in ["zeta", "Alpha", "beta", ".hidden", "repo/.git", "added"] {
            std::fs::create_dir_all(home.join(d)).unwrap();
        }
        std::fs::write(home.join("notes.txt"), "x").unwrap();
        std::fs::write(home.join(".profile"), "x").unwrap();
        let workspaces = HashSet::from([home.join("added")]);

        let listing = browse(&home, &[], &workspaces).unwrap();
        assert_eq!(names(&listing), ["added", "Alpha", "beta", "repo", "zeta"]);
        assert_eq!(listing.display_path, "~");
        assert!(!listing.truncated);
        let entry = |n: &str| listing.entries.iter().find(|e| e.name == n).unwrap();
        assert!(entry("repo").is_git_repo);
        assert!(!entry("beta").is_git_repo);
        assert!(entry("added").is_workspace);
        assert!(!entry("repo").is_workspace);
        assert!(entry("beta").is_readable);
    }

    #[test]
    fn browses_one_level_down() {
        let (_dir, home) = home();
        std::fs::create_dir_all(home.join("Projects/zwiim")).unwrap();
        let listing = browse(&home, &segs(&["Projects"]), &HashSet::new()).unwrap();
        assert_eq!(names(&listing), ["zwiim"]);
        assert_eq!(listing.path, segs(&["Projects"]));
        assert_eq!(listing.display_path, "~/Projects");
    }

    #[test]
    fn rejects_segments_that_are_not_plain_names() {
        let (_dir, home) = home();
        std::fs::create_dir_all(home.join("a/.ssh")).unwrap();
        for bad in [
            &[".."][..],
            &["."],
            &[""],
            &["a/b"],
            &["a", ".ssh"],
            &["/etc"],
            &["a\0b"],
        ] {
            let e = browse(&home, &segs(bad), &HashSet::new()).unwrap_err();
            assert!(matches!(e.code, AppErrorCode::InvalidInput), "{bad:?}");
            let e = resolve_home_path(&home, &segs(bad)).unwrap_err();
            assert!(matches!(e.code, AppErrorCode::InvalidInput), "{bad:?}");
        }
    }

    #[test]
    fn missing_folder_or_file_is_not_found() {
        let (_dir, home) = home();
        std::fs::write(home.join("file"), "x").unwrap();
        for path in [&["gone"][..], &["file"]] {
            let e = browse(&home, &segs(path), &HashSet::new()).unwrap_err();
            assert!(matches!(e.code, AppErrorCode::NotFound), "{path:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_followed_only_inside_home() {
        let (dir, home) = home();
        std::fs::create_dir_all(home.join("real/inner")).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(outside.join("secret")).unwrap();
        std::os::unix::fs::symlink(home.join("real"), home.join("inside-link")).unwrap();
        std::os::unix::fs::symlink(&outside, home.join("outside-link")).unwrap();
        std::os::unix::fs::symlink(home.join("missing"), home.join("dangling")).unwrap();

        let listing = browse(&home, &[], &HashSet::new()).unwrap();
        assert_eq!(names(&listing), ["inside-link", "real"]);
        let inside = browse(&home, &segs(&["inside-link"]), &HashSet::new()).unwrap();
        assert_eq!(names(&inside), ["inner"]);
        let e = browse(&home, &segs(&["outside-link"]), &HashSet::new()).unwrap_err();
        assert!(matches!(e.code, AppErrorCode::NotFound));
        let e = resolve_home_path(&home, &segs(&["outside-link"])).unwrap_err();
        assert!(matches!(e.code, AppErrorCode::NotFound));
        assert!(
            !format!("{e:?}").contains("outside"),
            "nothing about the target: {e:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_folder_is_flagged_and_refused() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, home) = home();
        let locked = home.join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let listing = browse(&home, &[], &HashSet::new());
        let opened = browse(&home, &segs(&["locked"]), &HashSet::new());
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        let listing = listing.unwrap();
        assert!(!listing.entries[0].is_readable);
        assert!(matches!(
            opened.unwrap_err().code,
            AppErrorCode::PermissionDenied
        ));
    }

    #[test]
    fn caps_the_listing() {
        let (_dir, home) = home();
        for i in 0..=BROWSE_LIMIT {
            std::fs::create_dir(home.join(format!("d{i:04}"))).unwrap();
        }
        let listing = browse(&home, &[], &HashSet::new()).unwrap();
        assert_eq!(listing.entries.len(), BROWSE_LIMIT);
        assert!(listing.truncated);
        assert_eq!(listing.entries[0].name, "d0000");
    }

    #[test]
    fn resolves_a_folder_under_home() {
        let (_dir, home) = home();
        std::fs::create_dir_all(home.join("Projects/zwiim")).unwrap();
        assert_eq!(
            resolve_home_path(&home, &segs(&["Projects", "zwiim"])).unwrap(),
            home.join("Projects/zwiim")
        );
        assert_eq!(resolve_home_path(&home, &[]).unwrap(), home);
    }

    #[test]
    fn creates_one_folder_that_browses_and_resolves() {
        let (_dir, home) = home();
        std::fs::create_dir(home.join("Projects")).unwrap();
        let entry = create_folder(&home, &segs(&["Projects"]), "new app").unwrap();
        assert_eq!(
            entry,
            BrowseEntry {
                name: "new app".into(),
                is_git_repo: false,
                is_workspace: false,
                is_readable: true,
            }
        );
        assert!(home.join("Projects/new app").is_dir());
        let listing = browse(&home, &segs(&["Projects"]), &HashSet::new()).unwrap();
        assert_eq!(names(&listing), ["new app"]);
        assert_eq!(
            resolve_home_path(&home, &segs(&["Projects", "new app"])).unwrap(),
            home.join("Projects/new app")
        );
        // Straight inside home too.
        create_folder(&home, &[], "top").unwrap();
        assert!(home.join("top").is_dir());
    }

    #[test]
    fn create_folder_refuses_bad_names_and_parents() {
        let (_dir, home) = home();
        std::fs::create_dir_all(home.join("a/.ssh")).unwrap();
        for bad in ["", ".", "..", ".hidden", "a/b", "/etc", "..\\x", "a\0b"] {
            let e = create_folder(&home, &[], bad).unwrap_err();
            assert!(matches!(e.code, AppErrorCode::InvalidInput), "{bad:?}");
        }
        let e = create_folder(&home, &[], &"x".repeat(FOLDER_NAME_MAX + 1)).unwrap_err();
        assert!(matches!(e.code, AppErrorCode::InvalidInput));
        create_folder(&home, &[], &"x".repeat(FOLDER_NAME_MAX)).unwrap();
        for parent in [&[".."][..], &["a", ".ssh"], &["/etc"]] {
            let e = create_folder(&home, &segs(parent), "new").unwrap_err();
            assert!(matches!(e.code, AppErrorCode::InvalidInput), "{parent:?}");
        }
        // No parents are made.
        let e = create_folder(&home, &segs(&["missing"]), "new").unwrap_err();
        assert!(matches!(e.code, AppErrorCode::NotFound));
        assert!(!home.join("missing").exists());
    }

    #[test]
    fn create_folder_never_overwrites() {
        let (_dir, home) = home();
        std::fs::create_dir(home.join("taken")).unwrap();
        std::fs::write(home.join("taken/keep.txt"), "x").unwrap();
        std::fs::write(home.join("file"), "x").unwrap();
        for name in ["taken", "file"] {
            let e = create_folder(&home, &[], name).unwrap_err();
            assert!(matches!(e.code, AppErrorCode::AlreadyExists), "{name}");
        }
        assert!(home.join("taken/keep.txt").exists());
        assert!(home.join("file").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn create_folder_stays_inside_home() {
        let (dir, home) = home();
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, home.join("outside-link")).unwrap();
        let e = create_folder(&home, &segs(&["outside-link"]), "new").unwrap_err();
        assert!(matches!(e.code, AppErrorCode::NotFound));
        assert!(!outside.join("new").exists());
        // A dangling link in the name's place is taken, never followed.
        std::os::unix::fs::symlink(outside.join("made"), home.join("dangling")).unwrap();
        let e = create_folder(&home, &[], "dangling").unwrap_err();
        assert!(matches!(e.code, AppErrorCode::AlreadyExists));
        assert!(!outside.join("made").exists());
    }

    #[cfg(unix)]
    #[test]
    fn create_folder_in_a_read_only_folder_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, home) = home();
        let locked = home.join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let made = create_folder(&home, &segs(&["locked"]), "new");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(made.unwrap_err().code, AppErrorCode::PermissionDenied));
    }
}
