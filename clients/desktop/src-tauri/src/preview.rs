//! HTML previews: each preview is its own origin (`zwiim-preview://<id>/`,
//! `http://zwiim-preview.<id>/` on Windows) whose requests the main window's
//! JS answers from the workspace through the Host. The preview page gets no
//! IPC: on Windows it gets Tauri's IPC script too, but the app takes commands
//! only from its own origin and grants no core permissions.

use std::collections::HashMap;
use std::sync::Mutex;

use rand::Rng as _;
use serde::Serialize;
use tauri::http::{header, Response, StatusCode, Uri};

type Body = Response<Vec<u8>>;
type Respond = Box<dyn FnOnce(u16, Body) + Send>;
type ToPage = Box<dyn Fn(PreviewRequest) -> Result<(), ()> + Send + Sync>;

#[derive(Clone, Serialize)]
pub struct PreviewRequest {
    request: u64,
    method: String,
    path: String,
}

#[derive(Default)]
pub struct Previews(Mutex<Inner>);

#[derive(Default)]
struct Inner {
    open: HashMap<String, ToPage>,
    pending: HashMap<u64, (String, Respond)>,
    next: u64,
}

pub enum Route<'a> {
    Respond(u16, &'static str),
    Ask { id: &'a str, path: String },
}

fn is_id(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn route<'a>(method: &str, uri: &'a Uri, known: impl Fn(&str) -> bool) -> Route<'a> {
    let host = uri.host().unwrap_or("");
    let id = host.strip_prefix("zwiim-preview.").unwrap_or(host);
    if !is_id(id) || !known(id) {
        return Route::Respond(404, "Not found");
    }
    if method != "GET" && method != "HEAD" {
        return Route::Respond(405, "Method not allowed");
    }
    let path = uri
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".into());
    Route::Ask { id, path }
}

fn text(status: u16, body: &str) -> Body {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(body.as_bytes().to_vec())
        .unwrap()
}

impl Previews {
    pub fn open_with(
        &self,
        send: impl Fn(PreviewRequest) -> Result<(), ()> + Send + Sync + 'static,
    ) -> String {
        let id: String = rand::thread_rng()
            .gen::<[u8; 16]>()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        self.0
            .lock()
            .unwrap()
            .open
            .insert(id.clone(), Box::new(send));
        id
    }

    /// Hand a preview page's request to its main-window resolver; `respond`
    /// runs once, with its answer or an error.
    pub fn ask_with(
        &self,
        method: &str,
        uri: &Uri,
        respond: impl FnOnce(u16, Body) + Send + 'static,
    ) {
        let mut inner = self.0.lock().unwrap();
        let (id, path) = match route(method, uri, |id| inner.open.contains_key(id)) {
            Route::Respond(status, msg) => return respond(status, text(status, msg)),
            Route::Ask { id, path } => (id.to_string(), path),
        };
        let request = inner.next;
        inner.next += 1;
        // `Channel::send` does not come back into `Previews`, so holding the lock is safe.
        let sent = (inner.open[&id])(PreviewRequest {
            request,
            method: method.into(),
            path,
        });
        if sent.is_err() {
            return respond(502, text(502, "The preview is not connected"));
        }
        inner.pending.insert(request, (id, Box::new(respond)));
    }

    pub fn reply(
        &self,
        request: u64,
        status: u16,
        mime: &str,
        location: &str,
        body: Vec<u8>,
    ) -> Result<(), String> {
        let (_, respond) = self
            .0
            .lock()
            .unwrap()
            .pending
            .remove(&request)
            .ok_or("That request has ended")?;
        let mut res = Response::builder()
            .status(StatusCode::from_u16(status).map_err(|_| "Bad status")?)
            .header(header::CACHE_CONTROL, "no-store");
        if !mime.is_empty() {
            res = res.header(header::CONTENT_TYPE, mime);
        }
        if !location.is_empty() {
            res = res.header(header::LOCATION, location);
        }
        respond(status, res.body(body).map_err(|e| e.to_string())?);
        Ok(())
    }

    /// Forget a preview; its unanswered requests end as not found.
    pub fn close(&self, id: &str) {
        let mut inner = self.0.lock().unwrap();
        inner.open.remove(id);
        let ended: Vec<u64> = inner
            .pending
            .iter()
            .filter(|(_, (p, _))| p == id)
            .map(|(n, _)| *n)
            .collect();
        for n in ended {
            if let Some((_, respond)) = inner.pending.remove(&n) {
                respond(404, text(404, "Not found"));
            }
        }
    }
}

