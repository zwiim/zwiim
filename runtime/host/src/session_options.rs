//! Zwiim session options: the generic model / effort / mode view of an agent's
//! native ACP config options.
//!
//! Zwiim keeps no model catalog of its own. `model` and `effort` pass the
//! agent's own values and labels through (the ACP `model` and `thought_level`
//! config-option categories). `mode` is Zwiim's generic
//! `normal` / `plan` / `unattended` switch; this adapter maps it onto the
//! agent's native permission-mode select using the `_meta.kind` each value
//! carries (`plan`, `full_access`, …), plus Codex's separate
//! `collaboration_mode` select for plan. `unattended` is offered only when the
//! agent advertises a `full_access` value, and is never selected unless a
//! client asks for it.
//!
//! Chosen native values are persisted per session in `app_metadata` and handed
//! back to the agent as preferred values whenever the session's agent is
//! respawned (idle sweep, crash, host restart). Native option ids and values
//! stay inside this module and the ACP runtime; the boundary sees only the
//! generic shape.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;

use crate::acp::manager::ConnectionManager;
use crate::acp::plan_approval::CLAUDE_EXIT_PLAN_DEFAULT;
use crate::acp::types::{
    AcpEvent, ConnectionStatus, SessionConfigKindInfo, SessionConfigOptionInfo,
    SessionConfigSelectInfo, SessionConfigSelectOptionInfo,
};
use crate::app_state::AppState;
use crate::db::entities::conversation;
use crate::db::service::app_metadata_service;
use crate::models::agent::AgentType;
use crate::protocol::error::ZwiimError;

const MODE_CATEGORY: &str = "mode";
const MODEL_CATEGORY: &str = "model";
const EFFORT_CATEGORY: &str = "thought_level";
/// Codex keeps plan on its own select, separate from the permission mode.
const COLLABORATION_ID: &str = "collaboration_mode";
const PLAN: &str = "plan";
const FULL_ACCESS: &str = "full_access";

// ─── Boundary shape ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptionValue {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

/// A select passed through from the agent: its current value and every value
/// it offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub current: String,
    pub values: Vec<OptionValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The agent's normal, approval-based operation.
    Normal,
    /// The agent plans before it edits.
    Plan,
    /// The agent runs tools without asking (its native full-access mode).
    Unattended,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModeOption {
    pub current: Mode,
    /// The agent's own name for the native mode in effect.
    pub label: String,
    pub available: Vec<Mode>,
}

/// What a client may choose for a session. An option the agent does not
/// offer is `null`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionOptions {
    pub model: Option<ChoiceOption>,
    pub effort: Option<ChoiceOption>,
    pub mode: Option<ModeOption>,
    /// Whether `session.revert` can rewind this session's chat as well as its
    /// files. Set where the session's agent is known; absent elsewhere, so a
    /// client keeps the last value it was told.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_rewind_chat: Option<bool>,
}

/// A requested change; absent fields are left alone.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionsRequest {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub mode: Option<Mode>,
}

impl OptionsRequest {
    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.effort.is_none() && self.mode.is_none()
    }
}

// ─── Native → generic ────────────────────────────────────────────────────

struct Select<'a> {
    id: &'a str,
    select: &'a SessionConfigSelectInfo,
}

impl<'a> Select<'a> {
    fn values(&self) -> impl Iterator<Item = &'a SessionConfigSelectOptionInfo> {
        self.select
            .options
            .iter()
            .chain(self.select.groups.iter().flat_map(|g| g.options.iter()))
    }

    fn find(&self, value: &str) -> Option<&'a SessionConfigSelectOptionInfo> {
        self.values().find(|v| v.value == value)
    }

    fn current(&self) -> &'a str {
        &self.select.current_value
    }

    fn current_kind(&self) -> Option<&'a str> {
        self.find(self.current()).and_then(|v| v.kind.as_deref())
    }

    fn by_kind(&self, kind: &str) -> Option<&'a SessionConfigSelectOptionInfo> {
        self.values().find(|v| v.kind.as_deref() == Some(kind))
    }

    fn choice(&self) -> ChoiceOption {
        ChoiceOption {
            current: self.current().to_string(),
            values: self
                .values()
                .map(|v| OptionValue {
                    value: v.value.clone(),
                    label: v.name.clone(),
                    description: v.description.clone(),
                })
                .collect(),
        }
    }
}

fn select_where<'a>(
    options: &'a [SessionConfigOptionInfo],
    pred: impl Fn(&SessionConfigOptionInfo) -> bool,
) -> Option<Select<'a>> {
    options.iter().find(|o| pred(o)).and_then(|o| match &o.kind {
        SessionConfigKindInfo::Select(select) => Some(Select { id: &o.id, select }),
        SessionConfigKindInfo::Boolean(_) => None,
    })
}

struct Native<'a> {
    model: Option<Select<'a>>,
    effort: Option<Select<'a>>,
    /// The permission-mode select.
    mode: Option<Select<'a>>,
    /// A separate plan switch (Codex), only when it offers `plan`.
    collaboration: Option<Select<'a>>,
}

impl<'a> Native<'a> {
    fn of(options: &'a [SessionConfigOptionInfo]) -> Self {
        Self {
            model: select_where(options, |o| {
                o.category.as_deref() == Some(MODEL_CATEGORY) || o.id == MODEL_CATEGORY
            }),
            effort: select_where(options, |o| o.category.as_deref() == Some(EFFORT_CATEGORY)),
            mode: select_where(options, |o| o.category.as_deref() == Some(MODE_CATEGORY)),
            collaboration: select_where(options, |o| {
                o.id == COLLABORATION_ID || o.category.as_deref() == Some(COLLABORATION_ID)
            })
            .filter(|s| s.find(PLAN).is_some()),
        }
    }

    fn in_collaboration_plan(&self) -> bool {
        self.collaboration.as_ref().is_some_and(|c| c.current() == PLAN)
    }

    fn plan_available(&self) -> bool {
        self.collaboration.is_some() || self.mode.as_ref().is_some_and(|m| m.by_kind(PLAN).is_some())
    }

    fn unattended_available(&self) -> bool {
        self.mode.as_ref().is_some_and(|m| m.by_kind(FULL_ACCESS).is_some())
    }

    fn current_mode(&self) -> Mode {
        if self.in_collaboration_plan() {
            return Mode::Plan;
        }
        match self.mode.as_ref().and_then(|m| m.current_kind()) {
            Some(PLAN) => Mode::Plan,
            Some(FULL_ACCESS) => Mode::Unattended,
            _ => Mode::Normal,
        }
    }

