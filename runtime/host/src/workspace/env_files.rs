//! A new worktree gets the repository folder's Git-ignored `.env*` files,
//! so it runs like the folder although Git never holds them.

use std::path::{Component, Path, PathBuf};

/// Larger is not a settings file.
const MAX_ENV_FILE_BYTES: u64 = 1024 * 1024;

/// Copy each ignored `.env*` regular file of `root` to the same place in
/// `worktree`. Ignored directories (`node_modules/`) are not walked; an
/// existing file is never overwritten; failures are logged and skipped.
pub(super) async fn copy_env_files(root: &Path, worktree: &Path) {
    let listed = crate::process::tokio_command("git")
        .args([
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ])
        .current_dir(root)
        .output()
        .await;
    let listing = match listed {
        Ok(out) if out.status.success() => out.stdout,
        Ok(out) => {
            tracing::warn!(
                "[worktree] listing ignored files failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            return;
        }
        Err(e) => {
            tracing::warn!("[worktree] listing ignored files failed: {e}");
            return;
        }
    };
    let (root, worktree) = (root.to_path_buf(), worktree.to_path_buf());
    if let Err(e) =
        tokio::task::spawn_blocking(move || copy_listed(&root, &worktree, &listing)).await
    {
        tracing::warn!("[worktree] copying .env files did not finish: {e}");
    }
}

fn copy_listed(root: &Path, worktree: &Path, listing: &[u8]) {
    let inside = match worktree.canonicalize() {
        Ok(inside) => inside,
        Err(e) => {
            tracing::warn!(
                "[worktree] resolving the worktree path failed: {} ({e})",
                worktree.display()
            );
            return;
        }
    };
    for entry in listing.split(|b| *b == 0) {
        let rel = match std::str::from_utf8(entry) {
            Ok(rel) => rel,
            Err(e) => {
                tracing::warn!(
                    "[worktree] a listed path was not UTF-8: {:?} ({e})",
                    String::from_utf8_lossy(entry)
                );
                continue;
            }
        };
        if rel.is_empty() || rel.ends_with('/') {
            continue;
        }
        let rel = Path::new(rel);
        let env_named = rel
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(".env"));
        if !env_named || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            continue;
        }
        let from = root.join(rel);
        let meta = match std::fs::symlink_metadata(&from) {
            Ok(meta) => meta,
            Err(e) => {
                tracing::warn!(
                    "[worktree] reading a .env file's metadata failed: {} ({e})",
                    from.display()
                );
                continue;
            }
        };
        if !meta.file_type().is_file() || meta.len() > MAX_ENV_FILE_BYTES {
            continue;
        }
        if let Err(e) = copy_one(&from, &inside, rel) {
            tracing::warn!("[worktree] copying a .env file failed: {e}");
        }
    }
}

/// Copy `from` to `inside/rel` unless something is there, refusing a parent
/// that resolves outside `inside` (a tracked symlink).
fn copy_one(from: &Path, inside: &Path, rel: &Path) -> std::io::Result<()> {
    let to: PathBuf = inside.join(rel);
    if std::fs::symlink_metadata(&to).is_ok() {
        return Ok(());
    }
    let parent = to.parent().unwrap_or(inside);
    std::fs::create_dir_all(parent)?;
    if !parent.canonicalize()?.starts_with(inside) {
        return Ok(());
    }
    std::fs::copy(from, &to).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    /// A repository ignoring `.env*`, `node_modules/` and `big/`, with a
    /// tracked `apps/web/` and a tracked symlink `linked -> <outside>`.
    fn repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(root.join("apps/web")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(root.join(".gitignore"), ".env*\nnode_modules/\n").unwrap();
        std::fs::write(root.join("apps/web/index.js"), "").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "base"]);
        (dir, root, outside)
    }

    #[tokio::test]
    async fn copies_ignored_env_files_and_nothing_else() {
        let (dir, root, outside) = repo();
        std::fs::write(root.join(".env"), "A=1").unwrap();
        std::fs::write(root.join("apps/web/.env.local"), "B=2").unwrap();
        std::fs::set_permissions(
            root.join(".env"),
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("node_modules/pkg/.env"), "C=3").unwrap();
        std::fs::write(root.join(".env.big"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
        std::os::unix::fs::symlink(root.join(".env"), root.join(".env.link")).unwrap();
        std::fs::write(root.join("linked/.env.out"), "D=4").unwrap();
        let wt = dir.path().join("wt");
        git(
            &root,
            &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()],
        );
        std::fs::write(wt.join(".env.keep"), "mine").unwrap();
        std::fs::write(root.join(".env.keep"), "theirs").unwrap();

        copy_env_files(&root, &wt).await;

        assert_eq!(std::fs::read_to_string(wt.join(".env")).unwrap(), "A=1");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(wt.join(".env"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::read_to_string(wt.join("apps/web/.env.local")).unwrap(),
            "B=2"
        );
        assert_eq!(
            std::fs::read_to_string(wt.join(".env.keep")).unwrap(),
            "mine"
        );
        assert!(!wt.join("node_modules").exists());
        assert!(!wt.join(".env.big").exists());
        assert!(std::fs::symlink_metadata(wt.join(".env.link")).is_err());
        // `linked/` in the worktree points outside it: nothing written there.
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn a_folder_that_is_not_a_repository_copies_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join("wt");
        std::fs::create_dir(&wt).unwrap();
        copy_env_files(dir.path(), &wt).await;
        assert_eq!(std::fs::read_dir(&wt).unwrap().count(), 0);
    }
}
