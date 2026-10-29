//! Zwiim-owned read-only workspace diff (`git.diff`).
//!
//! One combined view of everything that differs from the workspace's base:
//! - a worktree is compared against `merge-base(base_commit, HEAD)`, so its
//!   committed, staged and unstaged changes all show;
//! - a root workspace (or a worktree with no recorded base) is compared
//!   against its own `HEAD`: its uncommitted changes.
//!
//! Untracked files show as added; ignored files never show. Binary and
//! oversized files are listed without content. Symlinks are never followed.

use std::path::Path;

use serde::Serialize;

use crate::app_error::AppCommandError;
use crate::db::entities::folder;

/// At most this many files are listed; `truncated` reports the rest.
pub const MAX_FILES: usize = 1000;
/// A file whose patch is larger than this is listed without content.
pub const MAX_PATCH_BYTES: usize = 512 * 1024;
/// A tracked file with more changed lines than this is listed without content.
pub const MAX_PATCH_LINES: u64 = 20_000;
/// An untracked file larger than this is listed without content.
pub const MAX_UNTRACKED_BYTES: u64 = 1024 * 1024;

/// git's well-known empty tree, the base of a repository with no commits.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffFile {
    /// Workspace-relative path.
    pub path: String,
    pub change: ChangeKind,
    /// Changed line counts; `None` for binary or oversized files.
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub binary: bool,
    pub too_large: bool,
    /// Not tracked by git: shown as added.
    pub untracked: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceDiff {
    /// The commit the workspace is compared against; `None` before the
    /// repository's first commit.
    pub base_commit: Option<String>,
    pub files: Vec<DiffFile>,
    /// More than [`MAX_FILES`] files changed; the rest are not listed.
    pub truncated: bool,
    /// With `file`: its unified diff, or `None` when binary or oversized.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<Option<String>>,
}

fn git(dir: &str) -> tokio::process::Command {
    let mut cmd = crate::process::tokio_command("git");
    cmd.arg("--no-optional-locks")
        .args(["-c", "core.quotePath=false"])
        .current_dir(dir)
        // A dropped diff (the Git Harness's time limit, a cancelled call)
        // takes its git with it.
        .kill_on_drop(true);
    cmd
}

async fn run(dir: &str, args: &[&str]) -> Result<std::process::Output, AppCommandError> {
    git(dir)
        .args(args)
        .output()
        .await
        .map_err(AppCommandError::io)
}

