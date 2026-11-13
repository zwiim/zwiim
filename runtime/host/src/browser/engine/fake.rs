//! A stand-in engine for tests: just enough of CDP's `Target`, `Page` and
//! `Emulation` domains to drive the tab layer, with every call recorded.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::FutureExt;
use serde_json::{json, Value};
use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::{mpsc, oneshot};

use super::cdp::Cdp;
use super::manager::{start_fn, EngineError, Running, StartFn};

pub const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/157.0.0.0 Safari/537.36";

#[derive(Debug, Clone, Default)]
pub struct FakeTarget {
    pub context: Option<String>,
    pub session: Option<String>,
    pub history: Vec<String>,
    pub index: usize,
    pub title: String,
}

impl FakeTarget {
    pub fn url(&self) -> &str {
        &self.history[self.index]
    }
}

#[derive(Default)]
pub struct FakeState {
    pub starts: usize,
    /// `(method, params, sessionId)` of every call, in order.
    pub calls: Vec<(String, Value, Option<String>)>,
    pub contexts: Vec<String>,
    pub targets: HashMap<String, FakeTarget>,
    /// Cookies per browser context.
    pub cookies: HashMap<String, Vec<Value>>,
    /// `Page.navigate` never answers, as for a page waiting on a sign-in.
    pub hold_navigations: bool,
    /// `Page.navigate` fails with this `errorText`, as for an unknown host.
    pub nav_error: Option<String>,
    next: usize,
    /// The running engine's event sender and exit switch.
    events: Option<mpsc::UnboundedSender<Value>>,
    exit: Option<oneshot::Sender<()>>,
}

/// Shared handle to the fake: the test inspects it and makes it act.
#[derive(Clone, Default)]
pub struct FakeEngine {
    pub state: Arc<Mutex<FakeState>>,
}

impl FakeEngine {
    pub fn start_fn(&self) -> StartFn {
        let fake = self.clone();
        start_fn(move || {
            let fake = fake.clone();
            async move { Ok::<_, EngineError>(fake.start()) }
        })
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap()
    }

    pub fn calls(&self, method: &str) -> Vec<Value> {
        self.lock()
            .calls
            .iter()
            .filter(|(m, _, _)| m == method)
            .map(|(_, p, _)| p.clone())
            .collect()
    }

