//! The Host browser's agent tools (design 10.2): [`BrowserToolAccess`] over
//! [`BrowserService`].
//!
//! An agent reaches its own session's tabs and nothing else (10.1). A tab of
//! another session, or a personal one, is answered as if it did not exist,
//! so a refusal tells an agent nothing about what else is open.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use sea_orm::EntityTrait;
use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex};

use super::agent::{
    self, ActionError, ActionKind, ActionOutcome, ActionRequest, AgentTabSummary, Fidelity,
    GrantLevel, PageSnapshot, PointerButton, RectAnswer, SnapshotRequest, WorldAnswer,
};
use super::capture::{self, CaptureOutcome, CaptureRegion, CaptureRequest};
use super::console::{ConsoleLevel, ConsoleQuery};
use super::engine::cdp::Cdp;
use super::eval::EvalRequest;
use super::service::{AgentAction, BrowserError, BrowserEvent, BrowserService, TabInfo};
use super::store::TabOwner;
use super::ui::UiKind;
use crate::acp::browser_tools::{
    ActionEffects, BrowserActOutcome, BrowserCaptureOutcome, BrowserConsoleOutcome,
    BrowserEvalOutcome, BrowserSnapshotOutcome, BrowserTabOp, BrowserTabOutcome,
    BrowserTabsOutcome, BrowserToolAccess, DEFAULT_SNAPSHOT_MAX_CHARS, ERROR_ACTION_FAILED,
    ERROR_BAD_ADDRESS, ERROR_NO_SUCH_TAB, ERROR_OPEN_FAILED, ERROR_READ_FAILED, ERROR_UNAVAILABLE,
    ERROR_WAIT_TIMEOUT,
};
use crate::app_state::AppState;
use crate::db::entities::conversation;

/// Said to an agent that is not in a session, or whose Host cannot run the
/// browser.
pub const NO_SESSION_NOTE: &str =
    "The browser is not available to this agent: it is not running in a Zwiim session.";

/// The longest a navigation is waited on before the tool answers anyway.
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// How long after the address commits the page is given to finish loading.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);
const SETTLE_POLL: Duration = Duration::from_millis(100);
/// How long an action's effects get to start (a click that navigates)
/// before the page is waited on.
const ACTION_GRACE: Duration = Duration::from_millis(150);
/// How long `browser_wait_for` waits when not told, and at most.
const WAIT_DEFAULT_MS: u64 = 10_000;
const WAIT_MAX_MS: u64 = 60_000;
const WAIT_POLL: Duration = Duration::from_millis(200);
/// The isolated world the page-reading script runs in, where page scripts
/// cannot see or change it.
const WORLD_NAME: &str = "zwiim-agent";
/// At most this many console errors are told with an action, each cut to
/// this many characters.
const EFFECT_ERRORS: usize = 5;
const EFFECT_ERROR_CHARS: usize = 300;

/// The tools for the Host this process runs.
pub struct HostBrowserTools {
    state: Weak<AppState>,
    worlds: Worlds,
}

impl HostBrowserTools {
    pub fn new(state: &Arc<AppState>) -> Self {
        Self {
            state: Arc::downgrade(state),
            worlds: Worlds::default(),
        }
    }

    /// The browser and the session `caller` is in, or `None` when there is
    /// no such session or no browser.
    async fn context(&self, caller: &str) -> Option<(Arc<BrowserService>, String)> {
        let state = self.state.upgrade()?;
        let conversation_id = {
            let session = state.connection_manager.get_state(caller).await?;
            let id = session.read().await.conversation_id?;
            id
        };
        let session_id = conversation::Entity::find_by_id(conversation_id)
            .one(&state.db.conn)
            .await
            .ok()??
            .session_id?;
        let browser = state.browser().await.ok()?;
        Some((browser, session_id))
    }
}

#[async_trait]
impl BrowserToolAccess for HostBrowserTools {
    async fn list_tabs(&self, caller: &str) -> BrowserTabsOutcome {
        match self.context(caller).await {
            Some((browser, session)) => list(&browser, &session).await,
            None => BrowserTabsOutcome::unavailable(NO_SESSION_NOTE),
        }
    }

    async fn snapshot(
        &self,
        caller: &str,
        tab_id: &str,
        max_chars: Option<usize>,
    ) -> BrowserSnapshotOutcome {
        match self.context(caller).await {
            Some((browser, session)) => {
                snapshot(&browser, &self.worlds, &session, tab_id, max_chars).await
            }
            None => BrowserSnapshotOutcome::refused(tab_id, ERROR_UNAVAILABLE, NO_SESSION_NOTE),
        }
    }

    async fn act(&self, caller: &str, tab_id: &str, request: ActionRequest) -> BrowserActOutcome {
        match self.context(caller).await {
            Some((browser, session)) => {
                act(&browser, &self.worlds, &session, tab_id, &request).await
            }
            None => BrowserActOutcome::refused(tab_id, ERROR_UNAVAILABLE, NO_SESSION_NOTE),
        }
    }

    async fn console(
        &self,
        caller: &str,
        tab_id: &str,
        query: ConsoleQuery,
    ) -> BrowserConsoleOutcome {
        match self.context(caller).await {
            Some((browser, session)) => console(&browser, &session, tab_id, &query).await,
            None => BrowserConsoleOutcome::refused(tab_id, ERROR_UNAVAILABLE, NO_SESSION_NOTE),
        }
    }

    async fn capture(
        &self,
        caller: &str,
        tab_id: &str,
        request: CaptureRequest,
    ) -> BrowserCaptureOutcome {
        match self.context(caller).await {
            Some((browser, session)) => {
                screenshot(&browser, &self.worlds, &session, tab_id, &request).await
            }
            None => BrowserCaptureOutcome::refused(tab_id, ERROR_UNAVAILABLE, NO_SESSION_NOTE),
        }
    }

    async fn eval(&self, _caller: &str, tab_id: &str, _request: EvalRequest) -> BrowserEvalOutcome {
        BrowserEvalOutcome::refused(tab_id, ERROR_UNAVAILABLE, NO_SESSION_NOTE)
    }

    async fn tab_op(&self, caller: &str, op: BrowserTabOp) -> BrowserTabOutcome {
        match self.context(caller).await {
            Some((browser, session)) => tab_op(&browser, &self.worlds, &session, op).await,
            None => BrowserTabOutcome::refused(op.tab_id(), ERROR_UNAVAILABLE, NO_SESSION_NOTE),
        }
    }
}

