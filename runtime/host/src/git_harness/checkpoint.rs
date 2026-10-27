//! Per-turn checkpoints: a Git-native record of a worktree before and after
//! each agent turn.
//!
//! A checkpoint is a parentless commit whose tree is the worktree as it was
//! (tracked plus untracked files, ignored files left out), with HEAD, the
//! index tree and the prompt's fingerprint kept as trailers in its message.
//! It lives under `refs/zwiim/checkpoints/<session_id>/<ordinal>/{start,end}`:
//! never on a branch, never pushed. Refs are shared by every worktree of a
//! repository; the session id in the name keeps sessions apart.
//!
//! Saving never touches HEAD, the index, or the worktree: both trees are
//! written through a private copy of the index. A repository mid-operation
//! (merge, rebase, conflict, held index lock) is skipped with a reason, and so
//! is a directory that is not a Git repository, without running `git` at all.
//! The refs are the whole record: nothing else is stored, and they survive a
//! Host restart.
//!
//! [`restore`] puts a checkpoint's files and index back, saving the state it
//! replaces first. It never moves HEAD.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::error::{GitError, GitErrorCode};
use super::run::{git, run, run_with_input, Output};

/// Everything one save may take before it is abandoned.
const BUDGET: Duration = Duration::from_secs(10);
/// Each step of a restore. Writing a large worktree back takes longer than
/// reading it.
const RESTORE_BUDGET: Duration = Duration::from_secs(60);
/// Commit identity of a checkpoint. Not a person: these commits never join
/// any history.
const AUTHOR_NAME: &str = "Zwiim";
const AUTHOR_EMAIL: &str = "checkpoint@zwiim.invalid";
const NAMESPACE: &str = "refs/zwiim/checkpoints";
/// The most files a [`TurnChanges`] lists.
const MAX_CHANGED_FILES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointKey {
    pub session_id: String,
    /// The session's turn number, from 1.
    pub ordinal: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Start,
    End,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Phase::Start => "start",
            Phase::End => "end",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "start" => Some(Phase::Start),
            "end" => Some(Phase::End),
            _ => None,
        }
    }
}

/// Why no checkpoint was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    NotGit,
    /// The repository is mid-operation: `merge`, `rebase`, `cherry_pick`,
    /// `revert`, `conflict` (unmerged entries) or `index_lock`.
    Busy(&'static str),
    Timeout,
    Failed(String),
}

impl SkipReason {
    /// A short, stable name for logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            SkipReason::NotGit => "not_git",
            SkipReason::Busy(what) => what,
            SkipReason::Timeout => "timeout",
            SkipReason::Failed(_) => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Saved {
    Ref { oid: String },
    Skipped(SkipReason),
}

/// One saved checkpoint, read back from its ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub key: CheckpointKey,
    pub phase: Phase,
    /// HEAD when it was taken; `None` before the first commit.
    pub head: Option<String>,
    pub index_tree: String,
    pub worktree_tree: String,
    /// Fingerprint of the prompt's text in the canonical form
    /// `session_checkpoints` takes it in, on start checkpoints of prompts
    /// that had text.
    pub prompt_fingerprint: Option<String>,
}

/// What a turn changed between its start and end checkpoints.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnChanges {
    /// `(status, path)`, `status` as `git diff --name-status` prints it.
    /// At most [`MAX_CHANGED_FILES`].
    pub files: Vec<(String, String)>,
    /// How many files changed, including any past the list's cap.
    pub file_count: usize,
    pub added: u64,
    pub deleted: u64,
    pub truncated: bool,
}

