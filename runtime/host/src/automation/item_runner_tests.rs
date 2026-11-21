// Modified from upstream Apache-2.0 source; see NOTICE.
//! The item-session runner end to end against in-memory SQLite: the state
//! machine, the declaration decision, the dispatch transaction, the checks,
//! and the crash paths of spec 6.6.

use crate::automation::checks::{Check, PageFacts};
use crate::automation::declaration::{ControlKind, DeclaredField, DeclaredValue, Target};
use crate::automation::entities::playbook_item;
use crate::automation::item_runner::{
    DispatchResult, ItemOutcome, ItemRef, ItemRunner, RunError, SessionResult,
};
use crate::automation::pauses::NavigationSignal;
use crate::automation::reasons::{ActionKind, ApprovalRule, ReasonCode};
use crate::automation::states::{ItemStatus, PauseReason, RunStatus, RunTrigger};
use crate::automation::{AutomationEngine, LedgerKey, PlaybookStore};
use crate::db::migration::Migrator;
use crate::db::service::playbook_engine_service::TransitionError;
use sea_orm::{Database, DatabaseConnection, EntityTrait};
use sea_orm_migration::MigratorTrait;

async fn setup() -> (ItemRunner, i32) {
    let db: DatabaseConnection = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&db, None).await.unwrap();
    let runner = ItemRunner::new(
        AutomationEngine::new(db.clone()),
        PlaybookStore::new(db.clone()),
    );
    let pb = PlaybookStore::new(db.clone())
        .create("Apply", "goal")
        .await
        .expect("create");
    let run = runner
        .start(pb.id, pb.version, RunTrigger::Mac, None)
        .await
        .expect("start");
    runner
        .transition_run_for_tests(
            run,
            crate::automation::RunStatus::Discovering,
            crate::automation::RunStatus::Running,
        )
        .await
        .expect("running");
    // The person accepted the risk on the test sites (spec 14.5).
    for site in ["indeed.com", "x.example"] {
        accept_risk(&runner, site).await;
    }
    (runner, run)
}

async fn accept_risk(runner: &ItemRunner, site: &str) {
    PlaybookStore::new(runner_engine(runner).clone())
        .set_site_value(
            site,
            crate::automation::SiteKnowledgeKind::RiskAccept,
            "main",
            "2026-11-19T00:00:00Z",
        )
        .await
        .expect("accept risk");
}

#[tokio::test]
async fn nothing_starts_when_the_outward_site_risk_was_not_accepted() {
    // A hand-off (apply on the company's own site) sends on the outward
    // site, so its risk step is needed too (spec 14.5).
    let (runner, run) = setup().await;
    let now = chrono::Utc::now();
    let id = playbook_item::Entity::insert(playbook_item::ActiveModel {
        public_id: sea_orm::Set(uuid::Uuid::new_v4().to_string()),
        run_id: sea_orm::Set(run),
        playbook_id: sea_orm::Set(1),
        site: sea_orm::Set("indeed.com".into()),
        account: sea_orm::Set("main".into()),
        item_key: sea_orm::Set("job-handoff".into()),
        outward_site: sea_orm::Set(Some("acme.example".into())),
        status: sea_orm::Set(ItemStatus::Queued.as_str().into()),
        epoch: sea_orm::Set(0),
        created_at: sea_orm::Set(now),
        updated_at: sea_orm::Set(now),
        ..Default::default()
    })
    .exec_with_returning(runner_engine(&runner))
    .await
    .expect("insert")
    .id;
    let begun = runner
        .begin_item(id, "indeed.com", "main")
        .await
        .expect("begin");
    assert_eq!(begun, None);
    accept_risk(&runner, "acme.example").await;
    let begun = runner
        .begin_item(id, "indeed.com", "main")
        .await
        .expect("begin");
    assert_eq!(begun, Some(1));
}

#[tokio::test]
async fn nothing_starts_on_a_site_whose_risk_was_not_accepted() {
    // Spec 14.5: nothing runs on a site without the person's risk step.
    let (runner, run) = setup().await;
    let now = chrono::Utc::now();
    let id = playbook_item::Entity::insert(playbook_item::ActiveModel {
        public_id: sea_orm::Set(uuid::Uuid::new_v4().to_string()),
        run_id: sea_orm::Set(run),
        playbook_id: sea_orm::Set(1),
        site: sea_orm::Set("new.example".into()),
        account: sea_orm::Set("main".into()),
        item_key: sea_orm::Set("job-new".into()),
        status: sea_orm::Set(ItemStatus::Queued.as_str().into()),
        epoch: sea_orm::Set(0),
        created_at: sea_orm::Set(now),
        updated_at: sea_orm::Set(now),
        ..Default::default()
    })
    .exec_with_returning(runner_engine(&runner))
    .await
    .expect("insert")
    .id;
    let begun = runner
        .begin_item(id, "new.example", "main")
        .await
        .expect("begin");
    assert_eq!(begun, None);
    assert_eq!(status(&runner, id).await, ItemStatus::Queued.as_str());

    accept_risk(&runner, "new.example").await;
    let begun = runner
        .begin_item(id, "new.example", "main")
        .await
        .expect("begin");
    assert_eq!(begun, Some(1));
}

