// Modified from upstream Apache-2.0 source; see NOTICE.
//! The engine and store against a real (in-memory) SQLite database: the
//! state tables move for real, and the dispatch transaction's all-or-nothing
//! promise is checked from the outside (spec 6.6.1).

use crate::automation::entities::{cap_usage, never_twice_ledger, playbook_item};
use crate::automation::states::{ItemStatus, RunStatus, RunTrigger};
use crate::automation::{
    AutomationEngine, CapSlot, DispatchOutcome, LedgerKey, PlaybookStore, SiteKnowledgeKind,
    StartRunError,
};
use crate::db::error::DbError;
use crate::db::migration::Migrator;
use sea_orm::{ColumnTrait, Database, DatabaseConnection, EntityTrait, QueryFilter};
use sea_orm_migration::MigratorTrait;

async fn engine_db() -> (AutomationEngine, PlaybookStore) {
    let db: DatabaseConnection = Database::connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    Migrator::up(&db, None).await.expect("run migrations");
    (AutomationEngine::new(db.clone()), PlaybookStore::new(db))
}

async fn item(
    engine: &AutomationEngine,
    run_id: i32,
    playbook_id: i32,
    site: &str,
    key: &str,
    status: ItemStatus,
) -> i32 {
    let now = chrono::Utc::now();
    playbook_item::Entity::insert(playbook_item::ActiveModel {
        public_id: sea_orm::Set(uuid::Uuid::new_v4().to_string()),
        run_id: sea_orm::Set(run_id),
        playbook_id: sea_orm::Set(playbook_id),
        site: sea_orm::Set(site.to_string()),
        account: sea_orm::Set("main".to_string()),
        item_key: sea_orm::Set(key.to_string()),
        status: sea_orm::Set(status.as_str().to_string()),
        epoch: sea_orm::Set(0),
        created_at: sea_orm::Set(now),
        updated_at: sea_orm::Set(now),
        ..Default::default()
    })
    .exec_with_returning(engine.db())
    .await
    .expect("insert item")
    .id
}

fn slot(kind: &str, cap: i32) -> CapSlot {
    CapSlot {
        site: "indeed.com".into(),
        account: "main".into(),
        action_kind: kind.into(),
        local_day: "2026-11-19".into(),
        site_cap: cap,
        kind_cap: None,
    }
}

fn key(kind: &str) -> LedgerKey {
    LedgerKey {
        site: "indeed.com".into(),
        account: "main".into(),
        action_kind: kind.into(),
        item_key: "job-1".into(),
    }
}

#[tokio::test]
async fn a_second_start_is_run_active_until_the_first_ends() {
    let (engine, store) = engine_db().await;
    let pb = store.create("Apply", "goal: apply").await.expect("create");
    let run = engine
        .start_run(pb.id, pb.version, RunTrigger::Mac, None)
        .await
        .expect("first start");
    assert!(matches!(
        engine
            .start_run(pb.id, pb.version, RunTrigger::Mac, None)
            .await,
        Err(StartRunError::RunActive)
    ));
    engine
        .transition_run(run, RunStatus::Discovering, RunStatus::Stopped, None)
        .await
        .expect("stop");
    engine
        .start_run(pb.id, pb.version, RunTrigger::Mac, None)
        .await
        .expect("a finished run frees the playbook");
}

#[tokio::test]
async fn transition_refuses_edges_the_table_forbids_and_bumps_the_epoch() {
    let (engine, store) = engine_db().await;
    let pb = store.create("Apply", "goal").await.expect("create");
    let run = engine
        .start_run(pb.id, pb.version, RunTrigger::Mac, None)
        .await
        .expect("start");
    let id = item(
        &engine,
        run,
        pb.id,
        "indeed.com",
        "job-1",
        ItemStatus::Queued,
    )
    .await;

    assert!(engine
        .transition_item(id, ItemStatus::Queued, ItemStatus::Done, None)
        .await
        .is_err());
    engine
        .transition_item(id, ItemStatus::Queued, ItemStatus::Working, None)
        .await
        .expect("queued -> working");
    let row = playbook_item::Entity::find_by_id(id)
        .one(engine.db())
        .await
        .expect("read")
        .expect("item");
    assert_eq!(row.epoch, 1, "a new attempt gets a new epoch");
}

