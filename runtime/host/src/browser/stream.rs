//! Agent View streams (design 7.1, 7.2): a tab's screencast, sent to every
//! viewer as binary frames.
//!
//! `browser.view.open` makes a stream for a tab; a viewer then attaches to it
//! (the desktop over `/zwiim/v1/browser/stream/<stream_id>`). One
//! `Page.startScreencast` runs per watched tab while it has a viewer. Each
//! frame is encoded once and shared: a viewer always takes the newest frame,
//! so a slow viewer skips frames instead of queueing them, and Chromium is
//! acked as soon as the fastest viewer has sent a frame.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, watch};

use super::engine::cdp::Cdp;
use super::service::{BrowserError, BrowserEvent, BrowserService};

/// Frame message format version (7.1.2).
pub const FRAME_VERSION: u8 = 1;
/// Message type of a screencast frame.
pub const MESSAGE_FRAME: u8 = 1;
/// Header bytes before the JPEG.
pub const HEADER_LEN: usize = 34;
/// JPEG quality for a viewer on this Mac, where bytes cost nothing.
const LOCAL_QUALITY: u8 = 70;
/// A stream nobody attached to is forgotten after this.
const UNATTACHED_TTL: Duration = Duration::from_secs(30);

/// The page size a viewer shows, in CSS pixels, and its device pixel ratio.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    #[serde(default = "one")]
    pub scale: f64,
}

fn one() -> f64 {
    1.0
}

impl Viewport {
    /// Refused when it is not a size a page can have.
    pub fn check(&self) -> Result<(), BrowserError> {
        let ok = (100..=8192).contains(&self.width)
            && (100..=8192).contains(&self.height)
            && (0.5..=4.0).contains(&self.scale);
        if ok {
            Ok(())
        } else {
            Err(BrowserError {
                code: "invalid_viewport",
                message: "The viewport must be 100 to 8192 pixels each way, at scale 0.5 to 4"
                    .into(),
            })
        }
    }
}

/// What a screencast frame says about the page it shows.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameMeta {
    pub css_width: u16,
    pub css_height: u16,
    pub scale: f32,
    pub scroll_x: f32,
    pub scroll_y: f32,
}

impl FrameMeta {
    fn of(metadata: &Value) -> FrameMeta {
        let f = |k: &str| metadata[k].as_f64().unwrap_or(0.0);
        let px = |k: &str| f(k).round().clamp(0.0, u16::MAX as f64) as u16;
        FrameMeta {
            css_width: px("deviceWidth"),
            css_height: px("deviceHeight"),
            scale: match f("pageScaleFactor") {
                s if s > 0.0 => s as f32,
                _ => 1.0,
            },
            scroll_x: f("scrollOffsetX") as f32,
            scroll_y: f("scrollOffsetY") as f32,
        }
    }
}

/// One encoded frame message, shared by every viewer.
#[derive(Debug, Clone)]
pub struct Frame {
    pub number: u32,
    pub message: Bytes,
}

/// The binary frame message: the 7.1.2 header, little-endian, then the JPEG.
pub fn encode_frame(number: u32, timestamp_ms: u64, meta: &FrameMeta, jpeg: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + jpeg.len());
    out.push(FRAME_VERSION);
    out.push(MESSAGE_FRAME);
    out.extend_from_slice(&number.to_le_bytes());
    out.extend_from_slice(&timestamp_ms.to_le_bytes());
    out.extend_from_slice(&meta.css_width.to_le_bytes());
    out.extend_from_slice(&meta.css_height.to_le_bytes());
    out.extend_from_slice(&meta.scale.to_le_bytes());
    out.extend_from_slice(&meta.scroll_x.to_le_bytes());
    out.extend_from_slice(&meta.scroll_y.to_le_bytes());
    out.extend_from_slice(&(jpeg.len() as u32).to_le_bytes());
    out.extend_from_slice(jpeg);
    out
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn stream_not_found() -> BrowserError {
    BrowserError {
        code: "stream_not_found",
        message: "no such browser stream".into(),
    }
}

