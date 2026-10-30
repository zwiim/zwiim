//! Grok plan-mode approval ("exit plan mode") domain types.
//!
//! When a Grok session is in **plan mode** and the agent finishes planning, it
//! calls its native `exit_plan_mode` tool. That tool reads `plan.md` from disk
//! and issues a BLOCKING ACP ext request (`_x.ai/exit_plan_mode`,
//! `ExitPlanModeExtRequest { sessionId, toolCallId, planContent }`) to the client
//! to get the user's approval before leaving plan mode. Grok waits for the reply.
//!
//! Zwiim bridges that ext request into an interactive **plan-approval card**
//! rendered above the composer — the SAME shape as the `ask_user_question` bridge
//! ([`crate::acp::question`]) and the permission dialog: a pending request is
//! captured onto [`crate::acp::session_state::SessionState`] (in-memory, turn
//! scoped, carried on `to_snapshot()` so a mid-turn attach re-renders the card),
//! and the blocked ext request is answered once the user picks an action.
//!
//! Three outcomes exist (per Grok's binary + `~/.grok/docs/user-guide/19-plan-mode.md`):
//!   * **Approve** — leave plan mode and start implementing.
//!   * **Request changes** — freeform revision notes; the agent revises and plan
//!     mode stays active (the user can iterate).
//!   * **Abandon** — abandon the plan and turn plan mode off.
//!
//! ## Wire-format (confirmed against Grok 0.2.111, live ACP capture)
//!
//! Request `ExitPlanModeExtRequest = { sessionId, toolCallId, planContent }`.
//! Note `planContent` is often `null` — Grok does NOT embed the plan body in the
//! request; after approval it reads `plan.md` itself, so the card's plan preview
//! can be empty here.
//!
//! Reply `ExitPlanModeExtResponse = { "outcome": <string>, "feedback": <string> }`
//! (the field is `outcome`, NOT `decision`). Only two outcome values are
//! recognized; every other value (and a missing field) falls through Grok's
//! `#[serde(other)]` catch-all to keep-planning, and the reply `feedback` is then
//! discarded:
//!   * `"approved"`  — leave plan mode and start implementing. A non-empty
//!     `feedback` becomes "approve with review comments".
//!   * `"abandoned"` — abandon the plan and turn plan mode off.
//!   * anything else (we send `"keep_planning"`) — stay in plan mode. Grok drops
//!     the reply `feedback`, so `session::respond` delivers the user's revision
//!     notes as the next user message once the turn ends (mirroring Grok's own
//!     TUI, where "request changes" moves focus to the prompt).
//!
//! ## Claude
//!
//! Claude's `ExitPlanMode` arrives as an ordinary ACP permission request (kind
//! `switch_mode`) whose options are the adapter's own `exit-plan-*` / `reject`
//! ids. The Host bridges it into this same card ([`parse_claude_exit_plan_request`],
//! [`claude_exit_plan_option`]) so the adapter's ids never reach a client and
//! its clear-context variants, which restart Claude under a session Zwiim never
//! learns about, are never offered.
//!
//! ## Codex
//!
//! Codex's plan review (`_meta.codex.kind = "plan_review"`) is bridged the same
//! way ([`parse_codex_plan_review_request`], [`codex_plan_option`]), so every
//! agent shows one plan card. Approve maps to `implement_plan` (codex leaves
//! plan mode and implements in the same prompt); everything else maps to
//! `revise_plan`, which ends the turn in plan mode.
//!
//! An earlier inferred shape (`{ decision: "approve"/... }`) was wrong: Grok
//! silently defaulted it to keep-planning, so "Approve" read as "request changes".

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sacp::schema::RequestPermissionRequest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::oneshot;

