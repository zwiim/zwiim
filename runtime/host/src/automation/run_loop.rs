// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: the run loop, what moves runs and items along (automation
//! spec 6.1, 6.3.4, 6.5, 7.1).
//!
//! Each [`RunLoop::tick`] looks at the active runs and starts what can start:
//! discovery for a `discovering` run, then one item session per free lane and
//! free item-agent slot for a `running` one. A run with nothing left that can
//! progress without the person is ended with its outcome. Sessions run as
//! tasks, so a tick never waits on an agent; every state change is the item
//! runner's CAS write, so a tick that races another changes nothing twice.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
};
use tokio::task::JoinHandle;

use crate::automation::agent_gate::{ItemAgentGate, ItemAgentSlot, DEFAULT_ITEM_AGENTS};
use crate::automation::discovery::{item_id_rule, item_key, Discovery, LaneTabs};
use crate::automation::engine::AutomationEngine;
use crate::automation::entities::{playbook_item, playbook_run};
use crate::automation::item_runner::{
    ItemOutcome, ItemRef, ItemRunner, RunError as StepError, SessionLimits, SessionResult,
};
use crate::automation::launcher::{run_session, EngineLauncher, SessionEnd, SessionRequest};
use crate::automation::lockdown::{ItemAttempt, SessionKind};
use crate::automation::playbook_format::PlaybookDoc;
use crate::automation::playbook_store::PlaybookStore;
use crate::automation::reasons::ReasonCode;
use crate::automation::sessions::{EngineScope, EngineSessions, ItemReportStatus, SessionReport};
use crate::automation::states::{ItemStatus, RunError, RunOutcome, RunStatus};
use crate::browser::store::TabOwner;
use crate::db::error::DbError;
use crate::db::service::playbook_engine_service::TransitionError;
use crate::models::agent::AgentType;

/// The run error code when the playbook's agent cannot start (spec 6.5.6).
pub const AGENT_UNAVAILABLE: &str = "agent_unavailable";

/// Item statuses that still progress without the person.
const IN_FLIGHT: [ItemStatus; 3] = [
    ItemStatus::Queued,
    ItemStatus::Working,
    ItemStatus::WaitingApproval,
];

/// The loop and what it works with.
pub struct RunLoop {
    db: DatabaseConnection,
    engine: AutomationEngine,
    runner: ItemRunner,
    store: PlaybookStore,
    launcher: Arc<dyn EngineLauncher>,
    sessions: Arc<EngineSessions>,
    tabs: Arc<dyn LaneTabs>,
    gate: ItemAgentGate,
    limits: SessionLimits,
    /// Runs whose discovery is running now.
    discovering: Mutex<HashSet<i32>>,
}

impl RunLoop {
    pub fn new(
        db: DatabaseConnection,
        changes: Option<Arc<tokio::sync::watch::Sender<u64>>>,
        launcher: Arc<dyn EngineLauncher>,
        sessions: Arc<EngineSessions>,
        tabs: Arc<dyn LaneTabs>,
    ) -> Arc<Self> {
        let mut engine = AutomationEngine::new(db.clone());
        let store = PlaybookStore::new(db.clone());
        let mut runner = ItemRunner::new(engine.clone(), store.clone());
        if let Some(changes) = changes {
            engine = engine.with_changes(changes.clone());
            runner = runner.with_changes(changes);
        }
        Arc::new(Self {
            db,
            engine,
            runner,
            store,
            launcher,
            sessions,
            tabs,
            gate: ItemAgentGate::new(DEFAULT_ITEM_AGENTS),
            limits: SessionLimits::default(),
            discovering: Mutex::new(HashSet::new()),
        })
    }

    #[cfg(test)]
    fn with_limits(mut self: Arc<Self>, limits: SessionLimits) -> Arc<Self> {
        Arc::get_mut(&mut self).expect("not shared yet").limits = limits;
        self
    }

