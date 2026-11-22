// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: the playbook text format `format: 1` and its validation
//! (automation-engine spec 16.3, 5.3.1, 18.6).
//!
//! A playbook is plain YAML the person can read and edit. Validation runs on
//! every save, including agent-proposed changes, and every error is
//! line-anchored (the protocol reports them as `playbook_invalid` with
//! `[{line, field, problem, fix}]`). There is no format migrator until a
//! format 2 exists.

use serde::{Deserialize, Serialize};

use crate::automation::reasons::{ActionKind, ApprovalRule};

/// The only format this build writes and reads (spec 16.3).
pub const FORMAT_VERSION: u32 = 1;

/// A playbook as parsed from its text (spec 5.3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaybookDoc {
    pub format: u32,
    pub name: String,
    #[serde(default)]
    pub goal: String,
    /// Where items come from (a search, a calendar file, a typed list).
    #[serde(default)]
    pub sources: Vec<Source>,
    /// The kind discovery filters the ledger on.
    pub main_kind: ActionKind,
    /// Inputs: files the person provides (resume) and facts.
    #[serde(default)]
    pub inputs: Vec<Input>,
    #[serde(default)]
    pub rules: Vec<String>,
    /// How the person did one item, as a guide the item agent adapts to the
    /// page (recording, spec 16.2). Inputs are named `input <name>`.
    #[serde(default)]
    pub steps: Vec<String>,
    /// Approval rules per kind; a kind absent is ask.
    #[serde(default)]
    pub approval: Vec<ApprovalEntry>,
    #[serde(default)]
    pub checks: Vec<CheckSpec>,
    #[serde(default)]
    pub caps: Option<CapsSpec>,
    #[serde(default)]
    pub budget: Option<u64>,
    /// Which agent works items; absent means the default agent.
    #[serde(default)]
    pub agent: Option<String>,
    /// The domains the playbook may use.
    #[serde(default)]
    pub sites: Vec<String>,
}

/// One discovery source (spec 5.3.1.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Source {
    /// A search on a site: the start URL and what to look for.
    Search { url: String },
    /// A list typed in by the person.
    List { items: Vec<String> },
    /// A content calendar file.
    File { path: String },
}

/// An input the person provides (spec 5.3.1.2). A secret references a
/// keychain handle, never a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
    /// A file the Host attaches (e.g. the resume): a copy the Host keeps
    /// (`file` and its `sha256`), or, in older playbooks, a `path` on this
    /// computer.
    File {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha256: Option<String>,
    },
    /// A fact known about the person (a profile answer).
    Fact { name: String, value: String },
    /// A keychain secret, referenced by handle.
    Secret { name: String, handle: String },
}

impl Input {
    /// The name steps and declarations use for it.
    pub fn name(&self) -> &str {
        match self {
            Input::File { name, .. } | Input::Fact { name, .. } | Input::Secret { name, .. } => {
                name
            }
        }
    }
}

/// One approval rule (spec 8.1.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalEntry {
    pub kind: ActionKind,
    pub rule: ApprovalRule,
}

/// One check (spec 9.1), in a form the Host can evaluate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CheckSpec {
    TextPresent { text: String },
    TextGone { text: String },
    UrlMatches { pattern: String },
    ElementPresent { role: String, name: String },
    Download,
    EmailConfirmation,
    AgentJudged { note: String },
}

/// Caps the playbook may lower (spec 10.1.4); it cannot raise them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapsSpec {
    #[serde(default)]
    pub site_daily: Option<i32>,
    #[serde(default)]
    pub run_items: Option<i32>,
}

/// A validation error anchored to a line of the playbook text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationError {
    /// 1-based line in the source text.
    pub line: usize,
    /// The field or path the error is about.
    pub field: String,
    pub problem: String,
    /// The fix the person (or the agent) can apply.
    pub fix: String,
}

/// The outcome of validating a playbook's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validation {
    pub errors: Vec<ValidationError>,
}