/// Session ids become ref path components: keep them to a safe alphabet.
fn ref_safe(session_id: &str) -> bool {
    (1..=128).contains(&session_id.len())
        && session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn session_prefix(session_id: &str) -> String {
    format!("{NAMESPACE}/{session_id}/")
}

fn ref_name(key: &CheckpointKey, phase: Phase) -> String {
    format!(
        "{}{}/{}",
        session_prefix(&key.session_id),
        key.ordinal,
        phase.as_str()
    )
}

/// `(ordinal, phase)` of a checkpoint ref under `prefix`; `None` for anything
/// else stored there.
fn parse_ref(prefix: &str, refname: &str) -> Option<(u32, Phase)> {
    let (ordinal, phase) = refname.strip_prefix(prefix)?.split_once('/')?;
    if ordinal.is_empty() || !ordinal.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((ordinal.parse().ok()?, Phase::parse(phase)?))
}

/// `root` as the `&str` the Git runner takes, when it is a Git repository.
/// Detection is the Host's strict one (`<root>/.git` exists): a plain
/// directory inside a repository is not one, and no `git` runs for it.
fn repo_root(root: &Path) -> Option<&str> {
    if !crate::git_repo::is_git_repo(root) {
        return None;
    }
    root.to_str()
}

async fn git_ok(root: &str, args: &[&str], what: &str) -> Result<Output, SkipReason> {
    let mut cmd = git(root);
    cmd.args(args).env("GIT_OPTIONAL_LOCKS", "0");
    checked(run(cmd, BUDGET, what).await, what)
}

fn checked(out: Result<Output, super::error::GitError>, what: &str) -> Result<Output, SkipReason> {
    match out {
        Ok(out) if out.status.success() => Ok(out),
        Ok(out) => Err(SkipReason::Failed(format!("{what}: {}", out.text().trim()))),
        Err(e) if e.code == super::error::GitErrorCode::Timeout => Err(SkipReason::Timeout),
        Err(e) => Err(SkipReason::Failed(e.to_string())),
    }
}

/// The checkpoint refs stored for `session_id`, as full names. An error when
/// Git cannot list them: "none" would be a guess.
async fn session_refs(root: &str, session_id: &str) -> Result<Vec<String>, SkipReason> {
    let prefix = session_prefix(session_id);
    let out = git_ok(
        root,
        &["for-each-ref", "--format=%(refname)", &prefix],
        "list checkpoints",
    )
    .await?;
    Ok(out
        .stdout
        .lines()
        .filter(|r| r.starts_with(&prefix))
        .map(str::to_string)
        .collect())
}

/// `root` as a repository the refs of `session_id` can live in.
fn session_root<'a>(root: &'a Path, session_id: &str) -> Result<&'a str, SkipReason> {
    let root = repo_root(root).ok_or(SkipReason::NotGit)?;
    if !ref_safe(session_id) {
        return Err(SkipReason::Failed("invalid session id".into()));
    }
    Ok(root)
}

/// The ordinal the next turn of `session_id` takes: one past the highest
/// stored, else 1. An error when the stored ones cannot be read.
pub async fn next_ordinal(root: &Path, session_id: &str) -> Result<u32, SkipReason> {
    let root = session_root(root, session_id)?;
    let prefix = session_prefix(session_id);
    Ok(session_refs(root, session_id)
        .await?
        .iter()
        .filter_map(|r| parse_ref(&prefix, r))
        .map(|(ordinal, _)| ordinal)
        .max()
        .map_or(1, |m| m.saturating_add(1)))
}

/// Save the worktree at `root` as `key`'s `phase` checkpoint. A checkpoint
/// already stored under that name is never replaced: the save is skipped.
/// Takes at most [`BUDGET`]; never fails the caller.
pub async fn save(
    root: &Path,
    key: &CheckpointKey,
    phase: Phase,
    prompt_fingerprint: Option<&str>,
) -> Saved {
    let root = match session_root(root, &key.session_id) {
        Ok(root) => root,
        Err(reason) => return Saved::Skipped(reason),
    };
    match tokio::time::timeout(BUDGET, save_in(root, key, phase, prompt_fingerprint)).await {
        Ok(Ok(oid)) => Saved::Ref { oid },
        Ok(Err(reason)) => Saved::Skipped(reason),
        Err(_) => Saved::Skipped(SkipReason::Timeout),
    }
}