/// Per-field sanity bound (characters) for the plan markdown carried in the
/// request event + snapshot. The plan rides the broadcast `PlanApprovalRequest`
/// and every snapshot, so this caps the blast radius of a pathological payload.
/// Generous — a real `plan.md` is far smaller.
pub const MAX_PLAN_MARKDOWN_CHARS: usize = 262_144;
/// Per-field sanity bound (characters) for the user's freeform "request changes"
/// feedback, echoed back to Grok. Generous enough for real revision notes.
pub const MAX_FEEDBACK_CHARS: usize = 16_384;

/// The pending (awaiting-decision) plan approval stored on
/// `SessionState.pending_plan_approval` and carried on `to_snapshot()` so a
/// client attaching mid-turn (cold attach, reconnect, another window) re-renders
/// the approval card even though the one-shot `PlanApprovalRequest` event won't
/// replay for it. At most one is pending per connection (the agent is blocked in
/// its `exit_plan_mode` tool call).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingPlanApprovalState {
    /// Backend-minted correlation key for the answer (NOT Grok's toolCallId).
    pub approval_id: String,
    /// Grok's `toolCallId` for the `exit_plan_mode` call, so the synthesized
    /// in-stream result card can key on the same id as the (suppressed) native
    /// tool call. Empty when the request omitted it.
    pub tool_call_id: String,
    /// The plan markdown read from `plan.md` (Grok's `planContent`). May be empty
    /// — an empty/missing plan still opens the approval surface with an
    /// empty-state notice (per Grok's plan-mode doc).
    pub plan_markdown: String,
    pub created_at: DateTime<Utc>,
}

/// Which action the user took on the plan-approval card. `snake_case` on the
/// wire (`approve` / `request_changes` / `abandon`) — constructed by the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanApprovalDecision {
    /// Approve the plan → Grok leaves plan mode and starts implementing.
    Approve,
    /// Request changes → Grok revises the plan; plan mode stays active.
    RequestChanges,
    /// Abandon the plan → plan mode is turned off.
    Abandon,
    /// Hand the plan off to a new session (`session::respond` starts it); the
    /// agent is told the same as for `Abandon`.
    HandOff,
}

/// The user's submission for a pending plan approval (frontend → backend → the
/// blocked ext request). `feedback` carries the freeform revision notes for
/// `RequestChanges` (ignored / empty for the other decisions). camelCase on the
/// wire — this is constructed by the frontend, not read from an event payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanApprovalAnswer {
    pub decision: PlanApprovalDecision,
    #[serde(default)]
    pub feedback: Option<String>,
}

impl PlanApprovalAnswer {
    /// The trimmed, length-bounded feedback text (empty when none / whitespace).
    /// Applied on the answer side so a hand-rolled client hitting the plain
    /// `acp_answer_plan_approval` endpoint directly can't ride an unbounded blob
    /// back to Grok.
    pub fn normalized_feedback(&self) -> String {
        self.feedback
            .as_deref()
            .unwrap_or("")
            .trim()
            .chars()
            .take(MAX_FEEDBACK_CHARS)
            .collect()
    }
}

/// What [`SessionPlanApprovalAccess::register_plan_approval`] hands back to the
/// connection's `exit_plan_mode` ext handler: the new approval id plus the
/// receiver to await the user's decision on. Mirrors
/// [`crate::acp::question::RegisteredQuestion`].
pub struct RegisteredPlanApproval {
    pub approval_id: String,
    pub answer_rx: oneshot::Receiver<PlanApprovalAnswer>,
}

/// Connection-facing access to register / cancel a pending plan approval on a
/// parent connection. The production impl
/// (`crate::acp::manager::ConnectionManagerPlanApprovalLookup`) wraps the
/// `ConnectionManager`; tests use an in-memory stub. Mirrors
/// [`crate::acp::question::SessionQuestionAccess`] — kept in this neutral module
/// so `connection.rs` depends only on the trait, not on `manager.rs`.
#[async_trait]
pub trait SessionPlanApprovalAccess: Send + Sync {
    /// Register a pending plan approval on the parent connection (resolved from
    /// the connection id), broadcast `PlanApprovalRequest` to every attached
    /// client, and return a receiver that resolves when the user picks an action
    /// (or the approval is canceled). `None` when the connection is gone or one
    /// is already pending on it — the ext handler then declines to Grok, which
    /// keeps plan mode active.
    async fn register_plan_approval(
        &self,
        parent_connection_id: &str,
        tool_call_id: String,
        plan_markdown: String,
    ) -> Option<RegisteredPlanApproval>;

