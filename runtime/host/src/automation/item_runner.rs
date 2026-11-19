// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: the item-session loop that moves items through their states
//! (automation-engine spec 6.1, 6.4, 6.6, 7.1).
//!
//! Every step is a CAS transition committed before the next action, so a
//! crash loses at most one item and never redoes one past `outward_started`.
//! The loop owns none of the policy: caps, pauses, checks, declarations and
//! limits each live in their own module and are consulted here.

use chrono::Utc;

use crate::automation::caps::Caps;
use crate::automation::checks::{self, Check, PageFacts};
use crate::automation::declaration::{Declaration, PreSubmitCheck, ScopeControl};
use crate::automation::engine::{
    AutomationEngine, CapSlot, DispatchOutcome, LedgerKey, StartRunError,
};
use crate::automation::pauses::{NavigationOutcome, SitePause};
use crate::automation::playbook_store::PlaybookStore;
use crate::automation::reasons::{ActionKind, DeclarationDecision, ReasonCode};
use crate::automation::states::{ItemStatus, PauseReason, RunStatus, RunTrigger};
use crate::db::error::DbError;
use crate::db::service::playbook_engine_service::TransitionError;

/// Limits on one item session (spec 7.1.6 [est]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionLimits {
    pub minutes: u32,
    pub tool_calls: u32,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            minutes: 15,
            tool_calls: 40,
        }
    }
}

/// What the item session reports back (spec 7.1.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemOutcome {
    /// The agent filled and declared the form; it may now click (auto).
    ReadyToDispatch(Declaration),
    /// The agent returned `awaiting_approval`: the tab is parked and the
    /// item waits for the person.
    AwaitingApproval,
    /// A playbook rule (or `action_kind_never`) skips the item.
    Skip(ReasonCode),
    /// A question the playbook cannot answer (spec 7.3).
    Unanswerable(String),
    /// The agent failed (reason code; further sub-codes arrive with 1b).
    Failed(ReasonCode),
}

/// What the Host does after a dispatch (spec 6.6.5, 6.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchResult {
    /// The item was sent and its checks passed.
    Done,
    /// The item went out but its checks did not pass.
    CheckFailed,
    /// The page changed in a way the Host cannot settle; goes to the person.
    WaitingPerson(String),
    /// The keys or the cap blocked the send; ends skipped / deferred.
    NotSent(ReasonCode),
    /// No unused approval matches: nothing was sent and nothing changed.
    NotApproved,
    /// The page no longer matches the approved content; a new session
    /// restarts the item and the approval is kept (spec 8.7).
    Restart,
}

/// The item-session runner.
#[derive(Debug, Clone)]
pub struct ItemRunner {
    engine: AutomationEngine,
    store: PlaybookStore,
    caps: Caps,
    limits: SessionLimits,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error(transparent)]
    Transition(#[from] TransitionError),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Start(#[from] StartRunError),
}

type Result<T> = std::result::Result<T, RunError>;

impl ItemRunner {
    pub fn new(engine: AutomationEngine, store: PlaybookStore) -> Self {
        Self {
            engine,
            store,
            caps: Caps::default(),
            limits: SessionLimits::default(),
        }
    }

    pub fn with_caps(mut self, caps: Caps) -> Self {
        self.caps = caps;
        self
    }

    /// A rule, approval or person skip goes on the playbook's skip list
    /// (spec 6.7.2), so discovery does not find it again for this playbook.
    async fn add_to_skip_list(&self, item: &ItemRef, reason: ReasonCode) -> Result<()> {
        self.store
            .skip_item(
                item.playbook_id,
                &item.site,
                &item.account,
                &item.key,
                reason.as_str(),
            )
            .await
            .map_err(RunError::Db)
    }

    async fn item_run_id(&self, item_id: i32) -> Result<Option<i32>> {
        use crate::automation::entities::playbook_item;
        use sea_orm::EntityTrait;
        let row = playbook_item::Entity::find_by_id(item_id)
            .one(self.engine.db())
            .await
            .map_err(|e| RunError::Db(DbError::from(e)))?;
        Ok(row.map(|m| m.run_id))
    }

    async fn run_is_running(&self, run_id: i32) -> Result<bool> {
        use crate::automation::entities::playbook_run;
        use sea_orm::EntityTrait;
        let row = playbook_run::Entity::find_by_id(run_id)
            .one(self.engine.db())
            .await
            .map_err(|e| RunError::Db(DbError::from(e)))?;
        Ok(row
            .map(|m| m.status == RunStatus::Running.as_str())
            .unwrap_or(false))
    }

