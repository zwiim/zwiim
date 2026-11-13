//! `BrowserService`: the Host browser's tabs (design 6). Every tab belongs to
//! one owner, and every owner gets its own `BrowserContext` in the one engine,
//! so sessions never see each other's cookies or storage.
//!
//! The service is the only holder of tab state. Clients and agent tools go
//! through it; nothing else opens CDP sessions on a tab.
//!
//! - Tabs are persisted in `browser_tabs` and come back **discarded** after a
//!   Host restart or an idle stop of the engine: listed, not loaded, and
//!   reloaded from their URL when next used.
//! - After an engine crash the manager starts a new engine (a new
//!   generation); tabs that were loaded are opened again at their URL.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use sea_orm::DatabaseConnection;
use serde::Serialize;
use serde_json::json;
use tokio::sync::{broadcast, watch, Mutex};

use super::engine::cdp::{Cdp, Event};
use super::engine::manager::{EngineError, EngineManager, EngineState};
use super::snapshot::{self, PersonalSnapshot};
use super::store::{self, TabOwner, TabRecord};
use super::ui::{self, UiAnswer, UiRequest};

const EVENT_BUFFER: usize = 256;
const DEFAULT_WIDTH: u32 = 1280;
const DEFAULT_HEIGHT: u32 = 800;
/// How long after a personal page loads its cookies are saved.
const SAVE_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