struct StreamEntry {
    tab_id: String,
    opened_at: Instant,
    attached: bool,
    /// Ends the attached viewer.
    close: watch::Sender<bool>,
}

/// A tab's running screencast.
struct Cast {
    id: u64,
    frames: watch::Receiver<Option<Frame>>,
    sent: mpsc::UnboundedSender<u32>,
    viewers: usize,
    /// The size to cast at; a change restarts the screencast at it.
    viewport: watch::Sender<Option<Viewport>>,
    stop: watch::Sender<bool>,
}

#[derive(Default)]
struct HubInner {
    streams: HashMap<String, StreamEntry>,
    casts: HashMap<String, Cast>,
    viewports: HashMap<String, Viewport>,
    next_cast: u64,
}

/// Every tab's streams and screencasts.
pub struct StreamHub {
    me: Weak<StreamHub>,
    browser: Arc<BrowserService>,
    inner: Mutex<HubInner>,
}

/// An attached viewer of one stream. Dropping it detaches.
pub struct Viewer {
    hub: Arc<StreamHub>,
    stream_id: String,
    tab_id: String,
    frames: watch::Receiver<Option<Frame>>,
    sent: mpsc::UnboundedSender<u32>,
    closed: watch::Receiver<bool>,
}

impl StreamHub {
    /// A hub over `browser`; it ends the streams of tabs that close.
    pub fn new(browser: Arc<BrowserService>) -> Arc<Self> {
        let hub = Arc::new_cyclic(|me| StreamHub {
            me: me.clone(),
            browser: browser.clone(),
            inner: Mutex::new(HubInner::default()),
        });
        tokio::spawn(follow_tabs(Arc::downgrade(&hub), browser.subscribe()));
        hub
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HubInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A stream of `tab_id` shown at `viewport`. Loads a discarded tab.
    pub async fn open(&self, tab_id: &str, viewport: Viewport) -> Result<String, BrowserError> {
        self.resize(tab_id, viewport).await?;
        let stream_id = uuid::Uuid::new_v4().to_string();
        let mut inner = self.lock();
        inner
            .streams
            .retain(|_, s| s.attached || s.opened_at.elapsed() < UNATTACHED_TTL);
        inner.streams.insert(
            stream_id.clone(),
            StreamEntry {
                tab_id: tab_id.to_string(),
                opened_at: Instant::now(),
                attached: false,
                close: watch::channel(false).0,
            },
        );
        Ok(stream_id)
    }

    /// Show `tab_id` at `viewport`, and cast it at that size.
    pub async fn resize(&self, tab_id: &str, viewport: Viewport) -> Result<(), BrowserError> {
        viewport.check()?;
        let (cdp, session) = self.browser.session(tab_id).await?;
        set_viewport(&cdp, &session, &viewport).await?;
        let mut inner = self.lock();
        inner.viewports.insert(tab_id.to_string(), viewport);
        if let Some(cast) = inner.casts.get(tab_id) {
            cast.viewport.send_if_modified(|v| {
                let changed = *v != Some(viewport);
                *v = Some(viewport);
                changed
            });
        }
        Ok(())
    }

    /// End a stream, and its viewer if one is attached.
    pub fn close(&self, stream_id: &str) -> Result<(), BrowserError> {
        let entry = self
            .lock()
            .streams
            .remove(stream_id)
            .ok_or_else(stream_not_found)?;
        let _ = entry.close.send(true);
        Ok(())
    }

    /// Attach the one viewer a stream has, starting the tab's screencast if
    /// it has none.
    pub async fn attach(&self, stream_id: &str) -> Result<Viewer, BrowserError> {
        let tab_id = {
            let inner = self.lock();
            let entry = inner.streams.get(stream_id).ok_or_else(stream_not_found)?;
            if entry.attached {
                return Err(BrowserError {
                    code: "stream_attached",
                    message: "the browser stream already has a viewer".into(),
                });
            }
            entry.tab_id.clone()
        };
        let (cdp, session) = self.browser.session(&tab_id).await?;
        let hub = self.me.upgrade().ok_or_else(stream_not_found)?;
        let mut inner = self.lock();
        let viewport = inner.viewports.get(&tab_id).copied();
        let HubInner {
            streams,
            casts,
            next_cast,
            ..
        } = &mut *inner;
        let entry = streams.get_mut(stream_id).ok_or_else(stream_not_found)?;
        if entry.attached {
            return Err(BrowserError {
                code: "stream_attached",
                message: "the browser stream already has a viewer".into(),
            });
        }
        entry.attached = true;
        let closed = entry.close.subscribe();
        let cast = casts.entry(tab_id.clone()).or_insert_with(|| {
            *next_cast += 1;
            start_cast(
                Arc::downgrade(&hub),
                *next_cast,
                tab_id.clone(),
                cdp,
                session,
                viewport,
            )
        });
        cast.viewers += 1;
        Ok(Viewer {
            frames: cast.frames.clone(),
            sent: cast.sent.clone(),
            hub: hub.clone(),
            stream_id: stream_id.to_string(),
            tab_id,
            closed,
        })
    }

    fn detach(&self, stream_id: &str, tab_id: &str) {
        let mut inner = self.lock();
        inner.streams.remove(stream_id);
        let last = inner.casts.get_mut(tab_id).is_some_and(|cast| {
            cast.viewers = cast.viewers.saturating_sub(1);
            cast.viewers == 0
        });
        if last {
            if let Some(cast) = inner.casts.remove(tab_id) {
                let _ = cast.stop.send(true);
            }
        }
    }

    /// A screencast ended on its own (the engine went, the tab was
    /// replaced): its viewers end too, and reopen.
    fn cast_ended(&self, tab_id: &str, id: u64) {
        let mut inner = self.lock();
        if inner.casts.get(tab_id).is_some_and(|c| c.id == id) {
            inner.casts.remove(tab_id);
        }
    }

    fn tab_closed(&self, tab_id: &str) {
        let mut inner = self.lock();
        inner.viewports.remove(tab_id);
        if let Some(cast) = inner.casts.remove(tab_id) {
            let _ = cast.stop.send(true);
        }
        inner.streams.retain(|_, s| {
            if s.tab_id == tab_id {
                let _ = s.close.send(true);
            }
            s.tab_id != tab_id
        });
    }

    /// Streams open or attached, for tests and diagnostics.
    pub fn stream_count(&self) -> usize {
        self.lock().streams.len()
    }

    /// Whether `tab_id` has a running screencast.
    pub fn casting(&self, tab_id: &str) -> bool {
        self.lock().casts.contains_key(tab_id)
    }
}

impl Viewer {
    pub fn tab_id(&self) -> &str {
        &self.tab_id
    }

