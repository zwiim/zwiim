//! Zwiim-owned: what runs and their browser lanes tell each other. What a
//! lane's page asks of a person reaches the run (automation-engine spec
//! 14.3): a sign-in pauses the lane (10.3), any other ask parks the lane's
//! working item for the person. The browser learns which lanes have a
//! working item, whose tabs memory pressure never takes (15.4).

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::collections::HashSet;
use std::sync::Weak;

use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::watch;

use crate::automation::engine::AutomationEngine;
use crate::automation::entities::playbook_item;
use crate::automation::pauses::{PauseCause, SitePause};
use crate::automation::playbook_store::PlaybookStore;
use crate::automation::reasons::ReasonCode;
use crate::automation::states::ItemStatus;
use crate::browser::service::{BrowserEvent, BrowserService, LaneNeed};
use crate::browser::store::TabOwner;
use crate::db::error::DbError;
use crate::db::service::playbook_engine_service::TransitionError;

#[derive(Debug, thiserror::Error)]
pub enum LaneAskError {
    #[error(transparent)]
    Transition(#[from] TransitionError),
    #[error(transparent)]
    Db(#[from] DbError),
}

/// Lane (`site`, `account`)'s page needs a person.
pub async fn on_lane_ask(
    engine: &AutomationEngine,
    site: &str,
    account: &str,
    need: LaneNeed,
) -> Result<(), LaneAskError> {
    let working = working_on(engine.db(), site, account).await?;
    match need {
        LaneNeed::SignIn => {
            let store = PlaybookStore::new(engine.db().clone());
            SitePause::pause(&store, site, account, PauseCause::Login).await?;
            // Not a retry; past `outward_started` it stays for the person.
            if let Some(item) = working.filter(|i| i.outward_started_at.is_none()) {
                engine
                    .transition_item(item.id, ItemStatus::Working, ItemStatus::Queued, None)
                    .await?;
            }
        }
        LaneNeed::Page => {
            if let Some(item) = working {
                engine
                    .transition_item(
                        item.id,
                        ItemStatus::Working,
                        ItemStatus::WaitingPerson,
                        Some(ReasonCode::PageNeedsPerson.as_str()),
                    )
                    .await?;
            }
        }
    }
    Ok(())
}

/// The lane's one working item, if any (the one-tab-per-lane index). An
/// item's lane is its outward site after a hand-off (spec 6.9).
async fn working_on(
    db: &DatabaseConnection,
    site: &str,
    account: &str,
) -> Result<Option<playbook_item::Model>, DbError> {
    Ok(playbook_item::Entity::find()
        .filter(playbook_item::Column::Account.eq(account))
        .filter(playbook_item::Column::Status.eq(ItemStatus::Working.as_str()))
        .all(db)
        .await?
        .into_iter()
        .find(|i| i.outward_site.as_deref().unwrap_or(&i.site) == site))
}

/// The lanes with a working item.
pub async fn working_lanes(db: &DatabaseConnection) -> Result<HashSet<TabOwner>, DbError> {
    let items = playbook_item::Entity::find()
        .filter(playbook_item::Column::Status.eq(ItemStatus::Working.as_str()))
        .all(db)
        .await?;
    Ok(items
        .into_iter()
        .map(|i| TabOwner::Lane {
            site: i.outward_site.unwrap_or(i.site),
            account: i.account,
        })
        .collect())
}

/// Tell `browser` which lanes have a working item after every run or item
/// change, until either goes.
pub fn follow_working(
    mut changes: watch::Receiver<u64>,
    db: DatabaseConnection,
    browser: Weak<BrowserService>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match working_lanes(&db).await {
                Ok(lanes) => match browser.upgrade() {
                    Some(browser) => browser.set_working_lanes(lanes),
                    None => return,
                },
                Err(e) => tracing::warn!(error = %e, "could not read the working lanes"),
            }
            if changes.changed().await.is_err() {
                return;
            }
        }
    })
}