/// A tab as clients see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TabInfo {
    #[serde(rename = "tab_id")]
    pub id: String,
    pub owner: TabOwner,
    pub url: String,
    pub title: String,
    pub position: i32,
    pub loading: bool,
    /// Not loaded in the engine; it reloads from `url` when next used.
    pub discarded: bool,
    /// Its renderer died; reloading brings it back.
    pub crashed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserEvent {
    Tab(TabInfo),
    Removed { tab_id: String, owner: TabOwner },
    /// A tab waits on a person (design 9).
    Ui(UiRequest),
    /// That request was answered, timed out, or its tab went.
    UiDone { request_id: String, tab_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Navigation {
    Url(String),
    Back,
    Forward,
    Reload,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct BrowserError {
    /// `tab_not_found`, `unsupported_url`, `engine_error`, or one of the
    /// engine's setup codes (`download_offline`, `crash_loop`, …).
    pub code: &'static str,
    pub message: String,
}

impl From<EngineError> for BrowserError {
    fn from(e: EngineError) -> Self {
        BrowserError {
            code: e.code,
            message: e.message,
        }
    }
}

impl From<super::engine::cdp::CdpError> for BrowserError {
    fn from(e: super::engine::cdp::CdpError) -> Self {
        BrowserError {
            code: "engine_error",
            message: e.to_string(),
        }
    }
}

fn not_found(tab_id: &str) -> BrowserError {
    BrowserError {
        code: "tab_not_found",
        message: format!("no browser tab {tab_id}"),
    }
}

/// A tab loaded in the current engine.
#[derive(Debug, Clone)]
struct Live {
    target_id: String,
    session_id: String,
}

struct Tab {
    info: TabInfo,
    live: Option<Live>,
}

struct Inner {
    tabs: HashMap<String, Tab>,
    /// The engine generation `contexts` and every `Live` belong to.
    generation: u64,
    cdp: Option<Cdp>,
    contexts: HashMap<TabOwner, String>,
    user_agent: Option<String>,
    /// The personal context of this generation holds the snapshot's
    /// cookies. Until it does, nothing is saved over the snapshot.
    personal_restored: bool,
    /// A personal snapshot save is scheduled.
    save_pending: bool,
    /// Requests waiting on a person, by `request_id`.
    ui: HashMap<String, UiRequest>,
    next_ui: u64,
}

pub struct BrowserService {
    me: Weak<BrowserService>,
    engine: Arc<EngineManager>,
    snapshot: Option<Arc<PersonalSnapshot>>,
    db: DatabaseConnection,
    inner: Mutex<Inner>,
    events: broadcast::Sender<BrowserEvent>,
}

impl BrowserService {
    /// Load the persisted tabs (all discarded) and follow the engine.
    pub async fn start(
        engine: Arc<EngineManager>,
        db: DatabaseConnection,
        snapshot: Option<PersonalSnapshot>,
    ) -> Result<Arc<Self>, sea_orm::DbErr> {
        let tabs = store::load(&db)
            .await?
            .into_iter()
            .map(|record| {
                let info = TabInfo {
                    discarded: true,
                    ..info_of(record)
                };
                (info.id.clone(), Tab { info, live: None })
            })
            .collect();
        let state = engine.state();
        let service = Arc::new_cyclic(|me| BrowserService {
            me: me.clone(),
            engine,
            snapshot: snapshot.map(Arc::new),
            db,
            inner: Mutex::new(Inner {
                tabs,
                generation: 0,
                cdp: None,
                contexts: HashMap::new(),
                user_agent: None,
                personal_restored: false,
                save_pending: false,
                ui: HashMap::new(),
                next_ui: 0,
            }),
            events: broadcast::channel(EVENT_BUFFER).0,
        });
        tokio::spawn(follow_engine(Arc::downgrade(&service), state));
        Ok(service)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BrowserEvent> {
        self.events.subscribe()
    }

    pub fn engine(&self) -> &Arc<EngineManager> {
        &self.engine
    }

    /// Tabs in each owner's order; only `session_id`'s when given.
    pub async fn list(&self, session_id: Option<&str>) -> Vec<TabInfo> {
        let inner = self.inner.lock().await;
        let mut tabs: Vec<TabInfo> = inner
            .tabs
            .values()
            .map(|t| t.info.clone())
            .filter(|t| match (session_id, &t.owner) {
                (None, _) => true,
                (Some(id), TabOwner::Session { session_id }) => id == session_id,
                (Some(_), TabOwner::Personal) => false,
            })
            .collect();
        tabs.sort_by(|a, b| {
            (owner_key(&a.owner), a.position).cmp(&(owner_key(&b.owner), b.position))
        });
        tabs
    }

    pub async fn tab(&self, tab_id: &str) -> Option<TabInfo> {
        self.inner
            .lock()
            .await
            .tabs
            .get(tab_id)
            .map(|t| t.info.clone())
    }

    /// Open a tab for `owner`, at `url` or blank.
    pub async fn open(&self, owner: TabOwner, url: Option<&str>) -> Result<TabInfo, BrowserError> {
        let url = url.map(check_url).transpose()?;
        let mut inner = self.inner.lock().await;
        let cdp = self.cdp_locked(&mut inner).await?;
        let live = self.create_target(&cdp, &mut inner, &owner).await?;
        if let Some(url) = &url {
            cdp.call(
                "Page.navigate",
                json!({ "url": url }),
                Some(&live.session_id),
            )
            .await?;
        }
        let url = url.unwrap_or_else(|| "about:blank".into());
        Ok(self.add_tab(&mut inner, owner, url, live).await)
    }

    /// Record a new loaded tab and tell clients.
    async fn add_tab(
        &self,
        inner: &mut Inner,
        owner: TabOwner,
        url: String,
        live: Live,
    ) -> TabInfo {
        let position = store::next_position(&self.db, &owner).await.unwrap_or(0);
        let info = TabInfo {
            id: uuid::Uuid::new_v4().to_string(),
            owner,
            url,
            title: String::new(),
            position,
            loading: false,
            discarded: false,
            crashed: false,
        };
        if let Err(e) = store::insert(&self.db, &record_of(&info)).await {
            tracing::warn!(error = %e, "could not save a browser tab");
        }
        inner.tabs.insert(
            info.id.clone(),
            Tab {
                info: info.clone(),
                live: Some(live),
            },
        );
        self.emit(BrowserEvent::Tab(info.clone()));
        info
    }

    pub async fn close(&self, tab_id: &str) -> Result<(), BrowserError> {
        let mut inner = self.inner.lock().await;
        let tab = inner.tabs.remove(tab_id).ok_or_else(|| not_found(tab_id))?;
        self.drop_ui_locked(&mut inner, Some(tab_id));
        if let (Some(live), Some(cdp)) = (&tab.live, &inner.cdp) {
            let _ = cdp
                .call(
                    "Target.closeTarget",
                    json!({ "targetId": live.target_id }),
                    None,
                )
                .await;
        }
        drop(inner);
        if let Err(e) = store::remove(&self.db, tab_id).await {
            tracing::warn!(error = %e, "could not remove a browser tab");
        }
        if tab.info.owner == TabOwner::Personal {
            self.save_personal().await;
        }
        self.emit(BrowserEvent::Removed {
            tab_id: tab_id.to_string(),
            owner: tab.info.owner,
        });
        Ok(())
    }

    /// Close every tab of a session that has ended, and drop its context.
    pub async fn close_session(&self, session_id: &str) {
        let ids: Vec<String> = self
            .list(Some(session_id))
            .await
            .into_iter()
            .map(|t| t.id)
            .collect();
        for id in ids {
            let _ = self.close(&id).await;
        }
        let mut inner = self.inner.lock().await;
        let owner = TabOwner::Session {
            session_id: session_id.to_string(),
        };
        if let (Some(ctx), Some(cdp)) = (inner.contexts.remove(&owner), inner.cdp.clone()) {
            let _ = cdp
                .call(
                    "Target.disposeBrowserContext",
                    json!({ "browserContextId": ctx }),
                    None,
                )
                .await;
        }
    }

    pub async fn navigate(&self, tab_id: &str, to: Navigation) -> Result<TabInfo, BrowserError> {
        let target_url = match &to {
            Navigation::Url(url) => Some(check_url(url)?),
            _ => None,
        };
        let mut inner = self.inner.lock().await;
        let tab = inner.tabs.get(tab_id).ok_or_else(|| not_found(tab_id))?;
        // A discarded tab comes back at the address asked for, or its own.
        if tab.live.is_none() {
            let url = target_url.unwrap_or_else(|| tab.info.url.clone());
            if matches!(to, Navigation::Stop) {
                return Ok(tab.info.clone());
            }
            return self.revive_locked(&mut inner, tab_id, &url).await;
        }
        let cdp = self.cdp_locked(&mut inner).await?;
        let tab = inner
            .tabs
            .get_mut(tab_id)
            .ok_or_else(|| not_found(tab_id))?;
        let session = tab
            .live
            .as_ref()
            .map(|l| l.session_id.clone())
            .unwrap_or_default();
        let s = Some(session.as_str());
        match to {
            Navigation::Url(_) => {
                let url = target_url.unwrap_or_default();
                cdp.call("Page.navigate", json!({ "url": url }), s).await?;
            }
            Navigation::Reload => {
                tab.info.crashed = false;
                cdp.call("Page.reload", json!({}), s).await?;
            }
            Navigation::Stop => {
                cdp.call("Page.stopLoading", json!({}), s).await?;
            }
            Navigation::Back | Navigation::Forward => {
                let history = cdp.call("Page.getNavigationHistory", json!({}), s).await?;
                let current = history["currentIndex"].as_i64().unwrap_or(0);
                let step = if to == Navigation::Back { -1 } else { 1 };
                let entry = usize::try_from(current + step)
                    .ok()
                    .and_then(|i| history["entries"].get(i).cloned());
                if let Some(entry) = entry {
                    cdp.call(
                        "Page.navigateToHistoryEntry",
                        json!({ "entryId": entry["id"] }),
                        s,
                    )
                    .await?;
                }
            }
        }
        let tab = inner.tabs.get(tab_id).ok_or_else(|| not_found(tab_id))?;
        Ok(tab.info.clone())
    }

    /// Load a discarded tab again. A loaded tab is returned as is.
    pub async fn activate(&self, tab_id: &str) -> Result<TabInfo, BrowserError> {
        let mut inner = self.inner.lock().await;
        let tab = inner.tabs.get(tab_id).ok_or_else(|| not_found(tab_id))?;
        if tab.live.is_some() {
            return Ok(tab.info.clone());
        }
        let url = tab.info.url.clone();
        self.revive_locked(&mut inner, tab_id, &url).await
    }

    /// The tab's CDP session, loading it first if it was discarded. For the
    /// layers above (stream, agent tools), which must still go through the
    /// service's own checks first.
    pub async fn session(&self, tab_id: &str) -> Result<(Cdp, String), BrowserError> {
        self.activate(tab_id).await?;
        let mut inner = self.inner.lock().await;
        let cdp = self.cdp_locked(&mut inner).await?;
        let live = inner
            .tabs
            .get(tab_id)
            .and_then(|t| t.live.clone())
            .ok_or_else(|| not_found(tab_id))?;
        Ok((cdp, live.session_id))
    }

    async fn revive_locked(
        &self,
        inner: &mut Inner,
        tab_id: &str,
        url: &str,
    ) -> Result<TabInfo, BrowserError> {
        let cdp = self.cdp_locked(inner).await?;
        let owner = inner.tabs[tab_id].info.owner.clone();
        let live = self.create_target(&cdp, inner, &owner).await?;
        if url != "about:blank" {
            cdp.call(
                "Page.navigate",
                json!({ "url": url }),
                Some(&live.session_id),
            )
            .await?;
        }
        let tab = inner
            .tabs
            .get_mut(tab_id)
            .ok_or_else(|| not_found(tab_id))?;
        tab.live = Some(live);
        tab.info.discarded = false;
        tab.info.crashed = false;
        tab.info.url = url.to_string();
        let info = tab.info.clone();
        if let Err(e) = store::set_discarded(&self.db, tab_id, false).await {
            tracing::warn!(error = %e, "could not save a browser tab");
        }
        self.emit(BrowserEvent::Tab(info.clone()));
        Ok(info)
    }

    /// The current engine's connection. A new generation means a new
    /// engine: contexts are gone and loaded tabs are opened again.
    async fn cdp_locked(&self, inner: &mut Inner) -> Result<Cdp, BrowserError> {
        let (cdp, generation) = self.engine.connect().await?;
        self.resync_locked(inner, cdp, generation).await
    }

    /// Adopt `cdp` of `generation`. Nothing to do if it is the one in use.
    async fn resync_locked(
        &self,
        inner: &mut Inner,
        cdp: Cdp,
        generation: u64,
    ) -> Result<Cdp, BrowserError> {
        if generation == inner.generation && inner.cdp.is_some() {
            return Ok(cdp);
        }
        inner.generation = generation;
        self.drop_ui_locked(inner, None);
        inner.contexts.clear();
        inner.personal_restored = false;
        inner.cdp = Some(cdp.clone());
        // Subscribe before discovery, so no target event is missed.
        let events = cdp.subscribe();
        cdp.call(
            "Target.setDiscoverTargets",
            json!({ "discover": true }),
            None,
        )
        .await?;
        let version = cdp.call("Browser.getVersion", json!({}), None).await?;
        inner.user_agent = version["userAgent"]
            .as_str()
            .map(|ua| ua.replace("HeadlessChrome", "Chrome"));
        tokio::spawn(pump(self.me.clone(), cdp.clone(), events, generation));

        let reopen: Vec<(String, String)> = inner
            .tabs
            .iter_mut()
            .filter_map(|(id, tab)| {
                let was_live = tab.live.take().is_some();
                was_live.then(|| (id.clone(), tab.info.url.clone()))
            })
            .collect();
        for (id, url) in reopen {
            let owner = inner.tabs[&id].info.owner.clone();
            match self.create_target(&cdp, inner, &owner).await {
                Ok(live) => {
                    if url != "about:blank" {
                        let _ = cdp
                            .call(
                                "Page.navigate",
                                json!({ "url": url }),
                                Some(&live.session_id),
                            )
                            .await;
                    }
                    if let Some(tab) = inner.tabs.get_mut(&id) {
                        tab.live = Some(live);
                        tab.info.crashed = false;
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "could not restore a browser tab");
                    self.discard_locked(inner, &id);
                }
            }
        }
        Ok(cdp)
    }

    async fn create_target(
        &self,
        cdp: &Cdp,
        inner: &mut Inner,
        owner: &TabOwner,
    ) -> Result<Live, BrowserError> {
        let context = match inner.contexts.get(owner) {
            Some(ctx) => ctx.clone(),
            None => {
                let created = cdp
                    .call(
                        "Target.createBrowserContext",
                        json!({ "disposeOnDetach": false }),
                        None,
                    )
                    .await?;
                let ctx = created["browserContextId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                inner.contexts.insert(owner.clone(), ctx.clone());
                if *owner == TabOwner::Personal {
                    inner.personal_restored = self.restore_personal(cdp, &ctx).await;
                }
                ctx
            }
        };
        let created = cdp
            .call(
                "Target.createTarget",
                json!({
                    "url": "about:blank",
                    "browserContextId": context,
                    "width": DEFAULT_WIDTH,
                    "height": DEFAULT_HEIGHT,
                }),
                None,
            )
            .await?;
        let target_id = created["targetId"].as_str().unwrap_or_default().to_string();
        self.attach(cdp, inner, target_id).await
    }

    /// Attach to a page target and set it up as a tab's.
    async fn attach(
        &self,
        cdp: &Cdp,
        inner: &Inner,
        target_id: String,
    ) -> Result<Live, BrowserError> {
        let attached = cdp
            .call(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
                None,
            )
            .await?;
        let session_id = attached["sessionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let s = Some(session_id.as_str());
        cdp.call("Page.enable", json!({}), s).await?;
        if let Some(ua) = &inner.user_agent {
            cdp.call(
                "Emulation.setUserAgentOverride",
                json!({ "userAgent": ua }),
                s,
            )
            .await?;
        }
        Ok(Live {
            target_id,
            session_id,
        })
    }

    fn discard_locked(&self, inner: &mut Inner, tab_id: &str) {
        self.drop_ui_locked(inner, Some(tab_id));
        let Some(tab) = inner.tabs.get_mut(tab_id) else {
            return;
        };
        tab.live = None;
        tab.info.discarded = true;
        tab.info.loading = false;
        let info = tab.info.clone();
        let db = self.db.clone();
        let id = tab_id.to_string();
        tokio::spawn(async move {
            let _ = store::set_discarded(&db, &id, true).await;
        });
        self.emit(BrowserEvent::Tab(info));
    }

    fn emit(&self, event: BrowserEvent) {
        let _ = self.events.send(event);
    }

    /// Apply one engine event to the tab it names.
    async fn on_event(&self, generation: u64, event: Event) {
        let mut inner = self.inner.lock().await;
        if inner.generation != generation {
            return;
        }
        let p = &event.params;
        let by_target = |inner: &Inner, target: &str| {
            inner
                .tabs
                .iter()
                .find(|(_, t)| t.live.as_ref().is_some_and(|l| l.target_id == target))
                .map(|(id, _)| id.clone())
        };
        let by_session = |inner: &Inner, session: &str| {
            inner
                .tabs
                .iter()
                .find(|(_, t)| t.live.as_ref().is_some_and(|l| l.session_id == session))
                .map(|(id, _)| id.clone())
        };
        match event.method.as_str() {
            "Target.targetInfoChanged" => {
                let info = &p["targetInfo"];
                let Some(id) = by_target(&inner, info["targetId"].as_str().unwrap_or_default())
                else {
                    return;
                };
                let tab = inner.tabs.get_mut(&id).expect("found above");
                let url = info["url"].as_str().unwrap_or_default().to_string();
                let title = info["title"].as_str().unwrap_or_default().to_string();
                if tab.info.url == url && tab.info.title == title {
                    return;
                }
                tab.info.url = url;
                tab.info.title = title;
                let info = tab.info.clone();
                let db = self.db.clone();
                tokio::spawn(async move {
                    let _ = store::update_page(&db, &info.id, &info.url, &info.title).await;
                });
                self.emit(BrowserEvent::Tab(tab.info.clone()));
            }
            "Page.frameStartedLoading" | "Page.frameStoppedLoading" => {
                let Some(session) = event.session_id.as_deref() else {
                    return;
                };
                let Some(id) = by_session(&inner, session) else {
                    return;
                };
                let tab = inner.tabs.get_mut(&id).expect("found above");
                // Only the main frame, whose id is the target's.
                let main = tab.live.as_ref().map(|l| l.target_id.as_str());
                if p["frameId"].as_str() != main {
                    return;
                }
                let loading = event.method == "Page.frameStartedLoading";
                if tab.info.loading != loading {
                    tab.info.loading = loading;
                    self.emit(BrowserEvent::Tab(tab.info.clone()));
                }
                if !loading {
                    self.refresh_info(&inner, &id, generation);
                }
            }
            // Chromium reports a new title only when asked: `targetInfoChanged`
            // fires when the URL commits, not when the title changes.
            "Page.domContentEventFired"
            | "Page.loadEventFired"
            | "Page.navigatedWithinDocument" => {
                let Some(id) = event
                    .session_id
                    .as_deref()
                    .and_then(|s| by_session(&inner, s))
                else {
                    return;
                };
                self.refresh_info(&inner, &id, generation);
                let personal = inner.tabs[&id].info.owner == TabOwner::Personal;
                if personal && event.method == "Page.loadEventFired" {
                    self.schedule_save(&mut inner);
                }
            }
            // A page opened a window (`window.open`, `target=_blank`, an
            // OAuth popup): it becomes a tab of the opener's owner, in the
            // same context, so `window.opener` keeps working.
            "Target.targetCreated" => {
                let info = &p["targetInfo"];
                let target_id = info["targetId"].as_str().unwrap_or_default().to_string();
                let Some(opener) = info["openerId"].as_str().and_then(|o| by_target(&inner, o))
                else {
                    return;
                };
                if info["type"] != "page" || by_target(&inner, &target_id).is_some() {
                    return;
                }
                let (Some(cdp), owner) =
                    (inner.cdp.clone(), inner.tabs[&opener].info.owner.clone())
                else {
                    return;
                };
                let url = info["url"].as_str().unwrap_or("about:blank").to_string();
                match self.attach(&cdp, &inner, target_id).await {
                    Ok(live) => {
                        self.add_tab(&mut inner, owner, url, live).await;
                    }
                    Err(e) => tracing::warn!(error = %e, "could not adopt a popup"),
                }
            }
            "Page.javascriptDialogOpening" => {
                let Some(id) = event
                    .session_id
                    .as_deref()
                    .and_then(|s| by_session(&inner, s))
                else {
                    return;
                };
                inner.next_ui += 1;
                let request = UiRequest {
                    request_id: format!("ui-{}", inner.next_ui),
                    tab_id: id.clone(),
                    owner: inner.tabs[&id].info.owner.clone(),
                    kind: ui::dialog_of(p),
                };
                inner
                    .ui
                    .insert(request.request_id.clone(), request.clone());
                let request_id = request.request_id.clone();
                self.emit(BrowserEvent::Ui(request));
                let me = self.me.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(ui::DIALOG_TIMEOUT).await;
                    if let Some(service) = me.upgrade() {
                        service.expire(&request_id).await;
                    }
                });
            }
            // Answered here, by the page going away, or by another client of
            // the engine: whatever is still asked of that tab is done.
            "Page.javascriptDialogClosed" => {
                let Some(id) = event
                    .session_id
                    .as_deref()
                    .and_then(|s| by_session(&inner, s))
                else {
                    return;
                };
                self.drop_ui_locked(&mut inner, Some(&id));
            }
            "Target.targetCrashed" => {
                let Some(id) = by_target(&inner, p["targetId"].as_str().unwrap_or_default()) else {
                    return;
                };
                self.drop_ui_locked(&mut inner, Some(&id));
                let tab = inner.tabs.get_mut(&id).expect("found above");
                tab.info.crashed = true;
                tab.info.loading = false;
                self.emit(BrowserEvent::Tab(tab.info.clone()));
            }
            "Target.targetDestroyed" => {
                // The page closed itself (`window.close()`); ours were
                // removed from `tabs` before their target went.
                let Some(id) = by_target(&inner, p["targetId"].as_str().unwrap_or_default()) else {
                    return;
                };
                self.drop_ui_locked(&mut inner, Some(&id));
                let tab = inner.tabs.remove(&id).expect("found above");
                let db = self.db.clone();
                let tab_id = id.clone();
                tokio::spawn(async move {
                    let _ = store::remove(&db, &tab_id).await;
                });
                self.emit(BrowserEvent::Removed {
                    tab_id: id,
                    owner: tab.info.owner,
                });
            }
            _ => {}
        }
    }

    /// Every request waiting on a person, oldest first.
    pub async fn ui_requests(&self) -> Vec<UiRequest> {
        let inner = self.inner.lock().await;
        let mut requests: Vec<UiRequest> = inner.ui.values().cloned().collect();
        requests.sort_by_key(|r| ui_order(&r.request_id));
        requests
    }

    /// Give a person's answer to a request. The first answer wins; a later
    /// one finds no request.
    pub async fn answer(&self, request_id: &str, answer: &UiAnswer) -> Result<(), BrowserError> {
        let mut inner = self.inner.lock().await;
        let request = inner
            .ui
            .get(request_id)
            .cloned()
            .ok_or_else(|| BrowserError {
                code: "request_not_found",
                message: "That request was already answered or has expired".into(),
            })?;
        let (method, params) = ui::answer_call(&request.kind, answer)?;
        let session = inner
            .tabs
            .get(&request.tab_id)
            .and_then(|t| t.live.as_ref())
            .map(|l| l.session_id.clone());
        // A dialog that already closed is no error: the request is done.
        let delivered = match (inner.cdp.clone(), session) {
            (Some(cdp), Some(session)) => cdp.call(method, params, Some(&session)).await.map(|_| ()),
            _ => Ok(()),
        };
        self.drop_ui_locked(&mut inner, Some(&request.tab_id));
        if let Err(e) = delivered {
            tracing::debug!(error = %e, "a browser dialog closed before its answer");
        }
        Ok(())
    }

    /// Dismiss a request nobody answered in time.
    async fn expire(&self, request_id: &str) {
        let mut inner = self.inner.lock().await;
        let Some(request) = inner.ui.get(request_id).cloned() else {
            return;
        };
        tracing::warn!(tab_id = %request.tab_id, "dismissed a browser dialog nobody answered");
        let session = inner
            .tabs
            .get(&request.tab_id)
            .and_then(|t| t.live.as_ref())
            .map(|l| l.session_id.clone());
        if let (Some(cdp), Some(session)) = (inner.cdp.clone(), session) {
            let (method, params) = ui::dismiss_call();
            let _ = cdp.call(method, params, Some(&session)).await;
        }
        self.drop_ui_locked(&mut inner, Some(&request.tab_id));
    }

    /// End the requests of `tab_id`, or every request.
    fn drop_ui_locked(&self, inner: &mut Inner, tab_id: Option<&str>) {
        let done: Vec<UiRequest> = inner
            .ui
            .values()
            .filter(|r| tab_id.is_none_or(|t| r.tab_id == t))
            .cloned()
            .collect();
        for request in done {
            inner.ui.remove(&request.request_id);
            self.emit(BrowserEvent::UiDone {
                request_id: request.request_id,
                tab_id: request.tab_id,
            });
        }
    }

    /// Load the snapshot's cookies into the new personal context `ctx`.
    /// Returns whether the context now holds them (or there were none).
    async fn restore_personal(&self, cdp: &Cdp, ctx: &str) -> bool {
        let Some(snapshot) = self.snapshot.clone() else {
            return false;
        };
        let loaded = tokio::task::spawn_blocking(move || snapshot.load()).await;
        let contents = match loaded {
            Ok(Ok(contents)) => contents,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "could not read the browser's personal sign-ins");
                return false;
            }
            Err(_) => return false,
        };
        if contents.cookies.is_empty() {
            return true;
        }
        let set = cdp
            .call(
                "Storage.setCookies",
                json!({ "cookies": contents.cookies, "browserContextId": ctx }),
                None,
            )
            .await;
        if let Err(e) = &set {
            tracing::warn!(error = %e, "could not restore the browser's personal sign-ins");
        }
        set.is_ok()
    }

    /// Save the personal context's cookies to the snapshot now. Does nothing
    /// before that context exists, or when its restore failed: an empty or
    /// partial jar must never replace the saved one.
    pub async fn save_personal(&self) {
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        let (cdp, ctx) = {
            let inner = self.inner.lock().await;
            match (&inner.cdp, inner.contexts.get(&TabOwner::Personal)) {
                (Some(cdp), Some(ctx)) if inner.personal_restored => (cdp.clone(), ctx.clone()),
                _ => return,
            }
        };
        let got = match cdp
            .call(
                "Storage.getCookies",
                json!({ "browserContextId": ctx }),
                None,
            )
            .await
        {
            Ok(got) => got,
            Err(e) => {
                tracing::warn!(error = %e, "could not read the browser's personal cookies");
                return;
            }
        };
        let cookies = got["cookies"]
            .as_array()
            .map(|all| all.iter().filter_map(snapshot::cookie_param).collect())
            .unwrap_or_default();
        let contents = snapshot::Contents { cookies };
        match tokio::task::spawn_blocking(move || snapshot.save(&contents)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "could not save the browser's personal sign-ins")
            }
            Err(_) => {}
        }
    }

    /// Save the personal snapshot a little after page loads settle.
    fn schedule_save(&self, inner: &mut Inner) {
        if inner.save_pending || self.snapshot.is_none() {
            return;
        }
        inner.save_pending = true;
        let me = self.me.clone();
        tokio::spawn(async move {
            tokio::time::sleep(SAVE_DELAY).await;
            let Some(service) = me.upgrade() else {
                return;
            };
            service.inner.lock().await.save_pending = false;
            service.save_personal().await;
        });
    }

    /// Stop the engine on purpose, saving the personal sign-ins first.
    pub async fn stop_engine(&self) {
        self.save_personal().await;
        self.engine.stop().await;
    }

    /// Read the tab's URL and title from the engine, then apply them as a
    /// `targetInfoChanged` would.
    fn refresh_info(&self, inner: &Inner, tab_id: &str, generation: u64) {
        let (Some(cdp), Some(live)) = (
            inner.cdp.clone(),
            inner.tabs.get(tab_id).and_then(|t| t.live.clone()),
        ) else {
            return;
        };
        let me = self.me.clone();
        tokio::spawn(async move {
            let Ok(got) = cdp
                .call(
                    "Target.getTargetInfo",
                    json!({ "targetId": live.target_id }),
                    None,
                )
                .await
            else {
                return;
            };
            let Some(service) = me.upgrade() else {
                return;
            };
            let event = Event {
                session_id: None,
                method: "Target.targetInfoChanged".into(),
                params: Arc::new(got),
            };
            service.on_event(generation, event).await;
        });
    }

    /// The engine stopped on purpose: every loaded tab is now discarded.
    async fn on_stopped(&self) {
        let mut inner = self.inner.lock().await;
        inner.cdp = None;
        inner.contexts.clear();
        inner.personal_restored = false;
        let live: Vec<String> = inner
            .tabs
            .iter()
            .filter(|(_, t)| t.live.is_some())
            .map(|(id, _)| id.clone())
            .collect();
        for id in live {
            self.discard_locked(&mut inner, &id);
        }
    }

    /// A new engine generation is running: bring loaded tabs back now
    /// rather than on their next use.
    async fn on_restarted(&self) {
        let mut inner = self.inner.lock().await;
        // Never starts an engine: only one the manager already restarted.
        let Some((cdp, generation)) = self.engine.running().await else {
            return;
        };
        if inner.cdp.is_none() || generation == inner.generation {
            return;
        }
        if let Err(e) = self.resync_locked(&mut inner, cdp, generation).await {
            tracing::warn!(error = %e, "could not restore browser tabs after an engine restart");
        }
    }
}

