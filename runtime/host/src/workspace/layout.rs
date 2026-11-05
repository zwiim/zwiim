//! Zwiim-owned: how the Host's workspaces are organized for display.
//!
//! The layout is one ordered list of groups and ungrouped root workspaces, with
//! each root's live worktrees in order. It is cosmetic: nothing that runs reads
//! it. Removed worktrees are never in it, and clients address everything by
//! opaque id.
//!
//! `sort_order` is a position within a container, so it can't sort a flat list.
//! That is why [`crate::workspace::list`] stays in id order and order reaches
//! clients only through here.

use std::collections::{HashMap, HashSet};

use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::db_err;
use crate::app_error::AppCommandError;
use crate::db::entities::folder::{self, FolderKind};
use crate::db::entities::folder_group;
use crate::db::error::DbError;
use crate::db::service::{folder_group_service, folder_service};
use crate::db::AppDatabase;
use crate::models::{SidebarEntryKind, SidebarLayoutEntry};
use crate::protocol::error::ZwiimError;
use crate::web::event_bridge::{self, EventEmitter, FolderGroupChange};

/// Writes read the rows and write them back: one at a time, so none builds on
/// rows another has changed.
static WRITES: Mutex<()> = Mutex::const_new(());

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct WorkspaceLayout {
    pub entries: Vec<LayoutEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LayoutEntry {
    Group {
        group_id: String,
        name: String,
        workspaces: Vec<LayoutWorkspace>,
    },
    Workspace(LayoutWorkspace),
}

/// A root workspace and its live worktrees, in order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LayoutWorkspace {
    pub workspace_id: String,
    pub worktrees: Vec<String>,
}

/// The layout now. Holds the rows [`crate::workspace::list`] calls live, closed
/// ones included, so a client can match every live workspace to a position.
pub async fn read(conn: &DatabaseConnection) -> Result<WorkspaceLayout, AppCommandError> {
    let (groups, rows) = load(conn).await?;
    Ok(assemble(groups, rows))
}

async fn load(
    conn: &DatabaseConnection,
) -> Result<(Vec<folder_group::Model>, Vec<folder::Model>), AppCommandError> {
    let groups = folder_group::Entity::find()
        .order_by_asc(folder_group::Column::SortOrder)
        .order_by_asc(folder_group::Column::Id)
        .all(conn)
        .await
        .map_err(db_err)?;
    let rows = folder::Entity::find()
        .filter(folder::Column::Kind.eq(FolderKind::Regular))
        .filter(folder::Column::DeletedAt.is_null())
        .filter(folder::Column::WorkspaceId.is_not_null())
        .order_by_asc(folder::Column::SortOrder)
        .order_by_asc(folder::Column::Id)
        .all(conn)
        .await
        .map_err(db_err)?;
    Ok((groups, rows))
}

/// One top-level position. Group and root rows share a `sort_order` sequence;
/// a group goes first on a tie, because a root is only ever appended after one.
type TopLevelKey = (i32, u8, i32);

fn assemble(groups: Vec<folder_group::Model>, rows: Vec<folder::Model>) -> WorkspaceLayout {
    let live: HashSet<i32> = rows.iter().map(|r| r.id).collect();
    let mut worktrees: HashMap<i32, Vec<String>> = HashMap::new();
    for row in &rows {
        if let (Some(parent), Some(id)) = (row.parent_id, &row.workspace_id) {
            if live.contains(&parent) {
                worktrees.entry(parent).or_default().push(id.clone());
            }
        }
    }
    let mut members: HashMap<i32, Vec<LayoutWorkspace>> = HashMap::new();
    let mut top: Vec<(TopLevelKey, LayoutEntry)> = Vec::new();
    let known: HashSet<i32> = groups.iter().map(|g| g.id).collect();
    for row in rows {
        // A worktree whose root is gone stands alone, as `list` reports it.
        if row.parent_id.is_some_and(|p| live.contains(&p)) {
            continue;
        }
        let Some(workspace_id) = row.workspace_id else {
            continue;
        };
        let workspace = LayoutWorkspace {
            workspace_id,
            worktrees: worktrees.remove(&row.id).unwrap_or_default(),
        };
        // `group_id` means nothing on a worktree, and a group can be deleted
        // out from under a soft-deleted row that comes back later.
        match row
            .group_id
            .filter(|g| row.parent_id.is_none() && known.contains(g))
        {
            Some(group) => members.entry(group).or_default().push(workspace),
            None => top.push((
                (row.sort_order, 1, row.id),
                LayoutEntry::Workspace(workspace),
            )),
        }
    }
    for group in groups {
        let Some(group_id) = group.public_id else {
            continue;
        };
        let entry = LayoutEntry::Group {
            group_id,
            name: group.name,
            workspaces: members.remove(&group.id).unwrap_or_default(),
        };
        top.push(((group.sort_order, 0, group.id), entry));
    }
    top.sort_by_key(|(key, _)| *key);
    WorkspaceLayout {
        entries: top.into_iter().map(|(_, entry)| entry).collect(),
    }
}

/// A submitted layout: [`WorkspaceLayout`]'s entries without group names.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LayoutEntryInput {
    Group {
        group_id: String,
        workspaces: Vec<LayoutWorkspaceInput>,
    },
    Workspace(LayoutWorkspaceInput),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutWorkspaceInput {
    pub workspace_id: String,
    pub worktrees: Vec<String>,
}

