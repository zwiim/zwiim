//! Zwiim Host settings: what the user chose once for every device, kept on the
//! Host so a phone and this computer start chats the same way.
//!
//! Every setting is declared once in [`KEYS`], with the kind of value it holds.
//! Values live in `app_metadata`, one row per key (`zwiim.setting:<key>`,
//! JSON). An unset key is absent, never stored as `null`.
//!
//! The defaults for new sessions are applied here as well: onto what
//! `agent.list` reports a new session can choose, and onto `session.start`
//! for whatever the client did not choose itself. A default the agent does
//! not offer (a model it dropped, plan on an agent without one) is ignored,
//! never an error.

use std::collections::BTreeMap;
use std::path::Path;

use sea_orm::DatabaseConnection;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::db::service::app_metadata_service;
use crate::models::agent::AgentType;
use crate::protocol::error::ZwiimError;
use crate::session_options::{ChoiceOption, Mode, OptionsRequest, SessionOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// An agent's wire name.
    Agent,
    /// `normal`, `plan` or `unattended`.
    Mode,
    /// An agent's own value for a select (a model, an effort).
    Text,
    /// `true` or `false`.
    Bool,
    /// Where new branch names' prefix comes from: `github` or `custom`.
    PrefixSource,
    /// Text a branch name can start with (see [`valid_prefix`]).
    Prefix,
    /// The user's own instructions to agents, up to [`MAX_PROMPT`] bytes.
    Prompt,
    /// How much terminal output to keep, in bytes (see [`SCROLLBACK_RANGE`]).
    Scrollback,
}

pub const DEFAULT_AGENT: &str = "defaults.agent";
pub const DEFAULT_MODE: &str = "defaults.mode";
pub const BRANCH_PREFIX: &str = "git.branch_prefix";
pub const BRANCH_PREFIX_CUSTOM: &str = "git.branch_prefix_custom";
pub const DELETE_BRANCH_ON_REMOVE: &str = "git.delete_branch_on_remove";
pub const PROMPT_PULL_REQUEST: &str = "prompts.pull_request";
pub const PROMPT_BRANCH_NAME: &str = "prompts.branch_name";
pub const TERMINAL_SCROLLBACK: &str = "terminal.scrollback";
pub const PUSH_SETS_UPSTREAM: &str = "git.push_sets_upstream";
pub const BROWSER_AGENT_PASSWORDS: &str = "browser.agent_passwords";
pub const BROWSER_AGENT_ASK_SITES: &str = "browser.agent_ask_sites";

/// Every setting the Host keeps.
const KEYS: &[(&str, Kind)] = &[
    (DEFAULT_AGENT, Kind::Agent),
    (DEFAULT_MODE, Kind::Mode),
    ("defaults.claude_code.model", Kind::Text),
    ("defaults.claude_code.effort", Kind::Text),
    ("defaults.codex.model", Kind::Text),
    ("defaults.codex.effort", Kind::Text),
    (BRANCH_PREFIX, Kind::PrefixSource),
    (BRANCH_PREFIX_CUSTOM, Kind::Prefix),
    (DELETE_BRANCH_ON_REMOVE, Kind::Bool),
    (PROMPT_PULL_REQUEST, Kind::Prompt),
    (PROMPT_BRANCH_NAME, Kind::Prompt),
    (TERMINAL_SCROLLBACK, Kind::Scrollback),
    (PUSH_SETS_UPSTREAM, Kind::Bool),
    (BROWSER_AGENT_PASSWORDS, Kind::Bool),
    (BROWSER_AGENT_ASK_SITES, Kind::Bool),
];

/// The terminal output a setting can keep, in bytes.
pub const SCROLLBACK_RANGE: std::ops::RangeInclusive<u64> = 64 * 1024..=16 * 1024 * 1024;

/// Longest text value accepted.
const MAX_TEXT: usize = 200;

/// Longest prompt accepted, in bytes.
pub const MAX_PROMPT: usize = 8000;

fn storage_key(key: &str) -> String {
    format!("zwiim.setting:{key}")
}

fn db_error(e: crate::db::error::DbError) -> ZwiimError {
    ZwiimError::from(crate::app_error::AppCommandError::from(e))
}