async fn save_in(
    root: &str,
    key: &CheckpointKey,
    phase: Phase,
    prompt_fingerprint: Option<&str>,
) -> Result<String, SkipReason> {
    let snap = snapshot(root).await?;
    let subject = format!(
        "zwiim checkpoint {} {}/{}",
        phase.as_str(),
        key.session_id,
        key.ordinal
    );
    let mut trailers = format!(
        "Zwiim-Session: {}\nZwiim-Turn: {}\nZwiim-Phase: {}\n",
        key.session_id,
        key.ordinal,
        phase.as_str()
    );
    snap.push_trailers(&mut trailers);
    if let Some(fp) = prompt_fingerprint.filter(|fp| !fp.contains('\n')) {
        trailers.push_str(&format!("Zwiim-Prompt: {fp}\n"));
    }
    store(
        root,
        &ref_name(key, phase),
        &snap.worktree_tree,
        &subject,
        &trailers,
    )
    .await
}

/// The worktree at `root` as it is now, written as trees through a private
/// copy of the index. Nothing the user sees is touched.
struct Snapshot {
    index_tree: String,
    worktree_tree: String,
    head: Option<String>,
}

impl Snapshot {
    fn push_trailers(&self, trailers: &mut String) {
        if let Some(head) = &self.head {
            trailers.push_str(&format!("Zwiim-Head: {head}\n"));
        }
        trailers.push_str(&format!("Zwiim-Index-Tree: {}\n", self.index_tree));
    }
}

async fn snapshot(root: &str) -> Result<Snapshot, SkipReason> {
    // This worktree's own git dir and index; a linked worktree has both apart
    // from the main one's.
    let out = git_ok(
        root,
        &["rev-parse", "--absolute-git-dir", "--git-path", "index"],
        "locate git dir",
    )
    .await?;
    let mut lines = out.stdout.lines();
    let (Some(git_dir), Some(index)) = (lines.next(), lines.next()) else {
        return Err(SkipReason::Failed(
            "locate git dir: unexpected output".into(),
        ));
    };
    let git_dir = PathBuf::from(git_dir);
    let index = Path::new(root).join(index);
    if let Some(reason) = busy(&git_dir, &index) {
        return Err(SkipReason::Busy(reason));
    }

    // A private copy of the index. Even a plain `write-tree` may rewrite the
    // real one (its cache-tree), and the user's index must stay byte-identical.
    let scratch =
        tempfile::tempdir().map_err(|e| SkipReason::Failed(format!("scratch index: {e}")))?;
    let tmp_index = scratch.path().join("index");
    // No index yet (before the first `add`) means an empty one: leave the
    // copy absent, which Git reads as empty. An empty file would be invalid.
    if index.exists() {
        tokio::fs::copy(&index, &tmp_index)
            .await
            .map_err(|e| SkipReason::Failed(format!("scratch index: {e}")))?;
    }
    let tmp_index = tmp_index
        .to_str()
        .ok_or_else(|| SkipReason::Failed("scratch index path is not UTF-8".into()))?;

    let index_tree = match with_index(root, tmp_index, &["write-tree"], "write index tree").await {
        Ok(out) => out.stdout.trim().to_string(),
        Err(reason) => {
            // `write-tree` refuses an index with unmerged entries.
            let unmerged =
                with_index(root, tmp_index, &["ls-files", "-u", "-z"], "list conflicts").await;
            return match unmerged {
                Ok(out) if !out.stdout.is_empty() => Err(SkipReason::Busy("conflict")),
                _ => Err(reason),
            };
        }
    };
    with_index(root, tmp_index, &["add", "-A"], "stage worktree").await?;
    let worktree_tree = with_index(root, tmp_index, &["write-tree"], "write worktree tree")
        .await?
        .stdout
        .trim()
        .to_string();
    let head = head(root).await?;
    Ok(Snapshot {
        index_tree,
        worktree_tree,
        head,
    })
}

