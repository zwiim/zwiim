//! Migration safety: normal upgrade, failure, newer database, concurrent
//! start, and restoring a snapshot.

use std::path::Path;
use std::time::Duration;

use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement,
};
use sea_orm_migration::prelude::*;

use super::*;

// A tiny schema history: V1 knows `m1`, V2 adds `m2`, Broken adds a `m3`
// that fails halfway through.

struct M1;
struct M2;
struct M3Broken;

impl MigrationName for M1 {
    fn name(&self) -> &str {
        "m1_notes"
    }
}
impl MigrationName for M2 {
    fn name(&self) -> &str {
        "m2_tags"
    }
}
impl MigrationName for M3Broken {
    fn name(&self) -> &str {
        "m3_broken"
    }
}

async fn exec(manager: &SchemaManager<'_>, sql: &str) -> Result<(), DbErr> {
    manager
        .get_connection()
        .execute_unprepared(sql)
        .await
        .map(|_| ())
}

#[async_trait::async_trait]
impl MigrationTrait for M1 {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        exec(
            manager,
            "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL)",
        )
        .await
    }
}
#[async_trait::async_trait]
impl MigrationTrait for M2 {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        exec(
            manager,
            "CREATE TABLE tags (id INTEGER PRIMARY KEY, name TEXT)",
        )
        .await?;
        exec(manager, "ALTER TABLE notes ADD COLUMN tag_id INTEGER").await
    }
}
#[async_trait::async_trait]
impl MigrationTrait for M3Broken {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        exec(manager, "CREATE TABLE half_done (id INTEGER)").await?;
        exec(manager, "UPDATE notes SET body = 'clobbered'").await?;
        exec(manager, "THIS IS NOT SQL").await
    }
}

struct V1;
struct V2;
struct Broken;

impl MigratorTrait for V1 {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(M1)]
    }
}
impl MigratorTrait for V2 {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(M1), Box::new(M2)]
    }
}
impl MigratorTrait for Broken {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(M1), Box::new(M2), Box::new(M3Broken)]
    }
}

const DB: &str = "test.db";

async fn open(dir: &Path) -> DatabaseConnection {
    let url = format!("sqlite:{}?mode=rwc", dir.join(DB).display());
    let mut opts = ConnectOptions::new(url);
    opts.max_connections(1)
        .min_connections(1)
        .connect_timeout(Duration::from_secs(10))
        .sqlx_logging(false);
    let conn = Database::connect(opts).await.unwrap();
    for pragma in ["PRAGMA journal_mode=WAL", "PRAGMA busy_timeout=5000"] {
        conn.execute_unprepared(pragma).await.unwrap();
    }
    conn
}

async fn tables(conn: &DatabaseConnection) -> Vec<String> {
    conn.query_all(Statement::from_string(
        DbBackend::Sqlite,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    ))
    .await
    .unwrap()
    .iter()
    .map(|r| r.try_get::<String>("", "name").unwrap())
    .collect()
}

async fn note_bodies(conn: &DatabaseConnection) -> Vec<String> {
    conn.query_all(Statement::from_string(
        DbBackend::Sqlite,
        "SELECT body FROM notes ORDER BY id",
    ))
    .await
    .unwrap()
    .iter()
    .map(|r| r.try_get::<String>("", "body").unwrap())
    .collect()
}

/// A V1 database holding one note.
async fn v1_with_a_note(dir: &Path) -> DatabaseConnection {
    let conn = open(dir).await;
    migrate::<V1>(&conn, dir).await.unwrap();
    conn.execute_unprepared("INSERT INTO notes (body) VALUES ('keep me')")
        .await
        .unwrap();
    conn
}

fn file_bytes(dir: &Path) -> Vec<u8> {
    std::fs::read(dir.join(DB)).unwrap()
}

#[tokio::test]
async fn a_fresh_database_migrates_without_a_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open(dir.path()).await;
    assert_eq!(inspect::<V2>(&conn).await.unwrap(), SchemaState::Fresh);

    let outcome = migrate::<V2>(&conn, dir.path()).await.unwrap();
    assert_eq!(outcome.applied, ["m1_notes", "m2_tags"]);
    assert_eq!(outcome.snapshot, None, "nothing to lose yet");
    assert_eq!(inspect::<V2>(&conn).await.unwrap(), SchemaState::Current);
    assert!(snapshots(dir.path()).is_empty());

    // Running again is a no-op.
    assert_eq!(
        migrate::<V2>(&conn, dir.path()).await.unwrap(),
        MigrationOutcome::default()
    );
}

#[tokio::test]
async fn an_upgrade_snapshots_first_and_keeps_the_data() {
    let dir = tempfile::tempdir().unwrap();
    let conn = v1_with_a_note(dir.path()).await;
    assert_eq!(
        inspect::<V2>(&conn).await.unwrap(),
        SchemaState::Pending(vec!["m2_tags".into()])
    );

    let outcome = migrate::<V2>(&conn, dir.path()).await.unwrap();
    assert_eq!(outcome.applied, ["m2_tags"]);
    assert_eq!(note_bodies(&conn).await, ["keep me"]);
    assert!(tables(&conn).await.contains(&"tags".to_string()));

    // The snapshot is the database as it was before the upgrade.
    let snap = outcome.snapshot.expect("snapshot taken");
    assert!(snap.starts_with(dir.path().join(SNAPSHOT_DIR)));
    assert!(snap
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .ends_with("-pre-migration.db"));
    assert_eq!(snapshots(dir.path()), [snap.clone()]);
    let old = open_file(&snap).await;
    assert_eq!(
        inspect::<V2>(&old).await.unwrap(),
        SchemaState::Pending(vec!["m2_tags".into()])
    );
    assert_eq!(note_bodies(&old).await, ["keep me"]);
    assert!(!tables(&old).await.contains(&"tags".to_string()));
}