fn owner(session: &str) -> TabOwner {
    TabOwner::Session {
        session_id: session.to_string(),
    }
}

/// What an agent is told about one of its own tabs. It may do anything with
/// them, so every one is at `control`.
fn summary(tab: &TabInfo) -> AgentTabSummary {
    let origin = url::Url::parse(&tab.url)
        .ok()
        .map(|u| u.origin())
        .filter(|o| o.is_tuple())
        .map(|o| o.ascii_serialization());
    AgentTabSummary {
        tab_id: tab.id.clone(),
        origin,
        level: GrantLevel::Control,
        title: Some(tab.title.clone()),
    }
}

fn no_such_tab(tab_id: &str) -> String {
    format!("No browser tab {tab_id} is open.")
}

pub(crate) async fn list(browser: &BrowserService, session: &str) -> BrowserTabsOutcome {
    BrowserTabsOutcome {
        tabs: browser
            .list(Some(session))
            .await
            .iter()
            .map(summary)
            .collect(),
        note: None,
    }
}

/// `tab_id`, when it is one of `session`'s tabs.
async fn own_tab(browser: &BrowserService, session: &str, tab_id: &str) -> Option<TabInfo> {
    browser
        .tab(tab_id)
        .await
        .filter(|t| t.owner == owner(session))
}

pub(crate) async fn tab_op(
    browser: &BrowserService,
    worlds: &Worlds,
    session: &str,
    op: BrowserTabOp,
) -> BrowserTabOutcome {
    let (tab_id, url) = match op {
        BrowserTabOp::Open { url } => {
            if let Err(e) = super::service::check_url(&url) {
                return BrowserTabOutcome::refused(None, ERROR_BAD_ADDRESS, e.message);
            }
            match browser.open(owner(session), None).await {
                Ok(tab) => (tab.id, url),
                Err(e) => return BrowserTabOutcome::refused(None, ERROR_OPEN_FAILED, e.message),
            }
        }
        BrowserTabOp::Navigate { tab_id, url } => {
            if own_tab(browser, session, &tab_id).await.is_none() {
                return BrowserTabOutcome::refused(
                    Some(&tab_id),
                    ERROR_NO_SUCH_TAB,
                    no_such_tab(&tab_id),
                );
            }
            (tab_id, url)
        }
        BrowserTabOp::Close { tab_id } => {
            if own_tab(browser, session, &tab_id).await.is_none() {
                return BrowserTabOutcome::refused(
                    Some(&tab_id),
                    ERROR_NO_SUCH_TAB,
                    no_such_tab(&tab_id),
                );
            }
            worlds.0.lock().await.remove(&tab_id);
            return match browser.close(&tab_id).await {
                Ok(()) => BrowserTabOutcome::closed(&tab_id),
                Err(_) => BrowserTabOutcome::refused(
                    Some(&tab_id),
                    ERROR_NO_SUCH_TAB,
                    no_such_tab(&tab_id),
                ),
            };
        }
        BrowserTabOp::Permissions { tab_id, grant } => {
            let Some(tab) = own_tab(browser, session, &tab_id).await else {
                return BrowserTabOutcome::refused(
                    Some(&tab_id),
                    ERROR_NO_SUCH_TAB,
                    no_such_tab(&tab_id),
                );
            };
            let Some(origin) = summary(&tab).origin else {
                return BrowserTabOutcome::refused(
                    Some(&tab_id),
                    ERROR_ACTION_FAILED,
                    "The tab is on no site yet; open one first.",
                );
            };
            if let Err(e) = browser.session(&tab_id).await {
                return BrowserTabOutcome::refused(Some(&tab_id), ERROR_ACTION_FAILED, e.message);
            }
            return match browser.set_permissions(&tab.owner, &origin, &grant).await {
                Ok(()) => {
                    let mut out = BrowserTabOutcome::tab(summary(&tab), None);
                    out.note = Some(if grant.is_empty() {
                        format!("{origin} may use none of the site permissions in your tabs.")
                    } else {
                        format!(
                            "{origin} may now use {} in your tabs; the rest are denied.",
                            grant.join(", ")
                        )
                    });
                    out
                }
                Err(e) => BrowserTabOutcome::refused(Some(&tab_id), ERROR_ACTION_FAILED, e.message),
            };
        }
        BrowserTabOp::Wait {
            tab_id,
            text,
            selector,
            gone,
            timeout_ms,
        } => {
            let Some(tab) = own_tab(browser, session, &tab_id).await else {
                return BrowserTabOutcome::refused(
                    Some(&tab_id),
                    ERROR_NO_SUCH_TAB,
                    no_such_tab(&tab_id),
                );
            };
            let condition = match (text, selector) {
                (Some(text), _) => Condition::Text(text),
                (None, Some(selector)) => Condition::Selector(selector),
                (None, None) => {
                    return BrowserTabOutcome::refused(
                        Some(&tab_id),
                        ERROR_ACTION_FAILED,
                        "Say what to wait for: `text` or `selector`.",
                    )
                }
            };
            let timeout =
                Duration::from_millis(timeout_ms.unwrap_or(WAIT_DEFAULT_MS).min(WAIT_MAX_MS));
            return match wait_for(browser, worlds, &tab_id, &condition, gone, timeout).await {
                Ok(true) => match browser.tab(&tab_id).await {
                    Some(tab) => BrowserTabOutcome::tab(summary(&tab), None),
                    None => BrowserTabOutcome::refused(Some(&tab_id), ERROR_NO_SUCH_TAB, no_such_tab(&tab_id)),
                },
                Ok(false) => BrowserTabOutcome::refused(
                    Some(&tab_id),
                    ERROR_WAIT_TIMEOUT,
                    format!(
                        "After {} ms, {} on {}. Take a browser_snapshot to see what the page shows instead.",
                        timeout.as_millis(),
                        condition.describe(gone),
                        tab.url
                    ),
                ),
                Err(m) => BrowserTabOutcome::refused(Some(&tab_id), ERROR_ACTION_FAILED, m),
            };
        }
    };
    let load_error = match load(browser, &tab_id, &url).await {
        Ok(load_error) => load_error,
        Err(e) if e.code == "unsupported_url" => {
            return BrowserTabOutcome::refused(Some(&tab_id), ERROR_BAD_ADDRESS, e.message)
        }
        Err(e) => return BrowserTabOutcome::refused(Some(&tab_id), ERROR_OPEN_FAILED, e.message),
    };
    match browser.tab(&tab_id).await {
        Some(tab) => BrowserTabOutcome::tab(summary(&tab), load_error),
        None => BrowserTabOutcome::refused(Some(&tab_id), ERROR_OPEN_FAILED, no_such_tab(&tab_id)),
    }
}

