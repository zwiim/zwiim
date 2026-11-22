//! Zwiim-owned: recording what a person does in a tab of their own, so
//! Zwiim can draft a playbook from it (recording design 2026-11-22 §1).
//!
//! The page helper reports each click, entry, choice and file as a `step`;
//! the Host adds a `navigate` step when a new document loads, and the paths
//! of files the person chose in the Host's file chooser. Everything the
//! helper sends is page data: it is checked and capped here, and a value
//! typed into a protected field (a password, a one-time code) never enters
//! a recording.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Most steps one recording keeps.
pub const MAX_STEPS: usize = 200;
const MAX_NAME: usize = 200;
const MAX_ROLE: usize = 40;
const MAX_VALUE: usize = 2000;
const MAX_URL: usize = 2048;
const MAX_FILES: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepAction {
    Click,
    Fill,
    Choose,
    File,
    /// Typed into a protected field; the value is never kept.
    Protected,
    Navigate,
}

/// One thing the person did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedStep {
    pub action: StepAction,
    /// The control's role (`button`, `textbox`, `link`...).
    pub role: String,
    /// Its accessible name: label, aria-label or text.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// The page it happened on.
    pub url: String,
    /// For a file step: the files chosen, on this computer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

/// A tab's recording so far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recording {
    pub tab_id: String,
    pub start_url: String,
    pub steps: Vec<RecordedStep>,
    /// Files the person just chose, waiting for the page's file step.
    #[serde(skip)]
    pending_paths: Vec<String>,
    #[serde(skip)]
    last_url: String,
}

impl Recording {
    pub fn new(tab_id: &str, start_url: &str) -> Self {
        Self {
            tab_id: tab_id.to_string(),
            start_url: start_url.to_string(),
            steps: Vec::new(),
            pending_paths: Vec::new(),
            last_url: start_url.to_string(),
        }
    }

    /// Keep `step`; false once the recording is full. A file step takes the
    /// files the person just chose.
    pub fn push(&mut self, mut step: RecordedStep) -> bool {
        if self.steps.len() >= MAX_STEPS {
            return false;
        }
        if step.action == StepAction::File {
            step.paths = std::mem::take(&mut self.pending_paths);
        }
        self.last_url = step.url.clone();
        self.steps.push(step);
        true
    }

    /// A new document loaded at `url`: a navigate step when it is another
    /// page than the last step's.
    pub fn navigated(&mut self, url: &str) -> bool {
        if url == self.last_url || !web_url(url) {
            return false;
        }
        self.push(RecordedStep {
            action: StepAction::Navigate,
            role: String::new(),
            name: String::new(),
            value: None,
            url: url.chars().take(MAX_URL).collect(),
            paths: Vec::new(),
        })
    }

    /// The person chose `paths` in the Host's file chooser.
    pub fn chose_files(&mut self, mut paths: Vec<String>) {
        paths.truncate(MAX_FILES);
        self.pending_paths = paths;
    }
}

/// A `step` message from the page helper, checked and capped; `None` for
/// anything that does not check out.
pub fn step_from_helper(m: &Value) -> Option<RecordedStep> {
    let s = &m["step"];
    let text = |key: &str, max: usize| {
        s[key]
            .as_str()
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
            .map(|t| t.chars().take(max).collect::<String>())
    };
    let role = text("role", MAX_ROLE).unwrap_or_default();
    let mut action = match s["action"].as_str()? {
        "click" => StepAction::Click,
        "fill" => StepAction::Fill,
        "choose" => StepAction::Choose,
        "file" => StepAction::File,
        "protected" => StepAction::Protected,
        _ => return None,
    };
    if role == "password" {
        action = StepAction::Protected;
    }
    let url = s["url"].as_str().filter(|u| web_url(u))?;
    let value = match action {
        StepAction::Protected => None,
        // A value keeps its own spacing.
        _ => s["value"]
            .as_str()
            .map(|v| v.chars().take(MAX_VALUE).collect::<String>()),
    };
    Some(RecordedStep {
        action,
        role,
        name: text("name", MAX_NAME).unwrap_or_default(),
        value,
        url: url.chars().take(MAX_URL).collect(),
        paths: Vec::new(),
    })
}

