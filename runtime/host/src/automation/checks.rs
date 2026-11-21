// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: Host-evaluated checks (automation-engine spec 9, 6.8).
//!
//! The Host, never the agent, decides `done`: an item is done only when every
//! required check passes. A check the Host cannot evaluate mechanically may
//! be marked agent-judged; that is shown weaker in the record and never
//! feeds route learning (1b).

use serde::{Deserialize, Serialize};

/// What a check looks for on the page (spec 9.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Check {
    /// The text is present on the page.
    TextPresent { text: String },
    /// The text is gone from the page.
    TextGone { text: String },
    /// The current URL matches the (substring) pattern.
    UrlMatches { pattern: String },
    /// An element matching the descriptor is present.
    ElementPresent { role: String, name: String },
    /// A download completed this item.
    Download,
    /// An email-like confirmation address is on the page.
    EmailConfirmation,
    /// The Host cannot evaluate it mechanically; weaker, never feeds routes.
    AgentJudged { note: String },
}

impl From<&crate::automation::playbook_format::CheckSpec> for Check {
    fn from(spec: &crate::automation::playbook_format::CheckSpec) -> Self {
        use crate::automation::playbook_format::CheckSpec;
        match spec.clone() {
            CheckSpec::TextPresent { text } => Check::TextPresent { text },
            CheckSpec::TextGone { text } => Check::TextGone { text },
            CheckSpec::UrlMatches { pattern } => Check::UrlMatches { pattern },
            CheckSpec::ElementPresent { role, name } => Check::ElementPresent { role, name },
            CheckSpec::Download => Check::Download,
            CheckSpec::EmailConfirmation => Check::EmailConfirmation,
            CheckSpec::AgentJudged { note } => Check::AgentJudged { note },
        }
    }
}

impl Check {
    pub fn learned_by_host(&self) -> bool {
        !matches!(self, Check::AgentJudged { .. })
    }
}

/// The facts the Host gathered for a page (spec 9.1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageFacts {
    pub text: String,
    pub url: String,
    /// Present element names, as `role:name`.
    pub elements: std::collections::BTreeSet<String>,
    pub downloaded: bool,
}

impl PageFacts {
    /// The expression that reads a page's facts in the world whose last
    /// snapshot was `generation`; its value is JSON (see [`Self::parse`]).
    pub fn read_call(generation: &str) -> String {
        format!(
            "(() => {{ const A = globalThis.{global}; const gen = {generation}; \
             const elements = []; \
             const walk = (root) => {{ for (const el of root.querySelectorAll('*')) {{ \
               const r = el._ariaRef; \
               if (r && A && A.elementForRef(gen, r.ref) === el) elements.push(r.role + ':' + r.name); \
               if (el.shadowRoot) walk(el.shadowRoot); }} }}; \
             walk(document); \
             return JSON.stringify({{ text: document.body ? document.body.innerText : '', \
               url: String(location.href), elements }}); }})()",
            global = crate::browser::agent::AGENT_GLOBAL,
            generation = serde_json::Value::from(generation),
        )
    }

    /// The facts from the world's answer to [`Self::read_call`].
    pub fn parse(raw: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct Answer {
            text: String,
            url: String,
            elements: Vec<String>,
        }
        let answer: Answer = serde_json::from_str(raw)
            .map_err(|e| format!("the page answered with unreadable facts: {e}"))?;
        Ok(Self {
            text: answer.text,
            url: answer.url,
            elements: answer.elements.into_iter().collect(),
            downloaded: false,
        })
    }

    fn has_element(&self, role: &str, name: &str) -> bool {
        self.elements.contains(&format!("{role}:{name}"))
    }
}

/// One check's result, as the run record shows it (spec 11.1 `checks`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    pub ok: bool,
    /// Weaker when agent-judged (shown in the record).
    pub agent_judged: bool,
}