    /// Send an event as the engine would.
    pub fn emit(&self, method: &str, params: Value, session: Option<&str>) {
        let mut msg = json!({ "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        if let Some(tx) = &self.lock().events {
            let _ = tx.send(msg);
        }
    }

    /// The page changed its own title.
    pub fn set_title(&self, target_id: &str, title: &str) {
        let info = {
            let mut s = self.lock();
            let t = s.targets.get_mut(target_id).unwrap();
            t.title = title.to_string();
            target_info(target_id, t)
        };
        self.emit(
            "Target.targetInfoChanged",
            json!({ "targetInfo": info }),
            None,
        );
    }

    /// The page in `opener` opens a window at `url`. Returns its target id.
    pub fn open_popup(&self, opener: &str, url: &str) -> String {
        let (id, info) = {
            let mut s = self.lock();
            s.next += 1;
            let id = format!("target-{}", s.next);
            let target = FakeTarget {
                context: s.targets[opener].context.clone(),
                history: vec![url.to_string()],
                ..Default::default()
            };
            let mut info = target_info(&id, &target);
            info["openerId"] = json!(opener);
            s.targets.insert(id.clone(), target);
            (id, info)
        };
        self.emit("Target.targetCreated", json!({ "targetInfo": info }), None);
        id
    }

    /// The engine process dies.
    pub fn crash(&self) {
        let exit = {
            let mut s = self.lock();
            s.targets.clear();
            s.contexts.clear();
            s.cookies.clear();
            s.events = None;
            s.exit.take()
        };
        if let Some(exit) = exit {
            let _ = exit.send(());
        }
    }

    pub fn target_of_url(&self, url: &str) -> Option<String> {
        self.lock()
            .targets
            .iter()
            .find(|(_, t)| t.url() == url)
            .map(|(id, _)| id.clone())
    }

    /// The target at `url`, once a navigation (which the service starts
    /// without waiting) has got there.
    pub async fn wait_url(&self, url: &str) -> String {
        let found = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Some(id) = self.target_of_url(url) {
                    return id;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await;
        found.unwrap_or_else(|_| panic!("no page reached {url}"))
    }

    fn start(&self) -> Running {
        let (to_engine, from_host) = duplex(1 << 20);
        let (to_host, from_engine) = duplex(1 << 20);
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (exit_tx, exit_rx) = oneshot::channel();
        {
            let mut s = self.lock();
            s.starts += 1;
            s.events = Some(events_tx.clone());
            s.exit = Some(exit_tx);
        }
        tokio::spawn(self.clone().serve(from_host, to_host, events_tx, events_rx));
        Running {
            cdp: Cdp::new(from_engine, to_engine),
            version: "fake".into(),
            exited: async move {
                let _ = exit_rx.await;
            }
            .boxed(),
        }
    }

    async fn serve(
        self,
        from_host: DuplexStream,
        mut to_host: DuplexStream,
        events_tx: mpsc::UnboundedSender<Value>,
        mut events_rx: mpsc::UnboundedReceiver<Value>,
    ) {
        let mut requests = BufReader::new(from_host);
        let mut buf = Vec::new();
        loop {
            tokio::select! {
                n = requests.read_until(0, &mut buf) => {
                    if n.unwrap_or(0) == 0 { return; }
                    buf.pop();
                    let req: Value = serde_json::from_slice(&buf).unwrap();
                    buf.clear();
                    let held = req["method"] == "Page.navigate" && self.lock().hold_navigations;
                    let reply = self.answer(&req, &events_tx);
                    if held { continue; }
                    let mut msg = match reply {
                        Ok(result) => json!({ "id": req["id"], "result": result }),
                        Err(message) => json!({ "id": req["id"], "error": { "code": -32000, "message": message } }),
                    };
                    if let Some(s) = req.get("sessionId") { msg["sessionId"] = s.clone(); }
                    if write(&mut to_host, &msg).await.is_err() { return; }
                }
                Some(event) = events_rx.recv() => {
                    if write(&mut to_host, &event).await.is_err() { return; }
                }
            }
        }
    }

    /// One call's result; events it causes go out after the reply.
    fn answer(&self, req: &Value, events: &mpsc::UnboundedSender<Value>) -> Result<Value, String> {
        let method = req["method"].as_str().unwrap_or_default().to_string();
        let params = req["params"].clone();
        let session = req["sessionId"].as_str().map(str::to_string);
        let mut s = self.lock();
        s.calls
            .push((method.clone(), params.clone(), session.clone()));
        let emit = |m: &str, p: Value| {
            let _ = events.send(json!({ "method": m, "params": p }));
        };
        let target_of_session = |s: &FakeState| {
            s.targets
                .iter()
                .find(|(_, t)| t.session == session)
                .map(|(id, _)| id.clone())
                .ok_or_else(|| "Session with given id not found.".to_string())
        };
        match method.as_str() {
            "Browser.getVersion" => {
                Ok(json!({ "product": "HeadlessChrome/157.0.8103.78", "userAgent": USER_AGENT }))
            }
            "Target.setDiscoverTargets" | "Browser.setDownloadBehavior" | "Browser.setPermission" => {
                Ok(json!({}))
            }
            "Target.createBrowserContext" => {
                s.next += 1;
                let id = format!("ctx-{}", s.next);
                s.contexts.push(id.clone());
                Ok(json!({ "browserContextId": id }))
            }
            "Target.disposeBrowserContext" => {
                let ctx = params["browserContextId"].as_str().map(str::to_string);
                s.contexts.retain(|c| Some(c) != ctx.as_ref());
                let gone: Vec<String> = s
                    .targets
                    .iter()
                    .filter(|(_, t)| t.context == ctx)
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in gone {
                    s.targets.remove(&id);
                    emit("Target.targetDestroyed", json!({ "targetId": id }));
                }
                Ok(json!({}))
            }
            "Target.createTarget" => {
                s.next += 1;
                let id = format!("target-{}", s.next);
                let target = FakeTarget {
                    context: params["browserContextId"].as_str().map(str::to_string),
                    history: vec![params["url"].as_str().unwrap_or("about:blank").to_string()],
                    ..Default::default()
                };
                emit(
                    "Target.targetCreated",
                    json!({ "targetInfo": target_info(&id, &target) }),
                );
                s.targets.insert(id.clone(), target);
                Ok(json!({ "targetId": id }))
            }
            "Target.attachToTarget" => {
                let id = params["targetId"].as_str().unwrap_or_default().to_string();
                s.next += 1;
                let session = format!("session-{}", s.next);
                let t = s
                    .targets
                    .get_mut(&id)
                    .ok_or("No target with given id found")?;
                t.session = Some(session.clone());
                Ok(json!({ "sessionId": session }))
            }
            "Target.closeTarget" => {
                let id = params["targetId"].as_str().unwrap_or_default().to_string();
                if s.targets.remove(&id).is_none() {
                    return Err("No target with given id found".into());
                }
                emit("Target.targetDestroyed", json!({ "targetId": id }));
                Ok(json!({ "success": true }))
            }
            "Page.navigate" => {
                let id = target_of_session(&s)?;
                if let Some(error) = s.nav_error.clone() {
                    return Ok(json!({ "frameId": id, "loaderId": "L", "errorText": error }));
                }
                let url = params["url"].as_str().unwrap_or_default().to_string();
                let t = s.targets.get_mut(&id).unwrap();
                t.history.truncate(t.index + 1);
                t.history.push(url);
                t.index = t.history.len() - 1;
                t.title = String::new();
                let info = target_info(&id, t);
                let session = session.clone();
                let send = |m: &str, p: Value| {
                    let _ = events.send(json!({ "method": m, "params": p, "sessionId": session }));
                };
                send("Page.frameStartedLoading", json!({ "frameId": id }));
                emit("Target.targetInfoChanged", json!({ "targetInfo": info }));
                send("Page.frameStoppedLoading", json!({ "frameId": id }));
                Ok(json!({ "frameId": id, "loaderId": "L" }))
            }
            "Storage.setCookies" => {
                let ctx = params["browserContextId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let jar = s.cookies.entry(ctx).or_default();
                for c in params["cookies"].as_array().cloned().unwrap_or_default() {
                    jar.retain(|old| old["name"] != c["name"] || old["domain"] != c["domain"]);
                    jar.push(c);
                }
                Ok(json!({}))
            }
            "Storage.getCookies" => {
                let ctx = params["browserContextId"].as_str().unwrap_or_default();
                let jar = s.cookies.get(ctx).cloned().unwrap_or_default();
                Ok(json!({ "cookies": jar }))
            }
            "Target.getTargetInfo" => {
                let id = params["targetId"].as_str().unwrap_or_default();
                let t = s.targets.get(id).ok_or("No target with given id found")?;
                Ok(json!({ "targetInfo": target_info(id, t) }))
            }
            "Page.getNavigationHistory" => {
                let id = target_of_session(&s)?;
                let t = &s.targets[&id];
                let entries: Vec<Value> = t
                    .history
                    .iter()
                    .enumerate()
                    .map(|(i, url)| json!({ "id": i, "url": url, "title": "" }))
                    .collect();
                Ok(json!({ "currentIndex": t.index, "entries": entries }))
            }
            "Page.navigateToHistoryEntry" => {
                let id = target_of_session(&s)?;
                let t = s.targets.get_mut(&id).unwrap();
                t.index = params["entryId"].as_u64().unwrap_or(0) as usize;
                let info = target_info(&id, t);
                emit("Target.targetInfoChanged", json!({ "targetInfo": info }));
                Ok(json!({}))
            }
            "Runtime.evaluate" if params["expression"] == "document.readyState" => {
                target_of_session(&s)?;
                Ok(json!({ "result": { "type": "string", "value": "complete" } }))
            }
            "Page.handleJavaScriptDialog" => {
                target_of_session(&s)?;
                let _ = events.send(json!({
                    "method": "Page.javascriptDialogClosed",
                    "params": { "result": params["accept"], "userInput": params["promptText"] },
                    "sessionId": session,
                }));
                Ok(json!({}))
            }
            "Page.enable"
            | "Page.reload"
            | "Page.stopLoading"
            | "Runtime.enable"
            | "Network.enable"
            | "Emulation.setUserAgentOverride"
            | "Emulation.setDeviceMetricsOverride"
            | "Page.startScreencast"
            | "Page.stopScreencast"
            | "Page.screencastFrameAck"
            | "Input.dispatchMouseEvent"
            | "Input.dispatchKeyEvent"
            | "Input.insertText"
            | "Fetch.enable"
            | "Fetch.continueRequest"
            | "Fetch.continueWithAuth"
            | "Fetch.failRequest"
            | "Page.setInterceptFileChooserDialog"
            | "DOM.setFileInputFiles" => {
                target_of_session(&s)?;
                Ok(json!({}))
            }
            other => Err(format!("'{other}' wasn't found")),
        }
    }
}

/// The real engine at `ZWIIM_TEST_ENGINE`, for the ignored tests that drive
/// a `chrome-headless-shell` binary.
#[cfg(unix)]
pub fn real_engine(profile: std::path::PathBuf) -> StartFn {
    use super::launch;
    let binary = std::path::PathBuf::from(std::env::var_os("ZWIIM_TEST_ENGINE").unwrap());
    start_fn(move || {
        let options = launch::LaunchOptions {
            binary: binary.clone(),
            profile_dir: profile.clone(),
            renderer_limit: Some(2),
            log_file: None,
        };
        async move {
            let engine = launch::spawn(&options).map_err(|e| EngineError {
                code: "setup_failed",
                message: e.to_string(),
            })?;
            let mut child = engine.child;
            Ok(Running {
                cdp: engine.cdp,
                version: "real".into(),
                exited: async move {
                    let _ = child.wait().await;
                }
                .boxed(),
            })
        }
    })
}

fn target_info(id: &str, t: &FakeTarget) -> Value {
    json!({
        "targetId": id,
        "type": "page",
        "title": if t.title.is_empty() { t.url().to_string() } else { t.title.clone() },
        "url": t.url(),
        "attached": t.session.is_some(),
        "browserContextId": t.context,
    })
}

async fn write(to: &mut DuplexStream, msg: &Value) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(msg).unwrap();
    bytes.push(0);
    to.write_all(&bytes).await
}