    /// At Host start no engine session is alive: every item left `working`
    /// takes the restart path (spec 6.6.6, 12.6). How many did.
    pub async fn recover(&self) -> Result<usize, DbError> {
        let working = playbook_item::Entity::find()
            .filter(playbook_item::Column::Status.eq(ItemStatus::Working.as_str()))
            .all(&self.db)
            .await?;
        let mut moved = 0;
        for item in working {
            let item_ref = ItemRef {
                playbook_id: item.playbook_id,
                site: item.site.clone(),
                account: item.account.clone(),
                key: item.item_key.clone(),
                outward_site: item.outward_site.clone(),
                outward_started: item.outward_started_at.is_some(),
                show_me: false,
                rules: Default::default(),
                show_me_run_kinds: Default::default(),
                scope: Vec::new(),
                epoch: item.epoch,
            };
            match self.runner.host_restarted(item.id, &item_ref).await {
                Ok(()) => moved += 1,
                Err(e) => tracing::warn!("[automation] item {} not recovered: {e}", item.id),
            }
        }
        Ok(moved)
    }

    /// One pass over the active runs; the tasks it started.
    pub async fn tick(self: &Arc<Self>) -> Result<Vec<JoinHandle<()>>, DbError> {
        let mut started = Vec::new();
        let runs = playbook_run::Entity::find()
            .filter(
                playbook_run::Column::Status
                    .is_in([RunStatus::Discovering.as_str(), RunStatus::Running.as_str()]),
            )
            .order_by_asc(playbook_run::Column::Id)
            .all(&self.db)
            .await?;
        for run in runs {
            if self.is_discovering(run.id) {
                continue;
            }
            if run.status == RunStatus::Discovering.as_str() {
                self.mark_discovering(run.id, true);
                let this = self.clone();
                started.push(tokio::spawn(async move {
                    if let Err(e) = this.discover(run.clone()).await {
                        tracing::warn!("[automation] discovery of run {} failed: {e}", run.id);
                    }
                    this.mark_discovering(run.id, false);
                }));
                continue;
            }
            started.extend(self.start_items(&run).await?);
            self.end_if_settled(&run).await?;
        }
        Ok(started)
    }