    /// The native value `normal` returns to: the remembered one while the agent
    /// still offers it as a normal mode, else the agent's first `standard`
    /// value, else its first value that is neither plan nor full access.
    fn normal_value(&self, remembered: Option<&str>) -> Option<&'a str> {
        let mode = self.mode.as_ref()?;
        let normal = |v: &SessionConfigSelectOptionInfo| is_normal_kind(v.kind.as_deref());
        remembered
            .and_then(|r| mode.find(r))
            .filter(|v| normal(v))
            .or_else(|| mode.values().find(|v| v.kind.as_deref() == Some("standard")))
            .or_else(|| mode.values().find(|v| normal(v)))
            .map(|v| v.value.as_str())
    }

    /// The native values of every select this module tracks.
    fn tracked_values(&self) -> BTreeMap<String, String> {
        [&self.model, &self.effort, &self.mode, &self.collaboration]
            .into_iter()
            .flatten()
            .map(|s| (s.id.to_string(), s.current().to_string()))
            .collect()
    }
}

fn is_normal_kind(kind: Option<&str>) -> bool {
    !matches!(kind, Some(PLAN) | Some(FULL_ACCESS))
}

/// Whether the session is in a plan mode the agent does not enforce: Codex's
/// plan switch only instructs the model, so file edits are still possible.
pub fn in_unenforced_plan(options: &[SessionConfigOptionInfo]) -> bool {
    Native::of(options).in_collaboration_plan()
}

/// The generic options for an agent's native config options.
pub fn project(options: &[SessionConfigOptionInfo]) -> SessionOptions {
    let native = Native::of(options);
    let mut available = vec![Mode::Normal];
    if native.plan_available() {
        available.push(Mode::Plan);
    }
    if native.unattended_available() {
        available.push(Mode::Unattended);
    }
    let mode = (available.len() > 1).then(|| {
        let current = native.current_mode();
        let label_from = match (&native.collaboration, current) {
            (Some(c), Mode::Plan) if native.in_collaboration_plan() => Some(c),
            _ => native.mode.as_ref(),
        };
        let label = label_from
            .and_then(|s| s.find(s.current()))
            .map(|v| v.name.clone())
            .unwrap_or_default();
        ModeOption {
            current,
            label,
            available,
        }
    });
    SessionOptions {
        model: native.model.as_ref().map(Select::choice),
        effort: native.effort.as_ref().map(Select::choice),
        mode,
        can_rewind_chat: None,
    }
}

// ─── Generic → native ────────────────────────────────────────────────────

pub fn option_unavailable(option: &str, value: &str) -> ZwiimError {
    ZwiimError::option_unavailable(option, value)
}

/// The native `(config_id, value)` sets that apply `request` to an agent whose
/// options are `options`, model first. Values the agent already holds are
/// skipped; a value or option the agent does not offer is `option_unavailable`.
pub fn resolve(
    options: &[SessionConfigOptionInfo],
    request: &OptionsRequest,
    remembered_normal: Option<&str>,
) -> Result<Vec<(String, String)>, ZwiimError> {
    let native = Native::of(options);
    let mut sets = Sets::default();
    for (name, select, value) in [
        ("model", &native.model, &request.model),
        ("effort", &native.effort, &request.effort),
    ] {
        let Some(value) = value else { continue };
        match select {
            Some(s) if s.find(value).is_some() => sets.push(s, value),
            _ => return Err(option_unavailable(name, value)),
        }
    }
    let Some(mode) = request.mode else {
        return Ok(sets.0);
    };
    let unavailable = || option_unavailable("mode", mode_name(mode));
    match mode {
        Mode::Normal => {
            sets.leave_collaboration_plan(&native);
            sets.to_normal(&native, remembered_normal).ok_or_else(unavailable)?;
        }
        Mode::Plan => match (&native.mode, &native.collaboration) {
            (Some(m), _) if m.by_kind(PLAN).is_some() => {
                sets.push(m, &m.by_kind(PLAN).expect("checked").value);
            }
            (_, Some(c)) => {
                sets.to_normal(&native, remembered_normal).ok_or_else(unavailable)?;
                sets.push(c, PLAN);
            }
            _ => return Err(unavailable()),
        },
        Mode::Unattended => {
            let m = native.mode.as_ref().ok_or_else(unavailable)?;
            let value = m.by_kind(FULL_ACCESS).ok_or_else(unavailable)?;
            sets.leave_collaboration_plan(&native);
            sets.push(m, &value.value);
        }
    }
    Ok(sets.0)
}

#[derive(Default)]
struct Sets(Vec<(String, String)>);

impl Sets {
    fn push(&mut self, select: &Select<'_>, value: &str) {
        if select.current() != value {
            self.0.push((select.id.to_string(), value.to_string()));
        }
    }

    fn leave_collaboration_plan(&mut self, native: &Native<'_>) {
        if let Some(c) = native.collaboration.as_ref().filter(|c| c.current() == PLAN) {
            if let Some(other) = c.values().find(|v| v.value != PLAN) {
                self.push(c, &other.value);
            }
        }
    }

    /// Return the permission mode to a normal value; `None` when it has to
    /// move and no normal value exists.
    fn to_normal(&mut self, native: &Native<'_>, remembered: Option<&str>) -> Option<()> {
        if let Some(m) = native.mode.as_ref() {
            if !is_normal_kind(m.current_kind()) {
                self.push(m, native.normal_value(remembered)?);
            }
        }
        Some(())
    }
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal => "normal",
        Mode::Plan => "plan",
        Mode::Unattended => "unattended",
    }
}

/// `options` with each tracked select's current value replaced by the value
/// in `values` the select still offers: what the agent will hold once those
/// values are replayed onto it.
fn overlay(
    mut options: Vec<SessionConfigOptionInfo>,
    values: &BTreeMap<String, String>,
) -> Vec<SessionConfigOptionInfo> {
    for option in &mut options {
        if let (Some(value), SessionConfigKindInfo::Select(select)) =
            (values.get(&option.id), &mut option.kind)
        {
            let offered = select
                .options
                .iter()
                .chain(select.groups.iter().flat_map(|g| g.options.iter()))
                .any(|v| &v.value == value);
            if offered {
                select.current_value = value.clone();
            }
        }
    }
    options
}

// ─── Persistence ─────────────────────────────────────────────────────────

/// A session's chosen native values.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Stored {
    values: BTreeMap<String, String>,
    /// The last normal (not plan, not full-access) permission mode, which
    /// `normal` returns to.
    normal_mode: Option<String>,
}

fn session_key(session_id: &str) -> String {
    format!("zwiim.session_options:{session_id}")
}

fn agent_key(agent: &str) -> String {
    format!("zwiim.agent_options:{agent}")
}

pub(crate) fn agent_name(agent_type: AgentType) -> Option<String> {
    serde_json::to_value(agent_type)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
}

async fn read_json<T: serde::de::DeserializeOwned>(
    db: &DatabaseConnection,
    key: &str,
) -> Option<T> {
    let raw = app_metadata_service::get_value(db, key).await.ok()??;
    serde_json::from_str(&raw).ok()
}

async fn write_json<T: Serialize>(db: &DatabaseConnection, key: &str, value: &T) {
    let Ok(raw) = serde_json::to_string(value) else {
        return;
    };
    if let Err(e) = app_metadata_service::upsert_value(db, key, &raw).await {
        tracing::warn!(key, error = %e, "[zwiim] failed to persist session options");
    }
}

async fn load(db: &DatabaseConnection, session_id: &str) -> Stored {
    read_json(db, &session_key(session_id)).await.unwrap_or_default()
}

