//! A new worktree gets the repository folder's Git-ignored files the user
//! chose (`.env*` unless they chose others), so it runs like the folder
//! although Git never holds them.

use std::path::{Component, Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// Larger is not a settings file.
const MAX_COPIED_FILE_BYTES: u64 = 1024 * 1024;

/// Copy each ignored regular file of `root` that `patterns` (`.gitignore`
/// syntax, `!` excluding) select to the same place in `worktree`. Ignored
/// directories (`node_modules/`) are not walked; an existing file is never
/// overwritten; failures are logged and skipped.
pub(super) async fn copy_ignored_files(root: &Path, worktree: &Path, patterns: &[String]) {
    let Some(selected) = selector(root, patterns) else {
        return;
    };
    let mut cmd = crate::process::tokio_command("git");
    cmd.args([
        "ls-files",
        "-z",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--directory",
    ])
    .current_dir(root)
    .kill_on_drop(true);
    // The user's Git config applies here too, so a hook could otherwise
    // hang this indefinitely; time out like the other git steps.
    let listed = tokio::time::timeout(super::repo_init::GIT_STEP_TIMEOUT, cmd.output()).await;
    let listing = match listed {
        Ok(Ok(out)) if out.status.success() => out.stdout,
        Ok(Ok(out)) => {
            tracing::warn!(
                "[worktree] listing ignored files failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            return;
        }
        Ok(Err(e)) => {
            tracing::warn!("[worktree] listing ignored files failed: {e}");
            return;
        }
        Err(_) => {
            tracing::warn!("[worktree] listing ignored files timed out");
            return;
        }
    };
    let (root, worktree) = (root.to_path_buf(), worktree.to_path_buf());
    if let Err(e) = tokio::task::spawn_blocking(move || {
        copy_listed(&root, &worktree, &listing, &selected)
    })
    .await
    {
        tracing::warn!("[worktree] copying ignored files did not finish: {e}");
    }
}

/// The matcher of `patterns`; `None` when none select anything.
fn selector(root: &Path, patterns: &[String]) -> Option<Gitignore> {
    if patterns.is_empty() {
        return None;
    }
    let mut builder = GitignoreBuilder::new(root);
    for pattern in patterns {
        // Checked when saved; one that no longer compiles selects nothing.
        if let Err(e) = builder.add_line(None, pattern) {
            tracing::warn!("[worktree] skipping a file pattern: {e}");
        }
    }
    match builder.build() {
        Ok(selected) if selected.num_ignores() > 0 => Some(selected),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("[worktree] file patterns did not compile: {e}");
            None
        }
    }
}

fn copy_listed(root: &Path, worktree: &Path, listing: &[u8], selected: &Gitignore) {
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
        if rel.components().any(|c| !matches!(c, Component::Normal(_)))
            || !selected.matched_path_or_any_parents(rel, false).is_ignore()
        {
            continue;
        }
        let from = root.join(rel);
        let meta = match std::fs::symlink_metadata(&from) {
            Ok(meta) => meta,
            Err(e) => {
                tracing::warn!(
                    "[worktree] reading an ignored file's metadata failed: {} ({e})",
                    from.display()
                );
                continue;
            }
        };
        if !meta.file_type().is_file() || meta.len() > MAX_COPIED_FILE_BYTES {
            continue;
        }
        if let Err(e) = copy_one(&from, &inside, rel) {
            tracing::warn!("[worktree] copying an ignored file failed: {e}");
        }
    }
}