/// Every setting, `null` when unset.
pub async fn get(db: &DatabaseConnection) -> Result<Map<String, Value>, ZwiimError> {
    let mut out = Map::new();
    for (key, _) in KEYS {
        let raw = app_metadata_service::get_value(db, &storage_key(key))
            .await
            .map_err(db_error)?;
        let value = raw
            .and_then(|r| serde_json::from_str(&r).ok())
            .unwrap_or(Value::Null);
        out.insert(key.to_string(), value);
    }
    Ok(out)
}

/// Set each key in `values` to its value, or clear it for `null`. Every value
/// is checked before any is written.
pub async fn update(db: &DatabaseConnection, values: &Map<String, Value>) -> Result<(), ZwiimError> {
    for (key, value) in values {
        check(key, value)?;
    }
    for (key, value) in values {
        let stored = storage_key(key);
        if value.is_null() {
            app_metadata_service::delete_value(db, &stored).await.map_err(db_error)?;
        } else {
            app_metadata_service::upsert_value(db, &stored, &value.to_string())
                .await
                .map_err(db_error)?;
        }
    }
    if values.contains_key(TERMINAL_SCROLLBACK) {
        apply_terminal_scrollback(db).await;
    }
    Ok(())
}

fn check(key: &str, value: &Value) -> Result<(), ZwiimError> {
    let Some((_, kind)) = KEYS.iter().find(|(k, _)| *k == key) else {
        return Err(ZwiimError::invalid_input(format!("Unknown setting `{key}`")));
    };
    if value.is_null() {
        return Ok(());
    }
    let valid = match (kind, value) {
        (Kind::Bool, value) => value.is_boolean(),
        (Kind::Agent, Value::String(s)) => AgentType::from_wire(s).is_some(),
        (Kind::Mode, Value::String(s)) => mode_of(s).is_some(),
        (Kind::Text, Value::String(s)) => !s.trim().is_empty() && s.len() <= MAX_TEXT,
        (Kind::PrefixSource, Value::String(s)) => s == "github" || s == "custom",
        (Kind::Prefix, Value::String(s)) => valid_prefix(s),
        (Kind::Prompt, Value::String(s)) => !s.trim().is_empty() && s.len() <= MAX_PROMPT,
        (Kind::Scrollback, value) => value.as_u64().is_some_and(|n| SCROLLBACK_RANGE.contains(&n)),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(ZwiimError::invalid_input(format!("Not a valid value for `{key}`")))
    }
}

/// Longest branch prefix accepted.
const MAX_PREFIX: usize = 40;

/// Whether `prefix` (a trailing `/` aside) can start a branch name: slash
/// separated parts of letters, digits, `.`, `_` and `-`, none empty, none
/// starting with `.` or ending in `.lock`, and no `..` — what git accepts in
/// a ref, kept to characters that need no quoting anywhere.
pub fn valid_prefix(prefix: &str) -> bool {
    let prefix = prefix.strip_suffix('/').unwrap_or(prefix);
    !prefix.is_empty()
        && prefix.len() <= MAX_PREFIX
        && !prefix.contains("..")
        && prefix.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && !part.ends_with(".lock")
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
}

fn mode_of(s: &str) -> Option<Mode> {
    serde_json::from_value(Value::String(s.to_string())).ok()
}

async fn read_value(db: &DatabaseConnection, key: &str) -> Option<Value> {
    let raw = app_metadata_service::get_value(db, &storage_key(key)).await.ok()??;
    serde_json::from_str(&raw).ok()
}

async fn read_text(db: &DatabaseConnection, key: &str) -> Option<String> {
    match read_value(db, key).await? {
        Value::String(s) => Some(s),
        _ => None,
    }
}

async fn read_bool(db: &DatabaseConnection, key: &str) -> bool {
    matches!(read_value(db, key).await, Some(Value::Bool(true)))
}

/// What new branch names start with, before a `/` (`ashish1101` in
/// `ashish1101/pune`), if anything. `github` is the login `gh` was signed in
/// as, read without the network and lowercased; `custom` the user's text.
/// Either one git wouldn't take is none, so a branch is never refused for it.
pub async fn branch_prefix(
    db: &DatabaseConnection,
    gh: Option<&Path>,
    cwd: &str,
) -> Option<String> {
    let prefix = match read_text(db, BRANCH_PREFIX).await?.as_str() {
        "github" => crate::git_harness::github_login_local(gh, cwd)
            .await?
            .to_lowercase(),
        "custom" => read_text(db, BRANCH_PREFIX_CUSTOM).await?,
        _ => return None,
    };
    valid_prefix(&prefix).then(|| prefix.trim_end_matches('/').to_string())
}

