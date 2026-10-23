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

use std::collections::HashSet;

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::acp::git_tools;
use crate::acp::plan_approval::PendingPlanApprovalState;
use crate::acp::question::{PendingQuestionState, QuestionSpec};
use crate::acp::session_state::{
    LiveContentBlock, LiveSessionSnapshot, PendingPermissionState, PendingUserMessage,
    ToolCallOutput, ToolCallState,
};
use crate::acp::types::{AcpEvent, EventEnvelope, PermissionOptionInfo, UserMessageBlock};
use crate::models::message::{ContentBlock, ImageData, MessageTurn, TurnRole};

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

/// Agent-supplied free text can quote host paths or a refused API key; strip
/// them like errors do.
fn scrub(text: &str) -> String {
    super::error::scrub_paths(&crate::redact::scrub_secrets(text))
}

/// Paths under `root` (the session's workspace directory) become relative to
/// it; `root` itself becomes `.`.
fn relativize(text: &str, root: Option<&str>) -> String {
    match root.map(|r| r.trim_end_matches('/')).filter(|r| !r.is_empty()) {
        Some(root) => text.replace(&format!("{root}/"), "").replace(root, "."),
        None => text.to_string(),
    }
}

/// A tool-call title as clients see it: Zwiim's Git tools by their readable
/// step names, never the agent's MCP naming.
fn tool_title(title: &str, root: Option<&str>) -> String {
    match git_tools::kind_of_title(title) {
        Some(kind) => git_tools::display_title(kind).to_string(),
        None => relativize(title, root),
    }
}

/// A tool-call kind as clients see it: a Zwiim Git tool is never a shell
/// command, whatever the agent files it under.
fn tool_kind<'a>(kind: Option<&'a str>, title: Option<&str>) -> Option<&'a str> {
    match title.and_then(git_tools::kind_of_title) {
        Some(_) => Some("other"),
        None => kind,
    }
}

