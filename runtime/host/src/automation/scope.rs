// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: reading an outward action's scope from the page (automation
//! spec 8.6.3).
//!
//! The Host finds the one element the declaration's target names, by the
//! role and accessible name its own snapshot gave it. When that element sits
//! in a form, the scope is the form's controls; otherwise (Follow, Like) it
//! is the element alone. The read runs in the Host's own isolated world, so
//! neither the page nor the agent can answer for it.

use serde::Deserialize;

use crate::automation::declaration::{ControlKind, ScopeControl, Target};
use crate::browser::agent::{AGENT_GLOBAL, ENGINE_ABSENT};

/// Why no scope was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeMiss {
    /// No element on the page has the target's role and name.
    NotFound,
    /// More than one does, so the Host cannot tell which is meant.
    Ambiguous,
}

/// What the Host read: the controls the action sends, empty for an element
/// outside any form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub in_form: bool,
    pub controls: Vec<ScopeControl>,
}

/// The expression that reads the scope of `target` in the world whose last
/// snapshot was `generation`; its value is JSON (see [`parse`]).
pub fn scope_call(generation: &str, target: &Target) -> String {
    format!(
        "typeof globalThis.{AGENT_GLOBAL} === 'undefined' ? {absent} : \
         ({SCOPE_JS})(globalThis.{AGENT_GLOBAL}, {generation}, {role}, {name})",
        absent = serde_json::Value::from(ENGINE_ABSENT),
        generation = serde_json::Value::from(generation),
        role = serde_json::Value::from(target.role.as_str()),
        name = serde_json::Value::from(target.name.as_str()),
    )
}

/// Walks the document (open shadow roots too) for elements the current
/// snapshot named with `role` and `name`, then lists the form's controls.
/// A password's value never leaves the page: only whether it is filled.
const SCOPE_JS: &str = r#"(A, gen, role, name) => {
  const all = [];
  const walk = (root) => {
    for (const el of root.querySelectorAll('*')) {
      all.push(el);
      if (el.shadowRoot) walk(el.shadowRoot);
    }
  };
  walk(document);
  const named = (el) => {
    const r = el._ariaRef;
    return r && A.elementForRef(gen, r.ref) === el ? r : null;
  };
  const hits = all.filter((el) => {
    const r = named(el);
    return r && r.role === role && r.name === name;
  });
  if (hits.length !== 1) return JSON.stringify({ found: hits.length });
  const el = hits[0];
  const form = el.form || el.closest('form');
  if (!form) return JSON.stringify({ found: 1, inForm: false, controls: [] });
  const fields = Array.from(form.elements).concat(
    Array.from(form.querySelectorAll('[contenteditable=""],[contenteditable="true"]')));
  const controls = [];
  for (const c of fields) {
    if (c === el || c instanceof HTMLButtonElement || c instanceof HTMLFieldSetElement
        || c instanceof HTMLOutputElement || c instanceof HTMLObjectElement) continue;
    const type = c instanceof HTMLInputElement ? c.type : '';
    if (['submit', 'button', 'reset', 'image'].includes(type)) continue;
    let kind = 'text', value = '', prefilled = false;
    if (c instanceof HTMLSelectElement) {
      kind = 'select';
      const chosen = Array.from(c.selectedOptions).filter((o) => o.value !== '');
      value = chosen.map((o) => o.label).join(', ');
      const noDefault = !Array.from(c.options).some((o) => o.defaultSelected);
      prefilled = chosen.length > 0 && chosen.every((o) => o.defaultSelected || (noDefault && o.index === 0));
    } else if (c instanceof HTMLTextAreaElement) {
      kind = 'text_area';
      value = c.value;
      prefilled = value !== '' && value === c.defaultValue;
    } else if (type === 'checkbox' || type === 'radio') {
      kind = type;
      value = c.checked ? 'checked' : '';
      prefilled = c.checked && c.defaultChecked;
    } else if (type === 'file') {
      kind = 'file';
      value = Array.from(c.files || []).map((f) => f.name).join(', ');
    } else if (type === 'password') {
      kind = 'secret';
      value = c.value === '' ? '' : 'filled';
    } else if (c instanceof HTMLInputElement) {
      value = c.value;
      prefilled = value !== '' && value === c.defaultValue;
    } else {
      kind = 'content_editable';
      value = c.innerText || '';
    }
    const r = named(c);
    const box = c.getBoundingClientRect();
    const style = getComputedStyle(c);
    controls.push({
      role: r ? r.role : '',
      name: r ? r.name : (c.getAttribute('name') || ''),
      control: kind,
      value,
      hidden: type === 'hidden' || style.visibility === 'hidden' || style.display === 'none'
        || (box.width === 0 && box.height === 0),
      readOnly: !!c.readOnly,
      disabled: c.matches(':disabled'),
      prefilled,
    });
  }
  return JSON.stringify({ found: 1, inForm: true, controls });
}"#;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Answer {
    found: usize,
    #[serde(default)]
    in_form: bool,
    #[serde(default)]
    controls: Vec<AnswerControl>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnswerControl {
    role: String,
    name: String,
    control: ControlKind,
    value: String,
    hidden: bool,
    read_only: bool,
    disabled: bool,
    prefilled: bool,
}

