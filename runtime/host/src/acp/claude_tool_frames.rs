//! Claude tool-call frames, made whole again.
//!
//! claude-agent-acp 0.99.0 treats Zwiim as an AIR client (we declare
//! `_meta.jetbrains.air`) and so trims what it sends:
//!
//! 1. A `tool_call_update` drops every `_meta` value an earlier frame of the
//!    same tool call already sent: keys of `claudeCode`, keys of
//!    `jetbrains.air`, and the flags `is_mcp_tool_call` / `terminal_info`
//!    (`tool-calls/changed-meta-filter.js`). The Host reads those keys on
//!    every update (sub-agent parent, launch marking, tool name), so
//!    [`ClaudeToolFrames`] merges them back in before anything else sees it.
//! 2. A permission request's tool call carries no `kind`, and no `content`
//!    unless the client declares `diffPatch` (`permissions/presentation.js`).
//!    The diff an Edit/Write card shows lives on the tool call's own frames,
//!    so [`ClaudeToolFrames::fill_permission`] copies the last `kind` and
//!    `content` seen for that tool call into the request.
//!
//! The handler sits in the connection's handler chain, so it sees each frame
//! in arrival order — the tool call is always recorded before the permission
//! request the adapter sends after it. Without a recorded frame both steps
//! leave the message as it came. Frames from an adapter that sends full
//! `_meta` on every update pass through unchanged.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use sacp::schema::{ToolCallContent, ToolCallUpdate, ToolKind};
use sacp::{Agent, ConnectionTo, Dispatch, Handled};
use serde_json::{Map, Value};

/// Tool calls remembered per connection; the adapter's filter keeps as many.
const TOOL_CALL_LIMIT: usize = 2048;

/// `_meta` namespaces whose keys the adapter sends only when they change.
const MERGED_NAMESPACES: [&str; 1] = ["claudeCode"];

/// Top-level `_meta` flags the adapter sends only when they change.
const FLAGS: [&str; 2] = ["is_mcp_tool_call", "terminal_info"];

/// What the Host last saw of one tool call.
#[derive(Debug, Default)]
struct Frame {
    /// Merged `_meta` keys: namespace (or `jetbrains.air`) → key → value, and
    /// the flags under their own names.
    meta: Map<String, Value>,
    kind: Option<Value>,
    content: Option<Value>,
}

#[derive(Debug, Default)]
pub struct ClaudeToolFrames {
    frames: HashMap<String, Frame>,
    order: VecDeque<String>,
}

pub type SharedClaudeToolFrames = Arc<Mutex<ClaudeToolFrames>>;

impl ClaudeToolFrames {
    pub fn shared() -> SharedClaudeToolFrames {
        Arc::new(Mutex::new(Self::default()))
    }

    /// Records a `session/update` notification's tool call frame and puts
    /// the trimmed `_meta` keys back on an update. Any other update is left
    /// as it is.
    pub fn apply(&mut self, params: &mut Value) {
        let Some(update) = params.get_mut("update").and_then(Value::as_object_mut) else {
            return;
        };
        let fresh = match update.get("sessionUpdate").and_then(Value::as_str) {
            Some("tool_call") => true,
            Some("tool_call_update") => false,
            _ => return,
        };
        let Some(id) = update
            .get("toolCallId")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return;
        };
        let frame = self.frame(id, fresh);
        if let Some(kind) = update.get("kind").filter(|k| k.is_string()) {
            frame.kind = Some(kind.clone());
        }
        if let Some(content) = update.get("content").filter(|c| c.is_array()) {
            frame.content = Some(content.clone());
        }
        let mut meta = match update.remove("_meta") {
            Some(Value::Object(meta)) => meta,
            Some(other) => {
                update.insert("_meta".into(), other);
                return;
            }
            None => Map::new(),
        };
        record(&mut frame.meta, &meta);
        if !fresh {
            restore(&frame.meta, &mut meta);
        }
        if !meta.is_empty() {
            update.insert("_meta".into(), Value::Object(meta));
        }
    }

    /// Gives a permission request's tool call the `kind` and `content` its
    /// tool call last carried, where the request left them out.
    pub fn fill_permission(&self, tool_call: &mut ToolCallUpdate) {
        let Some(frame) = self.frames.get(&tool_call.tool_call_id.to_string()) else {
            return;
        };
        let fields = &mut tool_call.fields;
        if fields.kind.is_none() {
            fields.kind = frame
                .kind
                .clone()
                .and_then(|k| serde_json::from_value::<ToolKind>(k).ok());
        }
        if fields.content.is_none() {
            fields.content = frame
                .content
                .clone()
                .and_then(|c| serde_json::from_value::<Vec<ToolCallContent>>(c).ok());
        }
    }

    /// The tool call's frame: a new one for a fresh `tool_call`, else the one
    /// recorded so far. Forgets the oldest past [`TOOL_CALL_LIMIT`].
    fn frame(&mut self, id: String, fresh: bool) -> &mut Frame {
        if !self.frames.contains_key(&id) {
            self.order.push_back(id.clone());
            if self.order.len() > TOOL_CALL_LIMIT {
                if let Some(oldest) = self.order.pop_front() {
                    self.frames.remove(&oldest);
                }
            }
        }
        let frame = self.frames.entry(id).or_default();
        if fresh {
            *frame = Frame::default();
        }
        frame
    }
}

