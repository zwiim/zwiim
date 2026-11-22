//! Zwiim-owned: a playbook drafted from a recording (recording design
//! 2026-11-22 §2).
//!
//! No model is asked: the draft is what the person did, named the way they
//! would name it. Each value they typed is offered as their info (an input)
//! or fixed text (kept in its step); each file is an input; the page they
//! started on is the source; the click that sent is marked for them to
//! confirm. The steps go to the item agent as a guide it adapts to each page.

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::automation::input_files::StoredFile;
use crate::automation::playbook_format::{
    ApprovalEntry, CheckSpec, Input, PlaybookDoc, Source, FORMAT_VERSION,
};
use crate::automation::reasons::{ActionKind, ApprovalRule};
use crate::browser::recording::{RecordedStep, Recording, StepAction};

/// Where the drafted playbook's items come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DraftSource {
    /// A list or search page the person picked an item from.
    Search { url: String },
    /// The pages to work, one per item.
    List { urls: Vec<String> },
}

/// What a step does with a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DraftValue {
    None,
    /// The person's info: an input named `input`.
    MyInfo {
        input: String,
        value: String,
    },
    /// The same text every time, kept in the step.
    Fixed {
        text: String,
    },
    /// A file input; `path` is the file chosen while recording, if any.
    File {
        input: String,
        path: Option<String>,
    },
    /// A protected field: the agent asks the person each time.
    NeedsYou,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftStep {
    pub action: StepAction,
    /// The control's name, as the person sees it.
    pub target: String,
    pub role: String,
    pub url: String,
    pub value: DraftValue,
}

/// A playbook as drafted from a recording, for the person to review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybookDraft {
    pub name: String,
    pub goal: String,
    pub source: DraftSource,
    pub sites: Vec<String>,
    pub steps: Vec<DraftStep>,
    /// The step that sends, by index; the person confirms it.
    pub send_step: Option<usize>,
    pub send_kind: ActionKind,
    /// Text the page shows once the send worked, if the person gives it.
    pub check_text: Option<String>,
}

/// The draft for what the person did in `recording`.
pub fn draft_from_recording(recording: &Recording) -> PlaybookDraft {
    let raw = &recording.steps;
    // The send: the last click a new page followed, else the last click.
    let followed = |i: usize| {
        raw.get(i + 1)
            .is_some_and(|n| n.action == StepAction::Navigate)
    };
    let clicks: Vec<usize> = (0..raw.len())
        .filter(|&i| raw[i].action == StepAction::Click)
        .collect();
    let send_raw = clicks
        .iter()
        .rev()
        .find(|&&i| followed(i))
        .or(clicks.last())
        .copied();

    let mut taken: Vec<String> = Vec::new();
    let mut fresh = |wanted: &str, fallback: &str| {
        // A step quotes the name, so it holds no double quote.
        let base = match wanted.trim() {
            "" => fallback.to_string(),
            w => w.replace('"', "'"),
        };
        let mut name = base.clone();
        let mut n = 1;
        while taken.contains(&name) {
            n += 1;
            name = format!("{base} {n}");
        }
        taken.push(name.clone());
        name
    };
    let mut steps = Vec::new();
    let mut send_step = None;
    for (i, r) in raw.iter().enumerate() {
        // A page a click opened is part of that click.
        if r.action == StepAction::Navigate && i > 0 && raw[i - 1].action == StepAction::Click {
            continue;
        }
        let value = match r.action {
            StepAction::Fill | StepAction::Choose => DraftValue::MyInfo {
                input: fresh(&r.name, "Info"),
                value: r.value.clone().unwrap_or_default(),
            },
            StepAction::File => DraftValue::File {
                input: fresh(&r.name, "File"),
                path: r.paths.first().cloned(),
            },
            StepAction::Protected => DraftValue::NeedsYou,
            StepAction::Click | StepAction::Navigate => DraftValue::None,
        };
        if Some(i) == send_raw {
            send_step = Some(steps.len());
        }
        steps.push(DraftStep {
            action: r.action,
            target: r.name.clone(),
            role: r.role.clone(),
            url: r.url.clone(),
            value,
        });
    }

    let host = host_of(&recording.start_url).unwrap_or_default();
    let sites: BTreeSet<String> = std::iter::once(recording.start_url.as_str())
        .chain(raw.iter().map(|r| r.url.as_str()))
        .filter_map(host_of)
        .collect();
    let opened_an_item = raw
        .first()
        .is_some_and(|r| r.action == StepAction::Click && r.role == "link")
        && raw.iter().any(|r| r.action == StepAction::Navigate);
    let source = match opened_an_item {
        true => DraftSource::Search {
            url: recording.start_url.clone(),
        },
        false => DraftSource::List {
            urls: vec![recording.start_url.clone()],
        },
    };
    let send_kind = match steps
        .iter()
        .any(|s| matches!(s.value, DraftValue::File { .. }))
    {
        true => ActionKind::SubmitApplication,
        false => ActionKind::SubmitForm,
    };
    let goal = match send_step {
        Some(at) => format!("Do what I did on {host}: {}.", steps[at].target),
        None => format!("Do what I did on {host}."),
    };
    PlaybookDraft {
        name: format!("{} on {host}", verb(send_kind)),
        goal,
        source,
        sites: sites.into_iter().collect(),
        steps,
        send_step,
        send_kind,
        check_text: None,
    }
}