/// Send the tab to `url` and wait for the page to load, at most
/// [`LOAD_TIMEOUT`] plus [`SETTLE_TIMEOUT`]. The answer is the kind of error
/// page the tab shows instead, if it does.
async fn load(
    browser: &BrowserService,
    tab_id: &str,
    url: &str,
) -> Result<Option<String>, BrowserError> {
    let url = super::service::check_url(url)?;
    let (cdp, session) = browser.session(tab_id).await?;
    let reply = cdp
        .call_with_timeout(
            "Page.navigate",
            json!({ "url": url }),
            Some(&session),
            LOAD_TIMEOUT,
        )
        .await?;
    if let Some(error) = reply["errorText"].as_str() {
        return Ok(Some(load_error_kind(error).to_string()));
    }
    settle(&cdp, &session).await;
    Ok(None)
}

/// Wait until the document has loaded, at most [`SETTLE_TIMEOUT`].
async fn settle(cdp: &Cdp, session: &str) {
    let ready = async {
        loop {
            let state = cdp
                .call(
                    "Runtime.evaluate",
                    json!({ "expression": "document.readyState", "returnByValue": true }),
                    Some(session),
                )
                .await;
            match state {
                Ok(v) if v["result"]["value"] == "complete" => return,
                Err(_) => return,
                Ok(_) => tokio::time::sleep(SETTLE_POLL).await,
            }
        }
    };
    let _ = tokio::time::timeout(SETTLE_TIMEOUT, ready).await;
}

/// What `browser_wait_for` waits on.
enum Condition {
    Text(String),
    Selector(String),
}

impl Condition {
    /// Whether it holds, as an expression for the tab's world.
    fn expression(&self) -> String {
        match self {
            Condition::Text(text) => format!(
                "String(!!document.body && document.body.innerText.includes({}))",
                Value::from(text.as_str())
            ),
            Condition::Selector(selector) => format!(
                "String(!!document.querySelector({}))",
                Value::from(selector.as_str())
            ),
        }
    }

    fn describe(&self, gone: bool) -> String {
        let (what, verb) = match self {
            Condition::Text(text) => (format!("the text {text:?}"), "on the page"),
            Condition::Selector(selector) => {
                (format!("an element matching {selector:?}"), "on the page")
            }
        };
        if gone {
            format!("{what} is still {verb}")
        } else {
            format!("{what} is not {verb}")
        }
    }
}

/// Wait until `condition` holds (or, with `gone`, stops holding), at most
/// `timeout`. A new document part way through is waited on like the old.
async fn wait_for(
    browser: &BrowserService,
    worlds: &Worlds,
    tab_id: &str,
    condition: &Condition,
    gone: bool,
    timeout: Duration,
) -> Result<bool, String> {
    let (cdp, session) = browser.session(tab_id).await.map_err(|e| e.message)?;
    let expression = condition.expression();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let context = match worlds.current(tab_id).await {
            Some(context) => Some(context),
            None => match worlds.create(&cdp, &session, tab_id).await {
                Ok(context) => Some(context),
                Err(WorldError::Gone) => None,
                Err(WorldError::Failed(m)) => return Err(m),
            },
        };
        if let Some(context) = context {
            match eval_in(&cdp, &session, context, &expression).await {
                Ok(holds) if (holds == "true") != gone => return Ok(true),
                Ok(_) => {}
                Err(WorldError::Gone) => worlds.forget(tab_id, context).await,
                Err(WorldError::Failed(m)) => return Err(m),
            }
        }
        if tokio::time::Instant::now() + WAIT_POLL > deadline {
            return Ok(false);
        }
        tokio::time::sleep(WAIT_POLL).await;
    }
}

/// Each tab's isolated world, by tab id: the execution context the
/// page-reading script was installed in. A new document ends it, and with it
/// every ref the script handed out.
#[derive(Default)]
pub(crate) struct Worlds(Mutex<HashMap<String, i64>>);

/// Why the world could not be asked.
enum WorldError {
    /// The world went with its document: refs from it are stale.
    Gone,
    Failed(String),
}

impl From<BrowserError> for WorldError {
    fn from(e: BrowserError) -> Self {
        WorldError::Failed(e.message)
    }
}

impl Worlds {
    /// The tab's world as it is, if the script was installed in it.
    async fn current(&self, tab_id: &str) -> Option<i64> {
        self.0.lock().await.get(tab_id).copied()
    }

    /// A new world in the tab's main frame.
    async fn create(&self, cdp: &Cdp, session: &str, tab_id: &str) -> Result<i64, WorldError> {
        let tree = cdp
            .call("Page.getFrameTree", json!({}), Some(session))
            .await
            .map_err(|e| WorldError::Failed(e.to_string()))?;
        let frame = tree["frameTree"]["frame"]["id"].clone();
        let world = cdp
            .call(
                "Page.createIsolatedWorld",
                json!({ "frameId": frame, "worldName": WORLD_NAME }),
                Some(session),
            )
            .await
            .map_err(|e| WorldError::Failed(e.to_string()))?;
        let context = world["executionContextId"]
            .as_i64()
            .ok_or_else(|| WorldError::Failed("the engine made no world".into()))?;
        self.0.lock().await.insert(tab_id.to_string(), context);
        Ok(context)
    }

    async fn forget(&self, tab_id: &str, context: i64) {
        let mut worlds = self.0.lock().await;
        if worlds.get(tab_id) == Some(&context) {
            worlds.remove(tab_id);
        }
    }
}

