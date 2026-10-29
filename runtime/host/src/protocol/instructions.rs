//! `host.instructions.set`: change the user's instructions at one scope.

use std::path::Path;

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect};
use serde::Deserialize;

use crate::db::entities::folder;
use crate::harness::user_instructions::{self, UserInstructions};
use crate::harness::InstructionTexts;

use super::error::ZwiimError;

/// Most bytes one text may hold.
pub const MAX_TEXT_BYTES: usize = 8 * 1024;

/// Where a change applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Host,
    Workspace,
}

/// The texts for one scope; each replaces what that scope held. Absent or
/// blank text clears it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstructionsChange {
    pub scope: Scope,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub general: Option<String>,
    #[serde(default)]
    pub resolve_conflicts: Option<String>,
}

/// The Host is the file's only writer; one change at a time.
static WRITE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Apply `change` to the instructions saved at `path`, dropping entries for
/// workspaces that are no longer live roots, and return what was saved.
pub async fn set(
    conn: &DatabaseConnection,
    path: &Path,
    change: InstructionsChange,
) -> Result<UserInstructions, ZwiimError> {
    let texts = InstructionTexts {
        general: checked(change.general)?,
        resolve_conflicts: checked(change.resolve_conflicts)?,
    };
    let _guard = WRITE.lock().await;
    let roots = live_roots(conn).await?;
    let key = match (change.scope, change.workspace_id) {
        (Scope::Host, None) => None,
        (Scope::Workspace, Some(id)) if roots.contains(&id) => Some(id),
        (Scope::Workspace, Some(_)) => {
            return Err(ZwiimError::not_found("No such root workspace"))
        }
        (Scope::Host, Some(_)) => {
            return Err(ZwiimError::invalid_input(
                "A host change does not name a workspace",
            ))
        }
        (Scope::Workspace, None) => {
            return Err(ZwiimError::invalid_input(
                "A workspace change needs a workspace_id",
            ))
        }
    };
    let mut value = user_instructions::load(path);
    match key {
        None => value.host = texts,
        Some(id) if texts == InstructionTexts::default() => {
            value.workspaces.remove(&id);
        }
        Some(id) => {
            value.workspaces.insert(id, texts);
        }
    }
    value.workspaces.retain(|id, _| roots.contains(id));
    user_instructions::save(path, &value).map_err(|e| {
        tracing::warn!("saving user instructions failed: {e}");
        ZwiimError::internal()
    })?;
    Ok(value)
}

/// Text as saved: blank is none, and none may be over `MAX_TEXT_BYTES`.
fn checked(text: Option<String>) -> Result<Option<String>, ZwiimError> {
    match text {
        Some(t) if t.len() > MAX_TEXT_BYTES => Err(ZwiimError::too_large(
            "An instruction text is larger than 8 KiB",
        )),
        Some(t) if t.trim().is_empty() => Ok(None),
        t => Ok(t),
    }
}