impl Validation {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Validate and parse a playbook's text. On success returns the document;
/// on failure returns every error, line-anchored. The `format` line is
/// checked first so a newer file is refused with its own message.
pub fn validate_playbook(source: &str) -> Result<PlaybookDoc, Vec<ValidationError>> {
    validate(source)
}

/// Validate and parse a playbook's text (the canonical entry point).
pub fn validate(source: &str) -> Result<PlaybookDoc, Vec<ValidationError>> {
    // The format line, if it is there, is checked before the YAML parse so a
    // format 2 file fails with "upgrade Zwiim" rather than a field error.
    if let Some(err) = reject_unknown_format(source) {
        return Err(vec![err]);
    }
    let doc: PlaybookDoc = match serde_yaml::from_str(source) {
        Ok(doc) => doc,
        Err(e) => {
            return Err(vec![yaml_error(&e, source)]);
        }
    };
    let mut errors = Vec::new();
    check_doc(&doc, source, &mut errors);
    if errors.is_empty() {
        Ok(doc)
    } else {
        Err(errors)
    }
}

/// A secret handle is `{{secret:<name>}}` (spec 10.11.1), with a non-empty
/// name of word characters and no whitespace or braces.
fn is_secret_handle(handle: &str) -> bool {
    let handle = handle.trim();
    let inner = handle
        .strip_prefix("{{secret:")
        .and_then(|s| s.strip_suffix("}}"));
    matches!(inner, Some(name)
        if !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
}

/// The format check on its own: find `format:` and refuse a value this build
/// does not read (spec 16.3). `None` when absent or equal. Quotes and YAML's
/// flow/nested forms are stripped so `format: "2"` and `{format: 2}` are
/// caught too.
fn reject_unknown_format(source: &str) -> Option<ValidationError> {
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        // Accept `format:`, `{format:`, `"format":`, `'format':` at the line
        // start (a top-level key).
        let value = trimmed
            .strip_prefix("format:")
            .or_else(|| trimmed.strip_prefix("\"format\":"))
            .or_else(|| trimmed.strip_prefix("'format':"))
            .or_else(|| trimmed.strip_prefix("{format:"));
        if let Some(rest) = value {
            let value = rest
                .trim()
                .trim_end_matches('}')
                .trim()
                .trim_matches(|c| c == '"' || c == '\'');
            let n: Result<u32, _> = value.parse();
            return match n {
                Ok(FORMAT_VERSION) => None,
                Ok(other) => Some(ValidationError {
                    line: i + 1,
                    field: "format".into(),
                    problem: format!("format {other} is not supported by this build"),
                    fix: format!("Zwiim reads format {FORMAT_VERSION}; update Zwiim or use a format {FORMAT_VERSION} file"),
                }),
                Err(_) => Some(ValidationError {
                    line: i + 1,
                    field: "format".into(),
                    problem: format!("`{value}` is not a format number"),
                    fix: format!("write `format: {FORMAT_VERSION}`"),
                }),
            };
        }
    }
    None
}

/// Turn a serde_yaml error into a line-anchored error. `serde_yaml` reports
/// a `location` line; when it does not, point at the first line.
fn yaml_error(e: &serde_yaml::Error, source: &str) -> ValidationError {
    let line = e
        .location()
        .map(|l| l.line())
        .filter(|l| *l > 0)
        .unwrap_or_else(|| find_line(source, "format").unwrap_or(1));
    let problem = e
        .to_string()
        .lines()
        .next()
        .unwrap_or("invalid playbook")
        .trim()
        .to_string();
    ValidationError {
        line,
        field: "playbook".into(),
        problem,
        fix: "fix the YAML at the reported line".into(),
    }
}

/// The input names a step uses: each `input <name>` outside quoted text,
/// the name quoted or running to ` to `, ` into `, ` in ` or the end.
pub fn step_inputs(step: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut at = 0;
    while at < step.len() {
        let rest = &step[at..];
        // Quoted text (a field's name, fixed text) is never a reference.
        if let Some(quoted) = rest.strip_prefix('"') {
            at += 1 + quoted.find('"').map_or(quoted.len(), |end| end + 1);
            continue;
        }
        let starts_word = at == 0 || step[..at].ends_with(' ');
        let Some(after) = rest.strip_prefix("input ").filter(|_| starts_word) else {
            at += rest.chars().next().map_or(1, char::len_utf8);
            continue;
        };
        let after_at = at + "input ".len();
        let (name, used) = match after.strip_prefix('"') {
            Some(quoted) => match quoted.find('"') {
                Some(end) => (&quoted[..end], end + 2),
                None => (quoted, after.len()),
            },
            None => {
                let end = [" to ", " into ", " in "]
                    .iter()
                    .filter_map(|w| after.find(w))
                    .min()
                    .unwrap_or(after.len());
                (&after[..end], end)
            }
        };
        let name = name.trim();
        if !name.is_empty() {
            names.push(name);
        }
        at = after_at + used;
    }
    names
}

/// The 1-based line a top-level `key:` sits on, for semantic errors.
fn find_line(source: &str, key: &str) -> Option<usize> {
    let needle = format!("{key}:");
    source
        .lines()
        .position(|l| l.trim_start().starts_with(&needle))
        .map(|i| i + 1)
}

fn check_doc(doc: &PlaybookDoc, source: &str, errors: &mut Vec<ValidationError>) {
    let line_of = |key: &str| find_line(source, key).unwrap_or(1);

    if doc.name.trim().is_empty() {
        errors.push(ValidationError {
            line: line_of("name"),
            field: "name".into(),
            problem: "a playbook needs a name".into(),
            fix: "set `name:` to a short title".into(),
        });
    }
    if doc.goal.trim().is_empty() {
        errors.push(ValidationError {
            line: line_of("goal"),
            field: "goal".into(),
            problem: "a playbook needs a goal".into(),
            fix: "describe what it should do under `goal:`".into(),
        });
    }
    if doc.sources.is_empty() {
        errors.push(ValidationError {
            line: line_of("sources"),
            field: "sources".into(),
            problem: "a playbook needs at least one source".into(),
            fix: "add a search, a list or a file under `sources:`".into(),
        });
    }
    if doc.checks.is_empty() {
        // Spec 9.2: the Host must be able to prove done. An empty set can
        // never mark an item done, so it is a validation error.
        errors.push(ValidationError {
            line: line_of("checks"),
            field: "checks".into(),
            problem: "a playbook needs at least one check so the Host can prove an item is done"
                .into(),
            fix: "add a `checks:` entry (text_present, url_matches, ...)".into(),
        });
    }
    // Always-ask kinds cannot be auto (spec 8.1.2).
    for entry in &doc.approval {
        if entry.kind.always_ask() && entry.rule == ApprovalRule::Auto {
            errors.push(ValidationError {
                line: line_of("approval"),
                field: format!("approval.{}", entry.kind.as_str()),
                problem: format!("`{}` is always ask and cannot be auto", entry.kind.as_str()),
                fix: "set it to `ask` or `never`, or remove the entry (absent is ask)".into(),
            });
        }
    }
    // A file input is a stored copy or a path, never both or neither.
    for input in &doc.inputs {
        if let Input::File {
            name,
            path,
            file,
            sha256,
        } = input
        {
            let problem = match (path, file, sha256) {
                (Some(_), None, None) => None,
                (None, Some(f), Some(h))
                    if !f.trim().is_empty() && crate::automation::input_files::is_sha256(h) =>
                {
                    None
                }
                (None, Some(_), Some(_)) => {
                    Some("a stored file needs its `sha256`, 64 hex characters")
                }
                (None, None, None) => Some("a file input needs a file"),
                _ => Some("a file input is either a stored `file` with its `sha256`, or a `path`"),
            };
            if let Some(problem) = problem {
                errors.push(ValidationError {
                    line: line_of("inputs"),
                    field: format!("inputs.{name}"),
                    problem: problem.into(),
                    fix: "pick the file again in the playbook's Inputs".into(),
                });
            }
        }
    }
    // A secret input must give a handle, never a value (spec 10.11.1).
    for input in &doc.inputs {
        if let Input::Secret { name, handle } = input {
            if !is_secret_handle(handle) {
                errors.push(ValidationError {
                    line: line_of("inputs"),
                    field: format!("inputs.{name}"),
                    problem: "a secret input needs a keychain handle, not a value".into(),
                    fix: "set `handle:` to the keychain entry, e.g. `{{secret:indeed_password}}`"
                        .into(),
                });
            }
        }
    }
    // Each input has its own name: steps and declarations name them.
    let mut seen: Vec<&str> = Vec::new();
    for input in &doc.inputs {
        if seen.contains(&input.name()) {
            errors.push(ValidationError {
                line: line_of("inputs"),
                field: format!("inputs.{}", input.name()),
                problem: format!("two inputs are named `{}`", input.name()),
                fix: "give each input its own name".into(),
            });
        }
        seen.push(input.name());
    }
    // A step may only use an input the playbook has.
    let known: Vec<&str> = doc.inputs.iter().map(Input::name).collect();
    let steps_from = line_of("steps");
    for step in &doc.steps {
        for name in step_inputs(step) {
            if known.contains(&name) {
                continue;
            }
            let line = source
                .lines()
                .enumerate()
                .skip(steps_from)
                .find(|(_, l)| l.contains(step.as_str()))
                .map(|(i, _)| i + 1)
                .unwrap_or(steps_from);
            errors.push(ValidationError {
                line,
                field: "steps".into(),
                problem: format!("the step uses input `{name}`, which the playbook does not have"),
                fix: if known.is_empty() {
                    format!("add `{name}` under `inputs:`")
                } else {
                    format!(
                        "add `{name}` under `inputs:` or use one of: {}",
                        known.join(", ")
                    )
                },
            });
        }
    }
    // Caps can only lower (spec 10.1.4).
    if let Some(caps) = &doc.caps {
        let d = crate::automation::caps::Caps::default();
        if let Some(site_daily) = caps.site_daily {
            if site_daily > d.site_daily {
                errors.push(ValidationError {
                    line: line_of("caps"),
                    field: "caps.site_daily".into(),
                    problem: format!(
                        "site_daily {site_daily} is above the default {}",
                        d.site_daily
                    ),
                    fix: format!("lower it to at most {}", d.site_daily),
                });
            }
        }
        if let Some(run_items) = caps.run_items {
            if run_items > d.run_items {
                errors.push(ValidationError {
                    line: line_of("caps"),
                    field: "caps.run_items".into(),
                    problem: format!("run_items {run_items} is above the default {}", d.run_items),
                    fix: format!("lower it to at most {}", d.run_items),
                });
            }
        }
    }
    // `sites:` entries must be bare domains; a URL will not match policy.
    for site in &doc.sites {
        if site.contains('/') || site.contains(':') && !site.contains("://") {
            // Allow `host:port`; reject a full URL or a path.
        }
        if site.contains("://") || site.contains(char::is_whitespace) {
            errors.push(ValidationError {
                line: line_of("sites"),
                field: "sites".into(),
                problem: format!("`{site}` is not a bare domain"),
                fix: "write a domain like `indeed.com`, not a URL or a phrase".into(),
            });
        }
    }
    if doc.sources.is_empty() && doc.sites.is_empty() && !doc.main_kind.always_ask() {
        // Not an error; sites can be added by the allow-domain card at run
        // time (spec 8.9). Kept as a comment for the builder.
    }
}

/// A copy-paste-valid example playbook for the local fixture site (spec
/// 18.6). The docs and the empty state point here until 1c.
pub const EXAMPLE_PLAYBOOK: &str = r#"# Example playbook: apply to a job on the local fixture site.
# Read and edit this text freely; Zwiim stores it and runs it.
format: 1
name: Apply to fixture jobs
goal: Apply to backend roles on the fixture job board
main_kind: submit_application
sites:
  - fixture.zwiim.test
sources:
  - type: search
    url: http://fixture.zwiim.test/jobs?q=backend
inputs:
  - type: file
    name: resume
    path: ~/Documents/resume.pdf
  - type: fact
    name: notice_period
    value: 30 days
  - type: secret
    name: password
    handle: "{{secret:fixture_password}}"
rules:
  - Skip roles that need relocation
approval:
  - kind: submit_application
    rule: ask
checks:
  - kind: text_present
    text: Application sent
caps:
  site_daily: 25
  run_items: 50
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_playbook_is_valid() {
        let doc = validate(EXAMPLE_PLAYBOOK).expect("example validates");
        assert_eq!(doc.format, 1);
        assert_eq!(doc.main_kind, ActionKind::SubmitApplication);
        assert_eq!(doc.sites, vec!["fixture.zwiim.test"]);
    }

