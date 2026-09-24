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
use tokio::sync::{OwnedRwLockReadGuard, RwLock};

use crate::acp::manager::ConnectionManager;
use crate::app_error::{AppCommandError, AppErrorCode};
use crate::commands::folders;
use crate::db::entities::conversation;
use crate::db::entities::folder::{self, FolderKind};
use crate::db::error::DbError;
use crate::db::AppDatabase;
use crate::protocol::error::ZwiimError;
use crate::terminal::manager::TerminalManager;
use crate::web::event_bridge::EventEmitter;

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
    /// Branch checked out now; `None` when detached, not a repo, or removed.
    pub branch: Option<String>,
    /// For a worktree: the root's branch it was created from.
    pub base_branch: Option<String>,
    /// For a worktree: the commit its branch started at.
    pub base_commit: Option<String>,
    /// A removed worktree: its sessions are readable, nothing runs in it.
    pub removed: bool,
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

/// [`resolve`] + [`begin_launch`] for a client-addressed workspace.
pub(crate) async fn resolve_for_launch(
    conn: &DatabaseConnection,
    workspace_id: &str,
) -> Result<(folder::Model, LaunchGuard), AppCommandError> {
    let row = resolve(conn, workspace_id).await?;
    begin_launch(conn, row.id).await
}

// ─── projection ──────────────────────────────────────────────────────────

async fn live_branch(row: &folder::Model) -> Option<String> {
    if row.deleted_at.is_some() {
        return None;
    }
    folders::resolve_git_head(&row.path).await.ok()?.branch
}

pub(crate) async fn info(
    conn: &DatabaseConnection,
    row: folder::Model,
) -> Result<WorkspaceInfo, AppCommandError> {
    let parent_workspace_id = match row.parent_id {
        Some(parent_id) => workspace_id_of(conn, parent_id).await?,
        None => None,
    };
    let branch = live_branch(&row).await;
    Ok(to_info(row, parent_workspace_id, branch))
}