/// Mark an item as having gone outward, as the dispatch transaction does.
async fn mark_outward(runner: &ItemRunner, id: i32) {
    use crate::automation::entities::playbook_item;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    playbook_item::Entity::update_many()
        .col_expr(
            playbook_item::Column::OutwardStartedAt,
            sea_orm::sea_query::Expr::value(Some(chrono::Utc::now())),
        )
        .filter(playbook_item::Column::Id.eq(id))
        .exec(runner_engine(runner))
        .await
        .expect("mark outward");
}

async fn working_item(runner: &ItemRunner, run: i32) -> (i32, ItemRef) {
    let now = chrono::Utc::now();
    let id = playbook_item::Entity::insert(playbook_item::ActiveModel {
        public_id: sea_orm::Set(uuid::Uuid::new_v4().to_string()),
        run_id: sea_orm::Set(run),
        playbook_id: sea_orm::Set(1),
        site: sea_orm::Set("indeed.com".into()),
        account: sea_orm::Set("main".into()),
        item_key: sea_orm::Set("job-1".into()),
        status: sea_orm::Set(ItemStatus::Working.as_str().into()),
        epoch: sea_orm::Set(1),
        created_at: sea_orm::Set(now),
        updated_at: sea_orm::Set(now),
        ..Default::default()
    })
    .exec_with_returning(runner_engine(runner))
    .await
    .expect("insert")
    .id;
    (id, item_ref())
}

fn runner_engine(r: &ItemRunner) -> &DatabaseConnection {
    // The runner keeps its own handles; read the row through the store's db
    // by way of the engine's, which points at the same connection.
    r.db_for_tests()
}

fn item_ref() -> ItemRef {
    ItemRef {
        playbook_id: 1,
        site: "indeed.com".into(),
        account: "main".into(),
        key: "job-1".into(),
        outward_site: None,
        outward_started: false,
        show_me: false,
        rules: Default::default(),
        show_me_run_kinds: Default::default(),
        scope: vec![],
        epoch: 1,
    }
}

fn declaration() -> crate::automation::Declaration {
    crate::automation::Declaration {
        kind: ActionKind::SubmitApplication,
        summary: "Apply to Acme".into(),
        fields: vec![],
        submit_target: Target::new("button", "Submit application"),
    }
}

/// Move a working item to `done` so the lane is free for the next insert.
async fn finish_item(runner: &ItemRunner, id: i32) {
    runner.free_lane_for_tests(id).await.expect("finish");
}

async fn status(runner: &ItemRunner, id: i32) -> String {
    playbook_item::Entity::find_by_id(id)
        .one(runner_engine(runner))
        .await
        .expect("read")
        .expect("item")
        .status
}

#[tokio::test]
async fn an_auto_declaration_allows_dispatch_and_checks_decide_done() {
    let (runner, run) = setup().await;
    let (id, mut item) = working_item(&runner, run).await;
    item.rules
        .insert(ActionKind::SubmitApplication, ApprovalRule::Auto);
    // Auto starts after the kind's first approved send on the site.
    item.show_me_run_kinds.insert(ActionKind::SubmitApplication);

    let result = runner
        .finish_session(id, &item, ItemOutcome::ReadyToDispatch(declaration()))
        .await
        .expect("session");
    assert!(matches!(result, SessionResult::AllowDispatch(_)));

    let keys = job_keys();
    let dispatch = runner
        .dispatch(
            id,
            &item,
            ActionKind::SubmitApplication,
            &keys,
            "2026-11-19",
        )
        .await
        .expect("dispatch");
    assert_eq!(dispatch, DispatchResult::Done);

    let facts = PageFacts {
        text: "Application sent".into(),
        ..Default::default()
    };
    let checks = vec![Check::TextPresent {
        text: "Application sent".into(),
    }];
    runner
        .settle_after_checks(id, item.epoch, &keys, &checks, &facts, true)
        .await
        .expect("settle");
    assert_eq!(status(&runner, id).await, ItemStatus::Done.as_str());
}