/// Only pages a person works on: http and https.
fn web_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn step(action: StepAction, name: &str, url: &str) -> RecordedStep {
        RecordedStep {
            action,
            role: "button".into(),
            name: name.into(),
            value: None,
            url: url.into(),
            paths: Vec::new(),
        }
    }

    #[test]
    fn a_step_message_is_checked_and_capped() {
        let long = "x".repeat(5000);
        let s = step_from_helper(&json!({
            "kind": "step",
            "step": { "action": "fill", "role": "textbox", "name": long, "value": long, "url": "https://jobs.example/apply" }
        }))
        .unwrap();
        assert_eq!(s.action, StepAction::Fill);
        assert_eq!(s.name.chars().count(), 200);
        assert_eq!(s.value.unwrap().chars().count(), 2000);
        assert!(step_from_helper(&json!({ "kind": "step", "step": { "action": "hover", "role": "x", "name": "y", "url": "https://a.test/" } })).is_none());
        assert!(step_from_helper(&json!({ "kind": "step", "step": { "action": "click", "role": "button", "name": "Go", "url": "javascript:alert(1)" } })).is_none());
        // The page never names a navigate or a file's paths.
        assert!(step_from_helper(&json!({ "kind": "step", "step": { "action": "navigate", "role": "", "name": "", "url": "https://a.test/" } })).is_none());
        let file = step_from_helper(&json!({ "kind": "step", "step": { "action": "file", "role": "button", "name": "CV", "value": "cv.pdf", "url": "https://a.test/", "paths": ["/etc/passwd"] } })).unwrap();
        assert!(file.paths.is_empty());
    }

    #[test]
    fn a_protected_value_never_enters_a_recording() {
        let s = step_from_helper(&json!({
            "kind": "step",
            "step": { "action": "protected", "role": "textbox", "name": "Password", "value": "hunter2", "url": "https://a.test/" }
        }))
        .unwrap();
        assert_eq!(s.action, StepAction::Protected);
        assert_eq!(s.value, None);
        // A fill the page says is a password field is protected too.
        let s = step_from_helper(&json!({
            "kind": "step",
            "step": { "action": "fill", "role": "password", "name": "Password", "value": "hunter2", "url": "https://a.test/" }
        }))
        .unwrap();
        assert_eq!((s.action, s.value), (StepAction::Protected, None));
    }

    #[test]
    fn steps_collect_in_order_up_to_the_cap() {
        let mut r = Recording::new("t1", "https://jobs.example/");
        assert!(r.push(step(StepAction::Click, "Apply", "https://jobs.example/")));
        assert!(r.push(step(StepAction::Click, "Next", "https://jobs.example/")));
        assert_eq!(r.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Apply", "Next"]);
        for _ in 2..MAX_STEPS {
            assert!(r.push(step(StepAction::Click, "More", "https://jobs.example/")));
        }
        assert!(!r.push(step(StepAction::Click, "Too many", "https://jobs.example/")));
        assert_eq!(r.steps.len(), MAX_STEPS);
    }

    #[test]
    fn a_new_page_is_a_navigate_step_and_the_same_page_is_not() {
        let mut r = Recording::new("t1", "https://jobs.example/search");
        assert!(!r.navigated("https://jobs.example/search"), "the page it started on");
        r.push(step(StepAction::Click, "Backend role", "https://jobs.example/search"));
        assert!(r.navigated("https://jobs.example/j/1"));
        assert!(!r.navigated("https://jobs.example/j/1"), "a reload is not a new page");
        let last = r.steps.last().unwrap();
        assert_eq!((last.action, last.url.as_str()), (StepAction::Navigate, "https://jobs.example/j/1"));
    }

    #[test]
    fn a_chosen_file_joins_its_step() {
        let mut r = Recording::new("t1", "https://jobs.example/");
        r.chose_files(vec!["/Users/me/ws/cv.pdf".into()]);
        r.push(step(StepAction::Click, "Apply", "https://jobs.example/"));
        r.push(step(StepAction::File, "Upload CV", "https://jobs.example/"));
        assert!(r.steps[0].paths.is_empty());
        assert_eq!(r.steps[1].paths, vec!["/Users/me/ws/cv.pdf".to_string()]);
        r.push(step(StepAction::File, "Cover letter", "https://jobs.example/"));
        assert!(r.steps[2].paths.is_empty(), "used once");
    }
}