/// Commit `tree` as a parentless checkpoint and store it at `refname`, which
/// must not exist yet. The new commit's oid.
async fn store(
    root: &str,
    refname: &str,
    tree: &str,
    subject: &str,
    trailers: &str,
) -> Result<String, SkipReason> {
    let mut cmd = git(root);
    cmd.args([
        "commit-tree",
        "--no-gpg-sign",
        tree,
        "-m",
        subject,
        "-m",
        trailers,
    ])
    .env("GIT_AUTHOR_NAME", AUTHOR_NAME)
    .env("GIT_AUTHOR_EMAIL", AUTHOR_EMAIL)
    .env("GIT_COMMITTER_NAME", AUTHOR_NAME)
    .env("GIT_COMMITTER_EMAIL", AUTHOR_EMAIL);
    let oid = checked(
        run(cmd, BUDGET, "write checkpoint").await,
        "write checkpoint",
    )?
    .stdout
    .trim()
    .to_string();
    // Create-only: the empty old value makes Git refuse a ref that exists.
    git_ok(root, &["update-ref", refname, &oid, ""], "store checkpoint").await?;
    Ok(oid)
}

/// What a [`restore`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    /// HEAD is no longer the commit it was when the checkpoint was taken.
    /// It was left where it is.
    pub head_moved: bool,
}

/// Put the worktree and index at `root` back to `cp`. HEAD, branches,
/// commits and ignored files are never touched: a turn's commits stay and
/// show as undone changes.
///
/// The state being replaced is saved first, as a recovery checkpoint at
/// `refs/zwiim/checkpoints/<session_id>/revert-<unix_ms>`. It is not a turn
/// (listing, numbering and capping ignore it) and goes when the session's
/// checkpoints do. When it cannot be saved, nothing is restored. A
/// repository mid-operation is refused with [`GitErrorCode::Conflict`] and
/// the operation as the detail.
///
/// The caller holds the workspace exclusively: nothing else may write the
/// worktree meanwhile.
pub async fn restore(root: &Path, cp: &Checkpoint) -> Result<Restored, GitError> {
    let root = session_root(root, &cp.key.session_id).map_err(restore_error)?;

    let snap = snapshot(root).await.map_err(restore_error)?;
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let mut trailers = format!(
        "Zwiim-Session: {}\nZwiim-Phase: revert\n",
        cp.key.session_id
    );
    snap.push_trailers(&mut trailers);
    store(
        root,
        &format!("{}revert-{unix_ms}", session_prefix(&cp.key.session_id)),
        &snap.worktree_tree,
        &format!("zwiim checkpoint revert {}", cp.key.session_id),
        &trailers,
    )
    .await
    .map_err(restore_error)?;

    // The worktree first, through the index: files the checkpoint has are
    // written, tracked files it lacks are removed. Then files no index entry
    // names (ignored ones excepted), then the index the checkpoint had.
    for (args, what) in [
        (
            vec!["read-tree", "--reset", "-u", cp.worktree_tree.as_str()],
            "restore files",
        ),
        (vec!["clean", "-f", "-d", "-q"], "remove new files"),
        (
            vec!["read-tree", cp.index_tree.as_str()],
            "restore the index",
        ),
    ] {
        let mut cmd = git(root);
        cmd.args(&args);
        checked(run(cmd, RESTORE_BUDGET, what).await, what).map_err(restore_error)?;
    }
    let head = head(root).await.map_err(restore_error)?;
    Ok(Restored {
        head_moved: head != cp.head,
    })
}

fn restore_error(reason: SkipReason) -> GitError {
    match reason {
        SkipReason::NotGit => GitError::new(
            GitErrorCode::NotAGitRepository,
            "The workspace is not a Git repository",
        ),
        SkipReason::Busy(what) => GitError::new(
            GitErrorCode::Conflict,
            "The repository is in the middle of another operation",
        )
        .with_detail(what),
        SkipReason::Timeout => GitError::new(GitErrorCode::Timeout, "Restoring files timed out"),
        SkipReason::Failed(detail) => {
            GitError::new(GitErrorCode::Failed, "Restoring files failed").with_detail(&detail)
        }
    }
}