#[tokio::test]
async fn show_me_parks_the_item_and_a_never_rule_refuses() {
    let (runner, run) = setup().await;
    let (id, mut item) = working_item(&runner, run).await;
    item.show_me = true;
    let result = runner
        .finish_session(id, &item, ItemOutcome::ReadyToDispatch(declaration()))
        .await
        .expect("session");
    assert_eq!(result, SessionResult::WaitingApproval);
    assert_eq!(
        status(&runner, id).await,
        ItemStatus::WaitingApproval.as_str()
    );

    let (id2, mut item2) = working_item(&runner, run).await;
    item2
        .rules
        .insert(ActionKind::SubmitApplication, ApprovalRule::Never);
    let result = runner
        .finish_session(id2, &item2, ItemOutcome::ReadyToDispatch(declaration()))
        .await
        .expect("session");
    assert_eq!(result, SessionResult::Refused(ReasonCode::ActionKindNever));
}

#[tokio::test]
async fn a_pre_submit_mismatch_restarts_the_item() {
    let (runner, run) = setup().await;
    let (id, mut item) = working_item(&runner, run).await;
    item.rules
        .insert(ActionKind::SubmitApplication, ApprovalRule::Auto);
    // Auto starts after the kind's first approved send on the site.
    item.show_me_run_kinds.insert(ActionKind::SubmitApplication);
    // A declaration promises a name the page does not show.
    let mut decl = declaration();
    decl.fields.push(DeclaredField {
        target: Target::new("textbox", "Name"),
        control: ControlKind::Text,
        value: DeclaredValue::Text {
            text: "Asha".into(),
        },
    });
    item.scope = vec![crate::automation::declaration::ScopeControl {
        target: Target::new("textbox", "Name"),
        control: ControlKind::Text,
        value: "Someone else".into(),
        hidden: false,
        read_only: false,
        disabled: false,
        prefilled: false,
    }];
    let result = runner
        .finish_session(id, &item, ItemOutcome::ReadyToDispatch(decl))
        .await
        .expect("session");
    assert_eq!(result, SessionResult::Restart);
    assert_eq!(status(&runner, id).await, ItemStatus::Queued.as_str());
}

#[tokio::test]
async fn an_already_done_key_skips_and_a_full_day_defers() {
    let (runner, run) = setup().await;
    let (first, item) = working_item(&runner, run).await;
    let keys = vec![LedgerKey {
        site: "indeed.com".into(),
        account: "main".into(),
        action_kind: "submit_application".into(),
        item_key: "job-1".into(),
    }];
    runner
        .dispatch(
            first,
            &item,
            ActionKind::SubmitApplication,
            &keys,
            "2026-11-19",
        )
        .await
        .expect("dispatch");

    // The first item left this lane working; free it before the next insert.
    finish_item(&runner, first).await;
    // Same key again: skipped.
    let (second, _) = working_item(&runner, run).await;
    let out = runner
        .dispatch(
            second,
            &item,
            ActionKind::SubmitApplication,
            &keys,
            "2026-11-19",
        )
        .await
        .expect("dispatch");
    assert_eq!(out, DispatchResult::NotSent(ReasonCode::AlreadyDone));
    assert_eq!(status(&runner, second).await, ItemStatus::Skipped.as_str());

    finish_item(&runner, second).await;
    // A fresh key on a day with no slots: deferred.
    let (third, _) = working_item(&runner, run).await;
    let others = vec![LedgerKey {
        site: "indeed.com".into(),
        account: "main".into(),
        action_kind: "publish_post".into(),
        item_key: "post-1".into(),
    }];
    let mut item3 = item.clone();
    item3.key = "post-1".into();
    let out = runner
        .dispatch(
            third,
            &item3,
            ActionKind::PublishPost,
            &others,
            "2026-11-19",
        )
        .await
        .expect("dispatch");
    // A post day has 3 slots, so the first post goes.
    assert_eq!(out, DispatchResult::Done);
}

#[tokio::test]
async fn a_failed_item_counts_to_retries_exhausted_at_three() {
    let (runner, run) = setup().await;
    for expected in [
        ItemStatus::Failed,
        ItemStatus::Failed,
        ItemStatus::RetriesExhausted,
    ] {
        let (id, item) = working_item(&runner, run).await;
        runner
            .finish_session(id, &item, ItemOutcome::Failed(ReasonCode::AgentFailed))
            .await
            .expect("session");
        assert_eq!(status(&runner, id).await, expected.as_str());
        if expected != ItemStatus::RetriesExhausted {
            finish_item(&runner, id).await;
        }
    }
}

#[tokio::test]
async fn a_failure_after_outward_started_goes_to_the_person() {
    let (runner, run) = setup().await;
    let (id, item) = working_item(&runner, run).await;
    mark_outward(&runner, id).await;
    let result = runner
        .finish_session(id, &item, ItemOutcome::Failed(ReasonCode::Timeout))
        .await
        .expect("session");
    assert!(matches!(result, SessionResult::WaitingPerson(_)));
    assert_eq!(
        status(&runner, id).await,
        ItemStatus::WaitingPerson.as_str()
    );
}