/// The word a draft's name starts with.
fn verb(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::SubmitApplication => "Apply",
        ActionKind::PublishPost => "Post",
        ActionKind::SendMessage => "Message",
        ActionKind::Comment => "Comment",
        ActionKind::Follow => "Follow",
        _ => "Submit",
    }
}

fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url).ok()?.host_str().map(str::to_string)
}

/// A name inside a step's quotes: a double quote becomes a single one.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "'"))
}

/// An input as a step names it: quoted when it has a space.
fn input_ref(name: &str) -> String {
    match name.contains(char::is_whitespace) {
        true => format!("input {}", quoted(name)),
        false => format!("input {name}"),
    }
}

/// The step line the agent reads.
fn step_line(step: &DraftStep, send: bool) -> String {
    let target = quoted(&step.target);
    let line = match (&step.action, &step.value) {
        (StepAction::Navigate, _) => format!("Go to {}", step.url),
        (_, DraftValue::NeedsYou) => format!("{target} needs you: ask the person"),
        (_, DraftValue::File { input, .. }) => format!("Attach {} to {target}", input_ref(input)),
        (StepAction::Choose, DraftValue::MyInfo { input, .. }) => {
            format!("Choose {} in {target}", input_ref(input))
        }
        (StepAction::Choose, DraftValue::Fixed { text }) => {
            format!("Choose {} in {target}", quoted(text))
        }
        (_, DraftValue::MyInfo { input, .. }) => format!("Fill {target} with {}", input_ref(input)),
        (_, DraftValue::Fixed { text }) => format!("Type {} into {target}", quoted(text)),
        (_, DraftValue::None) => format!("Click {target}"),
    };
    match send {
        true => format!("{line} (the send)"),
        false => line,
    }
}

