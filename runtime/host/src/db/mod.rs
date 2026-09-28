pub mod entities;
pub mod error;
pub mod migration;
pub mod schema_guard;
pub mod service;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_helpers;

use std::path::Path;
use std::time::Duration;

use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement,
};

use error::DbError;
use migration::Migrator;

pub struct AppDatabase {
    pub conn: DatabaseConnection,
}

/// What migrating did when this process opened the database, for diagnostics.
static STARTUP_MIGRATION: std::sync::OnceLock<schema_guard::MigrationOutcome> =
    std::sync::OnceLock::new();

pub fn startup_migration() -> Option<&'static schema_guard::MigrationOutcome> {
    STARTUP_MIGRATION.get()
}

pub(crate) fn database_file_name() -> &'static str {
    if cfg!(all(debug_assertions, feature = "tauri-runtime")) {
        "zwiim-dev.db"
    } else {
        "zwiim.db"
    }
}

/// The file name data directories created before the Zwiim rename use. Kept
/// only so [`adopt_legacy_database`] can carry an existing Host's data over.
fn legacy_database_file_name() -> &'static str {
    if cfg!(all(debug_assertions, feature = "tauri-runtime")) {
        "codeg-dev.db"
    } else {
        "codeg.db"
    }
}

/// Renames a pre-rename database (and its SQLite sidecars) to the current
/// name, once: only when the current file does not exist yet. Sidecars move
/// first, so an interrupted adoption never leaves the database without its WAL.
fn adopt_legacy_database(app_data_dir: &Path) -> std::io::Result<()> {
    let current = app_data_dir.join(database_file_name());
    let legacy = app_data_dir.join(legacy_database_file_name());
    if current.exists() || !legacy.exists() {
        return Ok(());
    }
    for suffix in ["-wal", "-shm"] {
        let from = app_data_dir.join(format!("{}{suffix}", legacy_database_file_name()));
        if from.exists() {
            std::fs::rename(from, app_data_dir.join(format!("{}{suffix}", database_file_name())))?;
        }
    }
    std::fs::rename(legacy, current)
}

pub async fn init_database(
    app_data_dir: impl AsRef<Path>,
    app_version: &str,
) -> Result<AppDatabase, DbError> {
    let app_data_dir = app_data_dir.as_ref();
    std::fs::create_dir_all(app_data_dir)?;
    adopt_legacy_database(app_data_dir)?;

    // Apply any pending restore BEFORE opening a connection — swapping
    // `zwiim.db` under a live SQLite handle would corrupt it. A failure here
    // aborts startup loudly (leaving the safety snapshot intact) rather than
    // booting a half-restored data dir.
    match crate::commands::backup::restore::apply_pending_restore_on_startup(app_data_dir) {
        Ok(crate::commands::backup::restore::RestoreApplied::Applied { .. }) => {}
        Ok(crate::commands::backup::restore::RestoreApplied::None) => {}
        Err(e) => return Err(DbError::Io(e)),
    }
    crate::commands::backup::restore::cleanup_transient_dirs(app_data_dir);

    let db_path = app_data_dir.join(database_file_name());
    let db_url = format!(
        "sqlite:{}?mode=rwc",
        urlencoding::encode(&db_path.to_string_lossy())
    );

    // Apply migrations on a dedicated single connection. The runtime pool below
    // keeps several connections open for read concurrency, but sea-orm spreads a
    // migration's statements across whichever pooled connections are free. A
    // statement that references a column an earlier migration just added (e.g.
    // the `is_chat` → `kind` backfill) can then land on a connection whose
    // cached SQLite schema predates the `ALTER TABLE`, producing a flaky
    // `no such column: "is_chat"` under load. One connection observes every DDL
    // change in order, so the schema it compiles against is always current.
    let mut migrate_opts = ConnectOptions::new(db_url.clone());
    migrate_opts
        .max_connections(1)
        .min_connections(1)
        .connect_timeout(Duration::from_secs(10))
        .sqlx_logging(false);
    let migrate_conn = Database::connect(migrate_opts).await?;
    apply_sqlite_pragmas(&migrate_conn).await?;
    // Refuses a newer schema untouched; snapshots, then migrates all or
    // nothing (see `schema_guard`).
    let migrated = schema_guard::migrate::<Migrator>(&migrate_conn, app_data_dir).await;
    migrate_conn.close().await?;
    let migrated = migrated?;
    crate::update::install::note_database_start(&migrated);
    let _ = STARTUP_MIGRATION.set(migrated.clone());
    if !migrated.applied.is_empty() {
        tracing::info!("[db] applied {} migration(s)", migrated.applied.len());
    }

    // Runtime connection pool. Migrations are already applied above, so the
    // schema is stable and spreading queries across pooled connections is safe.
    let mut opts = ConnectOptions::new(db_url);
    opts.max_connections(5)
        .min_connections(1)
        .connect_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(300))
        .sqlx_logging(false);
    let conn = Database::connect(opts).await?;
    apply_sqlite_pragmas(&conn).await?;

    service::app_metadata_service::update_app_version(&conn, app_version).await?;

    // Publish user-registered ACP agents into the process-global launch
    // registry before anything can ask for agent metadata. This is the single
    // chokepoint every runtime (desktop, server) goes through, so custom agents
    // are live from the first `all_acp_agents()` / `get_agent_meta()` call.
    // A failure here must not block startup — the built-in agents still work.
    if let Err(e) = service::custom_agent_service::hydrate_registry(&conn).await {
        tracing::warn!("[custom-agent] failed to hydrate custom agent registry: {e}");
    }

    // Load user-authorized workspace links before any file command can run, so
    // the workspace path guard follows exactly the symlinks the user created
    // and nothing else. A failure here fails *closed* (registry stays empty:
    // linked subtrees look unreadable) rather than blocking startup.
    match crate::folder_links::hydrate(&conn).await {
        Ok(count) if count > 0 => {
            tracing::info!("[folder-link] hydrated {count} workspace link(s)");
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("[folder-link] failed to hydrate workspace links: {e}"),
    }

    Ok(AppDatabase { conn })
}

