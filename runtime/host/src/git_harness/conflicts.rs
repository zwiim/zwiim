//! Whether a workspace's branch still merges with its base: the conflicts a
//! merge of `base` into `HEAD` would hit, or the merge or rebase already
//! stopped on them.
//!
//! The check never touches the worktree, the index, or any ref. A would-be
//! merge runs as `git merge-tree --write-tree`, which only adds unreferenced
//! objects to the repository.

use std::path::Path;

use serde::Serialize;

use super::error::{GitError, GitErrorCode};
use super::local::{not_an_option, ok};
use super::run::{self, Output, READ, REMOTE};
use super::GitHost;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConflictState {
    Clean,
    /// Merging `base` into `HEAD` would conflict in `files`.
    Conflicts {
        files: Vec<String>,
    },
    /// A merge or rebase is in progress; `files` are still unmerged.
    /// `operation` is `unmerged` when entries are left with none running
    /// (a conflicted `git stash pop` or `checkout -m`).
    InProgress {
        operation: String,
        files: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConflictCheck {
    /// The ref the branch is compared with, e.g. `origin/main`.
    pub base: String,
    pub base_sha: String,
    pub head_sha: String,
    #[serde(flatten)]
    pub state: ConflictState,
}

/// Where the repository at `root` stands against the remote-tracking ref
/// `base` (`<remote>/<branch>`): [`check_merge`], with files a resolve turn
/// committed conflict markers in (see [`note_markers`]) kept as conflicts
/// while they still hold them.
pub async fn check_conflicts(root: &Path, base: &str) -> Result<Option<ConflictCheck>, GitError> {
    let Some(mut check) = check_merge(root, base).await? else {
        return Ok(None);
    };
    if check.state != ConflictState::Clean {
        return Ok(Some(check));
    }
    let Some((noted_base, files)) = read_note(root).await? else {
        return Ok(Some(check));
    };
    if noted_base != base {
        return Ok(Some(check));
    }
    let markers = leftover_markers(root, &files).await?;
    if markers.is_empty() {
        note_markers(root, base, &[]).await?;
    } else {
        check.state = ConflictState::Conflicts { files: markers };
    }
    Ok(Some(check))
}

/// Where the repository at `root` stands against `base` by Git alone: a
/// stopped operation, else a would-be merge. `None` when `base`
/// or `HEAD` does not resolve to a commit, or this Git cannot run a
/// would-be merge (`merge-tree --write-tree` needs Git 2.38).
pub async fn check_merge(root: &Path, base: &str) -> Result<Option<ConflictCheck>, GitError> {
    let root = utf8(root)?;
    not_an_option("base", base)?;
    // The full ref, so a local branch or tag named like `base` cannot stand
    // in for it.
    let tracking = format!("refs/remotes/{base}");
    let (Some(base_sha), Some(head_sha)) =
        (commit(root, &tracking).await?, commit(root, "HEAD").await?)
    else {
        return Ok(None);
    };
    let state = match in_progress(root).await? {
        Some(state) => state,
        None => match merge_tree(root, &base_sha, &head_sha).await? {
            Some(state) => state,
            None => return Ok(None),
        },
    };
    Ok(Some(ConflictCheck {
        base: base.to_string(),
        base_sha,
        head_sha,
        state,
    }))
}

/// The remote-tracking ref a workspace is compared with: for a worktree,
/// its `base_branch` when that names one, else `origin/<base_branch>`; for
/// a root, its branch's upstream. `None` when there is none to compare with.
pub async fn conflict_base(
    root: &Path,
    worktree: bool,
    base_branch: Option<&str>,
) -> Result<Option<String>, GitError> {
    let root = utf8(root)?;
    if !worktree {
        let out = read(
            root,
            &["rev-parse", "--symbolic-full-name", "@{u}"],
            "git rev-parse",
        )
        .await?;
        // A branch tracking a local branch has no remote base.
        let upstream = out.stdout.trim().strip_prefix("refs/remotes/");
        return Ok(upstream
            .filter(|u| out.status.success() && !u.is_empty())
            .map(str::to_string));
    }
    let Some(branch) = base_branch else {
        return Ok(None);
    };
    not_an_option("base_branch", branch)?;
    for candidate in [branch.to_string(), format!("origin/{branch}")] {
        let out = read(
            root,
            &[
                "rev-parse",
                "-q",
                "--verify",
                &format!("refs/remotes/{candidate}^{{commit}}"),
            ],
            "git rev-parse",
        )
        .await?;
        if out.status.success() {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

/// Bring the remote-tracking ref `base` (`<remote>/<branch>`) up to date:
/// `git fetch --no-tags <remote> <branch>`, with the Host's credential
/// handling and never a prompt.
pub async fn fetch_base(host: &GitHost<'_>, root: &Path, base: &str) -> Result<(), GitError> {
    let root = utf8(root)?;
    not_an_option("base", base)?;
    let (remote, branch) = split_base(root, base).await?;
    not_an_option("branch", &branch)?;
    let mut cmd = super::remote::remote_git(host, root, &remote).await;
    cmd.args(["fetch", "--no-tags", &remote, &branch]);
    ok("fetch", run::run(cmd, REMOTE, "git fetch").await?)?;
    Ok(())
}

/// The operation in progress in the worktree whose git dir is `git_dir`, if
/// any.
pub(super) fn operation(git_dir: &Path) -> Option<&'static str> {
    if git_dir.join("MERGE_HEAD").exists() {
        return Some("merge");
    }
    if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        return Some("rebase");
    }
    if git_dir.join("CHERRY_PICK_HEAD").exists() {
        return Some("cherry_pick");
    }
    if git_dir.join("REVERT_HEAD").exists() {
        return Some("revert");
    }
    None
}

/// Files past this size are not scanned for markers.
const MARKER_SCAN_MAX_BYTES: u64 = 8 << 20;

/// The note in the git dir naming files a resolve turn left markers in.
const MARKER_NOTE: &str = "zwiim-resolve-markers";

/// Records that a resolve turn against `base` left conflict markers in
/// `files`, so [`check_conflicts`] keeps showing them after the merge is
/// committed. An empty `files` removes the note.
pub async fn note_markers(root: &Path, base: &str, files: &[String]) -> Result<(), GitError> {
    let path = note_path(root).await?;
    let files: Vec<&str> = files
        .iter()
        .map(String::as_str)
        .filter(|f| !f.contains('\n'))
        .collect();
    let written = if files.is_empty() {
        match tokio::fs::remove_file(&path).await {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    } else {
        tokio::fs::write(&path, format!("{base}\n{}\n", files.join("\n"))).await
    };
    written.map_err(|e| {
        GitError::new(GitErrorCode::Failed, "Recording conflict markers failed")
            .with_detail(&e.to_string())
    })
}

/// The files the marker note lists against `base`; empty when there is no
/// note or it was written against another base.
pub async fn noted_markers(root: &Path, base: &str) -> Result<Vec<String>, GitError> {
    Ok(match read_note(root).await? {
        Some((noted, files)) if noted == base => files,
        _ => Vec::new(),
    })
}

/// The base and files of the marker note, `None` when there is none.
async fn read_note(root: &Path) -> Result<Option<(String, Vec<String>)>, GitError> {
    let Ok(text) = tokio::fs::read_to_string(note_path(root).await?).await else {
        return Ok(None);
    };
    let mut lines = text.lines().filter(|l| !l.is_empty());
    let Some(base) = lines.next() else {
        return Ok(None);
    };
    Ok(Some((base.to_string(), lines.map(String::from).collect())))
}

async fn note_path(root: &Path) -> Result<std::path::PathBuf, GitError> {
    let git_dir = ok(
        "rev-parse",
        read(
            utf8(root)?,
            &["rev-parse", "--absolute-git-dir"],
            "git rev-parse",
        )
        .await?,
    )?;
    Ok(Path::new(git_dir.stdout.trim()).join(MARKER_NOTE))
}

/// Which of `files` (paths relative to `root`) still hold a conflict: a line
/// opening one (`<<<<<<< `) and a later line closing it (`>>>>>>> `), the
/// marker length taken from each file's `conflict-marker-size` attribute
/// (default 7). Reads the worktree's contents, whatever the index or HEAD
/// hold. Missing files, non-regular files, files past
/// [`MARKER_SCAN_MAX_BYTES`] and paths leaving `root` are skipped.
pub async fn leftover_markers(root: &Path, files: &[String]) -> Result<Vec<String>, GitError> {
    // A path leaving the worktree is skipped; `git check-attr` would fail on it.
    let files: Vec<&String> = files
        .iter()
        .filter(|f| {
            let path = Path::new(f.as_str());
            !path.is_absolute()
                && !path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
        })
        .collect();
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let dir = utf8(root)?;
    let mut args = vec!["check-attr", "-z", "conflict-marker-size", "--"];
    args.extend(files.iter().map(|f| f.as_str()));
    let attrs = ok("check-attr", read(dir, &args, "git check-attr").await?)?;
    // `<path>\0<attribute>\0<value>\0`, per path.
    let fields: Vec<&str> = attrs.stdout.split('\0').collect();
    let size_of = |file: &str| {
        fields
            .chunks(3)
            .find(|c| c.len() == 3 && c[0] == file)
            .and_then(|c| c[2].parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(7)
    };
    let wanted: Vec<(String, usize)> = files.iter().map(|f| (f.to_string(), size_of(f))).collect();
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        wanted
            .into_iter()
            .filter(|(file, size)| {
                let path = root.join(file);
                let regular = std::fs::symlink_metadata(&path)
                    .is_ok_and(|m| m.is_file() && m.len() <= MARKER_SCAN_MAX_BYTES);
                regular && std::fs::read(&path).is_ok_and(|bytes| holds_markers(&bytes, *size))
            })
            .map(|(file, _)| file)
            .collect()
    })
    .await
    .map_err(|e| {
        GitError::new(GitErrorCode::Failed, "Scanning for conflict markers failed")
            .with_detail(&e.to_string())
    })
}

/// A line of `size` `<` then a space, and a later line of `size` `>` then a
/// space.
fn holds_markers(bytes: &[u8], size: usize) -> bool {
    let marker = |line: &[u8], c: u8| {
        line.len() > size && line[..size].iter().all(|b| *b == c) && line[size] == b' '
    };
    let mut open = false;
    for line in bytes.split(|b| *b == b'\n') {
        if !open {
            open = marker(line, b'<');
        } else if marker(line, b'>') {
            return true;
        }
    }
    false
}

fn utf8(root: &Path) -> Result<&str, GitError> {
    root.to_str().ok_or_else(|| {
        GitError::new(
            GitErrorCode::InvalidInput,
            "The workspace path is not UTF-8",
        )
    })
}

/// A read-only `git <args>` that takes no optional locks.
async fn read(root: &str, args: &[&str], what: &str) -> Result<Output, GitError> {
    let mut cmd = run::git(root);
    cmd.args(args).env("GIT_OPTIONAL_LOCKS", "0");
    run::run(cmd, READ, what).await
}

/// `rev`'s commit, or `None` when it names none.
async fn commit(root: &str, rev: &str) -> Result<Option<String>, GitError> {
    let out = read(
        root,
        &["rev-parse", "-q", "--verify", &format!("{rev}^{{commit}}")],
        "git rev-parse",
    )
    .await?;
    let sha = out.stdout.trim();
    Ok((out.status.success() && !sha.is_empty()).then(|| sha.to_string()))
}

/// The stopped merge or rebase, or unmerged entries left by anything else.
pub(super) async fn in_progress(root: &str) -> Result<Option<ConflictState>, GitError> {
    let git_dir = ok(
        "rev-parse",
        read(root, &["rev-parse", "--absolute-git-dir"], "git rev-parse").await?,
    )?;
    let running = operation(Path::new(git_dir.stdout.trim()));
    let listed = ok(
        "ls-files",
        read(root, &["ls-files", "-u", "-z"], "git ls-files").await?,
    )?;
    let files = unmerged_paths(&listed.stdout);
    Ok(match (running, files.is_empty()) {
        (Some(op), _) => Some(ConflictState::InProgress {
            operation: op.to_string(),
            files,
        }),
        (None, false) => Some(ConflictState::InProgress {
            operation: "unmerged".to_string(),
            files,
        }),
        (None, true) => None,
    })
}

/// Paths from `git ls-files -u -z` (`<mode> <oid> <stage>\t<path>\0`), one
/// per path.
fn unmerged_paths(stdout: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for record in stdout.split('\0') {
        let Some((_, path)) = record.split_once('\t') else {
            continue;
        };
        if paths.last().map(String::as_str) != Some(path) {
            paths.push(path.to_string());
        }
    }
    paths
}

/// A would-be merge of `base` into `head`. `None` when Git cannot run one
/// (too old, unrelated histories).
async fn merge_tree(root: &str, base: &str, head: &str) -> Result<Option<ConflictState>, GitError> {
    let out = read(
        root,
        &[
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            "-z",
            base,
            head,
        ],
        "git merge-tree",
    )
    .await?;
    Ok(match out.status.code() {
        Some(0) => Some(ConflictState::Clean),
        Some(1) => {
            // `<tree oid>\0<path>\0<path>\0…`; a path with several conflicts
            // is listed once each.
            let mut files: Vec<String> = Vec::new();
            for path in out.stdout.split('\0').skip(1).filter(|p| !p.is_empty()) {
                if !files.iter().any(|f| f == path) {
                    files.push(path.to_string());
                }
            }
            Some(ConflictState::Conflicts { files })
        }
        _ => None,
    })
}

/// `base` as `(remote, branch)`, matching the longest configured remote
/// name, since remote names may contain `/`.
async fn split_base(root: &str, base: &str) -> Result<(String, String), GitError> {
    let remotes = ok("remote", read(root, &["remote"], "git remote").await?)?;
    remotes
        .stdout
        .lines()
        .map(str::trim)
        .filter_map(|remote| {
            let branch = base.strip_prefix(remote)?.strip_prefix('/')?;
            (!remote.is_empty() && !branch.is_empty()).then(|| (remote, branch))
        })
        .max_by_key(|(remote, _)| remote.len())
        .map(|(remote, branch)| (remote.to_string(), branch.to_string()))
        .ok_or_else(|| {
            GitError::new(
                GitErrorCode::NotFound,
                format!("{base} is not a branch of a remote"),
            )
        })
}