/// The scope from the world's answer to [`scope_call`].
pub fn parse(raw: &str) -> Result<Result<Scope, ScopeMiss>, String> {
    let answer: Answer = serde_json::from_str(raw)
        .map_err(|e| format!("the page answered with an unreadable scope: {e}"))?;
    match answer.found {
        0 => return Ok(Err(ScopeMiss::NotFound)),
        1 => {}
        _ => return Ok(Err(ScopeMiss::Ambiguous)),
    }
    let controls = answer
        .controls
        .into_iter()
        .map(|c| ScopeControl {
            target: Target::new(c.role, c.name),
            control: c.control,
            value: c.value,
            hidden: c.hidden,
            read_only: c.read_only,
            disabled: c.disabled,
            prefilled: c.prefilled,
        })
        .collect();
    Ok(Ok(Scope {
        in_form: answer.in_form,
        controls,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_call_quotes_the_target_and_checks_the_engine() {
        let call = scope_call("1.2.3", &Target::new("button", "Send \"now\"');x("));
        assert!(call.starts_with("typeof globalThis.__zwiimAgent === 'undefined' ? \"absent\""));
        assert!(
            call.ends_with(r#"(globalThis.__zwiimAgent, "1.2.3", "button", "Send \"now\"');x(")"#)
        );
    }

    #[test]
    fn zero_or_many_matches_read_no_scope() {
        assert_eq!(parse(r#"{"found":0}"#).unwrap(), Err(ScopeMiss::NotFound));
        assert_eq!(parse(r#"{"found":2}"#).unwrap(), Err(ScopeMiss::Ambiguous));
        assert!(parse("absent").is_err());
    }

    #[test]
    fn a_lone_button_has_an_empty_scope() {
        let scope = parse(r#"{"found":1,"inForm":false,"controls":[]}"#)
            .unwrap()
            .unwrap();
        assert!(!scope.in_form);
        assert!(scope.controls.is_empty());
    }

    #[test]
    fn a_forms_controls_become_scope_controls() {
        let raw = r#"{"found":1,"inForm":true,"controls":[
          {"role":"textbox","name":"Name","control":"text","value":"Asha",
           "hidden":false,"readOnly":false,"disabled":false,"prefilled":false},
          {"role":"combobox","name":"Notice","control":"select","value":"30 days",
           "hidden":false,"readOnly":false,"disabled":false,"prefilled":true},
          {"role":"","name":"csrf","control":"text","value":"tok",
           "hidden":true,"readOnly":false,"disabled":false,"prefilled":true}]}"#;
        let scope = parse(raw).unwrap().unwrap();
        assert!(scope.in_form);
        assert_eq!(scope.controls.len(), 3);
        assert_eq!(scope.controls[0].target, Target::new("textbox", "Name"));
        assert_eq!(scope.controls[1].control, ControlKind::Select);
        assert!(scope.controls[1].prefilled);
        assert!(scope.controls[2].hidden);
    }
}