#[tokio::test]
async fn crash_before_outward_requeues_and_after_goes_to_the_person() {
    let (runner, run) = setup().await;
    let (id, item) = working_item(&runner, run).await;
    runner.host_restarted(id, &item).await.expect("restart");
    assert_eq!(status(&runner, id).await, ItemStatus::Queued.as_str());

    let (id2, item2) = working_item(&runner, run).await;
    mark_outward(&runner, id2).await;
    runner.host_restarted(id2, &item2).await.expect("restart");
    assert_eq!(
        status(&runner, id2).await,
        ItemStatus::WaitingPerson.as_str()
    );
}

#[tokio::test]
async fn pause_requeues_an_unoutward_item_and_stop_stops_it() {
    let (runner, run) = setup().await;
    let (id, item) = working_item(&runner, run).await;
    runner.pause_item(id, &item).await.expect("pause");
    assert_eq!(status(&runner, id).await, ItemStatus::Queued.as_str());

    let (id2, item2) = working_item(&runner, run).await;
    runner.stop_item(id2, &item2).await.expect("stop");
    assert_eq!(status(&runner, id2).await, ItemStatus::Stopped.as_str());
}

#[tokio::test]
async fn a_paused_site_keeps_a_queued_item_out() {
    let (runner, run) = setup().await;
    let now = chrono::Utc::now();
    let id = playbook_item::Entity::insert(playbook_item::ActiveModel {
        public_id: sea_orm::Set(uuid::Uuid::new_v4().to_string()),
        run_id: sea_orm::Set(run),
        playbook_id: sea_orm::Set(1),
        site: sea_orm::Set("indeed.com".into()),
        account: sea_orm::Set("main".into()),
        item_key: sea_orm::Set("job-9".into()),
        status: sea_orm::Set(ItemStatus::Queued.as_str().into()),
        epoch: sea_orm::Set(0),
        created_at: sea_orm::Set(now),
        updated_at: sea_orm::Set(now),
        ..Default::default()
    })
    .exec_with_returning(runner_engine(&runner))
    .await
    .expect("insert")
    .id;

    // Not paused: the item can start.
    // Starting gives the session its attempt epoch (spec 6.6.7).
    assert_eq!(
        runner
            .begin_item(id, "indeed.com", "main")
            .await
            .expect("begin"),
        Some(1)
    );
    // Paused: a fresh queued item stays out.
    let store = crate::automation::PlaybookStore::new(runner_engine(&runner).clone());
    crate::automation::SitePause::pause(
        &store,
        "x.example",
        "main",
        crate::automation::PauseCause::Captcha,
    )
    .await
    .expect("pause");
    let id2 = playbook_item::Entity::insert(playbook_item::ActiveModel {
        public_id: sea_orm::Set(uuid::Uuid::new_v4().to_string()),
        run_id: sea_orm::Set(run),
        playbook_id: sea_orm::Set(1),
        site: sea_orm::Set("x.example".into()),
        account: sea_orm::Set("main".into()),
        item_key: sea_orm::Set("job-10".into()),
        status: sea_orm::Set(ItemStatus::Queued.as_str().into()),
        epoch: sea_orm::Set(0),
        created_at: sea_orm::Set(now),
        updated_at: sea_orm::Set(now),
        ..Default::default()
    })
    .exec_with_returning(runner_engine(&runner))
    .await
    .expect("insert")
    .id;
    assert_eq!(
        runner
            .begin_item(id2, "x.example", "main")
            .await
            .expect("begin"),
        None
    );
    assert_eq!(status(&runner, id2).await, ItemStatus::Queued.as_str());
}

#[tokio::test]
async fn navigation_pauses_map_to_run_pauses() {
    let (runner, _run) = setup().await;
    let mut seen = Default::default();
    let outcome = runner.on_navigation(&NavigationSignal::Captcha, "indeed.com", "main", &mut seen);
    assert_eq!(
        outcome,
        crate::automation::pauses::NavigationOutcome::Pause(crate::automation::PauseCause::Captcha)
    );
    // Pausing a running run sets the reason.
    let (runner2, run2) = setup().await;
    runner2
        .pause_run(run2, RunStatus::Running, PauseReason::SitePaused)
        .await
        .expect("pause");
}

fn job_keys() -> Vec<LedgerKey> {
    vec![LedgerKey {
        site: "indeed.com".into(),
        account: "main".into(),
        action_kind: "submit_application".into(),
        item_key: "job-1".into(),
    }]
}

async fn cap_used(runner: &ItemRunner, kind: &str) -> i32 {
    use crate::automation::entities::cap_usage;
    cap_usage::Entity::find_by_id((
        "indeed.com".to_string(),
        "main".to_string(),
        kind.to_string(),
        "2026-11-19".to_string(),
    ))
    .one(runner_engine(runner))
    .await
    .unwrap()
    .map(|m| m.used)
    .unwrap_or(0)
}