    /// Cancel every pending plan approval parked on a connection that is tearing
    /// down. Called from the `run_connection` cleanup guard so a pending approval
    /// — and the ext handler task parked on it — is reclaimed synchronously on
    /// disconnect. Dropping the sender resolves the handler's await as a
    /// disconnect (Grok keeps plan mode active, re-surfaces on reconnect).
    async fn cancel_plan_approvals_by_parent(&self, parent_connection_id: &str);
}

/// Parse Grok's `_x.ai/exit_plan_mode` ext-request params into the plan markdown
/// and `toolCallId` Zwiim needs to render the approval card. Grok's wire shape is
/// `{ sessionId, toolCallId, planContent }`. Lenient: an empty / missing
/// `planContent` is valid (the empty-plan approval surface), so the only hard
/// error is a non-object payload. The markdown is length-bounded to
/// [`MAX_PLAN_MARKDOWN_CHARS`].
pub fn parse_grok_exit_plan_request(params: &Value) -> Result<(String, String), String> {
    let obj = params
        .as_object()
        .ok_or_else(|| "exit_plan_mode ext request params is not an object".to_string())?;
    let plan_markdown: String = obj
        .get("planContent")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .chars()
        .take(MAX_PLAN_MARKDOWN_CHARS)
        .collect();
    let tool_call_id = obj
        .get("toolCallId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok((plan_markdown, tool_call_id))
}

/// Serialize the user's decision into Grok's `ExitPlanModeExtResponse` — the
/// reply to a blocked `_x.ai/exit_plan_mode` ext request. See the module-level
/// wire-format note; the encoding is confirmed against Grok 0.2.111.
///
/// `RequestChanges` maps to `"keep_planning"` (any non-approve/non-abandon value
/// keeps Grok in plan mode via its `#[serde(other)]` catch-all). Grok discards the
/// reply `feedback` on that path, so the revision notes are ALSO delivered as a
/// follow-up prompt by the frontend — this field is carried for future-proofing
/// and is harmless if ignored.
pub fn build_grok_exit_plan_response(answer: &PlanApprovalAnswer) -> Value {
    let outcome = match answer.decision {
        PlanApprovalDecision::Approve => "approved",
        PlanApprovalDecision::RequestChanges => "keep_planning",
        PlanApprovalDecision::Abandon | PlanApprovalDecision::HandOff => "abandoned",
    };
    serde_json::json!({
        "outcome": outcome,
        "feedback": answer.normalized_feedback(),
    })
}

/// The `_x.ai/exit_plan_mode` reply for a connection tearing down mid-approval
/// (the parked responder is drained without a user decision). Mirrors Grok's own
/// "client disconnected mid-approval; plan mode stays active" behavior: reply with
/// the keep-planning outcome so Grok keeps plan mode active and re-surfaces the
/// approval on reconnect, rather than silently proceeding as if approved. Must be
/// neither `"approved"` nor `"abandoned"` (both leave plan mode).
pub fn grok_exit_plan_disconnect_response() -> Value {
    serde_json::json!({ "outcome": "keep_planning", "feedback": "" })
}

/// Claude adapter option that leaves plan mode in the same conversation.
pub const CLAUDE_EXIT_PLAN_DEFAULT: &str = "exit-plan-default";
/// Claude adapter option that leaves plan mode straight into
/// `bypassPermissions` (Zwiim's unattended mode); offered only where that mode
/// is.
pub const CLAUDE_EXIT_PLAN_BYPASS: &str = "exit-plan-bypass";
/// Claude adapter option that keeps planning: deny + interrupt, so the turn
/// ends cancelled and the session stays in plan mode.
pub const CLAUDE_EXIT_PLAN_REJECT: &str = "reject";
/// Prefix of the Claude adapter's clear-context options. Choosing one restarts
/// Claude under a private session id the Host never learns, so they are never
/// offered to a client.
pub const CLAUDE_EXIT_PLAN_CLEAR_PREFIX: &str = "exit-plan-clear-";

/// The plan markdown of a Claude `ExitPlanMode` permission request, or `None`
/// when `req` is not one Zwiim can bridge into the plan-approval card: the tool
/// kind must be `switch_mode`, the options must include
/// [`CLAUDE_EXIT_PLAN_DEFAULT`] and [`CLAUDE_EXIT_PLAN_REJECT`], and the plan
/// (`rawInput.plan`, else the text content) must be non-empty. Bounded to
/// [`MAX_PLAN_MARKDOWN_CHARS`].
pub fn parse_claude_exit_plan_request(req: &RequestPermissionRequest) -> Option<String> {
    let has_option = |id: &str| req.options.iter().any(|o| o.option_id.to_string() == id);
    if !has_option(CLAUDE_EXIT_PLAN_DEFAULT) || !has_option(CLAUDE_EXIT_PLAN_REJECT) {
        return None;
    }
    let tool_call = serde_json::to_value(&req.tool_call).ok()?;
    // Adapter 0.99.0 drops `kind` from the permission tool call for AIR
    // clients; the two option ids above already identify `ExitPlanMode`.
    match tool_call.get("kind").and_then(Value::as_str) {
        None | Some("switch_mode") => {}
        Some(_) => return None,
    }
    let from_input = tool_call
        .get("rawInput")
        .and_then(|i| i.get("plan"))
        .and_then(Value::as_str)
        .filter(|p| !p.trim().is_empty());
    let plan = match from_input {
        Some(plan) => plan.to_string(),
        None => tool_call
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|c| c.get("content")?.get("text")?.as_str())
            .collect::<Vec<_>>()
            .join("\n\n"),
    };
    let plan = plan.trim();
    if plan.is_empty() {
        return None;
    }
    Some(plan.chars().take(MAX_PLAN_MARKDOWN_CHARS).collect())
}

