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
    for key in ["user.name", "user.email"] {
        match git(path, &[], &["config", "--local", "--get", key]).await {
            Ok(value) if !value.trim().is_empty() => return Ok(false),
            _ => {}
        }
    }
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
    Ok(true)
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

/// How long one git step may run. The user's Git config applies, so a
/// signing prompt or a hook could otherwise hang the first commit forever.
const GIT_STEP_TIMEOUT: Duration = Duration::from_secs(10 * 60);

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