/// Evaluate `expression` in world `context`; its value is a string.
async fn eval_in(
    cdp: &Cdp,
    session: &str,
    context: i64,
    expression: &str,
) -> Result<String, WorldError> {
    let reply = cdp
        .call(
            "Runtime.evaluate",
            json!({ "expression": expression, "contextId": context, "returnByValue": true }),
            Some(session),
        )
        .await
        .map_err(|e| {
            let message = e.to_string();
            if message.contains("Cannot find context") {
                WorldError::Gone
            } else {
                WorldError::Failed(message)
            }
        })?;
    if let Some(details) = reply.get("exceptionDetails") {
        let text = details["exception"]["description"]
            .as_str()
            .or_else(|| details["text"].as_str())
            .unwrap_or("the page script threw");
        return Err(WorldError::Failed(text.to_string()));
    }
    match &reply["result"]["value"] {
        Value::String(s) => Ok(s.clone()),
        other => Err(WorldError::Failed(format!("unexpected answer {other}"))),
    }
}

/// The host's half of a snapshot's generation: which world it came from.
fn epoch_of(context: i64) -> String {
    agent::epoch(context.unsigned_abs(), 0)
}

pub(crate) async fn snapshot(
    browser: &BrowserService,
    worlds: &Worlds,
    session: &str,
    tab_id: &str,
    max_chars: Option<usize>,
) -> BrowserSnapshotOutcome {
    if own_tab(browser, session, tab_id).await.is_none() {
        return BrowserSnapshotOutcome::refused(tab_id, ERROR_NO_SUCH_TAB, no_such_tab(tab_id));
    }
    let request = SnapshotRequest {
        max_chars: Some(max_chars.unwrap_or(DEFAULT_SNAPSHOT_MAX_CHARS)),
    };
    match read_page(browser, worlds, tab_id, &request).await {
        Ok(page) => BrowserSnapshotOutcome::page(tab_id, page),
        Err(message) => BrowserSnapshotOutcome::refused(tab_id, ERROR_READ_FAILED, message),
    }
}

/// Take a snapshot in the tab's world, making the world and installing the
/// script first when the page has none.
async fn read_page(
    browser: &BrowserService,
    worlds: &Worlds,
    tab_id: &str,
    request: &SnapshotRequest,
) -> Result<PageSnapshot, String> {
    let (cdp, session) = browser.session(tab_id).await.map_err(|e| e.message)?;
    // A world from an earlier document is gone; one more try makes a new one.
    for _ in 0..2 {
        let context = match worlds.current(tab_id).await {
            Some(context) => context,
            None => match worlds.create(&cdp, &session, tab_id).await {
                Ok(context) => context,
                Err(WorldError::Gone) => continue,
                Err(WorldError::Failed(m)) => return Err(m),
            },
        };
        let epoch = epoch_of(context);
        let raw = match eval_in(
            &cdp,
            &session,
            context,
            &agent::probe_and_snapshot(request, &epoch),
        )
        .await
        {
            Ok(raw) if raw == agent::ENGINE_ABSENT => {
                eval_in(
                    &cdp,
                    &session,
                    context,
                    &agent::install_and_snapshot(request, &epoch),
                )
                .await
            }
            other => other,
        };
        match raw {
            Ok(raw) => {
                return serde_json::from_str(&raw)
                    .map_err(|e| format!("the page answered with an unreadable snapshot: {e}"))
            }
            Err(WorldError::Gone) => worlds.forget(tab_id, context).await,
            Err(WorldError::Failed(m)) => return Err(m),
        }
    }
    Err("the page kept changing while it was read; try again".into())
}

pub(crate) async fn act(
    browser: &BrowserService,
    worlds: &Worlds,
    session: &str,
    tab_id: &str,
    request: &ActionRequest,
) -> BrowserActOutcome {
    if own_tab(browser, session, tab_id).await.is_none() {
        return BrowserActOutcome::refused(tab_id, ERROR_NO_SUCH_TAB, no_such_tab(tab_id));
    }
    if request.target.is_none() && !matches!(request.action, ActionKind::Press { .. }) {
        return BrowserActOutcome::refused(
            tab_id,
            ERROR_ACTION_FAILED,
            "This action needs the ref of an element from a snapshot.",
        );
    }
    let (cdp, cdp_session) = match browser.session(tab_id).await {
        Ok(found) => found,
        Err(e) => return BrowserActOutcome::refused(tab_id, ERROR_ACTION_FAILED, e.message),
    };
    let stale = |detail: &str| BrowserActOutcome::stale_ref(tab_id, detail);
    let Some(context) = worlds.current(tab_id).await else {
        return stale("No snapshot has been taken of this page; take one first.");
    };
    if !agent::ref_is_current(&request.generation, &epoch_of(context)) {
        return stale("The page has navigated since that snapshot; take a new one.");
    }
    let watch = Watch::start(browser, session, tab_id).await;
    let show = |x: f64, y: f64, bounds: Option<CaptureRegion>| {
        browser.agent_action(AgentAction {
            tab_id: tab_id.to_string(),
            kind: kind_of(&request.action),
            x,
            y,
            bounds,
        })
    };
    let outcome = match (&request.action, request.target.as_deref()) {
        (ActionKind::Click { .. } | ActionKind::Hover, Some(target)) => {
            pointer(&cdp, &cdp_session, context, request, target, |x, y| {
                show(x, y, None)
            })
            .await
        }
        _ => {
            if let Some(target) = request.target.as_deref() {
                let rect = agent::rect_call(&request.generation, target);
                if let Ok(raw) = eval_in(&cdp, &cdp_session, context, &rect).await {
                    if let Ok(RectAnswer {
                        ok: true,
                        x: Some(x),
                        y: Some(y),
                        width: Some(w),
                        height: Some(h),
                        ..
                    }) = serde_json::from_str::<RectAnswer>(&raw)
                    {
                        let bounds = CaptureRegion {
                            x,
                            y,
                            width: w,
                            height: h,
                        };
                        show(x + w / 2.0, y + h / 2.0, Some(bounds));
                    }
                }
            }
            match eval_in(&cdp, &cdp_session, context, &agent::act_call(request)).await {
                Ok(raw) => answer(&raw, Fidelity::Synthetic),
                Err(e) => Err(e),
            }
        }
    };
    match outcome {
        Ok(Ok(outcome)) => {
            tokio::time::sleep(ACTION_GRACE).await;
            settle(&cdp, &cdp_session).await;
            let effects = watch.effects(browser, session, tab_id).await;
            BrowserActOutcome::done(tab_id, outcome).with_effects(effects)
        }
        Ok(Err((Some(ActionError::Stale) | None, detail))) => stale(&detail),
        Ok(Err((Some(_), detail))) => {
            BrowserActOutcome::refused(tab_id, ERROR_ACTION_FAILED, detail)
        }
        Err(WorldError::Gone) => {
            worlds.forget(tab_id, context).await;
            stale("The page has navigated since that snapshot; take a new one.")
        }
        Err(WorldError::Failed(m)) => BrowserActOutcome::refused(tab_id, ERROR_ACTION_FAILED, m),
    }
}

