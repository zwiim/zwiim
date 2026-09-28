//! Schema upgrades that cannot lose data.
//!
//! Every Host start runs [`migrate`] before the database is used:
//!
//! - **Newer data is refused.** A database that records migrations this
//!   binary does not know was written by a newer Zwiim. It is left exactly as
//!   it is and startup fails with [`DbError::NewerSchema`]: an older binary
//!   never runs on a schema it cannot read, and never silently downgrades it.
//! - **One migration run at a time, all or nothing.** Pending migrations run
//!   inside a single `BEGIN IMMEDIATE` transaction on one connection: the
//!   write lock is taken before the pending list is read (a concurrent start
//!   waits, then finds nothing left to do), and a failing migration rolls the
//!   whole run back, leaving the database as it was. The Host's instance lock
//!   (`host_lifecycle`) already keeps a second Host away; this holds even
//!   without it.
//! - **A snapshot first.** Before migrating an existing database, a
//!   consistent single-file copy is written to `<data_dir>/db-snapshots/`
//!   (`VACUUM INTO`, published by rename). An update that fails later can put
//!   it back with [`restore`], which moves the replaced database aside rather
//!   than deleting it.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use chrono::Utc;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use sea_orm_migration::MigratorTrait;

use super::error::DbError;

/// Where database snapshots live, under the data directory.
pub const SNAPSHOT_DIR: &str = "db-snapshots";
/// How many snapshots are kept; the oldest go first.
const SNAPSHOT_KEEP: usize = 3;
/// Every SQLite database file starts with this.
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// How the database's schema relates to this binary's migrations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaState {
    /// No schema yet: a new data directory.
    Fresh,
    /// Every known migration is applied.
    Current,
    /// Known migrations not applied yet, in order.
    Pending(Vec<String>),
    /// Applied migrations this binary does not know.
    Newer(Vec<String>),
}

/// What [`migrate`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationOutcome {
    pub applied: Vec<String>,
    /// The snapshot taken before migrating, if the database had data.
    pub snapshot: Option<PathBuf>,
}

/// Read the schema's state without changing anything.
pub async fn inspect<M: MigratorTrait>(conn: &DatabaseConnection) -> Result<SchemaState, DbError> {
    let table = M::migration_table_name().to_string();
    let exists = conn
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?",
            [table.clone().into()],
        ))
        .await?
        .is_some();
    if !exists {
        return Ok(SchemaState::Fresh);
    }
    let rows = conn
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            format!(
                "SELECT version FROM \"{}\" ORDER BY version",
                table.replace('"', "\"\"")
            ),
        ))
        .await?;
    let applied: Vec<String> = rows
        .iter()
        .map(|row| row.try_get::<String>("", "version"))
        .collect::<Result<_, _>>()?;
    let known: Vec<String> = M::migrations()
        .iter()
        .map(|m| m.name().to_string())
        .collect();
    let known_set: HashSet<&str> = known.iter().map(String::as_str).collect();
    let unknown: Vec<String> = applied
        .iter()
        .filter(|v| !known_set.contains(v.as_str()))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        return Ok(SchemaState::Newer(unknown));
    }
    let applied_set: HashSet<&str> = applied.iter().map(String::as_str).collect();
    let pending: Vec<String> = known
        .into_iter()
        .filter(|name| !applied_set.contains(name.as_str()))
        .collect();
    Ok(if pending.is_empty() {
        SchemaState::Current
    } else {
        SchemaState::Pending(pending)
    })
}

/// Bring the schema up to this binary's migrations (see the module docs).
/// `conn` must be a single-connection pool, so the transaction and the
/// migrations share one connection.
pub async fn migrate<M: MigratorTrait>(
    conn: &DatabaseConnection,
    data_dir: &Path,
) -> Result<MigrationOutcome, DbError> {
    let mut outcome = MigrationOutcome::default();
    match inspect::<M>(conn).await? {
        SchemaState::Newer(unknown) => return Err(DbError::NewerSchema(unknown)),
        SchemaState::Current => return Ok(outcome),
        SchemaState::Fresh => {}
        SchemaState::Pending(pending) => {
            let path = snapshot(conn, data_dir, "pre-migration").await?;
            tracing::info!(
                "[db] {} migration(s) pending; snapshot at {}",
                pending.len(),
                path.display()
            );
            outcome.snapshot = Some(path);
        }
    }

    conn.execute_unprepared("BEGIN IMMEDIATE").await?;
    // Re-read under the write lock: a concurrent start may have migrated.
    let result = async {
        let pending = match inspect::<M>(conn).await? {
            SchemaState::Newer(unknown) => return Err(DbError::NewerSchema(unknown)),
            SchemaState::Pending(pending) => pending,
            SchemaState::Fresh => M::migrations()
                .iter()
                .map(|m| m.name().to_string())
                .collect(),
            SchemaState::Current => Vec::new(),
        };
        if !pending.is_empty() {
            M::up(conn, None)
                .await
                .map_err(|e| DbError::Migration(e.to_string()))?;
        }
        Ok(pending)
    }
    .await;
    match result {
        Ok(applied) => {
            conn.execute_unprepared("COMMIT").await?;
            outcome.applied = applied;
            Ok(outcome)
        }
        Err(e) => {
            if let Err(rollback) = conn.execute_unprepared("ROLLBACK").await {
                tracing::error!("[db] rolling back a failed migration failed: {rollback}");
            }
            tracing::error!("[db] migration failed and was rolled back: {e}");
            Err(e)
        }
    }
}

