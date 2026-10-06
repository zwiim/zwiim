//! Zwiim-owned workspace facade over the Folder engine.
//!
//! A workspace is a user-facing Folder row — a root project or a managed git
//! worktree — addressed on the client boundary only by its opaque
//! `workspace_id` (minted in `folder::ActiveModel::before_save`). Chat scratch
//! folders have no `workspace_id` and never appear here. `folder.id` never
//! leaves this module's callers.
//!
//! A removed worktree keeps its row (soft-deleted) and its sessions: they stay
//! listable and readable under the same `workspace_id`, but nothing may run in
//! it again — resolving it for work fails with `workspace_removed`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use serde::Serialize;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

use crate::acp::manager::ConnectionManager;
use crate::app_error::{AppCommandError, AppErrorCode};
use crate::commands::folders;
use crate::db::entities::conversation;
use crate::db::entities::folder::{self, FolderKind};
use crate::db::error::DbError;
use crate::db::AppDatabase;
use crate::git_harness::{self, GitCaller, GitError, GitErrorCode, GitHost, GitOp, PrLifecycle};
use crate::protocol::error::ZwiimError;
use crate::terminal::manager::TerminalManager;
use crate::web::event_bridge::EventEmitter;

mod names;
mod repo_init;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceKind {
    Root,
    Worktree,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct WorkspaceInfo {
    pub workspace_id: String,
    pub name: String,
    pub alias: Option<String>,
    /// Absolute host path. Host-internal: never serialized to clients, who
    /// address the workspace by `workspace_id` and use relative paths.
    #[serde(skip_serializing)]
    pub path: String,
    pub kind: WorkspaceKind,
    /// The root workspace a worktree was created from; `None` for roots.
    pub parent_workspace_id: Option<String>,
    /// Git sees a repository here now (`.git` at the workspace's own
    /// folder); `false` for a plain folder, which is still a full workspace,
    /// and once removed. Only a repository has worktrees and branches.
    pub is_git_repo: bool,
    /// Branch checked out now; `None` when detached, not a repo, or removed.
    pub branch: Option<String>,
    /// For a worktree: the root's branch it was created from.
    pub base_branch: Option<String>,
    /// For a worktree: the commit its branch started at.
    pub base_commit: Option<String>,
    /// A removed worktree: its sessions are readable, nothing runs in it.
    pub removed: bool,
    /// Roots only: `~` or `~/Projects/zwiim` when inside the host user's
    /// home, else `None`. Presentation only; never a host path.
    pub display_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct WorktreeRemoved {
    pub workspace_id: String,
    pub parent_workspace_id: String,
    /// Sessions that stay attached to the removed workspace, readable only.
    pub kept_sessions: u32,
}

fn not_found(workspace_id: &str) -> AppCommandError {
    AppCommandError::not_found("Workspace not found").with_detail(workspace_id.to_string())
}

fn removed(workspace_id: &str) -> AppCommandError {
    AppCommandError::new(AppErrorCode::WorkspaceRemoved, "The workspace was removed")
        .with_detail(workspace_id.to_string())
}

fn db_err(e: sea_orm::DbErr) -> AppCommandError {
    AppCommandError::from(DbError::from(e))
}

/// Whether soft-deleted Folder `row` is a removed worktree workspace (as
/// opposed to a root the legacy surface hid, which simply stops resolving).
fn is_removed_worktree(row: &folder::Model) -> bool {
    row.deleted_at.is_some() && row.parent_id.is_some()
}

async fn find(
    conn: &DatabaseConnection,
    workspace_id: &str,
) -> Result<Option<folder::Model>, AppCommandError> {
    folder::Entity::find()
        .filter(folder::Column::WorkspaceId.eq(workspace_id))
        .filter(folder::Column::Kind.eq(FolderKind::Regular))
        .one(conn)
        .await
        .map_err(db_err)
}

/// `workspace_id → Folder row` for work in it. Only live user-facing rows
/// resolve; a removed worktree fails with `WorkspaceRemoved`.
pub async fn resolve(
    conn: &DatabaseConnection,
    workspace_id: &str,
) -> Result<folder::Model, AppCommandError> {
    match find(conn, workspace_id).await? {
        Some(row) if row.deleted_at.is_none() => Ok(row),
        Some(row) if is_removed_worktree(&row) => Err(removed(workspace_id)),
        _ => Err(not_found(workspace_id)),
    }
}

/// `workspace_id → Folder row` for reading its sessions: a live workspace or
/// a removed worktree.
pub async fn resolve_readable(
    conn: &DatabaseConnection,
    workspace_id: &str,
) -> Result<folder::Model, AppCommandError> {
    match find(conn, workspace_id).await? {
        Some(row) if row.deleted_at.is_none() || is_removed_worktree(&row) => Ok(row),
        _ => Err(not_found(workspace_id)),
    }
}

/// The `workspace_id` of Folder row `folder_id`, if it is a live user-facing
/// workspace.
pub async fn workspace_id_of(
    conn: &DatabaseConnection,
    folder_id: i32,
) -> Result<Option<String>, AppCommandError> {
    Ok(folder::Entity::find_by_id(folder_id)
        .filter(folder::Column::DeletedAt.is_null())
        .one(conn)
        .await
        .map_err(db_err)?
        .and_then(|f| f.workspace_id))
}

// ─── launch / removal gate ───────────────────────────────────────────────

/// Per-workspace gate that makes worktree removal atomic with respect to
/// starting work there: agent launches and terminal spawns hold it shared
/// until the new process is registered; removal holds it exclusively from its
/// running-work check through the soft-delete. Keyed by Folder row id.
static GATES: LazyLock<std::sync::Mutex<HashMap<i32, Arc<RwLock<()>>>>> =
    LazyLock::new(Default::default);

fn gate(folder_id: i32) -> Arc<RwLock<()>> {
    let mut gates = GATES.lock().unwrap_or_else(|e| e.into_inner());
    // Drop gates nobody holds or waits on.
    gates.retain(|_, gate| Arc::strong_count(gate) > 1);
    gates.entry(folder_id).or_default().clone()
}

/// Held while an agent or terminal is being started in a workspace.
pub type LaunchGuard = OwnedRwLockReadGuard<()>;

/// Enter Folder row `folder_id` to start an agent or terminal there. Returns
/// the row as it is once no removal can run concurrently; a removed worktree
/// fails with `WorkspaceRemoved`, so nothing is ever started in (or instead
/// of) a removed checkout. Hold the guard until the process is registered.
pub(crate) async fn begin_launch(
    conn: &DatabaseConnection,
    folder_id: i32,
) -> Result<(folder::Model, LaunchGuard), AppCommandError> {
    let guard = gate(folder_id).read_owned().await;
    let row = folder::Entity::find_by_id(folder_id)
        .one(conn)
        .await
        .map_err(db_err)?
        .ok_or_else(|| AppCommandError::not_found("Workspace not found"))?;
    if is_removed_worktree(&row) {
        return Err(removed(row.workspace_id.as_deref().unwrap_or_default()));
    }
    Ok((row, guard))
}

/// Held while a workspace's checkout itself changes under its agents.
pub(crate) type ExclusiveGuard = OwnedRwLockWriteGuard<()>;

/// Hold Folder row `folder_id` exclusively: no agent launch, prompt send or
/// terminal spawn starts there until the guard drops.
pub(crate) async fn enter_exclusive(folder_id: i32) -> ExclusiveGuard {
    gate(folder_id).write_owned().await
}

/// The agents working in `row`'s workspace (a turn runs or a request waits on
/// the user), leaving out connection `except`: how many, and the sessions they
/// belong to. An agent not bound to a session yet is only counted.
pub(crate) async fn busy_agents(
    conn: &DatabaseConnection,
    connections: &ConnectionManager,
    row: &folder::Model,
    except: Option<&str>,
) -> Result<(usize, Vec<String>), AppCommandError> {
    let path = PathBuf::from(&row.path);
    let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let spellings = [path.as_path(), canonical.as_path()];
    let busy = connections.busy_in_folder(row.id, &spellings, except).await;
    session_ids_of(conn, &busy)
        .await
        .map(|ids| (busy.len(), ids))
}

/// The session ids of the bound conversations among `agents`.
async fn session_ids_of(
    conn: &DatabaseConnection,
    agents: &[Option<i32>],
) -> Result<Vec<String>, AppCommandError> {
    let bound: Vec<i32> = agents.iter().flatten().copied().collect();
    Ok(conversation::Entity::find()
        .filter(conversation::Column::Id.is_in(bound))
        .filter(conversation::Column::SessionId.is_not_null())
        .all(conn)
        .await
        .map_err(db_err)?
        .into_iter()
        .filter_map(|c| c.session_id)
        .collect())
}

/// [`resolve`] + [`begin_launch`] for a client-addressed workspace.
pub(crate) async fn resolve_for_launch(
    conn: &DatabaseConnection,
    workspace_id: &str,
) -> Result<(folder::Model, LaunchGuard), AppCommandError> {
    let row = resolve(conn, workspace_id).await?;
    begin_launch(conn, row.id).await
}

// ─── projection ──────────────────────────────────────────────────────────

/// Whether the folder is a repository now, and its branch. A repository is
/// the folder holding `.git` itself (a file in a linked worktree), the same
/// test `create_worktree` applies: a plain folder inside some other
/// repository is not one.
async fn live_git(row: &folder::Model) -> (bool, Option<String>) {
    if row.deleted_at.is_some() {
        return (false, None);
    }
    let dot_git = Path::new(&row.path).join(".git");
    if tokio::fs::symlink_metadata(dot_git).await.is_err() {
        return (false, None);
    }
    let branch = folders::resolve_git_head(&row.path)
        .await
        .ok()
        .and_then(|head| head.branch);
    (true, branch)
}

pub(crate) async fn info(
    conn: &DatabaseConnection,
    row: folder::Model,
) -> Result<WorkspaceInfo, AppCommandError> {
    let parent_workspace_id = match row.parent_id {
        Some(parent_id) => workspace_id_of(conn, parent_id).await?,
        None => None,
    };
    let git = live_git(&row).await;
    Ok(to_info(row, parent_workspace_id, git))
}

/// The host user's home directory, canonical; `None` when there is none.
pub(crate) fn home_dir() -> Option<&'static Path> {
    static HOME: LazyLock<Option<PathBuf>> =
        LazyLock::new(|| dirs::home_dir().and_then(|h| h.canonicalize().ok()));
    HOME.as_deref()
}

/// `~` or `~/a/b` for a path inside `home`, else `None`. Presentation only.
fn display_path(path: &str, home: Option<&Path>) -> Option<String> {
    let rel = Path::new(path).strip_prefix(home?).ok()?;
    let rel = rel.to_string_lossy();
    Some(if rel.is_empty() {
        "~".to_string()
    } else {
        format!("~/{rel}")
    })
}

fn to_info(
    row: folder::Model,
    parent_workspace_id: Option<String>,
    (is_git_repo, branch): (bool, Option<String>),
) -> WorkspaceInfo {
    let display_path = match row.parent_id {
        None => display_path(&row.path, home_dir()),
        Some(_) => None,
    };
    WorkspaceInfo {
        display_path,
        removed: is_removed_worktree(&row),
        workspace_id: row.workspace_id.unwrap_or_default(),
        name: row.name,
        alias: row.alias,
        path: row.path,
        kind: if row.parent_id.is_some() {
            WorkspaceKind::Worktree
        } else {
            WorkspaceKind::Root
        },
        parent_workspace_id,
        is_git_repo,
        branch,
        base_branch: row.base_branch,
        base_commit: row.base_commit,
    }
}

/// The live workspace a directory belongs to: the one whose root is `dir` or,
/// failing that, the nearest one containing it.
pub fn workspace_for_dir(workspaces: &[WorkspaceInfo], dir: &str) -> Option<String> {
    let dir = std::path::Path::new(dir);
    workspaces
        .iter()
        .filter(|w| !w.removed && dir.starts_with(&w.path))
        .max_by_key(|w| w.path.len())
        .map(|w| w.workspace_id.clone())
}

/// Every user-facing workspace the host knows: the live ones, plus removed
/// worktrees of a live root that still hold sessions to read.
pub async fn list(conn: &DatabaseConnection) -> Result<Vec<WorkspaceInfo>, AppCommandError> {
    let live = folder::Entity::find()
        .filter(folder::Column::Kind.eq(FolderKind::Regular))
        .filter(folder::Column::DeletedAt.is_null())
        .filter(folder::Column::WorkspaceId.is_not_null())
        .order_by_asc(folder::Column::Id)
        .all(conn)
        .await
        .map_err(db_err)?;
    let ids: HashMap<i32, String> = live
        .iter()
        .filter_map(|r| r.workspace_id.clone().map(|w| (r.id, w)))
        .collect();

    let removed_rows = folder::Entity::find()
        .filter(folder::Column::Kind.eq(FolderKind::Regular))
        .filter(folder::Column::DeletedAt.is_not_null())
        .filter(folder::Column::ParentId.is_in(ids.keys().copied()))
        .filter(folder::Column::WorkspaceId.is_not_null())
        .order_by_asc(folder::Column::Id)
        .all(conn)
        .await
        .map_err(db_err)?;
    let with_sessions: HashSet<i32> = if removed_rows.is_empty() {
        HashSet::new()
    } else {
        conversation::Entity::find()
            .select_only()
            .column(conversation::Column::FolderId)
            .filter(conversation::Column::FolderId.is_in(removed_rows.iter().map(|r| r.id)))
            .filter(conversation::Column::SessionId.is_not_null())
            .filter(conversation::Column::DeletedAt.is_null())
            .into_tuple::<i32>()
            .all(conn)
            .await
            .map_err(db_err)?
            .into_iter()
            .collect()
    };

    let mut rows = live;
    rows.extend(
        removed_rows
            .into_iter()
            .filter(|r| with_sessions.contains(&r.id)),
    );
    rows.sort_by_key(|r| r.id);
    let gits = futures::future::join_all(rows.iter().map(live_git)).await;
    Ok(rows
        .into_iter()
        .zip(gits)
        .map(|(r, git)| {
            let parent = r.parent_id.and_then(|p| ids.get(&p).cloned());
            to_info(r, parent, git)
        })
        .collect())
}

/// Register the host directory `path` as a workspace through the existing
/// open-folder path. Re-adding a known (or previously removed) directory
/// revives its row, which keeps its original `workspace_id`. A folder with no
/// `.git` is first made a repository with one "Initial Commit"; if that commit
/// would be large it is refused with `large_folder` unless `allow_large`.
pub async fn add(
    db: &AppDatabase,
    emitter: &EventEmitter,
    path: &str,
    allow_large: bool,
) -> Result<WorkspaceInfo, ZwiimError> {
    let requested = Path::new(path);
    if !requested.is_absolute() {
        return Err(AppCommandError::invalid_input("Path must be absolute").into());
    }
    let canonical = requested
        .canonicalize()
        .ok()
        .filter(|p| p.is_dir())
        .ok_or_else(|| AppCommandError::not_found("Directory not found"))?;
    let canonical_str = canonical.to_string_lossy().into_owned();
    // A host-internal scratch folder has no workspace identity: refuse it
    // before touching it.
    let known = folder::Entity::find()
        .filter(folder::Column::Path.eq(canonical_str.as_str()))
        .one(&db.conn)
        .await
        .map_err(db_err)?;
    if known.is_some_and(|r| r.kind != FolderKind::Regular) {
        return Err(not_a_workspace().into());
    }

    // Spawned so that it completes, or rolls back, even if the request is
    // dropped mid-way.
    let conn = db.conn.clone();
    let emitter = emitter.clone();
    tokio::spawn(async move {
        let db = AppDatabase { conn };
        init_and_register(&db, &emitter, canonical, allow_large).await
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "[workspace] add: the repository task did not finish");
        ZwiimError::internal()
    })?
}