/// What a tab is like before an action, to tell what the action caused.
struct Watch {
    events: broadcast::Receiver<BrowserEvent>,
    url: Option<String>,
    console: u64,
    /// The session's tabs, the acting one first.
    tabs: Vec<String>,
}

impl Watch {
    async fn start(browser: &BrowserService, session: &str, tab_id: &str) -> Self {
        let events = browser.subscribe();
        let mut tabs = vec![tab_id.to_string()];
        tabs.extend(
            browser
                .list(Some(session))
                .await
                .into_iter()
                .map(|t| t.id)
                .filter(|id| id != tab_id),
        );
        Self {
            events,
            url: own_tab(browser, session, tab_id).await.map(|t| t.url),
            console: browser.console_mark(tab_id).await,
            tabs,
        }
    }

    /// What happened since [`Watch::start`]: the tab moved, the page opened
    /// tabs, downloads or dialogs, or it printed errors.
    async fn effects(
        mut self,
        browser: &BrowserService,
        session: &str,
        tab_id: &str,
    ) -> ActionEffects {
        let mut effects = ActionEffects::default();
        let mine = owner(session);
        // The acting tab and those it opened: what they ask or download is
        // the action's doing.
        let mut caused = vec![tab_id.to_string()];
        loop {
            match self.events.try_recv() {
                Ok(BrowserEvent::Tab(tab)) if tab.owner == mine && !self.tabs.contains(&tab.id) => {
                    self.tabs.push(tab.id.clone());
                    caused.push(tab.id.clone());
                    effects.opened_tabs.push(tab.id);
                }
                Ok(BrowserEvent::Ui(request)) if caused.contains(&request.tab_id) => {
                    effects.dialogs.push(asked(&request.kind));
                }
                Ok(BrowserEvent::Download { tab_id, name }) if caused.contains(&tab_id) => {
                    effects.downloads.push(name);
                }
                Ok(_) | Err(broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
        let now = own_tab(browser, session, tab_id).await.map(|t| t.url);
        if now.is_some() && now != self.url {
            effects.navigated = now;
        }
        let query = ConsoleQuery {
            since: self.console,
            min_level: Some(ConsoleLevel::Error),
            limit: Some(EFFECT_ERRORS),
        };
        if let Some(read) = browser.console(tab_id, &query).await {
            effects.errors = read
                .entries
                .into_iter()
                .map(|e| e.text.chars().take(EFFECT_ERROR_CHARS).collect())
                .collect();
        }
        effects
    }
}

/// What a page asked of a person, in a few words.
fn asked(kind: &UiKind) -> String {
    match kind {
        UiKind::Dialog {
            dialog, message, ..
        } => {
            let dialog = serde_json::to_value(dialog)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            let message: String = message.chars().take(EFFECT_ERROR_CHARS).collect();
            format!("{dialog}: {message}")
        }
        UiKind::Auth { origin, .. } => format!("sign-in for {origin}"),
        UiKind::FileChooser { .. } => "file chooser".to_string(),
    }
}

/// The action's name as clients show it.
fn kind_of(action: &ActionKind) -> &'static str {
    match action {
        ActionKind::Click { .. } => "click",
        ActionKind::Hover => "hover",
        ActionKind::Type { .. } => "type",
        ActionKind::Press { .. } => "press",
        ActionKind::Select { .. } => "select",
    }
}

/// What the world refused, and its words for it.
type Refusal = (Option<ActionError>, String);

/// The world's answer as the agent's: the outcome, or the refusal it was.
fn answer(raw: &str, fidelity: Fidelity) -> Result<Result<ActionOutcome, Refusal>, WorldError> {
    if raw == agent::ENGINE_ABSENT {
        return Ok(Err((
            None,
            "No snapshot has been taken of this page; take one first.".into(),
        )));
    }
    let answer: WorldAnswer = serde_json::from_str(raw)
        .map_err(|e| WorldError::Failed(format!("the page answered unreadably: {e}")))?;
    if answer.ok {
        return Ok(Ok(ActionOutcome {
            fidelity,
            url: answer.url,
            scrolled: answer.scrolled,
        }));
    }
    let detail = answer
        .detail
        .unwrap_or_else(|| "the action could not be done".to_string());
    Ok(Err((answer.error, detail)))
}

/// Ask the world where the element is and put a real pointer there.
async fn pointer(
    cdp: &Cdp,
    session: &str,
    context: i64,
    request: &ActionRequest,
    target: &str,
    show: impl FnOnce(f64, f64),
) -> Result<Result<ActionOutcome, Refusal>, WorldError> {
    let raw = eval_in(
        cdp,
        session,
        context,
        &agent::locate_call(&request.generation, target),
    )
    .await?;
    let located = match answer(&raw, Fidelity::Trusted)? {
        Ok(_) => serde_json::from_str::<WorldAnswer>(&raw)
            .map_err(|e| WorldError::Failed(format!("the page answered unreadably: {e}")))?,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let (Some(x), Some(y)) = (located.x, located.y) else {
        return Err(WorldError::Failed(
            "the page did not say where the element is".into(),
        ));
    };
    show(x, y);
    let mouse = |kind: &str, button: &str, clicks: u8| json!({ "type": kind, "x": x, "y": y, "button": button, "clickCount": clicks });
    let mut events = vec![mouse("mouseMoved", "none", 0)];
    if let ActionKind::Click { button, count } = &request.action {
        let button = match button.unwrap_or_default() {
            PointerButton::Left => "left",
            PointerButton::Right => "right",
        };
        for n in 1..=count.unwrap_or(1).clamp(1, 3) {
            events.push(mouse("mousePressed", button, n));
            events.push(mouse("mouseReleased", button, n));
        }
    }
    for event in events {
        cdp.call("Input.dispatchMouseEvent", event, Some(session))
            .await
            .map_err(|e| WorldError::Failed(e.to_string()))?;
    }
    Ok(Ok(ActionOutcome {
        fidelity: Fidelity::Trusted,
        url: located.url,
        scrolled: None,
    }))
}

pub(crate) async fn console(
    browser: &BrowserService,
    session: &str,
    tab_id: &str,
    query: &ConsoleQuery,
) -> BrowserConsoleOutcome {
    if own_tab(browser, session, tab_id).await.is_none() {
        return BrowserConsoleOutcome::refused(tab_id, ERROR_NO_SUCH_TAB, no_such_tab(tab_id));
    }
    match browser.console(tab_id, query).await {
        Some(readout) => BrowserConsoleOutcome::lines(tab_id, readout),
        None => BrowserConsoleOutcome::refused(tab_id, ERROR_NO_SUCH_TAB, no_such_tab(tab_id)),
    }
}

pub(crate) async fn screenshot(
    browser: &BrowserService,
    worlds: &Worlds,
    session: &str,
    tab_id: &str,
    request: &CaptureRequest,
) -> BrowserCaptureOutcome {
    let Some(tab) = own_tab(browser, session, tab_id).await else {
        return BrowserCaptureOutcome::refused(tab_id, ERROR_NO_SUCH_TAB, no_such_tab(tab_id));
    };
    let (cdp, cdp_session) = match browser.session(tab_id).await {
        Ok(found) => found,
        Err(e) => return BrowserCaptureOutcome::refused(tab_id, ERROR_READ_FAILED, e.message),
    };
    settle(&cdp, &cdp_session).await;
    let clip = match request.clip_target() {
        None => None,
        Some((generation, target)) => {
            match element_box(&cdp, &cdp_session, worlds, tab_id, generation, target).await {
                Ok(region) => Some(region),
                Err(Shot::Stale(detail)) => {
                    return BrowserCaptureOutcome::stale_ref(tab_id, &detail)
                }
                Err(Shot::Failed(m)) => {
                    return BrowserCaptureOutcome::refused(tab_id, ERROR_READ_FAILED, m)
                }
            }
        }
    };
    match capture_viewport(&cdp, &cdp_session, clip, request).await {
        Ok((fitted, viewport)) => BrowserCaptureOutcome::image(
            tab_id,
            CaptureOutcome {
                mime: request.format.mime().to_string(),
                data: B64.encode(&fitted.bytes),
                width: fitted.width,
                height: fitted.height,
                url: tab.url,
                region: clip.unwrap_or(viewport),
                clipped: clip.is_some(),
            },
        ),
        Err(m) => BrowserCaptureOutcome::refused(tab_id, ERROR_READ_FAILED, m),
    }
}

enum Shot {
    Stale(String),
    Failed(String),
}

/// The element's visible box, in viewport CSS pixels, brought into view.
async fn element_box(
    cdp: &Cdp,
    session: &str,
    worlds: &Worlds,
    tab_id: &str,
    generation: &str,
    target: &str,
) -> Result<CaptureRegion, Shot> {
    let navigated =
        || Shot::Stale("The page has navigated since that snapshot; take a new one.".into());
    let Some(context) = worlds.current(tab_id).await else {
        return Err(Shot::Stale(
            "No snapshot has been taken of this page; take one first.".into(),
        ));
    };
    if !agent::ref_is_current(generation, &epoch_of(context)) {
        return Err(navigated());
    }
    let raw = match eval_in(cdp, session, context, &agent::rect_call(generation, target)).await {
        Ok(raw) => raw,
        Err(WorldError::Gone) => {
            worlds.forget(tab_id, context).await;
            return Err(navigated());
        }
        Err(WorldError::Failed(m)) => return Err(Shot::Failed(m)),
    };
    if raw == agent::ENGINE_ABSENT {
        return Err(navigated());
    }
    let rect: RectAnswer = serde_json::from_str(&raw)
        .map_err(|e| Shot::Failed(format!("the page answered unreadably: {e}")))?;
    let detail = || {
        rect.detail
            .clone()
            .unwrap_or_else(|| "the element cannot be captured".into())
    };
    if !rect.ok {
        return Err(match rect.error {
            Some(ActionError::Stale) | None => Shot::Stale(detail()),
            Some(_) => Shot::Failed(detail()),
        });
    }
    let (Some(x), Some(y), Some(width), Some(height)) = (rect.x, rect.y, rect.width, rect.height)
    else {
        return Err(Shot::Failed(detail()));
    };
    if rect.scrolled {
        // The engine owes a frame of the scrolled page.
        tokio::time::sleep(SETTLE_POLL).await;
    }
    Ok(CaptureRegion {
        x,
        y,
        width,
        height,
    })
}

/// The viewport's pixels, cropped to `clip` and fitted to the request, and
/// the viewport's own box in CSS pixels.
async fn capture_viewport(
    cdp: &Cdp,
    session: &str,
    clip: Option<CaptureRegion>,
    request: &CaptureRequest,
) -> Result<(capture::Fitted, CaptureRegion), String> {
    let metrics = cdp
        .call("Page.getLayoutMetrics", json!({}), Some(session))
        .await
        .map_err(|e| e.to_string())?;
    let css = &metrics["cssVisualViewport"];
    let viewport = CaptureRegion {
        x: 0.0,
        y: 0.0,
        width: css["clientWidth"].as_f64().unwrap_or(0.0),
        height: css["clientHeight"].as_f64().unwrap_or(0.0),
    };
    let shot = cdp
        .call(
            "Page.captureScreenshot",
            json!({ "format": "png" }),
            Some(session),
        )
        .await
        .map_err(|e| e.to_string())?;
    let png = B64
        .decode(shot["data"].as_str().unwrap_or_default())
        .map_err(|e| format!("the engine sent an unreadable capture: {e}"))?;
    let max_width = capture::effective_max_width(request.max_width);
    let format = request.format;
    let width = viewport.width;
    let fitted =
        tokio::task::spawn_blocking(move || capture::fit(&png, width, clip, max_width, format))
            .await
            .map_err(|e| e.to_string())??;
    Ok((fitted, viewport))
}

/// Chromium's `net::ERR_*` name as one of the kinds the tools report.
fn load_error_kind(error: &str) -> &'static str {
    if error.contains("NAME_NOT_RESOLVED") || error.contains("NAME_RESOLUTION") {
        "dns"
    } else if error.contains("CERT") || error.contains("SSL") {
        "tls"
    } else if error.contains("BLOCKED") {
        "blocked"
    } else {
        "failed"
    }
}

#[cfg(test)]
mod tests {
    use super::super::engine::fake::FakeEngine;
    use super::super::engine::manager::EngineManager;
    use super::*;
    use crate::acp::browser_tools::BrowserTabOp;
    use crate::db::test_helpers::fresh_in_memory_db;

    async fn browser(fake: &FakeEngine) -> Arc<BrowserService> {
        let db = fresh_in_memory_db().await;
        BrowserService::start(EngineManager::new(fake.start_fn()), db.conn.clone(), None)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn an_agent_opens_lists_and_closes_its_own_tabs_only() {
        let fake = FakeEngine::default();
        let browser = browser(&fake).await;
        let other = browser
            .open(owner("s2"), Some("https://other.example/"))
            .await
            .unwrap();
        let personal = browser.open(TabOwner::Personal, None).await.unwrap();

        let opened = tab_op(
            &browser,
            &Worlds::default(),
            "s1",
            BrowserTabOp::Open {
                url: "http://localhost:5173/".into(),
            },
        )
        .await;
        assert_eq!(opened.error, None, "{:?}", opened.note);
        let tab = opened.tab.unwrap();
        assert_eq!(tab.origin.as_deref(), Some("http://localhost:5173"));
        assert_eq!(tab.level, GrantLevel::Control);

        let listed = list(&browser, "s1").await;
        assert_eq!(listed.tabs.len(), 1);
        assert_eq!(listed.tabs[0].tab_id, tab.tab_id);

        for foreign in [&other.id, &personal.id] {
            let nav = tab_op(
                &browser,
                &Worlds::default(),
                "s1",
                BrowserTabOp::Navigate {
                    tab_id: foreign.clone(),
                    url: "https://example.com/".into(),
                },
            )
            .await;
            assert_eq!(nav.error.as_deref(), Some(ERROR_NO_SUCH_TAB));
            let close = tab_op(
                &browser,
                &Worlds::default(),
                "s1",
                BrowserTabOp::Close {
                    tab_id: foreign.clone(),
                },
            )
            .await;
            assert_eq!(close.error.as_deref(), Some(ERROR_NO_SUCH_TAB));
        }
        assert!(browser.tab(&other.id).await.is_some());
        assert!(browser.tab(&personal.id).await.is_some());

        let moved = tab_op(
            &browser,
            &Worlds::default(),
            "s1",
            BrowserTabOp::Navigate {
                tab_id: tab.tab_id.clone(),
                url: "https://example.com/a".into(),
            },
        )
        .await;
        assert_eq!(
            moved.tab.unwrap().origin.as_deref(),
            Some("https://example.com")
        );

        let closed = tab_op(
            &browser,
            &Worlds::default(),
            "s1",
            BrowserTabOp::Close {
                tab_id: tab.tab_id.clone(),
            },
        )
        .await;
        assert_eq!(closed.error, None);
        assert!(list(&browser, "s1").await.tabs.is_empty());
    }

    #[tokio::test]
    async fn a_bad_address_or_an_error_page_is_reported() {
        let fake = FakeEngine::default();
        let browser = browser(&fake).await;
        let bad = tab_op(
            &browser,
            &Worlds::default(),
            "s1",
            BrowserTabOp::Open {
                url: "file:///etc/passwd".into(),
            },
        )
        .await;
        assert_eq!(bad.error.as_deref(), Some(ERROR_BAD_ADDRESS));
        assert!(list(&browser, "s1").await.tabs.is_empty(), "nothing opened");

        fake.lock().nav_error = Some("net::ERR_NAME_NOT_RESOLVED".into());
        let failed = tab_op(
            &browser,
            &Worlds::default(),
            "s1",
            BrowserTabOp::Open {
                url: "https://nowhere.invalid/".into(),
            },
        )
        .await;
        assert_eq!(failed.error, None);
        assert_eq!(failed.load_error.as_deref(), Some("dns"));
    }

    #[tokio::test]
    async fn an_action_reports_the_tabs_downloads_and_moves_it_caused() {
        let fake = FakeEngine::default();
        let browser = browser(&fake).await;
        let tab = browser
            .open(owner("s1"), Some("http://localhost:5173/"))
            .await
            .unwrap();
        let other = browser
            .open(owner("s2"), Some("http://localhost:5174/"))
            .await
            .unwrap();
        let target = fake.wait_url("http://localhost:5173/").await;
        let theirs = fake.wait_url("http://localhost:5174/").await;

        let watch = Watch::start(&browser, "s1", &tab.id).await;
        fake.open_popup(&target, "http://localhost:5173/pop");
        fake.open_popup(&theirs, "http://localhost:5174/pop");
        for (frame, name) in [(&target, "report.pdf"), (&theirs, "secret.pdf")] {
            fake.emit(
                "Browser.downloadWillBegin",
                json!({ "frameId": frame, "guid": name, "suggestedFilename": name, "url": "http://localhost/x" }),
                None,
            );
        }
        browser
            .navigate(
                &tab.id,
                super::super::service::Navigation::Url("http://localhost:5173/done".into()),
            )
            .await
            .unwrap();
        fake.wait_url("http://localhost:5173/done").await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while browser.list(None).await.len() < 4 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("both popups became tabs");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let effects = watch.effects(&browser, "s1", &tab.id).await;
        assert_eq!(
            effects.navigated.as_deref(),
            Some("http://localhost:5173/done")
        );
        assert_eq!(effects.opened_tabs.len(), 1, "{effects:?}");
        assert_ne!(effects.opened_tabs[0], other.id);
        assert_eq!(effects.downloads, vec!["report.pdf".to_string()]);
        assert!(effects.dialogs.is_empty() && effects.errors.is_empty());
    }

    /// The page-reading script reads a real page, a click lands as real
    /// input, typing reaches the field, and a new document makes old refs
    /// stale. Point `ZWIIM_TEST_ENGINE` at a `chrome-headless-shell` binary.
    #[tokio::test]
    #[ignore = "needs ZWIIM_TEST_ENGINE"]
    async fn the_real_engine_reads_and_acts_on_a_page() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dir = tempfile::tempdir().unwrap();
        let start = super::super::engine::fake::real_engine(dir.path().join("profile"));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = sock.read(&mut buf).await;
                    let body = "<title>Form</title><input aria-label=Name>\
                        <script>console.error('boom', 42)</script><button onclick=\"document.title='clicked:'+event.isTrusted;document.body.append('Thanks, Ada!');window.open('/pop');console.error('after click')\">Go</button>";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        let db = fresh_in_memory_db().await;
        let browser = BrowserService::start(EngineManager::new(start), db.conn.clone(), None)
            .await
            .unwrap();
        let worlds = Worlds::default();
        let url = format!("http://127.0.0.1:{port}/");
        let opened = tab_op(
            &browser,
            &worlds,
            "s1",
            BrowserTabOp::Open { url: url.clone() },
        )
        .await;
        let tab = opened.tab.expect("opened").tab_id;

        let page = snapshot(&browser, &worlds, "s1", &tab, None).await;
        let page = page.snapshot.unwrap_or_else(|| panic!("{:?}", page.note));
        let ref_of = |tree: &str, role: &str| {
            let line = tree.lines().find(|l| l.contains(role)).unwrap().to_string();
            let start = line.find("[ref=").unwrap() + 5;
            line[start..start + line[start..].find(']').unwrap()].to_string()
        };
        let button = ref_of(&page.tree, "button \"Go\"");
        let field = ref_of(&page.tree, "textbox \"Name\"");
        let request = |target: &str, action| ActionRequest {
            generation: page.generation.clone(),
            target: Some(target.to_string()),
            action,
        };

        let typed = act(
            &browser,
            &worlds,
            "s1",
            &tab,
            &request(
                &field,
                ActionKind::Type {
                    text: "Ada".into(),
                    submit: false,
                },
            ),
        )
        .await;
        assert_eq!(typed.error, None, "{:?}", typed.note);
        let mut events = browser.subscribe();
        let clicked = act(
            &browser,
            &worlds,
            "s1",
            &tab,
            &request(
                &button,
                ActionKind::Click {
                    button: None,
                    count: None,
                },
            ),
        )
        .await;
        assert_eq!(clicked.action.unwrap().fidelity, Fidelity::Trusted);
        // The click's popup and error came back with it.
        let effects = clicked.effects.expect("the click caused things");
        assert_eq!(effects.opened_tabs.len(), 1, "{effects:?}");
        assert_eq!(effects.errors, vec!["after click".to_string()]);
        assert_eq!(effects.navigated, None);
        // Clients were shown where the pointer went, before it landed.
        let shown = loop {
            if let super::super::service::BrowserEvent::AgentAction(a) =
                events.recv().await.unwrap()
            {
                break a;
            }
        };
        assert_eq!((shown.tab_id.as_str(), shown.kind), (tab.as_str(), "click"));
        assert!(shown.x > 0.0 && shown.y > 0.0);
        let after = snapshot(&browser, &worlds, "s1", &tab, None)
            .await
            .snapshot
            .unwrap();
        assert_eq!(after.title, "clicked:true");
        assert!(after.tree.contains("Ada"), "{}", after.tree);

        let printed = console(&browser, "s1", &tab, &ConsoleQuery::default()).await;
        let lines = printed.console.unwrap().entries;
        assert!(
            lines.iter().any(|l| l.text == "boom 42" && l.top),
            "{lines:?}"
        );

        let whole = screenshot(&browser, &worlds, "s1", &tab, &CaptureRequest::default()).await;
        let whole = whole.capture.unwrap_or_else(|| panic!("{:?}", whole.note));
        assert!(!whole.clipped && whole.width > 0 && whole.region.width > 0.0);
        let shot = CaptureRequest {
            generation: Some(after.generation.clone()),
            target: Some(ref_of(&after.tree, "button \"Go\"")),
            ..Default::default()
        };
        let element = screenshot(&browser, &worlds, "s1", &tab, &shot).await;
        let element = element
            .capture
            .unwrap_or_else(|| panic!("{:?}", element.note));
        assert!(
            element.clipped && element.width < whole.width,
            "{:?}",
            element.region
        );

        // Waiting: what is there holds at once; what never comes times out.
        let wait = |text: &str, gone: bool| BrowserTabOp::Wait {
            tab_id: tab.clone(),
            text: Some(text.into()),
            selector: None,
            gone,
            timeout_ms: Some(400),
        };
        let shown = tab_op(&browser, &worlds, "s1", wait("Thanks, Ada!", false)).await;
        assert_eq!(shown.error, None, "{:?}", shown.note);
        let late = tab_op(&browser, &worlds, "s1", wait("Never", false)).await;
        assert_eq!(late.error.as_deref(), Some(ERROR_WAIT_TIMEOUT));
        let away = tab_op(&browser, &worlds, "s1", wait("Never", true)).await;
        assert_eq!(away.error, None);

        // Permissions: granted for this site, the rest denied.
        let set = tab_op(
            &browser,
            &worlds,
            "s1",
            BrowserTabOp::Permissions {
                tab_id: tab.clone(),
                grant: vec!["geolocation".into()],
            },
        )
        .await;
        assert_eq!(set.error, None, "{:?}", set.note);
        let (cdp, cdp_session) = browser.session(&tab).await.unwrap();
        let states = cdp
            .call(
                "Runtime.evaluate",
                json!({ "expression": "Promise.all(['geolocation','camera'].map(n => navigator.permissions.query({name: n}).then(p => p.state))).then(a => a.join())",
                        "awaitPromise": true, "returnByValue": true }),
                Some(&cdp_session),
            )
            .await
            .unwrap();
        assert_eq!(states["result"]["value"], "granted,denied");

        // Another session sees none of it.
        let other = snapshot(&browser, &worlds, "s2", &tab, None).await;
        assert_eq!(other.error.as_deref(), Some(ERROR_NO_SUCH_TAB));

        // A new document: the old generation's refs are stale.
        tab_op(
            &browser,
            &worlds,
            "s1",
            BrowserTabOp::Navigate {
                tab_id: tab.clone(),
                url,
            },
        )
        .await;
        let late = act(
            &browser,
            &worlds,
            "s1",
            &tab,
            &request(
                &button,
                ActionKind::Click {
                    button: None,
                    count: None,
                },
            ),
        )
        .await;
        assert_eq!(
            late.error.as_deref(),
            Some(crate::acp::browser_tools::ERROR_STALE_REF),
            "{:?}",
            late.note
        );
        browser.stop_engine().await;
    }
}