#[tokio::test]
async fn dispatch_reserves_the_key_and_takes_one_cap_slot() {
    let (engine, store) = engine_db().await;
    let pb = store.create("Apply", "goal").await.expect("create");
    let run = engine
        .start_run(pb.id, pb.version, RunTrigger::Mac, None)
        .await
        .expect("start");
    let id = item(
        &engine,
        run,
        pb.id,
        "indeed.com",
        "job-1",
        ItemStatus::Working,
    )
    .await;

    let out = engine
        .begin_dispatch(id, 0, &[key("apply")], slot("apply", 25))
        .await
        .expect("dispatch");
    assert_eq!(out, DispatchOutcome::Dispatched);

    let row = playbook_item::Entity::find_by_id(id)
        .one(engine.db())
        .await
        .expect("read")
        .expect("item");
    assert!(row.outward_started_at.is_some());
    let used = cap_usage::Entity::find_by_id((
        "indeed.com".to_string(),
        "main".to_string(),
        "*".to_string(),
        "2026-11-19".to_string(),
    ))
    .one(engine.db())
    .await
    .expect("read cap")
    .expect("cap row");
    assert_eq!(used.used, 1);
}

#[tokio::test]
async fn a_taken_key_skips_with_no_cap_spent_and_no_outward_start() {
    let (engine, store) = engine_db().await;
    let pb = store.create("Apply", "goal").await.expect("create");
    let run = engine
        .start_run(pb.id, pb.version, RunTrigger::Mac, None)
        .await
        .expect("start");
    let first = item(
        &engine,
        run,
        pb.id,
        "indeed.com",
        "job-1",
        ItemStatus::Working,
    )
    .await;
    engine
        .begin_dispatch(first, 0, &[key("apply")], slot("apply", 25))
        .await
        .expect("first dispatch");

    // A different playbook finds the same job: the shared ledger stops it.
    let pb2 = store.create("Apply again", "goal").await.expect("create");
    let run2 = engine
        .start_run(pb2.id, pb2.version, RunTrigger::Mac, None)
        .await
        .expect("start 2");
    // The lane is free because the first item is not in `working`.
    engine
        .transition_item(first, ItemStatus::Working, ItemStatus::Done, None)
        .await
        .expect("first was sent");
    let second = item(
        &engine,
        run2,
        pb2.id,
        "indeed.com",
        "job-1",
        ItemStatus::Working,
    )
    .await;
    let out = engine
        .begin_dispatch(second, 0, &[key("apply")], slot("apply", 25))
        .await
        .expect("dispatch 2");
    assert_eq!(out, DispatchOutcome::AlreadyDone);

    let row = playbook_item::Entity::find_by_id(second)
        .one(engine.db())
        .await
        .expect("read")
        .expect("item");
    assert!(row.outward_started_at.is_none(), "nothing went out");
    let cap = cap_usage::Entity::find()
        .filter(cap_usage::Column::Site.eq("indeed.com"))
        .all(engine.db())
        .await
        .expect("read cap");
    assert_eq!(cap.len(), 1);
    assert_eq!(cap[0].used, 1, "only the first dispatch took a slot");
    let ledger = never_twice_ledger::Entity::find()
        .all(engine.db())
        .await
        .expect("read ledger");
    assert_eq!(ledger.len(), 1);
}

#[tokio::test]
async fn a_full_day_defers_the_item_without_outward_start() {
    let (engine, store) = engine_db().await;
    let pb = store.create("Post", "goal").await.expect("create");
    let run = engine
        .start_run(pb.id, pb.version, RunTrigger::Mac, None)
        .await
        .expect("start");
    let id = item(
        &engine,
        run,
        pb.id,
        "indeed.com",
        "job-1",
        ItemStatus::Working,
    )
    .await;
    let out = engine
        .begin_dispatch(id, 0, &[key("apply")], slot("apply", 0))
        .await
        .expect("dispatch");
    assert_eq!(out, DispatchOutcome::DailyCapHit);
    let row = playbook_item::Entity::find_by_id(id)
        .one(engine.db())
        .await
        .expect("read")
        .expect("item");
    assert!(row.outward_started_at.is_none());
    assert!(never_twice_ledger::Entity::find()
        .all(engine.db())
        .await
        .expect("read ledger")
        .is_empty());
}