    /// The newest frame not yet taken; `None` once the stream ended.
    pub async fn next(&mut self) -> Option<Frame> {
        if *self.closed.borrow() {
            return None;
        }
        loop {
            tokio::select! {
                changed = self.frames.changed() => {
                    changed.ok()?;
                    if let Some(frame) = self.frames.borrow_and_update().clone() {
                        return Some(frame);
                    }
                }
                changed = self.closed.changed() => {
                    if changed.is_err() || *self.closed.borrow() {
                        return None;
                    }
                }
            }
        }
    }

    /// Deliver the viewer's `input` to its tab.
    pub async fn input(&self, input: &super::input::Input) -> Result<(), BrowserError> {
        if let Some(viewport) = input.viewport() {
            return self.hub.resize(&self.tab_id, viewport).await;
        }
        if matches!(
            input,
            super::input::Input::Key { .. } | super::input::Input::Text { .. }
        ) {
            self.hub.browser.person_typed(&self.tab_id).await;
        }
        let (cdp, session) = self.hub.browser.session(&self.tab_id).await?;
        super::input::dispatch(&cdp, &session, input).await
    }

    /// `frame` reached this viewer; the first viewer to say so acks it.
    pub fn sent(&self, frame: &Frame) {
        let _ = self.sent.send(frame.number);
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.hub.detach(&self.stream_id, &self.tab_id);
    }
}

async fn follow_tabs(hub: Weak<StreamHub>, mut events: broadcast::Receiver<BrowserEvent>) {
    loop {
        match events.recv().await {
            Ok(BrowserEvent::Removed { tab_id, .. }) => match hub.upgrade() {
                Some(hub) => hub.tab_closed(&tab_id),
                None => return,
            },
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                if hub.strong_count() == 0 {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

async fn set_viewport(cdp: &Cdp, session: &str, viewport: &Viewport) -> Result<(), BrowserError> {
    cdp.call(
        "Emulation.setDeviceMetricsOverride",
        json!({
            "width": viewport.width,
            "height": viewport.height,
            "deviceScaleFactor": viewport.scale,
            "mobile": false,
        }),
        Some(session),
    )
    .await?;
    Ok(())
}

fn screencast_params(viewport: Option<Viewport>) -> Value {
    let mut params = json!({ "format": "jpeg", "quality": LOCAL_QUALITY });
    if let Some(v) = viewport {
        params["maxWidth"] = json!((v.width as f64 * v.scale).round() as u32);
        params["maxHeight"] = json!((v.height as f64 * v.scale).round() as u32);
    }
    params
}

fn start_cast(
    hub: Weak<StreamHub>,
    id: u64,
    tab_id: String,
    cdp: Cdp,
    session: String,
    viewport: Option<Viewport>,
) -> Cast {
    let (frames_tx, frames) = watch::channel(None);
    let (sent, sent_rx) = mpsc::unbounded_channel();
    let (stop, stop_rx) = watch::channel(false);
    let (viewport, viewport_rx) = watch::channel(viewport);
    tokio::spawn(async move {
        tokio::select! {
            _ = run_cast(&cdp, &session, viewport_rx, frames_tx, sent_rx, stop_rx) => {}
            _ = cdp.closed() => {}
        }
        if let Some(hub) = hub.upgrade() {
            hub.cast_ended(&tab_id, id);
        }
    });
    Cast {
        id,
        frames,
        sent,
        viewers: 0,
        viewport,
        stop,
    }
}

/// Run one screencast until it is stopped or the engine goes.
async fn run_cast(
    cdp: &Cdp,
    session: &str,
    mut viewport: watch::Receiver<Option<Viewport>>,
    frames: watch::Sender<Option<Frame>>,
    mut sent: mpsc::UnboundedReceiver<u32>,
    mut stop: watch::Receiver<bool>,
) {
    let mut events = cdp.subscribe();
    let start = |cdp: &Cdp, viewport: Option<Viewport>| {
        let cdp = cdp.clone();
        let session = session.to_string();
        async move {
            cdp.call(
                "Page.startScreencast",
                screencast_params(viewport),
                Some(&session),
            )
            .await
        }
    };
    let size = *viewport.borrow_and_update();
    if let Err(e) = start(cdp, size).await {
        tracing::warn!(error = %e, "the browser screencast did not start");
        return;
    }
    let mut number: u32 = 0;
    // The frame waiting for a viewer to send it, and Chromium's id for it.
    let mut unacked: Option<(u32, Value)> = None;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => {
                    if event.method != "Page.screencastFrame"
                        || event.session_id.as_deref() != Some(session)
                    {
                        continue;
                    }
                    let Ok(jpeg) = B64.decode(event.params["data"].as_str().unwrap_or_default())
                    else {
                        continue;
                    };
                    number = number.wrapping_add(1);
                    let meta = FrameMeta::of(&event.params["metadata"]);
                    let message = encode_frame(number, now_ms(), &meta, &jpeg);
                    unacked = Some((number, event.params["sessionId"].clone()));
                    frames.send_replace(Some(Frame {
                        number,
                        message: Bytes::from(message),
                    }));
                }
                // A frame was missed and will never be acked: start over.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    unacked = None;
                    let _ = cdp.call("Page.stopScreencast", json!({}), Some(session)).await;
                    let size = *viewport.borrow();
                    if start(cdp, size).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            Some(n) = sent.recv() => {
                if let Some((waiting, ack)) = &unacked {
                    if *waiting == n {
                        let _ = cdp
                            .call("Page.screencastFrameAck", json!({ "sessionId": ack }), Some(session))
                            .await;
                        unacked = None;
                    }
                }
            }
            Ok(()) = viewport.changed() => {
                let size = *viewport.borrow_and_update();
                unacked = None;
                let _ = cdp.call("Page.stopScreencast", json!({}), Some(session)).await;
                if start(cdp, size).await.is_err() {
                    return;
                }
            }
            _ = stop.changed() => {
                let _ = cdp.call("Page.stopScreencast", json!({}), Some(session)).await;
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::engine::fake::FakeEngine;
    use crate::browser::engine::manager::EngineManager;
    use crate::browser::store::TabOwner;
    use crate::db::test_helpers::fresh_in_memory_db;

    const VIEW: Viewport = Viewport {
        width: 800,
        height: 600,
        scale: 2.0,
    };

    struct Setup {
        fake: FakeEngine,
        browser: Arc<BrowserService>,
        hub: Arc<StreamHub>,
        tab: String,
        session: String,
        _db: crate::db::AppDatabase,
    }

    async fn setup() -> Setup {
        let fake = FakeEngine::default();
        let db = fresh_in_memory_db().await;
        let browser =
            BrowserService::start(EngineManager::new(fake.start_fn()), db.conn.clone(), None)
                .await
                .unwrap();
        let tab = browser.open(TabOwner::Personal, None).await.unwrap();
        let (_, session) = browser.session(&tab.id).await.unwrap();
        let hub = StreamHub::new(browser.clone());
        Setup {
            fake,
            browser,
            hub,
            tab: tab.id,
            session,
            _db: db,
        }
    }

    fn paint(fake: &FakeEngine, session: &str, ack: u64, jpeg: &[u8]) {
        fake.emit(
            "Page.screencastFrame",
            json!({
                "data": B64.encode(jpeg),
                "sessionId": ack,
                "metadata": {
                    "offsetTop": 0, "pageScaleFactor": 1, "deviceWidth": 800, "deviceHeight": 600,
                    "scrollOffsetX": 0, "scrollOffsetY": 120.5, "timestamp": 1.0
                }
            }),
            Some(session),
        );
    }

    async fn eventually(mut check: impl FnMut() -> bool) {
        for _ in 0..200 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("condition never held");
    }

    #[test]
    fn frames_carry_the_header_then_the_jpeg() {
        let meta = FrameMeta {
            css_width: 800,
            css_height: 600,
            scale: 1.5,
            scroll_x: 2.0,
            scroll_y: 120.5,
        };
        let m = encode_frame(7, 1_700_000_000_123, &meta, b"JPEG");
        assert_eq!(m.len(), HEADER_LEN + 4);
        assert_eq!(&m[..2], &[1, 1]);
        assert_eq!(u32::from_le_bytes(m[2..6].try_into().unwrap()), 7);
        assert_eq!(
            u64::from_le_bytes(m[6..14].try_into().unwrap()),
            1_700_000_000_123
        );
        assert_eq!(u16::from_le_bytes(m[14..16].try_into().unwrap()), 800);
        assert_eq!(u16::from_le_bytes(m[16..18].try_into().unwrap()), 600);
        assert_eq!(f32::from_le_bytes(m[18..22].try_into().unwrap()), 1.5);
        assert_eq!(f32::from_le_bytes(m[22..26].try_into().unwrap()), 2.0);
        assert_eq!(f32::from_le_bytes(m[26..30].try_into().unwrap()), 120.5);
        assert_eq!(u32::from_le_bytes(m[30..34].try_into().unwrap()), 4);
        assert_eq!(&m[34..], b"JPEG");
    }

    #[tokio::test]
    async fn a_viewer_gets_frames_and_the_first_send_acks_them() {
        let Setup {
            fake,
            hub,
            tab,
            session,
            ..
        } = setup().await;
        let stream = hub.open(&tab, VIEW).await.unwrap();
        let metrics = fake.calls("Emulation.setDeviceMetricsOverride");
        assert_eq!(metrics.last().unwrap()["width"], 800);
        assert_eq!(metrics.last().unwrap()["deviceScaleFactor"], 2.0);
        // Nothing is cast before a viewer attaches.
        assert!(fake.calls("Page.startScreencast").is_empty());

        let mut viewer = hub.attach(&stream).await.unwrap();
        eventually(|| !fake.calls("Page.startScreencast").is_empty()).await;
        let start = &fake.calls("Page.startScreencast")[0];
        assert_eq!(start["format"], "jpeg");
        assert_eq!(start["maxWidth"], 1600);
        assert_eq!(start["maxHeight"], 1200);

        paint(&fake, &session, 41, b"one");
        let frame = viewer.next().await.unwrap();
        assert_eq!(frame.number, 1);
        assert_eq!(&frame.message[HEADER_LEN..], b"one");
        assert_eq!(
            f32::from_le_bytes(frame.message[26..30].try_into().unwrap()),
            120.5
        );
        assert!(fake.calls("Page.screencastFrameAck").is_empty());
        viewer.sent(&frame);
        eventually(|| fake.calls("Page.screencastFrameAck") == vec![json!({ "sessionId": 41 })])
            .await;
    }

    #[tokio::test]
    async fn a_slow_viewer_skips_to_the_newest_frame() {
        let Setup {
            fake,
            hub,
            tab,
            session,
            ..
        } = setup().await;
        let fast_stream = hub.open(&tab, VIEW).await.unwrap();
        let slow_stream = hub.open(&tab, VIEW).await.unwrap();
        let mut fast = hub.attach(&fast_stream).await.unwrap();
        let mut slow = hub.attach(&slow_stream).await.unwrap();
        eventually(|| !fake.calls("Page.startScreencast").is_empty()).await;
        // One screencast serves both.
        assert_eq!(fake.calls("Page.startScreencast").len(), 1);

        for (ack, jpeg) in [(1, b"a"), (2, b"b"), (3, b"c")] {
            paint(&fake, &session, ack, jpeg);
            let frame = fast.next().await.unwrap();
            assert_eq!(&frame.message[HEADER_LEN..], jpeg);
            fast.sent(&frame);
            let acks = ack as usize;
            eventually(|| fake.calls("Page.screencastFrameAck").len() == acks).await;
        }
        // The slow viewer never queued the old ones.
        let frame = slow.next().await.unwrap();
        assert_eq!(&frame.message[HEADER_LEN..], b"c");
        assert_eq!(frame.number, 3);
    }

    #[tokio::test]
    async fn the_screencast_stops_with_the_last_viewer() {
        let Setup { fake, hub, tab, .. } = setup().await;
        let a = hub.open(&tab, VIEW).await.unwrap();
        let b = hub.open(&tab, VIEW).await.unwrap();
        let first = hub.attach(&a).await.unwrap();
        let second = hub.attach(&b).await.unwrap();
        assert_eq!(hub.attach(&a).await.err().unwrap().code, "stream_attached");
        drop(first);
        assert!(hub.casting(&tab));
        drop(second);
        assert!(!hub.casting(&tab));
        eventually(|| fake.calls("Page.stopScreencast").len() == 1).await;
        assert_eq!(hub.stream_count(), 0);
        assert_eq!(hub.attach(&a).await.err().unwrap().code, "stream_not_found");
    }

    #[tokio::test]
    async fn closing_a_stream_or_its_tab_ends_the_viewer() {
        let Setup {
            browser, hub, tab, ..
        } = setup().await;
        let a = hub.open(&tab, VIEW).await.unwrap();
        let mut viewer = hub.attach(&a).await.unwrap();
        hub.close(&a).unwrap();
        assert!(viewer.next().await.is_none());
        assert_eq!(hub.close(&a).unwrap_err().code, "stream_not_found");
        drop(viewer);

        let b = hub.open(&tab, VIEW).await.unwrap();
        let mut viewer = hub.attach(&b).await.unwrap();
        browser.close(&tab).await.unwrap();
        assert!(viewer.next().await.is_none());
    }

    #[tokio::test]
    async fn a_crashed_engine_ends_the_viewers() {
        let Setup { fake, hub, tab, .. } = setup().await;
        let a = hub.open(&tab, VIEW).await.unwrap();
        let mut viewer = hub.attach(&a).await.unwrap();
        eventually(|| hub.casting(&tab)).await;
        fake.crash();
        assert!(viewer.next().await.is_none());
        eventually(|| !hub.casting(&tab)).await;
    }

    #[tokio::test]
    async fn a_new_viewport_resizes_the_running_screencast() {
        let Setup {
            fake,
            hub,
            tab,
            session,
            ..
        } = setup().await;
        let a = hub.open(&tab, VIEW).await.unwrap();
        let mut viewer = hub.attach(&a).await.unwrap();
        eventually(|| fake.calls("Page.startScreencast").len() == 1).await;
        let wide = Viewport {
            width: 1280,
            height: 800,
            scale: 1.0,
        };
        hub.open(&tab, wide).await.unwrap();
        eventually(|| fake.calls("Page.startScreencast").len() == 2).await;
        assert_eq!(fake.calls("Page.startScreencast")[1]["maxWidth"], 1280);
        assert_eq!(fake.calls("Page.stopScreencast").len(), 1);
        // The first viewer keeps watching.
        paint(&fake, &session, 1, b"wide");
        assert_eq!(&viewer.next().await.unwrap().message[HEADER_LEN..], b"wide");
    }

    /// Real Chromium paints a page into frames, and keeps painting once
    /// they are acked. Point `ZWIIM_TEST_ENGINE` at `chrome-headless-shell`.
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "needs ZWIIM_TEST_ENGINE"]
    async fn the_real_engine_streams_frames() {
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_in_memory_db().await;
        let engine = EngineManager::new(crate::browser::engine::fake::real_engine(
            dir.path().join("profile"),
        ));
        let browser = BrowserService::start(engine, db.conn.clone(), None)
            .await
            .unwrap();
        let tab = browser.open(TabOwner::Personal, None).await.unwrap();
        let hub = StreamHub::new(browser.clone());
        let stream = hub.open(&tab.id, VIEW).await.unwrap();
        let mut viewer = hub.attach(&stream).await.unwrap();
        let (cdp, session) = browser.session(&tab.id).await.unwrap();
        cdp.call(
            "Runtime.evaluate",
            json!({ "expression": "document.body.style.background='red'; document.body.innerHTML='<h1>one</h1>'" }),
            Some(&session),
        )
        .await
        .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(10), viewer.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.message[0], FRAME_VERSION);
        let jpeg = &first.message[HEADER_LEN..];
        assert_eq!(&jpeg[..2], &[0xff, 0xd8], "a JPEG");
        let image = image::load_from_memory(jpeg).unwrap();
        // Captured at the engine's forced scale of 2.
        assert_eq!((image.width(), image.height()), (1600, 1200));
        assert_eq!(
            u16::from_le_bytes(first.message[14..16].try_into().unwrap()),
            800
        );
        viewer.sent(&first);

        // After the ack, a change paints a new frame.
        cdp.call(
            "Runtime.evaluate",
            json!({ "expression": "document.body.innerHTML='<h1>two</h1>'" }),
            Some(&session),
        )
        .await
        .unwrap();
        let second = tokio::time::timeout(Duration::from_secs(10), viewer.next())
            .await
            .unwrap()
            .unwrap();
        assert!(second.number > first.number);
        drop(viewer);
        browser.stop_engine().await;
    }

    /// Real Chromium takes a viewer's click, keys and text.
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "needs ZWIIM_TEST_ENGINE"]
    async fn the_real_engine_takes_input() {
        use crate::browser::input::Input;
        let dir = tempfile::tempdir().unwrap();
        let db = fresh_in_memory_db().await;
        let engine = EngineManager::new(crate::browser::engine::fake::real_engine(
            dir.path().join("profile"),
        ));
        let browser = BrowserService::start(engine, db.conn.clone(), None)
            .await
            .unwrap();
        let tab = browser.open(TabOwner::Personal, None).await.unwrap();
        let hub = StreamHub::new(browser.clone());
        let stream = hub.open(&tab.id, VIEW).await.unwrap();
        let viewer = hub.attach(&stream).await.unwrap();
        let (cdp, session) = browser.session(&tab.id).await.unwrap();
        let eval = |expression: &'static str| {
            let cdp = cdp.clone();
            let session = session.clone();
            async move {
                cdp.call(
                    "Runtime.evaluate",
                    json!({ "expression": expression, "returnByValue": true }),
                    Some(&session),
                )
                .await
                .unwrap()["result"]["value"]
                    .clone()
            }
        };
        eval(
            "document.body.innerHTML = '<form onsubmit=\"document.title=\\'sent \\'+f.value; return false\"><input id=f style=\"position:absolute;left:0;top:0;width:400px;height:40px\"></form>'",
        )
        .await;
        let parse = |v: Value| serde_json::from_value::<Input>(v).unwrap();
        for v in [
            json!({"type": "mouse", "action": "down", "x": 50, "y": 20, "button": "left", "buttons": 1, "click_count": 1}),
            json!({"type": "mouse", "action": "up", "x": 50, "y": 20, "button": "left", "click_count": 1}),
            json!({"type": "key", "action": "down", "key": "a", "code": "KeyA", "text": "a", "key_code": 65}),
            json!({"type": "key", "action": "up", "key": "a", "code": "KeyA", "key_code": 65}),
            json!({"type": "text", "text": "bc"}),
            json!({"type": "key", "action": "down", "key": "Backspace", "code": "Backspace", "key_code": 8}),
            json!({"type": "key", "action": "up", "key": "Backspace", "code": "Backspace", "key_code": 8}),
            json!({"type": "key", "action": "down", "key": "Enter", "code": "Enter", "key_code": 13}),
            json!({"type": "key", "action": "up", "key": "Enter", "code": "Enter", "key_code": 13}),
        ] {
            viewer.input(&parse(v)).await.unwrap();
        }
        assert_eq!(eval("document.activeElement.id").await, "f");
        assert_eq!(eval("f.value").await, "ab");
        assert_eq!(eval("document.title").await, "sent ab");
        // Agents hold off while the person types (design 10.3).
        assert!(browser.last_typed(&tab.id).await.is_some());
        drop(viewer);
        browser.stop_engine().await;
    }

    #[tokio::test]
    async fn bad_viewports_and_tabs_are_refused() {
        let Setup { hub, tab, .. } = setup().await;
        let tiny = Viewport { width: 10, ..VIEW };
        assert_eq!(
            hub.open(&tab, tiny).await.unwrap_err().code,
            "invalid_viewport"
        );
        assert_eq!(
            hub.open("nope", VIEW).await.unwrap_err().code,
            "tab_not_found"
        );
    }
}
