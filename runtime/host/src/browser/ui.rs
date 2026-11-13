//! Browser UI the stream does not show (design 9): Chromium paints none of
//! it into screencast frames, so the Host turns each into a request every
//! client draws with its own UI, and the first answer wins.
//!
//! For now: JavaScript dialogs (`alert`, `confirm`, `prompt`,
//! `beforeunload`), HTTP auth and file choosers. A dialog nobody answers
//! within [`DIALOG_TIMEOUT`] is dismissed, since an open dialog stalls the
//! page and its frames; a sign-in or a file chooser nobody answers within
//! [`AUTH_TIMEOUT`] is cancelled. Credentials go to the engine and nowhere
//! else: never stored or logged. A file chooser gets workspace files only,
//! which the protocol checks and resolves before the answer reaches here.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::helper::{self, PageBox, SelectOption};
use super::service::BrowserError;
use super::store::TabOwner;

pub const DIALOG_TIMEOUT: Duration = Duration::from_secs(60);
/// Longer: nothing stalls while a sign-in or a file chooser waits, and
/// typing a password or finding a file takes time.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(300);

/// Something a tab is waiting on a person for.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiRequest {
    pub request_id: String,
    pub tab_id: String,
    pub owner: TabOwner,
    #[serde(flatten)]
    pub kind: UiKind,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UiKind {
    Dialog {
        dialog: DialogType,
        message: String,
        /// A `prompt`'s default answer.
        #[serde(skip_serializing_if = "Option::is_none")]
        default_text: Option<String>,
        /// The page that asked.
        url: String,
    },
    /// A server (or a proxy) asks for a username and password.
    Auth {
        /// Who asks: `scheme://host:port`.
        origin: String,
        /// `basic`, `digest`, `ntlm`, `negotiate`.
        scheme: String,
        realm: String,
        /// A proxy asks, not the site.
        proxy: bool,
        /// The page being loaded.
        url: String,
        /// The paused request, the engine's own id: never sent.
        #[serde(skip)]
        fetch_request: String,
        #[serde(skip)]
        frame_id: String,
    },
    /// A page's `<input type=file>` asks for files.
    FileChooser {
        /// More than one file may be picked.
        multiple: bool,
        /// The page that asked.
        url: String,
        /// The input, the engine's own id: never sent.
        #[serde(skip)]
        backend_node: i64,
    },
    /// A `<select>`'s list, which the engine does not paint: one option to
    /// pick, at the select's box.
    Select {
        options: Vec<SelectOption>,
        #[serde(flatten)]
        at: PageBox,
        /// The page helper's world and its name for the select: never sent.
        #[serde(skip)]
        context: i64,
        #[serde(skip)]
        element: u64,
    },
    /// A date or time input's picker: `input_type` is the input's `type`
    /// (`date`, `time`, `datetime-local`, `month`, `week`), `value` its
    /// value in the input's own format.
    Date {
        input_type: String,
        value: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        min: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max: Option<String>,
        #[serde(flatten)]
        at: PageBox,
        #[serde(skip)]
        context: i64,
        #[serde(skip)]
        element: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DialogType {
    Alert,
    Confirm,
    Prompt,
    Beforeunload,
}

impl UiKind {
    /// A pick the page helper waits on, which goes with the document.
    pub fn is_pick(&self) -> bool {
        matches!(self, UiKind::Select { .. } | UiKind::Date { .. })
    }
}

/// A person's answer to a [`UiRequest`].
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiAnswer {
    /// OK (or Leave) when `accept`, Cancel (or Stay) otherwise; `text` is a
    /// `prompt`'s answer.
    Dialog {
        accept: bool,
        #[serde(default)]
        text: Option<String>,
    },
    /// Sign in when `accept`, Cancel otherwise.
    Auth {
        accept: bool,
        #[serde(default)]
        username: String,
        #[serde(default)]
        password: String,
    },
    /// Give the page `paths`, files in workspace `workspace_id` relative to
    /// its root, when `accept`; leave the input as it was otherwise.
    FileChooser {
        accept: bool,
        #[serde(default)]
        workspace_id: Option<String>,
        #[serde(default)]
        paths: Vec<String>,
        /// `paths` on disk, filled in by the protocol once checked.
        #[serde(skip)]
        files: Vec<PathBuf>,
    },
    /// Pick the option at `index` (the request's `index`) when `accept`.
    Select {
        accept: bool,
        #[serde(default)]
        index: Option<u32>,
    },
    /// Give the input `value`, in its own format (empty clears it), when
    /// `accept`.
    Date {
        accept: bool,
        #[serde(default)]
        value: String,
    },
}

/// Never shows a password.
impl std::fmt::Debug for UiAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UiAnswer::Dialog { accept, text } => f
                .debug_struct("Dialog")
                .field("accept", accept)
                .field("text", text)
                .finish(),
            UiAnswer::Auth { accept, .. } => f
                .debug_struct("Auth")
                .field("accept", accept)
                .finish_non_exhaustive(),
            UiAnswer::FileChooser {
                accept,
                workspace_id,
                paths,
                ..
            } => f
                .debug_struct("FileChooser")
                .field("accept", accept)
                .field("workspace_id", workspace_id)
                .field("paths", paths)
                .finish_non_exhaustive(),
            UiAnswer::Select { accept, index } => f
                .debug_struct("Select")
                .field("accept", accept)
                .field("index", index)
                .finish(),
            UiAnswer::Date { accept, value } => f
                .debug_struct("Date")
                .field("accept", accept)
                .field("value", value)
                .finish(),
        }
    }
}

