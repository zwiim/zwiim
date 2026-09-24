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

// ─── Session operations ──────────────────────────────────────────────────

/// Session `row`'s options: live from its agent when one is attached, else
/// what the agent will hold once respawned. `None` when neither is known.
pub async fn read(
    db: &DatabaseConnection,
    connections: &ConnectionManager,
    row: &conversation::Model,
    session_id: &str,
) -> Option<SessionOptions> {
    if let Some(live) = live_options(connections, row).await {
        return Some(project(&live));
    }
    let catalog = agent_catalog(db, &row.agent_type).await?;
    let stored = load(db, session_id).await;
    Some(project(&idle_options(catalog, &stored)))
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
    let catalog = agent_catalog(db, &row.agent_type)
        .await
        .ok_or_else(|| option_unavailable_any(request))?;
    let options = idle_options(catalog, &stored);
    let sets = resolve(&options, request, stored.normal_mode.as_deref())?;
    let mut next = stored;
    for (config_id, value) in sets {
        next.values.insert(config_id, value);
    }
    remember_normal(&mut next, &options);
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
/// reporting it. Claude (claude-agent-acp ≤ 0.79) applies the mode chosen on
/// ExitPlanMode inside the SDK but leaves its advertised mode on `plan`.
fn mode_after_plan_exit(agent: AgentType, option_id: &str) -> Option<&'static str> {
    match (agent, option_id) {
        (AgentType::ClaudeCode, "exit-plan-default") => Some("default"),
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
    let Some(state) = connections.get_state(connection_id).await else {
        return;
    };
    let config_id = {
        let s = state.read().await;
        let options = s.config_options.clone().unwrap_or_default();
        let native = Native::of(&options);
        match native.mode {
            Some(mode) if mode.current() != target && mode.find(target).is_some() => {
                mode.id.to_string()
            }
            _ => return,
        }
    };
    if let Err(e) = connections
        .set_config_option(connection_id, config_id, target.to_string())
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
        write_json(db, &agent_key(&agent), &options).await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::types::SessionConfigBooleanInfo;

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
    fn claude(mode: &str) -> Vec<SessionConfigOptionInfo> {
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
    fn codex(mode: &str, collaboration: &str) -> Vec<SessionConfigOptionInfo> {
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
}
