//! Zwiim-owned: what the Host feed tells clients about runs and their items
//! (automation-engine spec 11.4.1). The feed re-reads these after each
//! playbook change and publishes the ones that differ.

use serde::Serialize;

use crate::automation::entities::{playbook_item, playbook_run};
use crate::automation::states::{ItemStatus, RunStatus};
use crate::db::error::DbError;

/// One run as `run.changed` carries it: its state and item counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunView {
    pub run_id: i32,
    pub playbook_id: i32,
    pub status: String,
    pub pause_reason: Option<String>,
    pub outcome: Option<String>,
    pub totals: RunTotals,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Item counts by outcome (spec 11.1 `totals`). `failed` includes items
/// whose retries are exhausted; `waiting` is approval plus person.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RunTotals {
    pub items: u32,
    pub done: u32,
    pub skipped: u32,
    pub failed: u32,
    pub deferred: u32,
    pub waiting: u32,
}

/// One item as `run.item_changed` carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemView {
    pub run_id: i32,
    pub item_id: i32,
    pub title: Option<String>,
    pub status: String,
    pub reason_code: Option<String>,
}

impl RunView {
    pub fn is_active(&self) -> bool {
        RunStatus::parse(&self.status).is_some_and(|s| !s.is_terminal())
    }
}

/// The active runs, plus `also` (runs the feed was following, so their
/// final state is seen once they end), with their items.
pub async fn read(
    db: &sea_orm::DatabaseConnection,
    also: &[i32],
) -> Result<(Vec<RunView>, Vec<ItemView>), DbError> {
    use sea_orm::{ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder};
    let active = [
        RunStatus::Discovering.as_str(),
        RunStatus::Running.as_str(),
        RunStatus::Paused.as_str(),
    ];
    let runs = playbook_run::Entity::find()
        .filter(
            Condition::any()
                .add(playbook_run::Column::Status.is_in(active))
                .add(playbook_run::Column::Id.is_in(also.iter().copied())),
        )
        .order_by_asc(playbook_run::Column::Id)
        .all(db)
        .await?;
    let items = playbook_item::Entity::find()
        .filter(playbook_item::Column::RunId.is_in(runs.iter().map(|r| r.id)))
        .order_by_asc(playbook_item::Column::Id)
        .all(db)
        .await?;
    let views = runs
        .into_iter()
        .map(|r| RunView {
            totals: totals(items.iter().filter(|i| i.run_id == r.id)),
            run_id: r.id,
            playbook_id: r.playbook_id,
            status: r.status,
            pause_reason: r.pause_reason,
            outcome: r.outcome,
            created_at: r.created_at,
            ended_at: r.ended_at,
        })
        .collect();
    let items = items
        .into_iter()
        .map(|i| ItemView {
            run_id: i.run_id,
            item_id: i.id,
            title: i.title,
            status: i.status,
            reason_code: i.reason_code,
        })
        .collect();
    Ok((views, items))
}

fn totals<'a>(items: impl Iterator<Item = &'a playbook_item::Model>) -> RunTotals {
    let mut t = RunTotals::default();
    for item in items {
        t.items += 1;
        match ItemStatus::parse(&item.status) {
            Some(ItemStatus::Done) => t.done += 1,
            Some(ItemStatus::Skipped) => t.skipped += 1,
            Some(ItemStatus::Failed | ItemStatus::RetriesExhausted) => t.failed += 1,
            Some(ItemStatus::Deferred) => t.deferred += 1,
            Some(ItemStatus::WaitingApproval | ItemStatus::WaitingPerson) => t.waiting += 1,
            _ => {}
        }
    }
    t
}