async fn open_file(path: &Path) -> DatabaseConnection {
    let url = format!("sqlite:{}?mode=ro", path.display());
    Database::connect(ConnectOptions::new(url)).await.unwrap()
}

#[tokio::test]
async fn a_failing_migration_rolls_back_the_whole_run() {
    let dir = tempfile::tempdir().unwrap();
    let conn = v1_with_a_note(dir.path()).await;

    let err = migrate::<Broken>(&conn, dir.path()).await.unwrap_err();
    assert!(matches!(err, DbError::Migration(_)), "{err}");

    // Nothing of m2 or m3 is left: not the tables, not the rows they
    // touched, not their migration records.
    assert_eq!(
        inspect::<Broken>(&conn).await.unwrap(),
        SchemaState::Pending(vec!["m2_tags".into(), "m3_broken".into()])
    );
    let t = tables(&conn).await;
    assert!(
        !t.contains(&"tags".to_string()) && !t.contains(&"half_done".to_string()),
        "{t:?}"
    );
    assert_eq!(note_bodies(&conn).await, ["keep me"]);
    // The pre-migration snapshot is kept for the operator (or the updater).
    assert_eq!(snapshots(dir.path()).len(), 1);

    // The connection is usable, and a fixed binary migrates normally.
    assert_eq!(
        migrate::<V2>(&conn, dir.path()).await.unwrap().applied,
        ["m2_tags"]
    );
}

#[tokio::test]
async fn a_database_from_a_newer_zwiim_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open(dir.path()).await;
    migrate::<V2>(&conn, dir.path()).await.unwrap();
    conn.execute_unprepared("INSERT INTO notes (body) VALUES ('from the future')")
        .await
        .unwrap();
    conn.execute_unprepared("PRAGMA wal_checkpoint(TRUNCATE)")
        .await
        .unwrap();
    let before = file_bytes(dir.path());

    // An older binary (V1) opens it.
    assert_eq!(
        inspect::<V1>(&conn).await.unwrap(),
        SchemaState::Newer(vec!["m2_tags".into()])
    );
    let err = migrate::<V1>(&conn, dir.path()).await.unwrap_err();
    match &err {
        DbError::NewerSchema(unknown) => assert_eq!(unknown, &["m2_tags"]),
        other => panic!("expected NewerSchema, got {other}"),
    }
    assert!(err.to_string().contains("newer version of Zwiim"));

    conn.execute_unprepared("PRAGMA wal_checkpoint(TRUNCATE)")
        .await
        .unwrap();
    assert_eq!(file_bytes(dir.path()), before, "not a byte changed");
    assert!(
        snapshots(dir.path()).is_empty(),
        "no snapshot of data it cannot read"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_starts_migrate_once() {
    let dir = tempfile::tempdir().unwrap();
    // Two processes' worth of connections on one file, started together.
    let (a, b) = (open(dir.path()).await, open(dir.path()).await);
    let path = dir.path().to_path_buf();
    let (ra, rb) = tokio::join!(migrate::<V2>(&a, &path), migrate::<V2>(&b, &path));
    let (ra, rb) = (ra.unwrap(), rb.unwrap());

    // Exactly one of them ran the migrations; the other found nothing to do.
    let mut applied = [ra.applied.len(), rb.applied.len()];
    applied.sort();
    assert_eq!(applied, [0, 2]);
    let rows = a
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT version FROM seaql_migrations",
        ))
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(inspect::<V2>(&b).await.unwrap(), SchemaState::Current);
}

#[tokio::test]
async fn restoring_a_snapshot_rolls_the_database_back() {
    let dir = tempfile::tempdir().unwrap();
    let conn = v1_with_a_note(dir.path()).await;
    let snap = migrate::<V2>(&conn, dir.path())
        .await
        .unwrap()
        .snapshot
        .unwrap();
    // The upgraded version writes data, then its update fails.
    conn.execute_unprepared("INSERT INTO notes (body) VALUES ('written after')")
        .await
        .unwrap();
    conn.close().await.unwrap();

    let replaced = restore(dir.path(), DB, &snap)
        .unwrap()
        .expect("a database was replaced");

    let conn = open(dir.path()).await;
    assert_eq!(inspect::<V1>(&conn).await.unwrap(), SchemaState::Current);
    assert_eq!(note_bodies(&conn).await, ["keep me"]);
    // The replaced database is kept aside, whole, never deleted.
    assert!(replaced.starts_with(dir.path().join(SNAPSHOT_DIR)));
    let aside = open_file(&replaced).await;
    assert_eq!(note_bodies(&aside).await, ["keep me", "written after"]);
    // …and it is not mistaken for a snapshot to restore.
    assert_eq!(snapshots(dir.path()), [snap]);
}

#[test]
fn restore_refuses_a_file_that_is_not_a_database() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(DB), b"live").unwrap();
    let bogus = dir.path().join("bogus.db");
    std::fs::write(&bogus, b"definitely not sqlite").unwrap();
    let err = restore(dir.path(), DB, &bogus).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert_eq!(std::fs::read(dir.path().join(DB)).unwrap(), b"live");
}

#[tokio::test]
async fn only_the_newest_snapshots_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let conn = v1_with_a_note(dir.path()).await;
    let mut taken = Vec::new();
    for _ in 0..5 {
        taken.push(snapshot(&conn, dir.path(), "pre-update").await.unwrap());
        // Distinct millisecond stamps.
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    assert_eq!(snapshots(dir.path()), taken[2..]);
}