async fn stdout_ok(dir: &str, what: &str, args: &[&str]) -> Result<Vec<u8>, AppCommandError> {
    let out = run(dir, args).await?;
    if !out.status.success() {
        return Err(AppCommandError::external_command(
            format!("git {what} failed"),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(out.stdout)
}

/// The commit (or empty tree) workspace `row` is compared against.
async fn base_of(row: &folder::Model) -> Result<Option<String>, AppCommandError> {
    if let (Some(_), Some(base)) = (row.parent_id, row.base_commit.as_deref()) {
        let out = stdout_ok(&row.path, "merge-base", &["merge-base", base, "HEAD"]).await?;
        return Ok(Some(String::from_utf8_lossy(&out).trim().to_string()));
    }
    let head = run(&row.path, &["rev-parse", "--verify", "HEAD^{commit}"]).await?;
    Ok(head
        .status
        .success()
        .then(|| String::from_utf8_lossy(&head.stdout).trim().to_string()))
}

/// Split NUL-terminated git output into UTF-8 fields.
fn fields(out: &[u8]) -> impl Iterator<Item = String> + '_ {
    out.split(|b| *b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| String::from_utf8_lossy(f).into_owned())
}

async fn tracked(dir: &str, base: &str) -> Result<Vec<DiffFile>, AppCommandError> {
    const OPTS: [&str; 4] = [
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
    ];
    let mut args = vec!["diff"];
    args.extend(OPTS);
    args.extend(["--name-status", "-z", base]);
    let status = stdout_ok(dir, "diff", &args).await?;
    let mut args = vec!["diff"];
    args.extend(OPTS);
    args.extend(["--numstat", "-z", base]);
    let numstat = stdout_ok(dir, "diff", &args).await?;

    // numstat -z: "<add>\t<del>\t<path>\0"; binary files report "-\t-".
    let mut counts = std::collections::HashMap::new();
    for record in fields(&numstat) {
        let mut parts = record.splitn(3, '\t');
        let (Some(a), Some(d), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        counts.insert(
            path.to_string(),
            (a.parse::<u64>().ok(), d.parse::<u64>().ok()),
        );
    }

    // name-status -z: "<letter>\0<path>\0" pairs.
    let mut files = Vec::new();
    let mut it = fields(&status);
    while let (Some(letter), Some(path)) = (it.next(), it.next()) {
        let change = match letter.chars().next() {
            Some('A') => ChangeKind::Added,
            Some('D') => ChangeKind::Deleted,
            _ => ChangeKind::Modified,
        };
        let (additions, deletions) = counts.get(&path).copied().unwrap_or((None, None));
        let binary = additions.is_none() && deletions.is_none();
        let too_large = additions.unwrap_or(0) + deletions.unwrap_or(0) > MAX_PATCH_LINES;
        files.push(DiffFile {
            path,
            change,
            additions: additions.filter(|_| !too_large),
            deletions: deletions.filter(|_| !too_large),
            binary,
            too_large,
            untracked: false,
        });
    }
    Ok(files)
}

/// Describe untracked file `rel` without following symlinks.
fn untracked_file(dir: &Path, rel: String) -> DiffFile {
    let mut file = DiffFile {
        path: rel,
        change: ChangeKind::Added,
        additions: None,
        deletions: Some(0),
        binary: false,
        too_large: false,
        untracked: true,
    };
    let full = dir.join(&file.path);
    let Ok(meta) = std::fs::symlink_metadata(&full) else {
        return file;
    };
    if meta.file_type().is_symlink() {
        // A link is one line: its target path, never the target's content.
        file.additions = Some(1);
    } else if meta.len() > MAX_UNTRACKED_BYTES {
        file.too_large = true;
        file.deletions = None;
    } else if let Ok(bytes) = std::fs::read(&full) {
        if bytes[..bytes.len().min(8000)].contains(&0) {
            file.binary = true;
            file.deletions = None;
        } else {
            let lines = line_count(&bytes);
            file.additions = Some(lines);
        }
    }
    file
}

fn line_count(bytes: &[u8]) -> u64 {
    let newlines = bytes.iter().filter(|b| **b == b'\n').count() as u64;
    if bytes.last().is_some_and(|b| *b != b'\n') {
        newlines + 1
    } else {
        newlines
    }
}

async fn untracked(dir: &str) -> Result<Vec<DiffFile>, AppCommandError> {
    let out = stdout_ok(
        dir,
        "ls-files",
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )
    .await?;
    let root = Path::new(dir).to_path_buf();
    let paths: Vec<String> = fields(&out).filter(|p| !p.ends_with('/')).collect();
    tokio::task::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|p| untracked_file(&root, p))
            .collect()
    })
    .await
    .map_err(|e| AppCommandError::io(std::io::Error::other(e)))
}

/// The unified diff of one listed file, or `None` when it is too large.
async fn patch_of(
    dir: &str,
    base: &str,
    file: &DiffFile,
) -> Result<Option<String>, AppCommandError> {
    let out = if file.untracked {
        // `--no-index` exits 1 when the files differ, which they always do.
        run(
            dir,
            &[
                "diff",
                "--no-index",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--",
                "/dev/null",
                &file.path,
            ],
        )
        .await?
        .stdout
    } else {
        let spec = format!(":(literal){}", file.path);
        stdout_ok(
            dir,
            "diff",
            &[
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                base,
                "--",
                &spec,
            ],
        )
        .await?
    };
    if out.len() > MAX_PATCH_BYTES {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&out).into_owned()))
}

