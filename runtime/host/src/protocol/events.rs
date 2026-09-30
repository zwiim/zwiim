//! Projection of runtime session state and events into the Zwiim vocabulary.
//!
//! Session events: `{session_id, seq, type, payload}`. `seq` is the live agent
//! connection's event sequence (paired with the subscription's `epoch`); it is
//! monotonic but not contiguous — internal events that have no Zwiim
//! projection still consume a number. Payloads are built field-by-field here,
//! so connection/conversation/folder ids, agent-native session ids, raw ACP
//! JSON (`raw_input`/`raw_output`/`_meta`/`locations`), delegation, config and
//! mode internals never reach a client. Session options reach clients only
//! through the generic `session_options` projection, slash commands through
//! `session_commands`, and host paths inside the session's workspace are made
//! workspace-relative.

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::acp::plan_approval::PendingPlanApprovalState;
use crate::acp::question::{PendingQuestionState, QuestionSpec};
use crate::acp::session_state::{
    LiveContentBlock, LiveSessionSnapshot, PendingPermissionState, PendingUserMessage,
    ToolCallOutput, ToolCallState,
};
use crate::acp::types::{AcpEvent, EventEnvelope, PermissionOptionInfo, UserMessageBlock};
use crate::models::message::{ContentBlock, MessageTurn};