/// The native values to replay when session `session_id`'s agent respawns.
pub async fn preferred_values(db: &DatabaseConnection, session_id: &str) -> BTreeMap<String, String> {
    load(db, session_id).await.values
}

/// The native options `agent` advertised most recently.
async fn agent_catalog(db: &DatabaseConnection, agent: &str) -> Option<Vec<SessionConfigOptionInfo>> {
    read_json(db, &agent_key(agent)).await
}

fn model_effort_key(agent: &str) -> String {
    format!("zwiim.agent_model_effort:{agent}")
}

/// Effort belongs to the model, not the agent — Claude offers none on Haiku
/// and its own levels on each other model. Per model value: the native effort
/// option the agent reported while that model was current, `None` when it
/// offered none. A model not listed has not run on this host yet.
type ModelEfforts = BTreeMap<String, Option<SessionConfigOptionInfo>>;

async fn model_efforts(db: &DatabaseConnection, agent: &str) -> ModelEfforts {
    read_json(db, &model_effort_key(agent)).await.unwrap_or_default()
}

/// The effort a new session of `agent` can choose on each model that has run
/// here (`None`: that model offers no effort).
pub async fn effort_by_model(db: &DatabaseConnection, agent: &str) -> BTreeMap<String, Option<ChoiceOption>> {
    model_efforts(db, agent)
        .await
        .into_iter()
        .map(|(model, effort)| {
            let choice = effort
                .as_ref()
                .and_then(|o| select_where(std::slice::from_ref(o), |_| true))
                .map(|s| s.choice());
            (model, choice)
        })
        .collect()
}

/// `kept` once a session reports `reported`: the effort it reports now is
/// what its current model offers.
fn learn_model_effort(mut kept: ModelEfforts, reported: &[SessionConfigOptionInfo]) -> ModelEfforts {
    let native = Native::of(reported);
    let Some(model) = native.model.as_ref().map(Select::current).filter(|m| !m.is_empty()) else {
        return kept;
    };
    let effort = native
        .effort
        .as_ref()
        .and_then(|e| reported.iter().find(|o| o.id == e.id))
        .cloned();
    kept.insert(model.to_string(), effort);
    kept
}

/// `catalog` with its effort replaced by the one a model offers, or without
/// one when the model offers none.
fn with_model_effort(
    mut catalog: Vec<SessionConfigOptionInfo>,
    effort: Option<SessionConfigOptionInfo>,
) -> Vec<SessionConfigOptionInfo> {
    catalog.retain(|o| o.category.as_deref() != Some(EFFORT_CATEGORY));
    catalog.extend(effort);
    catalog
}

/// Keep what a session of `agent` reported: the agent's catalog and what its
/// current model offers.
async fn remember_agent_report(db: &DatabaseConnection, agent: &str, reported: &[SessionConfigOptionInfo]) {
    let catalog = merge_catalog(agent_catalog(db, agent).await, reported);
    write_json(db, &agent_key(agent), &catalog).await;
    let efforts = learn_model_effort(model_efforts(db, agent).await, reported);
    write_json(db, &model_effort_key(agent), &efforts).await;
}

/// The options a new session of `agent` can choose, from what the agent last
/// advertised on this host; `None` before its first session. A new session
/// starts in `normal` mode whatever the last one used.
pub async fn for_new_session(db: &DatabaseConnection, agent: &str) -> Option<SessionOptions> {
    let mut options = project(&agent_catalog(db, agent).await?);
    if let Some(mode) = options.mode.as_mut() {
        mode.current = Mode::Normal;
        mode.label = String::new();
    }
    Some(options)
}

/// How long after trying to learn an agent's options it is tried again.
const LEARN_RETRY: Duration = Duration::from_secs(300);

/// When each agent's options were last tried, so repeated `agent.list` calls
/// do not open a session each.
static LEARN_TRIED: std::sync::LazyLock<std::sync::Mutex<BTreeMap<AgentType, std::time::Instant>>> =
    std::sync::LazyLock::new(Default::default);

/// How long a throwaway session may take to advertise its options, and to
/// switch to another model.
const LEARN_WAIT: Duration = Duration::from_secs(60);
const SWITCH_WAIT: Duration = Duration::from_secs(5);
/// Lets updates the agent sends right after another land before reading.
const LEARN_GRACE: Duration = Duration::from_millis(500);

/// Learn what a new session of `agent_type` can choose before its first
/// session here, in the background: open a throwaway session in an empty
/// directory, keep the options and slash commands it advertises, switch it
/// through each model to learn the effort each offers, and close it. Nothing
/// is prompted. The next `agent.list` reports them.
pub fn learn_in_background(state: Arc<AppState>, agent_type: AgentType) {
    let Some(agent) = agent_name(agent_type) else {
        return;
    };
    {
        let mut tried = LEARN_TRIED.lock().unwrap_or_else(|e| e.into_inner());
        if tried.get(&agent_type).is_some_and(|at| at.elapsed() < LEARN_RETRY) {
            return;
        }
        tried.insert(agent_type, std::time::Instant::now());
    }
    tokio::spawn(async move {
        let dir = state.data_dir.join("probe");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(agent, error = %e, "[zwiim] no directory to learn agent options in");
            return;
        }
        if let Err(e) = learn(&state, agent_type, &agent, dir.to_string_lossy().into_owned()).await {
            tracing::warn!(agent, error = %e, "[zwiim] could not learn agent options");
        }
    });
}

async fn learn(
    state: &AppState,
    agent_type: AgentType,
    agent: &str,
    dir: String,
) -> Result<(), crate::acp::error::AcpError> {
    use crate::acp::error::AcpError;
    crate::commands::acp::verify_agent_installed(agent_type).await?;
    let env =
        crate::commands::acp::build_session_runtime_env(&state.db, agent_type, None, &state.data_dir).await?;
    let manager = &state.connection_manager;
    let conn_id = manager
        .spawn_agent(
            agent_type,
            Some(dir),
            None,
            env,
            "zwiim-options".to_string(),
            crate::web::event_bridge::EventEmitter::Noop,
            None,
            BTreeMap::new(),
        )
        .await?;
    let learned = match manager.get_state(&conn_id).await {
        Some(session) => learn_from(state, &conn_id, &session, agent).await,
        None => Err(AcpError::ConnectionNotFound(conn_id.clone())),
    };
    let _ = manager.disconnect(&conn_id).await;
    learned
}