/// Copy `from` to `inside/rel` unless something is there. Refuses (skips)
/// when the nearest existing ancestor of the target's parent, or the parent
/// once created, resolves outside `inside` (a tracked symlink), so nothing is
/// ever created through a symlink escaping the worktree.
fn copy_one(from: &Path, inside: &Path, rel: &Path) -> std::io::Result<()> {
    let to: PathBuf = inside.join(rel);
    if std::fs::symlink_metadata(&to).is_ok() {
        return Ok(());
    }
    let parent = to.parent().unwrap_or(inside);
    // Walk up to the nearest existing ancestor before creating anything: if
    // that ancestor is a symlink resolving outside the worktree,
    // `create_dir_all` would otherwise create directories through it,
    // escaping the worktree before the existing-parent check below ever
    // runs.
    let mut ancestor = parent;
    while !ancestor.exists() {
        match ancestor.parent() {
            Some(p) => ancestor = p,
            None => return Ok(()),
        }
    }
    if !ancestor.canonicalize()?.starts_with(inside) {
        return Ok(());
    }
    std::fs::create_dir_all(parent)?;
    if !parent.canonicalize()?.starts_with(inside) {
        return Ok(());
    }
    std::fs::copy(from, &to).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_files() -> Vec<String> {
        vec![".env*".to_string()]
    }

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

        copy_ignored_files(&root, &wt, &env_files()).await;

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

    /// A real directory existed where the worktree's `linked/` is, at the
    /// time `copy_ignored_files` starts walking it, then gets replaced by a
    /// symlink escaping the worktree — e.g. a racing process, or a prior
    /// partial checkout. The root has an ignored, *uncommitted* `linked/`
    /// directory of its own (never tracked), so a nested `.env` under it is
    /// reported as ignored by `git ls-files`, two levels deep, so that the
    /// vulnerable `create_dir_all` has an intermediate parent to create.
    #[tokio::test]
    async fn refuses_a_worktree_parent_replaced_by_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(root.join(".gitignore"), ".env*\n").unwrap();
        std::fs::write(root.join("README"), "").unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "base"]);

        // `linked/` is never tracked or committed in root; only the nested
        // `.env` inside it exists on disk, ignored by `.gitignore`.
        std::fs::create_dir_all(root.join("linked/sub")).unwrap();
        std::fs::write(root.join("linked/sub/.env"), "LEAK=1").unwrap();

        // Verify git actually reports it as ignored before relying on that.
        let listed = std::process::Command::new("git")
            .args(["ls-files", "-o", "-i", "--exclude-standard"])
            .current_dir(&root)
            .output()
            .unwrap();
        let listing = String::from_utf8_lossy(&listed.stdout).into_owned();
        assert!(
            listing.contains("linked/sub/.env"),
            "git did not report linked/sub/.env as ignored: {listing:?}"
        );

        let wt = dir.path().join("wt");
        git(
            &root,
            &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()],
        );
        // The worktree never checked out `linked/` (root never committed
        // it). A real directory briefly exists there, then is replaced by a
        // symlink escaping the worktree, before `copy_ignored_files` runs.
        std::fs::create_dir(wt.join("linked")).unwrap();
        std::fs::remove_dir(wt.join("linked")).unwrap();
        std::os::unix::fs::symlink(&outside, wt.join("linked")).unwrap();

        copy_ignored_files(&root, &wt, &env_files()).await;

        assert!(
            !outside.join("sub").exists(),
            "escaped the worktree through a symlinked ancestor"
        );
    }

    #[tokio::test]
    async fn copies_what_the_patterns_select() {
        let (dir, root, _) = repo();
        std::fs::write(
            root.join(".gitignore"),
            ".env*\nnode_modules/\n*.local.json\nsecrets/\nbuild.log\n",
        )
        .unwrap();
        std::fs::write(root.join(".env"), "A=1").unwrap();
        std::fs::write(root.join(".env.example"), "A=").unwrap();
        std::fs::write(root.join("apps/web/app.local.json"), "{}").unwrap();
        std::fs::create_dir_all(root.join("secrets/ci")).unwrap();
        std::fs::write(root.join("secrets/ci/key.pem"), "k").unwrap();
        std::fs::write(root.join("build.log"), "log").unwrap();
        // Selected but tracked: Git holds it, so the worktree already has it.
        std::fs::write(root.join("apps/web/tracked.local.json"), "tracked").unwrap();
        git(&root, &["add", "-f", "apps/web/tracked.local.json"]);
        git(&root, &["commit", "-q", "-m", "tracked"]);
        let wt = dir.path().join("wt");
        git(
            &root,
            &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()],
        );

        let patterns = [".env*", "!.env.example", "*.local.json", "/secrets/"];
        copy_ignored_files(&root, &wt, &patterns.map(String::from)).await;

        assert_eq!(std::fs::read_to_string(wt.join(".env")).unwrap(), "A=1");
        assert!(!wt.join(".env.example").exists());
        assert_eq!(
            std::fs::read_to_string(wt.join("apps/web/app.local.json")).unwrap(),
            "{}"
        );
        // An ignored directory is listed whole and not walked, even when selected.
        assert!(!wt.join("secrets").exists());
        assert!(!wt.join("build.log").exists());
        assert_eq!(
            std::fs::read_to_string(wt.join("apps/web/tracked.local.json")).unwrap(),
            "tracked"
        );
    }

    #[tokio::test]
    async fn no_patterns_copy_nothing() {
        let (dir, root, _) = repo();
        std::fs::write(root.join(".env"), "A=1").unwrap();
        let wt = dir.path().join("wt");
        git(
            &root,
            &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()],
        );
        copy_ignored_files(&root, &wt, &[]).await;
        copy_ignored_files(&root, &wt, &["!.env".to_string()]).await;
        assert!(!wt.join(".env").exists());
    }

    #[tokio::test]
    async fn a_folder_that_is_not_a_repository_copies_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join("wt");
        std::fs::create_dir(&wt).unwrap();
        copy_ignored_files(dir.path(), &wt, &env_files()).await;
        assert_eq!(std::fs::read_dir(&wt).unwrap().count(), 0);
    }
}