/// Write a consistent single-file copy of the database to the snapshot
/// directory and return its path. `reason` names it (`pre-migration`,
/// `pre-update`, …).
pub async fn snapshot(
    conn: &DatabaseConnection,
    data_dir: &Path,
    reason: &str,
) -> Result<PathBuf, DbError> {
    let dir = snapshot_dir(data_dir)?;
    let stamp = Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
    let path = dir.join(format!("{stamp}-{reason}.db"));
    let part = dir.join(format!("{stamp}-{reason}.db.part"));
    // `VACUUM INTO` requires the destination not to exist.
    let _ = std::fs::remove_file(&part);
    let literal = part.to_string_lossy().replace('\'', "''");
    conn.execute_unprepared(&format!("VACUUM INTO '{literal}'"))
        .await?;
    std::fs::rename(&part, &path)?;
    prune(&dir, SNAPSHOT_KEEP);
    Ok(path)
}

/// The snapshots on disk, oldest first.
pub fn snapshots(data_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(data_dir.join(SNAPSHOT_DIR)) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "db"))
        .filter(|p| !is_replaced(p))
        .collect();
    // Names start with a UTC stamp, so they sort by age.
    found.sort();
    found
}

/// Put `snapshot` back as the database `db_file` in `data_dir`. Call only
/// while nothing has the database open (before `init_database`). The
/// replaced database and its sidecars are moved into the snapshot directory
/// (`*-replaced.db`), never deleted. Returns where they went, if there was a
/// database to replace.
pub fn restore(data_dir: &Path, db_file: &str, snapshot: &Path) -> io::Result<Option<PathBuf>> {
    let mut header = [0u8; 16];
    {
        use std::io::Read;
        std::fs::File::open(snapshot)?.read_exact(&mut header)?;
    }
    if &header != SQLITE_HEADER {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not an SQLite database snapshot",
        ));
    }
    let dir = snapshot_dir(data_dir)?;
    let live = data_dir.join(db_file);

    // A complete copy next to the live file first, so the swap is a rename.
    let incoming = data_dir.join(format!("{db_file}.restoring"));
    std::fs::copy(snapshot, &incoming)?;
    std::fs::File::open(&incoming)?.sync_all()?;

    let stamp = Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
    let aside = dir.join(format!("{stamp}-replaced.db"));
    let replaced = if live.exists() {
        std::fs::rename(&live, &aside)?;
        Some(aside.clone())
    } else {
        None
    };
    // The sidecars belong to the replaced database; left in place they would
    // be replayed into the restored one.
    for suffix in ["-wal", "-shm"] {
        let sidecar = data_dir.join(format!("{db_file}{suffix}"));
        if sidecar.exists() {
            std::fs::rename(&sidecar, dir.join(format!("{stamp}-replaced.db{suffix}")))?;
        }
    }
    std::fs::rename(&incoming, &live)?;
    sync_dir(data_dir);
    Ok(replaced)
}

fn snapshot_dir(data_dir: &Path) -> io::Result<PathBuf> {
    let dir = data_dir.join(SNAPSHOT_DIR);
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

fn is_replaced(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.contains("-replaced.db"))
}

/// Keep the newest `keep` snapshots. Replaced databases are kept: they are
/// the only copy of what a restore replaced.
fn prune(dir: &Path, keep: usize) {
    let Some(data_dir) = dir.parent() else { return };
    let all = snapshots(data_dir);
    let excess = all.len().saturating_sub(keep);
    for old in &all[..excess] {
        if let Err(e) = std::fs::remove_file(old) {
            tracing::warn!("[db] could not remove old snapshot {}: {e}", old.display());
        }
    }
}

#[cfg(unix)]
fn sync_dir(dir: &Path) {
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) {}

#[cfg(test)]
mod tests;