async fn learn_from(
    state: &AppState,
    conn_id: &str,
    session: &tokio::sync::RwLock<crate::acp::SessionState>,
    agent: &str,
) -> Result<(), crate::acp::error::AcpError> {
    let db = &state.db.conn;
    wait_until(session, LEARN_WAIT, |s| s.selectors_ready.then_some(()))
        .await
        .ok_or(crate::acp::error::AcpError::ProbeTimedOut)?;
    tokio::time::sleep(LEARN_GRACE).await;
    let (reported, commands) = {
        let s = session.read().await;
        (s.config_options.clone().unwrap_or_default(), s.available_commands.clone())
    };
    if !commands.is_empty() {
        crate::session_commands::remember(db, agent, &commands).await;
    }
    if reported.is_empty() {
        return Ok(());
    }
    remember_agent_report(db, agent, &reported).await;
    // Effort belongs to the model: switch to each other model to learn its own.
    let Some(model) = Native::of(&reported).model else {
        return Ok(());
    };
    let (model_id, current) = (model.id.to_string(), model.current().to_string());
    let others: Vec<String> = model
        .values()
        .map(|v| v.value.clone())
        .filter(|v| *v != current)
        .collect();
    for value in others {
        state
            .connection_manager
            .set_config_option(conn_id, model_id.clone(), value.clone())
            .await?;
        let switched = wait_until(session, SWITCH_WAIT, |s| {
            let options = s.config_options.as_deref()?;
            let now = Native::of(options).model?.current() == value;
            now.then_some(())
        })
        .await;
        if switched.is_none() {
            continue;
        }
        tokio::time::sleep(LEARN_GRACE).await;
        let now = session.read().await.config_options.clone().unwrap_or_default();
        let efforts = learn_model_effort(model_efforts(db, agent).await, &now);
        write_json(db, &model_effort_key(agent), &efforts).await;
    }
    Ok(())
}