fn ui_order(request_id: &str) -> u64 {
    request_id
        .strip_prefix("ui-")
        .and_then(|n| n.parse().ok())
        .unwrap_or(u64::MAX)
}

/// Route engine events to the service until the engine goes away.
async fn pump(
    service: Weak<BrowserService>,
    cdp: Cdp,
    mut events: broadcast::Receiver<Event>,
    generation: u64,
) {
    loop {
        let event = tokio::select! {
            event = events.recv() => event,
            _ = cdp.closed() => return,
        };
        let event = match event {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(missed = n, "browser tab events fell behind");
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => return,
        };
        let Some(service) = service.upgrade() else {
            return;
        };
        service.on_event(generation, event).await;
    }
}

/// React to the engine stopping or coming back in a new generation.
/// `state` is subscribed before the service can start the engine, so no
/// generation is missed. A generation the service started itself is already
/// current there, and `on_restarted` leaves it alone.
async fn follow_engine(service: Weak<BrowserService>, mut state: watch::Receiver<EngineState>) {
    let mut last = 0;
    while state.changed().await.is_ok() {
        let now = state.borrow_and_update().clone();
        let Some(service) = service.upgrade() else {
            return;
        };
        match now {
            EngineState::Stopped => service.on_stopped().await,
            EngineState::Running { generation, .. } if generation != last => {
                last = generation;
                service.on_restarted().await;
            }
            _ => {}
        }
    }
}