/// `git <args>` against the private index at `index`.
async fn with_index(
    root: &str,
    index: &str,
    args: &[&str],
    what: &str,
) -> Result<Output, SkipReason> {
    let mut cmd = git(root);
    cmd.args(args)
        .env("GIT_INDEX_FILE", index)
        .env("GIT_OPTIONAL_LOCKS", "0");
    checked(run(cmd, BUDGET, what).await, what)
}

/// HEAD's commit; `None` before the first commit (`--verify -q` exits 1
/// and prints nothing). Any other failure is an error.
async fn head(root: &str) -> Result<Option<String>, SkipReason> {
    let mut cmd = git(root);
    cmd.args(["rev-parse", "-q", "--verify", "HEAD^{commit}"])
        .env("GIT_OPTIONAL_LOCKS", "0");
    let out = match run(cmd, BUDGET, "read HEAD").await {
        Ok(out) if out.status.code() == Some(1) && out.stdout.trim().is_empty() => {
            return Ok(None);
        }
        out => checked(out, "read HEAD")?,
    };
    Ok(Some(out.stdout.trim().to_string()).filter(|s| !s.is_empty()))
}

/// The operation in progress in this worktree, if any.
fn busy(git_dir: &Path, index: &Path) -> Option<&'static str> {
    if let Some(op) = super::conflicts::operation(git_dir) {
        return Some(op);
    }
    let mut lock = index.as_os_str().to_owned();
    lock.push(".lock");
    if Path::new(&lock).exists() {
        return Some("index_lock");
    }
    None
}

/// Every checkpoint of `session_id`, oldest first (start before end within a
/// turn). Refs that do not read back as checkpoints are left out. An error
/// when they cannot be listed.
pub async fn list(root: &Path, session_id: &str) -> Result<Vec<Checkpoint>, SkipReason> {
    let root = session_root(root, session_id)?;
    let prefix = session_prefix(session_id);
    let out = git_ok(
        root,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(tree)%00%(contents)%00",
            &prefix,
        ],
        "list checkpoints",
    )
    .await?;
    let fields: Vec<&str> = out.stdout.split('\0').collect();
    let mut found: Vec<Checkpoint> = fields
        .as_chunks::<3>()
        .0
        .iter()
        .filter_map(|record| {
            let refname = record[0].trim_start_matches('\n');
            let (ordinal, phase) = parse_ref(&prefix, refname)?;
            let trailer = |name: &str| {
                let tag = format!("{name}: ");
                record[2]
                    .lines()
                    .find_map(|l| l.strip_prefix(tag.as_str()))
                    .map(|v| v.trim().to_string())
            };
            Some(Checkpoint {
                key: CheckpointKey {
                    session_id: session_id.to_string(),
                    ordinal,
                },
                phase,
                head: trailer("Zwiim-Head"),
                index_tree: trailer("Zwiim-Index-Tree")?,
                worktree_tree: record[1].to_string(),
                prompt_fingerprint: trailer("Zwiim-Prompt"),
            })
        })
        .collect();
    found.sort_by_key(|c| (c.key.ordinal, c.phase == Phase::End));
    Ok(found)
}

/// Delete `refs` in one transaction.
async fn delete_refs(root: &str, refs: &[String]) {
    if refs.is_empty() {
        return;
    }
    let input: String = refs.iter().map(|r| format!("delete {r}\n")).collect();
    let mut cmd = git(root);
    cmd.args(["update-ref", "--stdin"]);
    if let Err(reason) = checked(
        run_with_input(cmd, input.as_bytes(), BUDGET, "delete checkpoints").await,
        "delete checkpoints",
    ) {
        tracing::warn!(
            count = refs.len(),
            reason = reason.as_str(),
            detail = ?reason,
            "[zwiim] checkpoint cleanup failed"
        );
    }
}

fn cleanup_failed(reason: &SkipReason) {
    tracing::warn!(
        reason = reason.as_str(),
        detail = ?reason,
        "[zwiim] checkpoint cleanup failed: could not list them"
    );
}

