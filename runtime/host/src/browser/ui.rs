//! Browser UI the stream does not show (design 9): Chromium paints none of
//! it into screencast frames, so the Host turns each into a request every
//! client draws with its own UI, and the first answer wins.
//!
//! For now: JavaScript dialogs (`alert`, `confirm`, `prompt`,
//! `beforeunload`). A dialog nobody answers within [`DIALOG_TIMEOUT`] is
//! dismissed, since an open dialog stalls the page and its frames.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::service::BrowserError;
use super::store::TabOwner;

pub const DIALOG_TIMEOUT: Duration = Duration::from_secs(60);

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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiAnswer {
    /// OK (or Leave) when `accept`, Cancel (or Stay) otherwise; `text` is a
    /// `prompt`'s answer.
    Dialog {
        accept: bool,
        #[serde(default)]
        text: Option<String>,
    },
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
    }
}

/// The call that dismisses a dialog nobody answered.
pub fn dismiss_call() -> (&'static str, Value) {
    ("Page.handleJavaScriptDialog", json!({ "accept": false }))
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
        assert!(serde_json::from_value::<UiAnswer>(
            json!({"kind": "dialog", "accept": true, "x": 1})
        )
        .is_err());
    }
}