/// The diff of workspace `row`; with `file`, only that file and its patch.
pub async fn diff(
    row: &folder::Model,
    file: Option<&str>,
) -> Result<WorkspaceDiff, AppCommandError> {
    crate::git_repo::ensure_git_repo(&row.path)?;
    let base_commit = base_of(row).await?;
    let base = base_commit.as_deref().unwrap_or(EMPTY_TREE);

    let mut files = tracked(&row.path, base).await?;
    files.extend(untracked(&row.path).await?);
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let Some(wanted) = file else {
        let truncated = files.len() > MAX_FILES;
        files.truncate(MAX_FILES);
        return Ok(WorkspaceDiff {
            base_commit,
            files,
            truncated,
            patch: None,
        });
    };
    let Some(mut entry) = files.into_iter().find(|f| f.path == wanted) else {
        return Err(AppCommandError::not_found("File has no changes"));
    };
    let patch = if entry.binary || entry.too_large {
        None
    } else {
        let patch = patch_of(&row.path, base, &entry).await?;
        if patch.is_none() {
            entry.too_large = true;
            entry.additions = None;
            entry.deletions = None;
        }
        patch
    };
    Ok(WorkspaceDiff {
        base_commit,
        files: vec![entry],
        truncated: false,
        patch: Some(patch),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
    use crate::web::event_bridge::EventEmitter;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn by_path<'a>(d: &'a WorkspaceDiff, path: &str) -> &'a DiffFile {
        d.files
            .iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("{path} not in {:?}", d.files))
    }

    /// A repo with tracked files, registered as a root workspace, plus one
    /// worktree created from it. Returns `(tempdir, root row, worktree row)`.
    async fn repo_and_worktree() -> (tempfile::TempDir, folder::Model, folder::Model) {
        let db = fresh_in_memory_db().await;
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("keep.txt"), "one\ntwo\n").unwrap();
        std::fs::write(repo.join("edit.txt"), "a\nb\n").unwrap();
        std::fs::write(repo.join("gone.txt"), "x\n").unwrap();
        std::fs::write(repo.join(".gitignore"), "*.log\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        let id = seed_folder(&db, repo.to_str().unwrap()).await;
        let root_wid = crate::workspace::workspace_id_of(&db.conn, id)
            .await
            .unwrap()
            .unwrap();
        let wt = crate::workspace::create_worktree(
            &db,
            &EventEmitter::Noop,
            &dir.path().join("worktrees"),
            &root_wid,
            None,
            None,
        )
        .await
        .unwrap();
        let root = crate::workspace::resolve(&db.conn, &root_wid)
            .await
            .unwrap();
        let wt = crate::workspace::resolve(&db.conn, &wt.workspace_id)
            .await
            .unwrap();
        (dir, root, wt)
    }

    #[tokio::test]
    async fn worktree_diff_combines_committed_staged_unstaged_and_untracked() {
        let (_dir, root, wt) = repo_and_worktree().await;
        let w = Path::new(&wt.path);
        // Committed on the worktree branch.
        std::fs::write(w.join("committed.txt"), "c1\nc2\nc3\n").unwrap();
        std::fs::remove_file(w.join("gone.txt")).unwrap();
        git(w, &["add", "-A"]);
        git(w, &["commit", "-q", "-m", "work"]);
        // Staged.
        std::fs::write(w.join("staged.txt"), "s\n").unwrap();
        git(w, &["add", "staged.txt"]);
        // Unstaged.
        std::fs::write(w.join("edit.txt"), "a\nB\nc\n").unwrap();
        // Untracked, ignored, binary, oversized.
        std::fs::write(w.join("new.txt"), "n1\nn2").unwrap();
        std::fs::write(w.join("debug.log"), "ignored\n").unwrap();
        std::fs::write(w.join("blob.bin"), [0u8, 1, 2, 0, 3]).unwrap();
        std::fs::write(
            w.join("huge.txt"),
            "x".repeat(MAX_UNTRACKED_BYTES as usize + 1),
        )
        .unwrap();
        // The root moving on after the worktree was made is not the worktree's change.
        std::fs::write(Path::new(&root.path).join("root-only.txt"), "r\n").unwrap();
        git(Path::new(&root.path), &["add", "."]);
        git(Path::new(&root.path), &["commit", "-q", "-m", "root moves"]);

        let d = diff(&wt, None).await.unwrap();
        assert_eq!(d.base_commit, wt.base_commit);
        let paths: Vec<_> = d.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "blob.bin",
                "committed.txt",
                "edit.txt",
                "gone.txt",
                "huge.txt",
                "new.txt",
                "staged.txt"
            ]
        );
        assert!(!d.truncated && d.patch.is_none());

        let c = by_path(&d, "committed.txt");
        assert_eq!(
            (c.change, c.additions, c.deletions),
            (ChangeKind::Added, Some(3), Some(0))
        );
        assert!(!c.untracked);
        assert_eq!(by_path(&d, "gone.txt").change, ChangeKind::Deleted);
        assert_eq!(by_path(&d, "staged.txt").change, ChangeKind::Added);
        let e = by_path(&d, "edit.txt");
        assert_eq!(
            (e.change, e.additions, e.deletions),
            (ChangeKind::Modified, Some(2), Some(1))
        );
        let n = by_path(&d, "new.txt");
        assert_eq!(
            (n.change, n.untracked, n.additions, n.deletions),
            (ChangeKind::Added, true, Some(2), Some(0))
        );
        let b = by_path(&d, "blob.bin");
        assert!(b.binary && b.untracked && b.additions.is_none());
        let h = by_path(&d, "huge.txt");
        assert!(h.too_large && h.untracked && h.additions.is_none());

        // One file: its patch. Untracked files diff against nothing.
        let one = diff(&wt, Some("edit.txt")).await.unwrap();
        assert_eq!(one.files.len(), 1);
        let patch = one.patch.clone().flatten().expect("patch");
        assert!(patch.contains("-b\n+B\n+c"), "{patch}");
        let one = diff(&wt, Some("new.txt")).await.unwrap();
        let patch = one.patch.clone().flatten().expect("patch");
        assert!(
            patch.contains("new file mode") && patch.contains("+n1"),
            "{patch}"
        );
        let committed = diff(&wt, Some("committed.txt")).await.unwrap();
        assert!(committed.patch.flatten().unwrap().contains("+c3"));
        // Binary and oversized files come without content.
        assert_eq!(diff(&wt, Some("blob.bin")).await.unwrap().patch, Some(None));
        assert_eq!(diff(&wt, Some("huge.txt")).await.unwrap().patch, Some(None));
        // Unchanged or ignored files are not part of the diff.
        for path in ["keep.txt", "debug.log", "../repo/keep.txt", "/etc/hosts"] {
            let err = diff(&wt, Some(path)).await.unwrap_err();
            assert!(
                matches!(err.code, crate::app_error::AppErrorCode::NotFound),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn root_diff_is_uncommitted_changes_against_head() {
        let (_dir, root, _wt) = repo_and_worktree().await;
        let r = Path::new(&root.path);
        assert!(diff(&root, None).await.unwrap().files.is_empty());

        std::fs::write(r.join("edit.txt"), "a\n").unwrap();
        std::fs::write(r.join("fresh.txt"), "f\n").unwrap();
        let d = diff(&root, None).await.unwrap();
        assert_eq!(
            d.base_commit.as_deref(),
            Some(git(r, &["rev-parse", "HEAD"]).as_str())
        );
        let paths: Vec<_> = d.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["edit.txt", "fresh.txt"]);
        assert_eq!(by_path(&d, "edit.txt").deletions, Some(1));

        // Committing clears it: committed history is not a root's diff.
        git(r, &["add", "."]);
        git(r, &["commit", "-q", "-m", "c"]);
        assert!(diff(&root, None).await.unwrap().files.is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn untracked_symlink_is_never_followed() {
        let (dir, _root, wt) = repo_and_worktree().await;
        let secret = dir.path().join("secret.txt");
        std::fs::write(&secret, "TOP-SECRET\n").unwrap();
        std::os::unix::fs::symlink(&secret, Path::new(&wt.path).join("link")).unwrap();

        let d = diff(&wt, Some("link")).await.unwrap();
        assert_eq!(d.files[0].additions, Some(1));
        let patch = d.patch.flatten().unwrap();
        assert!(!patch.contains("TOP-SECRET"), "{patch}");
    }
}