/// Apply SQLite performance and reliability pragmas to a freshly opened
/// connection. `journal_mode=WAL` persists in the database header; the rest are
/// per-connection settings that must be re-applied every time a connection opens.
async fn apply_sqlite_pragmas(conn: &DatabaseConnection) -> Result<(), DbError> {
    for pragma in [
        "PRAGMA journal_mode=WAL;",
        "PRAGMA busy_timeout=5000;",
        "PRAGMA synchronous=NORMAL;",
        "PRAGMA foreign_keys=ON;",
        "PRAGMA cache_size=-8000;",
    ] {
        conn.execute(Statement::from_string(DbBackend::Sqlite, pragma.to_owned()))
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adopts_a_pre_rename_database_once() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join(legacy_database_file_name());
        std::fs::write(&legacy, b"db").unwrap();
        let legacy_wal = format!("{}-wal", legacy_database_file_name());
        std::fs::write(dir.path().join(legacy_wal), b"wal").unwrap();
        adopt_legacy_database(dir.path()).unwrap();
        let current = dir.path().join(database_file_name());
        assert_eq!(std::fs::read(&current).unwrap(), b"db");
        let wal = dir.path().join(format!("{}-wal", database_file_name()));
        assert_eq!(std::fs::read(wal).unwrap(), b"wal");
        assert!(!legacy.exists());

        // A current database is never replaced by a stray legacy file.
        std::fs::write(&legacy, b"stale").unwrap();
        adopt_legacy_database(dir.path()).unwrap();
        assert_eq!(std::fs::read(&current).unwrap(), b"db");
    }

    #[tokio::test]
    async fn opens_an_adopted_database_with_its_data() {
        let dir = tempfile::tempdir().unwrap();
        let db = init_database(dir.path(), "test").await.unwrap();
        db.conn
            .execute(Statement::from_string(DbBackend::Sqlite, "CREATE TABLE kept (v TEXT)"))
            .await
            .unwrap();
        db.conn.close().await.unwrap();
        std::fs::rename(
            dir.path().join(database_file_name()),
            dir.path().join(legacy_database_file_name()),
        )
        .unwrap();
        let db = init_database(dir.path(), "test").await.unwrap();
        let kept = "SELECT name FROM sqlite_master WHERE name = 'kept'";
        let row = db
            .conn
            .query_one(Statement::from_string(DbBackend::Sqlite, kept))
            .await
            .unwrap();
        assert!(row.is_some());
    }
}