/// The `workspace_id`s of every live root workspace.
async fn live_roots(conn: &DatabaseConnection) -> Result<Vec<String>, ZwiimError> {
    let ids: Vec<Option<String>> = folder::Entity::find()
        .select_only()
        .column(folder::Column::WorkspaceId)
        .filter(folder::Column::ParentId.is_null())
        .filter(folder::Column::DeletedAt.is_null())
        .filter(folder::Column::WorkspaceId.is_not_null())
        .into_tuple()
        .all(conn)
        .await
        .map_err(|e| {
            tracing::warn!("listing root workspaces failed: {e}");
            ZwiimError::internal()
        })?;
    Ok(ids.into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::service::folder_service;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder, seed_worktree_folder};

    async fn root_id(db: &crate::db::AppDatabase, id: i32) -> String {
        crate::workspace::workspace_id_of(&db.conn, id)
            .await
            .unwrap()
            .unwrap()
    }

    fn change(scope: Scope, workspace_id: Option<&str>, general: &str) -> InstructionsChange {
        InstructionsChange {
            scope,
            workspace_id: workspace_id.map(str::to_string),
            general: Some(general.into()),
            resolve_conflicts: None,
        }
    }

    #[tokio::test]
    async fn set_rejects_oversize() {
        let db = fresh_in_memory_db().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instructions.json");
        let big = "x".repeat(MAX_TEXT_BYTES + 1);
        let err = set(&db.conn, &path, change(Scope::Host, None, &big))
            .await
            .unwrap_err();
        assert_eq!(err.code, "too_large");
        let mut conflicts = change(Scope::Host, None, "ok");
        conflicts.resolve_conflicts = Some(big);
        assert_eq!(
            set(&db.conn, &path, conflicts).await.unwrap_err().code,
            "too_large"
        );
        assert!(!path.exists());
        let exact = "x".repeat(MAX_TEXT_BYTES);
        set(&db.conn, &path, change(Scope::Host, None, &exact))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn set_rejects_non_root_workspace() {
        let db = fresh_in_memory_db().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instructions.json");
        let child = seed_worktree_folder(&db, "/w/feature").await;
        let child_row = folder::Entity::find_by_id(child).one(&db.conn).await.unwrap().unwrap();
        let root = root_id(&db, child_row.parent_id.unwrap()).await;
        for bad in [None, Some("nope")] {
            let err = set(&db.conn, &path, change(Scope::Workspace, bad, "t"))
                .await
                .unwrap_err();
            assert_eq!(err.code, if bad.is_none() { "invalid_input" } else { "not_found" });
        }
        // A non-root folder holding a workspace_id is still not a root.
        let mut stray: folder::ActiveModel = child_row.into();
        stray.workspace_id = sea_orm::Set(Some("child-id".into()));
        sea_orm::ActiveModelTrait::update(stray, &db.conn).await.unwrap();
        let err = set(&db.conn, &path, change(Scope::Workspace, Some("child-id"), "t"))
            .await
            .unwrap_err();
        assert_eq!(err.code, "not_found");
        let saved = set(&db.conn, &path, change(Scope::Workspace, Some(&root), "Repo rule."))
            .await
            .unwrap();
        assert_eq!(saved.workspaces[&root].general.as_deref(), Some("Repo rule."));
        assert_eq!(user_instructions::load(&path), saved);
        // A host change may not name a workspace.
        let err = set(&db.conn, &path, change(Scope::Host, Some(&root), "t"))
            .await
            .unwrap_err();
        assert_eq!(err.code, "invalid_input");
    }

    #[tokio::test]
    async fn concurrent_sets_both_persist() {
        let db = fresh_in_memory_db().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instructions.json");
        let root = root_id(&db, seed_folder(&db, "/w/a").await).await;
        let (a, b) = tokio::join!(
            set(&db.conn, &path, change(Scope::Host, None, "Host rule.")),
            set(&db.conn, &path, change(Scope::Workspace, Some(&root), "Repo rule.")),
        );
        a.unwrap();
        b.unwrap();
        let saved = user_instructions::load(&path);
        assert_eq!(saved.host.general.as_deref(), Some("Host rule."));
        assert_eq!(saved.workspaces[&root].general.as_deref(), Some("Repo rule."));
    }

    #[tokio::test]
    async fn set_prunes_removed_workspaces() {
        let db = fresh_in_memory_db().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instructions.json");
        let gone = seed_folder(&db, "/w/gone").await;
        let gone_id = root_id(&db, gone).await;
        let kept_id = root_id(&db, seed_folder(&db, "/w/kept").await).await;
        set(&db.conn, &path, change(Scope::Workspace, Some(&gone_id), "Old."))
            .await
            .unwrap();
        set(&db.conn, &path, change(Scope::Workspace, Some(&kept_id), "Kept."))
            .await
            .unwrap();
        folder_service::soft_delete_folder(&db.conn, gone).await.unwrap();
        let saved = set(&db.conn, &path, change(Scope::Host, None, "Host rule."))
            .await
            .unwrap();
        assert!(!saved.workspaces.contains_key(&gone_id));
        assert_eq!(saved.workspaces[&kept_id].general.as_deref(), Some("Kept."));
        // Clearing a workspace's texts drops its entry.
        let mut clear = change(Scope::Workspace, Some(&kept_id), "  ");
        clear.general = None;
        let saved = set(&db.conn, &path, clear).await.unwrap();
        assert!(saved.workspaces.is_empty());
        assert_eq!(saved.host.general.as_deref(), Some("Host rule."));
    }
}
