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

use super::console::{self, ConsoleQuery, ConsoleReadout, ConsoleRing};
use super::engine::cdp::{Cdp, Event};
use super::engine::manager::{EngineError, EngineManager, EngineState};
use super::governor::{self, TabUse};
use super::helper::{self, HelperMessage, PageMenu};
use super::network::{NetworkQuery, NetworkReadout, NetworkRing};
use super::policy::{self, HostRule};
use super::saving;
use super::sites::{self, Actor, Verdict};
use super::snapshot::{self, PersonalSnapshot};
use super::store::{self, TabOwner, TabRecord};
use super::ui::{self, UiAnswer, UiKind, UiRequest};

const EVENT_BUFFER: usize = 256;

/// The site permissions an agent may set for its tabs (decision 12), as the
/// web's Permissions API names them.
pub const PERMISSIONS: [&str; 7] = [
    "geolocation",
    "notifications",
    "camera",
    "microphone",
    "clipboard-read",
    "clipboard-write",
    "midi",
];
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

#[derive(Debug, Clone, PartialEq)]
pub enum BrowserEvent {
    Tab(TabInfo),
    Removed { tab_id: String, owner: TabOwner },
    /// A tab waits on a person (design 9).
    Ui(UiRequest),
    /// That request was answered, timed out, or its tab went.
    UiDone { request_id: String, tab_id: String },
    /// An agent is about to act on a tab (design 10.2).
    AgentAction(AgentAction),
    /// A page's context menu was asked for (design 9).
    Menu(PageMenu),
    /// A tab's page began a download, under the page's file name.
    Download {
        tab_id: String,
        name: String,
    },
}

/// What an agent is about to do, for clients to show: where its pointer
/// goes, in the page's CSS pixels, and the element's box when known.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentAction {
    pub tab_id: String,
    /// `click`, `hover`, `type`, `press` or `select`.
    pub kind: &'static str,
    pub x: f64,
    pub y: f64,
    #[serde(rename = "box", skip_serializing_if = "Option::is_none")]
    pub bounds: Option<super::capture::CaptureRegion>,
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
    /// Downloads the engine is saving, by its guid.
    saving: HashMap<String, Saving>,
    /// What session tabs printed, by tab id (design 10.2).
    consoles: HashMap<String, Console>,
    /// When a person last typed into each tab (design 10.3).
    typed: HashMap<String, tokio::time::Instant>,
    /// What session tabs fetched, by tab id (design 10.2).
    networks: HashMap<String, NetworkRing>,
    /// The page helper's world in each tab's current document (design 9).
    helpers: HashMap<String, i64>,
}

/// One tab's console lines, and its page's execution contexts: whether each
/// is the top document's, and its origin.
#[derive(Default)]
struct Console {
    ring: ConsoleRing,
    contexts: HashMap<i64, (bool, Option<String>)>,
}

/// A download in flight: the page's file name, and its row in the Host's
/// downloads.
struct Saving {
    /// The tab it came from, which is in use until it ends.
    tab_id: Option<String>,
    name: String,
    progress: crate::downloads::Progress,
    received: u64,
    sized: bool,
}

/// How tabs are used, for the memory governor (design 11), apart from
/// `inner` so the stream hub and the tools can say so without waiting.
struct Usage {
    /// Viewers of each tab.
    viewers: HashMap<String, usize>,
    /// When each tab was last viewed or driven.
    last_used: HashMap<String, tokio::time::Instant>,
    /// When an agent last used each tab.
    agent_at: HashMap<String, tokio::time::Instant>,
    /// Tabs the governor froze.
    frozen: std::collections::HashSet<String>,
    /// When anyone last viewed a tab or used the browser tools.
    active_at: tokio::time::Instant,
    /// Why new tabs are refused, while they are.
    refuse: Option<String>,
    /// The engine's tree runs in the background priority.
    background: bool,
}

/// What the site policy (design 13.2) knows, apart from `inner`: the pump
/// checks every page load with it, so a load never waits on the service.
#[derive(Default)]
struct Sites {
    /// Each loaded tab, by its CDP session.
    tabs: HashMap<String, SiteTab>,
    /// Strict site lists read so far, by session id.
    strict: HashMap<String, Vec<String>>,
    /// The sites a person always allows, once read.
    approved: Option<Vec<String>>,
    /// What a person allowed or denied for each session, by session id.
    answers: HashMap<String, Answers>,
    /// The administrator's rules.
    managed: Vec<HostRule>,
}

#[derive(Clone)]
struct SiteTab {
    owner: TabOwner,
    target_id: String,
    /// Who acted on the tab last.
    actor: Actor,
    /// Page loads refused since an agent last asked, newest last.
    refused: Vec<RefusedLoad>,
}

#[derive(Default, Clone)]
struct Answers {
    allowed: Vec<String>,
    denied: Vec<String>,
}

/// A page load the site policy refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedLoad {
    pub url: String,
    /// `site_blocked`, `site_not_allowed` or `site_denied`.
    pub code: &'static str,
}

/// Most refused loads kept per tab.
const MAX_REFUSED: usize = 10;
/// How long a site's name may take to resolve before it counts as public.
const RESOLVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