/// Serializes making repositories Host-wide, so two callers never both see
/// a folder without `.git` and one rolls back the other's repository.
static REPO_INIT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Held by anything that may make a folder a repository (`add`, the startup
/// sweep) from its `has_git` check through registration or rollback.
pub(crate) async fn repo_init_lock() -> tokio::sync::MutexGuard<'static, ()> {
    REPO_INIT.lock().await
}

/// Under the repository-init lock: make `canonical` a repository unless it
/// already has `.git` (refusing a large first commit without `allow_large`;
/// an init the Host stopped in is undone and redone), then register it,
/// undoing the init if registering fails.
async fn init_and_register(
    db: &AppDatabase,
    emitter: &EventEmitter,
    canonical: PathBuf,
    allow_large: bool,
) -> Result<WorkspaceInfo, ZwiimError> {
    let _init = repo_init_lock().await;
    if repo_init::recover_interrupted(&canonical).await {
        tracing::info!("[workspace] add: undid an unfinished repository init");
    }
    let created = if repo_init::has_git(&canonical) {
        None
    } else {
        let dir = canonical.clone();
        let size = tokio::task::spawn_blocking(move || repo_init::measure_first_commit(&dir))
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "[workspace] add: measuring did not finish");
                ZwiimError::internal()
            })?;
        if size.is_large() && !allow_large {
            return Err(ZwiimError::large_folder(
                size.files,
                size.bytes,
                size.at_least,
            ));
        }
        repo_init::init_repository(&canonical).await.map_err(|e| {
            tracing::warn!("[workspace] making a Git repository failed: {e}");
            ZwiimError::repository_init_failed()
        })?
    };

    match register(db, canonical.to_string_lossy().into_owned()).await {
        Ok((detail, row)) => {
            folders::emit_folder_upsert(emitter, detail);
            Ok(info(&db.conn, row).await?)
        }
        Err(e) => {
            if let Some(created) = created {
                repo_init::rollback(&canonical, created).await;
            }
            Err(e.into())
        }
    }
}

fn not_a_workspace() -> AppCommandError {
    AppCommandError::invalid_input("Directory cannot be used as a workspace")
}

/// Register (or revive) the folder row; only a `Regular` row with a
/// `workspace_id` is a workspace.
async fn register(
    db: &AppDatabase,
    path: String,
) -> Result<(crate::models::FolderDetail, folder::Model), AppCommandError> {
    let detail = folders::open_folder_core(db, path).await?;
    let row = folder::Entity::find_by_id(detail.id)
        .one(&db.conn)
        .await
        .map_err(db_err)?
        .ok_or_else(|| AppCommandError::not_found("Workspace not found"))?;
    if row.kind != FolderKind::Regular || row.workspace_id.is_none() {
        return Err(not_a_workspace());
    }
    Ok((detail, row))
}

/// Host start: make every live root that is still a plain folder a
/// repository, so a worktree can be made from it. A folder whose first commit
/// would be large is skipped (nobody is there to confirm); a failure is logged
/// and the sweep moves on. An init the Host stopped in is undone first. Clients learn of each through the folder upsert.
/// The repository-init lock is held per folder, so a concurrent `add` never
/// races the sweep on one folder nor waits for the whole sweep.
pub async fn init_plain_roots(db: &AppDatabase, emitter: &EventEmitter) {
    let rows = match folder::Entity::find()
        .filter(folder::Column::Kind.eq(FolderKind::Regular))
        .filter(folder::Column::DeletedAt.is_null())
        .filter(folder::Column::ParentId.is_null())
        .filter(folder::Column::WorkspaceId.is_not_null())
        .all(&db.conn)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("[workspace] listing roots to make repositories failed: {e}");
            return;
        }
    };
    for row in rows {
        let path = PathBuf::from(&row.path);
        {
            let _init = repo_init_lock().await;
            if repo_init::recover_interrupted(&path).await {
                tracing::info!(
                    "[workspace] host start: undid an unfinished repository init in {}",
                    row.path
                );
            }
            if !path.is_dir() || repo_init::has_git(&path) || !is_live_root(db, row.id).await {
                continue;
            }
            let dir = path.clone();
            let size =
                match tokio::task::spawn_blocking(move || repo_init::measure_first_commit(&dir))
                    .await
                {
                    Ok(size) => size,
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "[workspace] host start: measuring {} did not finish",
                            row.path
                        );
                        continue;
                    }
                };
            if size.is_large() {
                tracing::info!(
                    "[workspace] {} stays a plain folder: its first commit is large ({} files, {} bytes{})",
                    row.path,
                    size.files,
                    size.bytes,
                    if size.at_least { " or more" } else { "" }
                );
                continue;
            }
            if let Err(e) = repo_init::init_repository(&path).await {
                tracing::warn!("[workspace] making {} a Git repository failed: {e}", row.path);
                continue;
            }
        }
        match folders::get_folder_core(db, row.id).await {
            Ok(detail) => folders::emit_folder_upsert(emitter, detail),
            Err(e) => tracing::warn!("[workspace] announcing {} failed: {e}", row.path),
        }
    }
}

/// Row `id` is still a live, `Regular` root workspace. The sweep lists its
/// rows once, so it re-reads each under the repository-init lock: a root
/// removed meanwhile is not made a repository.
async fn is_live_root(db: &AppDatabase, id: i32) -> bool {
    match folder::Entity::find_by_id(id).one(&db.conn).await {
        Ok(row) => row.is_some_and(|r| {
            r.deleted_at.is_none()
                && r.kind == FolderKind::Regular
                && r.parent_id.is_none()
                && r.workspace_id.is_some()
        }),
        Err(e) => {
            tracing::warn!("[workspace] re-reading folder {id} failed: {e}");
            false
        }
    }
}

// ─── worktrees ───────────────────────────────────────────────────────────