fn info_of(record: TabRecord) -> TabInfo {
    TabInfo {
        id: record.id,
        owner: record.owner,
        url: record.url,
        title: record.title,
        position: record.position,
        loading: false,
        discarded: record.discarded,
        crashed: false,
    }
}

fn record_of(info: &TabInfo) -> TabRecord {
    TabRecord {
        id: info.id.clone(),
        owner: info.owner.clone(),
        url: info.url.clone(),
        title: info.title.clone(),
        position: info.position,
        discarded: info.discarded,
    }
}

/// Personal tabs first, then sessions by id.
fn owner_key(owner: &TabOwner) -> (u8, &str) {
    match owner {
        TabOwner::Personal => (0, ""),
        TabOwner::Session { session_id } => (1, session_id.as_str()),
    }
}

/// Addresses a tab may be sent to. Site policy (design 13.2) comes on top.
fn check_url(url: &str) -> Result<String, BrowserError> {
    let unsupported = || BrowserError {
        code: "unsupported_url",
        message: format!("the browser cannot open {url}"),
    };
    if url == "about:blank" {
        return Ok(url.to_string());
    }
    let parsed = url::Url::parse(url).map_err(|_| unsupported())?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed.to_string()),
        _ => Err(unsupported()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::engine::fake::FakeEngine;
    use super::*;
    use crate::db::test_helpers::fresh_in_memory_db;

    fn session(id: &str) -> TabOwner {
        TabOwner::Session {
            session_id: id.into(),
        }
    }

    async fn service(fake: &FakeEngine, db: &DatabaseConnection) -> Arc<BrowserService> {
        BrowserService::start(EngineManager::new(fake.start_fn()), db.clone(), None)
            .await
            .unwrap()
    }

    /// Wait until `f` holds for the service's tab `id`.
    async fn until(svc: &BrowserService, id: &str, f: impl Fn(&TabInfo) -> bool) -> TabInfo {
        let reached = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(tab) = svc.tab(id).await.filter(|t| f(t)) {
                    return tab;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        match reached {
            Ok(tab) => tab,
            Err(_) => panic!("the tab did not reach the state: {:?}", svc.tab(id).await),
        }
    }

    #[tokio::test]
    async fn each_owner_gets_its_own_context_and_tabs_are_saved() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        let mut events = svc.subscribe();

        let a = svc
            .open(session("s1"), Some("http://localhost:5173/"))
            .await
            .unwrap();
        let b = svc.open(session("s1"), None).await.unwrap();
        let c = svc.open(session("s2"), None).await.unwrap();
        let p = svc.open(TabOwner::Personal, None).await.unwrap();
        assert_eq!(
            (a.position, b.position, c.position, p.position),
            (0, 1, 0, 0)
        );
        assert_eq!(a.url, "http://localhost:5173/");

        let contexts: Vec<String> = fake
            .calls("Target.createTarget")
            .iter()
            .map(|c| c["browserContextId"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(contexts[0], contexts[1], "one session shares its context");
        assert_ne!(contexts[0], contexts[2], "sessions do not share");
        assert_ne!(contexts[2], contexts[3], "personal tabs have their own");
        assert_eq!(fake.calls("Target.createBrowserContext").len(), 3);

        let ua = fake.calls("Emulation.setUserAgentOverride");
        assert_eq!(ua.len(), 4);
        let ua = ua[0]["userAgent"].as_str().unwrap();
        assert!(
            ua.contains("Chrome/157") && !ua.contains("Headless"),
            "{ua}"
        );

        let ids: Vec<String> = svc
            .list(Some("s1"))
            .await
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![a.id.clone(), b.id.clone()]);
        assert_eq!(svc.list(None).await[0].id, p.id, "personal tabs first");
        assert_eq!(store::load(&db.conn).await.unwrap().len(), 4);
        assert_eq!(events.recv().await.unwrap(), BrowserEvent::Tab(a.clone()));
    }

    #[tokio::test]
    async fn page_changes_reach_the_tab_and_the_store() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        let tab = svc.open(session("s1"), None).await.unwrap();

        svc.navigate(&tab.id, Navigation::Url("https://example.com/".into()))
            .await
            .unwrap();
        let target = fake.target_of_url("https://example.com/").unwrap();
        fake.set_title(&target, "Example Domain");
        until(&svc, &tab.id, |t| t.title == "Example Domain").await;
        assert_eq!(svc.tab(&tab.id).await.unwrap().url, "https://example.com/");

        tokio::time::timeout(Duration::from_secs(5), async {
            while store::load(&db.conn).await.unwrap()[0].title != "Example Domain" {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();

        svc.navigate(&tab.id, Navigation::Url("https://example.org/".into()))
            .await
            .unwrap();
        until(&svc, &tab.id, |t| t.url == "https://example.org/").await;
        svc.navigate(&tab.id, Navigation::Back).await.unwrap();
        until(&svc, &tab.id, |t| t.url == "https://example.com/").await;
        svc.navigate(&tab.id, Navigation::Forward).await.unwrap();
        until(&svc, &tab.id, |t| t.url == "https://example.org/").await;
    }

    #[tokio::test]
    async fn closing_removes_the_target_the_record_and_tells_clients() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        let tab = svc.open(session("s1"), None).await.unwrap();
        let mut events = svc.subscribe();

        svc.close(&tab.id).await.unwrap();
        assert!(fake.lock().targets.is_empty());
        assert!(store::load(&db.conn).await.unwrap().is_empty());
        assert_eq!(
            events.recv().await.unwrap(),
            BrowserEvent::Removed {
                tab_id: tab.id.clone(),
                owner: session("s1")
            }
        );
        assert_eq!(svc.close(&tab.id).await.unwrap_err().code, "tab_not_found");
    }

    /// Open a dialog in the page at `url`; returns the request the clients see.
    async fn open_dialog(
        fake: &FakeEngine,
        events: &mut broadcast::Receiver<BrowserEvent>,
        url: &str,
        params: serde_json::Value,
    ) -> UiRequest {
        let target = fake.target_of_url(url).unwrap();
        let session = fake.lock().targets[&target].session.clone().unwrap();
        fake.emit("Page.javascriptDialogOpening", params, Some(&session));
        loop {
            if let BrowserEvent::Ui(request) = events.recv().await.unwrap() {
                return request;
            }
        }
    }

    async fn ui_done(events: &mut broadcast::Receiver<BrowserEvent>) -> String {
        loop {
            if let BrowserEvent::UiDone { request_id, .. } = events.recv().await.unwrap() {
                return request_id;
            }
        }
    }

    #[tokio::test]
    async fn a_dialog_waits_for_the_first_answer() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        let tab = svc
            .open(session("s1"), Some("https://example.com/"))
            .await
            .unwrap();
        let mut events = svc.subscribe();
        let request = open_dialog(
            &fake,
            &mut events,
            "https://example.com/",
            json!({"type": "prompt", "message": "Name?", "defaultPrompt": "Ann",
                   "url": "https://example.com/"}),
        )
        .await;
        assert_eq!((request.tab_id.as_str(), &request.owner), (tab.id.as_str(), &session("s1")));
        assert_eq!(svc.ui_requests().await, vec![request.clone()]);

        let answer = UiAnswer::Dialog {
            accept: true,
            text: Some("Bo".into()),
        };
        svc.answer(&request.request_id, &answer).await.unwrap();
        assert_eq!(
            fake.calls("Page.handleJavaScriptDialog"),
            vec![json!({"accept": true, "promptText": "Bo"})]
        );
        assert_eq!(ui_done(&mut events).await, request.request_id);
        assert!(svc.ui_requests().await.is_empty());
        assert_eq!(
            svc.answer(&request.request_id, &answer).await.unwrap_err().code,
            "request_not_found"
        );
    }

    #[tokio::test]
    async fn a_dialog_ends_with_its_tab_or_unanswered() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        let a = svc
            .open(session("s1"), Some("https://a.example/"))
            .await
            .unwrap();
        svc.open(session("s1"), Some("https://b.example/"))
            .await
            .unwrap();
        let mut events = svc.subscribe();
        let alert = json!({"type": "alert", "message": "Hi", "url": ""});

        let first = open_dialog(&fake, &mut events, "https://a.example/", alert.clone()).await;
        svc.close(&a.id).await.unwrap();
        assert_eq!(ui_done(&mut events).await, first.request_id);

        let second = open_dialog(&fake, &mut events, "https://b.example/", alert).await;
        svc.expire(&second.request_id).await;
        assert_eq!(fake.calls("Page.handleJavaScriptDialog"), vec![json!({"accept": false})]);
        assert_eq!(ui_done(&mut events).await, second.request_id);
        assert!(svc.ui_requests().await.is_empty());
    }

    #[tokio::test]
    async fn a_page_that_closes_itself_is_removed() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        let tab = svc
            .open(session("s1"), Some("https://example.com/"))
            .await
            .unwrap();
        let target = fake.target_of_url("https://example.com/").unwrap();
        fake.lock().targets.remove(&target);
        fake.emit(
            "Target.targetDestroyed",
            json!({ "targetId": target }),
            None,
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while svc.tab(&tab.id).await.is_some() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn an_ended_session_loses_its_tabs_and_context() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        svc.open(session("s1"), None).await.unwrap();
        svc.open(session("s1"), None).await.unwrap();
        let other = svc.open(session("s2"), None).await.unwrap();

        svc.close_session("s1").await;
        assert_eq!(svc.list(None).await, vec![other]);
        assert_eq!(fake.calls("Target.disposeBrowserContext").len(), 1);
        assert_eq!(fake.lock().contexts.len(), 1);
    }

    #[tokio::test]
    async fn after_a_host_restart_tabs_are_discarded_until_used() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let tab = {
            let svc = service(&fake, &db.conn).await;
            svc.open(session("s1"), Some("https://example.com/"))
                .await
                .unwrap()
        };

        let fresh = FakeEngine::default();
        let svc = service(&fresh, &db.conn).await;
        let listed = svc.list(None).await;
        assert_eq!(listed.len(), 1);
        assert!(listed[0].discarded);
        assert_eq!(fresh.lock().starts, 0, "listing does not start the engine");

        let back = svc.activate(&tab.id).await.unwrap();
        assert!(!back.discarded);
        assert_eq!(fresh.lock().starts, 1);
        assert!(fresh.target_of_url("https://example.com/").is_some());
        assert!(!store::load(&db.conn).await.unwrap()[0].discarded);
    }

    // Real time: the database pool's timeouts fire at once under paused time.
    // The restart waits 1 s.
    #[tokio::test]
    async fn after_an_engine_crash_loaded_tabs_open_again() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        let tab = svc
            .open(session("s1"), Some("https://example.com/"))
            .await
            .unwrap();

        fake.crash();
        tokio::time::timeout(Duration::from_secs(10), async {
            while fake.target_of_url("https://example.com/").is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the tab was opened again in the new engine");
        assert_eq!(fake.lock().starts, 2);
        let restored = svc.tab(&tab.id).await.unwrap();
        assert!(!restored.discarded);
        let (_, session_id) = svc.session(&tab.id).await.unwrap();
        let target = fake.target_of_url("https://example.com/").unwrap();
        assert_eq!(
            fake.lock().targets[&target].session.as_deref(),
            Some(session_id.as_str())
        );
    }

    #[tokio::test]
    async fn stopping_the_engine_discards_loaded_tabs() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        let tab = svc.open(session("s1"), None).await.unwrap();

        svc.engine().stop().await;
        until(&svc, &tab.id, |t| t.discarded).await;
    }

    #[tokio::test]
    async fn a_popup_becomes_a_tab_of_the_openers_owner() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        let opener = svc
            .open(session("s1"), Some("https://app.example/"))
            .await
            .unwrap();
        let opener_target = fake.target_of_url("https://app.example/").unwrap();

        let popup = fake.open_popup(&opener_target, "https://auth.example/login");
        let tabs = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let tabs = svc.list(Some("s1")).await;
                if tabs.len() == 2 {
                    return tabs;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(tabs[0].id, opener.id);
        assert_eq!(tabs[1].url, "https://auth.example/login");
        assert_eq!(tabs[1].position, 1);
        assert!(fake.lock().targets[&popup].session.is_some(), "attached");

        // A target whose opener is not one of our tabs is left alone.
        fake.emit(
            "Target.targetCreated",
            json!({ "targetInfo": { "targetId": "stray", "type": "page", "url": "https://x.example/", "openerId": "not-ours" } }),
            None,
        );
        let other = svc.open(session("s2"), None).await.unwrap();
        assert_eq!(svc.list(Some("s2")).await, vec![other]);
        assert_eq!(svc.list(Some("s1")).await.len(), 2);
    }

    fn personal_ctx(fake: &FakeEngine) -> String {
        let calls = fake.calls("Target.createTarget");
        calls.last().unwrap()["browserContextId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn personal_sign_ins_come_back_in_a_new_engine_and_only_there() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("personal.snapshot");
        let db = fresh_in_memory_db().await;
        let cookie = json!({ "name": "sid", "value": "secret", "domain": "example.com", "path": "/", "session": true });

        let fake = FakeEngine::default();
        let svc = BrowserService::start(
            EngineManager::new(fake.start_fn()),
            db.conn.clone(),
            Some(PersonalSnapshot::with_key(path.clone(), [3; 32])),
        )
        .await
        .unwrap();
        svc.open(TabOwner::Personal, None).await.unwrap();
        let ctx = personal_ctx(&fake);
        fake.lock().cookies.insert(ctx, vec![cookie.clone()]);
        svc.stop_engine().await;
        assert!(path.is_file());

        let fresh = FakeEngine::default();
        let svc = BrowserService::start(
            EngineManager::new(fresh.start_fn()),
            db.conn.clone(),
            Some(PersonalSnapshot::with_key(path.clone(), [3; 32])),
        )
        .await
        .unwrap();
        svc.open(session("s1"), None).await.unwrap();
        assert!(
            fresh.calls("Storage.setCookies").is_empty(),
            "sessions get none"
        );
        svc.open(TabOwner::Personal, None).await.unwrap();
        let set = fresh.calls("Storage.setCookies");
        assert_eq!(set.len(), 1);
        assert_eq!(set[0]["browserContextId"], personal_ctx(&fresh));
        assert_eq!(set[0]["cookies"][0]["value"], "secret");
        assert!(set[0]["cookies"][0].get("session").is_none());
    }

    #[tokio::test]
    async fn a_snapshot_that_cannot_be_read_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("personal.snapshot");
        PersonalSnapshot::with_key(path.clone(), [1; 32])
            .save(&snapshot::Contents {
                cookies: vec![json!({ "name": "sid", "value": "v", "domain": "x", "path": "/" })],
            })
            .unwrap();
        let before = std::fs::read(&path).unwrap();

        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = BrowserService::start(
            EngineManager::new(fake.start_fn()),
            db.conn.clone(),
            // Another key: as if the keychain entry were replaced.
            Some(PersonalSnapshot::with_key(path.clone(), [2; 32])),
        )
        .await
        .unwrap();
        svc.open(TabOwner::Personal, None).await.unwrap();
        svc.save_personal().await;
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[tokio::test]
    async fn only_web_addresses_open() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "chrome://settings",
            "not a url",
        ] {
            let err = svc.open(session("s1"), Some(url)).await.unwrap_err();
            assert_eq!(err.code, "unsupported_url", "{url}");
        }
        assert_eq!(fake.lock().starts, 0);
    }

    /// The service against real Chromium: titles, history and contexts.
    /// Point `ZWIIM_TEST_ENGINE` at a `chrome-headless-shell` binary.
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "needs ZWIIM_TEST_ENGINE"]
    async fn the_real_engine_reports_pages_and_keeps_owners_apart() {
        let dir = tempfile::tempdir().unwrap();
        let start = super::super::engine::fake::real_engine(dir.path().join("profile"));

        // A page that sets a cookie and its title.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let body = format!(
                        "<title>Page {path}</title><script>document.cookie='seen=1'</script>"
                    );
                    let resp = format!("HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        let url = |p: &str| format!("http://127.0.0.1:{port}{p}");

        let db = fresh_in_memory_db().await;
        let svc = BrowserService::start(EngineManager::new(start), db.conn.clone(), None)
            .await
            .unwrap();
        let a = svc.open(session("s1"), Some(&url("/one"))).await.unwrap();
        until(&svc, &a.id, |t| t.title == "Page /one" && !t.loading).await;
        svc.navigate(&a.id, Navigation::Url(url("/two")))
            .await
            .unwrap();
        until(&svc, &a.id, |t| t.title == "Page /two").await;
        svc.navigate(&a.id, Navigation::Back).await.unwrap();
        until(&svc, &a.id, |t| t.url == url("/one")).await;

        // The cookie set in s1's context is not in s2's.
        let b = svc.open(session("s2"), Some(&url("/three"))).await.unwrap();
        until(&svc, &b.id, |t| t.title == "Page /three").await;
        let cookies = |id: String| {
            let svc = svc.clone();
            async move {
                let (cdp, s) = svc.session(&id).await.unwrap();
                cdp.call("Network.getCookies", json!({}), Some(&s))
                    .await
                    .unwrap()["cookies"]
                    .as_array()
                    .unwrap()
                    .len()
            }
        };
        assert_eq!(cookies(a.id.clone()).await, 1);
        let (cdp, s) = svc.session(&b.id).await.unwrap();
        cdp.call(
            "Network.deleteCookies",
            json!({ "name": "seen", "url": url("/") }),
            Some(&s),
        )
        .await
        .unwrap();
        assert_eq!(cookies(b.id.clone()).await, 0);
        assert_eq!(
            cookies(a.id.clone()).await,
            1,
            "s2 cannot touch s1's cookie"
        );

        // window.open from s1's page becomes an s1 tab that shares its cookie.
        let (cdp, s) = svc.session(&a.id).await.unwrap();
        let popup_url = url("/popup");
        cdp.call(
            "Runtime.evaluate",
            json!({ "expression": format!("window.open('{popup_url}')"), "userGesture": true }),
            Some(&s),
        )
        .await
        .unwrap();
        let popup = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(t) = svc
                    .list(Some("s1"))
                    .await
                    .into_iter()
                    .find(|t| t.id != a.id)
                {
                    return t;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the popup became a tab");
        until(&svc, &popup.id, |t| t.title == "Page /popup").await;
        assert_eq!(
            cookies(popup.id.clone()).await,
            1,
            "same context as its opener"
        );

        svc.close(&a.id).await.unwrap();
        assert_eq!(svc.list(None).await.len(), 2);
        svc.engine().stop().await;
    }
}
