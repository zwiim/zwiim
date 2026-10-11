//! Making a plain folder a Git repository: `git init` on `main`, a basic
//! `.gitignore` when it has none, and one "Initial Commit" of everything that
//! is not ignored. A folder that already has `.git` is never touched.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::AsyncWriteExt;

/// Written when the folder has no top-level `.gitignore`, so credentials and
/// dependency/build output stay out of the first commit.
pub(crate) const BASIC_GITIGNORE: &str = "\
.env
.env.*
*.pem
*.key
node_modules/
.DS_Store
dist/
build/
target/
.venv/
__pycache__/
";

/// The author of a repository Zwiim publishes: the signed-in GitHub account
/// with its no-reply email, so a private email never reaches a commit and
/// GitHub's email-privacy push check never refuses one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Identity {
    pub name: String,
    pub email: String,
}

impl Identity {
    pub(crate) fn from_account(account: &crate::git_harness::GithubAccount) -> Self {
        let name = account
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(&account.login)
            .to_string();
        Self {
            name,
            email: format!("{}+{}@users.noreply.github.com", account.id, account.login),
        }
    }
}

/// Never pushed by a published first commit, whatever the folder's own
/// `.gitignore` says.
const SECRET_EXCLUDES: &str = ".env\n.env.*\n*.pem\n*.key\n";

/// A first commit over either limit waits for the user to confirm it.
pub(crate) const LARGE_FOLDER_FILES: u64 = 5_000;
pub(crate) const LARGE_FOLDER_BYTES: u64 = 500 * 1024 * 1024;

/// What `git add -A` would take in a folder with no `.git` yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FirstCommitSize {
    pub files: u64,
    pub bytes: u64,
    /// Counting stopped at a limit: the real numbers are higher.
    pub at_least: bool,
}

impl FirstCommitSize {
    pub(crate) fn is_large(&self) -> bool {
        self.files > LARGE_FOLDER_FILES || self.bytes > LARGE_FOLDER_BYTES
    }
}

/// Estimate the first commit without touching the folder: regular files and
/// symlinks Git would add (hidden ones included, `.gitignore` and global
/// excludes respected, the basic patterns standing in for a missing
/// top-level `.gitignore`), not descending into nested repositories,
/// stopping as soon as a limit is passed. Blocking: run it on
/// `spawn_blocking`.
pub(crate) fn measure_first_commit(path: &Path) -> FirstCommitSize {
    let walker = first_commit_walk(path, |_| {});
    let mut size = FirstCommitSize {
        files: 0,
        bytes: 0,
        at_least: false,
    };
    for entry in walker.flatten() {
        // Git commits a symlink as its own entry (the link text, never the
        // target); `entry.metadata()` already reports the link itself,
        // since `follow_links(false)` keeps it from resolving through.
        if !entry
            .file_type()
            .is_some_and(|t| t.is_file() || t.is_symlink())
        {
            continue;
        }
        size.files += 1;
        size.bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
        if size.is_large() {
            size.at_least = true;
            break;
        }
    }
    size
}

/// Walk `path` the way `git add -A` would in a folder with no `.git` yet
/// (see `measure_first_commit`). Each nested repository is handed to
/// `nested` and not descended into.
fn first_commit_walk(path: &Path, nested: impl Fn(&Path) + Send + Sync + 'static) -> ignore::Walk {
    let basic = (!path.join(".gitignore").exists()).then(|| {
        let mut builder = ignore::gitignore::GitignoreBuilder::new(path);
        for line in BASIC_GITIGNORE.lines() {
            let _ = builder.add_line(None, line);
        }
        builder
            .build()
            .unwrap_or_else(|_| ignore::gitignore::Gitignore::empty())
    });
    ignore::WalkBuilder::new(path)
        .hidden(false)
        .follow_links(false)
        .parents(false)
        .ignore(false)
        .git_global(true)
        .git_ignore(true)
        .git_exclude(false)
        .require_git(false)
        .filter_entry(move |entry| {
            if entry.depth() == 0 {
                return true;
            }
            if entry.file_name() == ".git" {
                return false;
            }
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            if basic.as_ref().is_some_and(|gi| {
                gi.matched_path_or_any_parents(entry.path(), is_dir)
                    .is_ignore()
            }) {
                return false;
            }
            // Git records a nested repository as one entry, not its files.
            if is_dir && entry.path().join(".git").exists() {
                nested(entry.path());
                return false;
            }
            true
        })
        .build()
}

/// The nested repositories `git add -A` would reach below `path`, at any
/// depth. Blocking: run it on `spawn_blocking`.
fn nested_repositories(path: &Path) -> Vec<PathBuf> {
    let found = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&found);
    let walker = first_commit_walk(path, move |dir| {
        sink.lock().unwrap().push(dir.to_path_buf());
    });
    for _ in walker {}
    let found = std::mem::take(&mut *found.lock().unwrap());
    found
}

/// An `info/exclude` line matching exactly the directory `rel` (relative to
/// the repository root): anchored, with glob characters and spaces escaped.
/// `None` for a name a pattern line cannot hold.
fn exclude_line(rel: &Path) -> Option<String> {
    let mut line = String::from("/");
    for part in rel.components() {
        let part = part.as_os_str().to_str()?;
        if part.contains(['\n', '\r']) {
            return None;
        }
        for c in part.chars() {
            if matches!(c, '\\' | '*' | '?' | '[' | ' ') {
                line.push('\\');
            }
            line.push(c);
        }
        line.push('/');
    }
    Some(line)
}

/// Leave out of the first commit each nested repository with no commit yet:
/// `git add -A` has no commit to record for it and fails the whole add. One
/// anchored line each goes in the new repository's `info/exclude`, which
/// rollback removes along with `.git`. Nested repositories with a commit are
/// still recorded (as gitlinks).
async fn exclude_unborn_nested(path: &Path, env: &[(&str, &str)]) -> Result<(), String> {
    let root = path.to_path_buf();
    let nested = tokio::task::spawn_blocking(move || nested_repositories(&root))
        .await
        .map_err(|e| format!("finding nested repositories: {e}"))?;
    let mut lines = String::new();
    for dir in nested {
        // Should discovery pass an unusable `dir/.git`, it reaches only the
        // new repository, whose HEAD does not resolve yet either.
        if git(&dir, env, &["rev-parse", "-q", "--verify", "HEAD"])
            .await
            .is_ok()
        {
            continue;
        }
        if let Some(line) = dir.strip_prefix(path).ok().and_then(exclude_line) {
            lines.push_str(&line);
            lines.push('\n');
        }
    }
    if lines.is_empty() {
        return Ok(());
    }
    append_exclude(path, &lines).await
}

/// Append `lines` to the repository's `.git/info/exclude`.
async fn append_exclude(path: &Path, lines: &str) -> Result<(), String> {
    let info = path.join(".git").join("info");
    tokio::fs::create_dir_all(&info)
        .await
        .map_err(|e| format!("creating .git/info: {e}"))?;
    let mut exclude = tokio::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(info.join("exclude"))
        .await
        .map_err(|e| format!("opening .git/info/exclude: {e}"))?;
    // A leading newline in case the file does not end in one.
    exclude
        .write_all(format!("\n{lines}").as_bytes())
        .await
        .map_err(|e| format!("writing .git/info/exclude: {e}"))
}

