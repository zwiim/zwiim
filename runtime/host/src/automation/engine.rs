// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: [`AutomationEngine`], the Host-side queue, dispatch
//! transaction, ledgers and caps (automation-engine spec 5.2).
//!
//! The engine never talks to a browser or an agent directly: it moves items
//! and runs through their CAS state machines and owns the two ledgers and the
//! cap slots. Playbooks and site knowledge belong to [`PlaybookStore`].

use sea_orm::{ConnectionTrait, DatabaseConnection};

use crate::automation::states::{ItemStatus, RunStatus};
use crate::db::error::DbError;
use crate::db::service::playbook_engine_service::{self as engine_svc, TransitionError};

/// One never-twice key: (site, account, action kind, normalized item key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerKey {
    pub site: String,
    pub account: String,
    pub action_kind: String,
    pub item_key: String,
}

impl LedgerKey {
    /// Take the key as `reserved` for `item_id`; `true` when this call
    /// inserted it. Runs inside the dispatch transaction so a lost race rolls
    /// back. Records the owning item so a later declaration in the same item
    /// can reuse it (spec 6.6.3).
    #[allow(clippy::wrong_self_convention)]
    pub async fn reserve<C: ConnectionTrait>(
        conn: &C,
        key: &LedgerKey,
        item_id: i32,
    ) -> Result<bool, DbError> {
        Ok(matches!(
            Self::reserve_or_reuse_unchecked(conn, key, item_id).await?,
            LedgerKeyOutcome::Fresh
        ))
    }

    /// Reserve the key, or accept it when the row belongs to this very item
    /// (a second declaration for a confirm step). `TakenByOther` when another
    /// item reserved or finished it, which ends the item `already_done`.
    pub async fn reserve_or_reuse<C: ConnectionTrait>(
        conn: &C,
        key: &LedgerKey,
        item_id: i32,
    ) -> Result<LedgerKeyOutcome, DbError> {
        Self::reserve_or_reuse_unchecked(conn, key, item_id).await
    }

    async fn reserve_or_reuse_unchecked<C: ConnectionTrait>(
        conn: &C,
        key: &LedgerKey,
        item_id: i32,
    ) -> Result<LedgerKeyOutcome, DbError> {
        use crate::automation::entities::never_twice_ledger;
        use sea_orm::{sea_query::OnConflict, ColumnTrait, EntityTrait, QueryFilter, Set};
        let now = chrono::Utc::now();
        let res = never_twice_ledger::Entity::insert(never_twice_ledger::ActiveModel {
            site: Set(key.site.clone()),
            account: Set(key.account.clone()),
            action_kind: Set(key.action_kind.clone()),
            item_key: Set(key.item_key.clone()),
            state: Set("reserved".to_string()),
            round_id: Set(None),
            item_id: Set(Some(item_id)),
            created_at: Set(now),
            updated_at: Set(now),
        })
        .on_conflict(OnConflict::new().do_nothing().to_owned())
        .exec_without_returning(conn)
        .await?;
        if res == 1 {
            return Ok(LedgerKeyOutcome::Fresh);
        }
        // The row existed (reserved or done). Own it only when this item
        // reserved it.
        let row = never_twice_ledger::Entity::find()
            .filter(never_twice_ledger::Column::Site.eq(key.site.clone()))
            .filter(never_twice_ledger::Column::Account.eq(key.account.clone()))
            .filter(never_twice_ledger::Column::ActionKind.eq(key.action_kind.clone()))
            .filter(never_twice_ledger::Column::ItemKey.eq(key.item_key.clone()))
            .one(conn)
            .await?;
        match row {
            Some(m) if m.state == "reserved" && m.item_id == Some(item_id) => {
                Ok(LedgerKeyOutcome::OwnedByThisItem)
            }
            _ => Ok(LedgerKeyOutcome::TakenByOther),
        }
    }

