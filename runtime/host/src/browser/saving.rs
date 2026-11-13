//! Where the Host browser saves downloads (design, decision 13).
//!
//! A session's tab saves to `<worktree>/.zwiim/browser/<id>/`, `<id>` being
//! the session id's first 8 characters, unless another session in that
//! worktree shares them; the folder goes with the session. The person may
//! choose a root folder instead (the Host setting `browser_download_root`),
//! which every worktree follows: `<root>/zwiim/<repo>/downloads/`, never
//! deleted by Zwiim. The person's own tabs save to `~/Downloads`.
//!
//! The engine saves each download under its guid and the Host gives it the
//! page's file name once it is complete ([`free_name`]).

use std::path::{Path, PathBuf};

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use super::store::TabOwner;
use crate::db::entities::{conversation, folder};
use crate::db::error::DbError;
use crate::db::service::app_metadata_service;

const ROOT_KEY: &str = "browser_download_root";
/// Characters of the session id that name its folder.
const SHORT_ID: usize = 8;
/// Longest file name the Host gives a download, in characters.
const MAX_NAME: usize = 200;

/// The root folder the person chose, if any.
pub async fn root(conn: &DatabaseConnection) -> Result<Option<PathBuf>, DbError> {
    Ok(app_metadata_service::get_value(conn, ROOT_KEY)
        .await?
        .filter(|raw| !raw.is_empty())
        .map(PathBuf::from))
}

/// Choose the root folder, or go back to the default with `None`.
pub async fn set_root(conn: &DatabaseConnection, root: Option<&Path>) -> Result<(), DbError> {
    let raw = root
        .map(|r| r.to_string_lossy().into_owned())
        .unwrap_or_default();
    app_metadata_service::upsert_value(conn, ROOT_KEY, &raw).await
}

/// Where `owner`'s tabs save downloads; none when the session is gone.
pub async fn folder_for(conn: &DatabaseConnection, owner: &TabOwner) -> Option<PathBuf> {
    let session_id = match owner {
        TabOwner::Personal => {
            return dirs::download_dir().or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        }
        TabOwner::Session { session_id } => session_id,
    };
    let row = crate::session::resolve(conn, session_id).await.ok()?;
    let worktree = folder::Entity::find_by_id(row.folder_id)
        .one(conn)
        .await
        .ok()??;
    if let Some(root) = root(conn).await.ok()? {
        let repo = match worktree.parent_id {
            Some(parent) => folder::Entity::find_by_id(parent).one(conn).await.ok()??,
            None => worktree,
        };
        return Some(
            root.join("zwiim")
                .join(repo_name(&repo.path))
                .join("downloads"),
        );
    }
    let id = short_id(conn, row.folder_id, session_id).await;
    Some(default_folder(Path::new(&worktree.path), &id))
}

/// The default folder of the session whose folder name is `id`.
pub fn default_folder(worktree: &Path, id: &str) -> PathBuf {
    worktree.join(".zwiim").join("browser").join(id)
}

/// The worktree `dir` is the default download folder of, if it is one, so
/// its `.zwiim/` can be kept out of Git.
pub fn worktree_of_default(dir: &Path) -> Option<&Path> {
    let browser = dir.parent()?;
    let zwiim = browser.parent()?;
    (browser.file_name()? == "browser" && zwiim.file_name()? == ".zwiim").then(|| zwiim.parent())?
}

/// The default folder of session `session_id`, to remove with it; none
/// when the person chose a root folder or the session is gone.
pub async fn session_folder(conn: &DatabaseConnection, session_id: &str) -> Option<PathBuf> {
    if root(conn).await.ok()?.is_some() {
        return None;
    }
    let row = crate::session::resolve(conn, session_id).await.ok()?;
    let worktree = folder::Entity::find_by_id(row.folder_id)
        .one(conn)
        .await
        .ok()??;
    let id = short_id(conn, row.folder_id, session_id).await;
    Some(default_folder(Path::new(&worktree.path), &id))
}

/// Remove a deleted session's default download folder, found with
/// [`session_folder`] before the delete.
pub async fn remove(dir: &Path) {
    match tokio::fs::remove_dir_all(dir).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(error = %e, "could not remove a session's browser downloads"),
    }
}

/// The session id's first characters, or all of it when another session in
/// the same worktree starts the same way.
async fn short_id(conn: &DatabaseConnection, folder_id: i32, session_id: &str) -> String {
    let Some(short) = session_id.get(..SHORT_ID) else {
        return session_id.to_string();
    };
    let shared = conversation::Entity::find()
        .filter(conversation::Column::FolderId.eq(folder_id))
        .filter(conversation::Column::SessionId.starts_with(short))
        .filter(conversation::Column::SessionId.ne(session_id))
        .one(conn)
        .await;
    match shared {
        Ok(None) => short.to_string(),
        _ => session_id.to_string(),
    }
}

/// The repository's folder name, safe as one path component.
fn repo_name(path: &str) -> String {
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    safe_component(&name).unwrap_or_else(|| "repository".into())
}

/// `name` as one plain path component: no separators, not `.` or `..`.
fn safe_component(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .take(MAX_NAME)
        .collect();
    let cleaned = cleaned.trim().to_string();
    (!cleaned.is_empty() && cleaned != "." && cleaned != "..").then_some(cleaned)
}

/// A path in `dir` for the page's file name `suggested` that nothing uses
/// yet: `report.pdf`, then `report (1).pdf`, `report (2).pdf`.
pub fn free_name(dir: &Path, suggested: &str) -> PathBuf {
    let name = safe_component(suggested).unwrap_or_else(|| "download".into());
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    let mut candidate = dir.join(&name);
    let mut n = 1;
    while candidate.exists() || candidate.is_symlink() {
        candidate = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_one_safe_component_and_never_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            free_name(dir.path(), "report.pdf"),
            dir.path().join("report.pdf")
        );
        std::fs::write(dir.path().join("report.pdf"), "").unwrap();
        std::fs::write(dir.path().join("report (1).pdf"), "").unwrap();
        assert_eq!(
            free_name(dir.path(), "report.pdf"),
            dir.path().join("report (2).pdf")
        );
        assert_eq!(
            free_name(dir.path(), "../../etc/passwd"),
            dir.path().join(".._.._etc_passwd")
        );
        assert_eq!(free_name(dir.path(), ".."), dir.path().join("download"));
        assert_eq!(free_name(dir.path(), ""), dir.path().join("download"));
        assert_eq!(free_name(dir.path(), ".env"), dir.path().join(".env"));
        assert_eq!(free_name(dir.path(), "notes"), dir.path().join("notes"));
        std::fs::write(dir.path().join("notes"), "").unwrap();
        assert_eq!(free_name(dir.path(), "notes"), dir.path().join("notes (1)"));
    }

    #[test]
    fn a_repository_name_is_one_component() {
        assert_eq!(repo_name("/Users/a/code/zwiim"), "zwiim");
        assert_eq!(repo_name("/"), "repository");
    }
}