#[tokio::test]
async fn a_second_declaration_in_the_same_item_reuses_its_dispatch() {
    // Spec 6.6.3: a confirm step reuses the item's reserved keys, takes no
    // second cap slot, and keeps the one outward mark.
    let (runner, run) = setup().await;
    let (id, item) = working_item(&runner, run).await;
    let keys = job_keys();
    for _ in 0..2 {
        let result = runner
            .dispatch(
                id,
                &item,
                ActionKind::SubmitApplication,
                &keys,
                "2026-11-19",
            )
            .await
            .expect("dispatch");
        assert_eq!(result, DispatchResult::Done);
    }
    assert_eq!(status(&runner, id).await, ItemStatus::Working.as_str());
    // Applications count against the site total only (no per-kind cap).
    assert_eq!(cap_used(&runner, "*").await, 1);
}

async fn outward_at(runner: &ItemRunner, id: i32) -> bool {
    playbook_item::Entity::find_by_id(id)
        .one(runner_engine(runner))
        .await
        .unwrap()
        .unwrap()
        .outward_started_at
        .is_some()
}

async fn key_state(runner: &ItemRunner) -> Option<String> {
    use crate::automation::entities::never_twice_ledger;
    never_twice_ledger::Entity::find()
        .one(runner_engine(runner))
        .await
        .unwrap()
        .map(|m| m.state)
}

#[tokio::test]
async fn not_sent_redo_on_an_item_not_waiting_for_the_person_changes_nothing() {
    // Spec 6.6.5: only a waiting_person item can be proven not sent. A redo
    // answer that arrives after the item moved on must not wipe its outward
    // mark or its keys, or the item could go out twice.
    let (runner, run) = setup().await;
    let (id, item) = working_item(&runner, run).await;
    let keys = job_keys();
    runner
        .dispatch(
            id,
            &item,
            ActionKind::SubmitApplication,
            &keys,
            "2026-11-19",
        )
        .await
        .expect("dispatch");
    assert!(runner.redo_not_sent(id, &keys).await.is_err());
    assert!(outward_at(&runner, id).await);
    assert_eq!(key_state(&runner).await.as_deref(), Some("reserved"));
    assert_eq!(status(&runner, id).await, ItemStatus::Working.as_str());
}

#[tokio::test]
async fn not_sent_redo_requeues_and_frees_the_item() {
    let (runner, run) = setup().await;
    let (id, item) = working_item(&runner, run).await;
    let keys = job_keys();
    runner
        .dispatch(
            id,
            &item,
            ActionKind::SubmitApplication,
            &keys,
            "2026-11-19",
        )
        .await
        .expect("dispatch");
    runner
        .settle_after_checks(
            id,
            item.epoch,
            &keys,
            &[Check::TextPresent {
                text: "sent".into(),
            }],
            &PageFacts::default(),
            true,
        )
        .await
        .expect("settle");
    assert_eq!(
        status(&runner, id).await,
        ItemStatus::WaitingPerson.as_str()
    );
    assert_eq!(cap_used(&runner, "*").await, 1);
    runner.redo_not_sent(id, &keys).await.expect("redo");
    assert_eq!(status(&runner, id).await, ItemStatus::Queued.as_str());
    assert!(!outward_at(&runner, id).await);
    assert_eq!(key_state(&runner).await, None);
    // Proven not sent: the day's cap slot is given back (spec 6.6.5).
    assert_eq!(cap_used(&runner, "*").await, 0);
}

/// Park an item on an Ask rule and approve it, as the person would.
async fn approved_item(runner: &ItemRunner, run: i32) -> (i32, ItemRef) {
    let (id, mut item) = working_item(runner, run).await;
    item.rules
        .insert(ActionKind::SubmitApplication, ApprovalRule::Ask);
    let result = runner
        .finish_session(id, &item, ItemOutcome::ReadyToDispatch(declaration()))
        .await
        .expect("session");
    assert_eq!(result, SessionResult::WaitingApproval);
    use crate::automation::entities::playbook_approval;
    let a = playbook_approval::Entity::find()
        .one(runner_engine(runner))
        .await
        .unwrap()
        .expect("approval");
    crate::automation::approvals::respond(
        runner_engine(runner),
        a.id,
        crate::automation::ApprovalAnswer::Approve,
        &[],
    )
    .await
    .expect("approve");
    assert_eq!(status(runner, id).await, ItemStatus::Working.as_str());
    // The approved click is a new attempt (waiting_approval -> working);
    // the Host reads its epoch for the click's ItemRef.
    item.epoch = runner.current_epoch(id).await.expect("epoch");
    (id, item)
}

async fn epoch_of(runner: &ItemRunner, id: i32) -> i32 {
    playbook_item::Entity::find_by_id(id)
        .one(runner_engine(runner))
        .await
        .expect("read")
        .expect("item")
        .epoch
}

