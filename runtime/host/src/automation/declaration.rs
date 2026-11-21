// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: the declaration payload and the whole-form pre-submit check
//! (automation-engine spec 8.2, 8.6) and the click guard (8.4).
//!
//! The Host never trusts an agent's word that a form is as it says. Before
//! every declared click it lists the filled controls in the submit scope,
//! adds the ones the page pre-filled and the agent left alone, and refuses
//! the submit if anything else is filled. The same comparison runs for the
//! Host's own approved click.

use serde::{Deserialize, Serialize};

use crate::automation::reasons::{ActionKind, DeclarationDecision};

/// A target descriptor: how the Host finds the element (spec 8.6.4, 11.1).
/// `role` and `name` are the primary pair; `text` and `attrs` break ties.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// ARIA role, e.g. `button`, `textbox`, `combobox`.
    pub role: String,
    /// Accessible name.
    #[serde(default)]
    pub name: String,
    /// Visible text, when the name is not enough.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// A few stable attributes (id, name, aria-label).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub attrs: std::collections::BTreeMap<String, String>,
}

impl Target {
    pub fn new(role: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            name: name.into(),
            text: None,
            attrs: Default::default(),
        }
    }
}

/// One declared field: a control and the value the agent means to put in it.
/// Tagged so a secret or a file never silently deserializes as plain text
/// (spec 8.2.1, 10.11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "value_kind", rename_all = "snake_case")]
pub enum DeclaredValue {
    /// A plain text value.
    Text { text: String },
    /// A handle such as `{{secret:indeed_password}}`; the Host types the
    /// real value and never records it (spec 10.11).
    Secret { handle: String },
    /// An input file the Host attaches from `playbook_attach_file`.
    InputFile { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredField {
    pub target: Target,
    /// The control's kind, for the per-type comparison (spec 8.6.4).
    pub control: ControlKind,
    pub value: DeclaredValue,
}

/// How a control is compared after the click is declared (spec 8.6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlKind {
    Text,
    TextArea,
    ContentEditable,
    Select,
    Checkbox,
    Radio,
    File,
    Secret,
}

impl ControlKind {
    pub fn compares_by_label(self) -> bool {
        matches!(self, ControlKind::Select | ControlKind::ContentEditable)
    }
}

/// The declaration the agent makes before an outward click (spec 8.2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declaration {
    pub kind: ActionKind,
    pub summary: String,
    pub fields: Vec<DeclaredField>,
    pub submit_target: Target,
}

/// What the Host saw in the submit scope at check time (spec 8.6.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeControl {
    pub target: Target,
    pub control: ControlKind,
    /// The current value as the Host read it: text, chosen label, checked
    /// state, file name, or a secret's real value (never recorded).
    pub value: String,
    pub hidden: bool,
    pub read_only: bool,
    pub disabled: bool,
    /// The page pre-filled it and nobody changed it since load.
    pub prefilled: bool,
}

/// The result of the pre-submit check (spec 8.6). `allowed` only when every
/// filled control is declared and every declared pair matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreSubmitCheck {
    /// Controls the page pre-filled and the agent left unchanged; the Host
    /// adds these to the declaration and shows them on the approval sheet.
    pub prefilled: Vec<DeclaredField>,
    /// The first mismatch, if any.
    pub mismatch: Option<String>,
}

impl PreSubmitCheck {
    pub fn passed(&self) -> bool {
        self.mismatch.is_none()
    }

    /// The check for the approved or automatic submit. `scope` is what the
    /// Host read from the page; `declared` is what the agent declared.
    pub fn evaluate(declared: &Declaration, scope: &[ScopeControl]) -> Self {
        let mut prefilled = Vec::new();
        for control in scope {
            if control.hidden || control.read_only || control.disabled {
                continue;
            }
            let declared_field = declared
                .fields
                .iter()
                .find(|f| same_target(&f.target, &control.target));
            match declared_field {
                Some(field) => {
                    let expected = expected_value(field);
                    let actual = control.value.trim();
                    if actual != expected.trim() {
                        // Never echo the page value: a secret field's real
                        // value must not reach an error string (spec 10.11).
                        return Self {
                            prefilled,
                            mismatch: Some(format!(
                                "declared field '{}' does not match the page",
                                control.target.name
                            )),
                        };
                    }
                }
                None if control.prefilled => {
                    if matches!(control.control, ControlKind::Secret) {
                        // A secret the page pre-filled and the agent did not
                        // declare is refused, never captured (spec 10.11).
                        return Self {
                            prefilled,
                            mismatch: Some(format!(
                                "secret control '{}' must be declared, not captured",
                                control.target.name
                            )),
                        };
                    }
                    prefilled.push(DeclaredField {
                        target: control.target.clone(),
                        control: control.control,
                        value: DeclaredValue::Text {
                            text: control.value.clone(),
                        },
                    });
                }
                None if !control.value.trim().is_empty() => {
                    // A filled control nobody declared (spec 8.6.3).
                    return Self {
                        prefilled,
                        mismatch: Some(format!(
                            "filled control '{}' is not declared",
                            control.target.name
                        )),
                    };
                }
                None => {}
            }
        }
        Self {
            prefilled,
            mismatch: None,
        }
    }
}

