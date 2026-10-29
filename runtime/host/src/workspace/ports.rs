//! Zwiim-owned: the ports a managed worktree's processes use, so the dev
//! servers of two worktrees of one repository never ask for the same one.
//!
//! Each worktree gets a block of [`BLOCK`] ports from [`FIRST`] on, the first
//! time something starts there, given to its scripts, terminals and agents as
//! `ZWIIM_PORT` (the block's first). It keeps the block until it is removed;
//! another worktree may then take it.

use std::collections::HashSet;
use std::net::{Ipv4Addr, TcpListener};

use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tokio::sync::Mutex;

use crate::db::entities::folder;

/// The first port of the first block.
pub const FIRST: u16 = 40_000;
/// Ports in a block.
pub const BLOCK: u16 = 10;
/// Blocks there are, up to port 49999.
const BLOCKS: u16 = 1_000;

/// Choices read, then claim, a block: one at a time, so no two get the same.
static CHOOSING: Mutex<()> = Mutex::const_new(());

/// The first port of worktree `folder_id`'s block. Its own when it has one no
/// other live workspace holds (a worktree removed and made again may find
/// its block taken); otherwise the lowest block no live workspace holds whose
/// ports are free now, kept for it. `None` when none is or it can't be kept.
pub async fn port_of(db: &DatabaseConnection, folder_id: i32) -> Option<u16> {
    let _one = CHOOSING.lock().await;
    let held = folder::Entity::find()
        .filter(folder::Column::DeletedAt.is_null())
        .filter(folder::Column::Port.is_not_null())
        .all(db)
        .await
        .ok()?;
    let own = folder::Entity::find_by_id(folder_id).one(db).await.ok()??.port;
    let taken: HashSet<i32> = held
        .iter()
        .filter(|f| f.id != folder_id)
        .filter_map(|f| f.port)
        .collect();
    if let Some(port) = own.filter(|p| !taken.contains(p)) {
        return u16::try_from(port).ok();
    }
    let port = (0..BLOCKS)
        .map(|i| FIRST + i * BLOCK)
        .find(|&first| !taken.contains(&i32::from(first)) && free(first))?;
    folder::Entity::update_many()
        .col_expr(folder::Column::Port, Expr::value(i32::from(port)))
        .filter(folder::Column::Id.eq(folder_id))
        .exec(db)
        .await
        .ok()?;
    Some(port)
}

/// Nothing listens on the block's ports on this machine now.
fn free(first: u16) -> bool {
    (first..first + BLOCK).all(|p| TcpListener::bind((Ipv4Addr::LOCALHOST, p)).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::service::folder_service::{add_folder_with_parent, soft_delete_folder};
    use crate::db::test_helpers::fresh_in_memory_db;

    #[tokio::test]
    async fn each_worktree_keeps_its_own_free_block() {
        let db = fresh_in_memory_db().await;
        let repo = add_folder_with_parent(&db.conn, "/tmp/zwiim-ports-repo", None).await.unwrap();
        let wt = |n: &str| {
            let path = format!("/tmp/zwiim-ports-{n}");
            let conn = &db.conn;
            async move { add_folder_with_parent(conn, &path, Some(repo.id)).await }
        };
        let (a, b) = (wt("a").await.unwrap(), wt("b").await.unwrap());

        let pa = port_of(&db.conn, a.id).await.unwrap();
        let pb = port_of(&db.conn, b.id).await.unwrap();
        assert!(pa >= FIRST && pa % BLOCK == 0 && pb % BLOCK == 0);
        assert_ne!(pa, pb);
        // Kept, even while its own server is listening on it.
        let _busy = TcpListener::bind((Ipv4Addr::LOCALHOST, pa + 3));
        assert_eq!(port_of(&db.conn, a.id).await, Some(pa));

        // Another worktree's block is skipped.
        let c = wt("c").await.unwrap();
        let pc = port_of(&db.conn, c.id).await.unwrap();
        assert!(![pa, pb].contains(&pc));

        // A removed worktree's block is free again; brought back, it gets
        // another when that one has been taken meanwhile.
        soft_delete_folder(&db.conn, b.id).await.unwrap();
        let d = wt("d").await.unwrap();
        let pd = port_of(&db.conn, d.id).await.unwrap();
        assert_eq!(pd, pb, "the lowest free block");
        assert_eq!(wt("b").await.unwrap().id, b.id);
        let again = port_of(&db.conn, b.id).await.unwrap();
        assert!(![pa, pc, pd].contains(&again));
    }
}