    /// Read the item's outward mark from the database, never from a caller's
    /// possibly-stale `ItemRef` (spec 6.4.1, 6.6.6). A missing row is `true`
    /// so an unknown item is never treated as safe to resend.
    async fn is_outward(&self, item_id: i32) -> Result<bool> {
        use crate::automation::entities::playbook_item;
        use sea_orm::EntityTrait;
        let row = playbook_item::Entity::find_by_id(item_id)
            .one(self.engine.db())
            .await
            .map_err(|e| RunError::Db(DbError::from(e)))?;
        Ok(row.map(|m| m.outward_started_at.is_some()).unwrap_or(true))
    }

    #[cfg(test)]
    pub(crate) fn db_for_tests(&self) -> &sea_orm::DatabaseConnection {
        self.engine.db()
    }

    #[cfg(test)]
    pub(crate) fn engine_for_tests(&self) -> &AutomationEngine {
        &self.engine
    }

    #[cfg(test)]
    pub(crate) async fn transition_run_for_tests(
        &self,
        run_id: i32,
        from: crate::automation::RunStatus,
        to: crate::automation::RunStatus,
    ) -> Result<()> {
        self.engine.transition_run(run_id, from, to, None).await?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn free_lane_for_tests(&self, item_id: i32) -> Result<()> {
        // Leave `working` (whatever the reason) so a test can insert the next
        // item on the same lane.
        use sea_orm::{EntityTrait, Set};
        let id = item_id;
        let row = crate::automation::entities::playbook_item::Entity::find_by_id(id)
            .one(self.engine.db())
            .await
            .map_err(|e| RunError::Db(DbError::from(e)))?
            .ok_or_else(|| RunError::Db(DbError::NotFound(format!("item {id}"))))?;
        let mut active: crate::automation::entities::playbook_item::ActiveModel = row.into();
        active.status = Set(ItemStatus::Done.as_str().to_string());
        crate::automation::entities::playbook_item::Entity::update(active)
            .exec(self.engine.db())
            .await
            .map_err(|e| RunError::Db(DbError::from(e)))?;
        Ok(())
    }

    pub fn with_limits(mut self, limits: SessionLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Start a run unless the playbook already has one active (spec 6.5.7).
    pub async fn start(
        &self,
        playbook_id: i32,
        version: i32,
        trigger: RunTrigger,
        item_limit: Option<i32>,
    ) -> Result<i32> {
        Ok(self
            .engine
            .start_run(playbook_id, version, trigger, item_limit)
            .await?)
    }

    /// Take a queued item to `working` when the lane and the run allow it
    /// (spec 6.4). When the site is paused or the run is not running the item
    /// stays queued.
    pub async fn begin_item(&self, item_id: i32, site: &str, account: &str) -> Result<bool> {
        let run_id = match self.item_run_id(item_id).await? {
            Some(id) => id,
            None => return Ok(false),
        };
        if !self.run_is_running(run_id).await? {
            return Ok(false);
        }
        if SitePause::current(&self.store, site, account)
            .await?
            .is_some()
        {
            return Ok(false);
        }
        // The approved click takes the lane (spec 8.5.5): items approved
        // while their lane was busy start before queued work.
        crate::automation::approvals::release_approved(self.engine.db(), run_id)
            .await
            .map_err(RunError::Db)?;
        match self
            .engine
            .transition_item(item_id, ItemStatus::Queued, ItemStatus::Working, None)
            .await
        {
            Ok(()) => Ok(true),
            // Lost the lane to another item, or the item moved: not an error
            // the caller retries, just "did not start".
            Err(TransitionError::ItemStale { .. }) => Ok(false),
            Err(TransitionError::Db(e))
                if e.to_string().to_ascii_lowercase().contains("unique") =>
            {
                Ok(false)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The item session returned. Apply it: a declaration ends in a
    /// dispatch decision, anything else moves the item to its end state.
    pub async fn finish_session(
        &self,
        item_id: i32,
        item: &ItemRef,
        outcome: ItemOutcome,
    ) -> Result<SessionResult> {
        match outcome {
            ItemOutcome::AwaitingApproval => {
                self.engine
                    .transition_item(
                        item_id,
                        ItemStatus::Working,
                        ItemStatus::WaitingApproval,
                        None,
                    )
                    .await?;
                Ok(SessionResult::WaitingApproval)
            }
            ItemOutcome::Skip(reason) => {
                self.engine
                    .transition_item(
                        item_id,
                        ItemStatus::Working,
                        ItemStatus::Skipped,
                        Some(reason.as_str()),
                    )
                    .await?;
                self.add_to_skip_list(item, reason).await?;
                Ok(SessionResult::Skipped(reason))
            }
            ItemOutcome::Unanswerable(question) => {
                self.engine
                    .transition_item(
                        item_id,
                        ItemStatus::Working,
                        ItemStatus::WaitingPerson,
                        Some(ReasonCode::FormFieldUnanswerable.as_str()),
                    )
                    .await?;
                Ok(SessionResult::WaitingPerson(question))
            }
            ItemOutcome::Failed(reason) => {
                // After outward_started the item never fails (6.4.1): send it
                // to the person instead. Read the mark from the database.
                if self.is_outward(item_id).await? {
                    self.engine
                        .transition_item(
                            item_id,
                            ItemStatus::Working,
                            ItemStatus::WaitingPerson,
                            Some(reason.as_str()),
                        )
                        .await?;
                    return Ok(SessionResult::WaitingPerson(
                        "may have been sent; check and tell me".to_string(),
                    ));
                }
                self.fail_or_exhaust(item_id, item, reason).await?;
                Ok(SessionResult::Failed(reason))
            }
            ItemOutcome::ReadyToDispatch(declaration) => {
                self.declare(item_id, item, declaration).await
            }
        }
    }

    /// Decide and act on a declaration (spec 8.2.2, 8.3, 8.6).
    async fn declare(
        &self,
        item_id: i32,
        item: &ItemRef,
        declaration: Declaration,
    ) -> Result<SessionResult> {
        // From Pause or Stop on, the Host refuses new declarations in that
        // run (spec 8.2.4, 8.8.1).
        if let Some(run_id) = self.item_run_id(item_id).await? {
            if !self.run_is_running(run_id).await? {
                return Ok(SessionResult::RunNotRunning);
            }
        }
        // A continuation session re-declaring what the person approved
        // proceeds on the exact match, without asking again (spec 8.7).
        if let Some(approved) =
            crate::automation::approvals::approved_declaration(self.engine.db(), item_id)
                .await
                .map_err(RunError::Db)?
        {
            if crate::automation::approvals::binding_hash(&approved)
                == crate::automation::approvals::binding_hash(&declaration)
            {
                return Ok(SessionResult::ApprovedDispatch(approved));
            }
        }
        let rule = item.rule_for(declaration.kind);
        let show_me_run = item.show_me_run_for_kind(declaration.kind);
        let decision = crate::automation::declaration::decide(
            rule,
            declaration.kind,
            item.show_me,
            show_me_run,
        );
        match decision {
            // Never: refused; the agent ends the item skipped.
            DeclarationDecision::Refused => Ok(SessionResult::Refused(ReasonCode::ActionKindNever)),
            // Ask: park the item and its tab.
            DeclarationDecision::Pending => {
                self.engine
                    .transition_item(
                        item_id,
                        ItemStatus::Working,
                        ItemStatus::WaitingApproval,
                        None,
                    )
                    .await?;
                // Record a single-use approval bound to the declaration hash
                // (spec 8.5.1, 8.5.3). The run id comes from the item.
                if let Some(run_id) = self.item_run_id(item_id).await? {
                    crate::automation::approvals::create(
                        self.engine.db(),
                        run_id,
                        item_id,
                        &declaration,
                    )
                    .await
                    .map_err(RunError::Db)?;
                }
                Ok(SessionResult::WaitingApproval)
            }
            // Auto: the pre-submit check, then the dispatch transaction.
            DeclarationDecision::Allowed => {
                let check = PreSubmitCheck::evaluate(&declaration, &item.scope);
                if !check.passed() {
                    // A mismatch sends the item to a new session (8.6.5).
                    self.engine
                        .transition_item(
                            item_id,
                            ItemStatus::Working,
                            ItemStatus::Queued,
                            Some(ReasonCode::UndeclaredOutwardAction.as_str()),
                        )
                        .await?;
                    return Ok(SessionResult::Restart);
                }
                Ok(SessionResult::AllowDispatch(declaration))
            }
        }
    }

    /// Run the dispatch transaction and act on its outcome (spec 6.6).
    pub async fn dispatch(
        &self,
        item_id: i32,
        item: &ItemRef,
        kind: ActionKind,
        keys: &[LedgerKey],
        local_day: &str,
    ) -> Result<DispatchResult> {
        let cap = self.cap_slot(item, kind, local_day);
        let outcome = self.engine.begin_dispatch(item_id, keys, cap).await?;
        self.settle_dispatch(item_id, outcome).await
    }

    /// The Host's approved click (spec 8.5.5, 8.7): the pre-submit check of
    /// the approved content (edits included) against the page, then the
    /// dispatch transaction, which uses the approval up. A page that no
    /// longer matches sends the item to a new session and keeps the
    /// approval; a second click is `NotApproved` and changes nothing.
    pub async fn dispatch_approved(
        &self,
        item_id: i32,
        item: &ItemRef,
        keys: &[LedgerKey],
        local_day: &str,
    ) -> Result<DispatchResult> {
        let Some(approved) =
            crate::automation::approvals::approved_declaration(self.engine.db(), item_id)
                .await
                .map_err(RunError::Db)?
        else {
            return Ok(DispatchResult::NotApproved);
        };
        if !PreSubmitCheck::evaluate(&approved, &item.scope).passed() {
            if self.is_outward(item_id).await? {
                // A confirm step on a page that changed: the person decides.
                self.engine
                    .transition_item(
                        item_id,
                        ItemStatus::Working,
                        ItemStatus::WaitingPerson,
                        Some(ReasonCode::PageUnrecognised.as_str()),
                    )
                    .await?;
                return Ok(DispatchResult::WaitingPerson(
                    "the page changed after approval".to_string(),
                ));
            }
            self.engine
                .transition_item(
                    item_id,
                    ItemStatus::Working,
                    ItemStatus::Queued,
                    Some(ReasonCode::UndeclaredOutwardAction.as_str()),
                )
                .await?;
            return Ok(DispatchResult::Restart);
        }
        let cap = self.cap_slot(item, approved.kind, local_day);
        let outcome = self
            .engine
            .begin_approved_dispatch(item_id, keys, cap, &approved)
            .await?;
        self.settle_dispatch(item_id, outcome).await
    }

    fn cap_slot(&self, item: &ItemRef, kind: ActionKind, local_day: &str) -> CapSlot {
        CapSlot {
            site: item
                .outward_site
                .clone()
                .unwrap_or_else(|| item.site.clone()),
            account: item.account.clone(),
            action_kind: kind.as_str().to_string(),
            local_day: local_day.to_string(),
            site_cap: self.caps.site_daily,
            kind_cap: kind.daily_cap(),
        }
    }

    async fn settle_dispatch(
        &self,
        item_id: i32,
        outcome: DispatchOutcome,
    ) -> Result<DispatchResult> {
        match outcome {
            DispatchOutcome::NotApproved => Ok(DispatchResult::NotApproved),
            DispatchOutcome::AlreadyDone => {
                self.engine
                    .transition_item(
                        item_id,
                        ItemStatus::Working,
                        ItemStatus::Skipped,
                        Some(ReasonCode::AlreadyDone.as_str()),
                    )
                    .await?;
                Ok(DispatchResult::NotSent(ReasonCode::AlreadyDone))
            }
            DispatchOutcome::DailyCapHit => {
                self.engine
                    .transition_item(
                        item_id,
                        ItemStatus::Working,
                        ItemStatus::Deferred,
                        Some(ReasonCode::DailyCapHit.as_str()),
                    )
                    .await?;
                Ok(DispatchResult::NotSent(ReasonCode::DailyCapHit))
            }
            DispatchOutcome::Dispatched => {
                // The click has gone out. Its checks decide `done` (spec 9).
                Ok(DispatchResult::Done)
            }
        }
    }

    /// Settle an item after the outward click by its checks (spec 9, 6.8).
    /// A check failure or an unsettled page goes to the person, never a redo.
    pub async fn settle_after_checks(
        &self,
        item_id: i32,
        keys: &[LedgerKey],
        checks: &[Check],
        facts: &PageFacts,
        agent_says_done: bool,
    ) -> Result<DispatchResult> {
        let results = checks::evaluate(checks, facts, agent_says_done);
        if checks::all_pass(&results) {
            self.engine
                .transition_item(item_id, ItemStatus::Working, ItemStatus::Done, None)
                .await?;
            // The reserved keys become `done` (spec 6.6.5, 6.7.1).
            for key in keys {
                LedgerKey::mark_done(self.engine.db(), key)
                    .await
                    .map_err(RunError::Db)?;
            }
            return Ok(DispatchResult::Done);
        }
        // Outward: never back to failed/queued; go to the person.
        self.engine
            .transition_item(
                item_id,
                ItemStatus::Working,
                ItemStatus::WaitingPerson,
                Some(ReasonCode::CheckFailed.as_str()),
            )
            .await?;
        Ok(DispatchResult::CheckFailed)
    }

    /// The person answered "Not sent, redo": proven not sent, so the item
    /// returns to `queued` with its outward mark cleared and its keys
    /// released, all in one transaction (spec 6.6.5, 6.4.1).
    pub async fn redo_not_sent(&self, item_id: i32, keys: &[LedgerKey]) -> Result<()> {
        crate::db::service::playbook_engine_service::redo_not_sent(self.engine.db(), item_id, keys)
            .await?;
        Ok(())
    }

    /// The person said "It was sent": the item is done and keeps its slot.
    pub async fn accept_sent(&self, item_id: i32, keys: &[LedgerKey]) -> Result<()> {
        self.engine
            .transition_item(item_id, ItemStatus::WaitingPerson, ItemStatus::Done, None)
            .await?;
        // The person confirmed it was sent: the keys are done.
        for key in keys {
            LedgerKey::mark_done(self.engine.db(), key)
                .await
                .map_err(RunError::Db)?;
        }
        Ok(())
    }

    /// A pause in the person's hands: an item without `outward_started` goes
    /// back to `queued`, one past it finishes (spec 8.8.1).
    pub async fn pause_item(&self, item_id: i32, _item: &ItemRef) -> Result<()> {
        if self.is_outward(item_id).await? {
            return Ok(());
        }
        self.engine
            .transition_item(item_id, ItemStatus::Working, ItemStatus::Queued, None)
            .await?;
        Ok(())
    }

    /// Stop: an item without `outward_started` ends `stopped` (spec 8.8.1).
    pub async fn stop_item(&self, item_id: i32, _item: &ItemRef) -> Result<()> {
        if self.is_outward(item_id).await? {
            return Ok(());
        }
        self.engine
            .transition_item(item_id, ItemStatus::Working, ItemStatus::Stopped, None)
            .await?;
        Ok(())
    }

    /// Host restart / wake gap: an item without `outward_started` restarts
    /// from its first step with a new epoch (spec 6.6.6); one past it goes to
    /// the person ("may have been sent").
    pub async fn host_restarted(&self, item_id: i32, _item: &ItemRef) -> Result<()> {
        if self.is_outward(item_id).await? {
            self.engine
                .transition_item(
                    item_id,
                    ItemStatus::Working,
                    ItemStatus::WaitingPerson,
                    Some(ReasonCode::HostRestarted.as_str()),
                )
                .await?;
        } else {
            self.engine
                .transition_item(item_id, ItemStatus::Working, ItemStatus::Queued, None)
                .await?;
        }
        Ok(())
    }

    /// A site pause: items on the site without `outward_started` stop before
    /// their next step and go back to queued (spec 10.2.4).
    pub async fn site_paused(&self, item_id: i32, _item: &ItemRef) -> Result<()> {
        if self.is_outward(item_id).await? {
            return Ok(());
        }
        self.engine
            .transition_item(item_id, ItemStatus::Working, ItemStatus::Queued, None)
            .await?;
        Ok(())
    }

    /// Map a navigation signal to the item and run effect (spec 10.2, 10.3).
    pub fn on_navigation(
        &self,
        signal: &crate::automation::pauses::NavigationSignal,
        site: &str,
        account: &str,
        forbidden: &mut std::collections::HashMap<(String, String), u32>,
    ) -> NavigationOutcome {
        crate::automation::pauses::on_navigation(signal, site, account, forbidden)
    }

    /// The run is paused for a reason; a run that is already paused is a
    /// no-op the caller treats as done (spec 6.5).
    pub async fn pause_run(
        &self,
        run_id: i32,
        current: RunStatus,
        reason: PauseReason,
    ) -> Result<()> {
        if current == RunStatus::Paused {
            return Ok(());
        }
        self.engine
            .transition_run(run_id, current, RunStatus::Paused, Some(reason.as_str()))
            .await?;
        Ok(())
    }

    /// Count one failure against the item's retry counter; the third makes
    /// it `retries_exhausted` (spec 6.7.4).
    async fn fail_or_exhaust(
        &self,
        item_id: i32,
        item: &ItemRef,
        reason: ReasonCode,
    ) -> Result<()> {
        let failures = self
            .bump_retry(item.playbook_id, &item.site, &item.account, &item.key)
            .await?;
        if failures >= 3 {
            self.engine
                .transition_item(
                    item_id,
                    ItemStatus::Working,
                    ItemStatus::Failed,
                    Some(reason.as_str()),
                )
                .await?;
            self.engine
                .transition_item(
                    item_id,
                    ItemStatus::Failed,
                    ItemStatus::RetriesExhausted,
                    Some(reason.as_str()),
                )
                .await?;
        } else {
            self.engine
                .transition_item(
                    item_id,
                    ItemStatus::Working,
                    ItemStatus::Failed,
                    Some(reason.as_str()),
                )
                .await?;
        }
        Ok(())
    }

    async fn bump_retry(
        &self,
        playbook_id: i32,
        site: &str,
        account: &str,
        key: &str,
    ) -> Result<i32> {
        use crate::automation::entities::item_retry_counter;
        use sea_orm::{ActiveModelTrait, EntityTrait, Set};
        let now = Utc::now();
        let id = (
            playbook_id,
            site.to_string(),
            account.to_string(),
            key.to_string(),
        );
        let existing = item_retry_counter::Entity::find_by_id(id.clone())
            .one(self.engine.db())
            .await
            .map_err(|e| RunError::Db(DbError::from(e)))?;
        match existing {
            Some(row) => {
                let failures = row.failures + 1;
                let mut active: item_retry_counter::ActiveModel = row.into();
                active.failures = Set(failures);
                active.updated_at = Set(now);
                active
                    .update(self.engine.db())
                    .await
                    .map_err(|e| RunError::Db(DbError::from(e)))?;
                Ok(failures)
            }
            None => {
                item_retry_counter::Entity::insert(item_retry_counter::ActiveModel {
                    playbook_id: Set(playbook_id),
                    site: Set(site.to_string()),
                    account: Set(account.to_string()),
                    item_key: Set(key.to_string()),
                    failures: Set(1),
                    updated_at: Set(now),
                })
                .exec(self.engine.db())
                .await
                .map_err(|e| RunError::Db(DbError::from(e)))?;
                Ok(1)
            }
        }
    }
}

/// What the loop returns after one session (spec 7.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionResult {
    /// The item is `waiting_approval`; the tab is parked.
    WaitingApproval,
    /// The item ended `skipped` with this reason.
    Skipped(ReasonCode),
    /// A card is needed (question, retry bound, page changed).
    WaitingPerson(String),
    /// The item ended `failed` (counter bumped).
    Failed(ReasonCode),
    /// The declaration was refused by a `never` rule.
    Refused(ReasonCode),
    /// The run is paused or stopped, so no new declaration is accepted
    /// (spec 8.2.4, 8.8.1); the agent ends the item.
    RunNotRunning,
    /// The pre-submit check failed; a new session restarts the item.
    Restart,
    /// Auto allowed: the caller may click `submit_target` and dispatch.
    AllowDispatch(Declaration),
    /// The person already approved exactly this content (a continuation
    /// session, spec 8.7): the caller performs the approved click with
    /// [`ItemRunner::dispatch_approved`]; no new ask.
    ApprovedDispatch(Declaration),
}

/// The item facts a session works from (spec 7.1.1). The runner only reads
/// them; the caller assembles them from the playbook and the DB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRef {
    pub playbook_id: i32,
    pub site: String,
    pub account: String,
    pub key: String,
    pub outward_site: Option<String>,
    pub outward_started: bool,
    /// Whether the playbook's show-me switch is on (locked on in 1a-i).
    pub show_me: bool,
    /// Per-kind rules from the playbook; a missing kind is ask.
    pub rules: std::collections::HashMap<ActionKind, crate::automation::reasons::ApprovalRule>,
    /// Kinds with a completed show-me run on this site (spec 8.1.3).
    pub show_me_run_kinds: std::collections::HashSet<ActionKind>,
    /// What the Host read from the submit scope now (spec 8.6).
    pub scope: Vec<ScopeControl>,
}

impl ItemRef {
    pub fn rule_for(&self, kind: ActionKind) -> crate::automation::reasons::ApprovalRule {
        self.rules
            .get(&kind)
            .copied()
            .unwrap_or(crate::automation::reasons::ApprovalRule::Ask)
    }

    pub fn show_me_run_for_kind(&self, kind: ActionKind) -> bool {
        self.show_me_run_kinds.contains(&kind)
    }
}