/// The commit `HEAD` names in `repo`.
async fn head_commit(repo: &str) -> Result<String, AppCommandError> {
    let out = crate::process::tokio_command("git")
        .args(["rev-parse", "--verify", "HEAD^{commit}"])
        .current_dir(repo)
        .output()
        .await
        .map_err(AppCommandError::io)?;
    if !out.status.success() {
        return Err(AppCommandError::invalid_input(
            "The root workspace has no commits yet",
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The commit of branch `base` — a local branch, else a remote-tracking one
/// (`origin/main`). Only branch refs resolve: never a revision expression.
async fn branch_commit(repo: &str, base: &str) -> Result<String, AppCommandError> {
    for reference in [format!("refs/heads/{base}"), format!("refs/remotes/{base}")] {
        let out = crate::process::tokio_command("git")
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{reference}^{{commit}}"),
            ])
            .current_dir(repo)
            .output()
            .await
            .map_err(AppCommandError::io)?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
        }
    }
    Err(
        AppCommandError::invalid_input("No such branch to start the worktree from")
            .with_detail(base.to_string()),
    )
}

/// How long a new worktree waits for its base branch to be brought up to date.
const BASE_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Bring remote-tracking branch `base` (`origin/main`) up to date before a
/// worktree starts from it, so the worktree starts from the remote's latest.
/// Best effort: offline, a refused sign-in or a slow remote leaves the local
/// copy as it was. A local branch is left alone.
async fn refresh_remote_base(repo: &str, base: &str) {
    let Some((remote, branch)) = base.split_once('/') else {
        return;
    };
    if remote.starts_with('-')
        || branch_exists(repo, &format!("refs/heads/{base}")).await
        || !branch_exists(repo, &format!("refs/remotes/{base}")).await
    {
        return;
    }
    let mut cmd = crate::process::tokio_command("git");
    cmd.args([
        "fetch",
        "--quiet",
        "--no-tags",
        remote,
        &format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}"),
    ])
    .current_dir(repo)
    .env("GIT_TERMINAL_PROMPT", "0")
    .stdin(std::process::Stdio::null())
    .kill_on_drop(true);
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    // Only the outcome is logged: git's message can carry the remote URL.
    match tokio::time::timeout(BASE_FETCH_TIMEOUT, cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => {}
        Ok(Ok(out)) => tracing::warn!(
            "[worktree] fetching {base} failed ({}); starting from the local copy",
            out.status
        ),
        Ok(Err(e)) => tracing::warn!("[worktree] fetching {base} failed ({e}); starting from the local copy"),
        Err(_) => tracing::warn!("[worktree] fetching {base} timed out; starting from the local copy"),
    }
}

async fn branch_exists(repo: &str, reference: &str) -> bool {
    crate::process::tokio_command("git")
        .args(["rev-parse", "--verify", "--quiet", &format!("{reference}^{{commit}}")])
        .current_dir(repo)
        .output()
        .await
        .is_ok_and(|o| o.status.success())
}

/// Whether `repo` has an `origin` remote.
async fn has_origin(repo: &str) -> bool {
    crate::process::tokio_command("git")
        .args(["config", "--get", "remote.origin.url"])
        .current_dir(repo)
        .output()
        .await
        .is_ok_and(|o| o.status.success())
}

/// Whether `dir` is a name the Host gives the worktrees it makes (`pune`,
/// `pune-3`).
pub fn is_host_worktree_name(dir: &str) -> bool {
    names::stem(dir).is_some()
}

/// The origin's default branch as a remote-tracking name (`origin/main`):
/// `origin/HEAD`, else `origin/main`, else `origin/master` — whichever
/// remote-tracking branch exists locally. No fetch.
pub async fn origin_default_branch(repo: &str) -> Option<String> {
    let origin_head = crate::process::tokio_command("git")
        .args([
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ])
        .current_dir(repo)
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let candidates = origin_head
        .into_iter()
        .chain(["origin/main".to_string(), "origin/master".to_string()]);
    for branch in candidates {
        let exists = crate::process::tokio_command("git")
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/remotes/{branch}^{{commit}}"),
            ])
            .current_dir(repo)
            .output()
            .await
            .is_ok_and(|o| o.status.success());
        if exists {
            return Some(branch);
        }
    }
    None
}

/// The branches a worktree of `repo` can start from, and the one to suggest:
/// the origin's default branch, else the checked-out branch.
#[derive(Debug, Serialize)]
pub struct WorktreeBases {
    pub local: Vec<String>,
    pub remote: Vec<String>,
    pub default: Option<String>,
}

pub async fn worktree_bases(repo: &str) -> Result<WorktreeBases, AppCommandError> {
    let list = folders::git_list_all_branches(repo.to_string()).await?;
    let default = match origin_default_branch(repo).await {
        Some(b) => Some(b),
        None => folders::resolve_git_head(repo)
            .await
            .ok()
            .and_then(|h| h.branch),
    };
    Ok(WorktreeBases {
        local: list.local,
        remote: list.remote,
        default,
    })
}

/// Names [`create_worktree`] and [`continue_after_merge`] try before giving up.
const NAME_ATTEMPTS: usize = 5;

/// Create a managed git worktree from root workspace `root_workspace_id`.
///
/// The host chooses both the branch and the directory, one name for both: an
/// Indian city not yet used in the repo (`pune`, at
/// `<worktrees_dir>/<repo>/pune`; see [`names`]); the client supplies
/// neither. The branch starts at branch `base` when given, else at the
/// origin's default branch (`origin/main` or `origin/master`, never a local
/// `master`); only a repo with no `origin` remote starts at its local HEAD. A
/// remote-tracking base is fetched first (best effort, see
/// [`refresh_remote_base`]). That commit and branch are recorded as the
/// worktree's base. The
/// checkout is made with the existing `git worktree add` and registered through
/// the existing worktree-folder path, so the new Folder row gets its own fresh
/// `workspace_id`.
pub async fn create_worktree(
    db: &AppDatabase,
    emitter: &EventEmitter,
    worktrees_dir: &Path,
    root_workspace_id: &str,
    base: Option<&str>,
) -> Result<WorkspaceInfo, AppCommandError> {
    let root = resolve(&db.conn, root_workspace_id).await?;
    if root.parent_id.is_some() {
        return Err(AppCommandError::invalid_input(
            "Worktrees can only be created from a root workspace",
        )
        .with_detail(root_workspace_id.to_string()));
    }
    crate::git_repo::ensure_git_repo(&root.path)?;
    let (base_commit, base_branch) = match base {
        Some(base) => {
            refresh_remote_base(&root.path, base).await;
            (
                branch_commit(&root.path, base).await?,
                Some(base.to_string()),
            )
        }
        None => match origin_default_branch(&root.path).await {
            Some(default) => {
                refresh_remote_base(&root.path, &default).await;
                (branch_commit(&root.path, &default).await?, Some(default))
            }
            None if has_origin(&root.path).await => {
                return Err(AppCommandError::invalid_input(
                    "The origin remote has no default branch here; fetch origin or choose a branch to start from",
                ));
            }
            None => (
                head_commit(&root.path).await?,
                folders::resolve_git_head(&root.path)
                    .await
                    .ok()
                    .and_then(|h| h.branch),
            ),
        },
    };

    let repo_name = Path::new(&root.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workspace".to_string());
    let parent_dir = worktrees_dir.join(&repo_name);
    std::fs::create_dir_all(&parent_dir).map_err(AppCommandError::io)?;

    // Another create may take the same name meanwhile, or git refuse one for
    // a reason the names in use don't show: try another.
    let mut refused = HashSet::new();
    let (branch, worktree_path) = loop {
        let mut taken = names::taken(&db.conn, &root.path, &parent_dir).await?;
        taken.extend(refused.iter().cloned());
        let name = names::fresh(&taken);
        let worktree_path = parent_dir.join(&name).to_string_lossy().into_owned();
        match folders::git_worktree_add(
            root.path.clone(),
            name.clone(),
            worktree_path.clone(),
            Some(base_commit.clone()),
        )
        .await
        {
            Ok(_) => break (name, worktree_path),
            Err(e) if refused.len() + 1 >= NAME_ATTEMPTS => return Err(e),
            Err(_) => {
                refused.insert(name);
            }
        }
    };

    let registered = async {
        let detail = folders::open_worktree_folder_core(db, worktree_path.clone(), root.id).await?;
        let row = folder::Entity::find_by_id(detail.id)
            .one(&db.conn)
            .await
            .map_err(db_err)?
            .ok_or_else(|| AppCommandError::not_found("Workspace not found after create"))?;
        let mut active = row.into_active_model();
        active.base_branch = Set(base_branch);
        active.base_commit = Set(Some(base_commit));
        let row = active.update(&db.conn).await.map_err(db_err)?;
        Ok::<_, AppCommandError>((detail, row))
    }
    .await;
    let (detail, row) = match registered {
        Ok(registered) => registered,
        Err(e) => {
            // Don't strand an unregistered checkout: it has no commits yet.
            let _ = folders::remove_worktree_checkout(&root.path, &worktree_path, true).await;
            let _ = crate::process::tokio_command("git")
                .args(["branch", "-D", &branch])
                .current_dir(&root.path)
                .output()
                .await;
            return Err(e);
        }
    };
    folders::emit_folder_upsert(emitter, detail);
    info(&db.conn, row).await
}

/// Remove managed worktree workspace `workspace_id`.
///
/// Holds the workspace's gate exclusively, so no agent or terminal can start
/// there meanwhile, and refuses with `workspace_busy` while any agent is
/// running in it — `force` never overrides that. Without `force` a checkout
/// with uncommitted changes is refused with `workspace_dirty`; with it they
/// are discarded. A to-do task working there also blocks it. Terminals never
/// block: once nothing refuses, every terminal running there is killed.
/// Then the checkout is removed and the folder converges without moving its
/// sessions: they stay on the (soft-deleted) workspace, readable under the
/// same `workspace_id`, and can never resume in another checkout. Tabs close,
/// clients are told. The branch is kept so no commits are lost.
pub async fn remove_worktree(
    db: &AppDatabase,
    emitter: &EventEmitter,
    connections: &ConnectionManager,
    terminals: &TerminalManager,
    workspace_id: &str,
    force: bool,
) -> Result<WorktreeRemoved, ZwiimError> {
    let wt = resolve(&db.conn, workspace_id).await?;
    let Some(root_id) = wt.parent_id else {
        return Err(ZwiimError::invalid_input(
            "Only worktree workspaces can be removed",
        ));
    };
    let _exclusive = gate(wt.id).write_owned().await;
    // Re-read under the gate: a concurrent removal may have finished first.
    let wt = resolve(&db.conn, workspace_id).await?;
    let root = folder::Entity::find_by_id(root_id)
        .filter(folder::Column::DeletedAt.is_null())
        .one(&db.conn)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ZwiimError::not_found("Root workspace not found"))?;
    let parent_workspace_id = root
        .workspace_id
        .clone()
        .ok_or_else(|| ZwiimError::not_found("Root workspace not found"))?;

    let path = PathBuf::from(&wt.path);
    let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let spellings = [path.as_path(), canonical.as_path()];
    let live_agents = connections.live_in_folder(wt.id, &spellings).await;
    if !live_agents.is_empty() {
        // Name the sessions whose agents run there, so a client can offer to
        // stop them; an agent not bound to a session yet is only counted.
        let session_ids = session_ids_of(&db.conn, &live_agents).await?;
        return Err(ZwiimError::workspace_busy(live_agents.len(), 0, session_ids));
    }

    let busy =
        crate::db::service::work_task_service::tasks_blocking_worktree_removal(&db.conn, wt.id)
            .await
            .map_err(AppCommandError::from)?;
    if !busy.is_empty() {
        return Err(ZwiimError::invalid_input(
            "A to-do task is still working in this worktree — cancel or finish it first",
        ));
    }

    if !force && path.exists() {
        let changes = folders::git_status(wt.path.clone(), None).await?.len();
        if changes > 0 {
            return Err(ZwiimError::workspace_dirty(changes));
        }
    }

    terminals.kill_under_dir(&spellings);
    folders::remove_worktree_checkout(&root.path, &wt.path, force).await?;
    folders::converge_removed_worktree_folder(db, emitter, wt.id, None, &wt.path).await;
    let kept_sessions = conversation::Entity::find()
        .filter(conversation::Column::FolderId.eq(wt.id))
        .filter(conversation::Column::SessionId.is_not_null())
        .filter(conversation::Column::DeletedAt.is_null())
        .count(&db.conn)
        .await
        .map_err(db_err)? as u32;

    Ok(WorktreeRemoved {
        workspace_id: workspace_id.to_string(),
        parent_workspace_id,
        kept_sessions,
    })
}