pub struct BrowserService {
    me: Weak<BrowserService>,
    engine: Arc<EngineManager>,
    snapshot: Option<Arc<PersonalSnapshot>>,
    db: DatabaseConnection,
    inner: Mutex<Inner>,
    sites: std::sync::Mutex<Sites>,
    usage: std::sync::Mutex<Usage>,
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
                saving: HashMap::new(),
                consoles: HashMap::new(),
                typed: HashMap::new(),
                networks: HashMap::new(),
                helpers: HashMap::new(),
            }),
            sites: std::sync::Mutex::new(Sites {
                managed: policy::read_managed_policy(&policy::managed_policy_path())
                    .map(|p| p.host_rules)
                    .unwrap_or_default(),
                ..Sites::default()
            }),
            usage: std::sync::Mutex::new(Usage {
                viewers: HashMap::new(),
                last_used: HashMap::new(),
                agent_at: HashMap::new(),
                frozen: std::collections::HashSet::new(),
                active_at: tokio::time::Instant::now(),
                refuse: None,
                background: false,
            }),
            events: broadcast::channel(EVENT_BUFFER).0,
        });
        tokio::spawn(follow_engine(Arc::downgrade(&service), state));
        tokio::spawn(govern(Arc::downgrade(&service)));
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
        self.wake().await;
        let mut inner = self.inner.lock().await;
        self.room_for_tab(&inner, &owner)?;
        let cdp = self.cdp_locked(&mut inner).await?;
        let live = self.create_target(&cdp, &mut inner, &owner).await?;
        if let Some(url) = &url {
            start_navigation(&cdp, &live.session_id, url);
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
        if let Some(live) = &tab.live {
            self.sites_mut().tabs.remove(&live.session_id);
        }
        self.forget_usage(tab_id);
        inner.consoles.remove(tab_id);
        inner.typed.remove(tab_id);
        inner.networks.remove(tab_id);
        inner.helpers.remove(tab_id);
        self.drop_ui_locked(&mut inner, |r| r.tab_id == tab_id);
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
        self.wake().await;
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
                start_navigation(&cdp, &session, &url);
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
        // Wanted again: a frozen tab wakes first.
        let frozen = {
            let mut usage = self.usage_mut();
            usage
                .last_used
                .insert(tab_id.to_string(), tokio::time::Instant::now());
            usage.frozen.remove(tab_id)
        };
        self.wake().await;
        if frozen {
            cdp.call(
                "Page.setWebLifecycleState",
                json!({ "state": "active" }),
                Some(&live.session_id),
            )
            .await?;
        }
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
            start_navigation(&cdp, &live.session_id, url);
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
        self.sites_mut().tabs.clear();
        self.drop_ui_locked(inner, |_| true);
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
                        start_navigation(&cdp, &live.session_id, &url);
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
                self.direct_downloads(cdp, owner, &ctx).await;
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
        // Until someone acts on it, a tab's loads are its opener's: the
        // person's, or an agent's tool, which says so before it loads.
        self.attach(cdp, inner, owner, target_id, Actor::Person)
            .await
    }

    /// Attach to a page target and set it up as a tab's, its loads counted
    /// as `actor`'s until someone acts on it.
    async fn attach(
        &self,
        cdp: &Cdp,
        inner: &Inner,
        owner: &TabOwner,
        target_id: String,
        actor: Actor,
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
        self.sites_mut().tabs.insert(
            session_id.clone(),
            SiteTab {
                owner: owner.clone(),
                target_id: target_id.clone(),
                actor,
                refused: Vec::new(),
            },
        );
        cdp.call("Page.enable", json!({}), s).await?;
        // Pages only, so a site's sign-in reaches a person (design 9) and
        // the site policy sees every page load (design 13.2).
        cdp.call(
            "Fetch.enable",
            json!({
                "handleAuthRequests": true,
                "patterns": [{ "urlPattern": "*", "resourceType": "Document" }],
            }),
            s,
        )
        .await?;
        // A page's file input asks a person, not the Mac's own file dialog.
        cdp.call(
            "Page.setInterceptFileChooserDialog",
            json!({ "enabled": true }),
            s,
        )
        .await?;
        // A session tab's console, for its agent. Personal tabs go without:
        // pages can tell `Runtime` is on.
        if matches!(owner, TabOwner::Session { .. }) {
            cdp.call("Runtime.enable", json!({}), s).await?;
            cdp.call("Network.enable", json!({}), s).await?;
        }
        if let Some(ua) = &inner.user_agent {
            cdp.call(
                "Emulation.setUserAgentOverride",
                json!({ "userAgent": ua }),
                s,
            )
            .await?;
        }
        tokio::spawn(helper::follow(
            self.me.clone(),
            cdp.clone(),
            session_id.clone(),
        ));
        Ok(Live {
            target_id,
            session_id,
        })
    }

    fn discard_locked(&self, inner: &mut Inner, tab_id: &str) {
        self.drop_ui_locked(inner, |r| r.tab_id == tab_id);
        // Its requests in flight will never finish.
        inner.networks.remove(tab_id);
        inner.helpers.remove(tab_id);
        let Some(tab) = inner.tabs.get_mut(tab_id) else {
            return;
        };
        if let Some(live) = tab.live.take() {
            self.sites_mut().tabs.remove(&live.session_id);
        }
        self.usage_mut().frozen.remove(tab_id);
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

    /// Tell clients what an agent is about to do.
    pub fn agent_action(&self, action: AgentAction) {
        self.emit(BrowserEvent::AgentAction(action));
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
                // Whoever made the opener open it.
                let actor = inner.tabs[&opener]
                    .live
                    .as_ref()
                    .and_then(|l| self.sites_mut().tabs.get(&l.session_id).map(|t| t.actor))
                    .unwrap_or(Actor::Agent);
                match self.attach(&cdp, &inner, &owner, target_id, actor).await {
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
                self.ask_locked(&mut inner, &id, ui::dialog_of(p));
            }
            // The pump already let it go on.
            "Fetch.requestPaused" => {
                // A new page in a frame ends a sign-in it was waiting on.
                let frame = p["frameId"].as_str().unwrap_or_default();
                if let Some(id) = by_session(&inner, &event.session_id.clone().unwrap_or_default())
                {
                    self.drop_ui_locked(&mut inner, |r| {
                        r.tab_id == id
                            && matches!(&r.kind, UiKind::Auth { frame_id, .. } if frame_id == frame)
                    });
                }
            }
            "Page.fileChooserOpened" => {
                let Some(id) = event
                    .session_id
                    .as_deref()
                    .and_then(|s| by_session(&inner, s))
                else {
                    return;
                };
                if let Some(kind) = ui::chooser_of(p, &inner.tabs[&id].info.url) {
                    self.ask_locked(&mut inner, &id, kind);
                }
            }
            "Fetch.authRequired" => {
                let Some(id) = event
                    .session_id
                    .as_deref()
                    .and_then(|s| by_session(&inner, s))
                else {
                    return;
                };
                self.ask_locked(&mut inner, &id, ui::auth_of(p));
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
                self.drop_ui_locked(&mut inner, |r| {
                    r.tab_id == id && matches!(r.kind, UiKind::Dialog { .. })
                });
            }
            "Browser.downloadWillBegin" => {
                let name = p["suggestedFilename"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let from = by_target(&inner, p["frameId"].as_str().unwrap_or_default());
                if let Some(tab_id) = from.clone() {
                    self.emit(BrowserEvent::Download {
                        tab_id,
                        name: name.clone(),
                    });
                }
                let progress = crate::downloads::global().begin(name.clone(), 1);
                let guid = p["guid"].as_str().unwrap_or_default().to_string();
                inner.saving.insert(
                    guid,
                    Saving {
                        tab_id: from,
                        name,
                        progress,
                        received: 0,
                        sized: false,
                    },
                );
            }
            "Browser.downloadProgress" => {
                let guid = p["guid"].as_str().unwrap_or_default();
                match p["state"].as_str() {
                    Some("inProgress") => {
                        let Some(d) = inner.saving.get_mut(guid) else {
                            return;
                        };
                        let total = p["totalBytes"].as_u64().unwrap_or(0);
                        if !d.sized && total > 0 {
                            d.sized = true;
                            d.progress.file(Some(total));
                        }
                        let received = p["receivedBytes"].as_u64().unwrap_or(0);
                        d.progress.received(received.saturating_sub(d.received));
                        d.received = d.received.max(received);
                    }
                    // The engine saved it under its guid: give it the page's name.
                    Some("completed") => {
                        let Some(d) = inner.saving.remove(guid) else {
                            return;
                        };
                        let saved = std::path::PathBuf::from(p["filePath"].as_str().unwrap_or_default());
                        tokio::task::spawn_blocking(move || {
                            let Some(dir) = saved.parent() else {
                                return;
                            };
                            let named = saving::free_name(dir, &d.name);
                            if let Err(e) = std::fs::rename(&saved, &named) {
                                tracing::warn!(error = %e, "could not name a browser download");
                            }
                            drop(d.progress);
                        });
                    }
                    // The engine removes what it had saved.
                    Some("canceled") => {
                        inner.saving.remove(guid);
                    }
                    _ => {}
                }
            }
            "Network.requestWillBeSent"
            | "Network.responseReceived"
            | "Network.loadingFinished"
            | "Network.loadingFailed" => {
                let Some(id) = event
                    .session_id
                    .as_deref()
                    .and_then(|s| by_session(&inner, s))
                else {
                    return;
                };
                let now = chrono::Utc::now().timestamp_millis();
                inner
                    .networks
                    .entry(id)
                    .or_default()
                    .on_event(&event.method, p, now);
            }
            "Runtime.executionContextCreated"
            | "Runtime.executionContextsCleared"
            | "Runtime.consoleAPICalled"
            | "Runtime.exceptionThrown" => {
                let Some(id) = event
                    .session_id
                    .as_deref()
                    .and_then(|s| by_session(&inner, s))
                else {
                    return;
                };
                let main = inner.tabs[&id].live.as_ref().map(|l| l.target_id.clone());
                let console = inner.consoles.entry(id).or_default();
                let at = chrono::Utc::now().timestamp_millis();
                let context_of = |console: &Console, context: &serde_json::Value| {
                    context
                        .as_i64()
                        .and_then(|c| console.contexts.get(&c).cloned())
                        .unwrap_or((false, None))
                };
                match event.method.as_str() {
                    "Runtime.executionContextCreated" => {
                        let context = &p["context"];
                        let top = context["auxData"]["isDefault"] == true
                            && context["auxData"]["frameId"].as_str() == main.as_deref();
                        let origin = console::cdp_context_origin(context["origin"].as_str());
                        if let Some(c) = context["id"].as_i64() {
                            console.contexts.insert(c, (top, origin));
                        }
                    }
                    // A new document.
                    "Runtime.executionContextsCleared" => {
                        console.ring.clear();
                        console.contexts.clear();
                    }
                    "Runtime.consoleAPICalled" => {
                        let (top, origin) = context_of(console, &p["executionContextId"]);
                        if let Some(line) = console::cdp_console_line(p, at, top, origin) {
                            console.ring.push(line, true);
                        }
                    }
                    _ => {
                        let context = &p["exceptionDetails"]["executionContextId"];
                        let (top, origin) = context_of(console, context);
                        if let Some(line) = console::cdp_exception_line(p, at, top, origin) {
                            console.ring.push(line, true);
                        }
                    }
                }
            }
            "Target.targetCrashed" => {
                let Some(id) = by_target(&inner, p["targetId"].as_str().unwrap_or_default()) else {
                    return;
                };
                self.drop_ui_locked(&mut inner, |r| r.tab_id == id);
                inner.networks.remove(&id);
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
                self.drop_ui_locked(&mut inner, |r| r.tab_id == id);
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

    /// Point the context `ctx` of `owner` at where its downloads go
    /// (design, decision 13), or refuse them when there is nowhere.
    async fn direct_downloads(&self, cdp: &Cdp, owner: &TabOwner, ctx: &str) {
        let params = match saving::folder_for(&self.db, owner).await {
            Some(dir) => {
                if let Some(worktree) = saving::worktree_of_default(&dir) {
                    let worktree = worktree.to_path_buf();
                    tokio::spawn(async move {
                        crate::commands::folder_links::add_to_git_exclude(&worktree, ".zwiim/").await;
                    });
                }
                json!({ "behavior": "allowAndName", "browserContextId": ctx,
                        "downloadPath": dir, "eventsEnabled": true })
            }
            None => json!({ "behavior": "deny", "browserContextId": ctx }),
        };
        if let Err(e) = cdp.call("Browser.setDownloadBehavior", params, None).await {
            tracing::warn!(error = %e, "could not set where browser downloads go");
        }
    }

    /// Set what `origin` may use in `owner`'s context (design, decision 12):
    /// each of `PERMISSIONS` granted when listed, denied otherwise.
    pub async fn set_permissions(
        &self,
        owner: &TabOwner,
        origin: &str,
        granted: &[String],
    ) -> Result<(), BrowserError> {
        let (cdp, ctx) = {
            let mut inner = self.inner.lock().await;
            let cdp = self.cdp_locked(&mut inner).await?;
            let ctx = inner.contexts.get(owner).cloned().ok_or_else(|| BrowserError {
                code: "tab_not_found",
                message: "no tab of this owner is loaded".into(),
            })?;
            (cdp, ctx)
        };
        for name in PERMISSIONS {
            let setting = if granted.iter().any(|g| g == name) { "granted" } else { "denied" };
            cdp.call(
                "Browser.setPermission",
                json!({ "permission": { "name": name }, "setting": setting,
                        "origin": origin, "browserContextId": ctx }),
                None,
            )
            .await?;
        }
        Ok(())
    }

    /// What tab `tab_id` printed to its console, if it is a session tab.
    pub async fn console(&self, tab_id: &str, query: &ConsoleQuery) -> Option<ConsoleReadout> {
        let inner = self.inner.lock().await;
        let tab = inner.tabs.get(tab_id)?;
        let empty = Console::default();
        let console = inner.consoles.get(tab_id).unwrap_or(&empty);
        Some(console.ring.read(query, &tab.info.url, |_| true))
    }

    /// What tab `tab_id` fetched, if it is open.
    pub async fn network(&self, tab_id: &str, query: &NetworkQuery) -> Option<NetworkReadout> {
        let inner = self.inner.lock().await;
        let tab = inner.tabs.get(tab_id)?;
        let empty = NetworkRing::default();
        let ring = inner.networks.get(tab_id).unwrap_or(&empty);
        Some(ring.read(query, &tab.info.url))
    }

    /// How many of tab `tab_id`'s requests are still waiting on an answer.
    pub async fn network_busy(&self, tab_id: &str) -> usize {
        let inner = self.inner.lock().await;
        inner.networks.get(tab_id).map_or(0, NetworkRing::busy)
    }

    /// Where `owner`'s browser files go: its downloads folder.
    pub async fn files_folder(&self, owner: &TabOwner) -> Option<std::path::PathBuf> {
        saving::folder_for(&self.db, owner).await
    }

    /// Whether the person lets agents type passwords on any site.
    pub async fn agent_passwords(&self) -> bool {
        crate::settings::browser_agent_passwords(&self.db).await
    }

    /// Whether a tab is attached on engine session `session`.
    pub async fn has_session(&self, session: &str) -> bool {
        let inner = self.inner.lock().await;
        tab_on(&inner, session).is_some()
    }

    /// The page helper is in the new document of the tab on `session`.
    /// Picks the old document waited on are gone with it.
    pub async fn helper_ready(&self, session: &str, context: i64) {
        let mut inner = self.inner.lock().await;
        let Some(id) = tab_on(&inner, session) else {
            return;
        };
        inner.helpers.insert(id.clone(), context);
        self.drop_ui_locked(&mut inner, |r| r.tab_id == id && r.kind.is_pick());
    }

    /// Highlight `query` in tab `tab_id` and move to its next match (the one
    /// before with `backwards`); an empty query clears it. `(matches,
    /// current)`, `current` counting from 1, 0 with no match.
    pub async fn find(
        &self,
        tab_id: &str,
        query: &str,
        backwards: bool,
    ) -> Result<(u32, u32), BrowserError> {
        let (cdp, session, context) = {
            let inner = self.inner.lock().await;
            let tab = inner.tabs.get(tab_id).ok_or_else(|| not_found(tab_id))?;
            let live = tab.live.as_ref().ok_or_else(|| BrowserError {
                code: "page_not_ready",
                message: "The page is not loaded yet".into(),
            })?;
            let context = inner.helpers.get(tab_id).copied();
            (inner.cdp.clone(), live.session_id.clone(), context)
        };
        let (Some(cdp), Some(context)) = (cdp, context) else {
            return Err(BrowserError {
                code: "page_not_ready",
                message: "The page is not loaded yet".into(),
            });
        };
        let reply = cdp
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": helper::find_expression(query, backwards),
                    "contextId": context,
                    "returnByValue": true,
                }),
                Some(&session),
            )
            .await?;
        let found: serde_json::Value = reply["result"]["value"]
            .as_str()
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default();
        let count = |key: &str| found[key].as_u64().unwrap_or(0).min(u32::MAX.into()) as u32;
        Ok((count("matches"), count("current")))
    }

    /// What the page helper of the tab on `session` reported.
    pub async fn helper_said(&self, session: &str, context: i64, messages: Vec<HelperMessage>) {
        let mut inner = self.inner.lock().await;
        let Some(id) = tab_on(&inner, session) else {
            return;
        };
        if inner.helpers.get(&id) != Some(&context) {
            return;
        }
        for message in messages {
            match message {
                HelperMessage::Menu {
                    x,
                    y,
                    link_url,
                    image_url,
                    selection,
                    editable,
                } => self.emit(BrowserEvent::Menu(PageMenu {
                    tab_id: id.clone(),
                    x,
                    y,
                    link_url,
                    image_url,
                    selection,
                    editable,
                })),
                HelperMessage::Title => {
                    let generation = inner.generation;
                    self.refresh_info(&inner, &id, generation);
                }
                HelperMessage::Select {
                    id: element,
                    options,
                    at,
                } => {
                    self.drop_ui_locked(&mut inner, |r| r.tab_id == id && r.kind.is_pick());
                    let kind = UiKind::Select {
                        options,
                        at,
                        context,
                        element,
                    };
                    self.ask_locked(&mut inner, &id, kind);
                }
                HelperMessage::Date {
                    id: element,
                    input_type,
                    value,
                    min,
                    max,
                    at,
                } => {
                    self.drop_ui_locked(&mut inner, |r| r.tab_id == id && r.kind.is_pick());
                    let kind = UiKind::Date {
                        input_type,
                        value,
                        min,
                        max,
                        at,
                        context,
                        element,
                    };
                    self.ask_locked(&mut inner, &id, kind);
                }
            }
        }
    }

    fn usage_mut(&self) -> std::sync::MutexGuard<'_, Usage> {
        self.usage.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn forget_usage(&self, tab_id: &str) {
        let mut usage = self.usage_mut();
        usage.viewers.remove(tab_id);
        usage.last_used.remove(tab_id);
        usage.agent_at.remove(tab_id);
        usage.frozen.remove(tab_id);
    }

    /// Someone is using the browser: out of the background priority at once
    /// rather than at the next check (design 11.4).
    async fn wake(&self) {
        let was_background = {
            let mut usage = self.usage_mut();
            usage.active_at = tokio::time::Instant::now();
            std::mem::replace(&mut usage.background, false)
        };
        #[cfg(target_os = "macos")]
        if was_background {
            if let Some(pid) = self.engine.pid().await {
                let _ = tokio::task::spawn_blocking(move || {
                    governor::sys::set_priority(pid, super::engine::launch::ENGINE_NICE, false)
                })
                .await;
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = was_background;
    }

    /// A viewer started (`on`) or stopped watching tab `tab_id`.
    pub fn viewing(&self, tab_id: &str, on: bool) {
        let mut usage = self.usage_mut();
        let now = tokio::time::Instant::now();
        let viewers = usage.viewers.entry(tab_id.to_string()).or_default();
        *viewers = if on {
            *viewers + 1
        } else {
            viewers.saturating_sub(1)
        };
        if *viewers == 0 {
            usage.viewers.remove(tab_id);
        }
        usage.last_used.insert(tab_id.to_string(), now);
        usage.active_at = now;
    }

    /// An agent used tab `tab_id` through its browser tools.
    pub fn agent_used(&self, tab_id: &str) {
        let mut usage = self.usage_mut();
        let now = tokio::time::Instant::now();
        usage.agent_at.insert(tab_id.to_string(), now);
        usage.last_used.insert(tab_id.to_string(), now);
        usage.active_at = now;
    }

    /// Whether `owner` may open another tab: within the tab caps, and while
    /// the governor is not refusing new tabs (design 11.2, 11.3).
    fn room_for_tab(&self, inner: &Inner, owner: &TabOwner) -> Result<(), BrowserError> {
        if inner.tabs.len() >= governor::MAX_TABS {
            return Err(BrowserError {
                code: "too_many_tabs",
                message: format!(
                    "The browser holds {} tabs, its most; close some first.",
                    governor::MAX_TABS
                ),
            });
        }
        let mine = inner
            .tabs
            .values()
            .filter(|t| t.info.owner == *owner)
            .count();
        if matches!(owner, TabOwner::Session { .. }) && mine >= governor::MAX_SESSION_TABS {
            return Err(BrowserError {
                code: "too_many_tabs",
                message: format!(
                    "This session has {} tabs, its most; close some first.",
                    governor::MAX_SESSION_TABS
                ),
            });
        }
        if let Some(why) = self.usage_mut().refuse.clone() {
            return Err(BrowserError {
                code: "browser_memory_full",
                message: why,
            });
        }
        Ok(())
    }

    /// Measure, then freeze, discard and refuse as the governor says, and
    /// set the engine's priority (design 11).
    async fn govern_once(&self) {
        let Some(pid) = self.engine.pid().await else {
            return;
        };
        let background = {
            let mut usage = self.usage_mut();
            usage.background =
                usage.viewers.is_empty() && usage.active_at.elapsed() >= governor::IDLE_AFTER;
            usage.background
        };
        #[cfg(target_os = "macos")]
        let measured = tokio::task::spawn_blocking(move || {
            // Background when nobody views a tab or uses the tools; new
            // processes since the last check get it too.
            governor::sys::set_priority(pid, super::engine::launch::ENGINE_NICE, background);
            (governor::sys::usage(pid).0, governor::sys::pressure())
        })
        .await
        .ok();
        #[cfg(not(target_os = "macos"))]
        let measured: Option<(u64, governor::Pressure)> = {
            let _ = (pid, background);
            None
        };
        if let Some((used, pressure)) = measured {
            self.ease(used, governor::budget(), pressure).await;
        }
    }

    /// With the engine at `used` of `budget`: freeze, discard and refuse new
    /// tabs as the ladder says (design 11.3).
    async fn ease(&self, used: u64, budget: u64, pressure: governor::Pressure) {
        let now = tokio::time::Instant::now();
        let mut inner = self.inner.lock().await;
        let busy: std::collections::HashSet<String> = inner
            .saving
            .values()
            .filter_map(|d| d.tab_id.clone())
            .collect();
        let tabs: Vec<TabUse> = {
            let usage = self.usage_mut();
            inner
                .tabs
                .iter()
                .filter(|(_, t)| t.live.is_some())
                .map(|(id, _)| TabUse {
                    tab_id: id.clone(),
                    in_use: usage.viewers.contains_key(id)
                        || busy.contains(id)
                        || usage
                            .agent_at
                            .get(id)
                            .is_some_and(|at| now.duration_since(*at) < governor::AGENT_IN_USE),
                    frozen: usage.frozen.contains(id),
                    last_used: usage.last_used.get(id).copied().unwrap_or(now),
                })
                .collect()
        };
        let plan = governor::plan(now, used, budget, pressure, &tabs);
        self.usage_mut().refuse = plan.refuse_new.then(|| busiest(&inner, used, budget));
        if plan.freeze.is_empty() && plan.discard.is_empty() {
            return;
        }
        tracing::info!(
            used_mb = used >> 20,
            budget_mb = budget >> 20,
            ?pressure,
            freeze = plan.freeze.len(),
            discard = plan.discard.len(),
            "the browser governor is easing memory"
        );
        let Some(cdp) = inner.cdp.clone() else {
            return;
        };
        for id in &plan.freeze {
            let Some(live) = inner.tabs.get(id).and_then(|t| t.live.clone()) else {
                continue;
            };
            let frozen = cdp
                .call(
                    "Page.setWebLifecycleState",
                    json!({ "state": "frozen" }),
                    Some(&live.session_id),
                )
                .await;
            if frozen.is_ok() {
                self.usage_mut().frozen.insert(id.clone());
            }
        }
        for id in &plan.discard {
            let Some(live) = inner.tabs.get(id).and_then(|t| t.live.clone()) else {
                continue;
            };
            let _ = cdp
                .call(
                    "Target.closeTarget",
                    json!({ "targetId": live.target_id }),
                    None,
                )
                .await;
            self.discard_locked(&mut inner, id);
        }
    }

    fn sites_mut(&self) -> std::sync::MutexGuard<'_, Sites> {
        self.sites.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `actor` acted on tab `tab_id`: its next page loads are theirs (design
    /// 13.2).
    pub async fn acted(&self, tab_id: &str, actor: Actor) {
        let inner = self.inner.lock().await;
        let Some(live) = inner.tabs.get(tab_id).and_then(|t| t.live.as_ref()) else {
            return;
        };
        if let Some(tab) = self.sites_mut().tabs.get_mut(&live.session_id) {
            tab.actor = actor;
            if actor == Actor::Agent {
                tab.refused.clear();
            }
        }
    }

    /// The page loads of tab `tab_id` the site policy refused since an agent
    /// last acted on it.
    pub async fn refused_loads(&self, tab_id: &str) -> Vec<RefusedLoad> {
        let inner = self.inner.lock().await;
        let Some(live) = inner.tabs.get(tab_id).and_then(|t| t.live.as_ref()) else {
            return Vec::new();
        };
        self.sites_mut()
            .tabs
            .get(&live.session_id)
            .map(|t| t.refused.clone())
            .unwrap_or_default()
    }

    /// What the site policy says of an agent loading `url` in tab `tab_id`,
    /// before it does.
    pub async fn site_check(&self, tab_id: &str, url: &str) -> Verdict {
        let session = {
            let inner = self.inner.lock().await;
            match inner.tabs.get(tab_id).and_then(|t| t.live.as_ref()) {
                Some(live) => live.session_id.clone(),
                None => return Verdict::Allow,
            }
        };
        self.site_verdict_as(&session, url, None, Some(Actor::Agent))
            .await
    }

    /// Session `session_id`'s strict site list; empty for none.
    pub async fn strict_sites(&self, session_id: &str) -> Vec<String> {
        if let Some(list) = self.sites_mut().strict.get(session_id) {
            return list.clone();
        }
        let list = store::strict_sites(&self.db, session_id)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "could not read a strict site list");
                Vec::new()
            });
        self.sites_mut()
            .strict
            .insert(session_id.to_string(), list.clone());
        list
    }

    /// Replace session `session_id`'s strict site list with `entries`
    /// (site patterns or pasted addresses); empty removes it. Returns the
    /// list as kept.
    pub async fn set_strict_sites(
        &self,
        session_id: &str,
        entries: &[String],
    ) -> Result<Vec<String>, BrowserError> {
        let mut list: Vec<String> = Vec::new();
        for entry in entries {
            let pattern = sites::site_pattern(entry).ok_or_else(|| BrowserError {
                code: "invalid_site",
                message: format!("{entry:?} is not a site: use a name like example.com, *.example.com or localhost:3000"),
            })?;
            if !list.contains(&pattern) {
                list.push(pattern);
            }
        }
        if list.len() > sites::MAX_SITES {
            return Err(BrowserError {
                code: "invalid_site",
                message: format!(
                    "a strict site list holds at most {} sites",
                    sites::MAX_SITES
                ),
            });
        }
        store::set_strict_sites(&self.db, session_id, &list)
            .await
            .map_err(|e| BrowserError {
                code: "engine_error",
                message: e.to_string(),
            })?;
        self.sites_mut()
            .strict
            .insert(session_id.to_string(), list.clone());
        Ok(list)
    }

    /// The site policy's answer for loading `url` in the tab of CDP session
    /// `session`, in frame `frame` (the tab's own page when none).
    async fn site_verdict(&self, session: &str, url: &str, frame: Option<&str>) -> Verdict {
        self.site_verdict_as(session, url, frame, None).await
    }

    async fn site_verdict_as(
        &self,
        session: &str,
        url: &str,
        frame: Option<&str>,
        actor: Option<Actor>,
    ) -> Verdict {
        let Ok(url) = url::Url::parse(url) else {
            return Verdict::Allow;
        };
        if !matches!(url.scheme(), "http" | "https") {
            return Verdict::Allow;
        }
        let (tab, managed) = {
            let sites = self.sites_mut();
            let Some(tab) = sites.tabs.get(session) else {
                return Verdict::Allow;
            };
            (tab.clone(), sites.managed.clone())
        };
        let actor = actor.unwrap_or(tab.actor);
        let main_frame = frame.is_none_or(|f| f == tab.target_id);
        let mut check = sites::Check {
            url: &url,
            main_frame,
            actor,
            managed: &managed,
            strict: &[],
            ask: false,
            approved: &[],
            denied: &[],
            workspace_server: false,
            resolves_local: false,
        };
        // Blocked, or nothing more to know.
        let quick = sites::decide(&check);
        let TabOwner::Session { session_id } = &tab.owner else {
            return quick;
        };
        if quick != Verdict::Allow || actor == Actor::Person || !main_frame {
            return quick;
        }
        let strict = self.strict_sites(session_id).await;
        let ask = strict.is_empty() && crate::settings::browser_agent_ask_sites(&self.db).await;
        if strict.is_empty() && !ask {
            return Verdict::Allow;
        }
        let approved = if ask {
            self.approved_sites().await
        } else {
            Vec::new()
        };
        let answers = self
            .sites_mut()
            .answers
            .get(session_id)
            .cloned()
            .unwrap_or_default();
        let approved: Vec<String> = approved.into_iter().chain(answers.allowed).collect();
        check.strict = &strict;
        check.ask = ask;
        check.approved = &approved;
        check.denied = &answers.denied;
        check.workspace_server = self.workspace_server(session_id, &url).await;
        check.resolves_local = resolves_local(&url).await;
        sites::decide(&check)
    }

    async fn approved_sites(&self) -> Vec<String> {
        if let Some(approved) = &self.sites_mut().approved {
            return approved.clone();
        }
        let approved = store::approved_sites(&self.db).await.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "could not read the allowed sites");
            Vec::new()
        });
        self.sites_mut().approved = Some(approved.clone());
        approved
    }

    /// `url` is a loopback server running from session `session_id`'s
    /// worktree (design 10.5).
    async fn workspace_server(&self, session_id: &str, url: &url::Url) -> bool {
        let Some(port) = super::listener::loopback_port(&url.origin().ascii_serialization()) else {
            return false;
        };
        let Some(worktree) = saving::worktree_of(&self.db, session_id).await else {
            return false;
        };
        let identity = tokio::task::spawn_blocking(move || super::listener::identify(port))
            .await
            .ok()
            .flatten();
        identity
            .and_then(|i| i.workdir)
            .is_some_and(|dir| std::path::Path::new(&dir).starts_with(&worktree))
    }

    /// Note a page load the site policy refused in the tab of CDP session
    /// `session`.
    fn refused(&self, session: &str, url: &str, code: &'static str) {
        let site = url::Url::parse(url).ok().and_then(|u| sites::site_of(&u));
        tracing::info!(
            site = site.as_deref().unwrap_or("?"),
            code,
            "refused a browser page load"
        );
        if let Some(tab) = self.sites_mut().tabs.get_mut(session) {
            if tab.refused.len() >= MAX_REFUSED {
                tab.refused.remove(0);
            }
            tab.refused.push(RefusedLoad {
                url: url.to_string(),
                code,
            });
        }
    }

    /// Ask the person whether the paused load `fetch_request` of `url` may
    /// go on. A newer load in the same tab replaces one still waiting.
    async fn ask_site(&self, session: &str, fetch_request: String, url: &str) {
        let Ok(parsed) = url::Url::parse(url) else {
            return;
        };
        let local = resolves_local(&parsed).await;
        let mut inner = self.inner.lock().await;
        let Some(tab_id) = tab_on(&inner, session) else {
            return;
        };
        let waiting: Vec<String> = inner
            .ui
            .values()
            .filter(|r| r.tab_id == tab_id)
            .filter_map(|r| match &r.kind {
                UiKind::Site { fetch_request, .. } => Some(fetch_request.clone()),
                _ => None,
            })
            .collect();
        if let Some(cdp) = inner.cdp.clone() {
            for old in waiting {
                let (method, params) = ui::refuse_load(&old);
                let _ = cdp.call(method, params, Some(session)).await;
            }
        }
        self.drop_ui_locked(&mut inner, |r| {
            r.tab_id == tab_id && matches!(r.kind, UiKind::Site { .. })
        });
        let kind = UiKind::Site {
            site: sites::site_of(&parsed).unwrap_or_default(),
            url: url.to_string(),
            local,
            fetch_request,
        };
        self.ask_locked(&mut inner, &tab_id, kind);
    }

    /// A person typed into tab `tab_id`.
    pub async fn person_typed(&self, tab_id: &str) {
        let now = tokio::time::Instant::now();
        self.inner
            .lock()
            .await
            .typed
            .insert(tab_id.to_string(), now);
    }

    /// When a person last typed into tab `tab_id`, if they have.
    pub async fn last_typed(&self, tab_id: &str) -> Option<tokio::time::Instant> {
        self.inner.lock().await.typed.get(tab_id).copied()
    }

    /// The seq of tab `tab_id`'s newest console line, to read only what
    /// comes after it.
    pub async fn console_mark(&self, tab_id: &str) -> u64 {
        let inner = self.inner.lock().await;
        inner.consoles.get(tab_id).map_or(0, |c| c.ring.last_seq())
    }

    /// The download folder setting changed: every open context follows it.
    pub async fn redirect_downloads(&self) {
        let inner = self.inner.lock().await;
        let Some(cdp) = inner.cdp.clone() else {
            return;
        };
        for (owner, ctx) in &inner.contexts {
            self.direct_downloads(&cdp, owner, ctx).await;
        }
    }

    /// The request `request_id`, while it waits.
    pub async fn ui_request(&self, request_id: &str) -> Option<UiRequest> {
        self.inner.lock().await.ui.get(request_id).cloned()
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
        let call = ui::answer_call(&request.kind, answer)?;
        if let (
            UiKind::Site { site, .. },
            UiAnswer::Site { allow, always },
            TabOwner::Session { session_id },
        ) = (&request.kind, answer, &request.owner)
        {
            if *allow && *always {
                if let Err(e) = store::approve_site(&self.db, site).await {
                    tracing::warn!(error = %e, "could not save an allowed site");
                }
                // Unread yet: the next read finds it.
                if let Some(approved) = &mut self.sites_mut().approved {
                    approved.push(site.clone());
                }
            }
            let mut sites = self.sites_mut();
            let answers = sites.answers.entry(session_id.clone()).or_default();
            if *allow {
                &mut answers.allowed
            } else {
                &mut answers.denied
            }
            .push(site.clone());
        }
        let session = inner
            .tabs
            .get(&request.tab_id)
            .and_then(|t| t.live.as_ref())
            .map(|l| l.session_id.clone());
        // A dialog or sign-in that already went is no error: the request is done.
        let delivered = match (inner.cdp.clone(), session, call) {
            (Some(cdp), Some(session), Some((method, params))) => {
                cdp.call(method, params, Some(&session)).await.map(|_| ())
            }
            _ => Ok(()),
        };
        self.drop_ui_locked(&mut inner, |r| r.request_id == request_id);
        if let Err(e) = delivered {
            tracing::debug!(error = %e, "a browser request went before its answer");
        }
        Ok(())
    }

    /// Dismiss a request nobody answered in time.
    async fn expire(&self, request_id: &str) {
        let mut inner = self.inner.lock().await;
        let Some(request) = inner.ui.get(request_id).cloned() else {
            return;
        };
        tracing::warn!(tab_id = %request.tab_id, "ended a browser request nobody answered");
        let session = inner
            .tabs
            .get(&request.tab_id)
            .and_then(|t| t.live.as_ref())
            .map(|l| l.session_id.clone());
        if let (Some(cdp), Some(session), Some((method, params))) =
            (inner.cdp.clone(), session, ui::dismiss_call(&request.kind))
        {
            let _ = cdp.call(method, params, Some(&session)).await;
        }
        self.drop_ui_locked(&mut inner, |r| r.request_id == request_id);
    }

    /// Ask a person for `kind` on behalf of the tab `tab_id`, until answered
    /// or [`ui::timeout`].
    fn ask_locked(&self, inner: &mut Inner, tab_id: &str, kind: UiKind) {
        inner.next_ui += 1;
        let request = UiRequest {
            request_id: format!("ui-{}", inner.next_ui),
            tab_id: tab_id.to_string(),
            owner: inner.tabs[tab_id].info.owner.clone(),
            kind,
        };
        let wait = ui::timeout(&request.kind);
        let request_id = request.request_id.clone();
        inner.ui.insert(request_id.clone(), request.clone());
        self.emit(BrowserEvent::Ui(request));
        let me = self.me.clone();
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            if let Some(service) = me.upgrade() {
                service.expire(&request_id).await;
            }
        });
    }

    /// End the requests `done` picks.
    fn drop_ui_locked(&self, inner: &mut Inner, done: impl Fn(&UiRequest) -> bool) {
        let done: Vec<UiRequest> = inner.ui.values().filter(|r| done(r)).cloned().collect();
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

/// Load `url` in the tab of `session` without waiting for the server: a
/// page that wants a sign-in answers only once a person has, and that needs
/// the service.
fn start_navigation(cdp: &Cdp, session: &str, url: &str) {
    let reply = cdp.request(
        "Page.navigate",
        json!({ "url": url }),
        Some(session),
        super::engine::cdp::DEFAULT_TIMEOUT,
    );
    tokio::spawn(async move {
        if let Err(e) = reply.await {
            tracing::debug!(error = %e, "a browser navigation did not start");
        }
    });
}

/// Decide a paused page load (`Fetch.enable` in `attach`) by the site
/// policy, without the service lock: a load the service waits on would
/// never come. It goes on, is refused, or waits on a person.
async fn screen(service: Weak<BrowserService>, cdp: Cdp, event: Event) {
    let p = &event.params;
    let session = event.session_id.clone().unwrap_or_default();
    let request = p["requestId"].as_str().unwrap_or_default().to_string();
    let url = p["request"]["url"].as_str().unwrap_or_default();
    let verdict = match service.upgrade() {
        Some(service) => {
            let frame = p["frameId"].as_str().unwrap_or_default();
            service.site_verdict(&session, url, Some(frame)).await
        }
        None => Verdict::Allow,
    };
    let (method, params) = match verdict {
        Verdict::Allow => ("Fetch.continueRequest", json!({ "requestId": request })),
        Verdict::Refuse(code) => {
            if let Some(service) = service.upgrade() {
                service.refused(&session, url, code);
            }
            ui::refuse_load(&request)
        }
        Verdict::Ask => {
            if let Some(service) = service.upgrade() {
                service.ask_site(&session, request, url).await;
            }
            return;
        }
    };
    if let Err(e) = cdp.call(method, params, Some(&session)).await {
        tracing::debug!(error = %e, "a paused page request went away");
    }
}

/// Why new tabs are refused: the browser is at `used` of its `budget`, and
/// which owners hold the most loaded tabs.
fn busiest(inner: &Inner, used: u64, budget: u64) -> String {
    let mut held: HashMap<String, usize> = HashMap::new();
    for tab in inner.tabs.values().filter(|t| t.live.is_some()) {
        let who = match &tab.info.owner {
            TabOwner::Session { session_id } => format!("session {}", short(session_id)),
            TabOwner::Personal => "your own tabs".to_string(),
        };
        *held.entry(who).or_default() += 1;
    }
    let mut held: Vec<(String, usize)> = held.into_iter().collect();
    held.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let most: Vec<String> = held
        .iter()
        .take(3)
        .map(|(who, n)| format!("{who} ({n})"))
        .collect();
    format!(
        "The browser is using {} MB of its {} MB, so it opens no new tabs until some close. Most are held by {}.",
        used >> 20,
        budget >> 20,
        if most.is_empty() { "nobody".to_string() } else { most.join(", ") }
    )
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// Run the governor every [`governor::INTERVAL`] while the service lives.
async fn govern(service: Weak<BrowserService>) {
    let mut tick = tokio::time::interval(governor::INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let Some(service) = service.upgrade() else {
            return;
        };
        service.govern_once().await;
    }
}

/// `url`'s host names, or resolves to, a loopback, private or link-local
/// address. A name that does not resolve in time counts as public.
async fn resolves_local(url: &url::Url) -> bool {
    if sites::is_local_host(url) {
        return true;
    }
    let Some(url::Host::Domain(name)) = url.host() else {
        return false;
    };
    let port = url.port_or_known_default().unwrap_or(80);
    let lookup = tokio::net::lookup_host((name.to_string(), port));
    match tokio::time::timeout(RESOLVE_TIMEOUT, lookup).await {
        Ok(Ok(mut addrs)) => addrs.any(|a| sites::is_local_addr(a.ip())),
        _ => false,
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
        if event.method == "Fetch.requestPaused" {
            tokio::spawn(screen(service.clone(), cdp.clone(), event.clone()));
        }
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
/// The tab attached on engine session `session`.
fn tab_on(inner: &Inner, session: &str) -> Option<String> {
    inner
        .tabs
        .iter()
        .find(|(_, t)| t.live.as_ref().is_some_and(|l| l.session_id == session))
        .map(|(id, _)| id.clone())
}

pub(crate) fn check_url(url: &str) -> Result<String, BrowserError> {
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
        let target = fake.wait_url("https://example.com/").await;
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
        let target = fake.wait_url(url).await;
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
    async fn a_sign_in_waits_for_a_person_apart_from_dialogs() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        let tab = svc
            .open(TabOwner::Personal, Some("https://example.com/"))
            .await
            .unwrap();
        assert_eq!(
            fake.calls("Fetch.enable"),
            vec![json!({"handleAuthRequests": true,
                        "patterns": [{"urlPattern": "*", "resourceType": "Document"}]})]
        );
        let target = fake.wait_url("https://example.com/").await;
        let session = fake.lock().targets[&target].session.clone().unwrap();
        let mut events = svc.subscribe();

        let paused = |id: &str, frame: &str| {
            json!({"requestId": id, "frameId": frame, "resourceType": "Document",
                   "request": {"url": "https://example.com/admin"}})
        };
        let auth = |id: &str| {
            let mut p = paused(id, "F1");
            p["authChallenge"] = json!({"source": "Server", "origin": "https://example.com",
                                        "scheme": "basic", "realm": "Admin"});
            p
        };
        fake.emit("Fetch.requestPaused", paused("r1", "F1"), Some(&session));
        fake.emit("Fetch.authRequired", auth("r1"), Some(&session));
        let sign_in = loop {
            if let BrowserEvent::Ui(r) = events.recv().await.unwrap() {
                break r;
            }
        };
        assert_eq!(
            calls_until(&fake, "Fetch.continueRequest", 1).await,
            vec![json!({"requestId": "r1"})]
        );
        assert!(
            matches!(&sign_in.kind, UiKind::Auth { realm, proxy: false, .. } if realm == "Admin")
        );
        assert_eq!(sign_in.tab_id, tab.id);

        let dialog = open_dialog(
            &fake,
            &mut events,
            "https://example.com/",
            json!({"type": "alert", "message": "Hi", "url": ""}),
        )
        .await;
        let answer = UiAnswer::Auth {
            accept: true,
            username: "ann".into(),
            password: "pw".into(),
        };
        svc.answer(&sign_in.request_id, &answer).await.unwrap();
        assert_eq!(
            fake.calls("Fetch.continueWithAuth"),
            vec![json!({"requestId": "r1", "authChallengeResponse":
                        {"response": "ProvideCredentials", "username": "ann", "password": "pw"}})]
        );
        assert_eq!(ui_done(&mut events).await, sign_in.request_id);
        assert_eq!(svc.ui_requests().await, vec![dialog.clone()], "the dialog stays");
        assert_eq!(
            svc.answer(&dialog.request_id, &answer).await.unwrap_err().code,
            "invalid_answer"
        );

        // A new page in that frame ends a sign-in still waiting.
        fake.emit("Fetch.authRequired", auth("r2"), Some(&session));
        let second = loop {
            if let BrowserEvent::Ui(r) = events.recv().await.unwrap() {
                break r;
            }
        };
        fake.emit("Fetch.requestPaused", paused("r3", "F1"), Some(&session));
        assert_eq!(ui_done(&mut events).await, second.request_id);
        assert_eq!(svc.ui_requests().await, vec![dialog]);
    }

    /// The `method` calls once there are `n` of them.
    async fn calls_until(fake: &FakeEngine, method: &str, n: usize) -> Vec<serde_json::Value> {
        for _ in 0..200 {
            let calls = fake.calls(method);
            if calls.len() >= n {
                return calls;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "{method} was called {} times, not {n}",
            fake.calls(method).len()
        );
    }

    async fn asked(events: &mut broadcast::Receiver<BrowserEvent>) -> UiRequest {
        loop {
            if let BrowserEvent::Ui(r) = events.recv().await.unwrap() {
                return r;
            }
        }
    }

    fn page_load(id: &str, frame: &str, url: &str) -> serde_json::Value {
        json!({"requestId": id, "frameId": frame, "resourceType": "Document",
               "request": {"url": url}})
    }

    #[tokio::test]
    async fn a_strict_site_list_limits_what_the_agent_loads() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        let kept = svc
            .set_strict_sites(
                "s1",
                &["https://GitHub.com/x".into(), "*.example.com".into()],
            )
            .await
            .unwrap();
        assert_eq!(kept, vec!["github.com".to_string(), "*.example.com".into()]);
        assert_eq!(svc.strict_sites("s1").await, kept);
        assert_eq!(
            svc.set_strict_sites("s1", &["not a site".into()])
                .await
                .unwrap_err()
                .code,
            "invalid_site"
        );

        let tab = svc
            .open(session("s1"), Some("https://github.com/"))
            .await
            .unwrap();
        let target = fake.wait_url("https://github.com/").await;
        let cdp_session = fake.lock().targets[&target].session.clone().unwrap();
        let refused = |id: &str| json!({"requestId": id, "errorReason": "BlockedByClient"});

        // The person opened it: anything goes.
        fake.emit(
            "Fetch.requestPaused",
            page_load("r1", &target, "https://other.test/"),
            Some(&cdp_session),
        );
        assert_eq!(
            calls_until(&fake, "Fetch.continueRequest", 1).await,
            vec![json!({"requestId": "r1"})]
        );

        svc.acted(&tab.id, Actor::Agent).await;
        assert_eq!(
            svc.site_check(&tab.id, "https://other.test/").await,
            Verdict::Refuse(sites::ERROR_SITE_NOT_ALLOWED)
        );
        assert_eq!(
            svc.site_check(&tab.id, "https://a.example.com/").await,
            Verdict::Allow
        );
        fake.emit(
            "Fetch.requestPaused",
            page_load("r2", &target, "https://other.test/"),
            Some(&cdp_session),
        );
        assert_eq!(
            calls_until(&fake, "Fetch.failRequest", 1).await,
            vec![refused("r2")]
        );
        // A redirect or link to a listed site, and a frame inside the page.
        fake.emit(
            "Fetch.requestPaused",
            page_load("r3", &target, "https://docs.example.com/"),
            Some(&cdp_session),
        );
        fake.emit(
            "Fetch.requestPaused",
            page_load("r4", "F9", "https://embed.other.test/"),
            Some(&cdp_session),
        );
        let went = calls_until(&fake, "Fetch.continueRequest", 3).await;
        assert!(
            went.contains(&json!({"requestId": "r3"}))
                && went.contains(&json!({"requestId": "r4"}))
        );
        let loads = svc.refused_loads(&tab.id).await;
        assert_eq!(loads.len(), 1);
        assert_eq!(loads[0].url, "https://other.test/");
        assert_eq!(loads[0].code, sites::ERROR_SITE_NOT_ALLOWED);

        // The person clicks: theirs again.
        svc.acted(&tab.id, Actor::Person).await;
        fake.emit(
            "Fetch.requestPaused",
            page_load("r5", &target, "https://other.test/"),
            Some(&cdp_session),
        );
        calls_until(&fake, "Fetch.continueRequest", 4).await;

        // Personal tabs and other sessions are not limited.
        svc.set_strict_sites("s1", &[]).await.unwrap();
        svc.acted(&tab.id, Actor::Agent).await;
        assert!(svc.refused_loads(&tab.id).await.is_empty());
        fake.emit(
            "Fetch.requestPaused",
            page_load("r6", &target, "https://other.test/"),
            Some(&cdp_session),
        );
        calls_until(&fake, "Fetch.continueRequest", 5).await;
        assert_eq!(fake.calls("Fetch.failRequest").len(), 1);
    }

    #[tokio::test]
    async fn asking_lets_the_person_allow_or_deny_a_site_for_the_session() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        let on = serde_json::Map::from_iter([(
            crate::settings::BROWSER_AGENT_ASK_SITES.to_string(),
            json!(true),
        )]);
        crate::settings::update(&db.conn, &on).await.unwrap();
        let tab = svc
            .open(session("s1"), Some("https://start.test/"))
            .await
            .unwrap();
        let target = fake.wait_url("https://start.test/").await;
        let cdp_session = fake.lock().targets[&target].session.clone().unwrap();
        svc.acted(&tab.id, Actor::Agent).await;
        let mut events = svc.subscribe();

        fake.emit(
            "Fetch.requestPaused",
            page_load("r1", &target, "https://example.com/a"),
            Some(&cdp_session),
        );
        let first = asked(&mut events).await;
        assert!(
            matches!(&first.kind, UiKind::Site { site, local: false, .. } if site == "example.com:443")
        );
        assert!(
            fake.calls("Fetch.continueRequest").is_empty(),
            "the page waits"
        );
        // A newer load replaces the one waiting.
        fake.emit(
            "Fetch.requestPaused",
            page_load("r2", &target, "https://example.com/b"),
            Some(&cdp_session),
        );
        let second = asked(&mut events).await;
        assert_eq!(
            fake.calls("Fetch.failRequest"),
            vec![json!({"requestId": "r1", "errorReason": "BlockedByClient"})]
        );
        assert_eq!(svc.ui_requests().await, vec![second.clone()]);
        svc.answer(
            &second.request_id,
            &UiAnswer::Site {
                allow: true,
                always: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            fake.calls("Fetch.continueRequest"),
            vec![json!({"requestId": "r2"})]
        );
        fake.emit(
            "Fetch.requestPaused",
            page_load("r3", &target, "https://example.com/c"),
            Some(&cdp_session),
        );
        calls_until(&fake, "Fetch.continueRequest", 2).await;

        fake.emit(
            "Fetch.requestPaused",
            page_load("r4", &target, "http://127.0.0.1:9/"),
            Some(&cdp_session),
        );
        let local = asked(&mut events).await;
        assert!(matches!(&local.kind, UiKind::Site { local: true, .. }));
        svc.answer(
            &local.request_id,
            &UiAnswer::Site {
                allow: false,
                always: false,
            },
        )
        .await
        .unwrap();
        fake.emit(
            "Fetch.requestPaused",
            page_load("r5", &target, "http://127.0.0.1:9/x"),
            Some(&cdp_session),
        );
        let failed = calls_until(&fake, "Fetch.failRequest", 3).await;
        assert_eq!(failed[2]["requestId"], "r5", "denied without asking again");

        // Always: every session, and kept.
        fake.emit(
            "Fetch.requestPaused",
            page_load("r6", &target, "https://docs.test/"),
            Some(&cdp_session),
        );
        let docs = asked(&mut events).await;
        svc.answer(
            &docs.request_id,
            &UiAnswer::Site {
                allow: true,
                always: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            store::approved_sites(&db.conn).await.unwrap(),
            vec!["docs.test:443".to_string()]
        );
        let other = svc
            .open(session("s2"), Some("https://start.test/"))
            .await
            .unwrap();
        svc.acted(&other.id, Actor::Agent).await;
        assert_eq!(
            svc.site_check(&other.id, "https://docs.test/").await,
            Verdict::Allow
        );
        assert_eq!(
            svc.site_check(&other.id, "https://example.com/").await,
            Verdict::Ask
        );
    }

    #[tokio::test]
    async fn the_governor_frees_memory_from_tabs_nobody_uses() {
        use governor::Pressure;
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        let mut ids = Vec::new();
        for _ in 0..4 {
            ids.push(svc.open(session("s1"), None).await.unwrap().id);
        }
        let [viewed, driven, old, older] = [&ids[0], &ids[1], &ids[2], &ids[3]];
        svc.viewing(viewed, true);
        svc.agent_used(driven);
        {
            let now = tokio::time::Instant::now();
            let mut usage = svc.usage_mut();
            for (id, age) in [(viewed, 900), (old, 600), (older, 1200)] {
                usage
                    .last_used
                    .insert(id.clone(), now - Duration::from_secs(age));
            }
        }
        let frozen = |id: &str| svc.usage_mut().frozen.contains(id);
        let discarded = |svc: Arc<BrowserService>, id: String| async move {
            svc.tab(&id).await.unwrap().discarded
        };

        // Under the first rung nothing happens.
        svc.ease(50, 100, Pressure::Normal).await;
        assert!(fake.calls("Page.setWebLifecycleState").is_empty());

        // Freeze the idle ones, discard the least recently used.
        svc.ease(86, 100, Pressure::Normal).await;
        assert!(discarded(svc.clone(), older.clone()).await);
        assert!(frozen(old) && !frozen(viewed) && !frozen(driven));
        assert_eq!(
            fake.calls("Page.setWebLifecycleState"),
            vec![json!({"state": "frozen"})]
        );
        assert!(!discarded(svc.clone(), viewed.clone()).await);

        // Used again: it wakes first.
        svc.session(old).await.unwrap();
        assert!(!frozen(old));
        assert_eq!(
            fake.calls("Page.setWebLifecycleState").last().unwrap(),
            &json!({"state": "active"})
        );

        // At the budget, new tabs wait; below it, they open again.
        svc.ease(100, 100, Pressure::Normal).await;
        let full = svc.open(session("s2"), None).await.unwrap_err();
        assert_eq!(full.code, "browser_memory_full");
        assert!(full.message.contains("session s1 (3)"), "{}", full.message);
        svc.ease(10, 100, Pressure::Normal).await;
        svc.open(session("s2"), None).await.unwrap();

        // Critical: everything not in use goes.
        svc.viewing(viewed, false);
        {
            let past = tokio::time::Instant::now() - Duration::from_secs(600);
            svc.usage_mut().agent_at.insert(driven.clone(), past);
        }
        svc.ease(10, 100, Pressure::Critical).await;
        for id in [viewed, driven, old] {
            assert!(discarded(svc.clone(), id.clone()).await, "{id}");
        }
    }

    #[tokio::test]
    async fn a_session_holds_at_most_its_share_of_tabs() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        for _ in 0..governor::MAX_SESSION_TABS {
            svc.open(session("s1"), None).await.unwrap();
        }
        assert_eq!(
            svc.open(session("s1"), None).await.unwrap_err().code,
            "too_many_tabs"
        );
        svc.open(session("s2"), None).await.unwrap();
        svc.open(TabOwner::Personal, None).await.unwrap();
    }

    #[tokio::test]
    async fn a_page_waiting_on_its_server_holds_nothing_up() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        let tab = svc.open(TabOwner::Personal, None).await.unwrap();
        fake.lock().hold_navigations = true;
        let mut events = svc.subscribe();
        tokio::time::timeout(
            Duration::from_secs(2),
            svc.navigate(&tab.id, Navigation::Url("https://example.com/".into())),
        )
        .await
        .expect("navigate returns before the page answers")
        .unwrap();

        // Meanwhile its sign-in still reaches a person.
        let target = fake.wait_url("https://example.com/").await;
        let session = fake.lock().targets[&target].session.clone().unwrap();
        fake.emit(
            "Fetch.authRequired",
            json!({"requestId": "r1", "frameId": target, "request": {"url": "https://example.com/"},
                   "authChallenge": {"source": "Server", "origin": "https://example.com",
                                     "scheme": "basic", "realm": ""}}),
            Some(&session),
        );
        let asked = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let BrowserEvent::Ui(r) = events.recv().await.unwrap() {
                    return r;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(asked.tab_id, tab.id);
    }

    #[tokio::test]
    async fn a_download_is_shown_then_named_as_the_page_named_it() {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let svc = service(&fake, &db.conn).await;
        svc.open(TabOwner::Personal, None).await.unwrap();
        // A session the Host does not know has nowhere to save.
        svc.open(session("gone"), None).await.unwrap();
        let behaviors = fake.calls("Browser.setDownloadBehavior");
        assert_eq!(behaviors[0]["behavior"], "allowAndName");
        assert_eq!(behaviors[0]["eventsEnabled"], true);
        assert!(behaviors[0]["downloadPath"].as_str().unwrap().ends_with("Downloads"));
        assert_eq!(behaviors[1]["behavior"], "deny");

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("report.pdf"), "older").unwrap();
        let saved = dir.path().join("0c1d-guid");
        std::fs::write(&saved, "new").unwrap();
        let label = "report-for-the-download-test.pdf";
        fake.emit(
            "Browser.downloadWillBegin",
            json!({"guid": "0c1d-guid", "frameId": "F", "url": "https://example.com/r",
                   "suggestedFilename": label}),
            None,
        );
        fake.emit(
            "Browser.downloadProgress",
            json!({"guid": "0c1d-guid", "totalBytes": 3, "receivedBytes": 2, "state": "inProgress"}),
            None,
        );
        let listed = |want: Option<(u64, Option<u64>)>| async move {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let found = crate::downloads::global()
                        .list()
                        .into_iter()
                        .find(|d| d.label == label)
                        .map(|d| (d.received, d.total));
                    if found == want {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("the download row changes");
        };
        listed(Some((2, Some(3)))).await;

        fake.emit(
            "Browser.downloadProgress",
            json!({"guid": "0c1d-guid", "totalBytes": 3, "receivedBytes": 3, "state": "completed",
                   "filePath": saved}),
            None,
        );
        listed(None).await;
        assert_eq!(std::fs::read_to_string(dir.path().join(label)).unwrap(), "new");
        assert!(!saved.exists());
        assert_eq!(std::fs::read_to_string(dir.path().join("report.pdf")).unwrap(), "older");
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
        let target = fake.wait_url("https://example.com/").await;
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
        fresh.wait_url("https://example.com/").await;
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
        let target = fake.wait_url("https://example.com/").await;
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
        let opener_target = fake.wait_url("https://app.example/").await;

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
    /// The real engine freezes, wakes and discards a tab, and its tree is
    /// measured. Point `ZWIIM_TEST_ENGINE` at a `chrome-headless-shell`.
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "needs ZWIIM_TEST_ENGINE"]
    async fn the_real_engine_is_eased_by_the_governor() {
        let dir = tempfile::tempdir().unwrap();
        let start = super::super::engine::fake::real_engine(dir.path().join("profile"));
        let db = fresh_in_memory_db().await;
        let svc = BrowserService::start(EngineManager::new(start), db.conn.clone(), None)
            .await
            .unwrap();
        let a = svc.open(session("s1"), None).await.unwrap().id;
        let b = svc.open(session("s1"), None).await.unwrap().id;
        let pid = svc.engine.pid().await.expect("the real engine has a pid");
        #[cfg(target_os = "macos")]
        {
            let (bytes, count) = governor::sys::usage(pid);
            assert!(
                bytes > 10 << 20 && count >= 2,
                "{bytes} bytes in {count} processes"
            );
        }
        let _ = pid;
        {
            let old = tokio::time::Instant::now() - Duration::from_secs(600);
            let mut usage = svc.usage_mut();
            usage.last_used.insert(a.clone(), old);
            usage
                .last_used
                .insert(b.clone(), old - Duration::from_secs(60));
        }
        svc.ease(86, 100, governor::Pressure::Normal).await;
        assert!(svc.tab(&b).await.unwrap().discarded);
        assert!(svc.usage_mut().frozen.contains(&a));
        // A frozen page runs no script; woken, it does.
        let (cdp, session) = svc.session(&a).await.unwrap();
        let answer = cdp
            .call_with_timeout(
                "Runtime.evaluate",
                json!({"expression": "1 + 1", "returnByValue": true}),
                Some(&session),
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        assert_eq!(answer["result"]["value"], 2);
        // A discarded tab loads again when used.
        svc.session(&b).await.unwrap();
        assert!(!svc.tab(&b).await.unwrap().discarded);
        svc.govern_once().await;
    }

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