/// Whether agents may type into password fields on any site, not only the
/// loopback dev servers (design 10.3). Off unless the person turns it on.
pub async fn browser_agent_passwords(db: &DatabaseConnection) -> bool {
    read_bool(db, BROWSER_AGENT_PASSWORDS).await
}

/// Whether a site an agent opens for the first time in a session waits for
/// the person to allow it (design 13.2). Off unless the person turns it on.
pub async fn browser_agent_ask_sites(db: &DatabaseConnection) -> bool {
    read_bool(db, BROWSER_AGENT_ASK_SITES).await
}

/// Whether removing a worktree also deletes its branch, when no work is lost.
pub async fn delete_branch_on_remove(db: &DatabaseConnection) -> bool {
    read_bool(db, DELETE_BRANCH_ON_REMOVE).await
}

/// Whether a plain `git push` in a worktree sets its branch's upstream. On
/// unless turned off: a worktree's branch is made from a commit, so it has
/// none, and a push without one fails.
pub async fn push_sets_upstream(db: &DatabaseConnection) -> bool {
    !matches!(read_value(db, PUSH_SETS_UPSTREAM).await, Some(Value::Bool(false)))
}

/// How much output each terminal keeps, in bytes, when the user chose.
async fn terminal_scrollback(db: &DatabaseConnection) -> Option<usize> {
    let n = read_value(db, TERMINAL_SCROLLBACK).await?.as_u64()?;
    SCROLLBACK_RANGE.contains(&n).then_some(n as usize)
}

/// Make the terminals keep what the user chose. Run once the database opens;
/// [`update`] keeps it current after that.
pub async fn apply_terminal_scrollback(db: &DatabaseConnection) {
    crate::terminal::manager::set_scrollback_limit(terminal_scrollback(db).await);
}

/// A prompt the user set, trimmed.
async fn read_prompt(db: &DatabaseConnection, key: &str) -> Option<String> {
    let text = read_text(db, key).await?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// What the user asked of every agent's session instructions.
pub async fn user_preferences(db: &DatabaseConnection) -> crate::harness::UserPreferences {
    crate::harness::UserPreferences {
        pull_request: read_prompt(db, PROMPT_PULL_REQUEST).await,
        branch_name: read_prompt(db, PROMPT_BRANCH_NAME).await,
    }
}

/// The agent new sessions start with, when one is chosen.
pub async fn default_agent(db: &DatabaseConnection) -> Option<AgentType> {
    AgentType::from_wire(&read_text(db, DEFAULT_AGENT).await?)
}

/// What a new session of an agent starts with unless a client chooses
/// otherwise. Absent fields keep the agent's own default.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AgentDefaults {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<Mode>,
}

/// The defaults chosen for `agent` (its wire name), whether it offers them or not.
pub async fn agent_defaults(db: &DatabaseConnection, agent: &str) -> AgentDefaults {
    let per_agent = |field: &str| format!("defaults.{agent}.{field}");
    let declared = |key: &str| KEYS.iter().any(|(k, _)| *k == key);
    let (model_key, effort_key) = (per_agent("model"), per_agent("effort"));
    AgentDefaults {
        model: match declared(&model_key) {
            true => read_text(db, &model_key).await,
            false => None,
        },
        effort: match declared(&effort_key) {
            true => read_text(db, &effort_key).await,
            false => None,
        },
        mode: read_text(db, DEFAULT_MODE).await.as_deref().and_then(mode_of),
    }
}

/// The effort a new session can choose on `model`: what that model offered
/// here, else (a model not run here yet) the agent's catalog effort.
fn effort_on<'a>(
    options: &'a SessionOptions,
    efforts: &'a BTreeMap<String, Option<ChoiceOption>>,
    model: Option<&str>,
) -> Option<&'a ChoiceOption> {
    match model.and_then(|m| efforts.get(m)) {
        Some(known) => known.as_ref(),
        None => options.effort.as_ref(),
    }
}

fn offers(choice: Option<&ChoiceOption>, value: &str) -> bool {
    choice.is_some_and(|c| c.values.iter().any(|v| v.value == value))
}