/// The playbook a reviewed draft becomes. `stored` has the Host's copy of
/// each file input, by input name. `Err` names what is missing.
pub fn draft_to_doc(
    draft: &PlaybookDraft,
    stored: &HashMap<String, StoredFile>,
) -> Result<PlaybookDoc, String> {
    let send = draft
        .send_step
        .filter(|&at| at < draft.steps.len())
        .ok_or_else(|| "Choose which step sends.".to_string())?;
    let mut inputs = Vec::new();
    for step in &draft.steps {
        match &step.value {
            DraftValue::MyInfo { input, value } => inputs.push(Input::Fact {
                name: input.clone(),
                value: value.clone(),
            }),
            DraftValue::File { input, .. } => {
                let file = stored
                    .get(input)
                    .ok_or_else(|| format!("Pick the file for {input}."))?;
                inputs.push(Input::File {
                    name: input.clone(),
                    path: None,
                    file: Some(file.file.clone()),
                    sha256: Some(file.sha256.clone()),
                });
            }
            _ => {}
        }
    }
    let check = match draft.check_text.as_deref().map(str::trim) {
        Some(text) if !text.is_empty() => CheckSpec::TextPresent { text: text.into() },
        _ => CheckSpec::AgentJudged {
            note: format!(
                "the page shows that \"{}\" went through",
                draft.steps[send].target.replace('"', "'")
            ),
        },
    };
    Ok(PlaybookDoc {
        format: FORMAT_VERSION,
        name: draft.name.trim().to_string(),
        goal: draft.goal.trim().to_string(),
        sources: vec![match &draft.source {
            DraftSource::Search { url } => Source::Search { url: url.clone() },
            DraftSource::List { urls } => Source::List {
                items: urls.clone(),
            },
        }],
        main_kind: draft.send_kind,
        inputs,
        rules: Vec::new(),
        steps: draft
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| step_line(s, i == send))
            .collect(),
        approval: vec![ApprovalEntry {
            kind: draft.send_kind,
            rule: ApprovalRule::Ask,
        }],
        checks: vec![check],
        caps: None,
        budget: None,
        agent: None,
        sites: draft.sites.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(
        action: StepAction,
        role: &str,
        name: &str,
        value: Option<&str>,
        url: &str,
    ) -> RecordedStep {
        RecordedStep {
            action,
            role: role.into(),
            name: name.into(),
            value: value.map(str::to_string),
            url: url.into(),
            paths: Vec::new(),
        }
    }

    const SEARCH: &str = "https://jobs.example/search?q=rust";
    const JOB: &str = "https://jobs.example/j/1";

    fn job_application() -> Recording {
        let mut r = Recording::new("t1", SEARCH);
        r.push(step(
            StepAction::Click,
            "link",
            "Rust engineer",
            None,
            SEARCH,
        ));
        r.navigated(JOB);
        r.push(step(StepAction::Click, "button", "Apply", None, JOB));
        r.push(step(
            StepAction::Fill,
            "textbox",
            "Full name",
            Some("Ada Lovelace"),
            JOB,
        ));
        r.push(step(
            StepAction::Fill,
            "textbox",
            "Email",
            Some("ada@example.com"),
            JOB,
        ));
        r.chose_files(vec!["/Users/ada/ws/cv.pdf".into()]);
        r.push(step(
            StepAction::File,
            "button",
            "Upload CV",
            Some("cv.pdf"),
            JOB,
        ));
        r.push(step(
            StepAction::Click,
            "button",
            "Submit application",
            None,
            JOB,
        ));
        r.navigated("https://apply.example/thanks");
        r
    }

    fn stored(name: &str) -> HashMap<String, StoredFile> {
        HashMap::from([(
            name.to_string(),
            StoredFile {
                sha256: "a".repeat(64),
                file: "cv.pdf".into(),
                size: 2,
            },
        )])
    }

    #[test]
    fn a_job_application_recording_becomes_a_draft() {
        let d = draft_from_recording(&job_application());
        assert_eq!(d.source, DraftSource::Search { url: SEARCH.into() });
        assert_eq!(
            d.sites,
            vec!["apply.example".to_string(), "jobs.example".to_string()]
        );
        assert_eq!(d.send_kind, ActionKind::SubmitApplication);
        assert_eq!(d.name, "Apply on jobs.example");
        let send = d.send_step.expect("a send");
        assert_eq!(d.steps[send].target, "Submit application");
        assert_eq!(d.goal, "Do what I did on jobs.example: Submit application.");
        let values: Vec<&DraftValue> = d.steps.iter().map(|s| &s.value).collect();
        assert!(values.contains(&&DraftValue::MyInfo {
            input: "Full name".into(),
            value: "Ada Lovelace".into()
        }));
        assert!(values.contains(&&DraftValue::MyInfo {
            input: "Email".into(),
            value: "ada@example.com".into()
        }));
        assert!(values.contains(&&DraftValue::File {
            input: "Upload CV".into(),
            path: Some("/Users/ada/ws/cv.pdf".into())
        }));
        // A page a click opened is not a step of its own.
        assert!(
            d.steps.iter().all(|s| s.action != StepAction::Navigate),
            "{:?}",
            d.steps
        );
        assert_eq!(d.check_text, None);
    }

    #[test]
    fn a_single_page_form_is_a_list_source() {
        let url = "https://forms.example/contact";
        let mut r = Recording::new("t1", url);
        r.push(step(
            StepAction::Fill,
            "textbox",
            "Message",
            Some("Hi"),
            url,
        ));
        r.push(step(StepAction::Click, "button", "Send", None, url));
        let d = draft_from_recording(&r);
        assert_eq!(
            d.source,
            DraftSource::List {
                urls: vec![url.into()]
            }
        );
        assert_eq!(d.send_kind, ActionKind::SubmitForm);
        assert_eq!(d.name, "Submit on forms.example");
        assert_eq!(d.send_step, Some(1), "the last click when no page followed");
    }

    #[test]
    fn duplicate_field_names_get_numbered_inputs() {
        let url = "https://forms.example/";
        let mut r = Recording::new("t1", url);
        r.push(step(
            StepAction::Fill,
            "textbox",
            "Email",
            Some("a@x.test"),
            url,
        ));
        r.push(step(
            StepAction::Fill,
            "textbox",
            "Email",
            Some("b@x.test"),
            url,
        ));
        r.push(step(StepAction::Fill, "textbox", "", Some("c"), url));
        let d = draft_from_recording(&r);
        let inputs: Vec<String> = d
            .steps
            .iter()
            .filter_map(|s| match &s.value {
                DraftValue::MyInfo { input, .. } => Some(input.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(inputs, ["Email", "Email 2", "Info"]);
    }

    #[test]
    fn a_protected_step_needs_the_person() {
        let url = "https://forms.example/";
        let mut r = Recording::new("t1", url);
        r.push(step(
            StepAction::Protected,
            "password",
            "Password",
            None,
            url,
        ));
        let d = draft_from_recording(&r);
        assert_eq!(d.steps[0].value, DraftValue::NeedsYou);
        assert_eq!(d.send_step, None, "no click, no send");
    }

    #[test]
    fn the_playbook_names_inputs_not_values() {
        let mut d = draft_from_recording(&job_application());
        // The person keeps one value as fixed text.
        let at = d
            .steps
            .iter()
            .position(|s| s.target == "Full name")
            .unwrap();
        d.steps[at].value = DraftValue::Fixed {
            text: "Ada Lovelace".into(),
        };
        let doc = draft_to_doc(&d, &stored("Upload CV")).unwrap();
        let text = serde_yaml::to_string(&doc).unwrap();
        let back = crate::automation::validate_playbook(&text).expect("valid");
        assert_eq!(back.main_kind, ActionKind::SubmitApplication);
        assert!(
            back.steps
                .contains(&"Fill \"Email\" with input Email".to_string()),
            "{:?}",
            back.steps
        );
        assert!(back
            .steps
            .contains(&"Type \"Ada Lovelace\" into \"Full name\"".to_string()));
        assert!(back
            .steps
            .contains(&"Attach input \"Upload CV\" to \"Upload CV\"".to_string()));
        assert!(back
            .steps
            .contains(&"Click \"Submit application\" (the send)".to_string()));
        assert!(
            !back.steps.iter().any(|s| s.contains("ada@example.com")),
            "a value stays in its input"
        );
        assert!(back.inputs.contains(&Input::Fact {
            name: "Email".into(),
            value: "ada@example.com".into()
        }));
        assert!(back.inputs.contains(&Input::File {
            name: "Upload CV".into(),
            path: None,
            file: Some("cv.pdf".into()),
            sha256: Some("a".repeat(64)),
        }));
        assert!(
            !back.inputs.iter().any(|i| i.name() == "Full name"),
            "fixed text is no input"
        );
        assert_eq!(
            back.approval,
            vec![ApprovalEntry {
                kind: ActionKind::SubmitApplication,
                rule: ApprovalRule::Ask
            }]
        );
        assert_eq!(back.sources, vec![Source::Search { url: SEARCH.into() }]);
        assert_eq!(back.sites, d.sites);
        // No text to check for: the agent judges, the weaker check.
        assert!(
            matches!(&back.checks[..], [CheckSpec::AgentJudged { .. }]),
            "{:?}",
            back.checks
        );
        d.check_text = Some("Application sent".into());
        let doc = draft_to_doc(&d, &stored("Upload CV")).unwrap();
        assert_eq!(
            doc.checks,
            vec![CheckSpec::TextPresent {
                text: "Application sent".into()
            }]
        );
    }

    #[test]
    fn a_draft_needs_its_send_and_its_files() {
        let mut d = draft_from_recording(&job_application());
        assert!(draft_to_doc(&d, &HashMap::new())
            .unwrap_err()
            .contains("Upload CV"));
        d.send_step = None;
        assert_eq!(
            draft_to_doc(&d, &stored("Upload CV")).unwrap_err(),
            "Choose which step sends."
        );
    }

    #[test]
    fn a_quote_in_a_name_does_not_break_its_step() {
        let url = "https://forms.example/";
        let mut r = Recording::new("t1", url);
        r.push(step(StepAction::Click, "button", "Say \"hi\"", None, url));
        let d = draft_from_recording(&r);
        let doc = draft_to_doc(&d, &HashMap::new()).unwrap();
        assert_eq!(doc.steps, vec!["Click \"Say 'hi'\" (the send)".to_string()]);

        let mut r = Recording::new("t1", url);
        r.push(step(
            StepAction::Fill,
            "textbox",
            "Your \"nick\" name",
            Some("Ada"),
            url,
        ));
        r.push(step(StepAction::Click, "button", "Go", None, url));
        let doc = draft_to_doc(&draft_from_recording(&r), &HashMap::new()).unwrap();
        let text = serde_yaml::to_string(&doc).unwrap();
        crate::automation::validate_playbook(&text).expect("the step names its input");
    }
}