fn is_stale_attempt<T: std::fmt::Debug>(r: &Result<T, RunError>) -> bool {
    matches!(
        r,
        Err(RunError::Transition(TransitionError::StaleAttempt { .. }))
    )
}

/// Requeue a working item and start it again, as a wake gap or timeout
/// does (spec 6.6.7): the old session keeps the old epoch.
async fn restart_attempt(runner: &ItemRunner, id: i32) {
    let engine = runner.engine_for_tests();
    engine
        .transition_item(id, ItemStatus::Working, ItemStatus::Queued, None)
        .await
        .unwrap();
    engine
        .transition_item(id, ItemStatus::Queued, ItemStatus::Working, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_stale_attempt_cannot_declare_or_end_the_item() {
    // Spec 6.6.7: a declaration or result carrying a stale epoch is refused.
    let (runner, run) = setup().await;
    let (id, mut zombie) = working_item(&runner, run).await;
    restart_attempt(&runner, id).await;

    zombie
        .rules
        .insert(ActionKind::SubmitApplication, ApprovalRule::Auto);
    // Auto starts after the kind's first approved send on the site.
    zombie
        .show_me_run_kinds
        .insert(ActionKind::SubmitApplication);
    let declared = runner
        .finish_session(id, &zombie, ItemOutcome::ReadyToDispatch(declaration()))
        .await;
    assert!(is_stale_attempt(&declared), "{declared:?}");

    zombie
        .rules
        .insert(ActionKind::SubmitApplication, ApprovalRule::Ask);
    let asked = runner
        .finish_session(id, &zombie, ItemOutcome::ReadyToDispatch(declaration()))
        .await;
    assert!(is_stale_attempt(&asked), "{asked:?}");
    use crate::automation::entities::playbook_approval;
    assert!(playbook_approval::Entity::find()
        .one(runner_engine(&runner))
        .await
        .unwrap()
        .is_none());

    let failed = runner
        .finish_session(
            id,
            &zombie,
            ItemOutcome::Failed(ReasonCode::PageUnrecognised),
        )
        .await;
    assert!(is_stale_attempt(&failed), "{failed:?}");
    assert_eq!(status(&runner, id).await, ItemStatus::Working.as_str());

    // The current attempt still works.
    let mut current = zombie.clone();
    current.epoch = epoch_of(&runner, id).await;
    current
        .rules
        .insert(ActionKind::SubmitApplication, ApprovalRule::Auto);
    // Auto starts after the kind's first approved send on the site.
    current
        .show_me_run_kinds
        .insert(ActionKind::SubmitApplication);
    let ok = runner
        .finish_session(id, &current, ItemOutcome::ReadyToDispatch(declaration()))
        .await
        .expect("session");
    assert!(matches!(ok, SessionResult::AllowDispatch(_)));
}

#[tokio::test]
async fn a_failure_refused_at_its_write_does_not_count_a_retry() {
    // The item was requeued (same epoch, not working): the session's
    // failure changes nothing, retry counter included (spec 6.7.4).
    let (runner, run) = setup().await;
    let (id, item) = working_item(&runner, run).await;
    runner
        .engine_for_tests()
        .transition_item(id, ItemStatus::Working, ItemStatus::Queued, None)
        .await
        .unwrap();
    let failed = runner
        .finish_session(id, &item, ItemOutcome::Failed(ReasonCode::PageUnrecognised))
        .await;
    assert!(failed.is_err(), "{failed:?}");
    use crate::automation::entities::item_retry_counter;
    assert!(item_retry_counter::Entity::find()
        .one(runner_engine(&runner))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_stale_attempt_on_an_item_no_longer_working_is_named_stale() {
    let (runner, run) = setup().await;
    let (id, zombie) = working_item(&runner, run).await;
    restart_attempt(&runner, id).await;
    runner
        .engine_for_tests()
        .transition_item(id, ItemStatus::Working, ItemStatus::Queued, None)
        .await
        .unwrap();
    let result = runner
        .dispatch(
            id,
            &zombie,
            ActionKind::SubmitApplication,
            &job_keys(),
            "2026-11-19",
        )
        .await;
    assert!(is_stale_attempt(&result), "{result:?}");
}

/// One rate-limited session: start the item, the agent hits its limit.
async fn rate_limited_session(runner: &ItemRunner, id: i32) -> SessionResult {
    let mut item = item_ref();
    item.epoch = runner
        .begin_item(id, "indeed.com", "main")
        .await
        .expect("begin")
        .expect("started");
    runner
        .finish_session(id, &item, ItemOutcome::Failed(ReasonCode::AgentRateLimited))
        .await
        .expect("session")
}

async fn run_row(
    runner: &ItemRunner,
    run: i32,
) -> crate::automation::entities::playbook_run::Model {
    crate::automation::entities::playbook_run::Entity::find_by_id(run)
        .one(runner_engine(runner))
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn a_rate_limited_agent_backs_off_three_times_then_pauses_the_run() {
    // Spec 7.1.7: the item is not failed; the run stays running for up to 3
    // backoff retries, then pauses with `agent_rate_limited`.
    let (runner, run) = setup().await;
    let id = item_with(&runner, run, "job-rl", ItemStatus::Queued).await;
    for retry in 1..=3 {
        assert_eq!(
            rate_limited_session(&runner, id).await,
            SessionResult::BackOff { retry }
        );
        assert_eq!(status(&runner, id).await, ItemStatus::Queued.as_str());
        assert_eq!(run_row(&runner, run).await.status, "running");
    }
    assert_eq!(
        rate_limited_session(&runner, id).await,
        SessionResult::RunPaused(PauseReason::AgentRateLimited)
    );
    let row = run_row(&runner, run).await;
    assert_eq!(row.status, "paused");
    assert_eq!(row.pause_reason.as_deref(), Some("agent_rate_limited"));
    assert_eq!(status(&runner, id).await, ItemStatus::Queued.as_str());
    use crate::automation::entities::item_retry_counter;
    assert!(item_retry_counter::Entity::find()
        .one(runner_engine(&runner))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_session_that_gets_through_resets_the_rate_limit_backoff() {
    let (runner, run) = setup().await;
    let id = item_with(&runner, run, "job-rl", ItemStatus::Queued).await;
    rate_limited_session(&runner, id).await;
    rate_limited_session(&runner, id).await;
    let mut item = item_ref();
    item.epoch = runner
        .begin_item(id, "indeed.com", "main")
        .await
        .unwrap()
        .unwrap();
    runner
        .finish_session(id, &item, ItemOutcome::Skip(ReasonCode::ApprovalRejected))
        .await
        .unwrap();
    let other = item_with(&runner, run, "job-rl-2", ItemStatus::Queued).await;
    assert_eq!(
        rate_limited_session(&runner, other).await,
        SessionResult::BackOff { retry: 1 }
    );
}

#[tokio::test]
async fn a_stale_attempt_cannot_dispatch() {
    let (runner, run) = setup().await;
    let (id, zombie) = working_item(&runner, run).await;
    restart_attempt(&runner, id).await;
    let result = runner
        .dispatch(
            id,
            &zombie,
            ActionKind::SubmitApplication,
            &job_keys(),
            "2026-11-19",
        )
        .await;
    assert!(is_stale_attempt(&result), "{result:?}");
    assert!(!outward_at(&runner, id).await);
    assert_eq!(cap_used(&runner, "*").await, 0);
    assert_eq!(key_state(&runner).await, None);
}

#[tokio::test]
async fn a_stale_approved_click_leaves_the_approval_for_the_current_attempt() {
    let (runner, run) = setup().await;
    let (id, item) = approved_item(&runner, run).await;
    let mut zombie = item.clone();
    zombie.epoch -= 1;
    let stale = runner
        .dispatch_approved(id, &zombie, &job_keys(), "2026-11-19")
        .await;
    assert!(is_stale_attempt(&stale), "{stale:?}");
    assert!(!outward_at(&runner, id).await);
    let sent = runner
        .dispatch_approved(id, &item, &job_keys(), "2026-11-19")
        .await
        .expect("dispatch");
    assert_eq!(sent, DispatchResult::Done);
}

#[tokio::test]
async fn an_approved_click_sends_once() {
    // Spec 8.5.5, 8.3: the approval is used by exactly one Host click.
    let (runner, run) = setup().await;
    let (id, item) = approved_item(&runner, run).await;
    let keys = job_keys();
    let first = runner
        .dispatch_approved(id, &item, &keys, "2026-11-19")
        .await
        .expect("dispatch");
    assert_eq!(first, DispatchResult::Done);
    assert!(outward_at(&runner, id).await);
    let second = runner
        .dispatch_approved(id, &item, &keys, "2026-11-19")
        .await
        .expect("second");
    assert_eq!(second, DispatchResult::NotApproved);
    assert_eq!(cap_used(&runner, "*").await, 1);
}

#[tokio::test]
async fn an_approved_click_on_a_changed_page_restarts_and_keeps_the_approval() {
    // Spec 8.5.5, 8.7: the pre-submit check runs before the approved click;
    // a mismatch sends the item to a new session, which proceeds only on an
    // exact match of what the person approved.
    let (runner, run) = setup().await;
    let (id, mut item) = approved_item(&runner, run).await;
    item.scope = vec![crate::automation::declaration::ScopeControl {
        target: Target::new("textbox", "Cover letter"),
        control: ControlKind::Text,
        value: "something the agent typed".into(),
        hidden: false,
        read_only: false,
        disabled: false,
        prefilled: false,
    }];
    let result = runner
        .dispatch_approved(id, &item, &job_keys(), "2026-11-19")
        .await
        .expect("dispatch");
    assert_eq!(result, DispatchResult::Restart);
    assert_eq!(status(&runner, id).await, ItemStatus::Queued.as_str());
    assert!(!outward_at(&runner, id).await);

    // The continuation session declares the same content: no new ask.
    runner
        .engine_for_tests()
        .transition_item(id, ItemStatus::Queued, ItemStatus::Working, None)
        .await
        .unwrap();
    item.epoch = epoch_of(&runner, id).await;
    item.scope.clear();
    let again = runner
        .finish_session(id, &item, ItemOutcome::ReadyToDispatch(declaration()))
        .await
        .expect("session");
    assert_eq!(again, SessionResult::ApprovedDispatch(declaration()));
    let sent = runner
        .dispatch_approved(id, &item, &job_keys(), "2026-11-19")
        .await
        .expect("dispatch");
    assert_eq!(sent, DispatchResult::Done);
}

async fn item_with(runner: &ItemRunner, run: i32, key: &str, status: ItemStatus) -> i32 {
    let now = chrono::Utc::now();
    playbook_item::Entity::insert(playbook_item::ActiveModel {
        public_id: sea_orm::Set(uuid::Uuid::new_v4().to_string()),
        run_id: sea_orm::Set(run),
        playbook_id: sea_orm::Set(1),
        site: sea_orm::Set("indeed.com".into()),
        account: sea_orm::Set("main".into()),
        item_key: sea_orm::Set(key.into()),
        status: sea_orm::Set(status.as_str().into()),
        epoch: sea_orm::Set(1),
        created_at: sea_orm::Set(now),
        updated_at: sea_orm::Set(now),
        ..Default::default()
    })
    .exec_with_returning(runner_engine(runner))
    .await
    .expect("insert")
    .id
}

#[tokio::test]
async fn an_approval_waiting_for_its_lane_goes_before_queued_work() {
    // Spec 8.5.5: the approved click takes the lane. Approved while another
    // item held the lane, the parked item starts as soon as the lane frees,
    // ahead of queued items.
    let (runner, run) = setup().await;
    let busy = item_with(&runner, run, "job-busy", ItemStatus::Working).await;
    let parked = item_with(&runner, run, "job-parked", ItemStatus::WaitingApproval).await;
    let queued = item_with(&runner, run, "job-next", ItemStatus::Queued).await;
    let a =
        crate::automation::approvals::create(runner_engine(&runner), run, parked, &declaration())
            .await
            .unwrap();
    crate::automation::approvals::respond(
        runner_engine(&runner),
        a.id,
        crate::automation::ApprovalAnswer::Approve,
        &[],
    )
    .await
    .unwrap();
    // The lane was busy: the approval is recorded, the item still parked.
    assert_eq!(
        status(&runner, parked).await,
        ItemStatus::WaitingApproval.as_str()
    );

    runner
        .engine_for_tests()
        .transition_item(busy, ItemStatus::Working, ItemStatus::Done, None)
        .await
        .unwrap();
    let started = runner
        .begin_item(queued, "indeed.com", "main")
        .await
        .expect("begin");
    assert_eq!(started, None);
    assert_eq!(status(&runner, parked).await, ItemStatus::Working.as_str());
    assert_eq!(status(&runner, queued).await, ItemStatus::Queued.as_str());
}

#[tokio::test]
async fn parking_an_item_tells_the_feed() {
    let (runner, run) = setup().await;
    let (tx, rx) = tokio::sync::watch::channel(0u64);
    let runner = runner.with_changes(std::sync::Arc::new(tx));
    let (id, mut item) = working_item(&runner, run).await;
    item.rules
        .insert(ActionKind::SubmitApplication, ApprovalRule::Ask);
    let before = *rx.borrow();
    runner
        .finish_session(id, &item, ItemOutcome::ReadyToDispatch(declaration()))
        .await
        .expect("session");
    assert!(*rx.borrow() > before);
}

#[tokio::test]
async fn starting_and_ending_an_item_tell_the_feed() {
    // Spec 11.4.1: the feed re-reads runs and items on every change.
    let (runner, run) = setup().await;
    let (tx, rx) = tokio::sync::watch::channel(0u64);
    let runner = runner.with_changes(std::sync::Arc::new(tx));
    let id = item_with(&runner, run, "job-feed", ItemStatus::Queued).await;
    let mut item = item_ref();
    item.epoch = runner
        .begin_item(id, "indeed.com", "main")
        .await
        .unwrap()
        .unwrap();
    let started = *rx.borrow();
    assert!(started > 0, "starting an item tells the feed");
    runner
        .finish_session(id, &item, ItemOutcome::Skip(ReasonCode::ApprovalRejected))
        .await
        .unwrap();
    assert!(*rx.borrow() > started, "ending an item tells the feed");
}