    /// Mark a reserved key `done` (spec 6.6.5, 6.7.1): the item finished.
    pub async fn mark_done<C: ConnectionTrait>(conn: &C, key: &LedgerKey) -> Result<(), DbError> {
        use crate::automation::entities::never_twice_ledger;
        use sea_orm::sea_query::Expr;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        never_twice_ledger::Entity::update_many()
            .col_expr(never_twice_ledger::Column::State, Expr::value("done"))
            .col_expr(
                never_twice_ledger::Column::UpdatedAt,
                Expr::value(chrono::Utc::now()),
            )
            .filter(never_twice_ledger::Column::Site.eq(key.site.clone()))
            .filter(never_twice_ledger::Column::Account.eq(key.account.clone()))
            .filter(never_twice_ledger::Column::ActionKind.eq(key.action_kind.clone()))
            .filter(never_twice_ledger::Column::ItemKey.eq(key.item_key.clone()))
            .filter(never_twice_ledger::Column::State.eq("reserved"))
            .exec(conn)
            .await?;
        Ok(())
    }

    /// Release a reserved key for a proven-not-sent redo (spec 6.6.5): only
    /// a still-`reserved` key the item holds is deleted.
    pub async fn release<C: ConnectionTrait>(
        conn: &C,
        key: &LedgerKey,
        item_id: i32,
    ) -> Result<(), DbError> {
        use crate::automation::entities::never_twice_ledger;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        never_twice_ledger::Entity::delete_many()
            .filter(never_twice_ledger::Column::Site.eq(key.site.clone()))
            .filter(never_twice_ledger::Column::Account.eq(key.account.clone()))
            .filter(never_twice_ledger::Column::ActionKind.eq(key.action_kind.clone()))
            .filter(never_twice_ledger::Column::ItemKey.eq(key.item_key.clone()))
            .filter(never_twice_ledger::Column::State.eq("reserved"))
            .filter(never_twice_ledger::Column::ItemId.eq(item_id))
            .exec(conn)
            .await?;
        Ok(())
    }
}

/// What reserving a key found (spec 6.6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerKeyOutcome {
    /// This call inserted the reservation.
    Fresh,
    /// The same item already reserved it (a confirm step).
    OwnedByThisItem,
    /// Another item reserved or finished it.
    TakenByOther,
}

/// The daily cap check for one outward action (spec 10.1.1). Two counters
/// must both have room: the site total (25 per (site, account)) and the kind
/// (`submit_application` uses the site total, other kinds their own cap).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapSlot {
    pub site: String,
    pub account: String,
    pub action_kind: String,
    /// `YYYY-MM-DD` in local time (spec 6.7.7).
    pub local_day: String,
    /// The site-wide cap, shared by every action kind.
    pub site_cap: i32,
    /// The per-kind cap, if any; `None` means this kind uses only the site cap.
    pub kind_cap: Option<i32>,
}

impl CapSlot {
    /// The cap for this action: the smaller of the site total and the per-kind
    /// cap (spec 10.1.1).
    pub fn effective_cap(&self) -> i32 {
        match self.kind_cap {
            Some(k) => k.min(self.site_cap),
            None => self.site_cap,
        }
    }
}

/// What the dispatch transaction concluded (spec 6.6.1 / 6.6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// Keys reserved, a cap slot taken, `outward_started` committed.
    Dispatched,
    /// A never-twice key was already taken: the item ends `skipped`.
    AlreadyDone,
    /// No cap slot left: the item ends `deferred`, nothing reserved.
    DailyCapHit,
    /// The approved click found no unused approval for this content;
    /// nothing changed and nothing is sent.
    NotApproved,
}

/// The Host-side engine handle.
#[derive(Debug, Clone)]
pub struct AutomationEngine {
    db: DatabaseConnection,
    /// Bumped after every run or item write, so the Host feed re-reads them
    /// (spec 11.4.1). `None` outside the Host.
    changes: Option<std::sync::Arc<tokio::sync::watch::Sender<u64>>>,
}