impl LayoutEntryInput {
    fn ids(&self) -> Vec<&String> {
        let (own, workspaces) = match self {
            Self::Group {
                group_id,
                workspaces,
            } => (Some(group_id), workspaces.as_slice()),
            Self::Workspace(workspace) => (None, std::slice::from_ref(workspace)),
        };
        own.into_iter()
            .chain(
                workspaces
                    .iter()
                    .flat_map(|w| std::iter::once(&w.workspace_id).chain(&w.worktrees)),
            )
            .collect()
    }
}

/// A group, as clients know it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkspaceGroup {
    pub group_id: String,
    pub name: String,
}

const MAX_GROUP_NAME_CHARS: usize = 64;

/// A name people can tell from the others in a picker. Refused rather than
/// cut short, so what the user typed is never quietly changed.
async fn group_name(
    conn: &DatabaseConnection,
    name: &str,
    renaming: Option<i32>,
) -> Result<String, ZwiimError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ZwiimError::invalid_input("Give the group a name"));
    }
    if name.chars().count() > MAX_GROUP_NAME_CHARS {
        return Err(ZwiimError::invalid_input(
            "A group name can have at most 64 characters",
        ));
    }
    let taken = folder_group::Entity::find()
        .all(conn)
        .await
        .map_err(db_err)?
        .into_iter()
        .any(|g| Some(g.id) != renaming && g.name.to_lowercase() == name.to_lowercase());
    if taken {
        return Err(ZwiimError::invalid_input(
            "There is already a group with this name",
        ));
    }
    Ok(name.to_string())
}

async fn find_group(
    conn: &DatabaseConnection,
    group_id: &str,
) -> Result<folder_group::Model, ZwiimError> {
    folder_group::Entity::find()
        .filter(folder_group::Column::PublicId.eq(group_id))
        .one(conn)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ZwiimError::not_found("Group not found"))
}

async fn group_info(conn: &DatabaseConnection, id: i32) -> Result<WorkspaceGroup, ZwiimError> {
    let group = folder_group::Entity::find_by_id(id)
        .one(conn)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ZwiimError::not_found("Group not found"))?;
    Ok(WorkspaceGroup {
        group_id: group.public_id.ok_or_else(ZwiimError::internal)?,
        name: group.name,
    })
}

/// Tell the Host feed the layout moved. The payload is a bare nudge: the other
/// variants carry internal ids, and the feed re-reads either way.
fn announce(emitter: &EventEmitter) {
    event_bridge::emit_event(
        emitter,
        event_bridge::FOLDER_GROUP_CHANGED_EVENT,
        FolderGroupChange::Layout,
    );
}

pub async fn create_group(
    db: &AppDatabase,
    emitter: &EventEmitter,
    name: &str,
) -> Result<WorkspaceGroup, ZwiimError> {
    let name = group_name(&db.conn, name, None).await?;
    let group = folder_group_service::create_folder_group(&db.conn, name, None)
        .await
        .map_err(AppCommandError::from)?;
    let group = group_info(&db.conn, group.id).await?;
    announce(emitter);
    Ok(group)
}

pub async fn rename_group(
    db: &AppDatabase,
    emitter: &EventEmitter,
    group_id: &str,
    name: &str,
) -> Result<WorkspaceGroup, ZwiimError> {
    let id = find_group(&db.conn, group_id).await?.id;
    let name = group_name(&db.conn, name, Some(id)).await?;
    folder_group_service::update_folder_group(&db.conn, id, Some(name), None)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| ZwiimError::not_found("Group not found"))?;
    let group = group_info(&db.conn, id).await?;
    announce(emitter);
    Ok(group)
}

/// Delete a group, leaving its workspaces where the group was. The inherited
/// service appends them to the end and forgets soft-deleted members, which
/// would then come back into a group that no longer exists.
pub async fn delete_group(
    db: &AppDatabase,
    emitter: &EventEmitter,
    group_id: &str,
) -> Result<(), ZwiimError> {
    let _write = WRITES.lock().await;
    let id = find_group(&db.conn, group_id).await?.id;
    let (groups, rows) = load(&db.conn).await?;
    let ids = RowIds::new(&groups, &rows);
    let layout = assemble(groups, rows);
    let entries = layout
        .entries
        .into_iter()
        .flat_map(|entry| match entry {
            LayoutEntry::Group {
                group_id: other,
                workspaces,
                ..
            } if other == group_id => workspaces.into_iter().map(LayoutEntry::Workspace).collect(),
            other => vec![other],
        })
        .collect();

    let txn = db.conn.begin().await.map_err(db_err)?;
    write(&txn, &WorkspaceLayout { entries }, &ids)
        .await
        .map_err(AppCommandError::from)?;
    folder::Entity::update_many()
        .col_expr(folder::Column::GroupId, Expr::value(Option::<i32>::None))
        .filter(folder::Column::GroupId.eq(id))
        .exec(&txn)
        .await
        .map_err(db_err)?;
    folder_group::Entity::delete_by_id(id)
        .exec(&txn)
        .await
        .map_err(db_err)?;
    txn.commit().await.map_err(db_err)?;
    announce(emitter);
    Ok(())
}