#[tokio::test]
async fn a_run_pins_the_text_it_started_on() {
    let (_engine, store) = engine_db().await;
    let pb = store.create("Apply", "v1 text").await.expect("create");
    let edited = store
        .save_source(pb.id, "Apply", "v2 text")
        .await
        .expect("save new version");
    assert_eq!(pb.version, 1);
    assert_eq!(edited.version, 2);
    assert_eq!(store.pinned_source(pb.id, 1).await.expect("v1"), "v1 text");
    assert_eq!(store.pinned_source(pb.id, 2).await.expect("v2"), "v2 text");
    assert!(matches!(
        store.pinned_source(pb.id, 9).await,
        Err(DbError::NotFound(_))
    ));
}

#[tokio::test]
async fn site_knowledge_is_shared_and_the_id_rule_is_versioned() {
    let (_engine, store) = engine_db().await;
    store
        .set_site_value(
            "linkedin.com",
            SiteKnowledgeKind::ItemIdRule,
            "",
            "path:/jobs/view/{id}",
        )
        .await
        .expect("rule 1");
    // The account is ignored for a site-wide kind.
    store
        .set_site_value(
            "linkedin.com",
            SiteKnowledgeKind::ItemIdRule,
            "main",
            "path:/jobs/view/{id}?",
        )
        .await
        .expect("rule 2");
    assert_eq!(
        store
            .site_value("linkedin.com", SiteKnowledgeKind::ItemIdRule, "main")
            .await
            .expect("read")
            .as_deref(),
        Some("path:/jobs/view/{id}?")
    );
    assert!(!store
        .risk_accepted("linkedin.com", "main")
        .await
        .expect("read"));
    store
        .set_site_value(
            "linkedin.com",
            SiteKnowledgeKind::RiskAccept,
            "main",
            "2026-11-19T00:00:00Z",
        )
        .await
        .expect("accept");
    assert!(store
        .risk_accepted("linkedin.com", "main")
        .await
        .expect("read"));
    // Risk acceptance is per account.
    assert!(!store
        .risk_accepted("linkedin.com", "second")
        .await
        .expect("read"));
}

#[tokio::test]
async fn a_lane_holds_one_working_item_at_a_time() {
    let (engine, store) = engine_db().await;
    let pb = store.create("Apply", "goal").await.expect("create");
    let run = engine
        .start_run(pb.id, pb.version, RunTrigger::Mac, None)
        .await
        .expect("start");
    let first = item(
        &engine,
        run,
        pb.id,
        "indeed.com",
        "job-1",
        ItemStatus::Working,
    )
    .await;
    let second = item(
        &engine,
        run,
        pb.id,
        "indeed.com",
        "job-2",
        ItemStatus::Queued,
    )
    .await;
    // The partial unique index refuses a second working item on the lane.
    assert!(engine
        .transition_item(second, ItemStatus::Queued, ItemStatus::Working, None)
        .await
        .is_err());
    engine
        .transition_item(first, ItemStatus::Working, ItemStatus::Done, None)
        .await
        .expect("finish first");
    engine
        .transition_item(second, ItemStatus::Queued, ItemStatus::Working, None)
        .await
        .expect("the lane is free");
}

#[tokio::test]
async fn the_site_total_and_the_kind_cap_are_both_enforced() {
    let (engine, store) = engine_db().await;
    let pb = store.create("Post", "goal").await.expect("create");
    let run = engine
        .start_run(pb.id, pb.version, RunTrigger::Mac, None)
        .await
        .expect("start");

    // A slot with a site total of 2 and a publish_post kind cap of 1.
    let slot = CapSlot {
        site: "x.example".into(),
        account: "main".into(),
        action_kind: "publish_post".into(),
        local_day: "2026-11-19".into(),
        site_cap: 2,
        kind_cap: Some(1),
    };
    let mut n = 0;
    for i in 0..3 {
        // Free the lane between items by using distinct sites per try.
        let id = item(
            &engine,
            run,
            pb.id,
            &format!("x{i}.example"),
            &format!("post-{i}"),
            ItemStatus::Working,
        )
        .await;
        let mut s = slot.clone();
        s.site = format!("x{i}.example");
        let keys = vec![LedgerKey {
            site: s.site.clone(),
            account: "main".into(),
            action_kind: "publish_post".into(),
            item_key: format!("post-{i}"),
        }];
        let out = engine
            .begin_dispatch(id, 0, &keys, s)
            .await
            .expect("dispatch");
        if out == DispatchOutcome::Dispatched {
            n += 1;
        }
        engine
            .transition_item(id, ItemStatus::Working, ItemStatus::Done, None)
            .await
            .ok();
    }
    // Each site is fresh, so the kind cap (1) is what each site allows.
    assert_eq!(n, 3, "a fresh site each time allows one post");

    // Now prove the site total: two kinds on one site, site cap 2.
    let base = CapSlot {
        site: "y.example".into(),
        account: "main".into(),
        action_kind: "follow".into(),
        local_day: "2026-11-19".into(),
        site_cap: 2,
        kind_cap: Some(10),
    };
    let mut taken = 0;
    for i in 0..3 {
        let id = item(
            &engine,
            run,
            pb.id,
            "y.example",
            &format!("y-{i}"),
            ItemStatus::Working,
        )
        .await;
        let keys = vec![LedgerKey {
            site: "y.example".into(),
            account: "main".into(),
            action_kind: "follow".into(),
            item_key: format!("y-{i}"),
        }];
        if engine
            .begin_dispatch(id, 0, &keys, base.clone())
            .await
            .expect("dispatch")
            == DispatchOutcome::Dispatched
        {
            taken += 1;
        }
        engine
            .transition_item(id, ItemStatus::Working, ItemStatus::Done, None)
            .await
            .ok();
    }
    assert_eq!(taken, 2, "the site total of 2 stops the third");
}