/// The value a declaration promises for a field; a secret's real value is
/// compared by the Host against the bound secret, so the handle stands in.
fn expected_value(field: &DeclaredField) -> String {
    match &field.value {
        DeclaredValue::Text { text } => text.clone(),
        DeclaredValue::Secret { handle } => handle.clone(),
        DeclaredValue::InputFile { name } => name.clone(),
    }
}

fn same_target(a: &Target, b: &Target) -> bool {
    a.role == b.role && a.name == b.name
}

/// The click guard (spec 8.4): a click on a control whose name or form action
/// looks outward is refused unless a matching declaration is held.
const OUTWARD_WORDS: &[&str] = &[
    "submit", "apply", "post", "publish", "send", "pay", "buy", "delete", "confirm",
];

/// Whether the accessible name or form action looks outward (8.4.1). A
/// `not_outward` mark (per playbook per site) exempts the target.
pub fn looks_outward(target: &Target, marked_not_outward: bool) -> bool {
    if marked_not_outward {
        return false;
    }
    let haystack = if target.name.is_empty() {
        target.text.clone().unwrap_or_default()
    } else {
        target.name.clone()
    };
    let action = target
        .attrs
        .get("formaction")
        .or_else(|| target.attrs.get("action"))
        .cloned()
        .unwrap_or_default();
    let haystack = haystack.to_ascii_lowercase();
    let action = action.to_ascii_lowercase();
    OUTWARD_WORDS
        .iter()
        .any(|w| haystack.contains(w) || action.contains(w))
}