/// Delete every checkpoint of `session_id`. Nothing else is touched.
pub async fn gc_session(root: &Path, session_id: &str) {
    let Some(root) = repo_root(root).filter(|_| ref_safe(session_id)) else {
        return;
    };
    match session_refs(root, session_id).await {
        Ok(refs) => delete_refs(root, &refs).await,
        Err(reason) => cleanup_failed(&reason),
    }
}

/// Keep only the last `keep` turns of `session_id`'s checkpoints.
pub async fn cap(root: &Path, session_id: &str, keep: u32) {
    let Some(root) = repo_root(root).filter(|_| ref_safe(session_id)) else {
        return;
    };
    let prefix = session_prefix(session_id);
    let refs = match session_refs(root, session_id).await {
        Ok(refs) => refs,
        Err(reason) => return cleanup_failed(&reason),
    };
    let refs: Vec<(u32, String)> = refs
        .into_iter()
        .filter_map(|r| parse_ref(&prefix, &r).map(|(ordinal, _)| (ordinal, r)))
        .collect();
    let mut ordinals: Vec<u32> = refs.iter().map(|(o, _)| *o).collect();
    ordinals.sort_unstable();
    ordinals.dedup();
    let excess = ordinals.len().saturating_sub(keep as usize);
    if excess == 0 {
        return;
    }
    let oldest_kept = ordinals[excess];
    let doomed: Vec<String> = refs
        .into_iter()
        .filter(|(o, _)| *o < oldest_kept)
        .map(|(_, r)| r)
        .collect();
    delete_refs(root, &doomed).await;
}

/// Forget the turns of `session_id` from turn `from` on, both phases. Restore
/// points (`revert-<ms>`) and earlier turns stay. Failures are logged.
pub async fn drop_from(root: &Path, session_id: &str, from: u32) {
    let Some(root) = repo_root(root).filter(|_| ref_safe(session_id)) else {
        return;
    };
    let prefix = session_prefix(session_id);
    let refs = match session_refs(root, session_id).await {
        Ok(refs) => refs,
        Err(reason) => return cleanup_failed(&reason),
    };
    let doomed: Vec<String> = refs
        .into_iter()
        .filter(|r| parse_ref(&prefix, r).is_some_and(|(ordinal, _)| ordinal >= from))
        .collect();
    delete_refs(root, &doomed).await;
}

/// What changed between `key`'s start and end checkpoints; `None` when either
/// is missing or the diff fails.
pub async fn changes(root: &Path, key: &CheckpointKey) -> Option<TurnChanges> {
    let root = repo_root(root).filter(|_| ref_safe(&key.session_id))?;
    let start = ref_name(key, Phase::Start);
    let end = ref_name(key, Phase::End);
    let numstat = git_ok(
        root,
        &[
            "diff",
            "--numstat",
            "-z",
            "--no-renames",
            &start,
            &end,
            "--",
        ],
        "diff checkpoints",
    )
    .await
    .ok()?;
    let names = git_ok(
        root,
        &[
            "diff",
            "--name-status",
            "-z",
            "--no-renames",
            &start,
            &end,
            "--",
        ],
        "diff checkpoints",
    )
    .await
    .ok()?;

    let mut changes = TurnChanges::default();
    for line in numstat.stdout.split('\0').filter(|l| !l.is_empty()) {
        let mut cols = line.splitn(3, '\t');
        // Binary files count as changed with no lines (`-`).
        changes.added += cols.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        changes.deleted += cols.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    }
    let fields: Vec<&str> = names.stdout.split('\0').filter(|f| !f.is_empty()).collect();
    for pair in fields.as_chunks::<2>().0 {
        changes.file_count += 1;
        if changes.files.len() < MAX_CHANGED_FILES {
            changes
                .files
                .push((pair[0].to_string(), pair[1].to_string()));
        } else {
            changes.truncated = true;
        }
    }
    Some(changes)
}