/// Copies the merged keys of `meta` into `held`, the newest value winning.
fn record(held: &mut Map<String, Value>, meta: &Map<String, Value>) {
    for namespace in MERGED_NAMESPACES {
        if let Some(values) = meta.get(namespace).and_then(Value::as_object) {
            merge_into(held, namespace, values);
        }
    }
    if let Some(air) = meta
        .get("jetbrains")
        .and_then(|j| j.get("air"))
        .and_then(Value::as_object)
    {
        merge_into(held, "jetbrains.air", air);
    }
    for flag in FLAGS {
        if let Some(value) = meta.get(flag) {
            held.insert(flag.into(), value.clone());
        }
    }
}

fn merge_into(held: &mut Map<String, Value>, slot: &str, values: &Map<String, Value>) {
    let Value::Object(slot) = held
        .entry(slot)
        .or_insert_with(|| Value::Object(Map::new()))
    else {
        return;
    };
    for (key, value) in values {
        slot.insert(key.clone(), value.clone());
    }
}

/// Puts the held keys back on an update's `meta` wherever it lacks them.
fn restore(held: &Map<String, Value>, meta: &mut Map<String, Value>) {
    for namespace in MERGED_NAMESPACES {
        if let Some(values) = held.get(namespace).and_then(Value::as_object) {
            fill_object(meta, namespace, values);
        }
    }
    if let Some(air) = held.get("jetbrains.air").and_then(Value::as_object) {
        let jetbrains = meta
            .entry("jetbrains")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(jetbrains) = jetbrains {
            fill_object(jetbrains, "air", air);
        }
    }
    for flag in FLAGS {
        if let Some(value) = held.get(flag) {
            meta.entry(flag).or_insert_with(|| value.clone());
        }
    }
}

fn fill_object(parent: &mut Map<String, Value>, key: &str, values: &Map<String, Value>) {
    let Value::Object(target) = parent
        .entry(key)
        .or_insert_with(|| Value::Object(Map::new()))
    else {
        return;
    };
    for (k, v) in values {
        target.entry(k.clone()).or_insert_with(|| v.clone());
    }
}

/// The handler-chain entry feeding [`ClaudeToolFrames`]; inert unless
/// `enabled` (Claude connections only).
pub struct ClaudeToolFramesHandler {
    frames: SharedClaudeToolFrames,
    enabled: bool,
}

impl ClaudeToolFramesHandler {
    pub fn new(frames: SharedClaudeToolFrames, enabled: bool) -> Self {
        Self { frames, enabled }
    }
}