impl AutomationEngine {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db, changes: None }
    }

    /// Tell `changes` after every write (the Host passes
    /// `AppState::playbook_changes`).
    pub fn with_changes(
        mut self,
        changes: std::sync::Arc<tokio::sync::watch::Sender<u64>>,
    ) -> Self {
        self.changes = Some(changes);
        self
    }

    /// Tell the feed that runs, items or approvals may have changed.
    pub fn notify(&self) {
        if let Some(changes) = &self.changes {
            changes.send_modify(|v| *v += 1);
        }
    }

    fn noted<T, E>(&self, result: Result<T, E>) -> Result<T, E> {
        if result.is_ok() {
            self.notify();
        }
        result
    }

    pub fn db(&self) -> &DatabaseConnection {
        &self.db
    }

    /// Start a run unless the playbook already has an active one (spec 6.5.7).
    /// Returns `run_active` when it does, without inserting.
    pub async fn start_run(
        &self,
        playbook_id: i32,
        playbook_version: i32,
        trigger: crate::automation::states::RunTrigger,
        item_limit: Option<i32>,
    ) -> Result<i32, StartRunError> {
        if engine_svc::active_run_count(&self.db, playbook_id).await? > 0 {
            return Err(StartRunError::RunActive);
        }
        use crate::automation::entities::playbook_run;
        use sea_orm::{EntityTrait, Set};
        let now = chrono::Utc::now();
        let inserted = playbook_run::Entity::insert(playbook_run::ActiveModel {
            public_id: Set(uuid::Uuid::new_v4().to_string()),
            playbook_id: Set(playbook_id),
            playbook_version: Set(playbook_version),
            trigger: Set(trigger.as_str().to_string()),
            status: Set(RunStatus::Discovering.as_str().to_string()),
            pause_reason: Set(None),
            outcome: Set(None),
            item_limit: Set(item_limit),
            items_total: Set(0),
            items_done: Set(0),
            error: Set(None),
            created_at: Set(now),
            started_at: Set(None),
            ended_at: Set(None),
            ..Default::default()
        })
        .exec_with_returning(&self.db)
        .await;
        match inserted {
            Ok(m) => {
                self.notify();
                Ok(m.id)
            }
            Err(e) => {
                if is_unique_violation(&e) {
                    // The partial unique index caught a concurrent start.
                    Err(StartRunError::RunActive)
                } else {
                    Err(StartRunError::Db(e.into()))
                }
            }
        }
    }

    pub async fn transition_item(
        &self,
        item_id: i32,
        from: ItemStatus,
        to: ItemStatus,
        reason_code: Option<&str>,
    ) -> Result<(), TransitionError> {
        self.noted(engine_svc::transition_item(&self.db, item_id, from, to, reason_code).await)
    }

    /// `queued -> working` with the new attempt's epoch (spec 6.6.7).
    pub async fn start_attempt(&self, item_id: i32) -> Result<i32, TransitionError> {
        self.noted(engine_svc::start_attempt(&self.db, item_id).await)
    }

    /// [`Self::transition_item`] for one attempt's session (spec 6.6.7).
    pub async fn transition_attempt(
        &self,
        item_id: i32,
        epoch: i32,
        from: ItemStatus,
        to: ItemStatus,
        reason_code: Option<&str>,
    ) -> Result<(), TransitionError> {
        self.noted(
            engine_svc::transition_attempt(&self.db, item_id, epoch, from, to, reason_code).await,
        )
    }

    pub async fn transition_run(
        &self,
        run_id: i32,
        from: RunStatus,
        to: RunStatus,
        pause_reason: Option<&str>,
    ) -> Result<(), TransitionError> {
        self.noted(engine_svc::transition_run(&self.db, run_id, from, to, pause_reason).await)
    }

    pub async fn begin_dispatch(
        &self,
        item_id: i32,
        epoch: i32,
        keys: &[LedgerKey],
        cap: CapSlot,
    ) -> Result<DispatchOutcome, TransitionError> {
        self.noted(engine_svc::begin_dispatch(&self.db, item_id, epoch, keys, cap).await)
    }

    pub async fn begin_approved_dispatch(
        &self,
        item_id: i32,
        epoch: i32,
        keys: &[LedgerKey],
        cap: CapSlot,
        approved: &crate::automation::Declaration,
    ) -> Result<DispatchOutcome, TransitionError> {
        self.noted(
            engine_svc::begin_approved_dispatch(&self.db, item_id, epoch, keys, cap, approved)
                .await,
        )
    }
}

fn is_unique_violation(e: &sea_orm::DbErr) -> bool {
    matches!(e, sea_orm::DbErr::RecordNotInserted)
        || e.to_string().to_ascii_lowercase().contains("unique")
}

#[derive(Debug, thiserror::Error)]
pub enum StartRunError {
    #[error("run_active")]
    RunActive,
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<TransitionError> for StartRunError {
    fn from(e: TransitionError) -> Self {
        match e {
            TransitionError::Db(e) => StartRunError::Db(e),
            other => StartRunError::Db(DbError::Conflict(other.to_string())),
        }
    }
}