/// The dialog `Page.javascriptDialogOpening` describes.
pub fn dialog_of(params: &Value) -> UiKind {
    let dialog = match params["type"].as_str() {
        Some("confirm") => DialogType::Confirm,
        Some("prompt") => DialogType::Prompt,
        Some("beforeunload") => DialogType::Beforeunload,
        _ => DialogType::Alert,
    };
    UiKind::Dialog {
        dialog,
        message: params["message"].as_str().unwrap_or_default().to_string(),
        default_text: (dialog == DialogType::Prompt).then(|| {
            params["defaultPrompt"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        }),
        url: params["url"].as_str().unwrap_or_default().to_string(),
    }
}

/// The sign-in `Fetch.authRequired` asks for.
pub fn auth_of(params: &Value) -> UiKind {
    let challenge = &params["authChallenge"];
    UiKind::Auth {
        origin: challenge["origin"].as_str().unwrap_or_default().to_string(),
        scheme: challenge["scheme"].as_str().unwrap_or_default().to_string(),
        realm: challenge["realm"].as_str().unwrap_or_default().to_string(),
        proxy: challenge["source"] == "Proxy",
        url: params["request"]["url"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        fetch_request: params["requestId"].as_str().unwrap_or_default().to_string(),
        frame_id: params["frameId"].as_str().unwrap_or_default().to_string(),
    }
}

/// The file chooser `Page.fileChooserOpened` reports for the page at `url`;
/// none when no input element opened it.
pub fn chooser_of(params: &Value, url: &str) -> Option<UiKind> {
    Some(UiKind::FileChooser {
        multiple: params["mode"] == "selectMultiple",
        url: url.to_string(),
        backend_node: params["backendNodeId"].as_i64()?,
    })
}

fn continue_with_auth(fetch_request: &str, response: Value) -> (&'static str, Value) {
    (
        "Fetch.continueWithAuth",
        json!({ "requestId": fetch_request, "authChallengeResponse": response }),
    )
}

fn invalid_answer() -> BrowserError {
    BrowserError {
        code: "invalid_answer",
        message: "That answer does not fit the request".into(),
    }
}

/// The CDP call that gives `answer` to `request`; none when there is nothing
/// to tell the engine (a cancelled file chooser).
pub fn answer_call(
    request: &UiKind,
    answer: &UiAnswer,
) -> Result<Option<(&'static str, Value)>, BrowserError> {
    match (request, answer) {
        (UiKind::Dialog { .. }, UiAnswer::Dialog { accept, text }) => {
            let mut params = json!({ "accept": accept });
            if let Some(text) = text {
                params["promptText"] = json!(text);
            }
            Ok(Some(("Page.handleJavaScriptDialog", params)))
        }
        (UiKind::FileChooser { .. }, UiAnswer::FileChooser { accept: false, .. }) => Ok(None),
        (
            UiKind::FileChooser {
                multiple,
                backend_node,
                ..
            },
            UiAnswer::FileChooser { paths, files, .. },
        ) => {
            // Unchecked paths never reach the engine.
            if files.is_empty() || files.len() != paths.len() || (!multiple && files.len() > 1) {
                return Err(invalid_answer());
            }
            Ok(Some((
                "DOM.setFileInputFiles",
                json!({ "files": files, "backendNodeId": backend_node }),
            )))
        }
        (
            UiKind::Auth { fetch_request, .. },
            UiAnswer::Auth {
                accept,
                username,
                password,
            },
        ) => Ok(Some(continue_with_auth(
            fetch_request,
            if *accept {
                json!({ "response": "ProvideCredentials", "username": username, "password": password })
            } else {
                json!({ "response": "CancelAuth" })
            },
        ))),
        (UiKind::Select { .. }, UiAnswer::Select { accept: false, .. })
        | (UiKind::Date { .. }, UiAnswer::Date { accept: false, .. }) => Ok(None),
        (
            UiKind::Select {
                options,
                context,
                element,
                ..
            },
            UiAnswer::Select {
                index: Some(index), ..
            },
        ) => {
            if !options.iter().any(|o| o.index == *index && !o.disabled) {
                return Err(invalid_answer());
            }
            Ok(Some(helper_answer(*context, *element, json!([index]))))
        }
        (
            UiKind::Date {
                context, element, ..
            },
            UiAnswer::Date { value, .. },
        ) => {
            let fits = value.len() <= MAX_DATE_VALUE
                && value
                    .chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, '-' | ':' | 'T' | 'W' | '.'));
            if !fits {
                return Err(invalid_answer());
            }
            Ok(Some(helper_answer(*context, *element, json!(value))))
        }
        _ => Err(invalid_answer()),
    }
}