/// The declaration's outcome given the playbook's rule and whether a
/// completed show-me run with this kind exists on the site (spec 8.1.2-4).
pub fn decide(
    rule: crate::automation::reasons::ApprovalRule,
    kind: ActionKind,
    show_me: bool,
    show_me_run_for_kind: bool,
) -> DeclarationDecision {
    use crate::automation::reasons::ApprovalRule;
    // A `never` rule refuses the action outright, before show-me or the
    // always-ask kinds can turn it into a pending approval (spec 8.2.2.3).
    if rule == ApprovalRule::Never {
        return DeclarationDecision::Refused;
    }
    // Pay, change-account and delete are always ask.
    if kind.always_ask() || show_me {
        return DeclarationDecision::Pending;
    }
    match rule {
        ApprovalRule::Never => DeclarationDecision::Refused,
        ApprovalRule::Ask => DeclarationDecision::Pending,
        // Auto starts only after the kind's first approved send on the site.
        ApprovalRule::Auto if !show_me_run_for_kind => DeclarationDecision::Pending,
        ApprovalRule::Auto => DeclarationDecision::Allowed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::reasons::ApprovalRule;

    fn target(name: &str) -> Target {
        Target::new("textbox", name)
    }

    fn text(name: &str, value: &str) -> DeclaredField {
        DeclaredField {
            target: target(name),
            control: ControlKind::Text,
            value: DeclaredValue::Text {
                text: value.to_string(),
            },
        }
    }

    fn control(name: &str, value: &str, prefilled: bool) -> ScopeControl {
        ScopeControl {
            target: target(name),
            control: ControlKind::Text,
            value: value.to_string(),
            hidden: false,
            read_only: false,
            disabled: false,
            prefilled,
        }
    }

    fn decl() -> Declaration {
        Declaration {
            kind: ActionKind::SubmitApplication,
            summary: "Apply to Acme".into(),
            fields: vec![text("Name", "Asha"), text("Notice", "30 days")],
            submit_target: Target::new("button", "Submit"),
        }
    }

    #[test]
    fn the_declared_form_submits() {
        let scope = vec![
            control("Name", "Asha", false),
            control("Notice", "30 days", false),
        ];
        assert!(PreSubmitCheck::evaluate(&decl(), &scope).passed());
    }

    #[test]
    fn an_undeclared_filled_field_refuses_the_submit() {
        let mut scope = vec![
            control("Name", "Asha", false),
            control("Notice", "30 days", false),
        ];
        scope.push(control("Salary", "20 LPA", false));
        let check = PreSubmitCheck::evaluate(&decl(), &scope);
        assert!(!check.passed());
        assert!(check.mismatch.unwrap().contains("Salary"));
    }

    #[test]
    fn a_prefilled_field_the_agent_left_alone_is_added_not_refused() {
        let mut scope = vec![
            control("Name", "Asha", false),
            control("Notice", "30 days", false),
        ];
        scope.push(control("Phone", "+91 98…", true));
        let check = PreSubmitCheck::evaluate(&decl(), &scope);
        assert!(check.passed());
        assert_eq!(check.prefilled.len(), 1);
        assert_eq!(check.prefilled[0].target.name, "Phone");
    }

    #[test]
    fn a_changed_declared_value_mismatches() {
        let mut scope = vec![
            control("Name", "Asha", false),
            control("Notice", "60 days", false),
        ];
        scope[1].value = "60 days".into();
        assert!(!PreSubmitCheck::evaluate(&decl(), &scope).passed());
    }

    #[test]
    fn whitespace_is_trimmed_for_comparison() {
        let scope = vec![
            control("Name", "  Asha  ", false),
            control("Notice", "30 days ", false),
        ];
        assert!(PreSubmitCheck::evaluate(&decl(), &scope).passed());
    }

    #[test]
    fn a_secret_field_is_compared_by_handle() {
        let mut d = decl();
        d.fields.push(DeclaredField {
            target: target("Password"),
            control: ControlKind::Secret,
            value: DeclaredValue::Secret {
                handle: "{{secret:pw}}".into(),
            },
        });
        let mut scope = vec![
            control("Name", "Asha", false),
            control("Notice", "30 days", false),
        ];
        scope.push(control("Password", "{{secret:pw}}", false));
        assert!(PreSubmitCheck::evaluate(&d, &scope).passed());
    }

    #[test]
    fn the_click_guard_blocks_outward_names_unless_marked() {
        assert!(looks_outward(&Target::new("button", "Submit"), false));
        assert!(looks_outward(&Target::new("button", "Easy Apply"), false));
        assert!(looks_outward(&Target::new("button", "Post"), false));
        assert!(!looks_outward(&Target::new("button", "Next"), false));
        assert!(!looks_outward(&Target::new("button", "Submit"), true));
    }

    #[test]
    fn show_me_forces_pending_but_never_still_refuses() {
        // Show-me makes auto and ask pending, but a `never` rule refuses
        // outright (spec 8.2.2.3).
        for rule in [ApprovalRule::Auto, ApprovalRule::Ask] {
            assert_eq!(
                decide(rule, ActionKind::SubmitApplication, true, true),
                DeclarationDecision::Pending
            );
        }
        assert_eq!(
            decide(
                ApprovalRule::Never,
                ActionKind::SubmitApplication,
                true,
                true
            ),
            DeclarationDecision::Refused
        );
    }

    #[test]
    fn always_ask_kinds_are_pending_even_when_show_me_is_off() {
        for kind in [
            ActionKind::Pay,
            ActionKind::Delete,
            ActionKind::ChangeAccount,
        ] {
            assert_eq!(
                decide(ApprovalRule::Auto, kind, false, true),
                DeclarationDecision::Pending
            );
        }
    }

    #[test]
    fn auto_needs_the_kind_show_me_run() {
        assert_eq!(
            decide(ApprovalRule::Auto, ActionKind::PublishPost, false, false),
            DeclarationDecision::Pending
        );
        assert_eq!(
            decide(ApprovalRule::Auto, ActionKind::PublishPost, false, true),
            DeclarationDecision::Allowed
        );
        // Every kind waits for its first approved send on the site.
        for &kind in ActionKind::all().iter().filter(|k| !k.always_ask()) {
            assert_eq!(
                decide(ApprovalRule::Auto, kind, false, false),
                DeclarationDecision::Pending,
                "{kind:?}"
            );
            assert_eq!(
                decide(ApprovalRule::Auto, kind, false, true),
                DeclarationDecision::Allowed,
                "{kind:?}"
            );
        }
        assert_eq!(
            decide(ApprovalRule::Never, ActionKind::Follow, false, true),
            DeclarationDecision::Refused
        );
    }
}