/// The Claude adapter option answering a bridged `ExitPlanMode` for `decision`.
/// Only Approve leaves plan mode, into unattended when the request offers it
/// (`bypass_offered`); every other decision keeps planning (hand-off and
/// adjustments continue from the Host, not inside this turn).
pub fn claude_exit_plan_option(decision: PlanApprovalDecision, bypass_offered: bool) -> &'static str {
    match decision {
        PlanApprovalDecision::Approve if bypass_offered => CLAUDE_EXIT_PLAN_BYPASS,
        PlanApprovalDecision::Approve => CLAUDE_EXIT_PLAN_DEFAULT,
        PlanApprovalDecision::RequestChanges
        | PlanApprovalDecision::Abandon
        | PlanApprovalDecision::HandOff => CLAUDE_EXIT_PLAN_REJECT,
    }
}

/// Codex adapter option that approves the plan: codex leaves plan mode itself
/// and implements it within the same prompt.
pub const CODEX_PLAN_IMPLEMENT: &str = "implement_plan";
/// Codex adapter option that keeps planning: codex stays in plan mode and the
/// turn ends normally.
pub const CODEX_PLAN_REVISE: &str = "revise_plan";

/// Whether `req` is Codex's plan-review gate. codex-acp 1.x marks the request
/// `_meta.codex.kind = "plan_review"`; 2.x sends no request `_meta` and is told
/// apart by its `plan-review:<itemId>` tool call id.
pub fn is_codex_plan_review_request(req: &RequestPermissionRequest) -> bool {
    let kind = req
        .meta
        .as_ref()
        .and_then(|m| m.get("codex"))
        .and_then(|codex| codex.get("kind"))
        .and_then(Value::as_str);
    kind == Some("plan_review") || req.tool_call.tool_call_id.to_string().starts_with("plan-review:")
}