/// What one `init_repository` call created, so a later failure undoes
/// exactly that and nothing of the user's.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Created {
    wrote_gitignore: bool,
}

/// Git sees a repository at `path` itself (`.git` dir, or file in a linked
/// worktree).
pub(crate) fn has_git(path: &Path) -> bool {
    std::fs::symlink_metadata(path.join(".git")).is_ok()
}

/// Make `path` a repository with one "Initial Commit". `Ok(None)`: it already
/// was one and nothing changed. `Err`: why, for the Host log; whatever this
/// call created is already removed.
pub(crate) async fn init_repository(path: &Path) -> Result<Option<Created>, String> {
    init_repository_with(path, &[]).await
}

async fn init_repository_with(
    path: &Path,
    env: &[(&str, &str)],
) -> Result<Option<Created>, String> {
    init_repository_as(path, env, None).await
}

/// [`init_repository`] for a repository about to be published: authored as
/// `identity` (set in its own config) and with [`SECRET_EXCLUDES`].
pub(crate) async fn init_published_repository(
    path: &Path,
    identity: &Identity,
) -> Result<Option<Created>, String> {
    init_repository_as(path, &[], Some(identity)).await
}

async fn init_repository_as(
    path: &Path,
    env: &[(&str, &str)],
    identity: Option<&Identity>,
) -> Result<Option<Created>, String> {
    if has_git(path) {
        return Ok(None);
    }
    let mut created = Created {
        wrote_gitignore: false,
    };
    match first_commit(path, env, identity, &mut created).await {
        Ok(()) => Ok(Some(created)),
        Err(e) => {
            rollback(path, created).await;
            Err(e)
        }
    }
}

async fn first_commit(
    path: &Path,
    env: &[(&str, &str)],
    identity: Option<&Identity>,
    created: &mut Created,
) -> Result<(), String> {
    let gitignore = path.join(".gitignore");
    let write_gitignore = tokio::fs::symlink_metadata(&gitignore).await.is_err();
    // `.git` and its marker come before `git init`, so a Host stopped at any
    // later point leaves a repository `recover_interrupted` knows is ours.
    let marker = path.join(".git").join(IN_PROGRESS);
    tokio::fs::create_dir(path.join(".git"))
        .await
        .map_err(|e| format!("creating .git: {e}"))?;
    tokio::fs::write(&marker, if write_gitignore { "gitignore\n" } else { "" })
        .await
        .map_err(|e| format!("writing the init marker: {e}"))?;
    git(path, env, &["init", "-q", "-b", "main"]).await?;
    if let Some(identity) = identity {
        git(
            path,
            env,
            &["config", "--local", "user.name", &identity.name],
        )
        .await?;
        git(
            path,
            env,
            &["config", "--local", "user.email", &identity.email],
        )
        .await?;
        append_exclude(path, SECRET_EXCLUDES).await?;
    }
    if write_gitignore {
        // Marked before writing: nothing was there, so even a partly written
        // file is ours for rollback to remove.
        created.wrote_gitignore = true;
        tokio::fs::write(&gitignore, BASIC_GITIGNORE)
            .await
            .map_err(|e| format!("writing .gitignore: {e}"))?;
    }
    exclude_unborn_nested(path, env).await?;
    git(path, env, &["add", "-A"]).await?;
    let mut commit = Vec::new();
    if !has_identity(path, env).await {
        commit.extend(["-c", "user.name=Zwiim", "-c", "user.email=zwiim@localhost"]);
    }
    commit.extend(["commit", "-q", "--allow-empty", "-m", "Initial Commit"]);
    git(path, env, &commit).await?;
    tokio::fs::remove_file(&marker)
        .await
        .map_err(|e| format!("removing the init marker: {e}"))
}

/// Kept in `.git` while `init_repository` runs and removed once the first
/// commit is made, so a Host stopped mid-way leaves it behind. It holds
/// `gitignore` when that run writes the basic `.gitignore`.
const IN_PROGRESS: &str = "zwiim-init";

/// Undo a repository that an earlier `init_repository` left half-made when
/// the Host stopped, so the folder is plain again and gets its first commit.
/// `true` if it did. Call under the repository-init lock, before `has_git`.
/// A repository the user made never has the marker and is never touched.
pub(crate) async fn recover_interrupted(path: &Path) -> bool {
    let dot_git = path.join(".git");
    match tokio::fs::read_to_string(dot_git.join(IN_PROGRESS)).await {
        Ok(marker) => {
            let created = Created {
                wrote_gitignore: marker.trim() == "gitignore",
            };
            rollback(path, created).await;
            true
        }
        // Stopped between creating `.git` and writing the marker. An empty
        // `.git` is no repository to Git, so removing it loses nothing.
        Err(_) => tokio::fs::remove_dir(&dot_git).await.is_ok(),
    }
}

/// Give an existing repository `identity` unless its own config already
/// names a `user.name` or `user.email`. `true` if this call set it.
pub(crate) async fn set_identity_if_unset(
    path: &Path,
    identity: &Identity,
) -> Result<bool, String> {
    if has_local_identity(path).await {
        return Ok(false);
    }
    set_identity(path, identity).await?;
    Ok(true)
}

/// The repository's own config names a `user.name` or `user.email`.
async fn has_local_identity(path: &Path) -> bool {
    for key in ["user.name", "user.email"] {
        match git(path, &[], &["config", "--local", "--get", key]).await {
            Ok(value) if !value.trim().is_empty() => return true,
            _ => {}
        }
    }
    false
}

/// Write `identity` to the repository's own config.
pub(crate) async fn set_identity(path: &Path, identity: &Identity) -> Result<(), String> {
    git(
        path,
        &[],
        &["config", "--local", "user.name", &identity.name],
    )
    .await?;
    git(
        path,
        &[],
        &["config", "--local", "user.email", &identity.email],
    )
    .await?;
    Ok(())
}

/// Undo [`set_identity_if_unset`].
pub(crate) async fn unset_identity(path: &Path) {
    for key in ["user.name", "user.email"] {
        let _ = git(path, &[], &["config", "--local", "--unset", key]).await;
    }
}

/// An `init_repository` the Host stopped in left `path` half-made.
pub(crate) fn interrupted(path: &Path) -> bool {
    path.join(".git").join(IN_PROGRESS).exists()
}

/// Both `user.name` and `user.email` resolve in the new repository (global,
/// system or `includeIf` config).
async fn has_identity(path: &Path, env: &[(&str, &str)]) -> bool {
    for key in ["user.name", "user.email"] {
        match git(path, env, &["config", "--get", key]).await {
            Ok(value) if !value.trim().is_empty() => {}
            _ => return false,
        }
    }
    true
}