/// Save a submitted layout. See [`reconcile`] for how it meets the rows there
/// are now. All positions go in one transaction, and the one announcement
/// follows the commit, worktree-only moves included.
pub async fn set(
    db: &AppDatabase,
    emitter: &EventEmitter,
    entries: Vec<LayoutEntryInput>,
) -> Result<(), ZwiimError> {
    let _write = WRITES.lock().await;
    let (groups, rows) = load(&db.conn).await?;
    let ids = RowIds::new(&groups, &rows);
    let has_parent: HashSet<String> = rows
        .iter()
        .filter(|r| r.parent_id.is_some())
        .filter_map(|r| r.workspace_id.clone())
        .collect();
    let layout = reconcile(&assemble(groups, rows), &has_parent, entries)?;

    let txn = db.conn.begin().await.map_err(db_err)?;
    write(&txn, &layout, &ids)
        .await
        .map_err(AppCommandError::from)?;
    txn.commit().await.map_err(db_err)?;
    announce(emitter);
    Ok(())
}

fn workspaces(layout: &WorkspaceLayout) -> impl Iterator<Item = &LayoutWorkspace> {
    layout.entries.iter().flat_map(|entry| match entry {
        LayoutEntry::Group { workspaces, .. } => workspaces.as_slice(),
        LayoutEntry::Workspace(workspace) => std::slice::from_ref(workspace),
    })
}

/// Fit a submitted layout to the rows there are now. Agents add and remove
/// worktrees while someone drags, so a request that was right when it left must
/// still land: ids that are gone drop out, rows it doesn't name go to the end of
/// their container, and a vanished group's members take its slot. Only a
/// request that could never have been right is refused, and nothing is written.
fn reconcile(
    current: &WorkspaceLayout,
    has_parent: &HashSet<String>,
    entries: Vec<LayoutEntryInput>,
) -> Result<WorkspaceLayout, ZwiimError> {
    let mut seen = HashSet::new();
    for id in entries.iter().flat_map(LayoutEntryInput::ids) {
        if !seen.insert(id) {
            return Err(ZwiimError::invalid_input("The layout lists a row twice"));
        }
    }
    let roots: HashMap<&str, &LayoutWorkspace> = workspaces(current)
        .map(|w| (w.workspace_id.as_str(), w))
        .collect();
    let owners: HashMap<&str, &str> = workspaces(current)
        .flat_map(|w| {
            w.worktrees
                .iter()
                .map(|t| (t.as_str(), w.workspace_id.as_str()))
        })
        .collect();
    let names: HashMap<&str, &str> = current
        .entries
        .iter()
        .filter_map(|entry| match entry {
            LayoutEntry::Group { group_id, name, .. } => Some((group_id.as_str(), name.as_str())),
            LayoutEntry::Workspace(_) => None,
        })
        .collect();

    let settle = |input: &LayoutWorkspaceInput, grouped: bool| {
        let id = input.workspace_id.as_str();
        if owners.contains_key(id) || (grouped && has_parent.contains(id)) {
            return Err(ZwiimError::invalid_input(
                "A worktree stays under its repository",
            ));
        }
        let Some(root) = roots.get(id) else {
            return Ok(None);
        };
        let mut worktrees = Vec::new();
        for worktree in &input.worktrees {
            match owners.get(worktree.as_str()) {
                Some(owner) if *owner == id => worktrees.push(worktree.clone()),
                Some(_) => {
                    return Err(ZwiimError::invalid_input(
                        "A worktree stays under its repository",
                    ))
                }
                None if roots.contains_key(worktree.as_str()) => {
                    return Err(ZwiimError::invalid_input(
                        "A repository cannot go under another",
                    ))
                }
                None => {}
            }
        }
        let unlisted: Vec<String> = root
            .worktrees
            .iter()
            .filter(|t| !worktrees.contains(t))
            .cloned()
            .collect();
        worktrees.extend(unlisted);
        Ok(Some(LayoutWorkspace {
            workspace_id: id.to_string(),
            worktrees,
        }))
    };

    let mut next = Vec::new();
    for entry in &entries {
        match entry {
            LayoutEntryInput::Group {
                group_id,
                workspaces: submitted,
            } => {
                let name = names.get(group_id.as_str());
                let mut members = Vec::new();
                for workspace in submitted {
                    members.extend(settle(workspace, name.is_some())?);
                }
                match name {
                    Some(name) => next.push(LayoutEntry::Group {
                        group_id: group_id.clone(),
                        name: name.to_string(),
                        workspaces: members,
                    }),
                    None => next.extend(members.into_iter().map(LayoutEntry::Workspace)),
                }
            }
            LayoutEntryInput::Workspace(workspace) => {
                next.extend(settle(workspace, false)?.map(LayoutEntry::Workspace));
            }
        }
    }

    let mut next = WorkspaceLayout { entries: next };
    let placed: HashSet<String> = workspaces(&next).map(|w| w.workspace_id.clone()).collect();
    let is_unlisted = |w: &&LayoutWorkspace| !placed.contains(&w.workspace_id);
    for entry in &current.entries {
        match entry {
            LayoutEntry::Workspace(workspace) => {
                if !placed.contains(&workspace.workspace_id) {
                    next.entries.push(entry.clone());
                }
            }
            LayoutEntry::Group {
                group_id,
                name,
                workspaces: members,
            } => {
                let unlisted = members.iter().filter(is_unlisted).cloned();
                let listed = next.entries.iter_mut().find_map(|e| match e {
                    LayoutEntry::Group {
                        group_id: other,
                        workspaces,
                        ..
                    } if other == group_id => Some(workspaces),
                    _ => None,
                });
                match listed {
                    Some(workspaces) => workspaces.extend(unlisted),
                    None => {
                        let workspaces = unlisted.collect();
                        next.entries.push(LayoutEntry::Group {
                            group_id: group_id.clone(),
                            name: name.clone(),
                            workspaces,
                        });
                    }
                }
            }
        }
    }
    Ok(next)
}