// ─── continue after a merged pull request ────────────────────────────────

/// Continue worktree workspace `workspace_id` after its pull request `number`
/// merged: the same checkout, `workspace_id` and sessions move to a fresh
/// branch cut from the merged pull request's base as `origin` has it now, and
/// that becomes the workspace's new base. The branch is a placeholder the
/// agent renames: the previous branch with a `-v<n>` suffix (`fix-login`
/// continues as `fix-login-v1`, `fix-login-v1` as `fix-login-v2`). A name
/// that was the previous branch becomes the new one.
///
/// Holds the workspace's gate exclusively, so no agent prompt, launch or
/// terminal spawn starts there meanwhile. Refuses — changing nothing — while
/// any agent there is working (`workspace_busy`), with uncommitted or
/// untracked changes (`workspace_dirty`), unless GitHub reports the branch's
/// pull request is `number` (`pr_changed`) and merged (`pr_not_merged`), and
/// unless the checkout is exactly the head that merged (`head_not_merged`).
/// The previous branch is kept as it is: never deleted, reset or forced.
/// Every session in the workspace is told once, on its next prompt.
pub async fn continue_after_merge(
    db: &AppDatabase,
    emitter: &EventEmitter,
    connections: &ConnectionManager,
    host: &GitHost<'_>,
    workspace_id: &str,
    number: u64,
) -> Result<WorkspaceInfo, ZwiimError> {
    let wt = resolve(&db.conn, workspace_id).await?;
    if wt.parent_id.is_none() {
        return Err(ZwiimError::invalid_input(
            "Only worktree workspaces can be continued",
        ));
    }
    let _exclusive = enter_exclusive(wt.id).await;
    // Re-read under the gate: a removal may have finished first.
    let wt = resolve(&db.conn, workspace_id).await?;

    let (busy, session_ids) = busy_agents(&db.conn, connections, &wt, None).await?;
    if busy > 0 {
        return Err(ZwiimError::workspace_busy(busy, 0, session_ids));
    }
    let changes = folders::git_status(wt.path.clone(), None).await?.len();
    if changes > 0 {
        return Err(ZwiimError::workspace_dirty(changes));
    }
    let previous = current_branch(&wt.path)
        .await
        .ok_or_else(|| ZwiimError::invalid_input("The workspace is not on a branch"))?;

    let caller = GitCaller::new(Some(workspace_id.to_string()), None)?;
    let pr = git_harness::pr_state(host, &caller, &previous)
        .await?
        .ok_or_else(|| ZwiimError::not_found("This branch has no pull request"))?;
    if pr.number != number {
        return Err(GitError::new(
            GitErrorCode::PrChanged,
            format!(
                "This branch's pull request is now #{}, not #{number}",
                pr.number
            ),
        )
        .into());
    }
    let state = match pr.state {
        PrLifecycle::Merged => None,
        PrLifecycle::Open => Some("open"),
        PrLifecycle::Draft => Some("draft"),
        PrLifecycle::Closed => Some("closed"),
    };
    if let Some(state) = state {
        return Err(ZwiimError::pr_not_merged(pr.number, state));
    }
    ensure_at_merged_head(&wt.path, &pr.head_sha).await?;

    git_harness::execute(
        host,
        &caller,
        GitOp::Fetch {
            remote: Some("origin".to_string()),
        },
    )
    .await?;
    let base_branch = format!("origin/{}", pr.base);
    let base_commit = remote_tracking_commit(&wt.path, &base_branch)
        .await
        .ok_or_else(|| {
            ZwiimError::not_found(format!("{base_branch} was not found after fetching origin"))
        })?;
    // Everything needed once the switch is made is read now, while failing
    // still leaves the workspace as it was.
    let parent_workspace_id = match wt.parent_id {
        Some(parent_id) => workspace_id_of(&db.conn, parent_id).await?,
        None => None,
    };
    let conversations = session_conversations(&db.conn, wt.id).await?;
    // Terminals and editors are not held off by the gate: check again after
    // the network round trips, just before switching.
    ensure_at_merged_head(&wt.path, &pr.head_sha).await?;

    // Git may refuse a free-looking name (a `fix-v1/x` ref, a race): try the
    // next one.
    let mut refused = HashSet::new();
    let branch = loop {
        let mut taken = names::refs(&wt.path).await?;
        taken.extend(refused.iter().cloned());
        let branch = names::continued(&previous, &taken);
        let Err(e) = git_harness::execute(
            host,
            &caller,
            GitOp::BranchCreate {
                name: branch.clone(),
                from: Some(base_commit.clone()),
            },
        )
        .await
        else {
            break branch;
        };
        if current_branch(&wt.path).await.as_deref() == Some(branch.as_str()) {
            tracing::error!(
                workspace_id,
                code = e.code.as_str(),
                "[workspace] continue: creating the branch failed after switching"
            );
            return Err(compensate(
                db,
                emitter,
                host,
                &caller,
                &wt,
                &previous,
                &branch,
                &base_commit,
            )
            .await);
        }
        // Nothing switched: the workspace is as it was.
        if refused.len() + 1 >= NAME_ATTEMPTS {
            return Err(e.into());
        }
        refused.insert(branch.to_lowercase());
    };

    let mut active = wt.clone().into_active_model();
    active.base_branch = Set(Some(base_branch.clone()));
    active.base_commit = Set(Some(base_commit.clone()));
    // A worktree's name defaults to its branch: that name follows the
    // branch, while a name the user chose stays.
    if wt.alias.as_deref() == Some(previous.as_str()) {
        active.alias = Set(Some(branch.clone()));
    }
    let row = match active.update(&db.conn).await {
        Ok(row) => row,
        Err(e) => {
            tracing::error!(workspace_id, error = %e, "[workspace] continue: saving the new base failed");
            return Err(compensate(
                db,
                emitter,
                host,
                &caller,
                &wt,
                &previous,
                &branch,
                &base_commit,
            )
            .await);
        }
    };

    // Committed: nothing below fails.
    let notice = crate::harness::continuation_notice(
        pr.number,
        &base_branch,
        &branch,
        &previous,
        &base_commit,
    );
    for conversation_id in conversations {
        connections.queue_notice(conversation_id, notice.clone());
    }
    emit_upsert(db, emitter, wt.id).await;
    tracing::info!(workspace_id, number, %branch, %base_branch, "[workspace] continued after merge");
    let git = live_git(&row).await;
    Ok(to_info(row, parent_workspace_id, git))
}

/// Refuses unless checkout `path` has no uncommitted or untracked changes
/// (`workspace_dirty`) and is exactly at the merged head `pr_head`
/// (`head_not_merged`).
async fn ensure_at_merged_head(path: &str, pr_head: &str) -> Result<(), ZwiimError> {
    let changes = folders::git_status(path.to_string(), None).await?.len();
    if changes > 0 {
        return Err(ZwiimError::workspace_dirty(changes));
    }
    let head = head_commit(path).await?;
    if head != pr_head {
        return Err(ZwiimError::head_not_merged(&head, pr_head));
    }
    Ok(())
}

/// Undo a switch whose new base could not be saved. Never forces: switching
/// back refuses to overwrite changes, and the new branch is deleted only
/// while it still points at the commit it was created at.
#[allow(clippy::too_many_arguments)]
async fn compensate(
    db: &AppDatabase,
    emitter: &EventEmitter,
    host: &GitHost<'_>,
    caller: &GitCaller,
    wt: &folder::Model,
    previous: &str,
    branch: &str,
    base_commit: &str,
) -> ZwiimError {
    let back = git_harness::execute(
        host,
        caller,
        GitOp::Switch {
            branch: previous.to_string(),
        },
    )
    .await;
    if back.is_ok() {
        let deleted = crate::process::tokio_command("git")
            .args([
                "update-ref",
                "-d",
                &format!("refs/heads/{branch}"),
                base_commit,
            ])
            .current_dir(&wt.path)
            .output()
            .await;
        if !deleted.is_ok_and(|out| out.status.success()) {
            tracing::warn!(%branch, "[workspace] continue: the new branch moved; kept");
        }
    }
    emit_upsert(db, emitter, wt.id).await;
    let message = match back {
        Ok(_) => format!("Continue could not save the new base; the workspace is back on {previous}"),
        Err(_) => format!(
            "Continue could not save the new base or switch back; the workspace is on {branch} with its previous base"
        ),
    };
    ZwiimError {
        message,
        ..ZwiimError::internal()
    }
}

/// The branch checked out in `path`; `None` when detached.
async fn current_branch(path: &str) -> Option<String> {
    folders::resolve_git_head(path)
        .await
        .ok()
        .and_then(|head| head.branch)
}

/// The commit remote-tracking branch `name` (`origin/main`) points at.
async fn remote_tracking_commit(repo: &str, name: &str) -> Option<String> {
    let out = crate::process::tokio_command("git")
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/remotes/{name}^{{commit}}"),
        ])
        .current_dir(repo)
        .output()
        .await
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The conversations behind Folder row `folder_id`'s sessions.
async fn session_conversations(
    conn: &DatabaseConnection,
    folder_id: i32,
) -> Result<Vec<i32>, AppCommandError> {
    Ok(conversation::Entity::find()
        .filter(conversation::Column::FolderId.eq(folder_id))
        .filter(conversation::Column::SessionId.is_not_null())
        .filter(conversation::Column::DeletedAt.is_null())
        .all(conn)
        .await
        .map_err(db_err)?
        .into_iter()
        .map(|c| c.id)
        .collect())
}