/// Evaluate every check against `facts`. Agent-judged checks pass only on
/// the agent's word and are recorded as such.
pub fn evaluate(checks: &[Check], facts: &PageFacts, agent_says_done: bool) -> Vec<CheckResult> {
    checks
        .iter()
        .map(|check| {
            let (name, ok, agent_judged) = match check {
                Check::TextPresent { text } => (
                    "text_present".to_string(),
                    facts.text.contains(text.as_str()),
                    false,
                ),
                Check::TextGone { text } => (
                    "text_gone".to_string(),
                    !facts.text.contains(text.as_str()),
                    false,
                ),
                Check::UrlMatches { pattern } => (
                    "url_matches".to_string(),
                    facts.url.contains(pattern.as_str()),
                    false,
                ),
                Check::ElementPresent { role, name: n } => (
                    "element_present".to_string(),
                    facts.has_element(role, n),
                    false,
                ),
                Check::Download => ("download".to_string(), facts.downloaded, false),
                Check::EmailConfirmation => (
                    "email_confirmation".to_string(),
                    looks_like_email(&facts.text),
                    false,
                ),
                Check::AgentJudged { note } => {
                    (format!("agent_judged: {note}"), agent_says_done, true)
                }
            };
            CheckResult {
                name,
                ok,
                agent_judged,
            }
        })
        .collect()
}

/// Whether every required check passed (spec 9.2): the item is done only
/// then. An empty set of checks is not proof, so it fails.
pub fn all_pass(results: &[CheckResult]) -> bool {
    !results.is_empty() && results.iter().all(|r| r.ok)
}

/// Whether every check was Host-evaluated, so learning may use the steps
/// (spec 9.3, 7.6.2).
pub fn all_host_evaluated(results: &[CheckResult]) -> bool {
    results.iter().all(|r| !r.agent_judged)
}

fn looks_like_email(text: &str) -> bool {
    // A loose match: something@something.something.
    let bytes = text.as_bytes();
    let mut at = None;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'@' {
            if at.is_some() {
                return true;
            }
            at = Some(i);
        }
    }
    let Some(at) = at else { return false };
    let before_ok = at > 0 && !text[..at].trim_end().ends_with(char::is_whitespace);
    let after = &text[at + 1..];
    let dot = after.find('.');
    before_ok && dot.map(|d| d > 0 && d + 1 < after.len()).unwrap_or(false)
}

/// The after-the-fact check (spec 6.8). Exactly one match is `done`; none,
/// two or unreadable retry, then go to the person. Never a redo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AfterTheFactCheck {
    /// The list page URL (the site's "applied" or "my posts").
    pub url: String,
    /// The item's ID on the site, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// The subject or exact body text to match (whitespace-normalized).
    pub subject: String,
}

/// How many matches the Host found on the list page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterTheFactResult {
    Found,
    NotFound,
    Ambiguous,
    Unreadable,
}

impl AfterTheFactCheck {
    /// Count matches in the list page's text: the item ID plus the subject,
    /// or the exact body text. `normalized_page` is the page text with
    /// whitespace collapsed.
    pub fn count_matches(&self, normalized_page: &str) -> AfterTheFactResult {
        if normalized_page.is_empty() {
            return AfterTheFactResult::Unreadable;
        }
        let needle = normalize(&self.subject);
        if needle.is_empty() {
            return AfterTheFactResult::Ambiguous;
        }
        let n = normalized_page.matches(&needle).count();
        match (n, &self.item_id) {
            (0, _) => AfterTheFactResult::NotFound,
            (1, Some(id)) => {
                if normalized_page.contains(normalize(id).as_str()) {
                    AfterTheFactResult::Found
                } else {
                    AfterTheFactResult::NotFound
                }
            }
            (1, None) => AfterTheFactResult::Found,
            (_, _) => AfterTheFactResult::Ambiguous,
        }
    }

