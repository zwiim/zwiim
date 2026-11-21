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
    /// The item attempt an item session works; `None` for the other kinds.
    pub attempt: Option<ItemAttempt>,
}

/// One attempt at one item (spec 6.6.7). A call carrying an attempt the item
/// has moved past is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemAttempt {
    pub item_id: i32,
    pub epoch: i32,
}

/// The refusal a call from an ended attempt gets (spec 6.6.7, 22.6).
pub const STALE_ATTEMPT_REFUSAL: &str =
    "Refused: stale_attempt; this attempt at the item has ended, so stop.";

/// Answers whether an item attempt is still the item's current one, from
/// the engine's own rows.
pub struct DbItemAttempts {
    db: sea_orm::DatabaseConnection,
}

impl DbItemAttempts {
    pub fn new(db: sea_orm::DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl crate::acp::delegation::listener::ItemAttempts for DbItemAttempts {
    async fn is_current(&self, attempt: ItemAttempt) -> bool {
        use crate::automation::entities::playbook_item;
        use sea_orm::EntityTrait;
        match playbook_item::Entity::find_by_id(attempt.item_id)
            .one(&self.db)
            .await
        {
            Ok(row) => row.is_some_and(|item| item.epoch == attempt.epoch),
            // A read that fails refuses: the call may be stale.
            Err(e) => {
                tracing::warn!("[lockdown] reading item {} failed: {e}", attempt.item_id);
                false
            }
        }
    }
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

/// The Claude Code SDK options an engine launch adds under
/// `_meta.claudeCode.options` (spec 10.6, 10.7): no user, project or local
/// settings (so no user hooks, permissions or memory files), no MCP server
/// but the ones Zwiim passes, no built-in tools (shell, web, files), no
/// bypass mode, and every zwiim-mcp tool of the kind (engine and browser)
/// pre-allowed: the Host rejects each permission request an engine session
/// raises, so a tool left to ask could never be called. claude-agent-acp
/// 0.99.0 spreads these over its own defaults; checked in a live run.
pub fn claude_code_options(kind: SessionKind) -> serde_json::Value {
    let allowed: Vec<String> = kind
        .allowed_tools()
        .iter()
        .map(|tool| format!("mcp__zwiim-mcp__{tool}"))
        .collect();
    serde_json::json!({
        "settingSources": [],
        "strictMcpConfig": true,
        "tools": [],
        "allowDangerouslySkipPermissions": false,
        "allowedTools": allowed,
    })
}

/// Tokens an engine session's agent never needs, removed from its
/// environment so its own tools cannot send them anywhere (spec 10.6.1).
const SCRUBBED_ENV: &[&str] = &[
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "ZWIIM_HOST_TOKEN",
    "NPM_TOKEN",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
];

/// The variables removed from an engine launch of `agent`: every scrubbed
/// token except the one the agent signs in with.
pub fn scrubbed_env(agent: crate::models::agent::AgentType) -> Vec<&'static str> {
    use crate::models::agent::AgentType;
    let own: &[&str] = match agent {
        AgentType::ClaudeCode => &["ANTHROPIC_API_KEY"],
        AgentType::Codex => &["OPENAI_API_KEY"],
        AgentType::Gemini => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        _ => &[],
    };
    SCRUBBED_ENV
        .iter()
        .copied()
        .filter(|key| !own.contains(key))
        .collect()
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

/// What Zwiim knows of an agent's lockdown (spec 10.7.2). Claude Code runs
/// with no built-in tools, no bypass mode and no network tools, but its
/// isolation is not yet checked against a live CLI (spec 22.1), so it stays
/// best effort. Every other agent is unchecked.
pub fn capabilities(agent: crate::models::agent::AgentType) -> AgentCapabilities {
    let claude = agent == crate::models::agent::AgentType::ClaudeCode;
    AgentCapabilities {
        delegates_fs_and_terminal: claude,
        strict_permission_mode: claude,
        network_tools_disableable: claude,
        isolatable: false,
    }
}

/// The label for an agent in the picker and on the playbook page.
pub fn lockdown_level(agent: crate::models::agent::AgentType) -> LockdownLevel {
    capabilities(agent).level()
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

    #[tokio::test]
    async fn only_the_items_current_epoch_is_current() {
        use crate::acp::delegation::listener::ItemAttempts;
        use crate::automation::entities::playbook_item;
        use crate::automation::{AutomationEngine, PlaybookStore, RunTrigger};
        use sea_orm::{EntityTrait, Set};
        use sea_orm_migration::MigratorTrait;
        let db = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
        crate::db::migration::Migrator::up(&db, None).await.unwrap();
        let pb = PlaybookStore::new(db.clone())
            .create("Apply", "goal")
            .await
            .unwrap();
        let run = AutomationEngine::new(db.clone())
            .start_run(pb.id, pb.version, RunTrigger::Mac, None)
            .await
            .unwrap();
        let now = chrono::Utc::now();
        let item_id = playbook_item::Entity::insert(playbook_item::ActiveModel {
            public_id: Set(uuid::Uuid::new_v4().to_string()),
            run_id: Set(run),
            playbook_id: Set(pb.id),
            site: Set("jobsy.example".into()),
            account: Set("main".into()),
            item_key: Set("job-1".into()),
            status: Set("working".into()),
            epoch: Set(2),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        })
        .exec_with_returning(&db)
        .await
        .unwrap()
        .id;
        let attempts = DbItemAttempts::new(db);
        assert!(attempts.is_current(ItemAttempt { item_id, epoch: 2 }).await);
        assert!(!attempts.is_current(ItemAttempt { item_id, epoch: 1 }).await);
        assert!(
            !attempts
                .is_current(ItemAttempt {
                    item_id: item_id + 1,
                    epoch: 2
                })
                .await
        );
    }

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
    fn claude_code_runs_with_no_user_config_no_built_ins_and_its_tools_pre_allowed() {
        let options = claude_code_options(SessionKind::Item);
        assert_eq!(options["settingSources"], serde_json::json!([]));
        assert_eq!(options["strictMcpConfig"], true);
        assert_eq!(options["tools"], serde_json::json!([]));
        assert_eq!(options["allowDangerouslySkipPermissions"], false);
        // Every tool of the kind is pre-allowed, the engine tools too: the
        // Host rejects every permission request an engine session raises,
        // so a tool left to ask can never be called (live smoke run).
        for kind in SessionKind::all() {
            let allowed: Vec<String> = kind
                .allowed_tools()
                .iter()
                .map(|t| format!("mcp__zwiim-mcp__{t}"))
                .collect();
            let options = claude_code_options(*kind);
            assert_eq!(options["allowedTools"], serde_json::json!(allowed), "{kind:?}");
        }
        let item = options["allowedTools"].as_array().unwrap();
        for tool in ["playbook_item_result", "playbook_ask_person", "browser_snapshot"] {
            assert!(item.contains(&serde_json::json!(format!("mcp__zwiim-mcp__{tool}"))));
        }
        let batch = claude_code_options(SessionKind::JudgmentBatch);
        assert_eq!(
            batch["allowedTools"],
            serde_json::json!(["mcp__zwiim-mcp__playbook_item_result"])
        );
    }

    #[test]
    fn an_engine_agent_keeps_its_own_key_and_loses_every_other_token() {
        use crate::models::agent::AgentType;
        let claude = scrubbed_env(AgentType::ClaudeCode);
        for gone in [
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "ZWIIM_HOST_TOKEN",
            "OPENAI_API_KEY",
        ] {
            assert!(claude.contains(&gone), "{gone}");
        }
        assert!(!claude.contains(&"ANTHROPIC_API_KEY"));
        let codex = scrubbed_env(AgentType::Codex);
        assert!(codex.contains(&"ANTHROPIC_API_KEY"));
        assert!(!codex.contains(&"OPENAI_API_KEY"));
    }

    /// Spec 22.1: no agent is "fully locked" until its isolation has been
    /// checked against a live CLI.
    #[test]
    fn every_agent_is_best_effort_until_verified() {
        use crate::models::agent::AgentType;
        for agent in [AgentType::ClaudeCode, AgentType::Codex, AgentType::Gemini] {
            assert_eq!(lockdown_level(agent), LockdownLevel::BestEffort);
        }
    }
}