/// Git would sign commits here with some `user.email` (any config level).
pub(crate) async fn has_email(path: &Path, env: &[(&str, &str)]) -> bool {
    matches!(
        git(path, env, &["config", "--get", "user.email"]).await,
        Ok(v) if !v.trim().is_empty()
    )
}

/// How long one git step may run. The user's Git config applies, so a
/// signing prompt or a hook could otherwise hang the first commit forever.
pub(super) const GIT_STEP_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Run git in `path`; stdout on success, else a log-only message with stderr.
pub(super) async fn git(
    path: &Path,
    env: &[(&str, &str)],
    args: &[&str],
) -> Result<String, String> {
    let mut cmd = crate::process::tokio_command("git");
    cmd.args(args)
        .current_dir(path)
        .envs(env.iter().copied())
        .kill_on_drop(true);
    let out = tokio::time::timeout(GIT_STEP_TIMEOUT, cmd.output())
        .await
        .map_err(|_| format!("git {} timed out after 10 minutes", args.join(" ")))?
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Remove what `init_repository` created. User files are never touched.
pub(crate) async fn rollback(path: &Path, created: Created) {
    let _ = tokio::fs::remove_dir_all(path.join(".git")).await;
    if created.wrote_gitignore {
        let _ = tokio::fs::remove_file(path.join(".gitignore")).await;
    }
}

/// `path` is a repository with no commits at all: `HEAD` does not resolve
/// and no branch exists. An orphan branch in a repository with history is
/// not one, and neither is a repository git refuses to read (say, dubious
/// ownership): any git error means "not unborn".
pub(crate) async fn is_unborn(path: &Path, env: &[(&str, &str)]) -> bool {
    if !has_git(path) {
        return false;
    }
    // `rev-parse -q --verify` exits 1, silently, only for a ref that does not
    // resolve; a fatal error is 128.
    let mut cmd = crate::process::tokio_command("git");
    cmd.args(["rev-parse", "-q", "--verify", "HEAD"])
        .current_dir(path)
        .envs(env.iter().copied())
        .kill_on_drop(true);
    let head_unresolved = matches!(
        tokio::time::timeout(GIT_STEP_TIMEOUT, cmd.output()).await,
        Ok(Ok(out)) if out.status.code() == Some(1)
    );
    head_unresolved
        && git(path, env, &["for-each-ref", "--count=1", "refs/heads"])
            .await
            .is_ok_and(|branches| branches.trim().is_empty())
}

/// The `.git` files a first commit in an unborn repository changes and its
/// undo puts back byte for byte (`None`: absent before), besides the
/// branch's own reflog.
const UNBORN_SAVED: [&str; 3] = ["index", "info/exclude", "logs/HEAD"];

/// What one `commit_unborn` call did to the user's repository, so its undo
/// restores exactly what was there.
#[derive(Debug, Clone)]
pub(crate) struct Unborn {
    git_dir: PathBuf,
    branch_ref: String,
    /// Paths relative to `git_dir`.
    saved: Vec<(String, Option<Vec<u8>>)>,
    wrote_gitignore: bool,
    set_identity: bool,
    /// The first commit, once made: the undo deletes the branch only while
    /// it still names this commit.
    commit: Option<String>,
}

/// Make the first commit, "Initial Commit", in the unborn repository at
/// `path` on the branch `HEAD` names: the basic `.gitignore` when it has
/// none, nested unborn repositories left out, and with `identity` (a
/// publish) the secret excludes and that identity unless the repository has
/// its own. Nothing else in `.git` is touched and no marker is written:
/// `recover_interrupted` would delete the user's `.git`. `Err`: why, for the
/// Host log; the repository is already as it was.
pub(crate) async fn commit_unborn(
    path: &Path,
    env: &[(&str, &str)],
    identity: Option<&Identity>,
) -> Result<Unborn, String> {
    let git_dir = path.join(".git");
    if !tokio::fs::symlink_metadata(&git_dir)
        .await
        .is_ok_and(|m| m.is_dir())
    {
        return Err("no .git directory".to_string());
    }
    // Another git is at work in the repository, or one was stopped there.
    let index_lock = git_dir.join("index.lock");
    if tokio::fs::symlink_metadata(&index_lock).await.is_ok() {
        return Err(".git/index.lock exists".to_string());
    }
    let branch_ref = git(path, env, &["symbolic-ref", "-q", "HEAD"])
        .await
        .map_err(|e| format!("HEAD names no branch: {e}"))?
        .trim()
        .to_string();
    if !branch_ref.starts_with("refs/heads/") {
        return Err(format!("HEAD names {branch_ref}, not a branch"));
    }
    // The undo deletes this ref: it must not name a commit of the user's.
    if git(path, env, &["rev-parse", "-q", "--verify", &branch_ref])
        .await
        .is_ok()
    {
        return Err(format!("{branch_ref} already has a commit"));
    }
    let branch_log = format!("logs/{branch_ref}");
    let mut saved = Vec::with_capacity(UNBORN_SAVED.len() + 1);
    for rel in UNBORN_SAVED
        .map(str::to_string)
        .into_iter()
        .chain([branch_log])
    {
        match tokio::fs::read(git_dir.join(&rel)).await {
            Ok(bytes) => saved.push((rel, Some(bytes))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => saved.push((rel, None)),
            Err(e) => return Err(format!("saving .git/{rel}: {e}")),
        }
    }
    let mut u = Unborn {
        git_dir,
        branch_ref,
        saved,
        wrote_gitignore: false,
        set_identity: false,
        commit: None,
    };
    let made = async {
        let gitignore = path.join(".gitignore");
        if tokio::fs::symlink_metadata(&gitignore).await.is_err() {
            // Marked before writing: nothing was there, so even a partly
            // written file is ours for the undo to remove.
            u.wrote_gitignore = true;
            tokio::fs::write(&gitignore, BASIC_GITIGNORE)
                .await
                .map_err(|e| format!("writing .gitignore: {e}"))?;
        }
        exclude_unborn_nested(path, env).await?;
        if let Some(identity) = identity {
            append_exclude(path, SECRET_EXCLUDES).await?;
            if !has_local_identity(path).await {
                // Marked before writing, so a half-written identity is
                // unset too: the repository had none of its own.
                u.set_identity = true;
                set_identity(path, identity).await?;
            }
        }
        git(path, env, &["add", "-A"]).await?;
        let mut commit = Vec::new();
        if !has_identity(path, env).await {
            commit.extend(["-c", "user.name=Zwiim", "-c", "user.email=zwiim@localhost"]);
        }
        commit.extend(["commit", "-q", "--allow-empty", "-m", "Initial Commit"]);
        git(path, env, &commit).await?;
        let head = git(path, env, &["rev-parse", "--verify", "HEAD"]).await?;
        Ok::<_, String>(head.trim().to_string())
    }
    .await;
    match made {
        Ok(commit) => {
            u.commit = Some(commit);
            Ok(u)
        }
        Err(e) => {
            // A git killed mid-step (the timeout) leaves its lock behind;
            // there was none before.
            let _ = tokio::fs::remove_file(&index_lock).await;
            undo_first_commit(path, u).await;
            Err(e)
        }
    }
}

/// Undo [`commit_unborn`]: the branch goes, the saved `.git` files (the
/// branch's reflog among them) are restored exactly, so the staged files
/// are as before, a `.gitignore` it wrote is removed and an identity it set
/// unset. A branch that has moved on from the first commit (the user
/// committed since) is left as it is, with everything else. Never touches
/// `origin` or removes `.git`; the commit object is left for Git's garbage
/// collection.
pub(crate) async fn undo_first_commit(path: &Path, u: Unborn) {
    let mut delete = vec!["update-ref", "-d", u.branch_ref.as_str()];
    delete.extend(u.commit.as_deref());
    match git(path, &[], &delete).await {
        Ok(_) => {}
        // With no commit made the ref may not exist; with one, a ref that
        // no longer names it is the user's work and stays.
        Err(e) if u.commit.is_some() => {
            tracing::warn!(
                "[workspace] undoing the first commit: the branch moved on; left as is: {e}"
            );
            return;
        }
        Err(_) => {}
    }
    for (rel, bytes) in &u.saved {
        if let Err(e) = restore(&u.git_dir.join(rel), bytes.as_deref()).await {
            tracing::warn!(
                "[workspace] undoing the first commit: restoring .git/{rel} failed: {e}"
            );
        }
    }
    if u.wrote_gitignore {
        let _ = tokio::fs::remove_file(path.join(".gitignore")).await;
    }
    if u.set_identity {
        unset_identity(path).await;
    }
}

/// Put `bytes` back at `file` through a temporary file and a rename, so a
/// failure never leaves it half-written; `None` removes it.
async fn restore(file: &Path, bytes: Option<&[u8]>) -> std::io::Result<()> {
    let Some(bytes) = bytes else {
        return match tokio::fs::remove_file(file).await {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        };
    };
    // Deleting a ref can remove its reflog's now-empty directories.
    if let Some(parent) = file.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut tmp = file.as_os_str().to_owned();
    tmp.push(".zwiim-tmp");
    let tmp = PathBuf::from(tmp);
    let written = async {
        tokio::fs::write(&tmp, bytes).await?;
        tokio::fs::rename(&tmp, file).await
    }
    .await;
    if written.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    written
}

/// What one add made in the user's folder, undone if a later step fails.
#[derive(Debug)]
pub(crate) enum Made {
    /// `init_repository` made the repository.
    Repository(Created),
    /// `commit_unborn` made the first commit in the user's repository.
    FirstCommit(Unborn),
}

impl Made {
    pub(crate) async fn undo(self, path: &Path) {
        match self {
            Made::Repository(created) => rollback(path, created).await,
            Made::FirstCommit(u) => undo_first_commit(path, u).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Git config isolated from the developer's: an empty global file (or
    /// the given contents) and no system file.
    struct Config {
        _dir: tempfile::TempDir,
        global: String,
    }

    fn config(contents: &str) -> Config {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("gitconfig");
        std::fs::write(&global, contents).unwrap();
        Config {
            global: global.to_string_lossy().into_owned(),
            _dir: dir,
        }
    }

    const IDENTITY: &str = "[user]\n\tname = t\n\temail = t@example.com\n";

    impl Config {
        fn env(&self) -> [(&str, &str); 2] {
            [
                ("GIT_CONFIG_GLOBAL", self.global.as_str()),
                ("GIT_CONFIG_SYSTEM", "/dev/null"),
            ]
        }
    }

    fn git(dir: &Path, cfg: &Config, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .envs(cfg.env())
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn git_fails(dir: &Path, cfg: &Config, args: &[&str]) -> bool {
        !std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .envs(cfg.env())
            .output()
            .unwrap()
            .status
            .success()
    }

    fn committed(dir: &Path, cfg: &Config) -> Vec<String> {
        let mut files: Vec<String> = git(dir, cfg, &["ls-tree", "-r", "--name-only", "HEAD"])
            .lines()
            .map(str::to_string)
            .collect();
        files.sort();
        files
    }

    fn folder() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project");
        std::fs::create_dir(&path).unwrap();
        (dir, path)
    }

    #[tokio::test]
    async fn empty_folder_gets_main_and_one_commit_with_the_basic_gitignore() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        let created = init_repository_with(&path, &cfg.env()).await.unwrap();
        assert!(created.is_some());
        assert_eq!(git(&path, &cfg, &["branch", "--show-current"]), "main");
        assert_eq!(git(&path, &cfg, &["log", "--format=%s"]), "Initial Commit");
        assert_eq!(committed(&path, &cfg), [".gitignore"]);
        assert_eq!(
            std::fs::read_to_string(path.join(".gitignore")).unwrap(),
            BASIC_GITIGNORE
        );
    }

    #[tokio::test]
    async fn secrets_and_dependencies_stay_out_and_on_disk() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        std::fs::create_dir_all(path.join("src")).unwrap();
        std::fs::write(path.join("src/a.rs"), "fn main() {}\n").unwrap();
        std::fs::write(path.join(".env"), "KEY=secret\n").unwrap();
        std::fs::create_dir_all(path.join("node_modules")).unwrap();
        std::fs::write(path.join("node_modules/x"), "x").unwrap();
        init_repository_with(&path, &cfg.env()).await.unwrap();
        assert_eq!(committed(&path, &cfg), [".gitignore", "src/a.rs"]);
        assert_eq!(
            std::fs::read_to_string(path.join(".env")).unwrap(),
            "KEY=secret\n"
        );
        assert!(path.join("node_modules/x").exists());
    }

    #[tokio::test]
    async fn an_existing_gitignore_is_used_as_is() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        std::fs::write(path.join(".gitignore"), "secret.txt\n").unwrap();
        std::fs::write(path.join("secret.txt"), "s").unwrap();
        std::fs::write(path.join(".env"), "KEY=1\n").unwrap();
        init_repository_with(&path, &cfg.env()).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(path.join(".gitignore")).unwrap(),
            "secret.txt\n"
        );
        // Their rules, not ours: `.env` is theirs to ignore.
        assert_eq!(committed(&path, &cfg), [".env", ".gitignore"]);
    }

    #[tokio::test]
    async fn existing_repository_is_untouched() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        git(&path, &cfg, &["init", "-q", "-b", "dev"]);
        std::fs::write(path.join("a"), "1").unwrap();
        git(&path, &cfg, &["add", "a"]);
        git(&path, &cfg, &["commit", "-q", "-m", "mine"]);
        std::fs::write(path.join("a"), "2").unwrap();
        let before = (
            git(&path, &cfg, &["rev-parse", "HEAD"]),
            git(&path, &cfg, &["status", "--porcelain"]),
        );
        assert!(init_repository_with(&path, &cfg.env())
            .await
            .unwrap()
            .is_none());
        let after = (
            git(&path, &cfg, &["rev-parse", "HEAD"]),
            git(&path, &cfg, &["status", "--porcelain"]),
        );
        assert_eq!(before, after);
        assert_eq!(git(&path, &cfg, &["branch", "--show-current"]), "dev");
        assert!(!path.join(".gitignore").exists());
    }

    #[tokio::test]
    async fn a_folder_inside_another_repository_becomes_its_own() {
        let cfg = config(IDENTITY);
        let (_d, outer) = folder();
        git(&outer, &cfg, &["init", "-q", "-b", "main"]);
        let inner = outer.join("inner");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(inner.join("f"), "x").unwrap();
        init_repository_with(&inner, &cfg.env()).await.unwrap();
        assert!(inner.join(".git").is_dir());
        assert_eq!(committed(&inner, &cfg), [".gitignore", "f"]);
    }

    #[tokio::test]
    async fn a_nested_repository_without_a_commit_is_left_out() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        std::fs::write(path.join("a.txt"), "a").unwrap();
        let sub = path.join("sub");
        std::fs::create_dir(&sub).unwrap();
        git(&sub, &cfg, &["init", "-q"]);
        std::fs::write(sub.join("s.txt"), "s").unwrap();
        init_repository_with(&path, &cfg.env()).await.unwrap();
        assert_eq!(committed(&path, &cfg), [".gitignore", "a.txt"]);
        assert!(sub.join("s.txt").exists());
    }

    #[tokio::test]
    async fn unborn_nested_repositories_are_left_out_by_their_literal_names() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        let unborn = |rel: &str| {
            let dir = path.join(rel);
            std::fs::create_dir_all(&dir).unwrap();
            git(&dir, &cfg, &["init", "-q"]);
        };
        unborn("odd [x]* ?");
        unborn("deep/inner");
        // What the names would match as globs is still committed.
        std::fs::create_dir(path.join("odd x1 z")).unwrap();
        std::fs::write(path.join("odd x1 z/f"), "f").unwrap();
        std::fs::write(path.join("deep/d.txt"), "d").unwrap();
        // A nested repository with a commit stays a gitlink.
        let born = path.join("born");
        std::fs::create_dir(&born).unwrap();
        git(&born, &cfg, &["init", "-q"]);
        git(&born, &cfg, &["commit", "-q", "--allow-empty", "-m", "b"]);
        init_repository_with(&path, &cfg.env()).await.unwrap();
        assert_eq!(
            committed(&path, &cfg),
            [".gitignore", "born", "deep/d.txt", "odd x1 z/f"]
        );
    }

    #[tokio::test]
    async fn without_an_identity_the_author_is_zwiim() {
        let cfg = config("");
        let (_d, path) = folder();
        init_repository_with(&path, &cfg.env()).await.unwrap();
        assert_eq!(
            git(&path, &cfg, &["log", "--format=%an <%ae>"]),
            "Zwiim <zwiim@localhost>"
        );
    }

    #[tokio::test]
    async fn a_configured_identity_is_kept() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        init_repository_with(&path, &cfg.env()).await.unwrap();
        assert_eq!(
            git(&path, &cfg, &["log", "--format=%an <%ae>"]),
            "t <t@example.com>"
        );
    }

    fn octo() -> Identity {
        Identity::from_account(&crate::git_harness::GithubAccount {
            login: "octo".into(),
            id: 42,
            name: Some("Octo Cat".into()),
        })
    }

    #[test]
    fn identity_is_the_account_with_its_noreply_email() {
        assert_eq!(octo().name, "Octo Cat");
        assert_eq!(octo().email, "42+octo@users.noreply.github.com");
        for name in [None, Some("  ".to_string())] {
            let id = Identity::from_account(&crate::git_harness::GithubAccount {
                login: "octo".into(),
                id: 42,
                name,
            });
            assert_eq!(id.name, "octo");
        }
    }

    #[tokio::test]
    async fn published_first_commit_is_authored_as_the_account() {
        let cfg = config("");
        let (_d, path) = folder();
        std::fs::write(path.join("a.txt"), "a").unwrap();
        init_repository_as(&path, &cfg.env(), Some(&octo()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            git(&path, &cfg, &["log", "--format=%an <%ae>"]),
            "Octo Cat <42+octo@users.noreply.github.com>"
        );
        assert_eq!(
            git(&path, &cfg, &["config", "--local", "user.email"]),
            "42+octo@users.noreply.github.com"
        );
    }

    #[tokio::test]
    async fn published_init_never_commits_secrets_a_gitignore_forgets() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        std::fs::write(path.join(".gitignore"), "target\n").unwrap();
        for f in [".env", ".env.local", "key.pem", "id.key", "a.txt"] {
            std::fs::write(path.join(f), "x").unwrap();
        }
        init_repository_as(&path, &cfg.env(), Some(&octo()))
            .await
            .unwrap()
            .unwrap();
        let files = git(&path, &cfg, &["ls-files"]);
        let files: Vec<&str> = files.lines().collect();
        assert_eq!(files, [".gitignore", "a.txt"]);
    }

    #[tokio::test]
    async fn plain_init_keeps_its_fallback_author_and_no_secret_excludes() {
        let cfg = config("");
        let (_d, path) = folder();
        init_repository_with(&path, &cfg.env())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(git(&path, &cfg, &["log", "--format=%an"]), "Zwiim");
        let exclude = std::fs::read_to_string(path.join(".git/info/exclude")).unwrap_or_default();
        assert!(!exclude.contains(".env"), "{exclude}");
    }

    #[tokio::test]
    async fn identity_is_set_only_where_the_repository_has_none() {
        let cfg = config("");
        let (_d, path) = folder();
        git(&path, &cfg, &["init", "-q", "-b", "main"]);
        assert!(set_identity_if_unset(&path, &octo()).await.unwrap());
        assert_eq!(
            git(&path, &cfg, &["config", "--local", "user.name"]),
            "Octo Cat"
        );
        unset_identity(&path).await;
        assert!(git_fails(&path, &cfg, &["config", "--local", "user.name"]));

        git(
            &path,
            &cfg,
            &["config", "--local", "user.email", "me@example.com"],
        );
        assert!(!set_identity_if_unset(&path, &octo()).await.unwrap());
        assert_eq!(
            git(&path, &cfg, &["config", "--local", "user.email"]),
            "me@example.com"
        );
    }

    #[tokio::test]
    async fn a_failing_commit_rolls_back_only_what_it_created() {
        let hooks = tempfile::tempdir().unwrap();
        let hook = hooks.path().join("pre-commit");
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let cfg = config(&format!(
            "{IDENTITY}[core]\n\thooksPath = {}\n",
            hooks.path().display()
        ));
        let (_d, path) = folder();
        std::fs::write(path.join("keep.txt"), "mine").unwrap();
        let err = init_repository_with(&path, &cfg.env()).await.unwrap_err();
        assert!(err.contains("commit"), "{err}");
        assert!(!path.join(".git").exists());
        assert!(!path.join(".gitignore").exists());
        assert_eq!(
            std::fs::read_to_string(path.join("keep.txt")).unwrap(),
            "mine"
        );
    }

    #[tokio::test]
    async fn rollback_keeps_a_gitignore_the_user_had() {
        let hooks = tempfile::tempdir().unwrap();
        let hook = hooks.path().join("pre-commit");
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let cfg = config(&format!(
            "{IDENTITY}[core]\n\thooksPath = {}\n",
            hooks.path().display()
        ));
        let (_d, path) = folder();
        std::fs::write(path.join(".gitignore"), "mine\n").unwrap();
        init_repository_with(&path, &cfg.env()).await.unwrap_err();
        assert_eq!(
            std::fs::read_to_string(path.join(".gitignore")).unwrap(),
            "mine\n"
        );
        assert!(!path.join(".git").exists());
    }

    /// What a Host stopped after `git init` and the basic `.gitignore`, before
    /// the commit, leaves behind.
    fn interrupted(path: &Path, cfg: &Config, wrote_gitignore: bool) {
        std::fs::create_dir(path.join(".git")).unwrap();
        let marker = if wrote_gitignore { "gitignore\n" } else { "" };
        std::fs::write(path.join(".git").join(IN_PROGRESS), marker).unwrap();
        git(path, cfg, &["init", "-q", "-b", "main"]);
        if wrote_gitignore {
            std::fs::write(path.join(".gitignore"), BASIC_GITIGNORE).unwrap();
        }
    }

    #[tokio::test]
    async fn a_finished_init_leaves_no_marker() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        init_repository_with(&path, &cfg.env()).await.unwrap();
        assert!(!path.join(".git").join(IN_PROGRESS).exists());
        assert!(!recover_interrupted(&path).await);
        assert_eq!(git(&path, &cfg, &["log", "--format=%s"]), "Initial Commit");
    }

    #[tokio::test]
    async fn an_interrupted_init_is_undone_then_redone() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        std::fs::write(path.join("a.txt"), "mine").unwrap();
        interrupted(&path, &cfg, true);
        assert!(recover_interrupted(&path).await);
        assert!(!path.join(".git").exists());
        assert!(!path.join(".gitignore").exists());
        assert_eq!(std::fs::read_to_string(path.join("a.txt")).unwrap(), "mine");
        init_repository_with(&path, &cfg.env()).await.unwrap();
        assert_eq!(committed(&path, &cfg), [".gitignore", "a.txt"]);
    }

    #[tokio::test]
    async fn undoing_an_interrupted_init_keeps_the_users_gitignore() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        std::fs::write(path.join(".gitignore"), "mine\n").unwrap();
        interrupted(&path, &cfg, false);
        assert!(recover_interrupted(&path).await);
        assert!(!path.join(".git").exists());
        assert_eq!(
            std::fs::read_to_string(path.join(".gitignore")).unwrap(),
            "mine\n"
        );
    }

    #[tokio::test]
    async fn an_empty_git_directory_is_cleared() {
        let (_d, path) = folder();
        std::fs::create_dir(path.join(".git")).unwrap();
        assert!(recover_interrupted(&path).await);
        assert!(!has_git(&path));
    }

    #[tokio::test]
    async fn the_users_repository_without_a_commit_is_not_undone() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        git(&path, &cfg, &["init", "-q"]);
        std::fs::write(path.join(".gitignore"), "x\n").unwrap();
        assert!(!recover_interrupted(&path).await);
        assert!(path.join(".git/HEAD").exists());
        assert!(path.join(".gitignore").exists());
    }

    /// The bytes of `.git/<rel>`, `None` when absent.
    fn git_file(path: &Path, rel: &str) -> Option<Vec<u8>> {
        std::fs::read(path.join(".git").join(rel)).ok()
    }

    #[tokio::test]
    async fn is_unborn_only_for_a_repository_without_a_commit() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        assert!(!is_unborn(&path, &cfg.env()).await);
        git(&path, &cfg, &["init", "-q"]);
        assert!(is_unborn(&path, &cfg.env()).await);
        git(&path, &cfg, &["commit", "-q", "--allow-empty", "-m", "c"]);
        assert!(!is_unborn(&path, &cfg.env()).await);
    }

    #[tokio::test]
    async fn an_orphan_branch_in_a_repository_with_history_is_not_unborn() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        git(&path, &cfg, &["init", "-q", "-b", "main"]);
        git(&path, &cfg, &["commit", "-q", "--allow-empty", "-m", "c"]);
        git(&path, &cfg, &["checkout", "-q", "--orphan", "gh-pages"]);
        assert!(!is_unborn(&path, &cfg.env()).await);
    }

    #[tokio::test]
    async fn commit_unborn_commits_on_the_head_branch() {
        let cfg = config("");
        let (_d, path) = folder();
        git(&path, &cfg, &["init", "-q", "-b", "trunk"]);
        std::fs::write(path.join("f"), "x").unwrap();
        commit_unborn(&path, &cfg.env(), None).await.unwrap();
        assert_eq!(git(&path, &cfg, &["branch", "--show-current"]), "trunk");
        assert_eq!(git(&path, &cfg, &["log", "--format=%s"]), "Initial Commit");
        assert_eq!(
            git(&path, &cfg, &["log", "--format=%an <%ae>"]),
            "Zwiim <zwiim@localhost>"
        );
        assert_eq!(committed(&path, &cfg), [".gitignore", "f"]);
        assert_eq!(
            std::fs::read_to_string(path.join(".gitignore")).unwrap(),
            BASIC_GITIGNORE
        );
    }

    #[tokio::test]
    async fn commit_unborn_with_identity_sets_it_and_excludes_secrets() {
        let cfg = config("");
        let (_d, path) = folder();
        git(&path, &cfg, &["init", "-q", "-b", "main"]);
        std::fs::write(path.join(".gitignore"), "target\n").unwrap();
        for f in [".env", "key.pem", "a.txt"] {
            std::fs::write(path.join(f), "x").unwrap();
        }
        let u = commit_unborn(&path, &cfg.env(), Some(&octo()))
            .await
            .unwrap();
        assert!(u.set_identity);
        assert!(!u.wrote_gitignore);
        assert_eq!(committed(&path, &cfg), [".gitignore", "a.txt"]);
        assert_eq!(
            git(&path, &cfg, &["config", "--local", "user.email"]),
            "42+octo@users.noreply.github.com"
        );
        assert_eq!(
            git(&path, &cfg, &["log", "--format=%an <%ae>"]),
            "Octo Cat <42+octo@users.noreply.github.com>"
        );
    }

    /// Everything `undo_first_commit` must put back, as it is now.
    fn unborn_state(path: &Path, cfg: &Config) -> Vec<Option<Vec<u8>>> {
        // Status first: it may refresh the index's stat data.
        vec![
            Some(git(path, cfg, &["status", "--porcelain"]).into_bytes()),
            git_file(path, "index"),
            git_file(path, "info/exclude"),
            git_file(path, "logs/HEAD"),
            git_file(path, "zwiim-sentinel"),
            std::fs::read(path.join(".gitignore")).ok(),
            std::fs::read(path.join("a.txt")).ok(),
        ]
    }

    #[tokio::test]
    async fn undo_first_commit_restores_everything() {
        for users_gitignore in [false, true] {
            let cfg = config("");
            let (_d, path) = folder();
            git(&path, &cfg, &["init", "-q", "-b", "trunk"]);
            if users_gitignore {
                // An orphan branch after history: `logs/HEAD` exists and
                // the other branch must survive the undo.
                git(
                    &path,
                    &cfg,
                    &[
                        "-c",
                        "user.name=u",
                        "-c",
                        "user.email=u@example.com",
                        "commit",
                        "-q",
                        "--allow-empty",
                        "-m",
                        "old",
                    ],
                );
                git(&path, &cfg, &["checkout", "-q", "--orphan", "fresh"]);
                std::fs::write(path.join(".gitignore"), "mine\n").unwrap();
            }
            std::fs::write(path.join(".git/info/exclude"), "users-own\n").unwrap();
            std::fs::write(path.join(".git/zwiim-sentinel"), "keep").unwrap();
            std::fs::write(path.join("a.txt"), "staged").unwrap();
            git(&path, &cfg, &["add", "a.txt"]);
            // Unstaged on top of the staged version.
            std::fs::write(path.join("a.txt"), "edited").unwrap();
            std::fs::write(path.join(".env"), "KEY=1\n").unwrap();
            let before = unborn_state(&path, &cfg);
            assert_eq!(before[3].is_some(), users_gitignore);

            let u = commit_unborn(&path, &cfg.env(), Some(&octo()))
                .await
                .unwrap();
            assert!(!git_fails(
                &path,
                &cfg,
                &["rev-parse", "-q", "--verify", "HEAD"]
            ));
            undo_first_commit(&path, u).await;

            assert!(git_fails(
                &path,
                &cfg,
                &["rev-parse", "-q", "--verify", "HEAD"]
            ));
            assert_eq!(
                git(&path, &cfg, &["diff", "--cached", "--name-only"]),
                "a.txt"
            );
            assert_eq!(unborn_state(&path, &cfg), before, "{users_gitignore}");
            assert_eq!(path.join(".gitignore").exists(), users_gitignore);
            assert!(git_fails(&path, &cfg, &["config", "--local", "user.email"]));
            assert!(git_fails(&path, &cfg, &["config", "--local", "user.name"]));
            let branch = if users_gitignore { "fresh" } else { "trunk" };
            assert!(!path.join(".git/logs/refs/heads").join(branch).exists());
            if users_gitignore {
                assert_eq!(git(&path, &cfg, &["log", "--format=%s", "trunk"]), "old");
            }
            let tmp = file_names_under(&path.join(".git"));
            assert!(!tmp.iter().any(|n| n.ends_with(".zwiim-tmp")), "{tmp:?}");
        }
    }

    /// Every file name under `dir`, at any depth.
    fn file_names_under(dir: &Path) -> Vec<String> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            if entry.file_type().unwrap().is_dir() {
                names.extend(file_names_under(&entry.path()));
            } else {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names
    }

    #[tokio::test]
    async fn undo_first_commit_keeps_a_branch_the_user_committed_on() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        git(&path, &cfg, &["init", "-q", "-b", "trunk"]);
        std::fs::write(path.join("a.txt"), "a").unwrap();
        let u = commit_unborn(&path, &cfg.env(), None).await.unwrap();
        std::fs::write(path.join("b.txt"), "b").unwrap();
        git(&path, &cfg, &["add", "b.txt"]);
        git(&path, &cfg, &["commit", "-q", "-m", "mine"]);
        let index = git_file(&path, "index");
        undo_first_commit(&path, u).await;
        assert_eq!(
            git(&path, &cfg, &["log", "--format=%s", "trunk"]),
            "mine\nInitial Commit"
        );
        assert_eq!(git(&path, &cfg, &["branch", "--show-current"]), "trunk");
        assert_eq!(git_file(&path, "index"), index);
        assert!(path.join(".git/logs/refs/heads/trunk").exists());
        assert!(path.join(".gitignore").exists());
    }

    #[tokio::test]
    async fn undo_first_commit_restores_the_branchs_own_reflog() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        git(&path, &cfg, &["init", "-q", "-b", "trunk"]);
        let log = path.join(".git/logs/refs/heads/trunk");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, "the user's own reflog\n").unwrap();
        let u = commit_unborn(&path, &cfg.env(), None).await.unwrap();
        undo_first_commit(&path, u).await;
        assert!(git_fails(
            &path,
            &cfg,
            &["rev-parse", "-q", "--verify", "HEAD"]
        ));
        assert_eq!(std::fs::read(&log).unwrap(), b"the user's own reflog\n");
    }

    #[tokio::test]
    async fn commit_unborn_killed_mid_commit_removes_its_index_lock() {
        // Stands in for the step timeout: git is killed holding its lock.
        let hooks = tempfile::tempdir().unwrap();
        let hook = hooks.path().join("pre-commit");
        std::fs::write(
            &hook,
            "#!/bin/sh\n: > .git/index.lock\nkill -9 $PPID\nexit 1\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let cfg = config(&format!(
            "{IDENTITY}[core]\n\thooksPath = {}\n",
            hooks.path().display()
        ));
        let (_d, path) = folder();
        git(&path, &cfg, &["init", "-q", "-b", "main"]);
        std::fs::write(path.join("a.txt"), "staged").unwrap();
        git(&path, &cfg, &["add", "a.txt"]);
        let before = unborn_state(&path, &cfg);
        assert!(commit_unborn(&path, &cfg.env(), None).await.is_err());
        assert!(!path.join(".git/index.lock").exists());
        assert!(git_fails(
            &path,
            &cfg,
            &["rev-parse", "-q", "--verify", "HEAD"]
        ));
        assert_eq!(unborn_state(&path, &cfg), before);
    }

    #[tokio::test]
    async fn commit_unborn_refuses_while_the_index_is_locked() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        git(&path, &cfg, &["init", "-q", "-b", "main"]);
        std::fs::write(path.join("a.txt"), "a").unwrap();
        std::fs::write(path.join(".git/index.lock"), "theirs").unwrap();
        let index = git_file(&path, "index");
        assert!(commit_unborn(&path, &cfg.env(), None).await.is_err());
        assert_eq!(
            std::fs::read(path.join(".git/index.lock")).unwrap(),
            b"theirs"
        );
        assert_eq!(git_file(&path, "index"), index);
        assert!(!path.join(".gitignore").exists());
        assert!(git_fails(
            &path,
            &cfg,
            &["rev-parse", "-q", "--verify", "HEAD"]
        ));
    }

    #[tokio::test]
    async fn commit_unborn_failing_hook_restores_state() {
        let hooks = tempfile::tempdir().unwrap();
        let hook = hooks.path().join("pre-commit");
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let cfg = config(&format!(
            "{IDENTITY}[core]\n\thooksPath = {}\n",
            hooks.path().display()
        ));
        let (_d, path) = folder();
        git(&path, &cfg, &["init", "-q", "-b", "main"]);
        std::fs::write(path.join(".git/zwiim-sentinel"), "keep").unwrap();
        std::fs::write(path.join("a.txt"), "staged").unwrap();
        git(&path, &cfg, &["add", "a.txt"]);
        let before = unborn_state(&path, &cfg);
        let err = commit_unborn(&path, &cfg.env(), Some(&octo()))
            .await
            .unwrap_err();
        assert!(err.contains("commit"), "{err}");
        assert!(git_fails(
            &path,
            &cfg,
            &["rev-parse", "-q", "--verify", "HEAD"]
        ));
        assert_eq!(unborn_state(&path, &cfg), before);
        assert!(!path.join(".gitignore").exists());
        assert!(git_fails(&path, &cfg, &["config", "--local", "user.email"]));
    }

    #[tokio::test]
    async fn commit_unborn_refuses_detached() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        git(&path, &cfg, &["init", "-q", "-b", "main"]);
        git(&path, &cfg, &["commit", "-q", "--allow-empty", "-m", "c"]);
        git(&path, &cfg, &["checkout", "-q", "--detach"]);
        std::fs::write(path.join("a.txt"), "a").unwrap();
        let head = git(&path, &cfg, &["rev-parse", "HEAD"]);
        let before = unborn_state(&path, &cfg);
        assert!(commit_unborn(&path, &cfg.env(), None).await.is_err());
        assert_eq!(git(&path, &cfg, &["rev-parse", "HEAD"]), head);
        assert_eq!(unborn_state(&path, &cfg), before);
        assert!(!path.join(".gitignore").exists());
    }

    #[tokio::test]
    async fn commit_unborn_refuses_a_branch_with_a_commit() {
        // Its undo would delete the user's branch.
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        git(&path, &cfg, &["init", "-q", "-b", "main"]);
        git(&path, &cfg, &["commit", "-q", "--allow-empty", "-m", "c"]);
        std::fs::write(path.join("a.txt"), "a").unwrap();
        let before = unborn_state(&path, &cfg);
        assert!(commit_unborn(&path, &cfg.env(), None).await.is_err());
        assert_eq!(git(&path, &cfg, &["log", "--format=%s"]), "c");
        assert_eq!(unborn_state(&path, &cfg), before);
    }

    #[tokio::test]
    async fn commit_unborn_refuses_a_linked_worktree() {
        let cfg = config(IDENTITY);
        let (d, path) = folder();
        git(&path, &cfg, &["init", "-q", "-b", "main"]);
        git(&path, &cfg, &["commit", "-q", "--allow-empty", "-m", "c"]);
        let linked = d.path().join("linked");
        git(
            &path,
            &cfg,
            &[
                "worktree",
                "add",
                "-q",
                "--orphan",
                "-b",
                "o",
                linked.to_str().unwrap(),
            ],
        );
        assert!(commit_unborn(&linked, &cfg.env(), None).await.is_err());
        assert!(git_fails(
            &linked,
            &cfg,
            &["rev-parse", "-q", "--verify", "HEAD"]
        ));
    }

    #[tokio::test]
    async fn made_undoes_what_it_made() {
        let cfg = config(IDENTITY);
        let (_d, path) = folder();
        std::fs::write(path.join("a.txt"), "a").unwrap();
        let created = init_repository_with(&path, &cfg.env())
            .await
            .unwrap()
            .unwrap();
        Made::Repository(created).undo(&path).await;
        assert!(!has_git(&path));
        assert!(!path.join(".gitignore").exists());

        git(&path, &cfg, &["init", "-q", "-b", "main"]);
        let u = commit_unborn(&path, &cfg.env(), None).await.unwrap();
        Made::FirstCommit(u).undo(&path).await;
        assert!(has_git(&path));
        assert!(is_unborn(&path, &cfg.env()).await);
        assert!(!path.join(".gitignore").exists());
        assert_eq!(std::fs::read_to_string(path.join("a.txt")).unwrap(), "a");
    }

    fn files(dir: &Path, n: u64) {
        std::fs::create_dir_all(dir).unwrap();
        for i in 0..n {
            std::fs::write(dir.join(format!("f{i}")), "").unwrap();
        }
    }

    #[test]
    fn a_small_folder_is_counted_exactly() {
        let (_d, path) = folder();
        std::fs::write(path.join("a"), "12345").unwrap();
        std::fs::write(path.join(".hidden"), "1").unwrap();
        std::fs::write(path.join(".env"), "secret").unwrap();
        let size = measure_first_commit(&path);
        assert_eq!(
            size,
            FirstCommitSize {
                files: 2,
                bytes: 6,
                at_least: false
            }
        );
        assert!(!size.is_large());
    }

    #[test]
    fn over_the_file_limit_stops_counting_early() {
        let (_d, path) = folder();
        files(&path.join("src"), LARGE_FOLDER_FILES + 500);
        let size = measure_first_commit(&path);
        assert!(size.is_large() && size.at_least);
        assert_eq!(size.files, LARGE_FOLDER_FILES + 1);
    }

    #[test]
    fn over_the_byte_limit_is_large() {
        let (_d, path) = folder();
        // Sparse: no real disk used.
        let f = std::fs::File::create(path.join("big.bin")).unwrap();
        f.set_len(LARGE_FOLDER_BYTES + 1).unwrap();
        let size = measure_first_commit(&path);
        assert!(size.is_large() && size.at_least);
        assert_eq!(size.files, 1);
    }

    #[test]
    fn ignored_dependencies_do_not_count() {
        let (_d, path) = folder();
        files(&path.join("node_modules/pkg"), LARGE_FOLDER_FILES + 1);
        std::fs::write(path.join("index.js"), "x").unwrap();
        let size = measure_first_commit(&path);
        assert!(!size.is_large());
        assert_eq!(size.files, 1);
    }

    #[test]
    fn the_folders_own_gitignore_replaces_the_basic_one() {
        let (_d, path) = folder();
        std::fs::write(path.join(".gitignore"), "gen/\n").unwrap();
        files(&path.join("gen"), 3);
        files(&path.join("node_modules"), 2);
        // `.gitignore` + node_modules' 2; gen/ ignored by their rule.
        assert_eq!(measure_first_commit(&path).files, 3);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_counts_as_one_file_by_its_own_length() {
        let (_d, path) = folder();
        std::fs::write(path.join("a"), "x").unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("big.bin");
        let f = std::fs::File::create(&target).unwrap();
        f.set_len(LARGE_FOLDER_BYTES + 1).unwrap();
        std::os::unix::fs::symlink(&target, path.join("link")).unwrap();
        let size = measure_first_commit(&path);
        assert_eq!(size.files, 2);
        // The symlink's bytes are its own (short) length, not the big
        // target's: Git stores the link text, never the pointed-to file.
        assert!(size.bytes < LARGE_FOLDER_BYTES);
    }

    #[test]
    fn a_nested_repository_counts_as_nothing() {
        let (_d, path) = folder();
        let nested = path.join("vendor/lib");
        files(&nested, LARGE_FOLDER_FILES + 1);
        std::fs::create_dir(nested.join(".git")).unwrap();
        std::fs::write(path.join("main.rs"), "x").unwrap();
        let size = measure_first_commit(&path);
        assert!(!size.is_large());
        assert_eq!(size.files, 1);
    }
}