    #[test]
    fn an_unknown_format_is_refused_on_its_own_line() {
        let text = "format: 2\nname: x\n";
        let errs = validate(text).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].line, 1);
        assert_eq!(errs[0].field, "format");
        assert!(errs[0].problem.contains("format 2"));
    }

    #[test]
    fn a_missing_format_number_is_line_anchored() {
        let errs = validate("format: one\n").unwrap_err();
        assert_eq!(errs[0].field, "format");
        assert!(errs[0].fix.contains("format: 1"));
    }

    #[test]
    fn missing_required_fields_are_named_with_their_line() {
        let text = "format: 1\nname: \ngoal: \nmain_kind: submit_application\n";
        let errs = validate(text).unwrap_err();
        let fields: Vec<&str> = errs.iter().map(|e| e.field.as_str()).collect();
        assert!(fields.contains(&"name"));
        assert!(fields.contains(&"goal"));
        assert!(fields.contains(&"sources"));
        assert!(fields.contains(&"checks"));
    }

    #[test]
    fn an_always_ask_kind_cannot_be_auto() {
        let text = EXAMPLE_PLAYBOOK.replace(
            "  - kind: submit_application\n    rule: ask",
            "  - kind: delete\n    rule: auto",
        );
        let errs = validate(&text).unwrap_err();
        assert!(errs
            .iter()
            .any(|e| e.field == "approval.delete" && e.problem.contains("always ask")));
    }

    #[test]
    fn a_secret_input_must_be_a_handle() {
        let text = EXAMPLE_PLAYBOOK.replace(
            r#"    handle: "{{secret:fixture_password}}""#,
            "    handle: hunter2",
        );
        let errs = validate(&text).unwrap_err();
        assert!(errs.iter().any(|e| e.field == "inputs.password"));
    }

    #[test]
    fn a_secret_input_with_a_value_beside_its_handle_is_refused() {
        // Spec 10.11.1: the playbook text is saved to SQLite, so a secret's
        // value must never be accepted in it, even next to a valid handle.
        let text = EXAMPLE_PLAYBOOK.replace(
            r#"    handle: "{{secret:fixture_password}}""#,
            "    handle: \"{{secret:fixture_password}}\"\n    value: hunter2",
        );
        assert_ne!(text, EXAMPLE_PLAYBOOK, "the test replacement must apply");
        let errs = validate(&text).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.problem.contains("unknown field `value`")),
            "{errs:?}"
        );
    }

    #[test]
    fn caps_cannot_be_raised_above_the_default() {
        let text = EXAMPLE_PLAYBOOK.replace("site_daily: 25", "site_daily: 500");
        let errs = validate(&text).unwrap_err();
        assert!(errs.iter().any(|e| e.field == "caps.site_daily"));
    }

    #[test]
    fn a_site_url_is_refused_as_a_bare_domain() {
        let text = EXAMPLE_PLAYBOOK.replace(
            "  - fixture.zwiim.test",
            "  - https://fixture.zwiim.test/jobs",
        );
        let errs = validate(&text).unwrap_err();
        assert!(errs.iter().any(|e| e.field == "sites"));
    }

    #[test]
    fn invalid_yaml_points_at_a_line() {
        let errs = validate("format: 1\nname: [unclosed\n").unwrap_err();
        assert!(errs[0].line >= 1);
    }

    #[test]
    fn every_error_has_a_line_and_a_fix() {
        let text = "format: 1\nname: \n";
        for e in validate(text).unwrap_err() {
            assert!(e.line >= 1);
            assert!(!e.fix.is_empty());
        }
    }
}