/// Poll `session` until `ready` answers, or `None` after `timeout`.
async fn wait_until<T>(
    session: &tokio::sync::RwLock<crate::acp::SessionState>,
    timeout: Duration,
    ready: impl Fn(&crate::acp::SessionState) -> Option<T>,
) -> Option<T> {
    let start = std::time::Instant::now();
    loop {
        if let Some(value) = ready(&*session.read().await) {
            return Some(value);
        }
        if start.elapsed() >= timeout {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ─── Session operations ──────────────────────────────────────────────────

/// Session `row`'s options: live from its agent when one is attached, else
/// what the agent will hold once respawned. `None` when neither is known.
pub async fn read(
    db: &DatabaseConnection,
    connections: &ConnectionManager,
    row: &conversation::Model,
    session_id: &str,
) -> Option<SessionOptions> {
    let options = match live_options(connections, row).await {
        Some(live) => project(&live),
        None => {
            let stored = load(db, session_id).await;
            project(&idle_view(db, &row.agent_type, &stored, None).await?)
        }
    };
    Some(SessionOptions {
        can_rewind_chat: Some(rewinds_chat(db, row).await),
        ..options
    })
}

/// Whether session `row` can have its chat rewound: its agent can, and it is
/// not in a root workspace, where a rewind is refused (`root_workspace`).
async fn rewinds_chat(db: &DatabaseConnection, row: &conversation::Model) -> bool {
    agent_rewinds_chat(&row.agent_type)
        && <crate::db::entities::folder::Entity as sea_orm::EntityTrait>::find_by_id(row.folder_id)
            .one(db)
            .await
            .ok()
            .flatten()
            .is_some_and(|folder| folder.parent_id.is_some())
}

/// Whether an agent of wire type `agent_type` can have a chat rewound.
fn agent_rewinds_chat(agent_type: &str) -> bool {
    crate::models::agent::AgentType::from_wire(agent_type).is_some_and(crate::session::can_rewind_chat)
}

/// `idle_options` for session `stored` of `agent`, with the effort of the
/// model it will run (`model`, else its current one) once that model has run
/// here: a session moved to a model without effort shows none, and never
/// keeps another model's effort.
async fn idle_view(
    db: &DatabaseConnection,
    agent: &str,
    stored: &Stored,
    model: Option<&str>,
) -> Option<Vec<SessionConfigOptionInfo>> {
    let options = idle_options(agent_catalog(db, agent).await?, stored);
    let current = Native::of(&options).model.as_ref().map(|m| m.current().to_string());
    let Some(model) = model.map(str::to_string).or(current) else {
        return Some(options);
    };
    Some(match model_efforts(db, agent).await.remove(&model) {
        Some(effort) => overlay(with_model_effort(options, effort), &stored.values),
        None => options,
    })
}

/// What session `stored`'s agent will hold once (re)spawned: the agent's last
/// advertised options with its permission mode back at normal — the catalog's
/// current values belong to whichever session advertised last, and a new agent
/// never starts in another session's plan or full-access mode — then the
/// session's own values replayed on top.
fn idle_options(mut catalog: Vec<SessionConfigOptionInfo>, stored: &Stored) -> Vec<SessionConfigOptionInfo> {
    let (mode_id, normal, collaboration) = {
        let native = Native::of(&catalog);
        (
            native.mode.as_ref().map(|m| m.id.to_string()),
            native.normal_value(stored.normal_mode.as_deref()).map(str::to_string),
            native
                .collaboration
                .as_ref()
                .filter(|c| c.current() == PLAN)
                .and_then(|c| c.values().find(|v| v.value != PLAN).map(|v| (c.id.to_string(), v.value.clone()))),
        )
    };
    for option in &mut catalog {
        let SessionConfigKindInfo::Select(select) = &mut option.kind else {
            continue;
        };
        if Some(&option.id) == mode_id.as_ref() {
            if let Some(normal) = &normal {
                select.current_value = normal.clone();
            }
        }
        if let Some((id, value)) = &collaboration {
            if &option.id == id {
                select.current_value = value.clone();
            }
        }
    }
    overlay(catalog, &stored.values)
}

async fn live_options(
    connections: &ConnectionManager,
    row: &conversation::Model,
) -> Option<Vec<SessionConfigOptionInfo>> {
    let connection_id = crate::session::connection_for(connections, row).await?;
    let state = connections.get_state(&connection_id).await?;
    let s = state.read().await;
    s.config_options.clone()
}

/// Apply `request` to session `row` between turns. With an agent attached the
/// sets go to it now (and persist through the tracker once it confirms);
/// otherwise they are validated against what the agent last advertised and
/// persisted, to be applied when it respawns. Callers hold the session lock.
pub async fn change(
    db: &DatabaseConnection,
    connections: &ConnectionManager,
    row: &conversation::Model,
    session_id: &str,
    request: &OptionsRequest,
) -> Result<(), ZwiimError> {
    let stored = load(db, session_id).await;
    if let Some(connection_id) = crate::session::connection_for(connections, row).await {
        let state = connections
            .get_state(&connection_id)
            .await
            .ok_or_else(ZwiimError::session_not_live)?;
        // A starting agent has not advertised its options yet (and may be
        // about to run a queued first turn): wait for it before resolving.
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let options = loop {
            {
                let s = state.read().await;
                match s.status {
                    ConnectionStatus::Prompting => return Err(ZwiimError::turn_in_progress()),
                    ConnectionStatus::Connecting if std::time::Instant::now() < deadline => {}
                    _ => break s.config_options.clone().unwrap_or_default(),
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        let sets = resolve(&options, request, stored.normal_mode.as_deref())?;
        for (config_id, value) in &sets {
            connections
                .set_config_option(&connection_id, config_id.clone(), value.clone())
                .await?;
        }
        settle(&state, &sets).await;
        return Ok(());
    }
    // Effort is resolved against the requested model's own.
    let options = idle_view(db, &row.agent_type, &stored, request.model.as_deref())
        .await
        .ok_or_else(|| option_unavailable_any(request))?;
    let sets = resolve(&options, request, stored.normal_mode.as_deref())?;
    let mut next = stored;
    for (config_id, value) in sets {
        next.values.insert(config_id, value);
    }
    remember_normal(&mut next, &options);
    // A session keeps only an effort its model offers.
    let view = idle_view(db, &row.agent_type, &next, None).await.unwrap_or_default();
    let offered = Native::of(&view).effort.map(|e| (e.id.to_string(), e));
    let catalog = agent_catalog(db, &row.agent_type).await.unwrap_or_default();
    for effort_id in catalog.iter().filter(|o| o.category.as_deref() == Some(EFFORT_CATEGORY)).map(|o| &o.id) {
        let kept = next.values.get(effort_id).is_some_and(|value| {
            offered.as_ref().is_some_and(|(id, e)| id == effort_id && e.find(value).is_some())
        });
        if !kept {
            next.values.remove(effort_id);
        }
    }
    write_json(db, &session_key(session_id), &next).await;
    Ok(())
}

/// Wait (bounded) until the agent holds every value in `sets` or stops
/// answering, so the caller reads back what the agent actually settled on —
/// including a value it refused.
async fn settle(
    state: &tokio::sync::RwLock<crate::acp::SessionState>,
    sets: &[(String, String)],
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut errors_before = None;
    while std::time::Instant::now() < deadline {
        {
            let s = state.read().await;
            let errors = s.last_error.as_ref().map(|e| e.message.clone());
            let settled = sets.iter().all(|(id, value)| {
                s.config_options
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .find(|o| &o.id == id)
                    .is_some_and(|o| match &o.kind {
                        SessionConfigKindInfo::Select(select) => &select.current_value == value,
                        SessionConfigKindInfo::Boolean(_) => true,
                    })
            });
            if settled {
                return;
            }
            match &errors_before {
                None => errors_before = Some(errors),
                Some(before) if *before != errors => return,
                Some(_) => {}
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn option_unavailable_any(request: &OptionsRequest) -> ZwiimError {
    if let Some(model) = &request.model {
        option_unavailable("model", model)
    } else if let Some(effort) = &request.effort {
        option_unavailable("effort", effort)
    } else {
        option_unavailable("mode", request.mode.map(mode_name).unwrap_or_default())
    }
}

/// Keep `stored.normal_mode` on the permission mode in `stored.values` while
/// that mode is a normal one.
fn remember_normal(stored: &mut Stored, options: &[SessionConfigOptionInfo]) {
    let options = overlay(options.to_vec(), &stored.values);
    let native = Native::of(&options);
    if let Some(m) = native.mode.as_ref() {
        if is_normal_kind(m.current_kind()) {
            stored.normal_mode = Some(m.current().to_string());
        }
    }
}

/// The native values that give a brand-new session of `agent` the options in
/// `request`, resolved against what the agent last advertised here, for
/// `spawn_agent` to apply before the session is first used (and re-assert if
/// the agent drifts off them). `None` when the agent has never advertised its
/// options on this host: the caller applies them after spawn instead.
pub async fn preferred_for_start(
    db: &DatabaseConnection,
    agent: AgentType,
    request: &OptionsRequest,
) -> Result<Option<BTreeMap<String, String>>, ZwiimError> {
    if request.is_empty() {
        return Ok(Some(BTreeMap::new()));
    }
    let Some(name) = agent_name(agent) else {
        return Ok(None);
    };
    let Some(mut catalog) = agent_catalog(db, &name).await else {
        return Ok(None);
    };
    // Effort is checked against the requested model's own, once it has run here.
    if let Some(model) = &request.model {
        if let Some(effort) = model_efforts(db, &name).await.remove(model) {
            catalog = with_model_effort(catalog, effort);
        }
    }
    // A new session starts from the agent's defaults, not from what the last
    // one held: clear every current value so each requested one is applied.
    for option in &mut catalog {
        if let SessionConfigKindInfo::Select(select) = &mut option.kind {
            select.current_value.clear();
        }
    }
    Ok(Some(resolve(&catalog, request, None)?.into_iter().collect()))
}

/// Record the values a new session was started with, so it reads right
/// before its agent first reports. The tracker's first report replaces them;
/// an entry it already wrote is left alone.
pub async fn remember_start(db: &DatabaseConnection, session_id: &str, values: BTreeMap<String, String>) {
    if read_json::<Stored>(db, &session_key(session_id)).await.is_some() {
        return;
    }
    let stored = Stored {
        values,
        normal_mode: None,
    };
    write_json(db, &session_key(session_id), &stored).await;
}

/// Wait (bounded) for a freshly spawned agent to advertise its options, then
/// resolve `request` against them and apply them ahead of the first prompt.
pub async fn apply_on_start(
    connections: &ConnectionManager,
    connection_id: &str,
    request: &OptionsRequest,
) -> Result<(), ZwiimError> {
    let state = connections
        .get_state(connection_id)
        .await
        .ok_or_else(ZwiimError::session_not_live)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let options = loop {
        {
            let s = state.read().await;
            if s.selectors_ready {
                break s.config_options.clone().unwrap_or_default();
            }
            if matches!(
                s.status,
                ConnectionStatus::Disconnected | ConnectionStatus::Error
            ) {
                return Err(ZwiimError::session_not_live());
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(option_unavailable_any(request));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let sets = resolve(&options, request, None)?;
    for (config_id, value) in &sets {
        connections
            .set_config_option(connection_id, config_id.clone(), value.clone())
            .await?;
    }
    // Settle before the first prompt, so it runs with the options in effect.
    settle(&state, &sets).await;
    Ok(())
}

/// The native mode an agent enters when the user answers its plan-approval
/// (`switch_mode`) request with `option_id`, for agents that switch without
/// reporting it. Claude (claude-agent-acp ≤ 0.81.0) applies the mode chosen on
/// ExitPlanMode inside the SDK but leaves its advertised mode on `plan`.
fn mode_after_plan_exit(agent: AgentType, option_id: &str) -> Option<&'static str> {
    match (agent, option_id) {
        (AgentType::ClaudeCode, CLAUDE_EXIT_PLAN_DEFAULT) => Some("default"),
        (AgentType::ClaudeCode, "exit-plan-accept-edits") => Some("acceptEdits"),
        (AgentType::ClaudeCode, "exit-plan-auto") => Some("auto"),
        (AgentType::ClaudeCode, "exit-plan-bypass") => Some("bypassPermissions"),
        _ => None,
    }
}

/// After the user answered a plan-approval request with `option_id`, bring
/// the agent's advertised mode in line with the mode it actually switched to,
/// so clients (and a later Plan request) see the truth. Advisory: a failure
/// leaves the agent running, only its reported mode stale.
pub async fn sync_plan_exit(
    connections: &ConnectionManager,
    connection_id: &str,
    agent: AgentType,
    option_id: &str,
) {
    let Some(target) = mode_after_plan_exit(agent, option_id) else {
        return;
    };
    set_mode(connections, connection_id, |_| Some(target.to_string())).await;
}

/// After the user approved a Claude plan from the Zwiim plan card, return the
/// agent to session `session_id`'s remembered normal mode (the card always
/// exits plan through the adapter's default mode). Advisory, like
/// `sync_plan_exit`.
pub async fn sync_plan_approve(
    db: &DatabaseConnection,
    connections: &ConnectionManager,
    connection_id: &str,
    session_id: &str,
    agent: AgentType,
) {
    let Some(fallback) = mode_after_plan_exit(agent, CLAUDE_EXIT_PLAN_DEFAULT) else {
        return;
    };
    let remembered = load(db, session_id).await.normal_mode;
    set_mode(connections, connection_id, |native| {
        Some(
            native
                .normal_value(remembered.as_deref())
                .unwrap_or(fallback)
                .to_string(),
        )
    })
    .await;
}

/// Set the agent's permission mode to `target(native)` when it offers that
/// value and is not already on it.
async fn set_mode(
    connections: &ConnectionManager,
    connection_id: &str,
    target: impl FnOnce(&Native<'_>) -> Option<String>,
) {
    let Some(state) = connections.get_state(connection_id).await else {
        return;
    };
    let (config_id, target) = {
        let s = state.read().await;
        let options = s.config_options.clone().unwrap_or_default();
        let native = Native::of(&options);
        let Some(target) = target(&native) else {
            return;
        };
        match native.mode {
            Some(mode) if mode.current() != target && mode.find(&target).is_some() => {
                (mode.id.to_string(), target)
            }
            _ => return,
        }
    };
    if let Err(e) = connections
        .set_config_option(connection_id, config_id, target)
        .await
    {
        tracing::warn!("[session_options] failed to sync mode after plan approval: {e}");
    }
}

// ─── Tracker ─────────────────────────────────────────────────────────────

/// Follow every agent's option changes — the client's, and the agent's own
/// (e.g. leaving plan mode on plan approval) — into the per-agent catalog and
/// the per-session preferences.
pub fn spawn_tracker(state: Arc<AppState>) {
    let mut rx = state.acp_event_bus.subscribe();
    tokio::spawn(async move {
        loop {
            let envelope = match rx.recv().await {
                Ok(envelope) => envelope,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            };
            if matches!(
                envelope.payload,
                AcpEvent::SessionConfigOptions { .. } | AcpEvent::ConversationLinked { .. }
            ) {
                track(&state, &envelope.connection_id).await;
            }
        }
    });
}

async fn track(state: &AppState, connection_id: &str) {
    let Some(session_state) = state.connection_manager.get_state(connection_id).await else {
        return;
    };
    let (agent_type, conversation_id, options) = {
        let s = session_state.read().await;
        (s.agent_type, s.conversation_id, s.config_options.clone())
    };
    let Some(options) = options.filter(|o| !o.is_empty()) else {
        return;
    };
    let db = &state.db.conn;
    if let Some(agent) = agent_name(agent_type) {
        remember_agent_report(db, &agent, &options).await;
    }
    let Some(conversation_id) = conversation_id else {
        return;
    };
    let Ok(Some(row)) = <conversation::Entity as sea_orm::EntityTrait>::find_by_id(conversation_id)
        .one(db)
        .await
    else {
        return;
    };
    let Some(session_id) = row.session_id else {
        return;
    };
    let mut stored = load(db, &session_id).await;
    stored.values = Native::of(&options).tracked_values();
    remember_normal(&mut stored, &options);
    write_json(db, &session_key(&session_id), &stored).await;
}

/// The agent's catalog once a session reports `reported`: each reported option
/// replaces the one kept, and a kept option the report lacks stays. Some
/// options belong to the session's model, not the agent — Claude offers no
/// effort on Haiku — so one session on such a model must not take them from
/// every new session.
fn merge_catalog(
    kept: Option<Vec<SessionConfigOptionInfo>>,
    reported: &[SessionConfigOptionInfo],
) -> Vec<SessionConfigOptionInfo> {
    let mut catalog = reported.to_vec();
    catalog.extend(
        kept.into_iter()
            .flatten()
            .filter(|k| !reported.iter().any(|r| r.id == k.id)),
    );
    catalog
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::acp::types::SessionConfigBooleanInfo;

    #[test]
    fn options_say_whether_the_chat_can_be_rewound() {
        assert!(agent_rewinds_chat("codex"));
        assert!(agent_rewinds_chat("claude_code"));
        assert!(agent_rewinds_chat("deepseek"));
        assert!(!agent_rewinds_chat("gemini"));
        assert!(!agent_rewinds_chat("custom:mine"));
        assert!(!agent_rewinds_chat("nonsense"));
        let unknown = serde_json::to_value(SessionOptions::default()).unwrap();
        assert!(unknown.get("can_rewind_chat").is_none(), "{unknown}");
        let known = SessionOptions {
            can_rewind_chat: Some(false),
            ..Default::default()
        };
        assert_eq!(serde_json::to_value(known).unwrap()["can_rewind_chat"], false);
    }

    #[tokio::test]
    async fn a_root_workspace_session_cannot_rewind_its_chat() {
        use crate::db::test_helpers::{
            fresh_in_memory_db, seed_conversation, seed_folder, seed_worktree_folder,
        };
        let db = fresh_in_memory_db().await;
        let row = |id: i32| {
            let conn = db.conn.clone();
            async move {
                <conversation::Entity as sea_orm::EntityTrait>::find_by_id(id)
                    .one(&conn)
                    .await
                    .unwrap()
                    .unwrap()
            }
        };
        let root = seed_folder(&db, "/tmp/rewind-root").await;
        let worktree = seed_worktree_folder(&db, "/tmp/rewind-worktree").await;
        let in_root = row(seed_conversation(&db, root, AgentType::Codex).await).await;
        let in_worktree = row(seed_conversation(&db, worktree, AgentType::Codex).await).await;
        let gemini = row(seed_conversation(&db, worktree, AgentType::Gemini).await).await;

        assert!(!rewinds_chat(&db.conn, &in_root).await);
        assert!(rewinds_chat(&db.conn, &in_worktree).await);
        assert!(!rewinds_chat(&db.conn, &gemini).await);
    }

    fn value(value: &str, name: &str, kind: Option<&str>) -> SessionConfigSelectOptionInfo {
        SessionConfigSelectOptionInfo {
            value: value.into(),
            name: name.into(),
            description: None,
            kind: kind.map(str::to_string),
        }
    }

    fn select(
        id: &str,
        category: &str,
        current: &str,
        values: Vec<SessionConfigSelectOptionInfo>,
    ) -> SessionConfigOptionInfo {
        SessionConfigOptionInfo {
            id: id.into(),
            name: id.into(),
            description: None,
            category: Some(category.into()),
            kind: SessionConfigKindInfo::Select(SessionConfigSelectInfo {
                current_value: current.into(),
                options: values,
                groups: vec![],
            }),
            recommended_value: None,
        }
    }

    /// Claude (`claude-agent-acp` 0.79) as captured live.
    pub(crate) fn claude(mode: &str) -> Vec<SessionConfigOptionInfo> {
        vec![
            select(
                "mode",
                "mode",
                mode,
                vec![
                    value("default", "Manual", Some("standard")),
                    value("acceptEdits", "Accept edits", Some("standard")),
                    value("plan", "Plan", Some("plan")),
                    value("auto", "Auto", Some("auto_review")),
                    value("bypassPermissions", "Bypass permissions", Some("full_access")),
                ],
            ),
            select(
                "model",
                "model",
                "opus[1m]",
                vec![value("opus[1m]", "Opus 5", None), value("haiku", "Haiku 4.5", None)],
            ),
            select(
                "effort",
                "thought_level",
                "high",
                vec![value("low", "Low", None), value("high", "High", None)],
            ),
            SessionConfigOptionInfo {
                id: "fast".into(),
                name: "Fast".into(),
                description: None,
                category: Some("model_config".into()),
                kind: SessionConfigKindInfo::Boolean(SessionConfigBooleanInfo {
                    current_value: false,
                }),
                recommended_value: None,
            },
        ]
    }

    /// Codex (`codex-acp` 1.12) as captured live.
    pub(crate) fn codex(mode: &str, collaboration: &str) -> Vec<SessionConfigOptionInfo> {
        vec![
            select(
                "mode",
                "mode",
                mode,
                vec![
                    value("read-only", "Ask for approval", Some("standard")),
                    value("agent", "Approve for me", Some("auto_review")),
                    value("agent-full-access", "Full access", Some("full_access")),
                ],
            ),
            select(
                "collaboration_mode",
                "collaboration_mode",
                collaboration,
                vec![value("default", "Default", None), value("plan", "Plan", None)],
            ),
            select("model", "model", "gpt-5.5", vec![value("gpt-5.5", "GPT-5.5", None)]),
        ]
    }

    fn req(mode: Mode) -> OptionsRequest {
        OptionsRequest {
            mode: Some(mode),
            ..Default::default()
        }
    }

    fn sets(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn projects_claude_options() {
        let p = project(&claude("default"));
        assert_eq!(p.model.as_ref().unwrap().current, "opus[1m]");
        assert_eq!(p.model.as_ref().unwrap().values[1].label, "Haiku 4.5");
        assert_eq!(p.effort.as_ref().unwrap().current, "high");
        let mode = p.mode.unwrap();
        assert_eq!(mode.current, Mode::Normal);
        assert_eq!(mode.label, "Manual");
        assert_eq!(mode.available, vec![Mode::Normal, Mode::Plan, Mode::Unattended]);
        assert_eq!(project(&claude("bypassPermissions")).mode.unwrap().current, Mode::Unattended);
        assert_eq!(project(&claude("plan")).mode.unwrap().current, Mode::Plan);
        // Auto-review is the agent's own normal operation, not unattended.
        assert_eq!(project(&claude("auto")).mode.unwrap().current, Mode::Normal);
    }

    #[test]
    fn only_a_plan_switch_is_unenforced_plan_mode() {
        assert!(in_unenforced_plan(&codex("agent", "plan")));
        assert!(!in_unenforced_plan(&codex("agent", "default")));
        // Claude's plan mode blocks edits itself.
        assert!(!in_unenforced_plan(&claude("plan")));
        assert!(!in_unenforced_plan(&[]));
    }

    #[test]
    fn projects_codex_plan_from_collaboration_mode() {
        let p = project(&codex("agent", "plan"));
        let mode = p.mode.unwrap();
        assert_eq!(mode.current, Mode::Plan);
        assert_eq!(mode.label, "Plan");
        assert!(p.effort.is_none());
        assert_eq!(project(&codex("agent", "default")).mode.unwrap().current, Mode::Normal);
    }

    #[test]
    fn unattended_is_offered_only_with_a_full_access_value() {
        let mut options = claude("default");
        if let SessionConfigKindInfo::Select(s) = &mut options[0].kind {
            s.options.retain(|v| v.kind.as_deref() != Some(FULL_ACCESS));
        }
        let mode = project(&options).mode.unwrap();
        assert_eq!(mode.available, vec![Mode::Normal, Mode::Plan]);
        assert_eq!(
            resolve(&options, &req(Mode::Unattended), None).unwrap_err().code,
            "option_unavailable"
        );
    }

    #[test]
    fn no_mode_without_a_choice() {
        let options = vec![select("model", "model", "m", vec![value("m", "M", None)])];
        assert!(project(&options).mode.is_none());
        assert!(resolve(&options, &req(Mode::Plan), None).is_err());
    }

    #[test]
    fn resolves_claude_modes() {
        assert_eq!(resolve(&claude("default"), &req(Mode::Plan), None).unwrap(), sets(&[("mode", "plan")]));
        assert_eq!(
            resolve(&claude("default"), &req(Mode::Unattended), None).unwrap(),
            sets(&[("mode", "bypassPermissions")])
        );
        // Plan off returns to the remembered normal mode…
        assert_eq!(
            resolve(&claude("plan"), &req(Mode::Normal), Some("acceptEdits")).unwrap(),
            sets(&[("mode", "acceptEdits")])
        );
        // …or the agent's first standard mode.
        assert_eq!(
            resolve(&claude("bypassPermissions"), &req(Mode::Normal), None).unwrap(),
            sets(&[("mode", "default")])
        );
        // Already normal: nothing to send.
        assert!(resolve(&claude("acceptEdits"), &req(Mode::Normal), None).unwrap().is_empty());
    }

    #[test]
    fn resolves_codex_modes() {
        assert_eq!(
            resolve(&codex("agent", "default"), &req(Mode::Plan), None).unwrap(),
            sets(&[("collaboration_mode", "plan")])
        );
        assert_eq!(
            resolve(&codex("agent-full-access", "default"), &req(Mode::Plan), Some("agent")).unwrap(),
            sets(&[("mode", "agent"), ("collaboration_mode", "plan")])
        );
        assert_eq!(
            resolve(&codex("agent", "plan"), &req(Mode::Unattended), None).unwrap(),
            sets(&[("collaboration_mode", "default"), ("mode", "agent-full-access")])
        );
        assert_eq!(
            resolve(&codex("agent", "plan"), &req(Mode::Normal), None).unwrap(),
            sets(&[("collaboration_mode", "default")])
        );
    }

    #[test]
    fn rejects_values_the_agent_does_not_offer() {
        let bad_model = OptionsRequest {
            model: Some("bogus".into()),
            ..Default::default()
        };
        let err = resolve(&claude("default"), &bad_model, None).unwrap_err();
        assert_eq!(err.code, "option_unavailable");
        let effort = OptionsRequest {
            effort: Some("low".into()),
            ..Default::default()
        };
        assert!(resolve(&codex("agent", "default"), &effort, None).is_err());
        let both = OptionsRequest {
            model: Some("haiku".into()),
            effort: Some("low".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve(&claude("default"), &both, None).unwrap(),
            sets(&[("model", "haiku"), ("effort", "low")])
        );
    }

    #[test]
    fn an_idle_session_never_inherits_another_sessions_mode() {
        // The catalog was last advertised by a session running unattended.
        let catalog = claude("bypassPermissions");
        let fresh = Stored::default();
        assert_eq!(project(&idle_options(catalog.clone(), &fresh)).mode.unwrap().current, Mode::Normal);
        // Its own stored mode still wins.
        let own = Stored {
            values: [("mode".to_string(), "plan".to_string())].into(),
            normal_mode: None,
        };
        assert_eq!(project(&idle_options(catalog, &own)).mode.unwrap().current, Mode::Plan);
    }

    #[test]
    fn overlay_replays_only_offered_values() {
        let values: BTreeMap<String, String> = [
            ("model".to_string(), "haiku".to_string()),
            ("effort".to_string(), "retired".to_string()),
        ]
        .into();
        let p = project(&overlay(claude("default"), &values));
        assert_eq!(p.model.unwrap().current, "haiku");
        assert_eq!(p.effort.unwrap().current, "high");
    }

    #[test]
    fn maps_claude_plan_exit_choices_to_modes() {
        let claude = AgentType::ClaudeCode;
        assert_eq!(mode_after_plan_exit(claude, "exit-plan-default"), Some("default"));
        assert_eq!(mode_after_plan_exit(claude, "exit-plan-accept-edits"), Some("acceptEdits"));
        assert_eq!(mode_after_plan_exit(claude, "exit-plan-bypass"), Some("bypassPermissions"));
        assert_eq!(mode_after_plan_exit(claude, "reject"), None);
        assert_eq!(mode_after_plan_exit(AgentType::Codex, "exit-plan-default"), None);
    }

    #[test]
    fn tracks_normal_mode_and_values() {
        let mut stored = Stored::default();
        stored.values = Native::of(&claude("acceptEdits")).tracked_values();
        remember_normal(&mut stored, &claude("acceptEdits"));
        assert_eq!(stored.normal_mode.as_deref(), Some("acceptEdits"));
        assert_eq!(stored.values.get("mode").map(String::as_str), Some("acceptEdits"));
        assert!(!stored.values.contains_key("fast"));
        stored.values = Native::of(&claude("plan")).tracked_values();
        remember_normal(&mut stored, &claude("plan"));
        assert_eq!(stored.normal_mode.as_deref(), Some("acceptEdits"));
    }

    /// Claude on Haiku as captured live: the model switched, no effort, no fast.
    fn claude_on_haiku() -> Vec<SessionConfigOptionInfo> {
        let mut options = claude("default");
        options.retain(|o| o.id == "mode" || o.id == "model");
        if let SessionConfigKindInfo::Select(s) = &mut options[1].kind {
            s.current_value = "haiku".into();
        }
        options
    }

    #[test]
    fn a_haiku_session_keeps_effort_for_new_sessions() {
        let catalog = merge_catalog(Some(claude("default")), &claude_on_haiku());
        // Every supported effort value still starts a new session with it…
        for effort in ["low", "high"] {
            let request = OptionsRequest {
                model: Some("opus[1m]".into()),
                effort: Some(effort.into()),
                ..Default::default()
            };
            let mut fresh = catalog.clone();
            for option in &mut fresh {
                if let SessionConfigKindInfo::Select(s) = &mut option.kind {
                    s.current_value.clear();
                }
            }
            assert_eq!(
                resolve(&fresh, &request, None).unwrap(),
                sets(&[("model", "opus[1m]"), ("effort", effort)])
            );
        }
        // …and New Session still offers it, next to what the last session reported.
        let p = project(&catalog);
        assert_eq!(p.effort.unwrap().current, "high");
        assert_eq!(p.model.unwrap().current, "haiku");
    }

    #[test]
    fn a_report_replaces_what_it_names() {
        let mut low = claude("plan");
        if let SessionConfigKindInfo::Select(s) = &mut low[2].kind {
            s.current_value = "low".into();
        }
        let catalog = merge_catalog(Some(claude("default")), &low);
        assert_eq!(catalog.len(), 4);
        assert_eq!(project(&catalog).effort.unwrap().current, "low");
        assert_eq!(merge_catalog(None, &claude_on_haiku()).len(), 2);
    }

    #[test]
    fn learns_the_effort_each_model_offers() {
        let efforts = learn_model_effort(ModelEfforts::new(), &claude("default"));
        let efforts = learn_model_effort(efforts, &claude_on_haiku());
        assert_eq!(efforts.get("opus[1m]").unwrap().as_ref().unwrap().id, "effort");
        assert!(efforts.get("haiku").unwrap().is_none());
        // Another Opus report updates Opus only.
        let efforts = learn_model_effort(efforts, &claude("plan"));
        assert_eq!(efforts.len(), 2);
        assert!(efforts.get("haiku").unwrap().is_none());
    }

    #[tokio::test]
    async fn new_sessions_get_each_models_own_effort() {
        use crate::db::test_helpers::fresh_in_memory_db;
        let db = fresh_in_memory_db().await;
        let claude_code = AgentType::ClaudeCode;
        let start = |model: &str, effort: Option<&str>| OptionsRequest {
            model: Some(model.into()),
            effort: effort.map(str::to_string),
            ..Default::default()
        };
        remember_agent_report(&db.conn, "claude_code", &claude("default")).await;
        remember_agent_report(&db.conn, "claude_code", &claude_on_haiku()).await;

        let listed = effort_by_model(&db.conn, "claude_code").await;
        let opus = listed.get("opus[1m]").unwrap().as_ref().unwrap();
        assert_eq!(opus.values.iter().map(|v| v.value.as_str()).collect::<Vec<_>>(), ["low", "high"]);
        assert_eq!(listed.get("haiku"), Some(&None));
        // The agent-wide view keeps effort for the models that have it.
        assert!(for_new_session(&db.conn, "claude_code").await.unwrap().effort.is_some());

        // Haiku with an effort is refused up front instead of silently dropped…
        let err = preferred_for_start(&db.conn, claude_code, &start("haiku", Some("low")))
            .await
            .unwrap_err();
        assert_eq!(err.code, "option_unavailable");
        assert_eq!(err.details.unwrap()["option"], "effort");
        // …Haiku alone starts, and a model with effort still gets it.
        let haiku = preferred_for_start(&db.conn, claude_code, &start("haiku", None)).await.unwrap().unwrap();
        assert_eq!(haiku.get("model").map(String::as_str), Some("haiku"));
        assert!(!haiku.contains_key("effort"));
        let opus = preferred_for_start(&db.conn, claude_code, &start("opus[1m]", Some("low")))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(opus.get("effort").map(String::as_str), Some("low"));
        // A value the model doesn't offer is refused as before.
        let err = preferred_for_start(&db.conn, claude_code, &start("opus[1m]", Some("max")))
            .await
            .unwrap_err();
        assert_eq!(err.code, "option_unavailable");
        assert!(effort_by_model(&db.conn, "codex").await.is_empty());
    }
}