/// The part of `chosen` that a new session of an agent offering `options`
/// (and `efforts` per model) can start with. The effort counts as offered when
/// the model the session would start on offers it.
pub fn offered(
    chosen: &AgentDefaults,
    options: &SessionOptions,
    efforts: &BTreeMap<String, Option<ChoiceOption>>,
) -> AgentDefaults {
    let model = chosen
        .model
        .clone()
        .filter(|m| offers(options.model.as_ref(), m));
    let start_model = model
        .as_deref()
        .or_else(|| options.model.as_ref().map(|m| m.current.as_str()));
    let effort = chosen
        .effort
        .clone()
        .filter(|e| offers(effort_on(options, efforts, start_model), e));
    let mode = chosen
        .mode
        .filter(|m| options.mode.as_ref().is_some_and(|o| o.available.contains(m)));
    AgentDefaults { model, effort, mode }
}

/// `options` and `efforts` as a new session starts with `defaults` (already
/// [`offered`]): the default model current, its own effort, the default effort
/// current wherever it is offered, and the default mode.
pub fn apply(
    options: &mut SessionOptions,
    efforts: &mut BTreeMap<String, Option<ChoiceOption>>,
    defaults: &AgentDefaults,
) {
    if let (Some(model), Some(choice)) = (&defaults.model, options.model.as_mut()) {
        choice.current = model.clone();
        if let Some(known) = efforts.get(model) {
            options.effort = known.clone();
        }
    }
    if let Some(effort) = &defaults.effort {
        let choices = options.effort.iter_mut().chain(efforts.values_mut().flatten());
        for choice in choices {
            if choice.values.iter().any(|v| &v.value == effort) {
                choice.current = effort.clone();
            }
        }
    }
    if let (Some(mode), Some(option)) = (defaults.mode, options.mode.as_mut()) {
        option.current = mode;
    }
}

