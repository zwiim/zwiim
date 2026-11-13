//! The Host browser's agent tools (design 10.2): [`BrowserToolAccess`] over
//! [`BrowserService`].
//!
//! An agent reaches its own session's tabs and nothing else (10.1). A tab of
//! another session, or a personal one, is answered as if it did not exist,
//! so a refusal tells an agent nothing about what else is open.

use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use sea_orm::EntityTrait;
use serde_json::json;

use super::agent::{ActionRequest, AgentTabSummary, GrantLevel};
use super::capture::CaptureRequest;
use super::console::ConsoleQuery;
use super::engine::cdp::Cdp;
use super::eval::EvalRequest;
use super::service::{BrowserError, BrowserService, TabInfo};
use super::store::TabOwner;
use crate::acp::browser_tools::{
    BrowserActOutcome, BrowserCaptureOutcome, BrowserConsoleOutcome, BrowserEvalOutcome,
    BrowserSnapshotOutcome, BrowserTabOp, BrowserTabOutcome, BrowserTabsOutcome, BrowserToolAccess,
    ERROR_BAD_ADDRESS, ERROR_NO_SUCH_TAB, ERROR_OPEN_FAILED, ERROR_UNAVAILABLE,
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

/// The tools for the Host this process runs.
pub struct HostBrowserTools {
    state: Weak<AppState>,
}

impl HostBrowserTools {
    pub fn new(state: &Arc<AppState>) -> Self {
        Self {
            state: Arc::downgrade(state),
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
        _caller: &str,
        tab_id: &str,
        _max_chars: Option<usize>,
    ) -> BrowserSnapshotOutcome {
        BrowserSnapshotOutcome::refused(tab_id, ERROR_UNAVAILABLE, NO_SESSION_NOTE)
    }

    async fn act(&self, _caller: &str, tab_id: &str, _request: ActionRequest) -> BrowserActOutcome {
        BrowserActOutcome::refused(tab_id, ERROR_UNAVAILABLE, NO_SESSION_NOTE)
    }

    async fn console(
        &self,
        _caller: &str,
        tab_id: &str,
        _query: ConsoleQuery,
    ) -> BrowserConsoleOutcome {
        BrowserConsoleOutcome::refused(tab_id, ERROR_UNAVAILABLE, NO_SESSION_NOTE)
    }

    async fn capture(
        &self,
        _caller: &str,
        tab_id: &str,
        _request: CaptureRequest,
    ) -> BrowserCaptureOutcome {
        BrowserCaptureOutcome::refused(tab_id, ERROR_UNAVAILABLE, NO_SESSION_NOTE)
    }

    async fn eval(&self, _caller: &str, tab_id: &str, _request: EvalRequest) -> BrowserEvalOutcome {
        BrowserEvalOutcome::refused(tab_id, ERROR_UNAVAILABLE, NO_SESSION_NOTE)
    }

    async fn tab_op(&self, caller: &str, op: BrowserTabOp) -> BrowserTabOutcome {
        match self.context(caller).await {
            Some((browser, session)) => tab_op(&browser, &session, op).await,
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
            return match browser.close(&tab_id).await {
                Ok(()) => BrowserTabOutcome::closed(&tab_id),
                Err(_) => BrowserTabOutcome::refused(
                    Some(&tab_id),
                    ERROR_NO_SUCH_TAB,
                    no_such_tab(&tab_id),
                ),
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
            "s1",
            BrowserTabOp::Open {
                url: "https://nowhere.invalid/".into(),
            },
        )
        .await;
        assert_eq!(failed.error, None);
        assert_eq!(failed.load_error.as_deref(), Some("dns"));
    }
}