/// Longest date or time value an answer gives.
const MAX_DATE_VALUE: usize = 32;

/// The page helper gives its waiting element `value`.
fn helper_answer(context: i64, element: u64, value: Value) -> (&'static str, Value) {
    (
        "Runtime.evaluate",
        json!({ "expression": helper::answer_expression(element, &value), "contextId": context }),
    )
}

/// The call that ends a request nobody answered: the dialog dismissed, the
/// sign-in cancelled. A file chooser needs none.
pub fn dismiss_call(request: &UiKind) -> Option<(&'static str, Value)> {
    match request {
        UiKind::Dialog { .. } => Some(("Page.handleJavaScriptDialog", json!({ "accept": false }))),
        UiKind::Auth { fetch_request, .. } => Some(continue_with_auth(
            fetch_request,
            json!({ "response": "CancelAuth" }),
        )),
        UiKind::FileChooser { .. } | UiKind::Select { .. } | UiKind::Date { .. } => None,
    }
}

/// How long `request` waits for a person.
pub fn timeout(request: &UiKind) -> Duration {
    match request {
        UiKind::Dialog { .. } => DIALOG_TIMEOUT,
        UiKind::Auth { .. }
        | UiKind::FileChooser { .. }
        | UiKind::Select { .. }
        | UiKind::Date { .. } => AUTH_TIMEOUT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialogs_read_from_cdp() {
        let prompt = dialog_of(&json!({"type": "prompt", "message": "Name?",
            "defaultPrompt": "Ann", "url": "http://localhost:5173/"}));
        assert_eq!(
            prompt,
            UiKind::Dialog {
                dialog: DialogType::Prompt,
                message: "Name?".into(),
                default_text: Some("Ann".into()),
                url: "http://localhost:5173/".into(),
            }
        );
        let alert = dialog_of(&json!({"type": "alert", "message": "Hi", "url": "u"}));
        assert!(matches!(
            alert,
            UiKind::Dialog {
                dialog: DialogType::Alert,
                default_text: None,
                ..
            }
        ));
    }

    #[test]
    fn requests_serialize_flat_for_the_protocol() {
        let request = UiRequest {
            request_id: "ui-1".into(),
            tab_id: "t1".into(),
            owner: TabOwner::Personal,
            kind: dialog_of(&json!({"type": "confirm", "message": "Sure?", "url": "u"})),
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({"request_id": "ui-1", "tab_id": "t1", "owner": {"kind": "personal"},
                   "kind": "dialog", "dialog": "confirm", "message": "Sure?", "url": "u"})
        );
    }

    #[test]
    fn answers_become_cdp_calls() {
        let kind = dialog_of(&json!({"type": "prompt", "message": "", "url": ""}));
        let answer: UiAnswer =
            serde_json::from_value(json!({"kind": "dialog", "accept": true, "text": "Bo"}))
                .unwrap();
        assert_eq!(
            answer_call(&kind, &answer).unwrap().unwrap(),
            (
                "Page.handleJavaScriptDialog",
                json!({"accept": true, "promptText": "Bo"})
            )
        );
        assert_eq!(
            answer_call(
                &kind,
                &UiAnswer::Auth {
                    accept: true,
                    username: "a".into(),
                    password: "b".into()
                }
            )
            .unwrap_err()
            .code,
            "invalid_answer"
        );
        assert!(serde_json::from_value::<UiAnswer>(
            json!({"kind": "dialog", "accept": true, "x": 1})
        )
        .is_err());
    }

    #[test]
    fn a_sign_in_reads_from_cdp_and_never_shows_the_password() {
        let kind = auth_of(&json!({
            "requestId": "interception-7", "frameId": "F1",
            "request": {"url": "http://127.0.0.1:8766/secret"},
            "authChallenge": {"source": "Server", "origin": "http://127.0.0.1:8766",
                              "scheme": "basic", "realm": "Staging"},
        }));
        let request = UiRequest {
            request_id: "ui-2".into(),
            tab_id: "t1".into(),
            owner: TabOwner::Personal,
            kind: kind.clone(),
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({"request_id": "ui-2", "tab_id": "t1", "owner": {"kind": "personal"},
                   "kind": "auth", "origin": "http://127.0.0.1:8766", "scheme": "basic",
                   "realm": "Staging", "proxy": false, "url": "http://127.0.0.1:8766/secret"})
        );

        let answer: UiAnswer = serde_json::from_value(
            json!({"kind": "auth", "accept": true, "username": "ann", "password": "hunter2"}),
        )
        .unwrap();
        assert!(!format!("{answer:?}").contains("hunter2"));
        assert_eq!(
            answer_call(&kind, &answer).unwrap().unwrap(),
            (
                "Fetch.continueWithAuth",
                json!({"requestId": "interception-7", "authChallengeResponse":
                       {"response": "ProvideCredentials", "username": "ann", "password": "hunter2"}})
            )
        );
        assert_eq!(
            dismiss_call(&kind).unwrap(),
            (
                "Fetch.continueWithAuth",
                json!({"requestId": "interception-7", "authChallengeResponse": {"response": "CancelAuth"}})
            )
        );
        assert_eq!(timeout(&kind), AUTH_TIMEOUT);
    }

    #[test]
    fn a_file_chooser_takes_only_checked_files() {
        let kind = chooser_of(
            &json!({"frameId": "F", "mode": "selectSingle", "backendNodeId": 42}),
            "https://example.com/upload",
        )
        .unwrap();
        assert!(chooser_of(&json!({"mode": "selectSingle"}), "u").is_none());
        let request = UiRequest {
            request_id: "ui-3".into(),
            tab_id: "t1".into(),
            owner: TabOwner::Personal,
            kind: kind.clone(),
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({"request_id": "ui-3", "tab_id": "t1", "owner": {"kind": "personal"},
                   "kind": "file_chooser", "multiple": false, "url": "https://example.com/upload"})
        );

        let mut answer: UiAnswer = serde_json::from_value(json!({"kind": "file_chooser",
            "accept": true, "workspace_id": "w1", "paths": ["a.png"]}))
        .unwrap();
        assert_eq!(
            answer_call(&kind, &answer).unwrap_err().code,
            "invalid_answer",
            "unchecked"
        );
        if let UiAnswer::FileChooser { files, .. } = &mut answer {
            files.push(PathBuf::from("/repo/a.png"));
        }
        assert_eq!(
            answer_call(&kind, &answer).unwrap().unwrap(),
            (
                "DOM.setFileInputFiles",
                json!({"files": ["/repo/a.png"], "backendNodeId": 42})
            )
        );
        let two = UiAnswer::FileChooser {
            accept: true,
            workspace_id: Some("w1".into()),
            paths: vec!["a".into(), "b".into()],
            files: vec!["/repo/a".into(), "/repo/b".into()],
        };
        assert_eq!(
            answer_call(&kind, &two).unwrap_err().code,
            "invalid_answer",
            "one file only"
        );
        let cancel: UiAnswer =
            serde_json::from_value(json!({"kind": "file_chooser", "accept": false})).unwrap();
        assert_eq!(answer_call(&kind, &cancel).unwrap(), None);
        assert_eq!(dismiss_call(&kind), None);
        assert!(
            serde_json::from_value::<UiAnswer>(
                json!({"kind": "file_chooser", "accept": true, "files": ["/etc/passwd"]})
            )
            .is_err(),
            "files never come from a client"
        );
    }
}
