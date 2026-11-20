// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: the engine-session lockdown (automation-engine spec 10.5 to
//! 10.9).
//!
//! Engine sessions are hidden, isolated, and given exactly the tools their
//! kind needs. This module owns the *policy*: which session kinds exist, the
//! tool names each kind may call, the refusal message for everything else,
//! the auto-reject rule for stray permission requests, and the "fully
//! locked" / "best effort" label per agent. The launch wiring in `acp/`
//! consumes these (spec 22.1 leaves the per-agent isolation flags to the
//! builder).

use serde::{Deserialize, Serialize};

/// The engine session kinds (spec 5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    /// Works one item: lane-bound browser tools + `playbook_*`.
    Item,
    /// Reads one source's list pages: lane-bound browser tools (own tab only)
    /// + `playbook_*`.
    Discovery,
    /// Decides rules over ~10 candidates from extracted text: `playbook_*`
    /// only, no browser.
    JudgmentBatch,
    /// Writes drafts before browser work (1b): `playbook_*` only.
    DraftBatch,
}

impl SessionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionKind::Item => "item",
            SessionKind::Discovery => "discovery",
            SessionKind::JudgmentBatch => "judgment_batch",
            SessionKind::DraftBatch => "draft_batch",
        }
    }

    pub fn all() -> &'static [SessionKind] {
        &[
            SessionKind::Item,
            SessionKind::Discovery,
            SessionKind::JudgmentBatch,
            SessionKind::DraftBatch,
        ]
    }

    /// The engine tools every kind may call (spec 7.1.2, 18.3).
    pub fn engine_tools(self) -> &'static [&'static str] {
        match self {
            SessionKind::Item => &[
                "playbook_declare_action",
                "playbook_attach_file",
                "playbook_ask_person",
                "playbook_item_result",
            ],
            SessionKind::Discovery => &["playbook_item_result"],
            SessionKind::JudgmentBatch | SessionKind::DraftBatch => &["playbook_item_result"],
        }
    }

    /// The browser tool names a kind may call; empty for the batch kinds
    /// (spec 10.8.1).
    pub fn browser_tools(self) -> &'static [&'static str] {
        match self {
            SessionKind::Item => &[
                "browser_snapshot",
                "browser_click",
                "browser_type",
                "browser_select_option",
                "browser_press_key",
                "browser_wait_for",
            ],
            // Discovery reads list pages only, in its own tab.
            SessionKind::Discovery => &["browser_snapshot", "browser_navigate", "browser_wait_for"],
            SessionKind::JudgmentBatch | SessionKind::DraftBatch => &[],
        }
    }

    /// Whether the kind gets a browser context at all (spec 10.8.1).
    pub fn has_browser(self) -> bool {
        matches!(self, SessionKind::Item | SessionKind::Discovery)
    }

    /// The full allow-list: engine tools plus the kind's browser tools.
    /// Everything not here is refused (spec 10.7.1, 10.8.2).
    pub fn allowed_tools(self) -> Vec<&'static str> {
        let mut v = self.engine_tools().to_vec();
        v.extend_from_slice(self.browser_tools());
        v
    }

    /// Whether a tool name is allowed for this kind.
    pub fn allows(self, tool: &str) -> bool {
        self.allowed_tools().contains(&tool)
    }
}

/// What the Host knows about a connection it launched for the engine. Set at
/// launch from the Host's own state, so nothing the agent writes (title,
/// kind) can make a session an engine session or stop it being one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineLaunch {
    pub kind: SessionKind,
}

/// Tools that are never allowed to any engine session, whatever its kind
/// (spec 10.8.2): Git, session tools, authoring, `ask_user_question`,
/// `browser_eval`, console or network-log bodies.
pub const NEVER_ALLOWED: &[&str] = &[
    "ask_user_question",
    "browser_eval",
    "browser_console_messages",
    "browser_network_log",
    "browser_list_tabs",
    "create_automation",
    "create_work_task",
    "get_session_info",
    "delegate_to_agent",
    "get_delegation_status",
    "cancel_delegation",
    "resume_delegation",
    "git_status",
    "git_log",
    "git_diff",
    "git_commit",
    "git_pr",
    "git_pr_merge",
];

/// The refusal an engine agent gets for a tool outside its profile
/// (spec 10.7.1): one line, the code first, then the next step.
pub const LOCKDOWN_REFUSAL: &str =
    "Refused: not available in automation sessions; use the browser tools.";

