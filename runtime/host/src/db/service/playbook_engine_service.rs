// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: CAS writes for the automation engine's items and runs
//! (automation-engine spec 6.4, 6.5).
//!
//! Every transition here is a conditional UPDATE on the expected status, so a
//! stale writer (a late attempt, a lost tab, an old epoch) is a zero-row
//! no-op rather than a resurrection. The `allowed` table in
//! [`crate::automation::states`] is consulted first so a refused edge is an
//! error in the caller, not a silent miss.

use chrono::{DateTime, Utc};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, TransactionTrait,
};

use crate::automation::entities::{playbook_item, playbook_run};
use crate::automation::states::{ItemStatus, RunStatus};

#[derive(Debug, thiserror::Error)]
pub enum TransitionError {
    #[error("database: {0}")]
    Db(#[from] crate::db::error::DbError),
    /// The engine's own state table does not allow this edge.
    #[error("illegal item transition {from} -> {to}")]
    IllegalItem { from: ItemStatus, to: ItemStatus },
    #[error("illegal run transition {from} -> {to}")]
    IllegalRun { from: RunStatus, to: RunStatus },
    /// The row moved under us: a concurrent writer won the CAS. Callers read
    /// the row again and decide; it is never retried blind.
    #[error("item {item_id} was not in {expected}")]
    ItemStale { item_id: i32, expected: ItemStatus },
    #[error("run {run_id} was not in {expected}")]
    RunStale { run_id: i32, expected: RunStatus },
    #[error("not found: {0}")]
    NotFound(String),
    /// A session of an earlier attempt of the item (spec 6.6.7): the Host
    /// requeued or restarted the item since, so the call is refused.
    #[error("item {item_id}: stale attempt {epoch}")]
    StaleAttempt { item_id: i32, epoch: i32 },
}

type Result<T> = std::result::Result<T, TransitionError>;

impl From<sea_orm::DbErr> for TransitionError {
    fn from(e: sea_orm::DbErr) -> Self {
        TransitionError::Db(crate::db::error::DbError::from(e))
    }
}

/// Move an item from `from` to `to`, bumping its attempt epoch when it
/// re-enters `working` (spec 6.6.7: a new attempt gets a new epoch).
pub async fn transition_item(
    conn: &DatabaseConnection,
    item_id: i32,
    from: ItemStatus,
    to: ItemStatus,
    reason_code: Option<&str>,
) -> Result<()> {
    transition(conn, item_id, None, from, to, reason_code).await
}

/// [`transition_item`] on behalf of one attempt's session (spec 6.6.7): it
/// applies only while the item is still on `epoch`; a session of an earlier
/// attempt gets `StaleAttempt` and changes nothing.
pub async fn transition_attempt(
    conn: &DatabaseConnection,
    item_id: i32,
    epoch: i32,
    from: ItemStatus,
    to: ItemStatus,
    reason_code: Option<&str>,
) -> Result<()> {
    transition(conn, item_id, Some(epoch), from, to, reason_code).await
}

async fn transition(
    conn: &DatabaseConnection,
    item_id: i32,
    attempt: Option<i32>,
    from: ItemStatus,
    to: ItemStatus,
    reason_code: Option<&str>,
) -> Result<()> {
    if !ItemStatus::allowed(from, to) {
        return Err(TransitionError::IllegalItem { from, to });
    }
    let now = Utc::now();
    let mut update = playbook_item::Entity::update_many()
        .col_expr(playbook_item::Column::Status, Expr::value(to.as_str()))
        .col_expr(playbook_item::Column::UpdatedAt, Expr::value(now))
        .col_expr(playbook_item::Column::ReasonCode, Expr::value(reason_code));
    if to == ItemStatus::Working {
        // A new attempt always gets a new epoch (6.6.7). The outward mark is
        // never cleared here: it stays set from the moment the dispatch
        // transaction committed (6.4.1, 6.6.6), including across an approved
        // click (waiting_approval -> working).
        update = update.col_expr(
            playbook_item::Column::Epoch,
            Expr::col(playbook_item::Column::Epoch).add(1),
        );
    }
    let mut update = update.filter(playbook_item::Column::Id.eq(item_id));
    update = update.filter(playbook_item::Column::Status.eq(from.as_str()));
    if let Some(epoch) = attempt {
        update = update.filter(playbook_item::Column::Epoch.eq(epoch));
    }
    // Spec 6.4.1: after outward_started an item never becomes failed,
    // queued, deferred or stopped. The one exception is a proven-not-sent
    // redo (6.6.5), which goes through `redo_not_sent`.
    if matches!(
        to,
        ItemStatus::Failed | ItemStatus::Queued | ItemStatus::Deferred | ItemStatus::Stopped
    ) {
        update = update.filter(playbook_item::Column::OutwardStartedAt.is_null());
    }
    let res = update.exec(conn).await?;
    if res.rows_affected == 0 {
        if let Some(epoch) = attempt {
            let current = playbook_item::Entity::find_by_id(item_id)
                .one(conn)
                .await?
                .map(|m| m.epoch);
            if current.is_some_and(|e| e != epoch) {
                return Err(TransitionError::StaleAttempt { item_id, epoch });
            }
        }
        return Err(TransitionError::ItemStale {
            item_id,
            expected: from,
        });
    }
    Ok(())
}

/// "Not sent, redo" (spec 6.6.5): in one transaction, move a `waiting_person`
/// item back to `queued`, clear its outward mark and release its reserved
/// keys. An item in any other state is left untouched (`ItemStale`), so a
/// late answer can never free an item that may have gone out.
pub async fn redo_not_sent(
    conn: &DatabaseConnection,
    item_id: i32,
    keys: &[crate::automation::engine::LedgerKey],
) -> Result<()> {
    let tx = conn.begin().await?;
    let slot = playbook_item::Entity::find_by_id(item_id)
        .one(&tx)
        .await?
        .and_then(|m| Some((m.cap_site?, m.account, m.cap_kind, m.cap_day?)));
    let res = playbook_item::Entity::update_many()
        .col_expr(
            playbook_item::Column::Status,
            Expr::value(ItemStatus::Queued.as_str()),
        )
        .col_expr(playbook_item::Column::CapSite, Expr::value(None::<String>))
        .col_expr(playbook_item::Column::CapKind, Expr::value(None::<String>))
        .col_expr(playbook_item::Column::CapDay, Expr::value(None::<String>))
        .col_expr(
            playbook_item::Column::OutwardStartedAt,
            Expr::value(None::<DateTime<Utc>>),
        )
        .col_expr(
            playbook_item::Column::ReasonCode,
            Expr::value(None::<String>),
        )
        .col_expr(playbook_item::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(playbook_item::Column::Id.eq(item_id))
        .filter(playbook_item::Column::Status.eq(ItemStatus::WaitingPerson.as_str()))
        .exec(&tx)
        .await?;
    if res.rows_affected == 0 {
        tx.rollback().await?;
        return Err(TransitionError::ItemStale {
            item_id,
            expected: ItemStatus::WaitingPerson,
        });
    }
    for key in keys {
        crate::automation::engine::LedgerKey::release(&tx, key, item_id).await?;
    }
    // Give back the slot on the day it was taken, site total and kind.
    if let Some((site, account, kind, day)) = slot {
        cap::release_taken(&tx, &site, &account, kind.as_deref(), &day).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Move a run from `from` to `to`, setting or clearing the pause reason
/// together with the status so a paused run never exists without a reason.
pub async fn transition_run(
    conn: &DatabaseConnection,
    run_id: i32,
    from: RunStatus,
    to: RunStatus,
    pause_reason: Option<&str>,
) -> Result<()> {
    if !RunStatus::allowed(from, to) {
        return Err(TransitionError::IllegalRun { from, to });
    }
    let now = Utc::now();
    let mut update = playbook_run::Entity::update_many()
        .col_expr(playbook_run::Column::Status, Expr::value(to.as_str()))
        .col_expr(playbook_run::Column::PauseReason, Expr::value(pause_reason));
    if to == RunStatus::Paused {
        update = update.col_expr(playbook_run::Column::PauseReason, Expr::value(pause_reason));
    }
    if to == RunStatus::Running {
        update = update.col_expr(playbook_run::Column::StartedAt, Expr::value(now));
    }
    if to.is_terminal() {
        update = update.col_expr(playbook_run::Column::EndedAt, Expr::value(now));
    }
    let res = update
        .filter(playbook_run::Column::Id.eq(run_id))
        .filter(playbook_run::Column::Status.eq(from.as_str()))
        .exec(conn)
        .await?;
    if res.rows_affected == 0 {
        return Err(TransitionError::RunStale {
            run_id,
            expected: from,
        });
    }
    Ok(())
}

/// Count the playbook's active runs. A start that reads 0 and then inserts is
/// protected by the partial unique index; this is the pre-check that turns
/// the race into `run_active` without a constraint error.
pub async fn active_run_count(conn: &DatabaseConnection, playbook_id: i32) -> Result<u64> {
    let count = playbook_run::Entity::find()
        .filter(playbook_run::Column::PlaybookId.eq(playbook_id))
        .filter(playbook_run::Column::Status.is_in(["discovering", "running", "paused"]))
        .count(conn)
        .await?;
    Ok(count)
}

/// One dispatch transaction (spec 6.6.1): reserve every never-twice key,
/// take a cap slot, mark `outward_started`. Either all of it commits, or the
/// item stays without an outward start and no cap is spent.
pub async fn begin_dispatch(
    conn: &DatabaseConnection,
    item_id: i32,
    epoch: i32,
    keys: &[crate::automation::engine::LedgerKey],
    cap: crate::automation::engine::CapSlot,
) -> Result<crate::automation::engine::DispatchOutcome> {
    dispatch_tx(conn, item_id, epoch, keys, cap, None).await
}

/// [`begin_dispatch`] for the Host's approved click (spec 8.5.5): the
/// person's approval of `approved` is used up in the same transaction, so
/// a second click, or one with changed content, is `NotApproved`.
pub async fn begin_approved_dispatch(
    conn: &DatabaseConnection,
    item_id: i32,
    epoch: i32,
    keys: &[crate::automation::engine::LedgerKey],
    cap: crate::automation::engine::CapSlot,
    approved: &crate::automation::Declaration,
) -> Result<crate::automation::engine::DispatchOutcome> {
    dispatch_tx(conn, item_id, epoch, keys, cap, Some(approved)).await
}

async fn dispatch_tx(
    conn: &DatabaseConnection,
    item_id: i32,
    epoch: i32,
    keys: &[crate::automation::engine::LedgerKey],
    cap: crate::automation::engine::CapSlot,
    approved: Option<&crate::automation::Declaration>,
) -> Result<crate::automation::engine::DispatchOutcome> {
    use crate::automation::engine::{DispatchOutcome, LedgerKey, LedgerKeyOutcome};
    let tx = conn.begin().await?;

    let item = playbook_item::Entity::find_by_id(item_id)
        .one(&tx)
        .await?
        .filter(|m| m.status == ItemStatus::Working.as_str())
        .ok_or_else(|| TransitionError::NotFound(format!("item {item_id} is not working")))?;
    // Only the current attempt's session may send (6.6.7). The outward
    // update below also filters on the epoch, so a requeue racing this
    // transaction rolls it back.
    if item.epoch != epoch {
        tx.rollback().await?;
        return Err(TransitionError::StaleAttempt { item_id, epoch });
    }
    if let Some(declaration) = approved {
        if !crate::automation::approvals::consume(&tx, item_id, declaration).await? {
            tx.rollback().await?;
            return Ok(DispatchOutcome::NotApproved);
        }
    }
    // A later declaration in the same item (a confirm step) reuses the keys
    // and the one cap slot; the outward mark is already set (6.6.3).
    let already_outward = item.outward_started_at.is_some();

    for key in keys {
        match LedgerKey::reserve_or_reuse(&tx, key, item_id).await? {
            LedgerKeyOutcome::Fresh | LedgerKeyOutcome::OwnedByThisItem => {}
            LedgerKeyOutcome::TakenByOther => {
                // Reserved or done by another item: skip as already_done and
                // nothing else in the transaction persists. An item that
                // already went out never becomes `skipped`: refuse instead.
                tx.rollback().await?;
                if already_outward {
                    return Err(TransitionError::Db(crate::db::error::DbError::Conflict(
                        format!("item {item_id} went out; a later key is taken by another item"),
                    )));
                }
                return Ok(DispatchOutcome::AlreadyDone);
            }
        }
    }

    if already_outward {
        tx.commit().await?;
        return Ok(DispatchOutcome::Dispatched);
    }

    if !cap::take(&tx, &cap).await? {
        tx.rollback().await?;
        return Ok(DispatchOutcome::DailyCapHit);
    }

    let now = Utc::now();
    let res = playbook_item::Entity::update_many()
        .col_expr(
            playbook_item::Column::OutwardStartedAt,
            Expr::value(Some(now)),
        )
        .col_expr(
            playbook_item::Column::CapSite,
            Expr::value(cap.site.clone()),
        )
        .col_expr(
            playbook_item::Column::CapKind,
            Expr::value(cap.kind_cap.map(|_| cap.action_kind.clone())),
        )
        .col_expr(
            playbook_item::Column::CapDay,
            Expr::value(cap.local_day.clone()),
        )
        .col_expr(playbook_item::Column::UpdatedAt, Expr::value(now))
        .filter(playbook_item::Column::Id.eq(item_id))
        .filter(playbook_item::Column::OutwardStartedAt.is_null())
        .filter(playbook_item::Column::Status.eq(ItemStatus::Working.as_str()))
        .filter(playbook_item::Column::Epoch.eq(epoch))
        .exec(&tx)
        .await?;
    if res.rows_affected == 0 {
        tx.rollback().await?;
        return Err(TransitionError::NotFound(format!(
            "item {item_id} is not working or already outward"
        )));
    }

    tx.commit().await?;
    Ok(DispatchOutcome::Dispatched)
}

pub mod cap {
    use super::*;
    use crate::automation::engine::CapSlot;
    use crate::automation::entities::cap_usage;
    use sea_orm::{ConnectionTrait, Set};

    /// Take the slot for `(site, account, kind, local day)` and count it
    /// against BOTH the site total and (if any) the per-kind cap (spec
    /// 10.1.1). Two counters keyed by `kind` `"*"` (the site total) and the
    /// action kind. False when either is spent, so nothing is counted.
    ///
    /// The kind row is taken first: when it succeeds but the site row is
    /// full, both are rolled back by the caller (the dispatch transaction),
    /// so a refused dispatch never spends a slot.
    pub async fn take<C: ConnectionTrait>(conn: &C, slot: &CapSlot) -> Result<bool> {
        if slot.effective_cap() <= 0 {
            return Ok(false);
        }
        // The site total first, then the kind; the caller's transaction rolls
        // both back if either refused or a later step fails.
        if !increment(conn, slot, "*", slot.site_cap).await? {
            return Ok(false);
        }
        if slot.kind_cap.is_some()
            && !increment(conn, slot, &slot.action_kind, slot.kind_cap.unwrap()).await?
        {
            // The site slot was taken; the caller's rollback undoes it.
            return Ok(false);
        }
        Ok(true)
    }

    /// Increment one counter, creating it at 1 on first use, and refuse when
    /// it is already at `cap`. The update is conditional (`used < cap`) so two
    /// concurrent dispatches cannot both slip past.
    async fn increment<C: ConnectionTrait>(
        conn: &C,
        slot: &CapSlot,
        kind: &str,
        cap: i32,
    ) -> Result<bool> {
        if cap <= 0 {
            return Ok(false);
        }
        let row = cap_usage::Entity::find_by_id((
            slot.site.clone(),
            slot.account.clone(),
            kind.to_string(),
            slot.local_day.clone(),
        ))
        .one(conn)
        .await?;
        if row.is_none() {
            // Insert at 1; a concurrent insert loses the race and falls
            // through to the conditional update below.
            let inserted = cap_usage::Entity::insert(cap_usage::ActiveModel {
                site: Set(slot.site.clone()),
                account: Set(slot.account.clone()),
                action_kind: Set(kind.to_string()),
                local_day: Set(slot.local_day.clone()),
                used: Set(1),
            })
            .on_conflict(
                sea_orm::sea_query::OnConflict::new()
                    .do_nothing()
                    .to_owned(),
            )
            .exec_without_returning(conn)
            .await?;
            if inserted == 1 {
                return Ok(true);
            }
            // Lost the race; try the update.
        }
        let res = cap_usage::Entity::update_many()
            .col_expr(
                cap_usage::Column::Used,
                Expr::col(cap_usage::Column::Used).add(1),
            )
            .filter(cap_usage::Column::Site.eq(slot.site.clone()))
            .filter(cap_usage::Column::Account.eq(slot.account.clone()))
            .filter(cap_usage::Column::ActionKind.eq(kind.to_string()))
            .filter(cap_usage::Column::LocalDay.eq(slot.local_day.clone()))
            .filter(Expr::col(cap_usage::Column::Used).lt(cap))
            .exec(conn)
            .await?;
        Ok(res.rows_affected == 1)
    }

    /// Give back the slot an item recorded at dispatch, for a proven-not-sent
    /// redo (spec 6.6.5): the site total, and the kind when that kind has its
    /// own cap; never below zero.
    pub async fn release_taken<C: ConnectionTrait>(
        conn: &C,
        site: &str,
        account: &str,
        kind: Option<&str>,
        local_day: &str,
    ) -> Result<()> {
        let slot = CapSlot {
            site: site.to_string(),
            account: account.to_string(),
            action_kind: kind.unwrap_or("*").to_string(),
            local_day: local_day.to_string(),
            site_cap: 0,
            kind_cap: None,
        };
        decrement(conn, &slot, "*").await?;
        if let Some(kind) = kind {
            decrement(conn, &slot, kind).await?;
        }
        Ok(())
    }

    async fn decrement<C: ConnectionTrait>(conn: &C, slot: &CapSlot, kind: &str) -> Result<()> {
        cap_usage::Entity::update_many()
            .col_expr(
                cap_usage::Column::Used,
                Expr::col(cap_usage::Column::Used).sub(1),
            )
            .filter(cap_usage::Column::Site.eq(slot.site.clone()))
            .filter(cap_usage::Column::Account.eq(slot.account.clone()))
            .filter(cap_usage::Column::ActionKind.eq(kind.to_string()))
            .filter(cap_usage::Column::LocalDay.eq(slot.local_day.clone()))
            .filter(Expr::col(cap_usage::Column::Used).gt(0))
            .exec(conn)
            .await?;
        Ok(())
    }
}