/// Tool-call content as clients see it: workspace-relative, without the
/// agent-side terminal ids the host inlines as `[Terminal: <id>]`, Git tools
/// by their readable names and a PR opened or merged as one line.
fn tool_content(text: &str, root: Option<&str>) -> String {
    if let Some(line) = git_tools::display_result(text) {
        return line;
    }
    let readable = git_tools::readable_tool_names(text);
    let text = readable.as_str();
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
        AcpEvent::ContentImage { data, mime_type } => (
            "agent.message_image",
            crate::agent_images::one(root, data, mime_type, None)?,
        ),
        AcpEvent::UserMessage { message_id, blocks } => (
            "agent.user_message",
            json!({ "message_id": message_id, "blocks": user_blocks(blocks, root) }),
        ),
        AcpEvent::ToolCall {
            tool_call_id,
            title,
            kind,
            status,
            content,
            raw_output,
            locations,
            images,
            ..
        } => (
            "agent.tool_call",
            with_images(
                json!({
                    "tool_call_id": tool_call_id,
                    "title": tool_title(title, root),
                    "kind": tool_kind(Some(kind.as_str()), Some(title)),
                    "status": status,
                    "content": content_opt(content)
                        .or_else(|| command_output(raw_output.as_deref(), root)),
                }),
                root,
                images.as_deref(),
                first_location(locations.as_ref()),
            ),
        ),
        AcpEvent::ToolCallUpdate {
            tool_call_id,
            title,
            status,
            content,
            raw_output,
            locations,
            images,
            ..
        } => (
            "agent.tool_call_update",
            with_images(
                json!({
                    "tool_call_id": tool_call_id,
                    "title": title.as_deref().map(|t| tool_title(t, root)),
                    "status": status,
                    "content": content_opt(content)
                        .or_else(|| command_output(raw_output.as_deref(), root)),
                }),
                root,
                images.as_deref(),
                first_location(locations.as_ref()),
            ),
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

fn user_blocks(blocks: &[UserMessageBlock], root: Option<&str>) -> Value {
    Value::Array(
        blocks
            .iter()
            .flat_map(|b| match b {
                UserMessageBlock::Text { text } => user_text(text, root),
                UserMessageBlock::Image { data, mime_type } => {
                    vec![json!({ "type": "image", "data": data, "mime_type": mime_type })]
                }
            })
            .collect(),
    )
}

/// User text → its blocks: the attachment note becomes `attachment` blocks
/// after the text (dropped when the user typed nothing), so clients never see
/// the note or a host path.
fn user_text(text: &str, root: Option<&str>) -> Vec<Value> {
    let Some(split) = crate::attachments::split_note(text, root) else {
        return vec![json!({ "type": "text", "text": text })];
    };
    let mut out = Vec::with_capacity(split.attachments.len() + 1);
    if !split.text.is_empty() {
        out.push(json!({ "type": "text", "text": split.text }));
    }
    out.extend(split.attachments);
    out
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
        "kind": tool_kind(
            tool_call.get("kind").and_then(Value::as_str),
            tool_call.get("title").and_then(Value::as_str),
        ),
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
            .map(|t| tool_title(t, root)),
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
            "content": m.content.iter().filter_map(|b| live_block(b, root)).collect::<Vec<_>>(),
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
        "pending_user_message": s.pending_user_message.as_ref().map(|m| pending_user_message(m, root)),
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

fn live_block(block: &LiveContentBlock, root: Option<&str>) -> Option<Value> {
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
        LiveContentBlock::Image { data, mime_type } => {
            agent_image(crate::agent_images::one(root, data, mime_type, None), None).pop()
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
    with_images(
        json!({
            "tool_call_id": t.id,
            "title": tool_title(&t.label, root),
            "kind": match git_tools::kind_of_title(&t.label) {
                Some(_) => json!("other"),
                None => to_value(&t.kind),
            },
            "status": to_value(&t.status),
            "content": t.content.as_deref().map(|c| tool_content(c, root)).or(output),
        }),
        root,
        Some(&t.images),
        first_location(t.locations.as_ref()),
    )
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

fn pending_user_message(m: &PendingUserMessage, root: Option<&str>) -> Value {
    json!({ "message_id": m.message_id, "blocks": user_blocks(&m.blocks, root) })
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

/// `v` with `images` set when any of `images` could be stored.
fn with_images(mut v: Value, root: Option<&str>, images: Option<&[ImageData]>, source: Option<&str>) -> Value {
    let projected = images.map(|i| crate::agent_images::project(root, i, source)).unwrap_or_default();
    if !projected.is_empty() {
        v["images"] = Value::Array(projected);
    }
    v
}

/// The first file a tool reported touching (ACP `locations[0].path`).
fn first_location(locations: Option<&Value>) -> Option<&str> {
    locations?.get(0)?.get("path")?.as_str()
}

/// Tool use id → the file its input names (`file_path` / `path`), for
/// pairing a history result's image with the file it was read from.
fn input_files(blocks: &[ContentBlock]) -> std::collections::HashMap<&str, String> {
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { tool_use_id: Some(id), input_preview: Some(input), .. } => {
                let v = serde_json::from_str::<Value>(input).ok()?;
                let path = v.get("file_path").or_else(|| v.get("path"))?.as_str()?.to_string();
                Some((id.as_str(), path))
            }
            _ => None,
        })
        .collect()
}

fn agent_image(image: Option<Value>, revised_prompt: Option<&str>) -> Vec<Value> {
    image
        .map(|mut v| {
            v["type"] = json!("agent_image");
            if let Some(p) = revised_prompt {
                v["revised_prompt"] = json!(p);
            }
            vec![v]
        })
        .unwrap_or_default()
}

/// An agent turn's block; images become files (`agent_image`), never bytes.
fn agent_block(
    block: &ContentBlock,
    root: Option<&str>,
    sources: &std::collections::HashMap<&str, String>,
) -> Vec<Value> {
    use crate::agent_images::one;
    match block {
        ContentBlock::Image { data, mime_type, .. } => agent_image(one(root, data, mime_type, None), None),
        ContentBlock::ImageGeneration { revised_prompt, image } => match image {
            Some(i) => agent_image(one(root, &i.data, &i.mime_type, None), revised_prompt.as_deref()),
            None => vec![], // still generating
        },
        ContentBlock::ToolResult { tool_use_id, images, .. } => {
            let source = tool_use_id.as_deref().and_then(|id| sources.get(id)).map(String::as_str);
            vec![with_images(history_block(block, root), root, Some(images), source)]
        }
        _ => vec![history_block(block, root)],
    }
}

/// Transcript turns → `session.history` turns. Block vocabulary matches the
/// live stream (`text`, `image` (user), `agent_image`, `thought`, `tool_call`,
/// `tool_result`);
/// delegation meta, execution stats, image URIs and agent-native message ids
/// stay on the host. Tool calls read workspace-relative, as live ones do;
/// `root` is the session's workspace directory.
pub fn project_history(turns: &[MessageTurn], root: Option<&str>) -> Value {
    project_history_with(turns, root, &HashSet::new())
}

/// [`project_history`], with `revertible` on every user turn: true when the
/// turn's id is in `revertible`, that is, the files it started with were saved
/// and `session.revert` can put them back.
pub fn project_history_with(turns: &[MessageTurn], root: Option<&str>, revertible: &HashSet<String>) -> Value {
    Value::Array(
        turns
            .iter()
            .map(|t| {
                let sources = input_files(&t.blocks);
                let mut turn = json!({
                    "turn_id": t.id,
                    "role": to_value(&t.role),
                    "blocks": t.blocks.iter().flat_map(|b| match (&t.role, b) {
                        (TurnRole::User, ContentBlock::Text { text }) => user_text(&without_plan_tags(text), root),
                        (TurnRole::User, _) => vec![history_block(b, root)],
                        _ => agent_block(b, root, &sources),
                    }).collect::<Vec<_>>(),
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
                });
                if matches!(t.role, TurnRole::User) {
                    turn["revertible"] = Value::Bool(revertible.contains(&t.id));
                }
                turn
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
            "title": tool_title(tool_name, root),
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
        assert_eq!(types, ["thought", "tool_call", "tool_result", "text"]);
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
    fn a_live_user_message_shows_attachments_not_the_note() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();
        let file = dir.path().join(".zwiim/attachments/k3F9a2Qx/Q3 report.pdf");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"12345").unwrap();
        let event = AcpEvent::UserMessage {
            message_id: "m1".into(),
            blocks: vec![
                UserMessageBlock::Text {
                    text: "summarize this".into(),
                },
                UserMessageBlock::Text {
                    text: crate::attachments::note(&[file.clone()]),
                },
            ],
        };
        let (kind, payload) = project_event(&event, Some(root)).unwrap();
        assert_eq!(kind, "agent.user_message");
        assert_eq!(
            payload["blocks"],
            json!([
                { "type": "text", "text": "summarize this" },
                {
                    "type": "attachment",
                    "attachment_id": "k3F9a2Qx",
                    "name": "Q3 report.pdf",
                    "mime_type": "application/pdf",
                    "size": 5,
                    "path": ".zwiim/attachments/k3F9a2Qx/Q3 report.pdf",
                },
            ])
        );
        let wire = payload.to_string();
        assert!(!wire.contains("zwiim-attachments") && !wire.contains(root));

        let pending = PendingUserMessage {
            message_id: "m1".into(),
            blocks: vec![UserMessageBlock::Text {
                text: crate::attachments::note(&[file]),
            }],
        };
        let v = pending_user_message(&pending, Some(root));
        assert_eq!(v["blocks"].as_array().unwrap().len(), 1);
        assert_eq!(v["blocks"][0]["type"], "attachment");
    }

    #[test]
    fn a_history_turn_of_only_attachments_is_kept() {
        let note = crate::attachments::note(&[
            "/ws/repo/.zwiim/attachments/k3F9a2Qx/a.xlsx".into(),
            "/elsewhere/.zwiim/attachments/Zp81QaLm/b.csv".into(),
        ]);
        let turns = vec![MessageTurn {
            id: "turn-0".into(),
            role: TurnRole::User,
            blocks: vec![ContentBlock::Text { text: note.clone() }],
            timestamp: chrono::Utc::now(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
            agent_message_id: None,
        }];
        let v = project_history(&turns, Some("/ws/repo"));
        let blocks = v[0]["blocks"].as_array().unwrap();
        // The file outside the session's workspace stays text, never a chip.
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "text");
        assert!(blocks[0]["text"].as_str().unwrap().contains("b.csv"));
        assert_eq!(blocks[1]["attachment_id"], "k3F9a2Qx");
        assert_eq!(blocks[1]["path"], ".zwiim/attachments/k3F9a2Qx/a.xlsx");
        assert!(blocks[1].get("size").is_none(), "file is gone");

        // An agent turn quoting a note is not the user's attachment.
        let mut agent = turns;
        agent[0].role = TurnRole::Assistant;
        let v = project_history(&agent, Some("/ws/repo"));
        assert_eq!(v[0]["blocks"][0]["text"], note);
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

    #[test]
    fn agent_errors_never_carry_an_api_key() {
        let refused = env(AcpEvent::Error {
            message: "401 Incorrect API key provided: sk-proj-**wxyz. See your key sk-proj-Abc123".into(),
            agent_type: "codex".into(),
            code: None,
            details: None,
            terminal: true,
        });
        let v = project_envelope("s", None, &refused).expect("projected");
        let message = v.payload["message"].as_str().unwrap();
        assert!(message.contains("Incorrect API key") && !message.contains("sk-"), "{message}");
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
    fn zwiim_git_tools_show_as_readable_steps() {
        for (raw, kind) in [
            ("mcp__zwiim-mcp__git_pr_create", "other"),
            ("mcp.zwiim-mcp.git_pr_create", "execute"),
        ] {
            let call = project_envelope(
                "s",
                Some("/w/p"),
                &env(AcpEvent::ToolCall {
                    tool_call_id: "t1".into(),
                    title: raw.into(),
                    kind: kind.into(),
                    status: "pending".into(),
                    content: Some(format!("Tool: {raw}")),
                    raw_input: None,
                    raw_output: None,
                    locations: None,
                    meta: None,
                    images: None,
                }),
            )
            .unwrap();
            assert_eq!(call.payload["title"], "Create pull request");
            assert_eq!(call.payload["kind"], "other", "never a shell command");
            assert_eq!(call.payload["content"], "Tool: Create pull request");

            let done = project_envelope(
                "s",
                Some("/w/p"),
                &env(AcpEvent::ToolCallUpdate {
                    tool_call_id: "t1".into(),
                    title: Some(raw.into()),
                    status: Some("completed".into()),
                    content: Some(
                        r#"{"number":62,"op":"pr_create","url":"https://github.com/o/r/pull/62"}"#
                            .into(),
                    ),
                    raw_input: None,
                    raw_output: None,
                    raw_output_append: None,
                    locations: None,
                    meta: None,
                    images: None,
                }),
            )
            .unwrap();
            assert_eq!(done.payload["title"], "Create pull request");
            assert_eq!(
                done.payload["content"],
                "Opened #62 · https://github.com/o/r/pull/62"
            );
        }

        // The permission card names the step, not the MCP tool.
        let ask = project_envelope(
            "s",
            None,
            &env(AcpEvent::PermissionRequest {
                request_id: "r1".into(),
                tool_call: json!({"title": "mcp__zwiim-mcp__git_pr_merge", "kind": "execute"}),
                options: vec![],
                queued: 0,
            }),
        )
        .unwrap();
        assert_eq!(ask.payload["title"], "Merge pull request");
        assert_eq!(ask.payload["tool"]["kind"], "other");

        // A merge result, pretty-printed as the MCP result text is.
        let merged = serde_json::to_string_pretty(
            &json!({"op": "pr_merge", "number": 7, "method": "squash", "merged_at": null}),
        )
        .unwrap();
        assert_eq!(tool_content(&merged, None), "Merged #7");

        // Transcript history: Codex's spelling too.
        let block = history_block(
            &ContentBlock::ToolUse {
                tool_use_id: Some("call_1".into()),
                tool_name: "mcp__zwiim_mcp__git_push".into(),
                input_preview: None,
                status: Some("completed".into()),
                meta: None,
            },
            None,
        );
        assert_eq!(block["title"], "Push branch");

        // Other tools, and lookalikes, are left alone.
        assert_eq!(tool_title("Run git push", None), "Run git push");
        assert_eq!(tool_title("mcp__other__git_push", None), "mcp__other__git_push");
        assert_eq!(tool_title("mcp__zwiim-mcp__browser_open", None), "mcp__zwiim-mcp__browser_open");
        assert_eq!(tool_content(r#"{"op":"status"}"#, None), r#"{"op":"status"}"#);
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
            AcpEvent::HistoryRewound,
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

    const PNG: &str = "iVBORw0KGgo=";

    fn img() -> crate::models::message::ImageData {
        crate::models::message::ImageData { data: PNG.into(), mime_type: "image/png".into(), uri: None }
    }

    #[test]
    fn tool_call_images_project_as_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();
        let (kind, p) = project_event(
            &AcpEvent::ToolCallUpdate {
                tool_call_id: "t1".into(),
                title: None,
                status: Some("completed".into()),
                content: None,
                raw_input: None,
                raw_output: None,
                raw_output_append: None,
                locations: Some(json!([{ "path": "/tmp/elsewhere/shot.png" }])),
                meta: None,
                images: Some(vec![img()]),
            },
            Some(root),
        )
        .unwrap();
        assert_eq!(kind, "agent.tool_call_update");
        assert_no_internal_keys(&p);
        let path = p["images"][0]["path"].as_str().unwrap();
        assert!(path.starts_with(".zwiim/images/"), "{p}");
        assert!(dir.path().join(path).is_file());
        assert!(!p.to_string().contains(PNG), "no base64 crosses");
        // No images: no key (replace-on-update keeps the earlier ones).
        let (_, p) = project_event(
            &AcpEvent::ToolCallUpdate {
                tool_call_id: "t1".into(), title: None, status: None, content: None, raw_input: None,
                raw_output: None, raw_output_append: None, locations: None, meta: None, images: None,
            },
            Some(root),
        )
        .unwrap();
        assert!(p.get("images").is_none(), "{p}");
    }

    #[test]
    fn corrupt_image_is_omitted_but_event_still_projects() {
        let dir = tempfile::tempdir().unwrap();
        let (_, p) = project_event(
            &AcpEvent::ToolCallUpdate {
                tool_call_id: "t1".into(),
                title: None,
                status: Some("completed".into()),
                content: Some("done".into()),
                raw_input: None,
                raw_output: None,
                raw_output_append: None,
                locations: None,
                meta: None,
                images: Some(vec![crate::models::message::ImageData {
                    data: "%%%".into(), mime_type: "image/png".into(), uri: None,
                }]),
            },
            dir.path().to_str(),
        )
        .unwrap();
        assert_eq!(p["status"], "completed");
        assert_eq!(p["content"], "done");
        assert!(p.get("images").is_none(), "{p}");
    }

    #[test]
    fn read_of_a_workspace_image_points_at_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".zwiim/images")).unwrap();
        std::fs::write(dir.path().join(".zwiim/images/shot.png"), b"x").unwrap();
        let abs = dir.path().join(".zwiim/images/shot.png");
        let (_, p) = project_event(
            &AcpEvent::ToolCallUpdate {
                tool_call_id: "t1".into(), title: None, status: None, content: None, raw_input: None,
                raw_output: None, raw_output_append: None,
                locations: Some(json!([{ "path": abs.to_str().unwrap() }])),
                meta: None, images: Some(vec![img()]),
            },
            dir.path().to_str(),
        )
        .unwrap();
        assert_eq!(p["images"], json!([{ "name": "shot.png", "mime_type": "image/png", "path": ".zwiim/images/shot.png" }]));
        assert_eq!(std::fs::read_dir(dir.path().join(".zwiim/images")).unwrap().count(), 1, "no copy");
    }

    #[test]
    fn snapshot_tool_call_carries_image_paths() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = SessionState::new("conn".into(), AgentType::Codex, Some(dir.path().to_path_buf()), "w".into(), None);
        state.apply_event(&AcpEvent::ToolCall {
            tool_call_id: "t1".into(),
            title: "Generate image".into(),
            kind: "other".into(),
            status: "completed".into(),
            content: None,
            raw_input: None,
            raw_output: None,
            locations: None,
            meta: None,
            images: Some(vec![img()]),
        });
        let v = project_snapshot(&state.to_snapshot(), dir.path().to_str());
        assert_no_internal_keys(&v);
        assert!(v["tool_calls"][0]["images"][0]["path"].as_str().unwrap().starts_with(".zwiim/images/"));
        assert!(!v.to_string().contains(PNG));
    }

    #[test]
    fn history_agent_images_project_as_paths() {
        use crate::models::message::TurnRole;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.png"), b"x").unwrap();
        let abs = dir.path().join("a.png");
        let turns = vec![MessageTurn {
            id: "t".into(),
            role: TurnRole::Assistant,
            blocks: vec![
                ContentBlock::Image { data: PNG.into(), mime_type: "image/png".into(), uri: None },
                ContentBlock::ImageGeneration { revised_prompt: Some("a cat".into()), image: Some(img()) },
                ContentBlock::ImageGeneration { revised_prompt: Some("pending".into()), image: None },
                ContentBlock::ToolUse {
                    tool_use_id: Some("r1".into()),
                    tool_name: "Read".into(),
                    input_preview: Some(json!({ "file_path": abs.to_str().unwrap() }).to_string()),
                    status: Some("completed".into()),
                    meta: None,
                },
                ContentBlock::ToolResult {
                    tool_use_id: Some("r1".into()),
                    output_preview: None,
                    is_error: false,
                    agent_stats: None,
                    images: vec![img()],
                },
            ],
            timestamp: chrono::Utc::now(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
            agent_message_id: None,
        }];
        let v = project_history(&turns, dir.path().to_str());
        assert_no_internal_keys(&v);
        assert!(!v.to_string().contains(PNG));
        let blocks = v[0]["blocks"].as_array().unwrap();
        let types: Vec<_> = blocks.iter().map(|b| b["type"].as_str().unwrap()).collect();
        assert_eq!(types, ["agent_image", "agent_image", "tool_call", "tool_result"], "in-flight generation omitted");
        assert!(blocks[0]["path"].as_str().unwrap().starts_with(".zwiim/images/"));
        assert_eq!(blocks[1]["revised_prompt"], "a cat");
        assert_eq!(blocks[3]["images"], json!([{ "name": "a.png", "mime_type": "image/png", "path": "a.png" }]));
    }

    #[test]
    fn message_image_chunks_project_as_paths_live_and_in_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str();
        let (kind, p) = project_event(
            &AcpEvent::ContentImage { data: PNG.into(), mime_type: "image/png".into() },
            root,
        )
        .unwrap();
        assert_eq!(kind, "agent.message_image");
        assert!(p["path"].as_str().unwrap().starts_with(".zwiim/images/"));
        assert!(!p.to_string().contains(PNG));

        let mut state = SessionState::new("conn".into(), AgentType::Codex, Some(dir.path().to_path_buf()), "w".into(), None);
        state.apply_event(&AcpEvent::ContentDelta { text: "look:".into(), parent_tool_use_id: None });
        state.apply_event(&AcpEvent::ContentImage { data: PNG.into(), mime_type: "image/png".into() });
        let v = project_snapshot(&state.to_snapshot(), root);
        let content = &v["live_message"]["content"];
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "agent_image");
        assert_eq!(content[1]["path"], p["path"]);
        assert!(!v.to_string().contains(PNG));
    }

    #[test]
    fn corrupt_message_image_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        assert!(project_event(
            &AcpEvent::ContentImage { data: "%%%".into(), mime_type: "image/png".into() },
            dir.path().to_str(),
        )
        .is_none());
    }
}
