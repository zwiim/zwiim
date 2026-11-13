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
use crate::browser::ui::UiAnswer;
use crate::browser::stream::{StreamHub, Viewport};

type ZResult<T> = Result<Json<T>, ZwiimError>;

impl From<BrowserError> for ZwiimError {
    fn from(e: BrowserError) -> Self {
        let (status, message) = match e.code {
            "tab_not_found" => (StatusCode::NOT_FOUND, "No such browser tab".to_string()),
            "unsupported_url" => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Only http, https and about:blank addresses open in the browser".to_string(),
            ),
            "stream_not_found" => (StatusCode::NOT_FOUND, "No such browser stream".to_string()),
            "stream_attached" => (
                StatusCode::CONFLICT,
                "The browser stream already has a viewer".to_string(),
            ),
            "invalid_viewport" => (StatusCode::BAD_REQUEST, e.message),
            "request_not_found" => (StatusCode::NOT_FOUND, e.message),
            "invalid_answer" => (StatusCode::BAD_REQUEST, e.message),
            "invalid_site" => (StatusCode::BAD_REQUEST, e.message),
            "too_many_tabs" | "browser_memory_full" => (StatusCode::CONFLICT, e.message),
            "page_too_large" => (StatusCode::PAYLOAD_TOO_LARGE, e.message),
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
    let browser = service(&state).await?;
    // A client's navigation is the person's (design 13.2).
    browser
        .acted(&p.tab_id, crate::browser::sites::Actor::Person)
        .await;
    Ok(Json(browser.navigate(&p.tab_id, to).await?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabFindParams {
    pub tab_id: String,
    pub query: String,
    #[serde(default)]
    pub backwards: bool,
}

/// Longest text find in page looks for.
const MAX_FIND: usize = 200;

/// `browser.tab.find {tab_id, query, backwards?}` → `{matches, current}`:
/// highlight `query` on the page and go to its next match, or the one before;
/// an empty query clears it (design 7.7).
pub async fn tab_find(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<TabFindParams>,
) -> ZResult<Value> {
    if p.query.chars().count() > MAX_FIND {
        return Err(ZwiimError::invalid_request(format!(
            "Look for at most {MAX_FIND} characters"
        )));
    }
    let (matches, current) = service(&state)
        .await?
        .find(&p.tab_id, &p.query, p.backwards)
        .await?;
    Ok(Json(json!({ "matches": matches, "current": current })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiAnswerParams {
    pub request_id: String,
    pub answer: UiAnswer,
}

/// Most files one file chooser answer gives a page.
const MAX_CHOSEN_FILES: usize = 100;

/// `browser.ui.answer {request_id, answer}`: answer what a tab is waiting
/// on (design 9). The first answer wins; a later one is `request_not_found`.
pub async fn ui_answer(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(mut p): ZJson<UiAnswerParams>,
) -> ZResult<Value> {
    let browser = service(&state).await?;
    if let UiAnswer::FileChooser {
        accept: true,
        workspace_id,
        paths,
        files,
    } = &mut p.answer
    {
        let request = browser
            .ui_request(&p.request_id)
            .await
            .ok_or_else(|| ZwiimError::from(BrowserError {
                code: "request_not_found",
                message: "That request was already answered or has expired".into(),
            }))?;
        let workspace_id = workspace_id
            .as_deref()
            .ok_or_else(|| ZwiimError::invalid_request("Name the workspace the files are in"))?;
        *files = chosen_files(&state, &request.owner, workspace_id, paths).await?;
    }
    browser.answer(&p.request_id, &p.answer).await?;
    Ok(Json(json!({})))
}

/// `paths` in workspace `workspace_id` as files on disk, for a page's file
/// chooser. A session's tab takes files from its own workspace only; every
/// file must be a file inside the workspace (symlinks too) and not a hidden
/// one a link hides behind another name.
async fn chosen_files(
    state: &AppState,
    owner: &TabOwner,
    workspace_id: &str,
    paths: &[String],
) -> Result<Vec<std::path::PathBuf>, ZwiimError> {
    if paths.is_empty() || paths.len() > MAX_CHOSEN_FILES {
        return Err(ZwiimError::invalid_request(format!(
            "Choose between 1 and {MAX_CHOSEN_FILES} files"
        )));
    }
    let root = files_root(state, owner, workspace_id).await?;
    let root = root.as_path();
    let not_found = || ZwiimError::not_found("File not found");
    let canonical_root = root.canonicalize().map_err(|_| not_found())?;
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let joined = crate::commands::folders::resolve_tree_path(root, path)?;
        let target = joined.canonicalize().map_err(|_| not_found())?;
        if !target.starts_with(&canonical_root)
            || !target.is_file()
            || super::http::link_reaches_hidden(&canonical_root, &target, path)
        {
            return Err(not_found());
        }
        files.push(target);
    }
    Ok(files)
}

/// The root of workspace `workspace_id`, when `owner`'s tab may take files
/// from it: a session's tab only from its session's workspace.
async fn files_root(
    state: &AppState,
    owner: &TabOwner,
    workspace_id: &str,
) -> Result<std::path::PathBuf, ZwiimError> {
    if let TabOwner::Session { session_id } = owner {
        let row = crate::session::resolve(&state.db.conn, session_id).await?;
        let own = crate::workspace::workspace_id_of(&state.db.conn, row.folder_id).await?;
        if own.as_deref() != Some(workspace_id) {
            return Err(ZwiimError::permission_denied(
                "This tab can only take files from its session's workspace",
            ));
        }
    }
    Ok(crate::workspace::resolve(&state.db.conn, workspace_id)
        .await?
        .path
        .into())
}

/// `browser.tab.print {tab_id}` → `{name}`: the page saved as a PDF in the
/// tab's downloads folder (design 7.7).
pub async fn tab_print(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<TabParams>,
) -> ZResult<Value> {
    let name = service(&state).await?.print(&p.tab_id).await?;
    Ok(Json(json!({ "name": name })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabDropParams {
    pub tab_id: String,
    /// Where, in the page's CSS pixels.
    pub x: f64,
    pub y: f64,
    pub workspace_id: String,
    /// Files in the workspace, relative to it.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Files uploaded with `attachments.upload` to the workspace.
    #[serde(default)]
    pub attachment_ids: Vec<String>,
}

/// `browser.tab.drop {tab_id, x, y, workspace_id, paths?, attachment_ids?}`:
/// drop files from the workspace on the page at `(x, y)`, as the person
/// dragging them there (design 7.6).
pub async fn tab_drop(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<TabDropParams>,
) -> ZResult<Value> {
    if !(p.x.is_finite() && p.y.is_finite()) {
        return Err(ZwiimError::invalid_request("Give the point in the page"));
    }
    let browser = service(&state).await?;
    let tab = browser.tab(&p.tab_id).await.ok_or_else(|| {
        ZwiimError::from(BrowserError {
            code: "tab_not_found",
            message: "No such tab".into(),
        })
    })?;
    let mut files = Vec::new();
    if !p.paths.is_empty() {
        files = chosen_files(&state, &tab.owner, &p.workspace_id, &p.paths).await?;
    }
    if !p.attachment_ids.is_empty() {
        let root = files_root(&state, &tab.owner, &p.workspace_id).await?;
        for id in &p.attachment_ids {
            files.push(crate::attachments::resolve(&root, id)?);
        }
    }
    if files.is_empty() || files.len() > MAX_CHOSEN_FILES {
        return Err(ZwiimError::invalid_request(format!(
            "Drop between 1 and {MAX_CHOSEN_FILES} files"
        )));
    }
    browser.drop_files(&p.tab_id, p.x, p.y, &files).await?;
    Ok(Json(json!({})))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SitesParams {
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SitesSetParams {
    pub session_id: String,
    /// Site patterns or pasted addresses; empty removes the list.
    pub sites: Vec<String>,
}

async fn known_session(state: &AppState, session_id: &str) -> Result<(), ZwiimError> {
    crate::session::resolve(&state.db.conn, session_id)
        .await
        .map(|_| ())
        .map_err(|_| ZwiimError::not_found("No such session"))
}

/// `browser.sites.get {session_id}` → `{sites}`: the session's strict site
/// list, empty for none (design 13.2).
pub async fn sites_get(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SitesParams>,
) -> ZResult<Value> {
    known_session(&state, &p.session_id).await?;
    let sites = service(&state).await?.strict_sites(&p.session_id).await;
    Ok(Json(json!({ "sites": sites })))
}

/// `browser.sites.set {session_id, sites}` → `{sites}`: the session's agent
/// opens only these sites and its own dev servers from now on; an empty
/// list lets it open any. Entries come back as kept (`https://x.com/a` is
/// `x.com`); a bad one is `invalid_site`.
pub async fn sites_set(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<SitesSetParams>,
) -> ZResult<Value> {
    known_session(&state, &p.session_id).await?;
    let sites = service(&state)
        .await?
        .set_strict_sites(&p.session_id, &p.sites)
        .await?;
    Ok(Json(json!({ "sites": sites })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownloadLocationParams {
    /// A folder on the Host, or null for the default.
    pub root: Option<String>,
}

async fn download_location(state: &AppState) -> Result<Value, ZwiimError> {
    let root = crate::browser::saving::root(&state.db.conn).await.map_err(|e| {
        tracing::warn!(error = %e, "could not read the browser download folder");
        ZwiimError::internal()
    })?;
    Ok(json!({ "root": root }))
}

/// `browser.downloads.location` → `{root}`: the folder the person chose for
/// browser downloads, or null for the default (design, decision 13).
pub async fn downloads_location(Extension(state): Extension<Arc<AppState>>) -> ZResult<Value> {
    Ok(Json(download_location(&state).await?))
}

/// `browser.downloads.set_location {root}` → `{root}`: every worktree saves
/// to `<root>/zwiim/<repo>/downloads/` from now on, or to its own
/// `.zwiim/browser/` again with null. `root` must be an existing folder.
pub async fn downloads_set_location(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<DownloadLocationParams>,
) -> ZResult<Value> {
    let root = match p.root.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        None => None,
        Some(raw) => {
            let path = std::path::Path::new(raw);
            if !path.is_absolute() {
                return Err(ZwiimError::invalid_input("Choose a folder by its full path"));
            }
            let folder = path
                .canonicalize()
                .ok()
                .filter(|f| f.is_dir())
                .ok_or_else(|| ZwiimError::not_found("Folder not found"))?;
            Some(folder)
        }
    };
    crate::browser::saving::set_root(&state.db.conn, root.as_deref())
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "could not save the browser download folder");
            ZwiimError::internal()
        })?;
    if let Some(browser) = state.browser.get() {
        browser.redirect_downloads().await;
    }
    Ok(Json(download_location(&state).await?))
}

/// `browser.engine.reset` → `{engine}`: clear a failure (`crash_loop` too)
/// so the next tab starts the engine again.
pub async fn engine_reset(Extension(state): Extension<Arc<AppState>>) -> ZResult<Value> {
    let browser = service(&state).await?;
    browser.engine().reset().await;
    let engine = EngineView::from(&*browser.engine().state().borrow());
    Ok(Json(json!({ "engine": engine })))
}

async fn streams(state: &AppState) -> Result<Arc<StreamHub>, ZwiimError> {
    state.browser_streams().await.map_err(|e| {
        tracing::warn!(error = %e, "[zwiim] browser tabs could not be read");
        ZwiimError::internal()
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewOpenParams {
    pub tab_id: String,
    pub viewport: Viewport,
}

/// `browser.view.open {tab_id, viewport}` → `{stream_id}`: a stream of the
/// tab shown at `viewport` (`{width, height, scale?}` in CSS pixels), for
/// one viewer to open at `/zwiim/v1/browser/stream/<stream_id>` within 30 s.
/// Loads a discarded tab.
pub async fn view_open(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<ViewOpenParams>,
) -> ZResult<Value> {
    let stream_id = streams(&state).await?.open(&p.tab_id, p.viewport).await?;
    Ok(Json(json!({ "stream_id": stream_id })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewCloseParams {
    pub stream_id: String,
}

/// `browser.view.close {stream_id}`: ends the stream and its socket.
pub async fn view_close(
    Extension(state): Extension<Arc<AppState>>,
    ZJson(p): ZJson<ViewCloseParams>,
) -> ZResult<Value> {
    streams(&state).await?.close(&p.stream_id)?;
    Ok(Json(json!({})))
}

/// Subprotocol the stream socket negotiates.
pub const STREAM_PROTOCOL: &str = "zwiim-browser-stream";

/// `GET /zwiim/v1/browser/stream/<stream_id>`: the stream's frames, one
/// binary message each (design 7.1.2), and the viewer's input as JSON text
/// (`crate::browser::input::Input`); input that cannot be delivered is
/// answered with an `error` text message. Refused before the upgrade when
/// the stream is unknown or already has its viewer.
pub async fn stream_socket(
    ws: axum::extract::WebSocketUpgrade,
    axum::extract::Path(stream_id): axum::extract::Path<String>,
    Extension(state): Extension<Arc<AppState>>,
    Extension(shutdown): Extension<Arc<crate::web::shutdown::ShutdownSignal>>,
) -> Result<axum::response::Response, ZwiimError> {
    let viewer = streams(&state).await?.attach(&stream_id).await?;
    Ok(ws
        .protocols([STREAM_PROTOCOL])
        .on_upgrade(move |socket| run_stream(socket, viewer, shutdown)))
}

async fn run_stream(
    mut socket: axum::extract::ws::WebSocket,
    mut viewer: crate::browser::stream::Viewer,
    shutdown: Arc<crate::web::shutdown::ShutdownSignal>,
) {
    use axum::extract::ws::Message;
    loop {
        tokio::select! {
            frame = viewer.next() => {
                let Some(frame) = frame else { break };
                if socket.send(Message::Binary(frame.message.clone())).await.is_err() {
                    return;
                }
                viewer.sent(&frame);
            }
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(text))) => {
                    let done = match serde_json::from_str::<crate::browser::input::Input>(&text) {
                        Ok(input) => viewer.input(&input).await.map_err(ZwiimError::from),
                        Err(e) => Err(ZwiimError::invalid_request(format!("Invalid input: {e}"))),
                    };
                    if let Err(e) = done {
                        let frame = json!({
                            "type": "error",
                            "error": { "code": e.code, "message": e.message },
                        });
                        if socket.send(Message::Text(frame.to_string().into())).await.is_err() {
                            return;
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
            },
            _ = shutdown.wait() => break,
        }
    }
    let _ = socket.send(Message::Close(None)).await;
}

/// One socket's `browser.subscribe`: a `browser.snapshot` frame, then each
/// change as `browser.tab.state`, `browser.tab.removed`,
/// `browser.engine.state`, `browser.ui.request`, `browser.ui.done` or
/// `browser.agent.action`. Events carry a tab's whole state, so one that
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
            "ui_requests": self.browser.ui_requests().await,
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
                    Ok(BrowserEvent::Ui(request)) => {
                        Some(json!({ "type": "browser.ui.request", "request": request }))
                    }
                    Ok(BrowserEvent::UiDone { request_id, tab_id }) => Some(json!({
                        "type": "browser.ui.done",
                        "request_id": request_id,
                        "tab_id": tab_id,
                    })),
                    Ok(BrowserEvent::AgentAction(action)) => {
                        Some(json!({ "type": "browser.agent.action", "action": action }))
                    }
                    Ok(BrowserEvent::Menu(menu)) => {
                        Some(json!({ "type": "browser.ui.menu", "menu": menu }))
                    }
                    // Told to the agent that acted (design 10.3); clients
                    // follow downloads in the Host's downloads.
                    Ok(BrowserEvent::Download { .. }) => continue,
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