    fn is_discovering(&self, run_id: i32) -> bool {
        self.discovering
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&run_id)
    }

    fn mark_discovering(&self, run_id: i32, on: bool) {
        let mut set = self
            .discovering
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if on {
            set.insert(run_id);
        } else {
            set.remove(&run_id);
        }
    }

    /// The run's pinned playbook, parsed.
    async fn doc(&self, run: &playbook_run::Model) -> Result<PlaybookDoc, String> {
        let source = self
            .store
            .pinned_source(run.playbook_id, run.playbook_version)
            .await
            .map_err(|e| e.to_string())?;
        crate::automation::validate_playbook(&source).map_err(|errors| {
            errors
                .first()
                .map(|e| format!("line {}: {}", e.line, e.problem))
                .unwrap_or_default()
        })
    }

    /// Discover the run's sources; then it runs, or ends with nothing to do.
    async fn discover(&self, run: playbook_run::Model) -> Result<(), DbError> {
        let doc = match self.doc(&run).await {
            Ok(doc) => doc,
            Err(problem) => {
                let code = crate::automation::ErrorCode::PlaybookInvalid.as_str();
                return self
                    .fail(&run, RunStatus::Discovering, code, &problem)
                    .await;
            }
        };
        Discovery {
            db: &self.db,
            launcher: self.launcher.as_ref(),
            sessions: &self.sessions,
            tabs: self.tabs.as_ref(),
            agent: agent_of(&doc),
        }
        .discover(run.id, &doc)
        .await?;
        let queued = self.count(run.id, &[ItemStatus::Queued]).await?;
        if queued > 0 {
            match self
                .engine
                .transition_run(run.id, RunStatus::Discovering, RunStatus::Running, None)
                .await
            {
                // Paused or stopped meanwhile: the person's choice stands.
                Ok(()) | Err(TransitionError::RunStale { .. }) => Ok(()),
                Err(e) => Err(db_err(e)),
            }
        } else {
            self.finish(&run, RunStatus::Discovering).await
        }
    }

    /// Start item sessions while lanes and item-agent slots are free.
    async fn start_items(
        self: &Arc<Self>,
        run: &playbook_run::Model,
    ) -> Result<Vec<JoinHandle<()>>, DbError> {
        let mut started = Vec::new();
        let queued = playbook_item::Entity::find()
            .filter(playbook_item::Column::RunId.eq(run.id))
            .filter(playbook_item::Column::Status.eq(ItemStatus::Queued.as_str()))
            .order_by_asc(playbook_item::Column::Id)
            .all(&self.db)
            .await?;
        // A lane takes its items in queue order: once one waits, so do the
        // ones behind it, even if the lane frees meanwhile.
        let mut waiting: HashSet<(String, String)> = HashSet::new();
        for item in queued {
            if waiting.contains(&(item.site.clone(), item.account.clone())) {
                continue;
            }
            let Some(slot) = self.gate.try_acquire() else {
                break;
            };
            // The lane busy, the site paused or the run no longer running:
            // the item waits for a later tick.
            let epoch = match self
                .runner
                .begin_item(item.id, &item.site, &item.account)
                .await
            {
                Ok(Some(epoch)) => epoch,
                Ok(None) => {
                    waiting.insert((item.site.clone(), item.account.clone()));
                    continue;
                }
                Err(e) => {
                    tracing::warn!("[automation] item {} did not start: {e}", item.id);
                    continue;
                }
            };
            let this = self.clone();
            let run = run.clone();
            started.push(tokio::spawn(async move {
                if let Err(e) = this.work_item(run, item.clone(), epoch, slot).await {
                    tracing::warn!("[automation] item {} session failed: {e}", item.id);
                }
            }));
        }
        Ok(started)
    }

    /// One item session (spec 7.1), from its tab to its end state.
    async fn work_item(
        &self,
        run: playbook_run::Model,
        item: playbook_item::Model,
        epoch: i32,
        _slot: ItemAgentSlot,
    ) -> Result<(), StepError> {
        let doc = self.doc(&run).await.map_err(|e| {
            StepError::Db(DbError::Validation(format!(
                "playbook of run {}: {e}",
                run.id
            )))
        })?;
        let book = self.store.get(run.playbook_id).await?;
        let item_ref = ItemRef {
            playbook_id: run.playbook_id,
            site: item.site.clone(),
            account: item.account.clone(),
            key: item.item_key.clone(),
            outward_site: item.outward_site.clone(),
            outward_started: item.outward_started_at.is_some(),
            show_me: book.show_me,
            rules: doc.approval.iter().map(|a| (a.kind, a.rule)).collect(),
            show_me_run_kinds: Default::default(),
            scope: Vec::new(),
            epoch,
        };
        let lane = TabOwner::Lane {
            site: item.site.clone(),
            account: item.account.clone(),
        };
        let (outcome, tab) = match self.open_item_page(&lane, &item).await? {
            Err(reason) => (ItemOutcome::Failed(reason), None),
            Ok(tab) => {
                let end = run_session(
                    self.launcher.as_ref(),
                    &self.sessions,
                    SessionRequest {
                        kind: SessionKind::Item,
                        agent: agent_of(&doc),
                        attempt: Some(ItemAttempt {
                            item_id: item.id,
                            epoch,
                        }),
                        prompt: item_prompt(
                            &doc,
                            &item,
                            &tab,
                            &crate::automation::knowledge::answers(
                                &self.db,
                                run.playbook_id,
                                &item.site,
                            )
                            .await?,
                        ),
                        scope: Some(EngineScope {
                            lane,
                            tabs: vec![tab.clone()],
                        }),
                        limits: self.limits,
                    },
                )
                .await;
                if let SessionEnd::LaunchFailed(e) = &end {
                    self.tabs.close(&tab).await;
                    return self.agent_unavailable(&run, &item_ref, item.id, e).await;
                }
                (outcome_of(end), Some(tab))
            }
        };
        let question = match &outcome {
            ItemOutcome::Unanswerable(question) => Some(question.clone()),
            _ => None,
        };
        let result = self
            .runner
            .finish_session(item.id, &item_ref, outcome)
            .await;
        // Asked once, kept for the person's answer (spec 7.3).
        if let (Some(question), Ok(SessionResult::WaitingPerson(_))) = (&question, &result) {
            crate::automation::knowledge::ask(
                &self.db,
                run.playbook_id,
                &item.site,
                item.id,
                question,
            )
            .await?;
        }
        // A parked approval keeps its tab (spec 8.7); every other end closes it.
        if let Some(tab) = tab {
            if !matches!(result, Ok(SessionResult::WaitingApproval)) {
                self.tabs.close(&tab).await;
            }
        }
        match result {
            // The attempt was ended meanwhile (Pause, Stop, restart).
            Err(StepError::Transition(TransitionError::StaleAttempt { .. })) => Ok(()),
            Err(e) => Err(e),
            Ok(_) => Ok(()),
        }
    }

    /// The item's tab, on the item's page (spec 6.3.4): a page that is not
    /// the item is `page_unrecognised` before any step.
    async fn open_item_page(
        &self,
        lane: &TabOwner,
        item: &playbook_item::Model,
    ) -> Result<Result<String, ReasonCode>, DbError> {
        let Some(url) = &item.url else {
            return Ok(Err(ReasonCode::PageUnrecognised));
        };
        let tab = match self.tabs.open(lane, url).await {
            Ok(tab) => tab,
            Err(e) => {
                tracing::warn!("[automation] item {} tab did not open: {e}", item.id);
                return Ok(Err(ReasonCode::AgentFailed));
            }
        };
        let rule = item_id_rule(&self.store, &item.site).await?;
        let landed = self.tabs.url(&tab).await;
        if landed.and_then(|u| item_key(&u, &rule)).as_deref() != Some(&item.item_key) {
            self.tabs.close(&tab).await;
            return Ok(Err(ReasonCode::PageUnrecognised));
        }
        Ok(Ok(tab))
    }

    /// The agent cannot start: the item goes back to the queue unharmed and
    /// the run fails (spec 6.5.6).
    async fn agent_unavailable(
        &self,
        run: &playbook_run::Model,
        item: &ItemRef,
        item_id: i32,
        problem: &str,
    ) -> Result<(), StepError> {
        match self
            .engine
            .transition_attempt(
                item_id,
                item.epoch,
                ItemStatus::Working,
                ItemStatus::Queued,
                None,
            )
            .await
        {
            Ok(()) | Err(TransitionError::StaleAttempt { .. }) => {}
            Err(e) => return Err(e.into()),
        }
        self.fail(run, RunStatus::Running, AGENT_UNAVAILABLE, problem)
            .await
            .map_err(StepError::Db)
    }

    async fn count(&self, run_id: i32, statuses: &[ItemStatus]) -> Result<u64, DbError> {
        Ok(playbook_item::Entity::find()
            .filter(playbook_item::Column::RunId.eq(run_id))
            .filter(playbook_item::Column::Status.is_in(statuses.iter().map(|s| s.as_str())))
            .count(&self.db)
            .await?)
    }

    /// End a running run once no item can progress without the person.
    async fn end_if_settled(&self, run: &playbook_run::Model) -> Result<(), DbError> {
        if self.count(run.id, &IN_FLIGHT).await? > 0 {
            return Ok(());
        }
        self.finish(run, RunStatus::Running).await
    }

    /// The run is `done`, with its outcome and totals (spec 6.5.5).
    async fn finish(&self, run: &playbook_run::Model, from: RunStatus) -> Result<(), DbError> {
        match self
            .engine
            .transition_run(run.id, from, RunStatus::Done, None)
            .await
        {
            Ok(()) => {}
            Err(TransitionError::RunStale { .. }) => return Ok(()),
            Err(e) => return Err(db_err(e)),
        }
        let waiting = self
            .count(
                run.id,
                &[ItemStatus::WaitingPerson, ItemStatus::RetriesExhausted],
            )
            .await?;
        let outcome = if waiting > 0 {
            RunOutcome::NeedsPerson
        } else {
            RunOutcome::Complete
        };
        let total = self.count(run.id, ItemStatus::all()).await?;
        let done = self.count(run.id, &[ItemStatus::Done]).await?;
        self.write_run(run.id, |m| {
            m.outcome = sea_orm::Set(Some(outcome.as_str().to_string()));
            m.items_total = sea_orm::Set(total as i32);
            m.items_done = sea_orm::Set(done as i32);
        })
        .await
    }

    /// The run cannot go on (spec 6.5.6).
    async fn fail(
        &self,
        run: &playbook_run::Model,
        from: RunStatus,
        code: &str,
        message: &str,
    ) -> Result<(), DbError> {
        match self
            .engine
            .transition_run(run.id, from, RunStatus::Failed, None)
            .await
        {
            Ok(()) => {}
            Err(TransitionError::RunStale { .. }) => return Ok(()),
            Err(e) => return Err(db_err(e)),
        }
        let error = RunError {
            code: code.to_string(),
            message: message.to_string(),
            context: serde_json::json!({ "playbook_version": run.playbook_version }),
            trace_path: None,
        };
        let error = serde_json::to_string(&error).unwrap_or_default();
        self.write_run(run.id, |m| m.error = sea_orm::Set(Some(error)))
            .await
    }

    async fn write_run(
        &self,
        run_id: i32,
        edit: impl FnOnce(&mut playbook_run::ActiveModel),
    ) -> Result<(), DbError> {
        use sea_orm::ActiveModelTrait;
        let Some(row) = playbook_run::Entity::find_by_id(run_id)
            .one(&self.db)
            .await?
        else {
            return Ok(());
        };
        let mut active: playbook_run::ActiveModel = row.into();
        edit(&mut active);
        active.update(&self.db).await?;
        self.engine.notify();
        Ok(())
    }
}