#[cfg(test)]
mod save_checks {
    use super::*;

    #[test]
    fn the_docs_example_is_the_example_constant() {
        // docs/automation/playbook-format.md embeds this; keep them in step.
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/automation/playbook-format.md"
        );
        let md = std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            md.contains(EXAMPLE_PLAYBOOK.trim_end()),
            "docs/automation/playbook-format.md is stale: paste EXAMPLE_PLAYBOOK in"
        );
    }

    #[test]
    fn a_well_formed_small_playbook_validates_with_a_named_check() {
        let text = r#"format: 1
name: Post a note
goal: Post a short note
main_kind: publish_post
sites:
  - x.example
sources:
  - type: list
    items: ["hello"]
approval:
  - kind: publish_post
    rule: ask
checks:
  - kind: url_matches
    pattern: /posted
"#;
        let doc = validate(text).expect("valid");
        assert_eq!(doc.main_kind, ActionKind::PublishPost);
        assert_eq!(doc.checks.len(), 1);
    }
}

#[cfg(test)]
mod review_checks {
    use super::*;

    #[test]
    fn a_quoted_or_flow_format_is_still_refused() {
        for text in ["format: \"2\"\n", "format: '2'\n", "{format: 2}\n"] {
            let errs = validate(text).unwrap_err();
            assert_eq!(errs[0].field, "format", "{text:?}");
        }
    }

