//! The page helper (design 9): `js/ui-helper.js`, run in an isolated world
//! of each tab's top document, reports what the engine does not paint into
//! the stream. The Host waits on it with `Runtime.evaluate` and
//! `awaitPromise`, which needs no `Runtime.enable`, so a personal tab's page
//! cannot tell it is there.
//!
//! What it sends is page data: every message is checked and capped here.

use std::sync::Weak;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::engine::cdp::Cdp;
use super::service::BrowserService;

pub const UI_HELPER_JS: &str = include_str!("js/ui-helper.js");
/// The helper's world; the agent's tools use their own.
pub const WORLD_NAME: &str = "zwiim-ui";
/// The helper answers an idle wait after 25 s; this is the Host's limit.
const WAIT_TIMEOUT: Duration = Duration::from_secs(40);
/// Before trying again after the document went, doubling while it fails.
const RETRY: Duration = Duration::from_millis(150);
const MAX_RETRY: Duration = Duration::from_secs(3);
/// Most messages, and most options, read from one answer.
const MAX_MESSAGES: usize = 20;
const MAX_OPTIONS: usize = 1000;
const MAX_URL: usize = 2048;
const MAX_LABEL: usize = 200;
const MAX_SELECTION: usize = 2000;
const MAX_VALUE: usize = 64;
const DATE_TYPES: &[&str] = &["date", "time", "datetime-local", "month", "week"];

/// A box in the page's CSS pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PageBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One option of a `<select>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectOption {
    pub index: u32,
    pub label: String,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub selected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// The page's context menu was asked for, at `x`, `y` in its CSS pixels.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PageMenu {
    pub tab_id: String,
    pub x: f64,
    pub y: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<String>,
    pub editable: bool,
}

/// A message from the helper, once checked.
#[derive(Debug, Clone, PartialEq)]
pub enum HelperMessage {
    Menu {
        x: f64,
        y: f64,
        link_url: Option<String>,
        image_url: Option<String>,
        selection: Option<String>,
        editable: bool,
    },
    Select {
        id: u64,
        options: Vec<SelectOption>,
        at: PageBox,
    },
    Date {
        id: u64,
        input_type: String,
        value: String,
        min: Option<String>,
        max: Option<String>,
        at: PageBox,
    },
    Title,
}

/// The messages in one answer of `next()`; what does not check out is
/// dropped.
pub fn parse(raw: &str) -> Vec<HelperMessage> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    items
        .iter()
        .take(MAX_MESSAGES)
        .filter_map(message)
        .collect()
}

fn message(m: &Value) -> Option<HelperMessage> {
    let text = |key: &str, max: usize| {
        m[key]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(max).collect::<String>())
    };
    let number = |key: &str| m[key].as_f64().filter(|n| n.is_finite());
    let at = || {
        Some(PageBox {
            x: number("x")?,
            y: number("y")?,
            width: number("width")?.max(0.0),
            height: number("height")?.max(0.0),
        })
    };
    match m["kind"].as_str()? {
        "menu" => Some(HelperMessage::Menu {
            x: number("x")?,
            y: number("y")?,
            link_url: text("link", MAX_URL).filter(|u| web_url(u)),
            image_url: text("image", MAX_URL).filter(|u| web_url(u)),
            selection: text("selection", MAX_SELECTION),
            editable: m["editable"] == true,
        }),
        "select" => {
            let options: Vec<SelectOption> = m["options"]
                .as_array()?
                .iter()
                .take(MAX_OPTIONS)
                .filter_map(|o| {
                    let mut o: SelectOption = serde_json::from_value(o.clone()).ok()?;
                    o.label = o.label.chars().take(MAX_LABEL).collect();
                    o.group = o.group.map(|g| g.chars().take(MAX_LABEL).collect());
                    Some(o)
                })
                .collect();
            Some(HelperMessage::Select {
                id: m["id"].as_u64()?,
                options,
                at: at()?,
            })
        }
        "date" => {
            let input_type = m["input_type"].as_str()?;
            if !DATE_TYPES.contains(&input_type) {
                return None;
            }
            Some(HelperMessage::Date {
                id: m["id"].as_u64()?,
                input_type: input_type.to_string(),
                value: text("value", MAX_VALUE).unwrap_or_default(),
                min: text("min", MAX_VALUE),
                max: text("max", MAX_VALUE),
                at: at()?,
            })
        }
        "title" => Some(HelperMessage::Title),
        _ => None,
    }
}

