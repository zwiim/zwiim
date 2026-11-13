//! `browser.*` operations: the Host browser's tabs (design 18).
//!
//! Tabs belong to a session or to the person (`owner`). Until devices carry
//! browser rights (P2a), every operation needs local host authority, so a
//! paired phone sees no tab addresses yet.

use std::sync::Arc;

use axum::{http::StatusCode, Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::error::{scrub_paths, ZJson, ZwiimError};
use crate::app_state::AppState;
use crate::browser::engine::manager::EngineState;
use crate::browser::service::{BrowserError, BrowserEvent, BrowserService, Navigation, TabInfo};
use crate::browser::store::TabOwner;

type ZResult<T> = Result<Json<T>, ZwiimError>;

impl From<BrowserError> for ZwiimError {
    fn from(e: BrowserError) -> Self {
        let (status, message) = match e.code {
            "tab_not_found" => (StatusCode::NOT_FOUND, "No such browser tab".to_string()),
            "unsupported_url" => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Only http, https and about:blank addresses open in the browser".to_string(),
            ),
            _ => (StatusCode::SERVICE_UNAVAILABLE, scrub_paths(&e.message)),
        };
        ZwiimError::new(status, e.code, message)
    }
}

/// The engine as clients see it: `{state}`, plus `version` while running
/// and `code` and `message` once it failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EngineView {
    Stopped,
    Starting,
    Running { version: String },
    Failed { code: &'static str, message: String },
}

impl From<&EngineState> for EngineView {
    fn from(state: &EngineState) -> Self {
        match state {
            EngineState::Stopped => EngineView::Stopped,
            EngineState::Starting => EngineView::Starting,
            EngineState::Running { version, .. } => EngineView::Running {
                version: version.clone(),
            },
            EngineState::Failed { code, message } => EngineView::Failed {
                code,
                message: scrub_paths(message),
            },
        }
    }
}

pub(super) async fn service(state: &AppState) -> Result<Arc<BrowserService>, ZwiimError> {
    state.browser().await.map_err(|e| {
        tracing::warn!(error = %e, "[zwiim] browser tabs could not be read");
        ZwiimError::internal()
    })
}

/// A session owner must name a live session.
async fn check_owner(state: &AppState, owner: &TabOwner) -> Result<(), ZwiimError> {
    if let TabOwner::Session { session_id } = owner {
        crate::session::resolve(&state.db.conn, session_id).await?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabsListParams {
    #[serde(default)]
    pub session_id: Option<String>,
}

/// `browser.tabs.list {session_id?}` → `{tabs, engine}`.
pub async fn tabs_list(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<TabsListParams>,
) -> ZResult<Value> {
    let browser = service(&state).await?;
    let engine = EngineView::from(&*browser.engine().state().borrow());
    Ok(Json(json!({
        "tabs": browser.list(p.session_id.as_deref()).await,
        "engine": engine,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabOpenParams {
    pub owner: TabOwner,
    #[serde(default)]
    pub url: Option<String>,
}

/// `browser.tab.open {owner, url?}` → the tab. Starts the engine, and
/// downloads it first if this Host has none yet.
pub async fn tab_open(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<TabOpenParams>,
) -> ZResult<TabInfo> {
    check_owner(&state, &p.owner).await?;
    let browser = service(&state).await?;
    Ok(Json(browser.open(p.owner, p.url.as_deref()).await?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabParams {
    pub tab_id: String,
}

/// `browser.tab.close {tab_id}`.
pub async fn tab_close(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<TabParams>,
) -> ZResult<Value> {
    service(&state).await?.close(&p.tab_id).await?;
    Ok(Json(json!({})))
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavigateAction {
    Back,
    Forward,
    Reload,
    Stop,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabNavigateParams {
    pub tab_id: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub action: Option<NavigateAction>,
}

/// `browser.tab.navigate {tab_id, url | action}` → the tab. `action` is
/// `back`, `forward`, `reload` or `stop`. A discarded tab loads again.
pub async fn tab_navigate(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<TabNavigateParams>,
) -> ZResult<TabInfo> {
    let to = match (p.url, p.action) {
        (Some(url), None) => Navigation::Url(url),
        (None, Some(NavigateAction::Back)) => Navigation::Back,
        (None, Some(NavigateAction::Forward)) => Navigation::Forward,
        (None, Some(NavigateAction::Reload)) => Navigation::Reload,
        (None, Some(NavigateAction::Stop)) => Navigation::Stop,
        _ => return Err(ZwiimError::invalid_request("Give either url or action")),
    };
    Ok(Json(service(&state).await?.navigate(&p.tab_id, to).await?))
}

/// `browser.engine.reset` → `{engine}`: clear a failure (`crash_loop` too)
/// so the next tab starts the engine again.
pub async fn engine_reset(Extension(state): Extension<Arc<AppState>>) -> ZResult<Value> {
    let browser = service(&state).await?;
    browser.engine().reset().await;
    let engine = EngineView::from(&*browser.engine().state().borrow());
    Ok(Json(json!({ "engine": engine })))
}

/// One socket's `browser.subscribe`: a `browser.snapshot` frame, then each
/// change as `browser.tab.state`, `browser.tab.removed` or
/// `browser.engine.state`. Events carry a tab's whole state, so one that
/// arrives after a snapshot already holding it changes nothing. A socket
/// that falls behind gets a fresh snapshot.
pub(super) struct BrowserFeed {
    browser: Arc<BrowserService>,
    events: tokio::sync::broadcast::Receiver<BrowserEvent>,
    engine: tokio::sync::watch::Receiver<EngineState>,
    sent_engine: EngineView,
}

impl BrowserFeed {
    /// Subscribe, then read the snapshot, so no change between is lost.
    pub(super) async fn subscribe(state: &AppState) -> Result<(Self, Value), ZwiimError> {
        let browser = service(state).await?;
        let mut feed = BrowserFeed {
            events: browser.subscribe(),
            engine: browser.engine().state(),
            sent_engine: EngineView::Stopped,
            browser,
        };
        let snapshot = feed.snapshot().await;
        Ok((feed, snapshot))
    }

    async fn snapshot(&mut self) -> Value {
        self.sent_engine = EngineView::from(&*self.engine.borrow_and_update());
        json!({
            "type": "browser.snapshot",
            "tabs": self.browser.list(None).await,
            "engine": self.sent_engine,
        })
    }

    /// The next frame to send; `None` once the browser is gone.
    pub(super) async fn next(&mut self) -> Option<Value> {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            tokio::select! {
                event = self.events.recv() => return match event {
                    Ok(BrowserEvent::Tab(tab)) => {
                        Some(json!({ "type": "browser.tab.state", "tab": tab }))
                    }
                    Ok(BrowserEvent::Removed { tab_id, owner }) => Some(json!({
                        "type": "browser.tab.removed",
                        "tab_id": tab_id,
                        "owner": owner,
                    })),
                    Err(RecvError::Lagged(_)) => Some(self.snapshot().await),
                    Err(RecvError::Closed) => None,
                },
                changed = self.engine.changed() => {
                    changed.ok()?;
                    let view = EngineView::from(&*self.engine.borrow_and_update());
                    // A restart at the same version looks the same to clients.
                    if view != self.sent_engine {
                        self.sent_engine = view.clone();
                        return Some(json!({ "type": "browser.engine.state", "engine": view }));
                    }
                }
            }
        }
    }
}