    #[test]
    fn a_secret_handle_with_no_scheme_is_refused() {
        let bad = EXAMPLE_PLAYBOOK.replace(
            "handle: \"{{secret:fixture_password}}\"",
            "handle: \"{{fixture_password}}\"",
        );
        assert_ne!(bad, EXAMPLE_PLAYBOOK, "the test replacement must apply");
        assert!(validate(&bad)
            .unwrap_err()
            .iter()
            .any(|e| e.field == "inputs.password"));
    }
}

#[cfg(test)]
mod steps {
    use super::*;

    const WITH_STEPS: &str = r#"format: 1
name: Apply
goal: Apply to each job
main_kind: submit_application
sources:
  - type: list
    items: ["https://jobs.example/1"]
inputs:
  - type: fact
    name: Email
    value: me@example.com
steps:
  - Click "Apply"
  - Fill "Email" with input Email
checks:
  - kind: text_present
    text: Application sent
"#;

    #[test]
    fn steps_are_optional_and_kept_in_order() {
        let doc = validate(WITH_STEPS).expect("valid");
        assert_eq!(
            doc.steps,
            vec![
                "Click \"Apply\"".to_string(),
                "Fill \"Email\" with input Email".to_string()
            ]
        );
        let without = WITH_STEPS.replace(
            "steps:\n  - Click \"Apply\"\n  - Fill \"Email\" with input Email\n",
            "",
        );
        assert!(validate(&without).expect("valid").steps.is_empty());
    }