/// Run the loop for the Host: recover, then tick whenever runs or items
/// change, and at least every few seconds.
pub fn spawn(state: Arc<crate::app_state::AppState>) {
    let run_loop = RunLoop::new(
        state.db.conn.clone(),
        Some(state.playbook_changes.clone()),
        Arc::new(crate::automation::launcher::AcpEngineLauncher {
            state: state.clone(),
        }),
        state.engine_sessions.clone(),
        Arc::new(crate::automation::discovery::BrowserLaneTabs {
            state: state.clone(),
        }),
    );
    let mut changes = state.playbook_changes.subscribe();
    tokio::spawn(async move {
        if let Err(e) = run_loop.recover().await {
            tracing::warn!("[automation] restart recovery failed: {e}");
        }
        loop {
            if let Err(e) = run_loop.tick().await {
                tracing::warn!("[automation] run loop tick failed: {e}");
            }
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(5), changes.changed()).await;
            // Let a burst of writes land before the next pass.
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });
}

fn db_err(e: TransitionError) -> DbError {
    match e {
        TransitionError::Db(e) => e,
        other => DbError::Validation(other.to_string()),
    }
}

/// The agent the playbook names, else Claude Code (spec 5.3.1.10).
fn agent_of(doc: &PlaybookDoc) -> AgentType {
    doc.agent
        .as_deref()
        .and_then(AgentType::from_wire)
        .unwrap_or(AgentType::ClaudeCode)
}

