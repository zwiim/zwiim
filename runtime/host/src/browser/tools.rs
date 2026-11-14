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
use super::network::{self, NetworkEntry};
use super::service::{
    AgentAction, BrowserError, BrowserEvent, BrowserService, RefusedLoad, TabInfo,
};
use super::sites::{self, Actor, Verdict};
use super::store::TabOwner;
use super::tree_diff;
use super::ui::UiKind;
use crate::acp::browser_tools::{
    ActionEffects, BrowserActOutcome, BrowserCaptureOutcome, BrowserConsoleOutcome,
    BrowserEvalOutcome, BrowserSnapshotOutcome, BrowserTabOp, BrowserTabOutcome,
    BrowserTabsOutcome, BrowserToolAccess, DEFAULT_SNAPSHOT_MAX_CHARS, ERROR_ACTION_FAILED,
    ERROR_BAD_ADDRESS, ERROR_BLOCKED, ERROR_NO_SUCH_TAB, ERROR_OPEN_FAILED, ERROR_PASSWORD_FIELD,
    ERROR_PERSON_TYPING, ERROR_READ_FAILED, ERROR_SITE_DENIED, ERROR_SITE_NOT_ALLOWED,
    ERROR_UNAVAILABLE, ERROR_URL_CHANGED, ERROR_WAIT_TIMEOUT,
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
/// Agent input waits this long after a person last typed into the tab, and
/// waits at most this long for them to stop (design 10.3).
const TYPING_QUIET: Duration = Duration::from_secs(2);
const TYPING_WAIT: Duration = Duration::from_secs(5);

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
        changes_only: bool,
    ) -> BrowserSnapshotOutcome {
        match self.context(caller).await {
            Some((browser, session)) => {
                snapshot(
                    &browser,
                    &self.worlds,
                    &session,
                    tab_id,
                    max_chars,
                    changes_only,
                )
                .await
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
    let tab = browser
        .tab(tab_id)
        .await
        .filter(|t| t.owner == owner(session))?;
    // The agent is using it: the memory governor leaves it alone.
    browser.agent_used(tab_id);
    Some(tab)
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
            worlds.contexts.lock().await.remove(&tab_id);
            worlds.seen.lock().await.remove(&tab_id);
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
        BrowserTabOp::Network { tab_id, query, har } => {
            let Some(tab) = own_tab(browser, session, &tab_id).await else {
                return BrowserTabOutcome::refused(
                    Some(&tab_id),
                    ERROR_NO_SUCH_TAB,
                    no_such_tab(&tab_id),
                );
            };
            let Some(mut readout) = browser.network(&tab_id, &query).await else {
                return BrowserTabOutcome::refused(
                    Some(&tab_id),
                    ERROR_NO_SUCH_TAB,
                    no_such_tab(&tab_id),
                );
            };
            if har {
                match write_har(browser, &tab, &readout.entries).await {
                    Ok(path) => readout.har = Some(path),
                    Err(m) => {
                        return BrowserTabOutcome::refused(Some(&tab_id), ERROR_READ_FAILED, m)
                    }
                }
            }
            return BrowserTabOutcome::network(&tab_id, readout);
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
        Ok(Load::Done(load_error)) => load_error,
        Ok(Load::Asked(site)) => {
            return match browser.tab(&tab_id).await {
                Some(tab) => {
                    let mut out = BrowserTabOutcome::tab(summary(&tab), None);
                    out.note = Some(format!(
                        "The person is asked whether you may open {site}; the page loads once they \
                         allow it. Wait with browser_wait_for, or take a snapshot later."
                    ));
                    out
                }
                None => BrowserTabOutcome::refused(
                    Some(&tab_id),
                    ERROR_OPEN_FAILED,
                    no_such_tab(&tab_id),
                ),
            };
        }
        Err(e) if e.code == "unsupported_url" => {
            return BrowserTabOutcome::refused(Some(&tab_id), ERROR_BAD_ADDRESS, e.message)
        }
        Err(e) if site_error(e.code).is_some() => {
            let code = site_error(e.code).unwrap_or(ERROR_BLOCKED);
            return BrowserTabOutcome::refused(Some(&tab_id), code, e.message);
        }
        Err(e) => return BrowserTabOutcome::refused(Some(&tab_id), ERROR_OPEN_FAILED, e.message),
    };
    browser
        .record_step(session, &tab_id, format!("Opened {url}"))
        .await;
    match browser.tab(&tab_id).await {
        Some(tab) => BrowserTabOutcome::tab(summary(&tab), load_error),
        None => BrowserTabOutcome::refused(Some(&tab_id), ERROR_OPEN_FAILED, no_such_tab(&tab_id)),
    }
}

/// Write `entries` as a HAR file in the session's browser folder; its path.
async fn write_har(
    browser: &BrowserService,
    tab: &TabInfo,
    entries: &[NetworkEntry],
) -> Result<String, String> {
    let dir = browser
        .files_folder(&tab.owner)
        .await
        .ok_or("this session has no folder to write to")?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let path = super::saving::free_name(&dir, &format!("network-{}-{stamp}.har", tab.id));
    let text = serde_json::to_vec_pretty(&network::har(entries)).map_err(|e| e.to_string())?;
    tokio::fs::write(&path, text)
        .await
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(path.to_string_lossy().into_owned())
}

/// How a [`load`] ended.
enum Load {
    /// Loaded, or the kind of error page the tab shows instead.
    Done(Option<String>),
    /// The person is asked about this site first (design 13.2): the page
    /// waits for them, and nothing waits here.
    Asked(String),
}

/// Send the tab to `url` and wait for the page to load, at most
/// [`LOAD_TIMEOUT`] plus [`SETTLE_TIMEOUT`].
async fn load(browser: &BrowserService, tab_id: &str, url: &str) -> Result<Load, BrowserError> {
    let url = super::service::check_url(url)?;
    browser.agent_used(tab_id);
    let (cdp, session) = browser.session(tab_id).await?;
    browser.acted(tab_id, Actor::Agent).await;
    if let Ok(parsed) = url::Url::parse(&url) {
        match browser.site_check(tab_id, &url).await {
            Verdict::Refuse(code) => {
                return Err(BrowserError {
                    code,
                    message: site_refusal(code, &parsed),
                })
            }
            Verdict::Ask => {
                browser
                    .navigate(tab_id, super::service::Navigation::Url(url.clone()))
                    .await?;
                let site = parsed.host_str().unwrap_or("that site").to_string();
                return Ok(Load::Asked(site));
            }
            Verdict::Allow => {}
        }
    }
    let reply = cdp
        .call_with_timeout(
            "Page.navigate",
            json!({ "url": url }),
            Some(&session),
            LOAD_TIMEOUT,
        )
        .await?;
    if let Some(error) = reply["errorText"].as_str() {
        return Ok(Load::Done(Some(load_error_kind(error).to_string())));
    }
    settle(browser, tab_id, &cdp, &session).await;
    Ok(Load::Done(None))
}

/// The agent's error for a site policy refusal `code` (design 13.2).
fn site_error(code: &str) -> Option<&'static str> {
    match code {
        sites::ERROR_SITE_BLOCKED => Some(ERROR_BLOCKED),
        sites::ERROR_SITE_NOT_ALLOWED => Some(ERROR_SITE_NOT_ALLOWED),
        sites::ERROR_SITE_DENIED => Some(ERROR_SITE_DENIED),
        _ => None,
    }
}

/// Why the site policy refused loading `url`, for the agent.
fn site_refusal(code: &str, url: &url::Url) -> String {
    let site = url.host_str().unwrap_or("that site");
    match code {
        sites::ERROR_SITE_NOT_ALLOWED => format!(
            "{site} is not on this session's strict site list, so agents may not open it. \
             Nothing was loaded. Ask the person to add it to the session's sites in the \
             Browser panel."
        ),
        sites::ERROR_SITE_DENIED => {
            format!("The person denied {site} for this session. Nothing was loaded.")
        }
        "too_many_tabs" => format!(
            "The page tried to open {site} in a new tab, but this session holds its most \
             tabs, so nothing was opened. Close tabs it no longer needs first."
        ),
        _ => format!("The administrator's policy blocks {site}. Nothing was loaded."),
    }
}

/// A refused page load, in a few words.
fn refused_note(load: &RefusedLoad) -> String {
    match url::Url::parse(&load.url) {
        Ok(url) => format!("{} ({})", load.url, site_refusal(load.code, &url)),
        Err(_) => load.url.clone(),
    }
}

/// Wait until the document has loaded and none of its requests are still
/// waiting on an answer, at most [`SETTLE_TIMEOUT`].
async fn settle(browser: &BrowserService, tab_id: &str, cdp: &Cdp, session: &str) {
    let ready = async {
        loop {
            if browser.network_busy(tab_id).await > 0 {
                tokio::time::sleep(SETTLE_POLL).await;
                continue;
            }
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
pub(crate) struct Worlds {
    contexts: Mutex<HashMap<String, i64>>,
    /// The tree each tab's last snapshot showed, for the next one to say only
    /// what changed.
    seen: Mutex<HashMap<String, Seen>>,
}

/// Tabs whose last tree is kept; past it, all are dropped and the next
/// snapshot of each comes whole.
const MAX_SEEN: usize = 64;

/// What a tab's last snapshot showed the agent.
#[derive(Clone)]
struct Seen {
    /// Which document it was of: its generation without the snapshot's own
    /// number.
    document: String,
    generation: String,
    tree: String,
    /// The agent was shown all of it, so a change can be told against it.
    whole: bool,
}

/// The document a snapshot generation (`<world>.<number>.<epoch>`) is of.
fn document_of(generation: &str) -> String {
    let mut parts = generation.split('.');
    let world = parts.next().unwrap_or_default();
    parts.next();
    let epoch: Vec<&str> = parts.collect();
    format!("{world}.{}", epoch.join("."))
}

/// `text` cut to `max` bytes at a line's end, the way the engine cuts a tree;
/// `0` is no cap.
fn cut_tree(text: &str, max: usize) -> (String, bool) {
    if max == 0 || text.len() <= max {
        return (text.to_string(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let end = text[..end].rfind('\n').filter(|&n| n > 0).unwrap_or(end);
    (text[..end].to_string(), true)
}

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
        self.contexts.lock().await.get(tab_id).copied()
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
        self.contexts
            .lock()
            .await
            .insert(tab_id.to_string(), context);
        Ok(context)
    }

    async fn forget(&self, tab_id: &str, context: i64) {
        let mut worlds = self.contexts.lock().await;
        if worlds.get(tab_id) == Some(&context) {
            worlds.remove(tab_id);
        }
    }

    /// The tree the tab's last snapshot showed in full, if it did.
    async fn seen(&self, tab_id: &str) -> Option<Seen> {
        self.seen
            .lock()
            .await
            .get(tab_id)
            .filter(|seen| seen.whole)
            .cloned()
    }

    async fn saw(&self, tab_id: &str, seen: Seen) {
        let mut all = self.seen.lock().await;
        if all.len() >= MAX_SEEN && !all.contains_key(tab_id) {
            all.clear();
        }
        all.insert(tab_id.to_string(), seen);
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
    changes_only: bool,
) -> BrowserSnapshotOutcome {
    if own_tab(browser, session, tab_id).await.is_none() {
        return BrowserSnapshotOutcome::refused(tab_id, ERROR_NO_SUCH_TAB, no_such_tab(tab_id));
    }
    let cap = max_chars.unwrap_or(DEFAULT_SNAPSHOT_MAX_CHARS);
    let base = match changes_only {
        true => worlds.seen(tab_id).await,
        false => None,
    };
    // With a tree to compare against, the page is read whole, so every ref in
    // it stays live, and the answer is cut here instead.
    let request = SnapshotRequest {
        max_chars: Some(if base.is_some() { 0 } else { cap }),
    };
    let mut page = match read_page(browser, worlds, tab_id, &request).await {
        Ok(page) => page,
        Err(message) => return BrowserSnapshotOutcome::refused(tab_id, ERROR_READ_FAILED, message),
    };
    let document = document_of(&page.generation);
    let whole = page.tree.clone();
    let base = base.filter(|base| base.document == document);
    let shown = match &base {
        Some(base) => tree_diff::changes(&base.tree, &whole).unwrap_or_default(),
        None => whole.clone(),
    };
    let (shown, truncated) = cut_tree(&shown, cap);
    page.truncated = page.truncated || truncated;
    page.changes_since = base.map(|base| base.generation);
    worlds
        .saw(
            tab_id,
            Seen {
                document,
                generation: page.generation.clone(),
                tree: if page.changes_since.is_some() {
                    whole
                } else {
                    shown.clone()
                },
                whole: !page.truncated,
            },
        )
        .await;
    page.tree = shown;
    BrowserSnapshotOutcome::page(tab_id, page)
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
    let Some(tab) = own_tab(browser, session, tab_id).await else {
        return BrowserActOutcome::refused(tab_id, ERROR_NO_SUCH_TAB, no_such_tab(tab_id));
    };
    if let Some(expected) = request.expect_url.as_deref() {
        if !same_page(expected, &tab.url) {
            return BrowserActOutcome::refused(
                tab_id,
                ERROR_URL_CHANGED,
                format!(
                    "Tab {tab_id} is at {}, not {expected}, so nothing was done. Take a \
                     browser_snapshot to see the page it is on.",
                    tab.url
                ),
            );
        }
    }
    if !person_quiet(browser, tab_id, TYPING_WAIT).await {
        return BrowserActOutcome::refused(
            tab_id,
            ERROR_PERSON_TYPING,
            format!(
                "The person is typing in tab {tab_id}, so nothing was done. Wait a little and \
                 try again, or leave this tab to them."
            ),
        );
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
    if types_text(&request.action) && !passwords_allowed(browser, &tab.url).await {
        let call = agent::password_field_call(&request.generation, request.target.as_deref());
        match eval_in(&cdp, &cdp_session, context, &call).await {
            Ok(raw) if raw == "false" => {}
            Ok(raw) if raw == "true" => {
                return BrowserActOutcome::refused(
                    tab_id,
                    ERROR_PASSWORD_FIELD,
                    "That is a password field, and agents may type passwords only on this \
                     computer's dev servers (localhost). Nothing was typed. Ask the person to \
                     sign in themselves, or to turn on \"Let agents type passwords on any \
                     site\" in Zwiim's Settings > Browser.",
                );
            }
            Ok(_) => return stale("No snapshot has been taken of this page; take one first."),
            Err(WorldError::Gone) => {
                worlds.forget(tab_id, context).await;
                return stale("The page has navigated since that snapshot; take a new one.");
            }
            Err(WorldError::Failed(m)) => {
                return BrowserActOutcome::refused(tab_id, ERROR_ACTION_FAILED, m)
            }
        }
    }
    browser.acted(tab_id, Actor::Agent).await;
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
            settle(browser, tab_id, &cdp, &cdp_session).await;
            let effects = watch.effects(browser, session, tab_id).await;
            browser
                .record_step(session, tab_id, step_note(&request.action))
                .await;
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
        effects.refused = browser
            .refused_loads(tab_id)
            .await
            .iter()
            .map(refused_note)
            .collect();
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
        UiKind::Select { .. } => "a list to pick from".to_string(),
        UiKind::Date { input_type, .. } => format!("a {input_type} picker"),
        UiKind::Site { site, .. } => {
            format!("whether you may open {site}: the page waits for the person's answer")
        }
    }
}

/// Wait until the person has not typed into the tab for [`TYPING_QUIET`],
/// at most `limit`. Whether they stopped.
async fn person_quiet(browser: &BrowserService, tab_id: &str, limit: Duration) -> bool {
    let give_up = tokio::time::Instant::now() + limit;
    loop {
        let Some(typed) = browser.last_typed(tab_id).await else {
            return true;
        };
        let quiet_at = typed + TYPING_QUIET;
        let now = tokio::time::Instant::now();
        if quiet_at <= now {
            return true;
        }
        if quiet_at > give_up {
            return false;
        }
        tokio::time::sleep_until(quiet_at).await;
    }
}

/// Whether the action puts text into the page: typing, or a key that types a
/// character. Enter, Tab and shortcuts type nothing.
fn types_text(action: &ActionKind) -> bool {
    match action {
        ActionKind::Type { .. } => true,
        ActionKind::Press { key } => {
            let mut parts = key.split('+');
            let last = parts.next_back().unwrap_or_default();
            let shortcut = parts.any(|m| matches!(m, "Control" | "Meta" | "Alt"));
            last.chars().count() == 1 && !shortcut
        }
        _ => false,
    }
}

/// Whether agents may type a password on the page at `url`: always on this
/// computer's dev servers, elsewhere only when the person turned it on.
async fn passwords_allowed(browser: &BrowserService, url: &str) -> bool {
    let loopback = url::Url::parse(url).is_ok_and(|u| match u.host() {
        Some(url::Host::Domain(d)) => d == "localhost" || d.ends_with(".localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    });
    loopback || browser.agent_passwords().await
}

/// Whether `expected` names the page at `url`; the fragment does not count.
fn same_page(expected: &str, url: &str) -> bool {
    let page = |raw: &str| {
        url::Url::parse(raw.trim())
            .map(|mut u| {
                u.set_fragment(None);
                u.to_string()
            })
            .unwrap_or_else(|_| raw.trim().to_string())
    };
    page(expected) == page(url)
}

/// The action's name as clients show it.
/// An action as its recorded step says it (design 10.7). Typed text is
/// never kept: it may be a secret.
fn step_note(action: &ActionKind) -> String {
    match action {
        ActionKind::Click { count: Some(2), .. } => "Double-clicked".into(),
        ActionKind::Click {
            button: Some(PointerButton::Right),
            ..
        } => "Right-clicked".into(),
        ActionKind::Click { .. } => "Clicked".into(),
        ActionKind::Hover => "Hovered".into(),
        ActionKind::Type { submit: true, .. } => "Typed into a field and pressed Enter".into(),
        ActionKind::Type { .. } => "Typed into a field".into(),
        ActionKind::Press { key } => format!("Pressed {key}"),
        ActionKind::Select { .. } => "Chose an option".into(),
    }
}

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
    settle(browser, tab_id, &cdp, &cdp_session).await;
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

    /// What an agent does in its tabs is recorded, a picture and a line a
    /// step, when the Host keeps a timeline (design 10.7).
    #[tokio::test]
    async fn an_agents_page_loads_are_recorded_as_steps() {
        let fake = FakeEngine::default();
        let browser = browser(&fake).await;
        let dir = tempfile::tempdir().unwrap();
        browser.keep_timeline(dir.path().to_path_buf());
        let opened = tab_op(
            &browser,
            &Worlds::default(),
            "s1",
            BrowserTabOp::Open {
                url: "http://localhost:5173/".into(),
            },
        )
        .await;
        let tab = opened.tab.unwrap();
        let steps = browser.timeline_steps("s1", None, 10).await;
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].action, "Opened http://localhost:5173/");
        assert_eq!(steps[0].tab_id, tab.tab_id);
        assert_eq!(
            browser.timeline_image("s1", steps[0].seq).await,
            Some(super::super::engine::fake::screenshot_jpeg())
        );
        assert!(browser.timeline_steps("s2", None, 10).await.is_empty());
        assert_eq!(
            step_note(&ActionKind::Type {
                text: "hunter2".into(),
                submit: false
            }),
            "Typed into a field"
        );
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
    async fn an_agent_is_refused_a_site_off_its_list_and_not_held_up_by_a_question() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let browser =
            BrowserService::start(EngineManager::new(fake.start_fn()), db.conn.clone(), None)
                .await
                .unwrap();
        let worlds = Worlds::default();
        browser
            .set_strict_sites("s1", &["example.com".into()])
            .await
            .unwrap();
        let open = |url: &str| BrowserTabOp::Open { url: url.into() };
        let ok = tab_op(&browser, &worlds, "s1", open("https://example.com/")).await;
        assert_eq!(ok.error, None, "{:?}", ok.note);
        let refused = tab_op(&browser, &worlds, "s1", open("https://other.test/")).await;
        assert_eq!(refused.error.as_deref(), Some(ERROR_SITE_NOT_ALLOWED));
        assert!(refused
            .note
            .unwrap()
            .contains("other.test is not on this session's strict site list"));

        browser.set_strict_sites("s1", &[]).await.unwrap();
        let on = serde_json::Map::from_iter([(
            crate::settings::BROWSER_AGENT_ASK_SITES.to_string(),
            json!(true),
        )]);
        crate::settings::update(&db.conn, &on).await.unwrap();
        let asked = tokio::time::timeout(
            Duration::from_secs(5),
            tab_op(&browser, &worlds, "s1", open("https://other.test/")),
        )
        .await
        .expect("returns at once");
        assert_eq!(asked.error, None);
        assert!(asked
            .note
            .unwrap()
            .contains("The person is asked whether you may open other.test"));
        fake.wait_url("https://other.test/").await;
    }

    #[tokio::test]
    async fn an_agent_waits_for_a_person_to_stop_typing() {
        let fake = FakeEngine::default();
        let browser = browser(&fake).await;
        let tab = browser.open(owner("s1"), None).await.unwrap();
        assert!(person_quiet(&browser, &tab.id, Duration::ZERO).await);
        browser.person_typed(&tab.id).await;
        assert!(!person_quiet(&browser, &tab.id, Duration::from_millis(100)).await);
        let started = tokio::time::Instant::now();
        assert!(person_quiet(&browser, &tab.id, TYPING_WAIT).await);
        assert!(started.elapsed() >= TYPING_QUIET - Duration::from_millis(100));
        let refused = act(
            &browser,
            &Worlds::default(),
            "s1",
            &tab.id,
            &ActionRequest {
                generation: "g.0.0".into(),
                target: None,
                action: ActionKind::Press { key: "a".into() },
                expect_url: None,
            },
        )
        .await;
        assert_ne!(refused.error.as_deref(), Some(ERROR_PERSON_TYPING));
    }

    #[tokio::test]
    async fn a_page_settles_once_its_requests_are_answered() {
        let fake = FakeEngine::default();
        let browser = browser(&fake).await;
        let tab = browser
            .open(owner("s1"), Some("http://localhost:5173/"))
            .await
            .unwrap();
        let target = fake.wait_url("http://localhost:5173/").await;
        let session = fake.lock().targets[&target].session.clone().unwrap();
        fake.emit(
            "Network.requestWillBeSent",
            json!({ "requestId": "r1", "type": "Fetch", "timestamp": 1.0,
                    "request": { "method": "GET", "url": "http://localhost:5173/api", "headers": {} } }),
            Some(&session),
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while browser.network_busy(&tab.id).await == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the request is in flight");
        let (cdp, cdp_session) = browser.session(&tab.id).await.unwrap();
        let answer = {
            let fake = fake.clone();
            let session = session.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                fake.emit(
                    "Network.loadingFinished",
                    json!({ "requestId": "r1", "timestamp": 1.3, "encodedDataLength": 10 }),
                    Some(&session),
                );
            })
        };
        let started = tokio::time::Instant::now();
        settle(&browser, &tab.id, &cdp, &cdp_session).await;
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(250), "{waited:?}");
        assert!(waited < SETTLE_TIMEOUT, "{waited:?}");
        answer.await.unwrap();
    }

    #[test]
    fn only_typing_and_character_keys_put_text_in_a_field() {
        let press = |key: &str| ActionKind::Press { key: key.into() };
        assert!(types_text(&ActionKind::Type {
            text: "x".into(),
            submit: false
        }));
        assert!(types_text(&press("a")));
        assert!(types_text(&press("Shift+A")));
        assert!(!types_text(&press("Enter")));
        assert!(!types_text(&press("Control+a")));
        assert!(!types_text(&ActionKind::Hover));
    }

    #[tokio::test]
    async fn passwords_go_only_to_dev_servers_unless_the_person_allows_more() {
        let db = fresh_in_memory_db().await;
        let fake = FakeEngine::default();
        let browser =
            BrowserService::start(EngineManager::new(fake.start_fn()), db.conn.clone(), None)
                .await
                .unwrap();
        for dev in [
            "http://localhost:5173/login",
            "http://app.localhost:3000/",
            "http://127.0.0.1:8080/",
            "http://[::1]:3000/",
        ] {
            assert!(passwords_allowed(&browser, dev).await, "{dev}");
        }
        for site in [
            "https://github.com/login",
            "http://192.168.1.4/",
            "about:blank",
        ] {
            assert!(!passwords_allowed(&browser, site).await, "{site}");
        }
        let on = serde_json::Map::from_iter([(
            crate::settings::BROWSER_AGENT_PASSWORDS.to_string(),
            json!(true),
        )]);
        crate::settings::update(&db.conn, &on).await.unwrap();
        assert!(passwords_allowed(&browser, "https://github.com/login").await);
    }

    #[test]
    fn an_expected_address_matches_the_page_whatever_its_fragment() {
        assert!(same_page("http://localhost:5173", "http://localhost:5173/"));
        assert!(same_page(
            "http://localhost:5173/a#x",
            "http://localhost:5173/a"
        ));
        assert!(!same_page(
            "http://localhost:5173/a",
            "http://localhost:5173/b"
        ));
        assert!(!same_page(
            "http://localhost:5173/a?q=1",
            "http://localhost:5173/a"
        ));
    }

    #[test]
    fn a_generation_names_its_document_and_a_tree_is_cut_at_a_line() {
        assert_eq!(document_of("abc.7.12.0"), "abc.12.0");
        assert_eq!(document_of("abc.8.12.0"), document_of("abc.7.12.0"));
        assert_ne!(document_of("abd.7.12.0"), document_of("abc.7.12.0"));
        assert_eq!(
            cut_tree("- a\n- b\n- c", 0),
            ("- a\n- b\n- c".into(), false)
        );
        assert_eq!(cut_tree("- a\n- b\n- c", 6), ("- a".into(), true));
        assert_eq!(cut_tree("- é", 3), ("- ".into(), true));
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

    /// A strict site list refuses what the agent opens and what its click
    /// leads to, and never what the person opens. Point `ZWIIM_TEST_ENGINE`
    /// at a `chrome-headless-shell` binary.
    #[tokio::test]
    #[ignore = "needs ZWIIM_TEST_ENGINE"]
    async fn the_real_engine_keeps_an_agent_to_its_site_list() {
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
                    let body = format!(
                        "<title>Home</title><a href=\"http://localhost:{port}/away\">Away</a>"
                    );
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
        browser
            .set_strict_sites("s1", &[format!("127.0.0.1:{port}")])
            .await
            .unwrap();
        let worlds = Worlds::default();
        let home = format!("http://127.0.0.1:{port}/");
        let opened = tab_op(
            &browser,
            &worlds,
            "s1",
            BrowserTabOp::Open { url: home.clone() },
        )
        .await;
        let tab = opened
            .tab
            .unwrap_or_else(|| panic!("{:?}", opened.note))
            .tab_id;

        let away = format!("http://localhost:{port}/away");
        let refused = tab_op(
            &browser,
            &worlds,
            "s1",
            BrowserTabOp::Navigate {
                tab_id: tab.clone(),
                url: away.clone(),
            },
        )
        .await;
        assert_eq!(
            refused.error.as_deref(),
            Some(ERROR_SITE_NOT_ALLOWED),
            "{:?}",
            refused.note
        );
        assert_eq!(browser.tab(&tab).await.unwrap().url, home);

        let page = snapshot(&browser, &worlds, "s1", &tab, None, false).await;
        let page = page.snapshot.unwrap_or_else(|| panic!("{:?}", page.note));
        let line = page
            .tree
            .lines()
            .find(|l| l.contains("link \"Away\""))
            .unwrap();
        let start = line.find("[ref=").unwrap() + 5;
        let link = line[start..start + line[start..].find(']').unwrap()].to_string();
        let clicked = act(
            &browser,
            &worlds,
            "s1",
            &tab,
            &ActionRequest {
                generation: page.generation.clone(),
                target: Some(link),
                action: ActionKind::Click {
                    button: None,
                    count: None,
                },
                expect_url: None,
            },
        )
        .await;
        let effects = clicked.effects.expect("the click was refused somewhere");
        assert_eq!(effects.refused.len(), 1, "{effects:?}");
        assert!(effects.refused[0].starts_with(&away), "{effects:?}");

        // The person goes there.
        browser.acted(&tab, Actor::Person).await;
        browser
            .navigate(&tab, super::super::service::Navigation::Url(away.clone()))
            .await
            .unwrap();
        for _ in 0..100 {
            let now = browser.tab(&tab).await.unwrap();
            if now.url == away && now.title == "Home" {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!(
            "the person's navigation did not load: {:?}",
            browser.tab(&tab).await
        );
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
                    let body = "<title>Form</title><input aria-label=Name><input type=password aria-label=Secret>\
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

        let page = snapshot(&browser, &worlds, "s1", &tab, None, false).await;
        let page = page.snapshot.unwrap_or_else(|| panic!("{:?}", page.note));
        let ref_of = |tree: &str, role: &str| {
            let line = tree.lines().find(|l| l.contains(role)).unwrap().to_string();
            let start = line.find("[ref=").unwrap() + 5;
            line[start..start + line[start..].find(']').unwrap()].to_string()
        };
        let button = ref_of(&page.tree, "button \"Go\"");
        let field = ref_of(&page.tree, "textbox \"Name\"");
        let secret = ref_of(&page.tree, "textbox \"Secret\"");
        // The Host tells a password field from any other.
        let context = worlds.current(&tab).await.unwrap();
        let (cdp, cdp_session) = browser.session(&tab).await.unwrap();
        for (target, password) in [(&secret, "true"), (&field, "false")] {
            let call = agent::password_field_call(&page.generation, Some(target));
            let found = eval_in(&cdp, &cdp_session, context, &call).await.ok();
            assert_eq!(found.as_deref(), Some(password), "{target}");
        }
        let request = |target: &str, action| ActionRequest {
            generation: page.generation.clone(),
            target: Some(target.to_string()),
            action,
            expect_url: None,
        };

        // Expected elsewhere: refused, nothing typed.
        let elsewhere = ActionRequest {
            expect_url: Some("http://127.0.0.1:1/other".into()),
            ..request(
                &field,
                ActionKind::Type {
                    text: "Bob".into(),
                    submit: false,
                },
            )
        };
        let refused = act(&browser, &worlds, "s1", &tab, &elsewhere).await;
        assert_eq!(refused.error.as_deref(), Some(ERROR_URL_CHANGED));
        let here = ActionRequest {
            expect_url: Some(format!("{url}#top")),
            ..elsewhere
        };
        let typed = act(&browser, &worlds, "s1", &tab, &here).await;
        assert_eq!(typed.error, None, "{:?}", typed.note);

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
        // Only what changed since the first snapshot comes back.
        let after = snapshot(&browser, &worlds, "s1", &tab, None, true)
            .await
            .snapshot
            .unwrap();
        assert_eq!(after.title, "clicked:true");
        assert_eq!(
            after.changes_since.as_deref(),
            Some(page.generation.as_str())
        );
        assert!(
            after.tree.contains("+ ") && after.tree.contains("Thanks, Ada!"),
            "{}",
            after.tree
        );
        assert!(!after.tree.contains("Secret"), "{}", after.tree);
        let again = snapshot(&browser, &worlds, "s1", &tab, None, true)
            .await
            .snapshot
            .unwrap();
        assert_eq!((again.tree.as_str(), again.truncated), ("", false));
        // A ref from the first snapshot still works with the new generation.
        let hovered = act(
            &browser,
            &worlds,
            "s1",
            &tab,
            &ActionRequest {
                generation: again.generation.clone(),
                ..request(&button, ActionKind::Hover)
            },
        )
        .await;
        assert_eq!(hovered.error, None, "{:?}", hovered.note);

        let printed = console(&browser, "s1", &tab, &ConsoleQuery::default()).await;
        let lines = printed.console.unwrap().entries;
        assert!(
            lines.iter().any(|l| l.text == "boom 42" && l.top),
            "{lines:?}"
        );

        // The page's own load is in its network log.
        let log = tab_op(
            &browser,
            &worlds,
            "s1",
            BrowserTabOp::Network {
                tab_id: tab.clone(),
                query: Default::default(),
                har: false,
            },
        )
        .await;
        let log = log.network.unwrap_or_else(|| panic!("{:?}", log.note));
        assert!(
            log.entries
                .iter()
                .any(|e| e.url == url && e.kind == "Document" && e.status == Some(200)),
            "{:?}",
            log.entries
        );

        let whole = screenshot(&browser, &worlds, "s1", &tab, &CaptureRequest::default()).await;
        let whole = whole.capture.unwrap_or_else(|| panic!("{:?}", whole.note));
        assert!(!whole.clipped && whole.width > 0 && whole.region.width > 0.0);
        let shot = CaptureRequest {
            generation: Some(again.generation.clone()),
            target: Some(button.clone()),
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
        let other = snapshot(&browser, &worlds, "s2", &tab, None, false).await;
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