/// Only addresses a person may copy or open: no `javascript:` or `data:`.
fn web_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
}

/// The expression that gives the helper's waiting element a pick.
pub fn answer_expression(id: u64, value: &Value) -> String {
    format!("globalThis.__zwiimUiHelper.answer({id}, {value})")
}

/// The expression that finds `query` on the page.
pub fn find_expression(query: &str, backwards: bool) -> String {
    format!(
        "globalThis.__zwiimUiHelper.find({}, {backwards})",
        Value::from(query)
    )
}

/// Install the helper in the top document of the tab on `session`, and
/// return its world.
async fn install(cdp: &Cdp, session: &str) -> Option<i64> {
    let tree = cdp
        .call("Page.getFrameTree", json!({}), Some(session))
        .await
        .ok()?;
    let frame = tree["frameTree"]["frame"]["id"].as_str()?.to_string();
    let world = cdp
        .call(
            "Page.createIsolatedWorld",
            json!({ "frameId": frame, "worldName": WORLD_NAME }),
            Some(session),
        )
        .await
        .ok()?;
    let context = world["executionContextId"].as_i64()?;
    cdp.call(
        "Runtime.evaluate",
        json!({ "expression": UI_HELPER_JS, "contextId": context }),
        Some(session),
    )
    .await
    .ok()?;
    Some(context)
}