#[derive(Debug, Clone, Serialize)]
pub struct SessionEvent {
    pub session_id: String,
    pub seq: u64,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerminalEvent {
    pub terminal_id: String,
    pub seq: u64,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub payload: Value,
}

/// Opaque replay epoch of one live agent connection. A one-way digest, so the
/// connection id itself never crosses the boundary; a respawned connection
/// (new `seq` space) always gets a different epoch.
pub fn epoch_of(connection_id: &str) -> String {
    let digest = Sha256::digest(format!("zwiim-epoch:{connection_id}").as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

fn to_value<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// Agent-supplied free text can quote host paths; strip them like errors do.
fn scrub(text: &str) -> String {
    super::error::scrub_paths(text)
}

/// Paths under `root` (the session's workspace directory) become relative to
/// it; `root` itself becomes `.`.
fn relativize(text: &str, root: Option<&str>) -> String {
    match root.map(|r| r.trim_end_matches('/')).filter(|r| !r.is_empty()) {
        Some(root) => text.replace(&format!("{root}/"), "").replace(root, "."),
        None => text.to_string(),
    }
}

/// Tool-call content as clients see it: workspace-relative, and without the
/// agent-side terminal ids the host inlines as `[Terminal: <id>]`.
fn tool_content(text: &str, root: Option<&str>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("[Terminal: ") {
        out.push_str(&rest[..at]);
        match rest[at..].find(']') {
            Some(end) => {
                out.push_str("[Terminal]");
                rest = &rest[at + end + 1..];
            }
            None => {
                out.push_str(&rest[at..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    relativize(&out, root)
}

/// `EventEnvelope → SessionEvent`, or `None` for internal-only events. `root`
/// is the session's workspace directory.
pub fn project_envelope(
    session_id: &str,
    root: Option<&str>,
    env: &EventEnvelope,
) -> Option<SessionEvent> {
    let (kind, payload) = project_event(&env.payload, root)?;
    Some(SessionEvent {
        session_id: session_id.to_string(),
        seq: env.seq,
        kind,
        payload,
    })
}

fn project_event(event: &AcpEvent, root: Option<&str>) -> Option<(&'static str, Value)> {
    let rel = |text: &str| relativize(text, root);
    let rel_opt = |text: &Option<String>| text.as_deref().map(rel);
    let content_opt = |text: &Option<String>| text.as_deref().map(|t| tool_content(t, root));
    Some(match event {
        // Sub-agent chunks (`parent_tool_use_id` set) are delegation internals.
        AcpEvent::ContentDelta {
            text,
            parent_tool_use_id: None,
        } => ("agent.message_delta", json!({ "text": text })),
        AcpEvent::Thinking {
            text,
            parent_tool_use_id: None,
        } => ("agent.thought_delta", json!({ "text": text })),
        AcpEvent::UserMessage { message_id, blocks } => (
            "agent.user_message",
            json!({ "message_id": message_id, "blocks": user_blocks(blocks) }),
        ),
        AcpEvent::ToolCall {
            tool_call_id,
            title,
            kind,
            status,
            content,
            raw_output,
            ..
        } => (
            "agent.tool_call",
            json!({
                "tool_call_id": tool_call_id,
                "title": rel(title),
                "kind": kind,
                "status": status,
                "content": content_opt(content)
                    .or_else(|| command_output(raw_output.as_deref(), root)),
            }),
        ),
        AcpEvent::ToolCallUpdate {
            tool_call_id,
            title,
            status,
            content,
            raw_output,
            ..
        } => (
            "agent.tool_call_update",
            json!({
                "tool_call_id": tool_call_id,
                "title": rel_opt(title),
                "status": status,
                "content": content_opt(content)
                    .or_else(|| command_output(raw_output.as_deref(), root)),
            }),
        ),
        AcpEvent::PermissionRequest {
            request_id,
            tool_call,
            options,
            ..
        } => (
            "agent.request",
            permission_request(request_id, tool_call, options, root),
        ),
        AcpEvent::QuestionRequest {
            question_id,
            questions,
        } => ("agent.request", question_request(question_id, questions)),
        AcpEvent::PlanApprovalRequest {
            approval_id,
            plan_markdown,
            ..
        } => ("agent.request", plan_request(approval_id, plan_markdown)),
        AcpEvent::PermissionResolved { request_id } => (
            "agent.request_resolved",
            json!({ "kind": "permission", "request_id": request_id }),
        ),
        AcpEvent::QuestionResolved { question_id } => (
            "agent.request_resolved",
            json!({ "kind": "question", "request_id": question_id }),
        ),
        AcpEvent::PlanApprovalResolved { approval_id } => (
            "agent.request_resolved",
            json!({ "kind": "plan_approval", "request_id": approval_id }),
        ),
        AcpEvent::TurnComplete { stop_reason, .. } => {
            ("agent.turn_complete", json!({ "stop_reason": stop_reason }))
        }
        AcpEvent::Error { message, code, .. } => (
            "agent.error",
            json!({ "message": scrub(message), "code": code }),
        ),
        // The provider's terminal failure (a model the account can't use, a
        // usage limit): the turn ends with no reply, so this is the reason.
        AcpEvent::SessionFailure { record } => {
            let (code, message) = record.as_error()?;
            ("agent.error", json!({ "message": scrub(&message), "code": code }))
        }
        AcpEvent::ConversationStatusChanged { status, .. } => {
            ("session.status", json!({ "status": to_value(status) }))
        }
        AcpEvent::SessionConfigOptions { config_options } => (
            "session.options",
            json!({ "options": crate::session_options::project(config_options) }),
        ),
        AcpEvent::AvailableCommands { commands } => (
            "session.commands",
            json!({ "commands": crate::session_commands::project(commands) }),
        ),
        AcpEvent::UsageUpdate { used, size } => {
            ("session.usage", json!({ "used": used, "size": size }))
        }
        AcpEvent::TurnUsage {
            total_tokens,
            input_tokens,
            output_tokens,
            thought_tokens,
            cached_read_tokens,
            cached_write_tokens,
        } => (
            "session.turn_usage",
            json!({
                "total_tokens": total_tokens,
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "thought_tokens": thought_tokens,
                "cached_read_tokens": cached_read_tokens,
                "cached_write_tokens": cached_write_tokens,
            }),
        ),
        _ => return None,
    })
}

fn user_blocks(blocks: &[UserMessageBlock]) -> Value {
    Value::Array(
        blocks
            .iter()
            .map(|b| match b {
                UserMessageBlock::Text { text } => json!({ "type": "text", "text": text }),
                UserMessageBlock::Image { data, mime_type } => {
                    json!({ "type": "image", "data": data, "mime_type": mime_type })
                }
            })
            .collect(),
    )
}

/// Longest text of one diff side or tool output a request carries.
const MAX_REQUEST_TEXT: usize = 200_000;

fn capped(text: &str) -> String {
    match text.char_indices().nth(MAX_REQUEST_TEXT) {
        Some((end, _)) => format!("{}\n…", &text[..end]),
        None => text.to_string(),
    }
}

/// The agent's plan, when this permission request asks to leave plan mode
/// (the ACP `switch_mode` tool kind): Codex puts it in `rawInput.plan`,
/// Claude in the tool call's text content.
fn plan_of(tool_call: &Value, text: Option<&str>) -> Option<String> {
    if tool_call.get("kind").and_then(Value::as_str) != Some("switch_mode") {
        return None;
    }
    tool_call
        .pointer("/rawInput/plan")
        .and_then(Value::as_str)
        .or(text)
        .map(str::to_string)
}

/// The tool a permission request is about: its text content, file edits and
/// shell command. Raw ACP input never crosses; only these fields do.
fn permission_tool(tool_call: &Value, root: Option<&str>) -> (Value, Option<String>) {
    let rel = |t: &str| relativize(t, root);
    let content = tool_call
        .get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let texts: Vec<&str> = content
        .iter()
        .filter(|c| c.get("type").and_then(Value::as_str) == Some("content"))
        .filter_map(|c| c.pointer("/content/text").and_then(Value::as_str))
        .collect();
    let text = (!texts.is_empty()).then(|| texts.join("\n\n"));
    let diffs: Vec<Value> = content
        .iter()
        .filter(|c| c.get("type").and_then(Value::as_str) == Some("diff"))
        .map(|c| {
            let side = |key: &str| c.get(key).and_then(Value::as_str).map(capped);
            json!({
                "path": c.get("path").and_then(Value::as_str).map(|p| scrub(&rel(p))),
                "old_text": side("oldText"),
                "new_text": side("newText"),
            })
        })
        .collect();
    let command = match tool_call.pointer("/rawInput/command") {
        Some(Value::String(c)) => Some(c.clone()),
        Some(Value::Array(parts)) => Some(
            parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    };
    let plan = plan_of(tool_call, text.as_deref()).map(|p| rel(&capped(&p)));
    let tool = json!({
        "tool_call_id": tool_call.get("toolCallId").and_then(Value::as_str),
        "kind": tool_call.get("kind").and_then(Value::as_str),
        "text": text.map(|t| rel(&capped(&t))),
        "diffs": diffs,
        "command": command.map(|c| rel(&c)),
    });
    (tool, plan)
}

fn permission_request(
    request_id: &str,
    tool_call: &Value,
    options: &[PermissionOptionInfo],
    root: Option<&str>,
) -> Value {
    let (tool, plan_markdown) = permission_tool(tool_call, root);
    json!({
        "kind": "permission",
        "request_id": request_id,
        "title": tool_call
            .get("title")
            .and_then(Value::as_str)
            .map(|t| relativize(t, root)),
        "tool": tool,
        "plan_markdown": plan_markdown,
        "options": options
            .iter()
            .map(|o| json!({ "option_id": o.option_id, "name": o.name, "kind": o.kind }))
            .collect::<Vec<_>>(),
    })
}

fn question_request(question_id: &str, questions: &[QuestionSpec]) -> Value {
    json!({
        "kind": "question",
        "request_id": question_id,
        "questions": questions
            .iter()
            .map(|q| json!({
                "question_id": q.id,
                "header": q.header,
                "question": q.question,
                "multi_select": q.multi_select,
                "options": q.options
                    .iter()
                    .map(|o| json!({ "label": o.label, "description": o.description }))
                    .collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>(),
    })
}

fn plan_request(approval_id: &str, plan_markdown: &str) -> Value {
    json!({
        "kind": "plan_approval",
        "request_id": approval_id,
        "plan_markdown": plan_markdown,
    })
}

/// Zwiim session snapshot: exactly the fields a client needs to render a live
/// session from cold.
pub fn project_snapshot(s: &LiveSessionSnapshot, root: Option<&str>) -> Value {
    json!({
        "status": to_value(&s.status),
        "live_message": s.live_message.as_ref().map(|m| json!({
            "message_id": m.id,
            "role": to_value(&m.role),
            "content": m.content.iter().filter_map(live_block).collect::<Vec<_>>(),
        })),
        "tool_calls": s
            .active_tool_calls
            .iter()
            .map(|t| tool_call(t, root))
            .collect::<Vec<_>>(),
        "pending_request": pending_request(
            s.pending_permission.as_ref(),
            s.pending_question.as_ref(),
            s.pending_plan_approval.as_ref(),
            root,
        ),
        "options": s
            .config_options
            .as_deref()
            .map(crate::session_options::project),
        "pending_user_message": s.pending_user_message.as_ref().map(pending_user_message),
        "last_error": s.last_error.as_ref().map(|e| json!({
            "message": scrub(&e.message),
            "code": e.code,
        })),
        "usage": s.usage.as_ref().map(|u| json!({ "used": u.used, "size": u.size })),
        "turn_usage": s.turn_usage.as_ref().map(|u| json!({
            "total_tokens": u.total_tokens,
            "input_tokens": u.input_tokens,
            "output_tokens": u.output_tokens,
            "thought_tokens": u.thought_tokens,
            "cached_read_tokens": u.cached_read_tokens,
            "cached_write_tokens": u.cached_write_tokens,
        })),
        "commands": crate::session_commands::project(&s.available_commands),
    })
}

fn live_block(block: &LiveContentBlock) -> Option<Value> {
    match block {
        LiveContentBlock::Text {
            text,
            parent_tool_use_id: None,
        } => Some(json!({ "type": "text", "text": text })),
        LiveContentBlock::Thinking {
            text,
            parent_tool_use_id: None,
        } => Some(json!({ "type": "thought", "text": text })),
        LiveContentBlock::ToolCallRef { tool_call_id } => {
            Some(json!({ "type": "tool_call", "tool_call_id": tool_call_id }))
        }
        _ => None,
    }
}

fn tool_call(t: &ToolCallState, root: Option<&str>) -> Value {
    let output = match &t.output {
        Some(ToolCallOutput::Json { value }) => {
            formatted_output(value).map(|text| tool_content(text, root))
        }
        _ => None,
    };
    json!({
        "tool_call_id": t.id,
        "title": relativize(&t.label, root),
        "kind": to_value(&t.kind),
        "status": to_value(&t.status),
        "content": t.content.as_deref().map(|c| tool_content(c, root)).or(output),
    })
}

/// A command's output where the agent reports it only as its raw result:
/// codex-acp's `{"formatted_output": …, "exit_code": …}`, whose content is just
/// the terminal the command ran in. Only the text crosses; the rest of the raw
/// result stays inside.
fn command_output(raw_output: Option<&str>, root: Option<&str>) -> Option<String> {
    let value = serde_json::from_str::<Value>(raw_output?).ok()?;
    formatted_output(&value).map(|text| tool_content(text, root))
}

fn formatted_output(value: &Value) -> Option<&str> {
    value.get("formatted_output")?.as_str()
}

fn pending_request(
    permission: Option<&PendingPermissionState>,
    question: Option<&PendingQuestionState>,
    plan: Option<&PendingPlanApprovalState>,
    root: Option<&str>,
) -> Value {
    if let Some(p) = permission {
        return permission_request(&p.request_id, &p.tool_call, &p.options, root);
    }
    if let Some(q) = question {
        return question_request(&q.question_id, &q.questions);
    }
    if let Some(p) = plan {
        return plan_request(&p.approval_id, &p.plan_markdown);
    }
    Value::Null
}

fn pending_user_message(m: &PendingUserMessage) -> Value {
    json!({ "message_id": m.message_id, "blocks": user_blocks(&m.blocks) })
}

/// A legacy firehose frame → Zwiim terminal event, for `terminal://output/<id>`
/// and `terminal://exit/<id>` only.
pub fn project_terminal(channel: &str, payload: &Value) -> Option<TerminalEvent> {
    let (kind, terminal_id) = if let Some(id) = channel.strip_prefix("terminal://output/") {
        ("terminal.output", id)
    } else if let Some(id) = channel.strip_prefix("terminal://exit/") {
        ("terminal.exit", id)
    } else {
        return None;
    };
    let seq = payload.get("seq").and_then(Value::as_u64).unwrap_or(0);
    let body = if kind == "terminal.output" {
        json!({ "data": payload.get("data").and_then(Value::as_str).unwrap_or_default() })
    } else {
        json!({})
    };
    Some(TerminalEvent {
        terminal_id: terminal_id.to_string(),
        seq,
        kind,
        payload: body,
    })
}

/// Transcript turns → `session.history` turns. Block vocabulary matches the
/// live stream (`text`, `image`, `thought`, `tool_call`, `tool_result`);
/// delegation meta, execution stats, image URIs and agent-native message ids
/// stay on the host. Tool calls read workspace-relative, as live ones do;
/// `root` is the session's workspace directory.
pub fn project_history(turns: &[MessageTurn], root: Option<&str>) -> Value {
    Value::Array(
        turns
            .iter()
            .map(|t| {
                json!({
                    "turn_id": t.id,
                    "role": to_value(&t.role),
                    "blocks": t.blocks.iter().map(|b| history_block(b, root)).collect::<Vec<_>>(),
                    "timestamp": t.timestamp,
                    "completed_at": t.completed_at,
                    "duration_ms": t.duration_ms,
                    "model": t.model,
                    "usage": t.usage.as_ref().map(|u| json!({
                        "input_tokens": u.input_tokens,
                        "output_tokens": u.output_tokens,
                        "cache_creation_input_tokens": u.cache_creation_input_tokens,
                        "cache_read_input_tokens": u.cache_read_input_tokens,
                    })),
                })
            })
            .collect(),
    )
}

/// Codex's transcript wraps a plan in `<proposed_plan>` tags that its live
/// stream leaves out; the plan reads the same either way.
fn without_plan_tags(text: &str) -> String {
    if !text.contains("<proposed_plan>") {
        return text.to_string();
    }
    text.replace("<proposed_plan>", "").replace("</proposed_plan>", "").trim().to_string()
}

fn history_block(block: &ContentBlock, root: Option<&str>) -> Value {
    let rel_opt = |text: &Option<String>| text.as_deref().map(|t| relativize(t, root));
    match block {
        ContentBlock::Text { text } => json!({ "type": "text", "text": without_plan_tags(text) }),
        ContentBlock::Image {
            data, mime_type, ..
        } => json!({ "type": "image", "data": data, "mime_type": mime_type }),
        ContentBlock::ImageGeneration {
            revised_prompt,
            image,
        } => json!({
            "type": "image_generation",
            "revised_prompt": revised_prompt,
            "image": image.as_ref().map(|i| json!({ "data": i.data, "mime_type": i.mime_type })),
        }),
        ContentBlock::ToolUse {
            tool_use_id,
            tool_name,
            input_preview,
            status,
            ..
        } => json!({
            "type": "tool_call",
            "tool_call_id": tool_use_id,
            "title": tool_name,
            "input_preview": rel_opt(input_preview),
            "status": status,
        }),
        ContentBlock::ToolResult {
            tool_use_id,
            output_preview,
            is_error,
            ..
        } => json!({
            "type": "tool_result",
            "tool_call_id": tool_use_id,
            "output_preview": output_preview.as_deref().map(|t| tool_content(t, root)),
            "is_error": is_error,
        }),
        ContentBlock::Thinking { text } => json!({ "type": "thought", "text": text }),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::acp::session_state::SessionState;
    use crate::models::agent::AgentType;

    /// Keys that must never appear anywhere in a projected value.
    pub(crate) const FORBIDDEN_KEYS: &[&str] = &[
        "connection_id",
        "conversation_id",
        "folder_id",
        "external_id",
        "agent_type",
        "raw_input",
        "raw_output",
        "meta",
        "_meta",
        "locations",
        "parent_conversation_id",
        "parent_tool_use_id",
        "id",
    ];

    pub(crate) fn assert_no_internal_keys(v: &Value) {
        match v {
            Value::Object(map) => {
                for (k, child) in map {
                    assert!(!FORBIDDEN_KEYS.contains(&k.as_str()), "leaked `{k}` in {v}");
                    assert_no_internal_keys(child);
                }
            }
            Value::Array(items) => items.iter().for_each(assert_no_internal_keys),
            _ => {}
        }
    }

    #[test]
    fn tool_content_drops_agent_terminal_ids() {
        assert_eq!(
            tool_content("ran [Terminal: term-9f] in /w/p/src", Some("/w/p")),
            "ran [Terminal] in src"
        );
        assert_eq!(tool_content("[Terminal: open", None), "[Terminal: open");
    }

    #[test]
    fn a_command_reported_only_in_its_raw_result_keeps_its_output() {
        // codex-acp: the call's content is the terminal, the update carries
        // only `rawOutput`.
        let update = |content: Option<&str>, raw_output: Option<&str>| {
            AcpEvent::ToolCallUpdate {
                tool_call_id: "t1".into(),
                title: None,
                status: Some("completed".into()),
                content: content.map(str::to_string),
                raw_input: None,
                raw_output: raw_output.map(str::to_string),
                raw_output_append: None,
                locations: None,
                meta: None,
                images: None,
            }
        };
        let raw = r#"{"formatted_output":"/w/p/a.rs\nok\n","exit_code":0}"#;
        let (_, payload) = project_event(&update(None, Some(raw)), Some("/w/p")).unwrap();
        assert_eq!(payload["content"], "a.rs\nok\n");
        assert_no_internal_keys(&payload);

        // The agent's own content wins; any other raw result stays inside.
        let (_, payload) = project_event(&update(Some("shown"), Some(raw)), None).unwrap();
        assert_eq!(payload["content"], "shown");
        let other = update(None, Some(r#"{"result":"x"}"#));
        let (_, payload) = project_event(&other, None).unwrap();
        assert_eq!(payload["content"], Value::Null);

        // Snapshot: from the parsed output.
        let mut state = SessionState::new("c".into(), AgentType::Codex, None, "w".into(), None);
        state.apply_event(&AcpEvent::ToolCall {
            tool_call_id: "t1".into(),
            title: "ls".into(),
            kind: "execute".into(),
            status: "in_progress".into(),
            content: None,
            raw_input: None,
            raw_output: None,
            locations: None,
            meta: None,
            images: None,
        });
        state.apply_event(&update(None, Some(raw)));
        let snap = tool_call(state.active_tool_calls.get("t1").unwrap(), Some("/w/p"));
        assert_eq!(snap["content"], "a.rs\nok\n");
    }

    #[test]
    fn history_projects_blocks_without_internal_fields() {
        use crate::models::message::{ImageData, TurnRole, TurnUsage};
        let turns = vec![
            MessageTurn {
                id: "turn-0".into(),
                role: TurnRole::User,
                blocks: vec![
                    ContentBlock::Text { text: "hi".into() },
                    ContentBlock::Image {
                        data: "AAA".into(),
                        mime_type: "image/png".into(),
                        uri: Some("file:///Users/a/shot.png".into()),
                    },
                ],
                timestamp: chrono::Utc::now(),
                usage: None,
                duration_ms: None,
                model: None,
                completed_at: None,
                agent_message_id: Some("msg_native".into()),
            },
            MessageTurn {
                id: "turn-1".into(),
                role: TurnRole::Assistant,
                blocks: vec![
                    ContentBlock::Thinking { text: "hmm".into() },
                    ContentBlock::ToolUse {
                        tool_use_id: Some("toolu_1".into()),
                        tool_name: "Bash".into(),
                        input_preview: Some("ls".into()),
                        status: Some("completed".into()),
                        meta: Some(json!({"zwiim.delegation": {"child_connection_id": "c"}})),
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: Some("toolu_1".into()),
                        output_preview: Some("a.txt".into()),
                        is_error: false,
                        agent_stats: None,
                        images: vec![ImageData {
                            data: "B".into(),
                            mime_type: "image/png".into(),
                            uri: None,
                        }],
                    },
                    ContentBlock::ImageGeneration {
                        revised_prompt: Some("cat".into()),
                        image: None,
                    },
                    ContentBlock::Text {
                        text: "done".into(),
                    },
                ],
                timestamp: chrono::Utc::now(),
                usage: Some(TurnUsage {
                    input_tokens: 1,
                    output_tokens: 2,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                }),
                duration_ms: Some(5),
                model: Some("m".into()),
                completed_at: None,
                agent_message_id: None,
            },
        ];
        let v = project_history(&turns, None);
        assert_no_internal_keys(&v);
        let text = v.to_string();
        for leaked in [
            "file:///",
            "msg_native",
            "zwiim.delegation",
            "child_connection_id",
        ] {
            assert!(!text.contains(leaked), "{leaked} in {text}");
        }
        assert_eq!(v[0]["turn_id"], "turn-0");
        assert_eq!(v[0]["role"], "user");
        assert_eq!(
            v[0]["blocks"][1],
            json!({"type": "image", "data": "AAA", "mime_type": "image/png"})
        );
        let types: Vec<_> = v[1]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["type"].as_str().unwrap())
            .collect();
        assert_eq!(
            types,
            [
                "thought",
                "tool_call",
                "tool_result",
                "image_generation",
                "text"
            ]
        );
        assert_eq!(v[1]["blocks"][1]["tool_call_id"], "toolu_1");
        assert_eq!(v[1]["blocks"][2]["tool_call_id"], "toolu_1");
        assert_eq!(v[1]["usage"]["output_tokens"], 2);
    }

    #[test]
    fn history_tool_calls_read_workspace_relative() {
        use crate::models::message::TurnRole;
        let turns = vec![MessageTurn {
            id: "turn-0".into(),
            role: TurnRole::Assistant,
            blocks: vec![
                ContentBlock::ToolUse {
                    tool_use_id: Some("toolu_1".into()),
                    tool_name: "Edit".into(),
                    input_preview: Some(r#"{"file_path":"/ws/repo/src/a.rs"}"#.into()),
                    status: Some("completed".into()),
                    meta: None,
                },
                ContentBlock::ToolResult {
                    tool_use_id: Some("toolu_1".into()),
                    output_preview: Some("updated /ws/repo/src/a.rs".into()),
                    is_error: false,
                    agent_stats: None,
                    images: vec![],
                },
            ],
            timestamp: chrono::Utc::now(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
            agent_message_id: None,
        }];
        let v = project_history(&turns, Some("/ws/repo"));
        assert_eq!(v[0]["blocks"][0]["input_preview"], r#"{"file_path":"src/a.rs"}"#);
        assert_eq!(v[0]["blocks"][1]["output_preview"], "updated src/a.rs");
    }

    #[test]
    fn a_codex_plan_reads_without_its_tags() {
        use crate::models::message::TurnRole;
        let turns = vec![MessageTurn {
            id: "turn-0".into(),
            role: TurnRole::Assistant,
            blocks: vec![ContentBlock::Text {
                text: "Here it is.\n\n<proposed_plan>\n**Create `hi.txt`**\n</proposed_plan>".into(),
            }],
            timestamp: chrono::Utc::now(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
            agent_message_id: None,
        }];
        let v = project_history(&turns, None);
        assert_eq!(v[0]["blocks"][0]["text"], "Here it is.\n\n\n**Create `hi.txt`**");
    }

    /// A provider's terminal failure reaches clients as `agent.error`, with the
    /// provider's words and paths scrubbed; a retry warning does not.
    #[test]
    fn a_terminal_provider_failure_is_an_agent_error() {
        let failure = |severity: &str| {
            env(AcpEvent::SessionFailure {
                record: crate::acp::types::SessionFailureRecord {
                    id: "f-1".into(),
                    revision: 1,
                    category: "access".into(),
                    severity: severity.into(),
                    title: "The model `gpt-5.5` does not exist, see /Users/me/.codex/log".into(),
                    details: None,
                    actions: vec![],
                    resolved: false,
                },
            })
        };
        let v = project_envelope("s", None, &failure("error")).expect("projected");
        assert_eq!(v.kind, "agent.error");
        assert_eq!(v.payload["code"], "provider_access");
        let message = v.payload["message"].as_str().unwrap();
        assert!(message.contains("gpt-5.5") && !message.contains("/Users/me"), "{message}");

        assert!(project_envelope("s", None, &failure("warning")).is_none());
    }

    fn env(payload: AcpEvent) -> EventEnvelope {
        EventEnvelope {
            seq: 7,
            connection_id: "conn-secret".into(),
            payload,
        }
    }

    #[test]
    fn projects_the_zwiim_vocabulary_without_internal_fields() {
        let cases = vec![
            (
                AcpEvent::ContentDelta {
                    text: "hi".into(),
                    parent_tool_use_id: None,
                },
                "agent.message_delta",
            ),
            (
                AcpEvent::Thinking {
                    text: "hm".into(),
                    parent_tool_use_id: None,
                },
                "agent.thought_delta",
            ),
            (
                AcpEvent::ToolCall {
                    tool_call_id: "t1".into(),
                    title: "Read".into(),
                    kind: "read".into(),
                    status: "pending".into(),
                    content: None,
                    raw_input: Some("{\"path\":\"/etc\"}".into()),
                    raw_output: None,
                    locations: Some(json!([{"path": "/etc"}])),
                    meta: Some(json!({"x": 1})),
                    images: None,
                },
                "agent.tool_call",
            ),
            (
                AcpEvent::PermissionRequest {
                    request_id: "r1".into(),
                    tool_call: json!({"title": "Run ls", "rawInput": {"cmd": "ls"}}),
                    options: vec![],
                    queued: 0,
                },
                "agent.request",
            ),
            (
                AcpEvent::TurnComplete {
                    session_id: "agent-native".into(),
                    stop_reason: "end_turn".into(),
                    agent_type: "claude_code".into(),
                },
                "agent.turn_complete",
            ),
            (
                AcpEvent::ConversationStatusChanged {
                    conversation_id: 42,
                    status: crate::db::entities::conversation::ConversationStatus::InProgress,
                },
                "session.status",
            ),
            (
                AcpEvent::Error {
                    message: "spawn failed at /Users/me/bin/agent".into(),
                    agent_type: "claude_code".into(),
                    code: None,
                    details: Some("stack".into()),
                    terminal: false,
                },
                "agent.error",
            ),
        ];
        for (event, expected) in cases {
            let projected = project_envelope("sess-1", None, &env(event)).expect("projected");
            assert_eq!(projected.kind, expected);
            let v = serde_json::to_value(&projected).unwrap();
            assert_eq!(v["session_id"], "sess-1");
            assert_eq!(v["seq"], 7);
            assert_no_internal_keys(&v);
            let text = v.to_string();
            assert!(!text.contains("conn-secret") && !text.contains("agent-native"));
            assert!(!text.contains("/Users/me") && !text.contains("rawInput"));
        }
        assert_eq!(
            project_envelope(
                "s",
                None,
                &env(AcpEvent::PermissionRequest {
                    request_id: "r1".into(),
                    tool_call: json!({"title": "Run ls"}),
                    options: vec![],
                    queued: 0,
                })
            )
            .unwrap()
            .payload["kind"],
            "permission"
        );
    }

    #[test]
    fn permission_requests_carry_tool_details_and_plans() {
        let edit = AcpEvent::PermissionRequest {
            request_id: "r1".into(),
            tool_call: json!({
                "toolCallId": "tc-1",
                "title": "Edit /ws/repo/src/a.rs",
                "kind": "edit",
                "content": [
                    {"type": "diff", "path": "/ws/repo/src/a.rs", "oldText": "a", "newText": "b"},
                    {"type": "diff", "path": "/etc/hosts", "oldText": null, "newText": "x"},
                ],
                "rawInput": {"command": ["git", "status"], "secret": "s"},
            }),
            options: vec![],
            queued: 0,
        };
        let v = project_envelope("s", Some("/ws/repo"), &env(edit)).unwrap().payload;
        assert_eq!(v["title"], "Edit src/a.rs");
        assert_eq!(v["tool"]["tool_call_id"], "tc-1");
        assert_eq!(v["tool"]["kind"], "edit");
        assert_eq!(v["tool"]["command"], "git status");
        assert_eq!(
            v["tool"]["diffs"],
            json!([
                {"path": "src/a.rs", "old_text": "a", "new_text": "b"},
                {"path": "<path>", "old_text": null, "new_text": "x"},
            ])
        );
        assert_eq!(v["plan_markdown"], Value::Null);
        assert!(!v.to_string().contains("secret") && !v.to_string().contains("/ws/repo"));

        // Claude's ExitPlanMode: the plan is the tool's text content.
        let claude_plan = AcpEvent::PermissionRequest {
            request_id: "r2".into(),
            tool_call: json!({
                "toolCallId": "tc-2",
                "title": "Ready to code?",
                "kind": "switch_mode",
                "content": [{"type": "content", "content": {"type": "text", "text": "## Plan\n1. Edit /ws/repo/b.rs"}}],
            }),
            options: vec![],
            queued: 0,
        };
        let v = project_envelope("s", Some("/ws/repo"), &env(claude_plan)).unwrap().payload;
        assert_eq!(v["plan_markdown"], "## Plan\n1. Edit b.rs");
        // Codex: the plan rides in its raw input.
        let codex_plan = AcpEvent::PermissionRequest {
            request_id: "r3".into(),
            tool_call: json!({"kind": "switch_mode", "rawInput": {"plan": "- step"}}),
            options: vec![],
            queued: 0,
        };
        let v = project_envelope("s", None, &env(codex_plan)).unwrap().payload;
        assert_eq!(v["plan_markdown"], "- step");
    }

    #[test]
    fn option_events_are_projected_generically() {
        let options: Vec<crate::acp::types::SessionConfigOptionInfo> = serde_json::from_value(json!([
            {"id": "mode", "name": "Mode", "description": null, "category": "mode", "kind": {"type": "select", "current_value": "bypassPermissions", "groups": [], "options": [
                {"value": "default", "name": "Manual", "description": null, "kind": "standard"},
                {"value": "bypassPermissions", "name": "Bypass permissions", "description": null, "kind": "full_access"},
            ]}},
        ]))
        .unwrap();
        let v = project_envelope("s", None, &env(AcpEvent::SessionConfigOptions { config_options: options }))
            .unwrap();
        assert_eq!(v.kind, "session.options");
        assert_eq!(v.payload["options"]["mode"]["current"], "unattended");
        assert_eq!(v.payload["options"]["model"], Value::Null);
        assert!(!v.payload.to_string().contains("full_access"));
    }

    #[test]
    fn command_events_carry_only_zwiim_fields() {
        let v = project_envelope(
            "s",
            None,
            &env(AcpEvent::AvailableCommands {
                commands: vec![crate::acp::types::AvailableCommandInfo {
                    name: "review".into(),
                    description: "Review a PR".into(),
                    input_hint: Some("pr number".into()),
                }],
            }),
        )
        .unwrap();
        assert_eq!(v.kind, "session.commands");
        assert_eq!(
            v.payload,
            json!({"commands": [{"name": "review", "description": "Review a PR", "hint": "pr number"}]})
        );
    }

    #[test]
    fn usage_events_carry_only_what_the_agent_reported() {
        let v = project_envelope("s", None, &env(AcpEvent::UsageUpdate { used: Some(5_000), size: Some(200_000) }))
            .unwrap();
        assert_eq!(v.kind, "session.usage");
        assert_eq!(v.payload, json!({"used": 5_000, "size": 200_000}));
        // A field the agent sent as null stays null, never 0.
        let v = project_envelope("s", None, &env(AcpEvent::UsageUpdate { used: Some(12), size: None })).unwrap();
        assert_eq!(v.payload, json!({"used": 12, "size": null}));

        let v = project_envelope(
            "s",
            None,
            &env(AcpEvent::TurnUsage {
                total_tokens: 120,
                input_tokens: 100,
                output_tokens: 20,
                thought_tokens: None,
                cached_read_tokens: Some(80),
                cached_write_tokens: None,
            }),
        )
        .unwrap();
        assert_eq!(v.kind, "session.turn_usage");
        assert_eq!(
            v.payload,
            json!({
                "total_tokens": 120,
                "input_tokens": 100,
                "output_tokens": 20,
                "thought_tokens": null,
                "cached_read_tokens": 80,
                "cached_write_tokens": null,
            })
        );

        let mut state = SessionState::new("c".into(), AgentType::Codex, None, "w".into(), None);
        let snap = project_snapshot(&state.to_snapshot(), None);
        assert_eq!((&snap["usage"], &snap["turn_usage"]), (&Value::Null, &Value::Null));
        state.apply_event(&AcpEvent::UsageUpdate { used: Some(7), size: Some(258_400) });
        state.apply_event(&AcpEvent::TurnUsage {
            total_tokens: 7,
            input_tokens: 5,
            output_tokens: 2,
            thought_tokens: Some(1),
            cached_read_tokens: None,
            cached_write_tokens: None,
        });
        let snap = project_snapshot(&state.to_snapshot(), None);
        assert_eq!(snap["usage"], json!({"used": 7, "size": 258_400}));
        assert_eq!(snap["turn_usage"]["thought_tokens"], 1);
        assert_eq!(snap["turn_usage"]["cached_read_tokens"], Value::Null);
    }

    #[test]
    fn internal_events_are_not_projected() {
        for event in [
            AcpEvent::SessionStarted {
                session_id: "agent-native".into(),
            },
            AcpEvent::ConversationLinked {
                conversation_id: 1,
                folder_id: 2,
                parent_conversation_id: None,
                parent_tool_use_id: None,
            },
            AcpEvent::ClaudeSdkMessage {
                session_id: "x".into(),
                message: json!({}),
            },
            AcpEvent::ContentDelta {
                text: "sub-agent".into(),
                parent_tool_use_id: Some("tu".into()),
            },
        ] {
            assert!(project_envelope("s", None, &env(event)).is_none());
        }
    }

    #[test]
    fn snapshot_projection_carries_only_zwiim_fields() {
        let mut state = SessionState::new(
            "conn-secret".into(),
            AgentType::ClaudeCode,
            Some("/Users/me/repo".into()),
            "w".into(),
            None,
        );
        state.conversation_id = Some(42);
        state.external_id = Some("agent-native".into());
        let v = project_snapshot(&state.to_snapshot(), None);
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "commands",
                "last_error",
                "live_message",
                "options",
                "pending_request",
                "pending_user_message",
                "status",
                "tool_calls",
                "turn_usage",
                "usage"
            ]
        );
        assert_no_internal_keys(&v);
        let text = v.to_string();
        assert!(!text.contains("conn-secret") && !text.contains("agent-native"));
        assert!(!text.contains("/Users/me"));
    }

    #[test]
    fn epoch_is_opaque_and_per_connection() {
        let a = epoch_of("conn-a");
        assert_eq!(a, epoch_of("conn-a"));
        assert_ne!(a, epoch_of("conn-b"));
        assert!(!a.contains("conn"));
    }

    #[test]
    fn terminal_frames_project_output_and_exit() {
        let out = project_terminal(
            "terminal://output/t1",
            &json!({"terminal_id": "t1", "data": "ls\r\n", "seq": 3}),
        )
        .unwrap();
        assert_eq!(
            (out.kind, out.seq, out.terminal_id.as_str()),
            ("terminal.output", 3, "t1")
        );
        assert_eq!(out.payload, json!({"data": "ls\r\n"}));
        let exit = project_terminal(
            "terminal://exit/t1",
            &json!({"terminal_id": "t1", "data": "", "seq": 0}),
        )
        .unwrap();
        assert_eq!(exit.kind, "terminal.exit");
        assert!(project_terminal("conversation://changed", &json!({})).is_none());
    }
}