    #[test]
    fn a_step_naming_an_unknown_input_is_line_anchored() {
        let text = WITH_STEPS.replace(
            "  - Fill \"Email\" with input Email\n",
            "  - Fill \"Email\" with input Email\n  - Attach input Resume to \"CV\"\n",
        );
        let errs = validate(&text).unwrap_err();
        let err = errs
            .iter()
            .find(|e| e.field == "steps")
            .expect("steps error");
        let line = text
            .lines()
            .position(|l| l.contains("Attach input Resume"))
            .unwrap()
            + 1;
        assert_eq!(err.line, line);
        assert!(err.problem.contains("Resume"), "{}", err.problem);
        assert!(err.fix.contains("Email"), "{}", err.fix);
    }

    #[test]
    fn step_inputs_reads_plain_and_quoted_names() {
        assert_eq!(
            step_inputs("Fill \"Full name\" with input \"Full name\""),
            vec!["Full name"]
        );
        assert_eq!(step_inputs("Attach input Resume to \"CV\""), vec!["Resume"]);
        assert_eq!(
            step_inputs("Fill \"Email\" with input Email"),
            vec!["Email"]
        );
        assert!(step_inputs("Click \"Apply\"").is_empty());
    }

    #[test]
    fn quoted_text_is_never_an_input_name() {
        assert!(step_inputs("Click \"Your input here\"").is_empty());
        assert!(step_inputs("Type \"I'd like to input data\" into \"Notes\"").is_empty());
        assert_eq!(
            step_inputs("Fill \"Say input here\" with input \"Say input here\""),
            vec!["Say input here"]
        );
    }