/// Follow the helper of the tab on `session` for as long as the tab is
/// there: install it in each new document, and hand what it reports to the
/// service.
pub async fn follow(service: Weak<BrowserService>, cdp: Cdp, session: String) {
    let mut retry = RETRY;
    loop {
        let Some(context) = install(&cdp, &session).await else {
            match service.upgrade() {
                Some(s) if s.has_session(&session).await => {
                    tokio::time::sleep(retry).await;
                    retry = (retry * 2).min(MAX_RETRY);
                    continue;
                }
                _ => return,
            }
        };
        retry = RETRY;
        match service.upgrade() {
            Some(s) => s.helper_ready(&session, context).await,
            None => return,
        }
        loop {
            let answer = cdp
                .call_with_timeout(
                    "Runtime.evaluate",
                    json!({
                        "expression": "globalThis.__zwiimUiHelper.next()",
                        "contextId": context,
                        "awaitPromise": true,
                        "returnByValue": true,
                    }),
                    Some(&session),
                    WAIT_TIMEOUT,
                )
                .await;
            let raw = match answer {
                Ok(v) if v.get("exceptionDetails").is_none() => {
                    v["result"]["value"].as_str().unwrap_or("[]").to_string()
                }
                // The document went, or the helper with it.
                _ => break,
            };
            let messages = parse(&raw);
            if messages.is_empty() {
                continue;
            }
            match service.upgrade() {
                Some(s) => s.helper_said(&session, context, messages).await,
                None => return,
            }
        }
        match service.upgrade() {
            Some(s) if s.has_session(&session).await => tokio::time::sleep(RETRY).await,
            _ => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_helper_says_is_checked_and_capped() {
        let long = "x".repeat(5000);
        let raw = json!([
            { "kind": "menu", "x": 10, "y": 20, "link": "https://a.test/x", "image": "javascript:alert(1)",
              "selection": long, "editable": true },
            { "kind": "select", "id": 3, "x": 1, "y": 2, "width": 100, "height": 20,
              "options": [{ "index": 0, "label": "One", "selected": true }, { "index": "bad" },
                          { "index": 1, "label": "Two", "group": "G", "disabled": true }] },
            { "kind": "date", "id": 4, "input_type": "date", "value": "2026-11-13", "min": "2026-01-01",
              "x": 0, "y": 0, "width": 120, "height": 24 },
            { "kind": "date", "id": 5, "input_type": "text", "value": "", "x": 0, "y": 0, "width": 1, "height": 1 },
            { "kind": "menu", "x": "no", "y": 1 },
            { "kind": "unknown" },
            { "kind": "title" }
        ])
        .to_string();
        let messages = parse(&raw);
        assert_eq!(messages.len(), 4, "{messages:?}");
        let HelperMessage::Menu {
            link_url,
            image_url,
            selection,
            editable,
            ..
        } = &messages[0]
        else {
            panic!("{:?}", messages[0]);
        };
        assert_eq!(link_url.as_deref(), Some("https://a.test/x"));
        assert_eq!(*image_url, None, "a script address is never offered");
        assert_eq!(selection.as_ref().unwrap().len(), MAX_SELECTION);
        assert!(*editable);
        let HelperMessage::Select { id, options, at } = &messages[1] else {
            panic!("{:?}", messages[1]);
        };
        assert_eq!((*id, options.len(), at.width), (3, 2, 100.0));
        assert_eq!(options[1].group.as_deref(), Some("G"));
        assert!(
            matches!(&messages[2], HelperMessage::Date { id: 4, value, .. } if value == "2026-11-13")
        );
        assert_eq!(messages[3], HelperMessage::Title);
        assert!(parse("not json").is_empty());
    }

    #[test]
    fn expressions_quote_what_they_pass() {
        assert_eq!(
            find_expression("a\"b", true),
            "globalThis.__zwiimUiHelper.find(\"a\\\"b\", true)"
        );
        assert_eq!(
            answer_expression(7, &json!([2])),
            "globalThis.__zwiimUiHelper.answer(7, [2])"
        );
    }
}

#[cfg(test)]
mod engine_tests {
    use std::sync::Arc;

    use super::*;
    use crate::browser::engine::manager::EngineManager;
    use crate::browser::service::BrowserEvent;
    use crate::browser::store::TabOwner;
    use crate::db::test_helpers::fresh_in_memory_db;

    /// Serve `body` as HTML on a loopback port; its address.
    pub(crate) async fn serve(body: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = sock.read(&mut buf).await;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        format!("http://127.0.0.1:{port}/")
    }

    pub(crate) async fn real_browser(dir: &std::path::Path) -> Arc<BrowserService> {
        let db = fresh_in_memory_db().await;
        let start = crate::browser::engine::fake::real_engine(dir.join("profile"));
        BrowserService::start(EngineManager::new(start), db.conn.clone(), None)
            .await
            .unwrap()
    }

    /// `poke` the page until an event `pick` takes arrives; what it took.
    pub(crate) async fn until<T>(
        events: &mut tokio::sync::broadcast::Receiver<BrowserEvent>,
        mut poke: impl FnMut() -> tokio::task::JoinHandle<()>,
        mut pick: impl FnMut(BrowserEvent) -> Option<T>,
    ) -> T {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                poke();
                let tick = tokio::time::sleep(Duration::from_millis(300));
                tokio::pin!(tick);
                loop {
                    tokio::select! {
                        _ = &mut tick => break,
                        event = events.recv() => {
                            if let Some(found) = event.ok().and_then(&mut pick) {
                                return found;
                            }
                        }
                    }
                }
            }
        })
        .await
        .expect("the helper answered")
    }

    /// A right-click shows where, on what link and selection; a title a
    /// script sets reaches the tab. A personal tab, so without `Runtime`.
    #[tokio::test]
    #[ignore = "needs ZWIIM_TEST_ENGINE"]
    async fn the_helper_reports_the_menu_and_a_new_title() {
        let dir = tempfile::tempdir().unwrap();
        let browser = real_browser(dir.path()).await;
        let url = serve(
            "<title>Old</title><a href=\"/docs\" style=\"display:block;width:200px;height:40px\">Docs</a>\
             <button onclick=\"document.title='New'\" style=\"display:block;width:200px;height:40px\">Rename</button>",
        )
        .await;
        let tab = browser.open(TabOwner::Personal, Some(&url)).await.unwrap();
        let mut events = browser.subscribe();
        let (cdp, session) = browser.session(&tab.id).await.unwrap();
        let click = |button: &'static str, x: f64, y: f64| {
            let cdp = cdp.clone();
            let session = session.clone();
            move || {
                let cdp = cdp.clone();
                let session = session.clone();
                tokio::spawn(async move {
                    for kind in ["mousePressed", "mouseReleased"] {
                        let _ = cdp
                            .call(
                                "Input.dispatchMouseEvent",
                                json!({ "type": kind, "x": x, "y": y, "button": button, "clickCount": 1 }),
                                Some(&session),
                            )
                            .await;
                    }
                })
            }
        };
        let menu = until(&mut events, click("right", 20.0, 20.0), |e| match e {
            BrowserEvent::Menu(m) => Some(m),
            _ => None,
        })
        .await;
        assert_eq!(menu.tab_id, tab.id);
        assert_eq!(menu.link_url, Some(format!("{url}docs")));
        assert!(!menu.editable);

        let renamed = until(&mut events, click("left", 20.0, 60.0), |e| match e {
            BrowserEvent::Tab(t) if t.id == tab.id && t.title == "New" => Some(t),
            _ => None,
        })
        .await;
        assert_eq!(renamed.title, "New");
        browser.stop_engine().await;
    }

    /// A click on a `<select>` or a date input's picker button asks the
    /// person; their pick reaches the page as if they had made it there.
    #[tokio::test]
    #[ignore = "needs ZWIIM_TEST_ENGINE"]
    async fn a_pick_from_the_person_reaches_the_page() {
        use crate::browser::ui::{UiAnswer, UiKind};
        let dir = tempfile::tempdir().unwrap();
        let browser = real_browser(dir.path()).await;
        let url = serve(
            "<select id=s onchange=\"document.title='picked '+this.value\" style=\"display:block;width:200px;height:30px\">\
             <option value=a>Apple</option><optgroup label=More><option value=b>Banana</option><option disabled>Cherry</option></optgroup></select>\
             <input id=d type=date oninput=\"document.title='dated '+this.value\" style=\"display:block;width:200px;height:30px\">",
        )
        .await;
        let tab = browser.open(TabOwner::Personal, Some(&url)).await.unwrap();
        let mut events = browser.subscribe();
        let (cdp, session) = browser.session(&tab.id).await.unwrap();
        let click = |x: f64, y: f64| {
            let cdp = cdp.clone();
            let session = session.clone();
            move || {
                let cdp = cdp.clone();
                let session = session.clone();
                tokio::spawn(async move {
                    for kind in ["mousePressed", "mouseReleased"] {
                        let _ = cdp
                            .call(
                                "Input.dispatchMouseEvent",
                                json!({ "type": kind, "x": x, "y": y, "button": "left", "clickCount": 1 }),
                                Some(&session),
                            )
                            .await;
                    }
                })
            }
        };
        let request = until(&mut events, click(20.0, 15.0), |e| match e {
            BrowserEvent::Ui(r) if matches!(r.kind, UiKind::Select { .. }) => Some(r),
            _ => None,
        })
        .await;
        let UiKind::Select { options, at, .. } = &request.kind else {
            unreachable!()
        };
        let labels: Vec<_> = options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["Apple", "Banana", "Cherry"]);
        assert_eq!(options[1].group.as_deref(), Some("More"));
        assert!(options[2].disabled && options[0].selected);
        assert_eq!(at.width, 200.0);
        let refused = UiAnswer::Select {
            accept: true,
            index: Some(2),
        };
        assert!(browser.answer(&request.request_id, &refused).await.is_err());
        let pick = UiAnswer::Select {
            accept: true,
            index: Some(1),
        };
        browser.answer(&request.request_id, &pick).await.unwrap();
        let title = |want: &'static str| {
            let browser = browser.clone();
            let id = tab.id.clone();
            async move {
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        if browser.tab(&id).await.is_some_and(|t| t.title == want) {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                })
                .await
                .unwrap_or_else(|_| panic!("the title became {want}"));
            }
        };
        title("picked b").await;

        let request = until(&mut events, click(195.0, 45.0), |e| match e {
            BrowserEvent::Ui(r) if matches!(r.kind, UiKind::Date { .. }) => Some(r),
            _ => None,
        })
        .await;
        assert!(matches!(&request.kind, UiKind::Date { input_type, .. } if input_type == "date"));
        let bad = UiAnswer::Date {
            accept: true,
            value: "1); alert(1".into(),
        };
        assert!(browser.answer(&request.request_id, &bad).await.is_err());
        let day = UiAnswer::Date {
            accept: true,
            value: "2026-12-25".into(),
        };
        browser.answer(&request.request_id, &day).await.unwrap();
        title("dated 2026-12-25").await;
        browser.stop_engine().await;
    }
}
