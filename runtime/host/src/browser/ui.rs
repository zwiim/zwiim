//! Browser UI the stream does not show (design 9): Chromium paints none of
//! it into screencast frames, so the Host turns each into a request every
//! client draws with its own UI, and the first answer wins.
//!
//! For now: JavaScript dialogs (`alert`, `confirm`, `prompt`,
//! `beforeunload`) and HTTP auth. A dialog nobody answers within
//! [`DIALOG_TIMEOUT`] is dismissed, since an open dialog stalls the page and
//! its frames; a sign-in nobody answers within [`AUTH_TIMEOUT`] is cancelled.
//! Credentials go to the engine and nowhere else: never stored or logged.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::service::BrowserError;
use super::store::TabOwner;

pub const DIALOG_TIMEOUT: Duration = Duration::from_secs(60);
/// Longer: nothing stalls while a sign-in waits, and typing a password takes time.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(300);

/// Something a tab is waiting on a person for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UiRequest {
    pub request_id: String,
    pub tab_id: String,
    pub owner: TabOwner,
    #[serde(flatten)]
    pub kind: UiKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DialogType {
    Alert,
    Confirm,
    Prompt,
    Beforeunload,
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

fn continue_with_auth(fetch_request: &str, response: Value) -> (&'static str, Value) {
    (
        "Fetch.continueWithAuth",
        json!({ "requestId": fetch_request, "authChallengeResponse": response }),
    )
}

/// The CDP call that gives `answer` to `request`.
pub fn answer_call(
    request: &UiKind,
    answer: &UiAnswer,
) -> Result<(&'static str, Value), BrowserError> {
    match (request, answer) {
        (UiKind::Dialog { .. }, UiAnswer::Dialog { accept, text }) => {
            let mut params = json!({ "accept": accept });
            if let Some(text) = text {
                params["promptText"] = json!(text);
            }
            Ok(("Page.handleJavaScriptDialog", params))
        }
        (
            UiKind::Auth { fetch_request, .. },
            UiAnswer::Auth {
                accept,
                username,
                password,
            },
        ) => Ok(continue_with_auth(
            fetch_request,
            if *accept {
                json!({ "response": "ProvideCredentials", "username": username, "password": password })
            } else {
                json!({ "response": "CancelAuth" })
            },
        )),
        _ => Err(BrowserError {
            code: "invalid_answer",
            message: "That answer does not fit the request".into(),
        }),
    }
}

/// The call that ends a request nobody answered: the dialog dismissed, the
/// sign-in cancelled.
pub fn dismiss_call(request: &UiKind) -> (&'static str, Value) {
    match request {
        UiKind::Dialog { .. } => ("Page.handleJavaScriptDialog", json!({ "accept": false })),
        UiKind::Auth { fetch_request, .. } => {
            continue_with_auth(fetch_request, json!({ "response": "CancelAuth" }))
        }
    }
}

/// How long `request` waits for a person.
pub fn timeout(request: &UiKind) -> Duration {
    match request {
        UiKind::Dialog { .. } => DIALOG_TIMEOUT,
        UiKind::Auth { .. } => AUTH_TIMEOUT,
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
            answer_call(&kind, &answer).unwrap(),
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
            answer_call(&kind, &answer).unwrap(),
            (
                "Fetch.continueWithAuth",
                json!({"requestId": "interception-7", "authChallengeResponse":
                       {"response": "ProvideCredentials", "username": "ann", "password": "hunter2"}})
            )
        );
        assert_eq!(
            dismiss_call(&kind),
            (
                "Fetch.continueWithAuth",
                json!({"requestId": "interception-7", "authChallengeResponse": {"response": "CancelAuth"}})
            )
        );
        assert_eq!(timeout(&kind), AUTH_TIMEOUT);
    }
}