/// The plan markdown of a Codex plan-review permission request, or `None` when
/// `req` is not one Zwiim can bridge into the plan-approval card: it must be a
/// plan review ([`is_codex_plan_review_request`]), the tool kind `switch_mode`,
/// the options must include [`CODEX_PLAN_IMPLEMENT`] and [`CODEX_PLAN_REVISE`],
/// and `rawInput.plan` must be non-empty. Bounded to [`MAX_PLAN_MARKDOWN_CHARS`].
pub fn parse_codex_plan_review_request(req: &RequestPermissionRequest) -> Option<String> {
    if !is_codex_plan_review_request(req) {
        return None;
    }
    let has_option = |id: &str| req.options.iter().any(|o| o.option_id.to_string() == id);
    if !has_option(CODEX_PLAN_IMPLEMENT) || !has_option(CODEX_PLAN_REVISE) {
        return None;
    }
    let tool_call = serde_json::to_value(&req.tool_call).ok()?;
    if tool_call.get("kind").and_then(Value::as_str) != Some("switch_mode") {
        return None;
    }
    let plan = tool_call
        .get("rawInput")
        .and_then(|i| i.get("plan"))
        .and_then(Value::as_str)?
        .trim();
    if plan.is_empty() {
        return None;
    }
    Some(plan.chars().take(MAX_PLAN_MARKDOWN_CHARS).collect())
}