fn to_info(
    row: folder::Model,
    parent_workspace_id: Option<String>,
    branch: Option<String>,
) -> WorkspaceInfo {
    WorkspaceInfo {
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
    let branches = futures::future::join_all(rows.iter().map(live_branch)).await;
    Ok(rows
        .into_iter()
        .zip(branches)
        .map(|(r, branch)| {
            let parent = r.parent_id.and_then(|p| ids.get(&p).cloned());
            to_info(r, parent, branch)
        })
        .collect())
}

/// Register the host directory `path` as a workspace through the existing
/// open-folder path. Re-adding a known (or previously removed) directory
/// revives its row, which keeps its original `workspace_id`.
pub async fn add(
    db: &AppDatabase,
    emitter: &EventEmitter,
    path: &str,
) -> Result<WorkspaceInfo, AppCommandError> {
    let requested = Path::new(path);
    if !requested.is_absolute() {
        return Err(AppCommandError::invalid_input("Path must be absolute"));
    }
    let canonical = requested
        .canonicalize()
        .ok()
        .filter(|p| p.is_dir())
        .ok_or_else(|| AppCommandError::not_found("Directory not found"))?;
    let detail = folders::open_folder_core(db, canonical.to_string_lossy().into_owned()).await?;
    let row = folder::Entity::find_by_id(detail.id)
        .one(&db.conn)
        .await
        .map_err(db_err)?
        .ok_or_else(|| AppCommandError::not_found("Workspace not found"))?;
    // A host-internal scratch folder has no workspace identity.
    if row.kind != FolderKind::Regular || row.workspace_id.is_none() {
        return Err(AppCommandError::invalid_input(
            "Directory cannot be used as a workspace",
        ));
    }
    folders::emit_folder_upsert(emitter, detail);
    info(&db.conn, row).await
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

/// The branches a worktree of `repo` can start from, and the one to suggest:
/// the remote's default branch (`origin/HEAD`), else the checked-out branch.
#[derive(Debug, Serialize)]
pub struct WorktreeBases {
    pub local: Vec<String>,
    pub remote: Vec<String>,
    pub default: Option<String>,
}

pub async fn worktree_bases(repo: &str) -> Result<WorktreeBases, AppCommandError> {
    let list = folders::git_list_all_branches(repo.to_string()).await?;
    let origin_head = crate::process::tokio_command("git")
        .args(["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"])
        .current_dir(repo)
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|b| list.remote.contains(b));
    let default = match origin_head {
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

/// Create a managed git worktree from root workspace `root_workspace_id`.
///
/// The host chooses both the branch (`zwiim/<suffix>`) and the directory
/// (`<worktrees_dir>/<root workspace_id>/<repo>-<suffix>`); the client supplies
/// neither. The branch starts at branch `base` when given, else at the root's
/// current local HEAD (no fetch either way); that commit and branch are
/// recorded as the worktree's base. The
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
        Some(base) => (
            branch_commit(&root.path, base).await?,
            Some(base.to_string()),
        ),
        None => (
            head_commit(&root.path).await?,
            folders::resolve_git_head(&root.path)
                .await
                .ok()
                .and_then(|h| h.branch),
        ),
    };

    let suffix = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
    let branch = format!("zwiim/{suffix}");
    let repo_name = Path::new(&root.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workspace".to_string());
    let parent_dir = worktrees_dir.join(root_workspace_id);
    std::fs::create_dir_all(&parent_dir).map_err(AppCommandError::io)?;
    let worktree_path: PathBuf = parent_dir.join(format!("{repo_name}-{suffix}"));
    let worktree_path = worktree_path.to_string_lossy().into_owned();

    folders::git_worktree_add(
        root.path.clone(),
        branch.clone(),
        worktree_path.clone(),
        Some(base_commit.clone()),
    )
    .await?;

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
/// there meanwhile, and refuses with `workspace_busy` while any agent or
/// terminal is running in it — `force` never overrides that. Without `force`
/// a checkout with uncommitted changes is refused with `workspace_dirty`;
/// with it they are discarded. A to-do task working there also blocks it.
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
    let live_agents = connections.count_live_in_folder(wt.id, &spellings).await;
    let live_terminals = terminals.count_live_under_dir(&spellings, Some(emitter));
    if live_agents > 0 || live_terminals > 0 {
        return Err(ZwiimError::workspace_busy(live_agents, live_terminals));
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
                "kind",
                "name",
                "parent_workspace_id",
                "removed",
                "workspace_id"
            ]
        );
        assert_eq!(json["kind"], "root");
        assert_eq!(json["removed"], false);
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
        assert!(Path::new(&wt.path).starts_with(worktrees.join(&root_wid)));
        let branch = git(Path::new(&wt.path), &["rev-parse", "--abbrev-ref", "HEAD"]);
        assert!(
            branch.starts_with("zwiim/"),
            "host-chosen branch, got {branch}"
        );
        assert_eq!(wt.alias.as_deref(), Some(branch.as_str()));
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
    async fn running_agent_or_terminal_blocks_removal_even_with_force() {
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
            serde_json::json!({ "live_sessions": 2, "terminals": 0 })
        );
        assert!(Path::new(&wt.path).exists());
        assert!(resolve(&db.conn, &wt.workspace_id).await.is_ok());

        // A live terminal started inside the worktree.
        let terminals = TerminalManager::new();
        spawn_terminal(&terminals, "term-wt", &wt.path);
        let err = remove_worktree(
            &db,
            &emitter,
            &idle_agents,
            &terminals,
            &wt.workspace_id,
            true,
        )
        .await
        .expect_err("terminal guard");
        assert_eq!(err.code, "workspace_busy");
        assert_eq!(
            details(&err),
            serde_json::json!({ "live_sessions": 0, "terminals": 1 })
        );
        assert!(Path::new(&wt.path).exists());

        terminals.kill("term-wt").unwrap();
        remove_worktree(
            &db,
            &emitter,
            &idle_agents,
            &terminals,
            &wt.workspace_id,
            false,
        )
        .await
        .expect("removal once nothing runs there");
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
}