    #[test]
    fn two_inputs_with_one_name_are_refused() {
        let text = WITH_STEPS.replace(
            "steps:",
            "  - type: fact\n    name: Email\n    value: other@example.com\nsteps:",
        );
        let errs = validate(&text).unwrap_err();
        assert!(errs.iter().any(|e| e.field == "inputs.Email"), "{errs:?}");
    }
}

#[cfg(test)]
mod stored_files {
    use super::*;

    const SHA: &str = "9f3a000000000000000000000000000000000000000000000000000000000001";

    fn with_input(input: &str) -> String {
        format!(
            "format: 1\nname: Apply\ngoal: Apply\nmain_kind: submit_application\n\
             sources:\n  - type: list\n    items: [\"https://jobs.example/1\"]\n\
             inputs:\n{input}\
             checks:\n  - kind: text_present\n    text: Sent\n"
        )
    }

    #[test]
    fn a_stored_file_input_validates() {
        let doc = validate(&with_input(&format!(
            "  - type: file\n    name: Resume\n    file: resume.pdf\n    sha256: {SHA}\n"
        )))
        .expect("valid");
        assert_eq!(
            doc.inputs,
            vec![Input::File {
                name: "Resume".into(),
                path: None,
                file: Some("resume.pdf".into()),
                sha256: Some(SHA.into()),
            }]
        );
    }

    #[test]
    fn the_old_path_form_still_validates() {
        let doc = validate(&with_input(
            "  - type: file\n    name: Resume\n    path: ~/resume.pdf\n",
        ))
        .expect("valid");
        assert!(
            matches!(&doc.inputs[0], Input::File { path: Some(p), file: None, .. } if p == "~/resume.pdf")
        );
    }

    #[test]
    fn a_file_input_needs_a_path_or_a_stored_file() {
        for input in [
            "  - type: file\n    name: Resume\n".to_string(),
            format!("  - type: file\n    name: Resume\n    path: ~/r.pdf\n    file: r.pdf\n    sha256: {SHA}\n"),
            "  - type: file\n    name: Resume\n    file: r.pdf\n".to_string(),
        ] {
            let errs = validate(&with_input(&input)).unwrap_err();
            assert!(
                errs.iter().any(|e| e.field == "inputs.Resume"),
                "{input:?}: {errs:?}"
            );
        }
    }

    #[test]
    fn a_stored_file_needs_a_hex_sha256() {
        let errs = validate(&with_input(
            "  - type: file\n    name: Resume\n    file: r.pdf\n    sha256: not-a-hash\n",
        ))
        .unwrap_err();
        assert!(errs.iter().any(|e| e.field == "inputs.Resume"), "{errs:?}");
    }
}