/// Tell clients of the legacy folder surface the folder changed; best-effort.
async fn emit_upsert(db: &AppDatabase, emitter: &EventEmitter, folder_id: i32) {
    match folders::get_folder_core(db, folder_id).await {
        Ok(detail) => folders::emit_folder_upsert(emitter, detail),
        Err(e) => tracing::warn!(folder_id, error = %e, "[workspace] folder upsert not emitted"),
    }
}

#[cfg(test)]
mod continue_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_error::AppErrorCode;
    use crate::db::service::folder_service;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};
    use crate::models::agent::AgentType;
    use crate::terminal::manager::SpawnOptions;

    async fn row(db: &AppDatabase, id: i32) -> folder::Model {
        folder::Entity::find_by_id(id)
            .one(&db.conn)
            .await
            .unwrap()
            .expect("row")
    }

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

    /// A one-commit repo registered as a root workspace, plus a managed
    /// worktrees dir. Returns `(tempdir, repo_path, root_workspace_id, worktrees_dir)`.
    async fn root_repo(db: &AppDatabase) -> (tempfile::TempDir, String, String, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
        let repo = repo.to_string_lossy().into_owned();
        let id = seed_folder(db, &repo).await;
        let wid = row(db, id).await.workspace_id.unwrap();
        let worktrees = dir.path().join("host-data").join("worktrees");
        (dir, repo, wid, worktrees)
    }

    #[tokio::test]
    async fn workspace_id_is_unique_stable_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let (a_wid, b_wid, a_id) = {
            let db = crate::db::init_database(dir.path(), "test").await.unwrap();
            let a = seed_folder(&db, "/tmp/zwiim-ws-a").await;
            let b = seed_folder(&db, "/tmp/zwiim-ws-b").await;
            (
                row(&db, a).await.workspace_id,
                row(&db, b).await.workspace_id,
                a,
            )
        };
        assert!(a_wid.is_some() && b_wid.is_some());
        assert_ne!(a_wid, b_wid);
        assert_ne!(a_wid.as_deref(), Some(a_id.to_string().as_str()));

        let db = crate::db::init_database(dir.path(), "test").await.unwrap();
        assert_eq!(row(&db, a_id).await.workspace_id, a_wid);
        assert_eq!(
            resolve(&db.conn, a_wid.as_deref().unwrap())
                .await
                .unwrap()
                .id,
            a_id
        );
    }

    #[tokio::test]
    async fn soft_delete_and_re_add_revives_the_same_workspace_id() {
        let db = fresh_in_memory_db().await;
        let id = seed_folder(&db, "/tmp/zwiim-ws-revive").await;
        let wid = row(&db, id).await.workspace_id.unwrap();

        folder_service::soft_delete_folder(&db.conn, id)
            .await
            .unwrap();
        assert!(resolve(&db.conn, &wid).await.is_err());

        let revived = seed_folder(&db, "/tmp/zwiim-ws-revive").await;
        assert_eq!(revived, id, "re-adding the path revives the same row");
        assert_eq!(
            row(&db, id).await.workspace_id.as_deref(),
            Some(wid.as_str())
        );
        assert_eq!(resolve(&db.conn, &wid).await.unwrap().id, id);
    }

    #[tokio::test]
    async fn chat_scratch_folder_has_no_workspace_identity() {
        let db = fresh_in_memory_db().await;
        seed_folder(&db, "/tmp/zwiim-ws-regular").await;
        let chat = folder_service::add_chat_folder(&db.conn, "/tmp/zwiim-ws-chat-scratch")
            .await
            .unwrap();
        assert_eq!(row(&db, chat.id).await.workspace_id, None);
        let listed = list(&db.conn).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed
            .iter()
            .all(|w| w.path != "/tmp/zwiim-ws-chat-scratch"));
    }

    #[test]
    fn display_path_is_home_relative() {
        let home = Path::new("/Users/me");
        assert_eq!(display_path("/Users/me", Some(home)).as_deref(), Some("~"));
        assert_eq!(
            display_path("/Users/me/Projects/zwiim", Some(home)).as_deref(),
            Some("~/Projects/zwiim")
        );
        assert_eq!(display_path("/Users/meow/x", Some(home)), None);
        assert_eq!(display_path("/opt/x", Some(home)), None);
        assert_eq!(display_path("/opt/x", None), None);
    }

    #[tokio::test]
    async fn workspace_info_carries_no_raw_folder_id() {
        let db = fresh_in_memory_db().await;
        seed_folder(&db, "/tmp/zwiim-ws-json").await;
        let json = serde_json::to_value(&list(&db.conn).await.unwrap()[0]).unwrap();
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "alias",
                "base_branch",
                "base_commit",
                "branch",
                "display_path",
                "is_git_repo",
                "kind",
                "name",
                "parent_workspace_id",
                "removed",
                "workspace_id"
            ]
        );
        assert_eq!(json["kind"], "root");
        assert_eq!(json["removed"], false);
        assert_eq!(json["is_git_repo"], false);
    }

    #[tokio::test]
    async fn host_creates_worktree_with_its_own_path_branch_and_id() {
        let db = fresh_in_memory_db().await;
        let (_dir, repo, root_wid, worktrees) = root_repo(&db).await;

        let wt = create_worktree(&db, &EventEmitter::Noop, &worktrees, &root_wid, None)
            .await
            .expect("create");

        assert_eq!(wt.kind, WorkspaceKind::Worktree);
        assert_eq!(wt.parent_workspace_id.as_deref(), Some(root_wid.as_str()));
        assert_ne!(wt.workspace_id, root_wid);
        let branch = git(Path::new(&wt.path), &["rev-parse", "--abbrev-ref", "HEAD"]);
        assert!(
            names::CITIES.contains(&branch.as_str()),
            "host-chosen city branch, got {branch}"
        );
        // Grouped per project, the folder named like its branch.
        assert_eq!(Path::new(&wt.path), worktrees.join("repo").join(&branch));
        assert_eq!(wt.alias.as_deref(), Some(branch.as_str()));
        assert_eq!(wt.display_path, None, "worktrees carry no display path");
        assert_eq!(wt.branch.as_deref(), Some(branch.as_str()));
        assert!(git(Path::new(&repo), &["worktree", "list"]).contains(&branch));

        // The base is the root's local HEAD at creation, recorded and persisted.
        let root_head = git(Path::new(&repo), &["rev-parse", "HEAD"]);
        assert_eq!(wt.base_branch.as_deref(), Some("main"));
        assert_eq!(wt.base_commit.as_deref(), Some(root_head.as_str()));
        assert_eq!(git(Path::new(&wt.path), &["rev-parse", "HEAD"]), root_head);
        let listed = list(&db.conn).await.unwrap();
        let listed_wt = listed
            .iter()
            .find(|w| w.workspace_id == wt.workspace_id)
            .unwrap();
        assert_eq!(listed_wt.base_commit.as_deref(), Some(root_head.as_str()));
        assert_eq!(listed_wt.branch.as_deref(), Some(branch.as_str()));
        let listed_root = listed.iter().find(|w| w.workspace_id == root_wid).unwrap();
        assert_eq!(listed_root.branch.as_deref(), Some("main"));
        assert_eq!(listed_root.base_commit, None);

        // Moving the root afterwards does not move the recorded base.
        git(
            Path::new(&repo),
            &["commit", "-q", "--allow-empty", "-m", "later"],
        );
        let again = info(&db.conn, resolve(&db.conn, &wt.workspace_id).await.unwrap())
            .await
            .unwrap();
        assert_eq!(again.base_commit.as_deref(), Some(root_head.as_str()));

        // Only a root workspace can spawn worktrees.
        let err = create_worktree(&db, &EventEmitter::Noop, &worktrees, &wt.workspace_id, None)
            .await
            .expect_err("worktree of a worktree");
        assert!(matches!(err.code, AppErrorCode::InvalidInput));
        let err = create_worktree(&db, &EventEmitter::Noop, &worktrees, "no-such-workspace", None)
            .await
            .expect_err("unknown workspace");
        assert!(matches!(err.code, AppErrorCode::NotFound));
    }

    #[tokio::test]
    async fn worktree_starts_from_a_chosen_branch() {
        let db = fresh_in_memory_db().await;
        let (_dir, repo, root_wid, worktrees) = root_repo(&db).await;
        let r = Path::new(&repo);
        git(r, &["branch", "feature"]);
        git(r, &["checkout", "-q", "feature"]);
        git(r, &["commit", "-q", "--allow-empty", "-m", "on feature"]);
        let feature = git(r, &["rev-parse", "HEAD"]);
        git(r, &["checkout", "-q", "main"]);
        let create = |base: &'static str| {
            create_worktree(&db, &EventEmitter::Noop, &worktrees, &root_wid, Some(base))
        };

        // Without a remote, the suggestion is the checked-out branch.
        let bases = worktree_bases(&repo).await.unwrap();
        assert!(bases.local.contains(&"main".to_string()));
        assert!(bases.local.contains(&"feature".to_string()));
        assert_eq!(bases.default.as_deref(), Some("main"));

        let wt = create("feature").await.expect("create from feature");
        assert_eq!(wt.base_branch.as_deref(), Some("feature"));
        assert_eq!(wt.base_commit.as_deref(), Some(feature.as_str()));
        assert_eq!(git(Path::new(&wt.path), &["rev-parse", "HEAD"]), feature);

        // A remote's default branch is suggested and can be the base.
        git(r, &["remote", "add", "origin", &repo]);
        git(r, &["fetch", "-q", "origin"]);
        git(r, &["remote", "set-head", "origin", "main"]);
        let bases = worktree_bases(&repo).await.unwrap();
        assert_eq!(bases.default.as_deref(), Some("origin/main"));
        assert!(bases.remote.contains(&"origin/feature".to_string()));
        let wt = create("origin/main").await.expect("create from origin/main");
        assert_eq!(wt.base_branch.as_deref(), Some("origin/main"));

        // Only branches resolve: not revisions, options, or unknown names.
        for bad in ["nope", "HEAD~1", "--output=/tmp/x", "main^{tree}", ""] {
            let err = create(bad).await.expect_err(bad);
            assert!(matches!(err.code, AppErrorCode::InvalidInput), "{bad}");
        }
    }

    #[tokio::test]
    async fn worktree_defaults_to_the_origin_default_branch_never_local_master() {
        let db = fresh_in_memory_db().await;
        let (dir, repo, root_wid, worktrees) = root_repo(&db).await;
        let r = Path::new(&repo);
        let pushed = git(r, &["rev-parse", "HEAD"]);
        let origin = dir.path().join("origin.git");
        let origin = origin.to_str().unwrap();
        git(r, &["init", "-q", "--bare", origin]);
        git(r, &["remote", "add", "origin", origin]);
        git(r, &["push", "-q", "origin", "main"]);
        git(r, &["fetch", "-q", "origin"]);
        // Local branches move on; the origin's do not.
        git(r, &["commit", "-q", "--allow-empty", "-m", "local only"]);
        git(r, &["branch", "master"]);
        let create = || create_worktree(&db, &EventEmitter::Noop, &worktrees, &root_wid, None);

        // No origin/HEAD: origin/main.
        let wt = create().await.expect("origin/main");
        assert_eq!(wt.base_branch.as_deref(), Some("origin/main"));
        assert_eq!(wt.base_commit.as_deref(), Some(pushed.as_str()));
        assert_eq!(git(Path::new(&wt.path), &["rev-parse", "HEAD"]), pushed);

        // No origin/main: origin/master, not the local master.
        git(r, &["update-ref", "refs/remotes/origin/master", &pushed]);
        git(r, &["update-ref", "-d", "refs/remotes/origin/main"]);
        let wt = create().await.expect("origin/master");
        assert_eq!(wt.base_branch.as_deref(), Some("origin/master"));
        assert_eq!(wt.base_commit.as_deref(), Some(pushed.as_str()));

        // origin/HEAD names the default when it is set.
        git(r, &["update-ref", "refs/remotes/origin/trunk", &pushed]);
        git(
            r,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/trunk",
            ],
        );
        let wt = create().await.expect("origin/HEAD");
        assert_eq!(wt.base_branch.as_deref(), Some("origin/trunk"));
        assert_eq!(
            worktree_bases(&repo).await.unwrap().default.as_deref(),
            Some("origin/trunk")
        );

        // An origin with no default branch is refused, not replaced by a local one.
        git(r, &["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"]);
        git(r, &["update-ref", "-d", "refs/remotes/origin/master"]);
        git(r, &["update-ref", "-d", "refs/remotes/origin/trunk"]);
        let err = create().await.expect_err("no origin default");
        assert!(matches!(err.code, AppErrorCode::InvalidInput));
    }

    #[tokio::test]
    async fn worktree_starts_from_the_origin_latest_and_works_offline() {
        let db = fresh_in_memory_db().await;
        let (dir, repo, root_wid, worktrees) = root_repo(&db).await;
        let r = Path::new(&repo);
        let origin = dir.path().join("origin.git");
        let origin = origin.to_str().unwrap();
        git(r, &["init", "-q", "--bare", origin]);
        git(r, &["remote", "add", "origin", origin]);
        git(r, &["push", "-q", "origin", "main"]);
        git(r, &["fetch", "-q", "origin"]);
        // Someone else pushes; this repo has not fetched since.
        let push_elsewhere = |message: &str| {
            let tip = git(r, &["--git-dir", origin, "rev-parse", "main"]);
            let commit = git(r, &["commit-tree", "HEAD^{tree}", "-p", &tip, "-m", message]);
            git(r, &["push", "-q", origin, &format!("{commit}:refs/heads/main")]);
            commit
        };
        let create = |base: Option<&'static str>| {
            create_worktree(&db, &EventEmitter::Noop, &worktrees, &root_wid, base)
        };

        let latest = push_elsewhere("pushed by someone else");
        let wt = create(None).await.expect("default");
        assert_eq!(wt.base_branch.as_deref(), Some("origin/main"));
        assert_eq!(wt.base_commit.as_deref(), Some(latest.as_str()));
        assert_eq!(git(Path::new(&wt.path), &["rev-parse", "HEAD"]), latest);

        // A chosen remote branch is brought up to date too.
        let newer = push_elsewhere("pushed again");
        let wt = create(Some("origin/main")).await.expect("origin/main");
        assert_eq!(wt.base_commit.as_deref(), Some(newer.as_str()));

        // Unreachable origin: the local copy is used.
        git(r, &["remote", "set-url", "origin", "/nonexistent/origin.git"]);
        push_elsewhere("not reachable");
        let wt = create(None).await.expect("offline");
        assert_eq!(wt.base_commit.as_deref(), Some(newer.as_str()));
    }

    #[tokio::test]
    async fn concurrent_worktrees_are_isolated() {
        let db = fresh_in_memory_db().await;
        let (_dir, repo, root_wid, worktrees) = root_repo(&db).await;
        let emitter = EventEmitter::Noop;

        let (a, b) = tokio::join!(
            create_worktree(&db, &emitter, &worktrees, &root_wid, None),
            create_worktree(&db, &emitter, &worktrees, &root_wid, None),
        );
        let (a, b) = (a.expect("a"), b.expect("b"));
        assert_ne!(a.workspace_id, b.workspace_id);
        assert_ne!(a.path, b.path);
        let branch_a = git(Path::new(&a.path), &["rev-parse", "--abbrev-ref", "HEAD"]);
        let branch_b = git(Path::new(&b.path), &["rev-parse", "--abbrev-ref", "HEAD"]);
        assert_ne!(branch_a, branch_b);

        std::fs::write(Path::new(&a.path).join("only-in-a.txt"), "a").unwrap();
        assert!(!Path::new(&b.path).join("only-in-a.txt").exists());
        assert!(!Path::new(&repo).join("only-in-a.txt").exists());
        assert_eq!(git(Path::new(&b.path), &["status", "--porcelain"]), "");
        assert_eq!(git(Path::new(&repo), &["status", "--porcelain"]), "");
    }

    /// Make every city but `free` a branch of `repo`.
    fn take_cities(repo: &Path, free: &[&str]) {
        let head = git(repo, &["rev-parse", "HEAD"]);
        let heads = repo.join(".git").join("refs").join("heads");
        for city in names::CITIES.iter().filter(|c| !free.contains(c)) {
            std::fs::write(heads.join(city), format!("{head}\n")).unwrap();
        }
    }

    #[tokio::test]
    async fn worktree_names_skip_every_name_in_use() {
        let db = fresh_in_memory_db().await;
        let (_dir, repo, root_wid, worktrees) = root_repo(&db).await;
        let r = Path::new(&repo);
        let parent = worktrees.join("repo");
        std::fs::create_dir_all(&parent).unwrap();
        // Every city but seven is a branch; six of those are held another way.
        let held = ["agra", "kochi", "leh", "ooty", "pune", "puri"];
        take_cities(r, &[&held[..], &["surat"]].concat());
        git(r, &["tag", "agra"]);
        // A worktree whose folder was deleted by hand is still registered.
        let ghost = parent.join("kochi");
        git(
            r,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "elsewhere",
                &ghost.to_string_lossy(),
            ],
        );
        std::fs::remove_dir_all(&ghost).unwrap();
        let head = git(r, &["rev-parse", "HEAD"]);
        git(r, &["update-ref", "refs/remotes/origin/leh", &head]);
        git(r, &["branch", "ooty/feature"]);
        std::fs::create_dir(parent.join("PUNE")).unwrap();
        // A removed worktree's path would bring its row and sessions back.
        let removed = seed_folder(&db, &parent.join("puri").to_string_lossy()).await;
        folder_service::soft_delete_folder(&db.conn, removed)
            .await
            .unwrap();
        let taken = names::taken(&db.conn, &repo, &parent).await.unwrap();
        for name in held {
            assert!(taken.contains(name), "{name} is in use");
        }
        assert!(!taken.contains("surat"));
        let create = || create_worktree(&db, &EventEmitter::Noop, &worktrees, &root_wid, None);

        let wt = create().await.expect("the one free city");
        assert_eq!(wt.branch.as_deref(), Some("surat"));
        assert_eq!(Path::new(&wt.path), parent.join("surat"));
        assert_ne!(
            wt.workspace_id,
            row(&db, removed).await.workspace_id.unwrap()
        );

        // Every city taken: a city again, numbered.
        let wt = create().await.expect("a numbered city");
        let branch = wt.branch.unwrap();
        assert!(
            branch.ends_with("-2") && names::stem(&branch).is_some(),
            "{branch}"
        );
        assert_eq!(Path::new(&wt.path), parent.join(&branch));
    }

    #[tokio::test]
    async fn a_name_git_refuses_is_skipped() {
        let db = fresh_in_memory_db().await;
        let (_dir, repo, root_wid, worktrees) = root_repo(&db).await;
        let r = Path::new(&repo);
        take_cities(r, &["pune"]);
        // Left behind by a git that crashed: `pune` can't be created.
        std::fs::write(r.join(".git/refs/heads/pune.lock"), "").unwrap();

        let wt = create_worktree(&db, &EventEmitter::Noop, &worktrees, &root_wid, None)
            .await
            .expect("another name");
        let branch = wt.branch.unwrap();
        assert!(branch.ends_with("-2"), "{branch}");
    }

    #[tokio::test]
    async fn concurrent_worktrees_never_share_a_name() {
        let db = fresh_in_memory_db().await;
        let (_dir, repo, root_wid, worktrees) = root_repo(&db).await;
        take_cities(Path::new(&repo), &["pune"]);
        let emitter = EventEmitter::Noop;

        let (a, b) = tokio::join!(
            create_worktree(&db, &emitter, &worktrees, &root_wid, None),
            create_worktree(&db, &emitter, &worktrees, &root_wid, None),
        );
        let branches = [a.expect("a").branch.unwrap(), b.expect("b").branch.unwrap()];
        let (city, numbered): (Vec<_>, Vec<_>) = branches.iter().partition(|b| *b == "pune");
        assert_eq!(city.len(), 1, "{branches:?}");
        assert!(numbered[0].ends_with("-2"), "{branches:?}");
    }

    fn spawn_terminal(terminals: &TerminalManager, id: &str, dir: &str) {
        terminals
            .spawn_with_id(
                SpawnOptions {
                    terminal_id: id.into(),
                    working_dir: dir.into(),
                    owner_window_label: "main".into(),
                    shell: None,
                    initial_command: None,
                    extra_env: None,
                    temp_files: Vec::new(),
                },
                EventEmitter::Noop,
            )
            .expect("spawn terminal");
    }

    fn details(e: &ZwiimError) -> serde_json::Value {
        e.details.clone().unwrap_or_default()
    }

    #[tokio::test]
    async fn running_agent_blocks_removal_even_with_force_terminals_are_killed() {
        let db = fresh_in_memory_db().await;
        let (_dir, _repo, root_wid, worktrees) = root_repo(&db).await;
        let emitter = EventEmitter::Noop;
        let wt = create_worktree(&db, &emitter, &worktrees, &root_wid, None)
            .await
            .unwrap();
        let idle_agents = ConnectionManager::new();
        let idle_terminals = TerminalManager::new();

        // Two live agent connections working inside the worktree.
        let agents = ConnectionManager::new();
        for id in ["conn-wt-1", "conn-wt-2"] {
            agents
                .insert_test_connection(
                    id,
                    AgentType::ClaudeCode,
                    Some(PathBuf::from(&wt.path)),
                    EventEmitter::Noop,
                )
                .await;
        }
        let err = remove_worktree(
            &db,
            &emitter,
            &agents,
            &idle_terminals,
            &wt.workspace_id,
            true,
        )
        .await
        .expect_err("agent guard");
        assert_eq!(err.code, "workspace_busy");
        assert_eq!(
            details(&err),
            serde_json::json!({ "live_sessions": 2, "terminals": 0, "session_ids": [] })
        );
        assert!(Path::new(&wt.path).exists());
        assert!(resolve(&db.conn, &wt.workspace_id).await.is_ok());

        // A live terminal started inside the worktree never blocks: the
        // removal kills it.
        let terminals = TerminalManager::new();
        spawn_terminal(&terminals, "term-wt", &wt.path);
        remove_worktree(
            &db,
            &emitter,
            &idle_agents,
            &terminals,
            &wt.workspace_id,
            false,
        )
        .await
        .expect("removal kills the terminal");
        assert!(!Path::new(&wt.path).exists());
        assert!(matches!(
            terminals.kill("term-wt"),
            Err(crate::terminal::error::TerminalError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn stopping_the_named_sessions_unblocks_removal() {
        let db = fresh_in_memory_db().await;
        let (_dir, _repo, root_wid, worktrees) = root_repo(&db).await;
        let emitter = EventEmitter::Noop;
        let wt = create_worktree(&db, &emitter, &worktrees, &root_wid, None)
            .await
            .unwrap();
        let folder_id = resolve(&db.conn, &wt.workspace_id).await.unwrap().id;
        let agents = ConnectionManager::new();
        let terminals = TerminalManager::new();

        // Two sessions with agents running in the worktree.
        let mut sessions = Vec::new();
        for id in ["conn-a", "conn-b"] {
            let conv = seed_conversation(&db, folder_id, AgentType::ClaudeCode).await;
            agents
                .insert_test_connection(
                    id,
                    AgentType::ClaudeCode,
                    Some(PathBuf::from(&wt.path)),
                    EventEmitter::Noop,
                )
                .await;
            agents.get_state(id).await.unwrap().write().await.conversation_id = Some(conv);
            let row = conversation::Entity::find_by_id(conv).one(&db.conn).await.unwrap().unwrap();
            sessions.push(row.session_id.unwrap());
        }
        let busy = |e: ZwiimError| {
            assert_eq!(e.code, "workspace_busy");
            let d = details(&e);
            let mut ids: Vec<String> =
                serde_json::from_value(d["session_ids"].clone()).unwrap();
            ids.sort();
            (d["live_sessions"].as_u64().unwrap(), d["terminals"].as_u64().unwrap(), ids)
        };
        let remove = |force| remove_worktree(&db, &emitter, &agents, &terminals, &wt.workspace_id, force);

        // The host names both sessions; force changes nothing.
        let mut both = sessions.clone();
        both.sort();
        assert_eq!(busy(remove(true).await.unwrap_err()), (2, 0, both));

        // Stopping one leaves the other blocking, still named.
        crate::session::stop(&db, &agents, &sessions[0]).await.unwrap();
        assert_eq!(
            busy(remove(true).await.unwrap_err()),
            (1, 0, vec![sessions[1].clone()])
        );

        // Both stopped: removal goes through, sessions kept.
        crate::session::stop(&db, &agents, &sessions[1]).await.unwrap();
        let removed = remove(false).await.expect("removal once all are stopped");
        assert_eq!(removed.kept_sessions, 2);
        assert!(!Path::new(&wt.path).exists());
    }

    #[tokio::test]
    async fn busy_agents_counts_only_working_agents() {
        let db = fresh_in_memory_db().await;
        let (_dir, _repo, root_wid, worktrees) = root_repo(&db).await;
        let wt = create_worktree(&db, &EventEmitter::Noop, &worktrees, &root_wid, None)
            .await
            .unwrap();
        let folder = resolve(&db.conn, &wt.workspace_id).await.unwrap();
        let agents = ConnectionManager::new();

        // conn-a mid-turn, conn-b idle, conn-c waiting on a permission.
        let mut sessions = Vec::new();
        for id in ["conn-a", "conn-b", "conn-c"] {
            let conv = seed_conversation(&db, folder.id, AgentType::ClaudeCode).await;
            agents
                .insert_test_connection(
                    id,
                    AgentType::ClaudeCode,
                    Some(PathBuf::from(&wt.path)),
                    EventEmitter::Noop,
                )
                .await;
            let state = agents.get_state(id).await.unwrap();
            let mut state = state.write().await;
            state.conversation_id = Some(conv);
            match id {
                "conn-a" => state.turn_in_flight = true,
                "conn-c" => {
                    state.pending_permission =
                        Some(crate::acp::session_state::PendingPermissionState {
                            request_id: "req-1".into(),
                            tool_call_id: "tc-1".into(),
                            tool_call: serde_json::json!({ "toolCallId": "tc-1" }),
                            options: vec![],
                            created_at: chrono::Utc::now(),
                            queued: 0,
                        })
                }
                _ => {}
            }
            let row = conversation::Entity::find_by_id(conv)
                .one(&db.conn)
                .await
                .unwrap()
                .unwrap();
            sessions.push(row.session_id.unwrap());
        }
        let sorted = |(n, mut ids): (usize, Vec<String>)| {
            ids.sort();
            (n, ids)
        };
        let mut a_and_c = vec![sessions[0].clone(), sessions[2].clone()];
        a_and_c.sort();

        let all = busy_agents(&db.conn, &agents, &folder, None).await.unwrap();
        assert_eq!(sorted(all), (2, a_and_c));
        let others = busy_agents(&db.conn, &agents, &folder, Some("conn-a"))
            .await
            .unwrap();
        assert_eq!(sorted(others), (1, vec![sessions[2].clone()]));
    }

    #[tokio::test]
    async fn removal_waits_for_an_in_flight_launch_and_then_sees_it() {
        let db = fresh_in_memory_db().await;
        let (_dir, _repo, root_wid, worktrees) = root_repo(&db).await;
        let emitter = EventEmitter::Noop;
        let wt = create_worktree(&db, &emitter, &worktrees, &root_wid, None)
            .await
            .unwrap();
        let agents = ConnectionManager::new();
        let terminals = TerminalManager::new();

        // An agent launch is under way in the worktree.
        let (_, launching) = resolve_for_launch(&db.conn, &wt.workspace_id)
            .await
            .unwrap();
        let done = std::sync::atomic::AtomicBool::new(false);
        let removal = async {
            let r =
                remove_worktree(&db, &emitter, &agents, &terminals, &wt.workspace_id, true).await;
            done.store(true, std::sync::atomic::Ordering::SeqCst);
            r
        };
        let launcher = async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            assert!(
                !done.load(std::sync::atomic::Ordering::SeqCst),
                "removal waits for the launch"
            );
            agents
                .insert_test_connection(
                    "conn-launched",
                    AgentType::ClaudeCode,
                    Some(PathBuf::from(&wt.path)),
                    EventEmitter::Noop,
                )
                .await;
            drop(launching);
        };
        let (removal, ()) = tokio::join!(removal, launcher);
        let err = removal.expect_err("sees the new agent");
        assert_eq!(err.code, "workspace_busy");
        assert!(Path::new(&wt.path).exists());

        // And once removal holds the gate, a launch after it finds the
        // workspace removed instead of starting anything.
        agents.disconnect("conn-launched").await.ok();
        remove_worktree(
            &db,
            &emitter,
            &ConnectionManager::new(),
            &terminals,
            &wt.workspace_id,
            true,
        )
        .await
        .expect("remove");
        let err = resolve_for_launch(&db.conn, &wt.workspace_id)
            .await
            .expect_err("removed");
        assert!(matches!(err.code, AppErrorCode::WorkspaceRemoved));
    }

    #[tokio::test]
    async fn dirty_worktree_needs_force_and_keeps_its_branch() {
        let db = fresh_in_memory_db().await;
        let (_dir, repo, root_wid, worktrees) = root_repo(&db).await;
        let emitter = EventEmitter::Noop;
        let wt = create_worktree(&db, &emitter, &worktrees, &root_wid, None)
            .await
            .unwrap();
        let branch = wt.branch.clone().unwrap();
        std::fs::write(Path::new(&wt.path).join("a.txt"), "a").unwrap();
        std::fs::write(Path::new(&wt.path).join("b.txt"), "b").unwrap();

        let err = remove_worktree(
            &db,
            &emitter,
            &ConnectionManager::new(),
            &TerminalManager::new(),
            &wt.workspace_id,
            false,
        )
        .await
        .expect_err("dirty");
        assert_eq!(err.code, "workspace_dirty");
        assert_eq!(details(&err), serde_json::json!({ "changes": 2 }));
        assert!(Path::new(&wt.path).join("a.txt").exists());

        remove_worktree(
            &db,
            &emitter,
            &ConnectionManager::new(),
            &TerminalManager::new(),
            &wt.workspace_id,
            true,
        )
        .await
        .expect("forced");
        assert!(!Path::new(&wt.path).exists());
        git(
            Path::new(&repo),
            &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
        );
    }

    #[tokio::test]
    async fn removal_keeps_sessions_on_the_removed_workspace() {
        let db = fresh_in_memory_db().await;
        let (_dir, repo, root_wid, worktrees) = root_repo(&db).await;
        let emitter = EventEmitter::Noop;
        let wt = create_worktree(&db, &emitter, &worktrees, &root_wid, None)
            .await
            .unwrap();
        let other = create_worktree(&db, &emitter, &worktrees, &root_wid, None)
            .await
            .unwrap();
        let empty = create_worktree(&db, &emitter, &worktrees, &root_wid, None)
            .await
            .unwrap();
        let branch = git(Path::new(&wt.path), &["rev-parse", "--abbrev-ref", "HEAD"]);
        git(
            Path::new(&wt.path),
            &["commit", "-q", "--allow-empty", "-m", "work"],
        );

        let wt_folder = resolve(&db.conn, &wt.workspace_id).await.unwrap();
        let mut sids = Vec::new();
        for _ in 0..2 {
            let conv = seed_conversation(&db, wt_folder.id, AgentType::ClaudeCode).await;
            sids.push(
                crate::db::entities::conversation::Entity::find_by_id(conv)
                    .one(&db.conn)
                    .await
                    .unwrap()
                    .unwrap()
                    .session_id
                    .unwrap(),
            );
        }

        let removed = remove_worktree(
            &db,
            &emitter,
            &ConnectionManager::new(),
            &TerminalManager::new(),
            &wt.workspace_id,
            false,
        )
        .await
        .expect("remove");
        assert_eq!(removed.parent_workspace_id, root_wid);
        assert_eq!(removed.kept_sessions, 2);
        remove_worktree(
            &db,
            &emitter,
            &ConnectionManager::new(),
            &TerminalManager::new(),
            &empty.workspace_id,
            false,
        )
        .await
        .expect("remove empty");

        // The removed worktree is no longer workable…
        let err = resolve(&db.conn, &wt.workspace_id).await.unwrap_err();
        assert!(matches!(err.code, AppErrorCode::WorkspaceRemoved));
        assert_eq!(ZwiimError::from(err).code, "workspace_removed");
        // …but stays listed (it holds sessions) under the same id, marked removed.
        let listed = list(&db.conn).await.unwrap();
        let ids: Vec<_> = listed.iter().map(|w| w.workspace_id.as_str()).collect();
        assert_eq!(
            ids,
            [
                root_wid.as_str(),
                wt.workspace_id.as_str(),
                other.workspace_id.as_str()
            ]
        );
        let gone = &listed[1];
        assert!(gone.removed);
        assert_eq!(gone.kind, WorkspaceKind::Worktree);
        assert_eq!(gone.parent_workspace_id.as_deref(), Some(root_wid.as_str()));
        assert_eq!(gone.branch, None);
        assert_eq!(gone.base_commit, wt.base_commit);
        assert!(!listed[0].removed && !listed[2].removed);

        // Git: checkout gone, branch (and its commit) kept, sibling untouched.
        assert!(!Path::new(&wt.path).exists());
        let worktree_list = git(Path::new(&repo), &["worktree", "list"]);
        assert!(!worktree_list.contains(&branch));
        assert!(Path::new(&other.path).exists());
        git(
            Path::new(&repo),
            &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
        );

        // Sessions: same ids, still under the removed worktree, never the root.
        let kept = crate::session::list_for_workspace(
            &db.conn,
            &ConnectionManager::new(),
            &wt.workspace_id,
        )
        .await
        .unwrap();
        let mut kept_ids: Vec<_> = kept.iter().map(|s| s.session_id.clone()).collect();
        kept_ids.sort();
        sids.sort();
        assert_eq!(kept_ids, sids);
        assert!(kept
            .iter()
            .all(|s| s.workspace_id.as_deref() == Some(wt.workspace_id.as_str())));
        let root_sessions =
            crate::session::list_for_workspace(&db.conn, &ConnectionManager::new(), &root_wid)
                .await
                .unwrap();
        assert!(root_sessions.is_empty());

        // A root workspace is not removable through this path.
        let err = remove_worktree(
            &db,
            &emitter,
            &ConnectionManager::new(),
            &TerminalManager::new(),
            &root_wid,
            false,
        )
        .await
        .expect_err("root is not a worktree");
        assert_eq!(err.code, "invalid_input");
    }

    #[tokio::test]
    async fn session_start_runs_the_agent_in_the_chosen_worktree() {
        use crate::session::orchestration_tests::{text, FakeLauncher};

        let db = fresh_in_memory_db().await;
        let (_dir, repo, root_wid, worktrees) = root_repo(&db).await;
        let emitter = EventEmitter::Noop;
        let wt = create_worktree(&db, &emitter, &worktrees, &root_wid, None)
            .await
            .unwrap();
        assert_ne!(wt.path, repo);
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());

        let in_worktree = crate::session::start(
            &db,
            &mgr,
            &launcher,
            &wt.workspace_id,
            AgentType::ClaudeCode,
            text("in the worktree"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        let in_root = crate::session::start(
            &db,
            &mgr,
            &launcher,
            &root_wid,
            AgentType::ClaudeCode,
            text("in the root"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();

        // Each agent runs in its own workspace's checkout, never the root's by default.
        assert_eq!(
            *launcher.working_dirs.lock().unwrap(),
            vec![wt.path.clone(), repo.clone()]
        );
        // And each session is listed under the workspace it was started in.
        let listed = |wid: String| {
            let (db, mgr) = (&db, &mgr);
            async move {
                crate::session::list_for_workspace(&db.conn, mgr, &wid)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|s| s.session_id)
                    .collect::<Vec<_>>()
            }
        };
        assert_eq!(listed(wt.workspace_id.clone()).await, vec![in_worktree]);
        assert_eq!(listed(root_wid.clone()).await, vec![in_root]);
    }

    #[tokio::test]
    async fn removed_worktree_session_never_resumes_anywhere() {
        use crate::session::orchestration_tests::{finish_turn, text, FakeLauncher};
        use std::sync::atomic::Ordering;

        let db = fresh_in_memory_db().await;
        let (_dir, _repo, root_wid, worktrees) = root_repo(&db).await;
        let emitter = EventEmitter::Noop;
        let wt = create_worktree(&db, &emitter, &worktrees, &root_wid, None)
            .await
            .unwrap();
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());

        // Two agent sessions share one worktree.
        let a = crate::session::start(
            &db,
            &mgr,
            &launcher,
            &wt.workspace_id,
            AgentType::ClaudeCode,
            text("a"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        let b = crate::session::start(
            &db,
            &mgr,
            &launcher,
            &wt.workspace_id,
            AgentType::Codex,
            text("b"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        assert_ne!(a, b);
        let listed = crate::session::list_for_workspace(&db.conn, &mgr, &wt.workspace_id)
            .await
            .unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().all(|s| s.attached));

        // Busy while either agent lives.
        let err = remove_worktree(
            &db,
            &emitter,
            &mgr,
            &TerminalManager::new(),
            &wt.workspace_id,
            true,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "workspace_busy");
        assert_eq!(details(&err)["live_sessions"], 2);

        for conn in ["fake-conn-1", "fake-conn-2"] {
            finish_turn(&mgr, conn).await;
            mgr.disconnect(conn).await.unwrap();
        }
        remove_worktree(
            &db,
            &emitter,
            &mgr,
            &TerminalManager::new(),
            &wt.workspace_id,
            false,
        )
        .await
        .expect("remove");

        // Sending would have to resume the agent: refused, nothing launched.
        let launches = launcher.launches.load(Ordering::SeqCst);
        let err = crate::session::send(&db, &mgr, &launcher, &a, text("more"), None)
            .await
            .unwrap_err();
        assert_eq!(err.code, "workspace_removed");
        assert_eq!(launcher.launches.load(Ordering::SeqCst), launches);
        // Nor can a new session or terminal start there.
        let err = crate::session::start(
            &db,
            &mgr,
            &launcher,
            &wt.workspace_id,
            AgentType::ClaudeCode,
            text("new"),
            None,
            &Default::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "workspace_removed");
        assert_eq!(launcher.launches.load(Ordering::SeqCst), launches);

        // The session is still the removed worktree's, never the root's.
        let row = crate::session::resolve(&db.conn, &a).await.unwrap();
        assert_eq!(
            row.folder_id,
            resolve_readable(&db.conn, &wt.workspace_id)
                .await
                .unwrap()
                .id
        );
        assert!(
            crate::session::list_for_workspace(&db.conn, &mgr, &root_wid)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn host_start_skips_a_root_removed_while_it_waited() {
        let db = fresh_in_memory_db().await;
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        let id = seed_folder(&db, plain.to_str().unwrap()).await;

        // The sweep lists the row, then waits on the lock; meanwhile the
        // root is removed.
        let held = repo_init_lock().await;
        let sweep = {
            let db = AppDatabase {
                conn: db.conn.clone(),
            };
            tokio::spawn(async move { init_plain_roots(&db, &EventEmitter::Noop).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        folder_service::soft_delete_folder(&db.conn, id)
            .await
            .unwrap();
        drop(held);
        sweep.await.unwrap();

        assert!(!plain.join(".git").exists());
        assert!(!plain.join(".gitignore").exists());
    }

    #[tokio::test]
    async fn host_start_redoes_an_init_the_host_stopped_in() {
        let db = fresh_in_memory_db().await;
        let dir = tempfile::tempdir().unwrap();
        let half = dir.path().join("half");
        std::fs::create_dir(&half).unwrap();
        std::fs::write(half.join("a.txt"), "x").unwrap();
        // A Host stopped after `git init`, before the commit.
        std::fs::create_dir(half.join(".git")).unwrap();
        std::fs::write(half.join(".git/zwiim-init"), "").unwrap();
        git(&half, &["init", "-q", "-b", "main"]);
        seed_folder(&db, half.to_str().unwrap()).await;

        init_plain_roots(&db, &EventEmitter::Noop).await;

        assert_eq!(git(&half, &["log", "--format=%s"]), "Initial Commit");
        assert!(!half.join(".git/zwiim-init").exists());
    }

    #[tokio::test]
    async fn only_a_live_regular_root_workspace_is_a_plain_root_to_init() {
        let db = fresh_in_memory_db().await;
        let live = seed_folder(&db, "/tmp/zwiim-sweep-live").await;
        assert!(is_live_root(&db, live).await);

        let removed = seed_folder(&db, "/tmp/zwiim-sweep-removed").await;
        folder_service::soft_delete_folder(&db.conn, removed)
            .await
            .unwrap();
        assert!(!is_live_root(&db, removed).await);

        let child = seed_folder(&db, "/tmp/zwiim-sweep-child").await;
        let mut active = row(&db, child).await.into_active_model();
        active.parent_id = sea_orm::Set(Some(live));
        active.update(&db.conn).await.unwrap();
        assert!(!is_live_root(&db, child).await);

        let chat = folder_service::add_chat_folder(&db.conn, "/tmp/zwiim-sweep-chat")
            .await
            .unwrap()
            .id;
        assert!(!is_live_root(&db, chat).await);
        assert!(!is_live_root(&db, i32::MAX).await);
    }

    #[tokio::test]
    async fn host_start_makes_plain_roots_repositories_and_skips_the_rest() {
        use std::os::unix::fs::PermissionsExt;
        // Root writes through 0o555, so the locked init would not fail.
        let as_root = unsafe { libc::geteuid() } == 0;
        let db = fresh_in_memory_db().await;
        let dir = tempfile::tempdir().unwrap();
        let mk = |name: &str| {
            let p = dir.path().join(name);
            std::fs::create_dir(&p).unwrap();
            p
        };
        let plain = mk("plain");
        std::fs::write(plain.join("a.txt"), "x").unwrap();
        let locked = mk("locked");
        let big = mk("big");
        for i in 0..5_001 {
            std::fs::write(big.join(format!("f{i}")), "").unwrap();
        }
        let repo = mk("repo");
        git(&repo, &["init", "-q", "-b", "dev"]);
        for p in [&locked, &plain, &big, &repo] {
            if as_root && p == &locked {
                continue;
            }
            seed_folder(&db, p.to_str().unwrap()).await;
        }
        seed_folder(&db, dir.path().join("gone").to_str().unwrap()).await;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();

        init_plain_roots(&db, &EventEmitter::Noop).await;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        // The locked one failed first; the sweep went on to the others.
        assert!(plain.join(".git").is_dir());
        assert_eq!(git(&plain, &["log", "--format=%s"]), "Initial Commit");
        assert!(!locked.join(".git").exists());
        assert!(!big.join(".git").exists() && !big.join(".gitignore").exists());
        assert_eq!(git(&repo, &["branch", "--show-current"]), "dev");
        assert!(!repo.join(".gitignore").exists());
    }
}