#[tauri::command]
pub fn preview_open(
    previews: tauri::State<'_, Previews>,
    on_request: tauri::ipc::Channel<PreviewRequest>,
) -> String {
    previews.open_with(move |req| on_request.send(req).map_err(|_| ()))
}

#[tauri::command]
pub fn preview_close(previews: tauri::State<'_, Previews>, id: String) {
    previews.close(&id);
}

/// The page's answer: the raw body, with request id, status, type and
/// location in `x-preview-*` headers.
#[tauri::command]
pub fn preview_reply(
    previews: tauri::State<'_, Previews>,
    request: tauri::ipc::Request<'_>,
) -> Result<(), String> {
    let h = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    let id: u64 = h("x-preview-request")
        .parse()
        .map_err(|_| "Missing request id")?;
    let status: u16 = h("x-preview-status")
        .parse()
        .map_err(|_| "Missing status")?;
    let body = match request.body() {
        tauri::ipc::InvokeBody::Raw(b) => b.clone(),
        _ => Vec::new(),
    };
    previews.reply(
        id,
        status,
        &h("x-preview-type"),
        &h("x-preview-location"),
        body,
    )
}

/// Runs in every frame; acts only in a preview. Links to the preview's own
/// origin stay in the frame; links elsewhere open as new windows, which the
/// app sends to the system browser. A custom scheme's `origin` is "null",
/// so origins are compared by protocol and host.
pub const LINKS_JS: &str = r#"(() => {
  const here = location;
  if (here.protocol !== "zwiim-preview:" && !here.hostname.startsWith("zwiim-preview.")) return;
  const same = (u) => u.protocol === here.protocol && u.host === here.host;
  document.addEventListener("click", (e) => {
    const a = e.target instanceof Element ? e.target.closest("a[href]") : null;
    if (!a || e.defaultPrevented) return;
    const url = new URL(a.getAttribute("href"), here.href);
    if (same(url)) {
      if (a.target === "_blank") { e.preventDefault(); location.href = url.href; }
      return;
    }
    if (/^(https?|mailto):$/.test(url.protocol)) { e.preventDefault(); window.open(url.href, "_blank", "noopener"); }
  }, true);
})();"#;

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::http;

    const ID: &str = "0123456789abcdef0123456789abcdef";
    fn uri(s: &str) -> http::Uri {
        s.parse().unwrap()
    }
    fn known(id: &str) -> bool {
        id == ID
    }

    #[test]
    fn routes_requests_by_preview_id() {
        let u = uri(&format!("zwiim-preview://{ID}/docs/report.html?x=1"));
        assert!(
            matches!(route("GET", &u, known), Route::Ask { id, path } if id == ID && path == "/docs/report.html?x=1")
        );
        let u = uri(&format!("http://zwiim-preview.{ID}/a.css"));
        assert!(matches!(route("HEAD", &u, known), Route::Ask { path, .. } if path == "/a.css"));
    }

    #[test]
    fn refuses_other_methods_and_unknown_ids() {
        let u = uri(&format!("zwiim-preview://{ID}/a.html"));
        assert!(matches!(route("POST", &u, known), Route::Respond(405, _)));
        let gone = uri("zwiim-preview://ffffffffffffffffffffffffffffffff/a.html");
        assert!(matches!(route("GET", &gone, known), Route::Respond(404, _)));
        assert!(matches!(
            route("GET", &uri("zwiim-preview://not-an-id/"), |_| true),
            Route::Respond(404, _)
        ));
    }

    #[test]
    fn close_answers_pending_requests_not_found() {
        let previews = Previews::default();
        let answered = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let id = previews.open_with(|_| Ok(()));
        let seen = answered.clone();
        previews.ask_with(
            "GET",
            &uri(&format!("zwiim-preview://{id}/a.html")),
            move |status, _| seen.lock().unwrap().push(status),
        );
        assert!(
            answered.lock().unwrap().is_empty(),
            "waits for the page's answer"
        );
        previews.close(&id);
        assert_eq!(*answered.lock().unwrap(), vec![404]);
        let seen = answered.clone();
        previews.ask_with(
            "GET",
            &uri(&format!("zwiim-preview://{id}/a.html")),
            move |status, _| seen.lock().unwrap().push(status),
        );
        assert_eq!(*answered.lock().unwrap(), vec![404, 404]);
    }
}