    /// Whether a result settles the item (spec 6.8.3): only `Found`.
    pub fn settles(result: AfterTheFactResult) -> bool {
        matches!(result, AfterTheFactResult::Found)
    }
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(text: &str) -> PageFacts {
        PageFacts {
            text: text.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn a_present_text_check_passes_and_a_required_set_gates_done() {
        let checks = vec![
            Check::TextPresent {
                text: "Application sent".into(),
            },
            Check::TextGone {
                text: "Submit application".into(),
            },
        ];
        let results = evaluate(&checks, &facts("Application sent"), true);
        assert!(all_pass(&results));

        let results = evaluate(&checks, &facts("Still here"), true);
        assert!(!all_pass(&results));
    }

    #[test]
    fn an_empty_check_set_is_never_done() {
        assert!(!all_pass(&[]));
    }

    #[test]
    fn an_agent_judged_check_is_weaker_and_blocks_learning() {
        let checks = vec![Check::AgentJudged {
            note: "essay looks right".into(),
        }];
        assert!(!Check::AgentJudged { note: "x".into() }.learned_by_host());
        let passed = evaluate(&checks, &facts(""), true);
        assert!(all_pass(&passed));
        assert!(!all_host_evaluated(&passed));
        // The agent saying no fails it.
        assert!(!all_pass(&evaluate(&checks, &facts(""), false)));
    }

    #[test]
    fn url_and_element_checks_read_the_facts() {
        let mut page = facts("ok");
        page.url = "https://site.example/applied".into();
        page.elements.insert("heading:Application sent".into());
        let checks = vec![
            Check::UrlMatches {
                pattern: "/applied".into(),
            },
            Check::ElementPresent {
                role: "heading".into(),
                name: "Application sent".into(),
            },
        ];
        assert!(all_pass(&evaluate(&checks, &page, false)));
    }

    #[test]
    fn email_confirmation_is_recognised() {
        assert!(looks_like_email(
            "We sent a confirmation to asha@example.com."
        ));
        assert!(!looks_like_email("no address here"));
        assert!(!looks_like_email("@ alone"));
        assert!(!looks_like_email("a@b"));
    }

    #[test]
    fn after_the_fact_needs_exactly_one_match() {
        let check = AfterTheFactCheck {
            url: "https://site.example/applied".into(),
            item_id: Some("9f2c".into()),
            subject: "Backend Engineer @ Acme".into(),
        };
        assert_eq!(
            check.count_matches("applied: 9f2c Backend Engineer @ Acme"),
            AfterTheFactResult::Found
        );
        assert_eq!(
            check.count_matches("applied list is empty"),
            AfterTheFactResult::NotFound
        );
        // The subject twice is ambiguous.
        assert_eq!(
            check.count_matches("Backend Engineer @ Acme ... Backend Engineer @ Acme"),
            AfterTheFactResult::Ambiguous
        );
        // The subject once but the ID missing is not a match.
        assert_eq!(
            check.count_matches("Backend Engineer @ Acme"),
            AfterTheFactResult::NotFound
        );
        assert_eq!(check.count_matches(""), AfterTheFactResult::Unreadable);
        assert!(AfterTheFactCheck::settles(AfterTheFactResult::Found));
        assert!(!AfterTheFactCheck::settles(AfterTheFactResult::Ambiguous));
    }

    #[test]
    fn after_the_fact_normalizes_whitespace_for_a_post() {
        let check = AfterTheFactCheck {
            url: "https://x.example/me".into(),
            item_id: None,
            subject: "a   short\npost".into(),
        };
        assert_eq!(
            check.count_matches("today: a short post here"),
            AfterTheFactResult::Found
        );
    }

    #[test]
    fn page_facts_parse_from_the_worlds_answer() {
        let facts = PageFacts::parse(
            r#"{"text":"Applied","url":"https://jobs.test/done","elements":["heading:Thanks"]}"#,
        )
        .unwrap();
        let checks = [
            Check::TextPresent {
                text: "Applied".into(),
            },
            Check::ElementPresent {
                role: "heading".into(),
                name: "Thanks".into(),
            },
        ];
        assert!(all_pass(&evaluate(&checks, &facts, false)));
        assert!(PageFacts::parse("absent").is_err());
        assert!(PageFacts::read_call("1.2").contains("\"1.2\""));
    }
}