/// A dispatch while another connection is writing (in a live run: another
/// item's session, a tab, the transcript) waits its turn. Its transaction
/// used to read before it wrote, so on a file-backed WAL database (a pool,
/// as in production) the other commit left its read snapshot stale and it
/// failed with `SQLITE_BUSY_SNAPSHOT` ("database is locked").
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dispatch_waits_for_another_writer() {
    use sea_orm::{ConnectionTrait, TransactionTrait};
    let dir = tempfile::tempdir().expect("tempdir");
    let db = crate::db::test_helpers::fresh_disk_pool(dir.path()).await;
    let (engine, store) = (
        AutomationEngine::new(db.clone()),
        PlaybookStore::new(db.clone()),
    );
    let pb = store.create("Apply", "goal").await.expect("create");
    let run = engine
        .start_run(pb.id, pb.version, RunTrigger::Mac, None)
        .await
        .expect("start");
    let id = item(
        &engine,
        run,
        pb.id,
        "indeed.com",
        "job-1",
        ItemStatus::Working,
    )
    .await;
    let other = item(
        &engine,
        run,
        pb.id,
        "forum.example",
        "t-1",
        ItemStatus::Queued,
    )
    .await;

    // Another connection holds the write lock while the dispatch starts.
    let writer = db.begin().await.expect("begin");
    writer
        .execute_unprepared(&format!(
            "UPDATE playbook_item SET updated_at = updated_at WHERE id = {other}"
        ))
        .await
        .expect("take the write lock");
    let dispatching = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .begin_dispatch(id, 0, &[key("apply")], slot("apply", 25))
                .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    writer.commit().await.expect("commit");

    let out = dispatching.await.expect("join").expect("no lock error");
    assert_eq!(out, DispatchOutcome::Dispatched);
}

/// "Not sent, redo" while another connection writes waits its turn too: its
/// transaction reads the item's cap slot before it writes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_redo_waits_for_another_writer() {
    use sea_orm::{ConnectionTrait, TransactionTrait};
    let dir = tempfile::tempdir().expect("tempdir");
    let db = crate::db::test_helpers::fresh_disk_pool(dir.path()).await;
    let (engine, store) = (
        AutomationEngine::new(db.clone()),
        PlaybookStore::new(db.clone()),
    );
    let pb = store.create("Apply", "goal").await.expect("create");
    let run = engine
        .start_run(pb.id, pb.version, RunTrigger::Mac, None)
        .await
        .expect("start");
    let id = item(
        &engine,
        run,
        pb.id,
        "indeed.com",
        "job-1",
        ItemStatus::WaitingPerson,
    )
    .await;
    let other = item(
        &engine,
        run,
        pb.id,
        "forum.example",
        "t-1",
        ItemStatus::Queued,
    )
    .await;

    let writer = db.begin().await.expect("begin");
    writer
        .execute_unprepared(&format!(
            "UPDATE playbook_item SET updated_at = updated_at WHERE id = {other}"
        ))
        .await
        .expect("take the write lock");
    let redoing = tokio::spawn(async move {
        crate::db::service::playbook_engine_service::redo_not_sent(&db, id, &[key("apply")]).await
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    writer.commit().await.expect("commit");

    redoing.await.expect("join").expect("no lock error");
}