/// The Codex adapter option answering a bridged plan review for `decision`.
/// Only Approve implements; every other decision keeps planning (hand-off and
/// adjustments continue from the Host once the turn ends).
pub fn codex_plan_option(decision: PlanApprovalDecision) -> &'static str {
    match decision {
        PlanApprovalDecision::Approve => CODEX_PLAN_IMPLEMENT,
        PlanApprovalDecision::RequestChanges
        | PlanApprovalDecision::Abandon
        | PlanApprovalDecision::HandOff => CODEX_PLAN_REVISE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sacp::schema::{
        PermissionOption, PermissionOptionKind, SessionId, ToolCallContent, ToolCallUpdate,
        ToolCallUpdateFields, ToolKind,
    };
    use serde_json::json;

    fn grok_params(plan: &str) -> Value {
        json!({
            "sessionId": "s-1",
            "toolCallId": "call-42",
            "planContent": plan,
        })
    }

    #[test]
    fn parse_reads_plan_and_tool_call_id() {
        let (plan, tc) = parse_grok_exit_plan_request(&grok_params("# Plan\n- step")).unwrap();
        assert_eq!(plan, "# Plan\n- step");
        assert_eq!(tc, "call-42");
    }

    #[test]
    fn parse_accepts_empty_or_missing_plan() {
        let (plan, tc) = parse_grok_exit_plan_request(&grok_params("")).unwrap();
        assert_eq!(plan, "");
        assert_eq!(tc, "call-42");
        // Missing planContent + toolCallId → empty strings, still Ok.
        let (plan, tc) =
            parse_grok_exit_plan_request(&json!({ "sessionId": "s-1" })).unwrap();
        assert!(plan.is_empty());
        assert!(tc.is_empty());
    }

    #[test]
    fn parse_rejects_non_object() {
        assert!(parse_grok_exit_plan_request(&json!("nope")).is_err());
        assert!(parse_grok_exit_plan_request(&json!([1, 2, 3])).is_err());
    }

    #[test]
    fn parse_bounds_plan_markdown() {
        let huge = "x".repeat(MAX_PLAN_MARKDOWN_CHARS + 500);
        let (plan, _) = parse_grok_exit_plan_request(&grok_params(&huge)).unwrap();
        assert_eq!(plan.chars().count(), MAX_PLAN_MARKDOWN_CHARS);
    }

    #[test]
    fn build_response_maps_each_decision() {
        // Confirmed against Grok 0.2.111: field is `outcome`; approve/abandon are
        // past-tense; request-changes uses the keep-planning catch-all value.
        let approve = PlanApprovalAnswer {
            decision: PlanApprovalDecision::Approve,
            feedback: None,
        };
        assert_eq!(build_grok_exit_plan_response(&approve)["outcome"], "approved");
        assert_eq!(build_grok_exit_plan_response(&approve)["feedback"], "");
        // The obsolete `decision` field must be gone (it was silently defaulted).
        assert!(build_grok_exit_plan_response(&approve)
            .get("decision")
            .is_none());

        let changes = PlanApprovalAnswer {
            decision: PlanApprovalDecision::RequestChanges,
            feedback: Some("  use SSE instead  ".into()),
        };
        let v = build_grok_exit_plan_response(&changes);
        assert_eq!(v["outcome"], "keep_planning");
        // Feedback is trimmed (carried for future-proofing; Grok ignores it here).
        assert_eq!(v["feedback"], "use SSE instead");

        let abandon = PlanApprovalAnswer {
            decision: PlanApprovalDecision::Abandon,
            feedback: None,
        };
        assert_eq!(
            build_grok_exit_plan_response(&abandon)["outcome"],
            "abandoned"
        );
    }

    #[test]
    fn feedback_is_bounded() {
        let huge = "y".repeat(MAX_FEEDBACK_CHARS + 100);
        let answer = PlanApprovalAnswer {
            decision: PlanApprovalDecision::RequestChanges,
            feedback: Some(huge),
        };
        assert_eq!(
            answer.normalized_feedback().chars().count(),
            MAX_FEEDBACK_CHARS
        );
    }

    #[test]
    fn answer_deserializes_from_camel_case_wire() {
        let a: PlanApprovalAnswer =
            serde_json::from_value(json!({ "decision": "request_changes", "feedback": "x" }))
                .unwrap();
        assert_eq!(a.decision, PlanApprovalDecision::RequestChanges);
        assert_eq!(a.feedback.as_deref(), Some("x"));
        // feedback optional.
        let a: PlanApprovalAnswer =
            serde_json::from_value(json!({ "decision": "approve" })).unwrap();
        assert_eq!(a.decision, PlanApprovalDecision::Approve);
        assert!(a.feedback.is_none());
    }

    /// claude-agent-acp 0.81.2's ExitPlanMode permission request, verified
    /// against `buildClaudePermissionPresentation` + `toolInfoFromToolUse` +
    /// `buildExitPlanModePermissionOptions`: kind `switch_mode`, the plan in
    /// `rawInput.plan` and mirrored as text content.
    fn claude_exit_plan(plan: &str, option_ids: &[&str]) -> RequestPermissionRequest {
        claude_request(plan, option_ids, ToolKind::SwitchMode)
    }

    fn claude_request(plan: &str, option_ids: &[&str], kind: ToolKind) -> RequestPermissionRequest {
        RequestPermissionRequest::new(
            SessionId::new("sess-1"),
            ToolCallUpdate::new(
                "toolu_plan",
                ToolCallUpdateFields::new()
                    .title("Ready to code?".to_string())
                    .kind(kind)
                    .raw_input(json!({ "plan": plan })),
            ),
            option_ids
                .iter()
                .map(|id| {
                    PermissionOption::new(
                        id.to_string(),
                        id.to_string(),
                        PermissionOptionKind::AllowAlways,
                    )
                })
                .collect(),
        )
    }

    const CLAUDE_0_81_2_OPTIONS: &[&str] = &[
        "exit-plan-clear-auto",
        "exit-plan-auto",
        "exit-plan-bypass",
        "exit-plan-default",
        "reject",
    ];

    #[test]
    fn claude_parse_accepts_the_exit_plan_card() {
        let req = claude_exit_plan("# Plan\n- step", CLAUDE_0_81_2_OPTIONS);
        assert_eq!(
            parse_claude_exit_plan_request(&req).as_deref(),
            Some("# Plan\n- step")
        );
        // The accept-edits set (no auto / bypass modes advertised).
        let req = claude_exit_plan(
            "plan",
            &[
                "exit-plan-clear-accept-edits",
                "exit-plan-accept-edits",
                "exit-plan-default",
                "reject",
            ],
        );
        assert_eq!(
            parse_claude_exit_plan_request(&req).as_deref(),
            Some("plan")
        );
    }

    #[test]
    fn claude_parse_falls_back_to_text_content() {
        let mut req = claude_exit_plan("", CLAUDE_0_81_2_OPTIONS);
        req.tool_call.fields.raw_input = None;
        req.tool_call.fields.content = Some(vec![ToolCallContent::from("from content")]);
        assert_eq!(
            parse_claude_exit_plan_request(&req).as_deref(),
            Some("from content")
        );
    }

    /// claude-agent-acp 0.99.0's ExitPlanMode request to an AIR client, verified
    /// against `permissions/presentation.js`: `{toolCallId, title, rawInput}`
    /// with no `kind` and no content.
    #[test]
    fn claude_parse_accepts_the_0_99_0_air_card() {
        let req = RequestPermissionRequest::new(
            SessionId::new("sess-1"),
            ToolCallUpdate::new(
                "toolu_plan",
                ToolCallUpdateFields::new()
                    .title("Approve Plan".to_string())
                    .raw_input(json!({ "plan": "# Plan\n- step" })),
            ),
            ["exit-plan-auto", "exit-plan-default", "reject"]
                .iter()
                .map(|id| {
                    PermissionOption::new(
                        id.to_string(),
                        id.to_string(),
                        PermissionOptionKind::AllowOnce,
                    )
                })
                .collect(),
        );
        assert_eq!(
            parse_claude_exit_plan_request(&req).as_deref(),
            Some("# Plan\n- step")
        );
    }

    #[test]
    fn claude_parse_rejects_anything_else() {
        let missing_default = claude_exit_plan("plan", &["exit-plan-auto", "reject"]);
        assert!(parse_claude_exit_plan_request(&missing_default).is_none());
        let missing_reject = claude_exit_plan("plan", &["exit-plan-default", "exit-plan-auto"]);
        assert!(parse_claude_exit_plan_request(&missing_reject).is_none());
        let empty_plan = claude_exit_plan("  \n ", CLAUDE_0_81_2_OPTIONS);
        assert!(parse_claude_exit_plan_request(&empty_plan).is_none());
        let other_kind = claude_request("plan", CLAUDE_0_81_2_OPTIONS, ToolKind::Other);
        assert!(parse_claude_exit_plan_request(&other_kind).is_none());
    }

    #[test]
    fn claude_parse_bounds_the_plan() {
        let huge = "p".repeat(MAX_PLAN_MARKDOWN_CHARS + 10);
        let req = claude_exit_plan(&huge, CLAUDE_0_81_2_OPTIONS);
        assert_eq!(
            parse_claude_exit_plan_request(&req)
                .unwrap()
                .chars()
                .count(),
            MAX_PLAN_MARKDOWN_CHARS
        );
    }

    #[test]
    fn claude_option_maps_every_decision() {
        assert_eq!(
            claude_exit_plan_option(PlanApprovalDecision::Approve, true),
            "exit-plan-bypass"
        );
        assert_eq!(
            claude_exit_plan_option(PlanApprovalDecision::Approve, false),
            "exit-plan-default"
        );
        for decision in [
            PlanApprovalDecision::RequestChanges,
            PlanApprovalDecision::Abandon,
            PlanApprovalDecision::HandOff,
        ] {
            assert_eq!(claude_exit_plan_option(decision, true), "reject");
        }
    }

    /// codex-acp 1.13.1's plan-review request (`src/permissions/plan-review.ts`):
    /// tool call `plan-review:<itemId>`, kind `switch_mode`, the plan in
    /// `rawInput.plan`, and `_meta.codex.kind = "plan_review"` on the request.
    fn codex_plan_review(plan: &str, option_ids: &[&str]) -> RequestPermissionRequest {
        let mut req = RequestPermissionRequest::new(
            SessionId::new("sess-1"),
            ToolCallUpdate::new(
                "plan-review:item-7",
                ToolCallUpdateFields::new()
                    .title("Implement this plan?".to_string())
                    .kind(ToolKind::SwitchMode)
                    .raw_input(json!({ "plan": plan })),
            ),
            option_ids
                .iter()
                .map(|id| {
                    PermissionOption::new(
                        id.to_string(),
                        id.to_string(),
                        PermissionOptionKind::AllowOnce,
                    )
                })
                .collect(),
        );
        req.meta = json!({ "codex": { "kind": "plan_review", "planItemId": "item-7" } })
            .as_object()
            .cloned();
        req
    }

    const CODEX_1_13_1_OPTIONS: &[&str] = &["implement_plan", "revise_plan"];

    #[test]
    fn codex_parse_accepts_the_plan_review() {
        let req = codex_plan_review("# Plan\n- step\n", CODEX_1_13_1_OPTIONS);
        assert_eq!(
            parse_codex_plan_review_request(&req).as_deref(),
            Some("# Plan\n- step")
        );
    }

    #[test]
    fn codex_parse_rejects_anything_else() {
        let missing_revise = codex_plan_review("plan", &["implement_plan"]);
        assert!(parse_codex_plan_review_request(&missing_revise).is_none());
        let empty_plan = codex_plan_review("  ", CODEX_1_13_1_OPTIONS);
        assert!(parse_codex_plan_review_request(&empty_plan).is_none());
        // codex-acp 2.x sends no request `_meta`; the `plan-review:` id marks it.
        let mut no_meta = codex_plan_review("plan", CODEX_1_13_1_OPTIONS);
        no_meta.meta = None;
        assert_eq!(parse_codex_plan_review_request(&no_meta).as_deref(), Some("plan"));
        no_meta.tool_call.tool_call_id = "switch-1".to_string().into();
        assert!(parse_codex_plan_review_request(&no_meta).is_none());
        let mut other_kind = codex_plan_review("plan", CODEX_1_13_1_OPTIONS);
        other_kind.tool_call.fields.kind = Some(ToolKind::Other);
        assert!(parse_codex_plan_review_request(&other_kind).is_none());
        let huge = "p".repeat(MAX_PLAN_MARKDOWN_CHARS + 10);
        let req = codex_plan_review(&huge, CODEX_1_13_1_OPTIONS);
        assert_eq!(
            parse_codex_plan_review_request(&req)
                .unwrap()
                .chars()
                .count(),
            MAX_PLAN_MARKDOWN_CHARS
        );
    }

    #[test]
    fn codex_option_maps_every_decision() {
        assert_eq!(
            codex_plan_option(PlanApprovalDecision::Approve),
            "implement_plan"
        );
        for decision in [
            PlanApprovalDecision::RequestChanges,
            PlanApprovalDecision::Abandon,
            PlanApprovalDecision::HandOff,
        ] {
            assert_eq!(codex_plan_option(decision), "revise_plan");
        }
    }

    #[test]
    fn hand_off_deserializes_and_grok_abandons_it() {
        let a: PlanApprovalAnswer =
            serde_json::from_value(json!({ "decision": "hand_off" })).unwrap();
        assert_eq!(a.decision, PlanApprovalDecision::HandOff);
        assert_eq!(build_grok_exit_plan_response(&a)["outcome"], "abandoned");
    }

    #[test]
    fn disconnect_response_keeps_planning() {
        // On disconnect Grok must stay in plan mode: neither of the two
        // plan-mode-leaving outcomes.
        let v = grok_exit_plan_disconnect_response();
        assert_eq!(v["outcome"], "keep_planning");
        assert_ne!(v["outcome"], "approved");
        assert_ne!(v["outcome"], "abandoned");
    }
}