/// Apply every lane ask `events` carries until the browser goes.
pub fn follow(
    mut events: broadcast::Receiver<BrowserEvent>,
    engine: AutomationEngine,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(BrowserEvent::LaneAsk {
                    site,
                    account,
                    need,
                    ..
                }) => {
                    if let Err(e) = on_lane_ask(&engine, &site, &account, need).await {
                        tracing::warn!(error = %e, %site, "a lane's ask did not reach its run");
                    }
                }
                Ok(_) => {}
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "lane asks were missed");
                }
                Err(RecvError::Closed) => return,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::RunTrigger;
    use crate::db::migration::Migrator;
    use sea_orm::Set;
    use sea_orm_migration::MigratorTrait;

    async fn db() -> DatabaseConnection {
        let db = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
        Migrator::up(&db, None).await.unwrap();
        db
    }

    /// A working item on (jobsy.example, main); its row id.
    async fn working_item(db: &DatabaseConnection, outward: bool) -> i32 {
        let pb = PlaybookStore::new(db.clone())
            .create("Apply", "goal")
            .await
            .unwrap();
        let run = AutomationEngine::new(db.clone())
            .start_run(pb.id, pb.version, RunTrigger::Mac, None)
            .await
            .unwrap();
        let now = chrono::Utc::now();
        playbook_item::Entity::insert(playbook_item::ActiveModel {
            public_id: Set(uuid::Uuid::new_v4().to_string()),
            run_id: Set(run),
            playbook_id: Set(pb.id),
            site: Set("jobsy.example".into()),
            account: Set("main".into()),
            item_key: Set("job-1".into()),
            status: Set("working".into()),
            epoch: Set(1),
            outward_started_at: Set(outward.then_some(now)),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        })
        .exec_with_returning(db)
        .await
        .unwrap()
        .id
    }

    async fn item(db: &DatabaseConnection, id: i32) -> playbook_item::Model {
        playbook_item::Entity::find_by_id(id)
            .one(db)
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn the_lanes_with_a_working_item_are_known() {
        let db = db().await;
        assert!(working_lanes(&db).await.unwrap().is_empty());
        let id = working_item(&db, false).await;
        assert_eq!(
            working_lanes(&db).await.unwrap(),
            [TabOwner::Lane {
                site: "jobsy.example".into(),
                account: "main".into(),
            }]
            .into()
        );
        AutomationEngine::new(db.clone())
            .transition_item(id, ItemStatus::Working, ItemStatus::Done, None)
            .await
            .unwrap();
        assert!(working_lanes(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_page_ask_parks_the_lanes_working_item_for_the_person() {
        let db = db().await;
        let id = working_item(&db, false).await;
        let engine = AutomationEngine::new(db.clone());
        // Another lane's ask leaves it alone.
        on_lane_ask(&engine, "jobsy.example", "second", LaneNeed::Page)
            .await
            .unwrap();
        assert_eq!(item(&db, id).await.status, "working");
        on_lane_ask(&engine, "jobsy.example", "main", LaneNeed::Page)
            .await
            .unwrap();
        let parked = item(&db, id).await;
        assert_eq!(parked.status, "waiting_person");
        assert_eq!(parked.reason_code.as_deref(), Some("page_needs_person"));
        // Asked again with nothing working: nothing to do.
        on_lane_ask(&engine, "jobsy.example", "main", LaneNeed::Page)
            .await
            .unwrap();
    }

    /// A handed-off item works in its outward site's lane (spec 6.9): that
    /// lane's asks reach it and that lane is the one it holds.
    #[tokio::test]
    async fn a_handed_off_item_belongs_to_its_outward_lane() {
        let db = db().await;
        let id = working_item(&db, false).await;
        let mut moved: playbook_item::ActiveModel = item(&db, id).await.into();
        moved.outward_site = Set(Some("apply.example".into()));
        sea_orm::ActiveModelTrait::update(moved, &db).await.unwrap();
        assert_eq!(
            working_lanes(&db).await.unwrap(),
            [TabOwner::Lane {
                site: "apply.example".into(),
                account: "main".into(),
            }]
            .into()
        );
        let engine = AutomationEngine::new(db.clone());
        on_lane_ask(&engine, "jobsy.example", "main", LaneNeed::Page)
            .await
            .unwrap();
        assert_eq!(item(&db, id).await.status, "working");
        on_lane_ask(&engine, "apply.example", "main", LaneNeed::Page)
            .await
            .unwrap();
        assert_eq!(item(&db, id).await.status, "waiting_person");
    }

    /// Spec 10.3: a sign-in pauses the lane with cause `login` and its item
    /// goes back to the queue, not counted as a retry.
    #[tokio::test]
    async fn a_sign_in_pauses_the_lane_and_requeues_its_item() {
        let db = db().await;
        let id = working_item(&db, false).await;
        let engine = AutomationEngine::new(db.clone());
        on_lane_ask(&engine, "jobsy.example", "main", LaneNeed::SignIn)
            .await
            .unwrap();
        let store = PlaybookStore::new(db.clone());
        let pause = SitePause::current(&store, "jobsy.example", "main")
            .await
            .unwrap()
            .expect("paused");
        assert_eq!(pause.cause, PauseCause::Login);
        let requeued = item(&db, id).await;
        assert_eq!(requeued.status, "queued");
        assert_eq!(requeued.reason_code, None);
    }

    /// Past `outward_started` an item never goes back to the queue (6.4.1).
    #[tokio::test]
    async fn a_sign_in_leaves_an_item_that_may_have_sent() {
        let db = db().await;
        let id = working_item(&db, true).await;
        let engine = AutomationEngine::new(db.clone());
        on_lane_ask(&engine, "jobsy.example", "main", LaneNeed::SignIn)
            .await
            .unwrap();
        assert_eq!(item(&db, id).await.status, "working");
        let store = PlaybookStore::new(db.clone());
        assert!(SitePause::current(&store, "jobsy.example", "main")
            .await
            .unwrap()
            .is_some());
    }

    /// The Host follows the browser: a dialog on a lane's page parks the
    /// lane's item, and the feed hears of it.
    #[tokio::test]
    async fn a_dialog_on_a_lane_page_parks_its_item() {
        use crate::browser::engine::{fake::FakeEngine, manager::EngineManager};
        use crate::browser::service::BrowserService;
        let db = db().await;
        let id = working_item(&db, false).await;
        let fake = FakeEngine::default();
        let browser = BrowserService::start(EngineManager::new(fake.start_fn()), db.clone(), None)
            .await
            .unwrap();
        let changes = std::sync::Arc::new(tokio::sync::watch::channel(0).0);
        let mut changed = changes.subscribe();
        let _follow = follow(
            browser.subscribe(),
            AutomationEngine::new(db.clone()).with_changes(changes),
        );
        let lane = TabOwner::Lane {
            site: "jobsy.example".into(),
            account: "main".into(),
        };
        browser
            .open(lane, Some("https://jobsy.example/"))
            .await
            .unwrap();
        let target = fake.wait_url("https://jobsy.example/").await;
        let page = fake.lock().targets[&target].session.clone().unwrap();
        fake.emit(
            "Page.javascriptDialogOpening",
            serde_json::json!({"type": "confirm", "message": "Visa?", "url": "https://jobsy.example/"}),
            Some(&page),
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), changed.changed())
            .await
            .expect("the feed was told")
            .unwrap();
        assert_eq!(item(&db, id).await.status, "waiting_person");
    }
}