/// What an ended item session means for its item.
fn outcome_of(end: SessionEnd) -> ItemOutcome {
    match end {
        SessionEnd::Reported(SessionReport::Item(report)) => match report.status {
            ItemReportStatus::Skipped { .. } => ItemOutcome::Skip(ReasonCode::SkippedByRule),
            ItemReportStatus::Failed { .. } => ItemOutcome::Failed(ReasonCode::AgentFailed),
            // Hand-offs are taken up in a later build step.
            ItemReportStatus::HandOff { .. } => ItemOutcome::Failed(ReasonCode::AgentFailed),
            ItemReportStatus::AskPerson { question } => ItemOutcome::Unanswerable(question),
        },
        SessionEnd::TimedOut => ItemOutcome::Failed(ReasonCode::Timeout),
        SessionEnd::ToolLimit => ItemOutcome::Failed(ReasonCode::AgentLimit),
        SessionEnd::Reported(_) | SessionEnd::NoResult | SessionEnd::LaunchFailed(_) => {
            ItemOutcome::Failed(ReasonCode::AgentFailed)
        }
    }
}

/// What an item session is told (spec 7.1.1, 7.3): the playbook, this item,
/// the inputs (secrets by handle name only), and how to end.
fn item_prompt(
    doc: &PlaybookDoc,
    item: &playbook_item::Model,
    tab: &str,
    answers: &[(String, String)],
) -> String {
    use crate::automation::playbook_format::Input;
    let rules: Vec<String> = doc.rules.iter().map(|r| format!("- {r}")).collect();
    let inputs: Vec<String> = doc
        .inputs
        .iter()
        .map(|input| match input {
            Input::Fact { name, value } => format!("- {name}: {value}"),
            Input::File { name, .. } => format!("- {name}: a file the Host attaches"),
            Input::Secret { name, .. } => format!("- {name}: a secret the Host types"),
        })
        .collect();
    let mut inputs = inputs;
    inputs.extend(answers.iter().map(|(q, a)| format!("- {q} {a}")));
    let none = || "- (none)".to_string();
    format!(
        "You are working one item for the playbook \"{name}\".\n\
         Goal: {goal}\n\n\
         Rules:\n{rules}\n\n\
         What you know about the person:\n{inputs}\n\n\
         The item: {title}\n{url}\n\
         Browser tab {tab} is open on it. Read the page with browser_snapshot \
         and work in tab {tab} only.\n\n\
         First check the rules against the page. If a rule says skip, call \
         playbook_item_result with status \"skipped\" and the rule.\n\
         Never guess a fact about the person that is not above: when the page \
         asks one, call playbook_ask_person with the question. Do not submit, \
         post or send anything: this build cannot declare an outward action \
         yet. When you cannot go on, call playbook_item_result with status \
         \"failed\" and say why. Then end your turn.",
        name = doc.name,
        goal = doc.goal,
        rules = if rules.is_empty() {
            none()
        } else {
            rules.join("\n")
        },
        inputs = if inputs.is_empty() {
            none()
        } else {
            inputs.join("\n")
        },
        title = item.title.as_deref().unwrap_or(&item.item_key),
        url = item.url.as_deref().unwrap_or(""),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::discovery::tests::FakeTabs;
    use crate::automation::launcher::tests::{FakeLauncher, Script};
    use crate::automation::playbook_store::SiteKnowledgeKind;
    use crate::automation::RunTrigger;
    use crate::db::test_helpers::fresh_in_memory_db;
    use serde_json::json;

    const PLAYBOOK: &str = "format: 1\nname: Apply\ngoal: Backend roles\n\
        main_kind: submit_application\nsites:\n  - jobs.test\nchecks:\n  - kind: text_present\n    text: Applied\n\
        rules:\n  - Skip roles that need relocation\n\
        inputs:\n  - type: fact\n    name: notice_period\n    value: 30 days\n\
        \x20 - type: secret\n    name: password\n    handle: \"{{secret:site_password}}\"\n\
        sources:\n  - type: list\n    items:\n      - https://jobs.test/j/1\n      - https://jobs.test/j/2\n      - https://jobs.test/j/3\n";

    struct Fixture {
        db: DatabaseConnection,
        run_loop: Arc<RunLoop>,
        launcher: Arc<FakeLauncher>,
        tabs: Arc<FakeTabs>,
        run_id: i32,
    }

    /// The run's judgment batch keeps every item; then `scripts` are the
    /// item sessions in order.
    async fn fixture(mut scripts: Vec<Script>) -> Fixture {
        scripts.insert(
            0,
            Script::Act {
                calls: 0,
                result: Some(json!({ "status": "judged", "decisions": [] })),
            },
        );
        let db = fresh_in_memory_db().await.conn;
        let store = PlaybookStore::new(db.clone());
        let pb = store.create("Apply", PLAYBOOK).await.unwrap();
        store
            .set_site_value("jobs.test", SiteKnowledgeKind::RiskAccept, "main", "yes")
            .await
            .unwrap();
        let run_id = AutomationEngine::new(db.clone())
            .start_run(pb.id, pb.version, RunTrigger::Mac, None)
            .await
            .unwrap();
        let sessions = Arc::new(EngineSessions::default());
        let launcher = Arc::new(FakeLauncher::new(sessions.clone(), scripts));
        let tabs = Arc::new(FakeTabs::default());
        let run_loop = RunLoop::new(db.clone(), None, launcher.clone(), sessions, tabs.clone());
        Fixture {
            db,
            run_loop,
            launcher,
            tabs,
            run_id,
        }
    }

    /// Tick until nothing more starts.
    async fn drive(run_loop: &Arc<RunLoop>) {
        for _ in 0..20 {
            let started = run_loop.tick().await.unwrap();
            if started.is_empty() {
                return;
            }
            for task in started {
                task.await.unwrap();
            }
        }
        panic!("the loop never settled");
    }

    async fn run(db: &DatabaseConnection, id: i32) -> playbook_run::Model {
        playbook_run::Entity::find_by_id(id)
            .one(db)
            .await
            .unwrap()
            .unwrap()
    }

    async fn items(db: &DatabaseConnection, run_id: i32) -> Vec<(String, Option<String>)> {
        playbook_item::Entity::find()
            .filter(playbook_item::Column::RunId.eq(run_id))
            .order_by_asc(playbook_item::Column::Id)
            .all(db)
            .await
            .unwrap()
            .into_iter()
            .map(|i| (i.status, i.reason_code))
            .collect()
    }

    fn item_prompts(launcher: &FakeLauncher) -> Vec<(SessionKind, String, Option<EngineScope>)> {
        launcher
            .prompted
            .lock()
            .unwrap()
            .iter()
            .filter(|(kind, _, _)| *kind == SessionKind::Item)
            .cloned()
            .collect()
    }

    fn result(status: serde_json::Value) -> Script {
        Script::Act {
            calls: 2,
            result: Some(status),
        }
    }

    /// A run of three listed items: each item gets a session on its lane
    /// tab, one lane tab at a time, and the run ends done with its outcome
    /// (spec 6.1, 6.5.5).
    #[tokio::test]
    async fn a_run_works_every_item_and_ends_done() {
        let f = fixture(vec![
            result(json!({ "status": "skipped", "rule": "Skip roles that need relocation" })),
            result(json!({ "status": "failed", "note": "no apply button" })),
            Script::Act {
                calls: 1,
                result: None,
            },
        ])
        .await;
        drive(&f.run_loop).await;

        let run = run(&f.db, f.run_id).await;
        assert_eq!(run.status, "done");
        assert_eq!(run.outcome.as_deref(), Some("complete"));
        assert_eq!(run.items_total, 3);
        assert_eq!(
            items(&f.db, f.run_id).await,
            vec![
                ("skipped".to_string(), Some("skipped_by_rule".to_string())),
                ("failed".to_string(), Some("agent_failed".to_string())),
                ("failed".to_string(), Some("agent_failed".to_string())),
            ]
        );
        let prompted = item_prompts(&f.launcher);
        assert_eq!(prompted.len(), 3);
        let lane = TabOwner::Lane {
            site: "jobs.test".into(),
            account: "main".into(),
        };
        for (n, (kind, prompt, scope)) in prompted.iter().enumerate() {
            assert_eq!(*kind, SessionKind::Item);
            let tab = format!("tab-{}", n + 1);
            assert_eq!(
                *scope,
                Some(EngineScope {
                    lane: lane.clone(),
                    tabs: vec![tab.clone()]
                })
            );
            assert!(prompt.contains(&format!("https://jobs.test/j/{}", n + 1)));
            assert!(prompt.contains("Skip roles that need relocation"));
            assert!(prompt.contains("notice_period: 30 days"));
            // A secret is named, never shown.
            assert!(prompt.contains("password: a secret the Host types"));
            assert!(!prompt.contains("site_password"));
        }
        assert_eq!(f.tabs.closed.lock().unwrap().len(), 3);
    }

    /// Session limits fail the item before anything went out (spec 7.1.6).
    #[tokio::test]
    async fn session_limits_fail_the_item() {
        let mut f = fixture(vec![
            Script::Act {
                calls: 41,
                result: None,
            },
            Script::Hang,
            result(json!({ "status": "skipped", "rule": "r" })),
        ])
        .await;
        // A zero clock: the hanging session times out at once.
        f.run_loop = f.run_loop.with_limits(SessionLimits {
            minutes: 0,
            tool_calls: 40,
        });
        drive(&f.run_loop).await;
        let reasons: Vec<Option<String>> = items(&f.db, f.run_id)
            .await
            .into_iter()
            .map(|(_, r)| r)
            .collect();
        assert_eq!(
            reasons,
            vec![
                Some("agent_limit".to_string()),
                Some("timeout".to_string()),
                Some("skipped_by_rule".to_string()),
            ]
        );
    }

    /// A tab that lands on another item's page fails the item with
    /// `page_unrecognised`, before any session (spec 6.3.4).
    #[tokio::test]
    async fn a_page_that_is_not_the_item_is_unrecognised() {
        let f = fixture(vec![]).await;
        f.tabs.redirects.lock().unwrap().insert(
            "https://jobs.test/j/1".into(),
            "https://jobs.test/expired".into(),
        );
        drive(&f.run_loop).await;
        let first = &items(&f.db, f.run_id).await[0];
        assert_eq!(first.1.as_deref(), Some("page_unrecognised"));
        let prompted = item_prompts(&f.launcher);
        assert!(!prompted.iter().any(|(_, p, _)| p.contains("/j/1\n")));
        assert_eq!(prompted.len(), 2);
    }

    /// A question the inputs do not cover is asked once and never guessed:
    /// the item waits for the person with the question kept, and a saved
    /// answer is in every later item's prompt (spec 7.3).
    #[tokio::test]
    async fn an_unanswerable_question_waits_for_the_person() {
        let f = fixture(vec![
            result(json!({ "status": "ask_person", "question": "Are you willing to relocate?" })),
            result(json!({ "status": "skipped", "rule": "r" })),
            result(json!({ "status": "skipped", "rule": "r" })),
        ])
        .await;
        let pb = run(&f.db, f.run_id).await.playbook_id;
        crate::automation::knowledge::ask(&f.db, pb, "jobs.test", 0, "Notice period?")
            .await
            .unwrap();
        let saved = crate::automation::knowledge::all(&f.db, pb, "jobs.test", "answer")
            .await
            .unwrap();
        crate::automation::knowledge::answer(&f.run_loop.engine, saved[0].id, "30 days")
            .await
            .unwrap();
        drive(&f.run_loop).await;

        let first = &items(&f.db, f.run_id).await[0];
        assert_eq!(first.0, "waiting_person");
        assert_eq!(first.1.as_deref(), Some("form_field_unanswerable"));
        let asked = crate::automation::knowledge::all(&f.db, pb, "jobs.test", "answer")
            .await
            .unwrap();
        assert!(asked
            .iter()
            .any(|q| q.text == "Are you willing to relocate?" && q.state == "open"));
        let run = run(&f.db, f.run_id).await;
        assert_eq!(run.outcome.as_deref(), Some("needs_person"));
        for (_, prompt, _) in item_prompts(&f.launcher) {
            assert!(prompt.contains("Notice period? 30 days"), "{prompt}");
            assert!(prompt.contains("playbook_ask_person"));
        }
    }

    /// After a Host restart no session is alive: an item left `working`
    /// restarts from its first step and its lane is free again (spec 6.6.6).
    #[tokio::test]
    async fn a_restart_requeues_what_was_working() {
        let f = fixture(vec![]).await;
        // Discover, then leave item 1 working as a crash would.
        let started = f.run_loop.tick().await.unwrap();
        for task in started {
            task.await.unwrap();
        }
        let first = playbook_item::Entity::find()
            .filter(playbook_item::Column::RunId.eq(f.run_id))
            .order_by_asc(playbook_item::Column::Id)
            .one(&f.db)
            .await
            .unwrap()
            .unwrap();
        AutomationEngine::new(f.db.clone())
            .start_attempt(first.id)
            .await
            .unwrap();

        assert_eq!(f.run_loop.recover().await.unwrap(), 1);
        assert_eq!(items(&f.db, f.run_id).await[0].0, "queued");
        drive(&f.run_loop).await;
        assert_eq!(run(&f.db, f.run_id).await.status, "done");
    }

    /// An agent that cannot start fails the run and leaves the item queued
    /// and unharmed (spec 6.5.6).
    #[tokio::test]
    async fn an_agent_that_cannot_start_fails_the_run() {
        let db = fresh_in_memory_db().await.conn;
        let store = PlaybookStore::new(db.clone());
        let pb = store.create("Apply", PLAYBOOK).await.unwrap();
        store
            .set_site_value("jobs.test", SiteKnowledgeKind::RiskAccept, "main", "yes")
            .await
            .unwrap();
        let run_id = AutomationEngine::new(db.clone())
            .start_run(pb.id, pb.version, RunTrigger::Mac, None)
            .await
            .unwrap();
        let sessions = Arc::new(EngineSessions::default());
        let mut launcher = FakeLauncher::new(sessions.clone(), vec![]);
        launcher.fail_spawn = true;
        let run_loop = RunLoop::new(
            db.clone(),
            None,
            Arc::new(launcher),
            sessions,
            Arc::new(FakeTabs::default()),
        );
        drive(&run_loop).await;
        let run = run(&db, run_id).await;
        assert_eq!(run.status, "failed");
        let error: RunError = serde_json::from_str(run.error.as_deref().unwrap()).unwrap();
        assert_eq!(error.code, AGENT_UNAVAILABLE);
        assert!(items(&db, run_id)
            .await
            .iter()
            .all(|(status, _)| status == "queued"));
    }
}