/// Internal rows behind the opaque ids of one [`load`].
struct RowIds {
    groups: HashMap<String, i32>,
    folders: HashMap<String, i32>,
}

impl RowIds {
    fn new(groups: &[folder_group::Model], rows: &[folder::Model]) -> Self {
        Self {
            groups: groups
                .iter()
                .filter_map(|g| Some((g.public_id.clone()?, g.id)))
                .collect(),
            folders: rows
                .iter()
                .filter_map(|r| Some((r.workspace_id.clone()?, r.id)))
                .collect(),
        }
    }
}

/// Number every container in `layout` from 1. Every id in it came from the
/// [`load`] that built `ids`.
async fn write<C: ConnectionTrait>(
    conn: &C,
    layout: &WorkspaceLayout,
    ids: &RowIds,
) -> Result<(), DbError> {
    let mut sidebar = Vec::new();
    let mut worktrees = Vec::new();
    for entry in &layout.entries {
        let (group_id, members) = match entry {
            LayoutEntry::Workspace(workspace) => (None, std::slice::from_ref(workspace)),
            LayoutEntry::Group {
                group_id,
                workspaces,
                ..
            } => {
                let id = ids.groups[group_id];
                sidebar.push(SidebarLayoutEntry {
                    kind: SidebarEntryKind::Group,
                    id,
                    group_id: None,
                });
                (Some(id), workspaces.as_slice())
            }
        };
        for workspace in members {
            sidebar.push(SidebarLayoutEntry {
                kind: SidebarEntryKind::Folder,
                id: ids.folders[&workspace.workspace_id],
                group_id,
            });
            for (order, worktree) in (1..).zip(&workspace.worktrees) {
                worktrees.push((ids.folders[worktree], order, None));
            }
        }
    }
    folder_group_service::apply_sidebar_layout(conn, sidebar).await?;
    folder_service::assign_folder_positions(conn, &worktrees).await
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use sea_orm::sea_query::Expr;
    use sea_orm::{ActiveModelTrait, ActiveValue::NotSet, ConnectionTrait, Set, Statement};
    use sea_orm_migration::MigratorTrait;

    use super::*;
    use crate::db::migration::Migrator;
    use crate::db::service::{folder_group_service, folder_service};
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
    use crate::db::AppDatabase;
    use crate::protocol::events::tests::assert_no_internal_keys;
    use crate::web::event_bridge::WebEventBroadcaster;

    async fn workspace_id(db: &AppDatabase, id: i32) -> String {
        folder::Entity::find_by_id(id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap()
            .workspace_id
            .unwrap()
    }

    async fn set_position(db: &AppDatabase, id: i32, sort_order: i32, group_id: Option<i32>) {
        folder::Entity::update_many()
            .col_expr(folder::Column::SortOrder, Expr::value(sort_order))
            .col_expr(folder::Column::GroupId, Expr::value(group_id))
            .filter(folder::Column::Id.eq(id))
            .exec(&db.conn)
            .await
            .unwrap();
    }

    fn bare(workspace_id: &str) -> LayoutEntry {
        LayoutEntry::Workspace(LayoutWorkspace {
            workspace_id: workspace_id.to_string(),
            worktrees: vec![],
        })
    }

    #[tokio::test]
    async fn top_level_interleaves_groups_and_roots_by_position() {
        let db = fresh_in_memory_db().await;
        let a = seed_folder(&db, "/a").await;
        let b = seed_folder(&db, "/b").await;
        let c = seed_folder(&db, "/c").await;
        let group = folder_group_service::create_folder_group(&db.conn, "backend".into(), None)
            .await
            .unwrap();
        set_position(&db, c, 1, None).await;
        set_position(&db, a, 2, Some(group.id)).await;
        set_position(&db, b, 1, Some(group.id)).await;
        let layout = read(&db.conn).await.unwrap();
        let [first, second] = layout.entries.as_slice() else {
            panic!("{layout:?}");
        };
        assert_eq!(first, &bare(&workspace_id(&db, c).await));
        let LayoutEntry::Group {
            name, workspaces, ..
        } = second
        else {
            panic!("{second:?}");
        };
        assert_eq!(name, "backend");
        let ids: Vec<&str> = workspaces.iter().map(|w| w.workspace_id.as_str()).collect();
        assert_eq!(
            ids,
            [workspace_id(&db, b).await, workspace_id(&db, a).await]
        );
    }

    #[tokio::test]
    async fn sort_order_ties_are_stable() {
        let db = fresh_in_memory_db().await;
        let a = seed_folder(&db, "/a").await;
        let b = seed_folder(&db, "/b").await;
        let group = folder_group_service::create_folder_group(&db.conn, "g".into(), None)
            .await
            .unwrap();
        set_position(&db, a, group.sort_order, None).await;
        set_position(&db, b, group.sort_order, None).await;
        let layout = read(&db.conn).await.unwrap();
        assert!(matches!(layout.entries[0], LayoutEntry::Group { .. }));
        assert_eq!(layout.entries[1], bare(&workspace_id(&db, a).await));
        assert_eq!(layout.entries[2], bare(&workspace_id(&db, b).await));
    }

    #[tokio::test]
    async fn worktrees_follow_their_root_in_order_and_ignore_group_id() {
        let db = fresh_in_memory_db().await;
        let root = seed_folder(&db, "/repo").await;
        let one = folder_service::add_folder_with_parent(&db.conn, "/repo-1", Some(root))
            .await
            .unwrap()
            .id;
        let two = folder_service::add_folder_with_parent(&db.conn, "/repo-2", Some(root))
            .await
            .unwrap()
            .id;
        let group = folder_group_service::create_folder_group(&db.conn, "g".into(), None)
            .await
            .unwrap();
        set_position(&db, one, 9, Some(group.id)).await;
        set_position(&db, two, 3, None).await;
        let layout = read(&db.conn).await.unwrap();
        let root_entry = layout
            .entries
            .iter()
            .find(|e| matches!(e, LayoutEntry::Workspace(_)))
            .unwrap();
        let LayoutEntry::Workspace(root_entry) = root_entry else {
            unreachable!();
        };
        assert_eq!(root_entry.workspace_id, workspace_id(&db, root).await);
        assert_eq!(
            root_entry.worktrees,
            [workspace_id(&db, two).await, workspace_id(&db, one).await]
        );
    }

    #[tokio::test]
    async fn closed_rows_stay_and_removed_rows_go() {
        let db = fresh_in_memory_db().await;
        let closed = seed_folder(&db, "/closed").await;
        let root = seed_folder(&db, "/repo").await;
        let removed = folder_service::add_folder_with_parent(&db.conn, "/repo-1", Some(root))
            .await
            .unwrap()
            .id;
        folder::Entity::update_many()
            .col_expr(folder::Column::IsOpen, Expr::value(false))
            .filter(folder::Column::Id.eq(closed))
            .exec(&db.conn)
            .await
            .unwrap();
        folder_service::soft_delete_folder(&db.conn, removed)
            .await
            .unwrap();
        let layout = read(&db.conn).await.unwrap();
        assert_eq!(
            layout.entries,
            [
                bare(&workspace_id(&db, closed).await),
                bare(&workspace_id(&db, root).await)
            ]
        );
    }

    #[tokio::test]
    async fn a_worktree_with_a_soft_deleted_root_stands_alone() {
        let db = fresh_in_memory_db().await;
        let root = seed_folder(&db, "/repo").await;
        let orphan = folder_service::add_folder_with_parent(&db.conn, "/repo-1", Some(root))
            .await
            .unwrap()
            .id;
        let group = folder_group_service::create_folder_group(&db.conn, "g".into(), None)
            .await
            .unwrap();
        set_position(&db, orphan, 5, Some(group.id)).await;
        // Direct, as `soft_delete_folder` on a root takes its worktrees with it.
        folder::Entity::update_many()
            .col_expr(folder::Column::DeletedAt, Expr::value(Utc::now()))
            .filter(folder::Column::Id.eq(root))
            .exec(&db.conn)
            .await
            .unwrap();
        let layout = read(&db.conn).await.unwrap();
        assert!(layout
            .entries
            .contains(&bare(&workspace_id(&db, orphan).await)));
        let LayoutEntry::Group { workspaces, .. } = &layout.entries[0] else {
            panic!("{layout:?}");
        };
        assert!(workspaces.is_empty());
    }

    #[tokio::test]
    async fn a_dangling_group_id_falls_back_to_top_level() {
        let db = fresh_in_memory_db().await;
        let a = seed_folder(&db, "/a").await;
        let group = folder_group_service::create_folder_group(&db.conn, "g".into(), None)
            .await
            .unwrap();
        set_position(&db, a, 1, Some(group.id)).await;
        let groups = vec![];
        let rows = folder::Entity::find().all(&db.conn).await.unwrap();
        assert_eq!(
            assemble(groups, rows).entries,
            [bare(&workspace_id(&db, a).await)]
        );
    }

    #[tokio::test]
    async fn the_wire_shape_carries_only_opaque_ids() {
        let db = fresh_in_memory_db().await;
        let a = seed_folder(&db, "/a").await;
        let group = folder_group_service::create_folder_group(&db.conn, "g".into(), None)
            .await
            .unwrap();
        set_position(&db, a, 1, Some(group.id)).await;
        let value = serde_json::to_value(read(&db.conn).await.unwrap()).unwrap();
        assert_no_internal_keys(&value);
        let entry = &value["entries"][0];
        assert_eq!(entry["kind"], "group");
        assert!(entry["group_id"].is_string(), "{entry}");
        assert_eq!(
            entry["workspaces"][0]["workspace_id"],
            workspace_id(&db, a).await
        );
    }

    #[tokio::test]
    async fn a_new_group_mints_its_own_id() {
        let db = fresh_in_memory_db().await;
        for name in ["a", "b"] {
            folder_group_service::create_folder_group(&db.conn, name.into(), None)
                .await
                .unwrap();
        }
        let ids: std::collections::HashSet<_> = folder_group::Entity::find()
            .all(&db.conn)
            .await
            .unwrap()
            .into_iter()
            .map(|g| g.public_id.expect("minted"))
            .collect();
        assert_eq!(ids.len(), 2);
    }

    /// An upgrade keeps the list as people saw it: by id, not the recency order
    /// a row adopted from an older database still carries.
    #[tokio::test]
    async fn the_migration_backfills_ids_and_keeps_adopted_rows_in_id_order() {
        let db = fresh_in_memory_db().await;
        let backend = db.conn.get_database_backend();
        Migrator::down(&db.conn, Some(1)).await.unwrap();
        let now = Utc::now();
        for (order, path) in [(3, "/oldest"), (2, "/middle"), (1, "/newest")] {
            folder::ActiveModel {
                id: NotSet,
                name: Set(path.to_string()),
                path: Set(path.to_string()),
                git_branch: Set(None),
                default_agent_type: Set(None),
                last_opened_at: Set(now),
                created_at: Set(now),
                updated_at: Set(now),
                deleted_at: Set(None),
                is_open: Set(true),
                sort_order: Set(order),
                color: Set("inherit".to_string()),
                parent_id: Set(None),
                kind: Set(FolderKind::Regular),
                alias: Set(None),
                group_id: Set(None),
                workspace_id: NotSet,
                base_branch: NotSet,
                base_commit: NotSet,
                port: NotSet,
            }
            .insert(&db.conn)
            .await
            .unwrap();
        }
        for name in ["a", "b"] {
            db.conn
                .execute(Statement::from_sql_and_values(
                    backend,
                    "INSERT INTO folder_group (name, color, sort_order, created_at, updated_at) \
                     VALUES (?, 'inherit', 9, ?, ?)",
                    [name.into(), now.into(), now.into()],
                ))
                .await
                .unwrap();
        }
        Migrator::up(&db.conn, None).await.unwrap();

        let groups = folder_group::Entity::find().all(&db.conn).await.unwrap();
        let ids: std::collections::HashSet<_> = groups
            .into_iter()
            .map(|g| g.public_id.expect("backfilled"))
            .collect();
        assert_eq!(ids.len(), 2);
        let rows = folder::Entity::find()
            .order_by_asc(folder::Column::Id)
            .all(&db.conn)
            .await
            .unwrap();
        assert!(rows.iter().all(|r| r.sort_order == r.id));
        assert_eq!(rows[0].path, "/oldest");
    }

    fn input(value: serde_json::Value) -> Vec<LayoutEntryInput> {
        serde_json::from_value(value).expect("a well-formed layout")
    }

    fn top(id: &str) -> serde_json::Value {
        serde_json::json!({"kind": "workspace", "workspace_id": id, "worktrees": []})
    }

    async fn seed_group(db: &AppDatabase, name: &str) -> String {
        create_group(db, &EventEmitter::Noop, name)
            .await
            .unwrap()
            .group_id
    }

    async fn order(db: &AppDatabase) -> Vec<String> {
        read(&db.conn)
            .await
            .unwrap()
            .entries
            .iter()
            .map(|entry| match entry {
                LayoutEntry::Group {
                    name, workspaces, ..
                } => format!("{name}:{}", workspaces.len()),
                LayoutEntry::Workspace(w) => w.workspace_id.clone(),
            })
            .collect()
    }

    #[tokio::test]
    async fn group_names_are_trimmed_capped_and_unique() {
        let db = fresh_in_memory_db().await;
        let emitter = EventEmitter::Noop;

        let group = create_group(&db, &emitter, "  backend  ").await.unwrap();
        assert_eq!(group.name, "backend");

        let at_cap = "x".repeat(64);
        assert!(create_group(&db, &emitter, &at_cap).await.is_ok());
        for refused in ["", "   ", &"y".repeat(65), "BACKEND", " Backend "] {
            let err = create_group(&db, &emitter, refused).await.unwrap_err();
            assert_eq!(err.code, "invalid_input", "{refused:?}");
        }
    }

    #[tokio::test]
    async fn renaming_keeps_the_id_and_may_change_only_case() {
        let db = fresh_in_memory_db().await;
        let emitter = EventEmitter::Noop;
        let backend = seed_group(&db, "backend").await;
        seed_group(&db, "frontend").await;

        let renamed = rename_group(&db, &emitter, &backend, "Backend")
            .await
            .unwrap();
        assert_eq!(renamed.group_id, backend);
        assert_eq!(renamed.name, "Backend");

        let taken = rename_group(&db, &emitter, &backend, "FRONTEND")
            .await
            .unwrap_err();
        assert_eq!(taken.code, "invalid_input");
        let gone = rename_group(&db, &emitter, "missing", "other")
            .await
            .unwrap_err();
        assert_eq!(gone.code, "not_found");
        let gone = delete_group(&db, &emitter, "missing").await.unwrap_err();
        assert_eq!(gone.code, "not_found");
    }

    #[tokio::test]
    async fn deleting_a_group_leaves_its_workspaces_in_its_slot() {
        let db = fresh_in_memory_db().await;
        let emitter = EventEmitter::Noop;
        let first = seed_folder(&db, "/repo/first").await;
        let inside = seed_folder(&db, "/repo/inside").await;
        let last = seed_folder(&db, "/repo/last").await;
        let hidden = seed_folder(&db, "/repo/hidden").await;
        let (first, inside, last, hidden_id) = (
            workspace_id(&db, first).await,
            workspace_id(&db, inside).await,
            workspace_id(&db, last).await,
            hidden,
        );
        let group = seed_group(&db, "backend").await;
        let internal = find_group(&db.conn, &group).await.unwrap().id;
        set(
            &db,
            &emitter,
            input(serde_json::json!([
                top(&first),
                {"kind": "group", "group_id": group, "workspaces": [
                    {"workspace_id": inside, "worktrees": []},
                ]},
                top(&last),
            ])),
        )
        .await
        .unwrap();
        set_position(&db, hidden_id, 9, Some(internal)).await;
        folder_service::soft_delete_folder(&db.conn, hidden_id)
            .await
            .unwrap();

        delete_group(&db, &emitter, &group).await.unwrap();

        assert_eq!(order(&db).await, vec![first, inside, last]);
        let hidden = folder::Entity::find_by_id(hidden_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(hidden.group_id, None);
    }

    #[tokio::test]
    async fn a_stale_request_lands_on_the_rows_there_are_now() {
        let db = fresh_in_memory_db().await;
        let emitter = EventEmitter::Noop;
        let a = seed_folder(&db, "/repo/a").await;
        let b = seed_folder(&db, "/repo/b").await;
        let c = seed_folder(&db, "/repo/c").await;
        let (a, b, c) = (
            workspace_id(&db, a).await,
            workspace_id(&db, b).await,
            workspace_id(&db, c).await,
        );
        let kept = seed_group(&db, "kept").await;

        // `b` and the group went unmentioned; `gone` and `vanished` are stale.
        set(
            &db,
            &emitter,
            input(serde_json::json!([
                top("gone"),
                {"kind": "group", "group_id": "vanished", "workspaces": [
                    {"workspace_id": c, "worktrees": ["gone-too"]},
                    {"workspace_id": "gone-three", "worktrees": []},
                ]},
                top(&a),
            ])),
        )
        .await
        .unwrap();

        assert_eq!(order(&db).await, vec![c, a, b, "kept:0".to_string()]);
        assert!(find_group(&db.conn, &kept).await.is_ok());
    }

    #[tokio::test]
    async fn a_move_racing_a_group_delete_leaves_no_member_of_a_gone_group() {
        // Each round starts the move a few queries later, to land it across
        // every step of the delete.
        for head_start in 0..8 {
            let db = fresh_in_memory_db().await;
            let emitter = EventEmitter::Noop;
            let row = seed_folder(&db, "/repo/a").await;
            let a = workspace_id(&db, row).await;
            let group = seed_group(&db, "backend").await;
            let entries = input(serde_json::json!([
                {"kind": "group", "group_id": group, "workspaces": [
                    {"workspace_id": a, "worktrees": []},
                ]},
            ]));

            let (moved, deleted) = tokio::join!(
                async {
                    for _ in 0..head_start {
                        read(&db.conn).await.unwrap();
                    }
                    set(&db, &emitter, entries).await
                },
                delete_group(&db, &emitter, &group)
            );
            moved.unwrap();
            deleted.unwrap();

            let member = folder::Entity::find_by_id(row).one(&db.conn).await.unwrap();
            assert_eq!(member.unwrap().group_id, None, "head start {head_start}");
        }
    }

    #[tokio::test]
    async fn unlisted_members_stay_at_the_end_of_their_group() {
        let db = fresh_in_memory_db().await;
        let emitter = EventEmitter::Noop;
        let a = seed_folder(&db, "/repo/a").await;
        let b = seed_folder(&db, "/repo/b").await;
        let (a_id, b_id) = (workspace_id(&db, a).await, workspace_id(&db, b).await);
        let group = seed_group(&db, "g").await;
        let internal = find_group(&db.conn, &group).await.unwrap().id;
        set_position(&db, a, 1, Some(internal)).await;
        set_position(&db, b, 2, Some(internal)).await;

        // Moved the group's order only, naming one member.
        set(
            &db,
            &emitter,
            input(serde_json::json!([
                {"kind": "group", "group_id": group, "workspaces": [
                    {"workspace_id": b_id, "worktrees": []},
                ]},
            ])),
        )
        .await
        .unwrap();

        let LayoutEntry::Group { workspaces, .. } = &read(&db.conn).await.unwrap().entries[0]
        else {
            panic!("expected the group");
        };
        let members: Vec<&str> = workspaces.iter().map(|w| &*w.workspace_id).collect();
        assert_eq!(members, vec![b_id.as_str(), a_id.as_str()]);
    }

    #[tokio::test]
    async fn structural_mistakes_write_nothing() {
        let db = fresh_in_memory_db().await;
        let emitter = EventEmitter::Noop;
        let a = seed_folder(&db, "/repo/a").await;
        let b = seed_folder(&db, "/repo/b").await;
        let worktree = folder_service::add_folder_with_parent(&db.conn, "/repo/a-wt", Some(a))
            .await
            .unwrap();
        let a = workspace_id(&db, a).await;
        let b = workspace_id(&db, b).await;
        let worktree = workspace_id(&db, worktree.id).await;
        let group = seed_group(&db, "g").await;
        let before = read(&db.conn).await.unwrap();

        let cases = [
            ("listed twice", serde_json::json!([top(&a), top(&a)])),
            (
                "stale id listed twice",
                serde_json::json!([top("gone"), top("gone")]),
            ),
            ("worktree at top level", serde_json::json!([top(&worktree)])),
            (
                "worktree in a group",
                serde_json::json!([{"kind": "group", "group_id": group, "workspaces": [
                    {"workspace_id": worktree, "worktrees": []},
                ]}]),
            ),
            (
                "worktree under the wrong repository",
                serde_json::json!([{"kind": "workspace", "workspace_id": b, "worktrees": [worktree]}]),
            ),
            (
                "repository under a repository",
                serde_json::json!([{"kind": "workspace", "workspace_id": a, "worktrees": [b]}]),
            ),
        ];
        for (case, entries) in cases {
            let err = set(&db, &emitter, input(entries)).await.unwrap_err();
            assert_eq!(err.code, "invalid_input", "{case}");
            assert_eq!(read(&db.conn).await.unwrap(), before, "{case}");
        }
    }

    #[test]
    fn nesting_and_names_are_unknown_fields() {
        let cases = [
            serde_json::json!([{"kind": "group", "group_id": "g", "name": "n", "workspaces": []}]),
            serde_json::json!([{"kind": "group", "group_id": "g", "workspaces": [
                {"kind": "group", "group_id": "h", "workspaces": []},
            ]}]),
            serde_json::json!([{"kind": "group", "group_id": "g", "workspaces": [
                {"workspace_id": "w", "worktrees": [], "nested": true},
            ]}]),
            serde_json::json!([{"kind": "workspace", "workspace_id": "w", "worktrees": [
                {"workspace_id": "x", "worktrees": []},
            ]}]),
        ];
        for entries in cases {
            let parsed = serde_json::from_value::<Vec<LayoutEntryInput>>(entries.clone());
            assert!(parsed.is_err(), "{entries}");
        }
    }

    #[tokio::test]
    async fn a_failed_write_keeps_the_old_layout() {
        let db = fresh_in_memory_db().await;
        let emitter = EventEmitter::Noop;
        let a = seed_folder(&db, "/repo/a").await;
        let b = seed_folder(&db, "/repo/b").await;
        let (a_id, b_id) = (workspace_id(&db, a).await, workspace_id(&db, b).await);
        let group = seed_group(&db, "g").await;
        let internal = find_group(&db.conn, &group).await.unwrap().id;
        set_position(&db, a, 1, Some(internal)).await;
        let before = read(&db.conn).await.unwrap();

        // Folders are written first, so each of these fails halfway through.
        for operation in ["UPDATE", "DELETE"] {
            db.conn
                .execute(Statement::from_string(
                    db.conn.get_database_backend(),
                    format!(
                        "CREATE TRIGGER refuse_group_{operation} BEFORE {operation} \
                         ON folder_group BEGIN SELECT RAISE(ABORT, 'refused'); END"
                    ),
                ))
                .await
                .unwrap();
        }

        let reordered = serde_json::json!([
            {"kind": "group", "group_id": group, "workspaces": [
                {"workspace_id": b_id, "worktrees": []},
            ]},
            top(&a_id),
        ]);
        assert!(set(&db, &emitter, input(reordered)).await.is_err());
        assert_eq!(read(&db.conn).await.unwrap(), before);
        assert!(delete_group(&db, &emitter, &group).await.is_err());
        assert_eq!(read(&db.conn).await.unwrap(), before);
    }

    #[tokio::test]
    async fn every_write_announces_once_after_it_commits() {
        let db = fresh_in_memory_db().await;
        let broadcaster = std::sync::Arc::new(WebEventBroadcaster::new());
        let emitter = EventEmitter::test_web_only(broadcaster.clone());
        let mut events = broadcaster.subscribe();
        let a = seed_folder(&db, "/repo/a").await;
        let worktree = folder_service::add_folder_with_parent(&db.conn, "/repo/a-wt", Some(a))
            .await
            .unwrap();
        let other = folder_service::add_folder_with_parent(&db.conn, "/repo/a-wt2", Some(a))
            .await
            .unwrap();
        let (a, worktree, other) = (
            workspace_id(&db, a).await,
            workspace_id(&db, worktree.id).await,
            workspace_id(&db, other.id).await,
        );

        let group = create_group(&db, &emitter, "g").await.unwrap().group_id;
        rename_group(&db, &emitter, &group, "h").await.unwrap();
        // Only worktrees move: nothing else would tell the feed.
        set(
            &db,
            &emitter,
            input(serde_json::json!([
                {"kind": "workspace", "workspace_id": a, "worktrees": [other, worktree]},
            ])),
        )
        .await
        .unwrap();
        delete_group(&db, &emitter, &group).await.unwrap();
        assert!(
            set(&db, &emitter, input(serde_json::json!([top(&worktree)])))
                .await
                .is_err()
        );

        for _ in 0..4 {
            let event = events.try_recv().expect("one event per write");
            assert_eq!(event.channel, event_bridge::FOLDER_GROUP_CHANGED_EVENT);
            let text = event.payload.to_string();
            assert!(!text.contains("\"id\""), "no internal ids: {text}");
        }
        assert!(events.try_recv().is_err(), "and no more");
    }

    #[tokio::test]
    async fn worktree_order_is_saved_within_its_repository() {
        let db = fresh_in_memory_db().await;
        let root = seed_folder(&db, "/repo/a").await;
        let one = folder_service::add_folder_with_parent(&db.conn, "/repo/one", Some(root))
            .await
            .unwrap();
        let two = folder_service::add_folder_with_parent(&db.conn, "/repo/two", Some(root))
            .await
            .unwrap();
        let (root, one, two) = (
            workspace_id(&db, root).await,
            workspace_id(&db, one.id).await,
            workspace_id(&db, two.id).await,
        );

        set(
            &db,
            &EventEmitter::Noop,
            input(serde_json::json!([
                {"kind": "workspace", "workspace_id": root, "worktrees": [two, one]},
            ])),
        )
        .await
        .unwrap();

        let layout = read(&db.conn).await.unwrap();
        let LayoutEntry::Workspace(workspace) = &layout.entries[0] else {
            panic!("expected the repository");
        };
        assert_eq!(workspace.worktrees, vec![two, one]);
    }
}