impl sacp::HandleDispatchFrom<Agent> for ClaudeToolFramesHandler {
    async fn handle_dispatch_from(
        &mut self,
        mut message: Dispatch,
        _connection: ConnectionTo<Agent>,
    ) -> Result<Handled<Dispatch>, sacp::Error> {
        if self.enabled {
            if let Dispatch::Notification(msg) = &mut message {
                if msg.method == "session/update" {
                    self.frames
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .apply(&mut msg.params);
                }
            }
        }
        Ok(Handled::No {
            message,
            retry: false,
        })
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "ClaudeToolFrames"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sacp::schema::ToolCallUpdateFields;
    use serde_json::json;

    fn notif(update: Value) -> Value {
        json!({ "sessionId": "sess-1", "update": update })
    }

    fn applied(frames: &mut ClaudeToolFrames, update: Value) -> Value {
        let mut params = notif(update);
        frames.apply(&mut params);
        params["update"].clone()
    }

    /// A 0.99.0 sub-agent child: the parent and tool name ride the
    /// `tool_call` only; its updates carry just what changed.
    #[test]
    fn a_trimmed_update_gets_its_meta_back() {
        let mut frames = ClaudeToolFrames::default();
        applied(
            &mut frames,
            json!({
                "sessionUpdate": "tool_call",
                "toolCallId": "toolu_child",
                "title": "Read",
                "kind": "read",
                "_meta": {
                    "claudeCode": { "toolName": "Read", "parentToolUseId": "toolu_agent" },
                    "jetbrains": { "air": { "subagent": false } },
                    "is_mcp_tool_call": false
                }
            }),
        );
        let update = applied(
            &mut frames,
            json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "toolu_child",
                "status": "completed",
                "_meta": { "claudeCode": { "toolResponse": { "ok": true } } }
            }),
        );
        assert_eq!(
            update["_meta"],
            json!({
                "claudeCode": {
                    "toolName": "Read",
                    "parentToolUseId": "toolu_agent",
                    "toolResponse": { "ok": true }
                },
                "jetbrains": { "air": { "subagent": false } },
                "is_mcp_tool_call": false
            })
        );
        // An update the filter emptied of `_meta` gets it all back.
        let update = applied(
            &mut frames,
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": "toolu_child", "title": "Read a.rs" }),
        );
        assert_eq!(
            update["_meta"]["claudeCode"]["parentToolUseId"],
            "toolu_agent"
        );
        assert_eq!(
            update["_meta"]["claudeCode"]["toolResponse"],
            json!({ "ok": true })
        );
        assert_eq!(update["title"], "Read a.rs");
    }

    #[test]
    fn the_update_value_wins_and_untracked_keys_pass_through() {
        let mut frames = ClaudeToolFrames::default();
        applied(
            &mut frames,
            json!({
                "sessionUpdate": "tool_call",
                "toolCallId": "t1",
                "_meta": { "claudeCode": { "toolName": "Bash" }, "terminal_info": { "terminal_id": "t1" } }
            }),
        );
        let update = applied(
            &mut frames,
            json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "t1",
                "_meta": {
                    "claudeCode": { "toolName": "BashOutput" },
                    "terminal_output": { "data": "hi" }
                }
            }),
        );
        assert_eq!(
            update["_meta"],
            json!({
                "claudeCode": { "toolName": "BashOutput" },
                "terminal_info": { "terminal_id": "t1" },
                "terminal_output": { "data": "hi" }
            })
        );
        // `terminal_output` is appended data, never carried forward.
        let update = applied(
            &mut frames,
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed" }),
        );
        assert!(update["_meta"].get("terminal_output").is_none());
        assert_eq!(update["_meta"]["claudeCode"]["toolName"], "BashOutput");
    }

    /// 0.81.2 sent every key on every update: nothing changes.
    #[test]
    fn full_meta_frames_pass_unchanged() {
        let mut frames = ClaudeToolFrames::default();
        let meta = json!({ "claudeCode": { "toolName": "Agent" } });
        applied(
            &mut frames,
            json!({ "sessionUpdate": "tool_call", "toolCallId": "a", "_meta": meta }),
        );
        let update = json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "a",
            "status": "completed",
            "_meta": { "claudeCode": { "toolName": "Agent", "toolResponse": { "totalTokens": 5 } } }
        });
        assert_eq!(applied(&mut frames, update.clone()), update);
        // A fresh `tool_call` for the same id starts over.
        let update = applied(
            &mut frames,
            json!({ "sessionUpdate": "tool_call", "toolCallId": "a", "title": "again" }),
        );
        assert!(update.get("_meta").is_none());
        // Other updates and unknown tool calls are untouched.
        let chunk = json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "x" } });
        assert_eq!(applied(&mut frames, chunk.clone()), chunk);
        let unknown =
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": "zzz", "status": "failed" });
        assert_eq!(applied(&mut frames, unknown.clone()), unknown);
    }

    /// 0.99.0's Edit permission to an AIR client: no kind, no content; the
    /// diff is on the tool call's own frames.
    #[test]
    fn a_permission_gets_kind_and_diff_from_its_tool_call() {
        let mut frames = ClaudeToolFrames::default();
        applied(
            &mut frames,
            json!({ "sessionUpdate": "tool_call", "toolCallId": "toolu_edit", "title": "Edit", "kind": "edit" }),
        );
        applied(
            &mut frames,
            json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "toolu_edit",
                "content": [{ "type": "diff", "path": "/w/a.rs", "oldText": "a", "newText": "b" }]
            }),
        );
        let mut tool_call = ToolCallUpdate::new(
            "toolu_edit",
            ToolCallUpdateFields::new()
                .title("Edit a.rs".to_string())
                .raw_input(json!({ "file_path": "/w/a.rs" })),
        );
        frames.fill_permission(&mut tool_call);
        let value = serde_json::to_value(&tool_call).unwrap();
        assert_eq!(value["kind"], "edit");
        assert_eq!(
            value["content"],
            json!([{ "type": "diff", "path": "/w/a.rs", "oldText": "a", "newText": "b" }])
        );
        assert_eq!(value["title"], "Edit a.rs");

        // What the request already carries is kept; unknown ids are left alone.
        let mut own = ToolCallUpdate::new(
            "toolu_edit",
            ToolCallUpdateFields::new()
                .kind(ToolKind::Execute)
                .content(vec![ToolCallContent::from("own")]),
        );
        frames.fill_permission(&mut own);
        let value = serde_json::to_value(&own).unwrap();
        assert_eq!(value["kind"], "execute");
        assert_eq!(value["content"][0]["content"]["text"], "own");
        let mut unknown = ToolCallUpdate::new("nope", ToolCallUpdateFields::new());
        frames.fill_permission(&mut unknown);
        assert!(unknown.fields.kind.is_none() && unknown.fields.content.is_none());
    }

    #[test]
    fn the_oldest_tool_call_is_forgotten_past_the_limit() {
        let mut frames = ClaudeToolFrames::default();
        for i in 0..=TOOL_CALL_LIMIT {
            applied(
                &mut frames,
                json!({ "sessionUpdate": "tool_call", "toolCallId": format!("t{i}"), "kind": "read" }),
            );
        }
        assert_eq!(frames.frames.len(), TOOL_CALL_LIMIT);
        assert!(!frames.frames.contains_key("t0"));
        assert!(frames.frames.contains_key(&format!("t{TOOL_CALL_LIMIT}")));
    }
}