/// `request` with what it leaves out taken from `defaults` (already
/// [`offered`]). An effort is only filled in when the model the session will
/// start on offers it.
pub fn fill_start(
    request: &OptionsRequest,
    defaults: &AgentDefaults,
    options: &SessionOptions,
    efforts: &BTreeMap<String, Option<ChoiceOption>>,
) -> OptionsRequest {
    let model = request.model.clone().or_else(|| defaults.model.clone());
    let start_model = model
        .as_deref()
        .or_else(|| options.model.as_ref().map(|m| m.current.as_str()));
    let effort = request.effort.clone().or_else(|| {
        defaults
            .effort
            .clone()
            .filter(|e| offers(effort_on(options, efforts, start_model), e))
    });
    OptionsRequest {
        model,
        effort,
        mode: request.mode.or(defaults.mode),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_options::{ModeOption, OptionValue};

    fn choice(current: &str, values: &[&str]) -> ChoiceOption {
        ChoiceOption {
            current: current.to_string(),
            values: values
                .iter()
                .map(|v| OptionValue {
                    value: v.to_string(),
                    label: v.to_string(),
                    description: None,
                })
                .collect(),
        }
    }

    fn claude() -> (SessionOptions, BTreeMap<String, Option<ChoiceOption>>) {
        let options = SessionOptions {
            model: Some(choice("opus", &["opus", "haiku"])),
            effort: Some(choice("high", &["low", "high"])),
            mode: Some(ModeOption {
                current: Mode::Normal,
                label: String::new(),
                available: vec![Mode::Normal, Mode::Plan],
            }),
            can_rewind_chat: None,
        };
        let efforts = BTreeMap::from([
            ("opus".to_string(), Some(choice("high", &["low", "high"]))),
            ("haiku".to_string(), None),
        ]);
        (options, efforts)
    }

    fn chosen(model: Option<&str>, effort: Option<&str>, mode: Option<Mode>) -> AgentDefaults {
        AgentDefaults {
            model: model.map(str::to_string),
            effort: effort.map(str::to_string),
            mode,
        }
    }

    #[test]
    fn check_rejects_unknown_keys_and_wrong_kinds() {
        assert!(check("defaults.agent", &Value::from("codex")).is_ok());
        assert!(check("defaults.agent", &Value::from("nobody")).is_err());
        assert!(check("defaults.mode", &Value::from("plan")).is_ok());
        assert!(check("defaults.mode", &Value::from("bypassPermissions")).is_err());
        assert!(check("defaults.codex.model", &Value::from(" ")).is_err());
        assert!(check("defaults.codex.model", &Value::from(1)).is_err());
        assert!(check("defaults.codex.model", &Value::Null).is_ok());
        assert!(check("defaults.gemini.model", &Value::from("x")).is_err());
        assert!(check("git.branch_prefix", &Value::from("github")).is_ok());
        assert!(check("git.branch_prefix", &Value::from("none")).is_err());
        assert!(check("git.branch_prefix_custom", &Value::from("team/ashish")).is_ok());
        assert!(check("git.delete_branch_on_remove", &Value::from(true)).is_ok());
        assert!(check("git.delete_branch_on_remove", &Value::from("true")).is_err());
        assert!(check("defaults.mode", &Value::from(true)).is_err());
        assert!(check("prompts.pull_request", &Value::from("Draft PRs only.\nKeep it short.")).is_ok());
        assert!(check("prompts.pull_request", &Value::from(" \n ")).is_err());
        assert!(check("prompts.branch_name", &Value::from("x".repeat(MAX_PROMPT))).is_ok());
        assert!(check("prompts.branch_name", &Value::from("x".repeat(MAX_PROMPT + 1))).is_err());
        assert!(check("prompts.branch_name", &Value::from(false)).is_err());
        // General and conflict instructions are `host.instructions`.
        assert!(check("prompts.general", &Value::from("x")).is_err());
        assert!(check("prompts.code_review", &Value::from("x")).is_err());
        assert!(check("terminal.scrollback", &Value::from(1024 * 1024)).is_ok());
        assert!(check("terminal.scrollback", &Value::from(1024)).is_err());
        assert!(check("terminal.scrollback", &Value::from(64 * 1024 * 1024)).is_err());
        assert!(check("terminal.scrollback", &Value::from(1.5e6)).is_err());
        assert!(check("terminal.scrollback", &Value::from("1MB")).is_err());
        assert!(check("git.push_sets_upstream", &Value::from(false)).is_ok());
    }

    #[test]
    fn a_prefix_is_what_git_takes_without_quoting() {
        for ok in ["ashish", "ashish/", "team/ashish", "a.b_c-d", "x1"] {
            assert!(valid_prefix(ok), "{ok}");
        }
        for bad in [
            "",
            "/",
            "/a",
            "a//b",
            "a..b",
            ".a",
            "a/.b",
            "a.lock",
            "a b",
            "a~b",
            "a:b",
            "a^b",
            "ä",
            &"a".repeat(41),
        ] {
            assert!(!valid_prefix(bad), "{bad}");
        }
    }

    #[test]
    fn offered_drops_what_the_agent_does_not_offer() {
        let (options, efforts) = claude();
        let got = offered(&chosen(Some("sonnet"), Some("high"), Some(Mode::Unattended)), &options, &efforts);
        assert_eq!(got, chosen(None, Some("high"), None));
        // Haiku offers no effort, so a default effort does not apply on it.
        let got = offered(&chosen(Some("haiku"), Some("low"), Some(Mode::Plan)), &options, &efforts);
        assert_eq!(got, chosen(Some("haiku"), None, Some(Mode::Plan)));
    }

    #[test]
    fn apply_makes_the_defaults_current() {
        let (mut options, mut efforts) = claude();
        apply(&mut options, &mut efforts, &chosen(Some("haiku"), None, Some(Mode::Plan)));
        assert_eq!(options.model.as_ref().unwrap().current, "haiku");
        assert_eq!(options.effort, None, "the default model's own effort");
        assert_eq!(options.mode.as_ref().unwrap().current, Mode::Plan);

        let (mut options, mut efforts) = claude();
        apply(&mut options, &mut efforts, &chosen(None, Some("low"), None));
        assert_eq!(options.effort.as_ref().unwrap().current, "low");
        assert_eq!(efforts["opus"].as_ref().unwrap().current, "low");
    }

    #[test]
    fn fill_start_keeps_what_the_client_chose() {
        let (options, efforts) = claude();
        let defaults = chosen(Some("opus"), Some("low"), Some(Mode::Plan));
        let request = OptionsRequest {
            model: None,
            effort: None,
            mode: Some(Mode::Normal),
        };
        let got = fill_start(&request, &defaults, &options, &efforts);
        assert_eq!(got.model.as_deref(), Some("opus"));
        assert_eq!(got.effort.as_deref(), Some("low"));
        assert_eq!(got.mode, Some(Mode::Normal));

        // The client picked Haiku, which offers no effort: none is filled in.
        let request = OptionsRequest {
            model: Some("haiku".to_string()),
            ..Default::default()
        };
        let got = fill_start(&request, &defaults, &options, &efforts);
        assert_eq!(got.effort, None);
        assert_eq!(got.mode, Some(Mode::Plan));
    }
}
