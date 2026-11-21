//! Zwiim-owned: what the Host feed tells clients about runs and their items
//! (automation-engine spec 11.4.1). The feed re-reads these after each
//! playbook change and publishes the ones that differ.

use serde::Serialize;

use crate::automation::entities::{playbook, playbook_item, playbook_run};
use crate::automation::states::{ItemStatus, RunStatus};
use crate::db::error::DbError;

/// One run as `run.changed` carries it: its state and item counts. Clients
/// see public ids only; the row ids stay for the feed's own bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunView {
    #[serde(skip)]
    pub id: i32,
    pub run_id: String,
    pub playbook_id: String,
    pub status: String,
    pub pause_reason: Option<String>,
    pub outcome: Option<String>,
    /// Why a failed run failed (a reason code), else null.
    pub error: Option<String>,
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
    #[serde(skip)]
    pub id: i32,
    /// The run's row id.
    #[serde(skip)]
    pub run: i32,
    pub run_id: String,
    pub item_id: String,
    pub title: Option<String>,
    /// As `run.items` shows them: the site it was found on and its page.
    pub site: String,
    pub url: Option<String>,
    pub status: String,
    pub reason_code: Option<String>,
    /// The question it waits on the person for.
    pub question: Option<String>,
    /// The browser tab it works in or is parked on.
    pub tab_id: Option<String>,
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
    use std::collections::HashMap;
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
    let playbooks: HashMap<i32, String> = playbook::Entity::find()
        .filter(playbook::Column::Id.is_in(runs.iter().map(|r| r.playbook_id)))
        .all(db)
        .await?
        .into_iter()
        .map(|p| (p.id, p.public_id))
        .collect();
    let run_ids: HashMap<i32, String> = runs.iter().map(|r| (r.id, r.public_id.clone())).collect();
    let ids: Vec<i32> = items.iter().map(|i| i.id).collect();
    let mut questions = crate::automation::knowledge::open_questions(db, &ids).await?;
    let views = runs
        .into_iter()
        .map(|r| RunView {
            totals: totals(items.iter().filter(|i| i.run_id == r.id)),
            id: r.id,
            run_id: r.public_id,
            playbook_id: playbooks.get(&r.playbook_id).cloned().unwrap_or_default(),
            status: r.status,
            pause_reason: r.pause_reason,
            outcome: r.outcome,
            error: r.error,
            created_at: r.created_at,
            ended_at: r.ended_at,
        })
        .collect();
    let items = items
        .into_iter()
        .map(|i| ItemView {
            id: i.id,
            run: i.run_id,
            run_id: run_ids.get(&i.run_id).cloned().unwrap_or_default(),
            item_id: i.public_id,
            title: i.title,
            site: i.site,
            url: i.url,
            status: i.status,
            reason_code: i.reason_code,
            question: questions.remove(&i.id).map(|(_, text)| text),
            tab_id: i.tab_id,
        })
        .collect();
    Ok((views, items))
}

/// The totals of each of `runs` (row ids), counted from their items.
pub async fn totals_of(
    db: &sea_orm::DatabaseConnection,
    runs: &[i32],
) -> Result<std::collections::HashMap<i32, RunTotals>, DbError> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let items = playbook_item::Entity::find()
        .filter(playbook_item::Column::RunId.is_in(runs.iter().copied()))
        .all(db)
        .await?;
    Ok(runs
        .iter()
        .map(|&run| (run, totals(items.iter().filter(|i| i.run_id == run))))
        .collect())
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