/// The isolation flags a launch applies for a given agent (spec 10.6): no
/// user MCP servers, no user hooks, no user settings, a separate config
/// directory, and a scrubbed environment. These are the *names* of the
/// settings, so the launch code and the conformance test agree on them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IsolationPlan {
    /// Where the agent's config directory points for this launch.
    pub config_dir: String,
    /// No user hooks are loaded.
    pub no_user_hooks: bool,
    /// No user MCP servers beyond zwiim-mcp.
    pub no_user_mcp: bool,
    /// No user settings files.
    pub no_user_settings: bool,
    /// Environment variable names removed from the child.
    pub scrub_env: Vec<String>,
}

impl IsolationPlan {
    /// A fully isolated plan for an engine launch in `config_dir`. The
    /// environment scrub list is the tokens an agent could otherwise
    /// exfiltrate through its own network tools (spec 10.6.1).
    pub fn strict(config_dir: impl Into<String>) -> Self {
        Self {
            config_dir: config_dir.into(),
            no_user_hooks: true,
            no_user_mcp: true,
            no_user_settings: true,
            scrub_env: vec![
                "ANTHROPIC_API_KEY".into(),
                "OPENAI_API_KEY".into(),
                "GITHUB_TOKEN".into(),
                "GH_TOKEN".into(),
                "ZWIIM_HOST_TOKEN".into(),
            ],
        }
    }
}

/// Whether an agent can be isolated and locked fully, or only best effort
/// (spec 10.7.2). The label shows in the agent picker and the playbook page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockdownLevel {
    /// Fully locked: fs and terminal delegated to Zwiim (or absent), strict
    /// permission mode, own network tools disableable, isolatable.
    FullyLocked,
    /// Everything else; the Host's outward guards still apply.
    BestEffort,
}

impl LockdownLevel {
    pub fn as_label(self) -> &'static str {
        match self {
            LockdownLevel::FullyLocked => "fully locked",
            LockdownLevel::BestEffort => "best effort",
        }
    }
}

/// What a launch declares about its agent to be labelled fully locked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentCapabilities {
    /// The agent delegates `fs/*` and `terminal/*` to Zwiim, or has none.
    pub delegates_fs_and_terminal: bool,
    /// The agent honours its strictest permission mode.
    pub strict_permission_mode: bool,
    /// The agent's own network tools can be disabled for this launch.
    pub network_tools_disableable: bool,
    /// The agent can run with a separate config directory and scrub.
    pub isolatable: bool,
}

impl AgentCapabilities {
    /// Fully locked only when every condition holds (spec 10.7.2).
    pub fn level(self) -> LockdownLevel {
        if self.delegates_fs_and_terminal
            && self.strict_permission_mode
            && self.network_tools_disableable
            && self.isolatable
        {
            LockdownLevel::FullyLocked
        } else {
            LockdownLevel::BestEffort
        }
    }
}

/// A zwiim-mcp tool group the engine launch enables (spec 10.8.3): groups
/// are built per session kind, not from global config. Engine sessions use
/// only the browser group (item / discovery) and their engine tools are the
/// `playbook_*` calls the Host serves, not companion groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProfile {
    pub kind: SessionKind,
    /// Tool names the companion may expose, exactly `kind.allowed_tools()`.
    pub tools: Vec<&'static str>,
    /// Whether this launch opens a browser context.
    pub has_browser: bool,
}

impl SessionProfile {
    pub fn for_kind(kind: SessionKind) -> Self {
        Self {
            kind,
            tools: kind.allowed_tools(),
            has_browser: kind.has_browser(),
        }
    }

    /// Whether the launch may open a browser context. Batch kinds may not.
    pub fn opens_browser(&self) -> bool {
        self.has_browser
    }
}

/// Build the `CompanionFeatures` an engine launch passes for a session kind
/// (spec 10.8.3): groups per kind, never from global config. Item and
/// discovery turn on `browser` (and never `browser_eval`); every other group
/// and the Git tools stay off.
pub fn companion_features(
    kind: SessionKind,
) -> crate::acp::delegation::companion::CompanionFeatures {
    let browser = kind.has_browser();
    crate::acp::delegation::companion::CompanionFeatures {
        delegation: false,
        feedback: false,
        ask: false,
        sessions: false,
        tasks: false,
        automations: false,
        taskboard: false,
        browser,
        // `browser_eval` is never on in an engine session (10.8.2).
        browser_eval: false,
        git: false,
        progress: false,
    }
}

/// Whether a stray ACP permission request from a hidden engine session must
/// be auto-rejected (spec 10.5.2). The Host never allows a request based on
/// fields the agent writes; a request from the hidden folder is always
/// rejected. `is_engine_folder` is what the Host knows about the session's
/// conversation folder.
pub fn auto_reject(is_engine_folder: bool) -> bool {
    is_engine_folder
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_gets_exactly_its_profile_tools() {
        for &kind in SessionKind::all() {
            let profile = SessionProfile::for_kind(kind);
            assert_eq!(profile.tools, kind.allowed_tools());
            for tool in kind.allowed_tools() {
                assert!(kind.allows(tool), "{kind:?} should allow {tool}");
            }
        }
    }

    #[test]
    fn batch_kinds_have_no_browser_and_item_and_discovery_do() {
        assert!(SessionKind::Item.has_browser());
        assert!(SessionKind::Discovery.has_browser());
        assert!(!SessionKind::JudgmentBatch.has_browser());
        assert!(!SessionKind::DraftBatch.has_browser());
        assert!(SessionKind::JudgmentBatch.browser_tools().is_empty());
        assert!(!SessionProfile::for_kind(SessionKind::DraftBatch).opens_browser());
    }

    #[test]
    fn item_has_the_four_engine_tools_and_no_authoring_or_eval() {
        let tools = SessionKind::Item.allowed_tools();
        for needed in [
            "playbook_declare_action",
            "playbook_attach_file",
            "playbook_ask_person",
            "playbook_item_result",
        ] {
            assert!(tools.contains(&needed), "item needs {needed}");
            assert!(!NEVER_ALLOWED.contains(&needed));
        }
        for forbidden in NEVER_ALLOWED {
            assert!(!tools.contains(forbidden), "item must not have {forbidden}");
            assert!(!SessionKind::Item.allows(forbidden));
        }
    }

    #[test]
    fn no_kind_reaches_a_never_allowed_tool() {
        for &kind in SessionKind::all() {
            for forbidden in NEVER_ALLOWED {
                assert!(!kind.allows(forbidden), "{kind:?} leaked {forbidden}");
            }
        }
    }

    #[test]
    fn discovery_reads_lists_but_cannot_click_or_type() {
        assert!(SessionKind::Discovery.allows("browser_snapshot"));
        assert!(SessionKind::Discovery.allows("browser_navigate"));
        assert!(!SessionKind::Discovery.allows("browser_click"));
        assert!(!SessionKind::Discovery.allows("browser_type"));
        // Discovery has no outward tools at all.
        assert!(!SessionKind::Discovery.allows("playbook_declare_action"));
    }

    #[test]
    fn a_request_from_the_hidden_folder_is_always_rejected() {
        assert!(auto_reject(true));
        assert!(!auto_reject(false));
    }

    #[test]
    fn fully_locked_needs_every_condition() {
        let all = AgentCapabilities {
            delegates_fs_and_terminal: true,
            strict_permission_mode: true,
            network_tools_disableable: true,
            isolatable: true,
        };
        assert_eq!(all.level(), LockdownLevel::FullyLocked);
        for drop in 0..4 {
            let mut caps = all;
            match drop {
                0 => caps.delegates_fs_and_terminal = false,
                1 => caps.strict_permission_mode = false,
                2 => caps.network_tools_disableable = false,
                _ => caps.isolatable = false,
            }
            assert_eq!(caps.level(), LockdownLevel::BestEffort);
        }
        assert_eq!(LockdownLevel::FullyLocked.as_label(), "fully locked");
        assert_eq!(LockdownLevel::BestEffort.as_label(), "best effort");
    }

    #[test]
    fn companion_features_match_the_profile_per_kind() {
        // The launch feature set and the profile agree: a profile tool the
        // companion would not serve (browser tools) is exactly the browser
        // groups being on; no `browser_eval`, Git or authoring is ever on.
        for &kind in SessionKind::all() {
            let features = companion_features(kind);
            assert_eq!(features.browser, kind.has_browser());
            assert!(!features.browser_eval);
            assert!(!features.git);
            assert!(!features.automations);
            assert!(!features.delegation);
            assert!(!features.ask);
            // A browser tool is allowed by the companion only when the kind
            // has a browser.
            for tool in kind.browser_tools() {
                assert!(features.allows_tool(tool), "{kind:?} {tool}");
            }
            if !kind.has_browser() {
                assert!(!features.allows_tool("browser_snapshot"));
            }
            // Engine tools are served by the Host, never the companion.
            for tool in kind.engine_tools() {
                assert!(
                    !features.allows_tool(tool),
                    "{kind:?} leaked {tool} to the companion"
                );
            }
        }
    }

    #[test]
    fn a_strict_isolation_plan_scrubs_tokens_and_user_config() {
        let plan = IsolationPlan::strict("/tmp/engine-config");
        assert!(plan.no_user_hooks && plan.no_user_mcp && plan.no_user_settings);
        assert!(plan.scrub_env.contains(&"GITHUB_TOKEN".to_string()));
        assert_eq!(plan.config_dir, "/tmp/engine-config");
    }
}
