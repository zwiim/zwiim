//! Codex sub-agents, linked into the session they were launched from.
//!
//! Once the Host advertises `nativeSubagentSessions` (see
//! `build_client_capabilities`), codex-acp runs each sub-agent as a session
//! of its own:
//!
//! 1. On the parent session it announces the child with a
//!    `subagent_spawned { subagentSessionId, name, task }` update. Nothing in
//!    it names a tool call; there is no launch tool call on the parent at
//!    all.
//! 2. The child's own updates (tool calls, text, thinking, usage, status)
//!    arrive under the child's session id.
//! 3. On the parent session, `subagent_state_update { subagentSessionId,
//!    state }` reports how the child ended.
//!
//! The Host turns that into the one shape every client already reads for a
//! sub-agent: a launch tool call on the parent session (the anchor), with
//! everything the child does carried under it by `parent_id`. The anchor id
//! is opaque: a salted hash of the child session id, so the agent's own
//! thread id never reaches a client. It stays the same for the life of the
//! connection, so snapshots and reconnects keep it.
//!
//! Two steps, both driven by one [`CodexChildRouter`]:
//!
//! 1. [`CodexChildReroute`] sits first in the connection's handler chain. It
//!    sees every `session/update` before any session claims it, rewrites a
//!    known child's session id to its root session and stamps the update
//!    with the anchor (`_meta["zwiim/parentId"]`). The child's frames then
//!    reach the root session's reader like its own.
//! 2. [`CodexChildRouter::claim`] runs in that reader before the typed
//!    pipeline. It answers the two announcements (which the typed pipeline
//!    cannot read) with the anchor's own events, swallows the child's
//!    session-level updates (its usage and status are not the root's) and
//!    hands everything else on.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use sacp::{Agent, ConnectionTo, Dispatch, Handled};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::acp::types::AcpEvent;

/// Update-level metadata key naming the launch a rerouted child's update
/// belongs to. Written only by the Host; an agent-sent copy is removed.
pub(crate) const PARENT_ID_META: &str = "zwiim/parentId";

/// Metadata key marking the Host's launch tool call for a Codex sub-agent;
/// on its settling update it carries `durationMs`.
pub(crate) const SUBAGENT_META: &str = "zwiim/subagent";

const ANCHOR_PREFIX: &str = "subagent-";

/// What the Host knows about one child session.
#[derive(Debug, Clone)]
pub struct ChildLink {
    /// The launch tool call the child's work is carried under.
    pub anchor_id: String,
    pub name: String,
    pub task: String,
    /// The session the child's frames are delivered on: the parent's, or,
    /// for a child of a child, the parent's own root.
    root_session: String,
    /// The anchor of the child that launched this one, if any.
    parent_anchor: Option<String>,
    started: Instant,
}

/// The child sessions of one connection, by child session id.
#[derive(Debug)]
pub struct CodexChildRouter {
    children: HashMap<String, ChildLink>,
    /// Random per connection: anchor ids cannot be reversed to a child
    /// session id, nor matched across connections.
    salt: [u8; 16],
}

impl Default for CodexChildRouter {
    fn default() -> Self {
        Self {
            children: HashMap::new(),
            salt: rand::random(),
        }
    }
}

/// One router, shared by the connection's handler and its session reader.
pub type SharedChildRouter = Arc<Mutex<CodexChildRouter>>;

impl CodexChildRouter {
    pub fn shared() -> SharedChildRouter {
        Arc::new(Mutex::new(Self::default()))
    }

    /// Record a child announced on `session`. The first announcement wins.
    fn register(&mut self, session: &str, child: &str, name: &str, task: &str) -> &ChildLink {
        let (root_session, parent_anchor) = match self.children.get(session) {
            Some(parent) => (parent.root_session.clone(), Some(parent.anchor_id.clone())),
            None => (session.to_string(), None),
        };
        let salt = self.salt;
        self.children
            .entry(child.to_string())
            .or_insert_with(|| ChildLink {
                anchor_id: anchor_id(&salt, child),
                name: name.to_string(),
                task: task.to_string(),
                root_session,
                parent_anchor,
                started: Instant::now(),
            })
    }

    /// Rewrite one `session/update`'s params in place: a known child's update
    /// moves to its root session, stamped with its anchor. Any
    /// `zwiim/parentId` or `zwiim/subagent` the agent itself wrote is
    /// removed first, so only the Host can link or mark a launch.
    pub fn reroute(&mut self, params: &mut Value) {
        let Some(update) = params.get_mut("update").and_then(Value::as_object_mut) else {
            return;
        };
        if let Some(meta) = update.get_mut("_meta").and_then(Value::as_object_mut) {
            meta.remove(PARENT_ID_META);
            meta.remove(SUBAGENT_META);
        }
        let Some(session) = params
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return;
        };
        let update = &params["update"];
        if update.get("sessionUpdate").and_then(Value::as_str) == Some("subagent_spawned") {
            if let Some(child) = text(update, "subagentSessionId") {
                let (name, task) = (
                    text(update, "name").unwrap_or_default(),
                    text(update, "task").unwrap_or_default(),
                );
                self.register(&session, &child, &name, &task);
            }
        }
        let Some(link) = self.children.get(&session) else {
            return;
        };
        let (root, anchor) = (link.root_session.clone(), link.anchor_id.clone());
        params["sessionId"] = Value::String(root);
        let update = params["update"].as_object_mut().expect("checked above");
        let meta = update.entry("_meta").or_insert_with(|| json!({}));
        if !meta.is_object() {
            *meta = json!({});
        }
        meta[PARENT_ID_META] = Value::String(anchor);
    }

    /// Read one `session/update` frame (the JSON-RPC message or its params)
    /// off the root session before the typed pipeline:
    ///
    /// 1. `subagent_spawned` → the anchor tool call.
    /// 2. `subagent_state_update` → the anchor's settling update; nothing
    ///    for a child the Host never saw announced.
    /// 3. A child's tool call, text or thinking → `None`: the typed pipeline
    ///    emits it, parented by its stamp.
    /// 4. A child's other updates (usage, status, title) → nothing.
    /// 5. Anything else → `None`.
    pub fn claim(&mut self, frame: &Value) -> Option<Vec<AcpEvent>> {
        let params = frame.get("params").unwrap_or(frame);
        let update = params.get("update")?;
        let kind = update.get("sessionUpdate")?.as_str()?;
        match kind {
            "subagent_spawned" => {
                let Some(child) = text(update, "subagentSessionId") else {
                    return Some(Vec::new());
                };
                let session = text(params, "sessionId").unwrap_or_default();
                let (name, task) = (
                    text(update, "name").unwrap_or_default(),
                    text(update, "task").unwrap_or_default(),
                );
                let link = self.register(&session, &child, &name, &task);
                Some(vec![anchor_tool_call(link)])
            }
            "subagent_state_update" => {
                let Some(link) =
                    text(update, "subagentSessionId").and_then(|c| self.children.get(&c))
                else {
                    return Some(Vec::new());
                };
                let status = match update.get("state").and_then(Value::as_str) {
                    Some("completed") => "completed",
                    Some("cancelled" | "failed" | "disconnected") => "failed",
                    _ => return Some(Vec::new()),
                };
                Some(vec![anchor_settled(link, status)])
            }
            _ if is_stamped(update) => {
                // Only the child's work reaches the typed pipeline, plus its
                // AIR `async_task` deltas: a background terminal the child
                // leaves running keeps the root session waiting just as one
                // of the root's own would, so it counts toward the root's
                // background state. Its other frames describe the child
                // session, not the root, so they are claimed and dropped:
                // 1. `plan`: the child's plan is not the root's plan strip.
                // 2. `usage_update`: the child's context use is not the
                //    root's.
                // 3. `session_info_update`, `current_mode_update`,
                //    `config_option_update`, `available_commands_update`:
                //    the child's title, mode, options and commands.
                // 4. `user_message_chunk` and anything unknown.
                match kind {
                    "tool_call"
                    | "tool_call_update"
                    | "agent_message_chunk"
                    | "agent_thought_chunk"
                    | "async_task_spawned"
                    | "async_task_progress"
                    | "async_task_state_update" => None,
                    _ => Some(Vec::new()),
                }
            }
            _ => None,
        }
    }
}

/// `subagent-` and the first 16 hex digits of SHA-256 over the salt and the
/// child session id.
fn anchor_id(salt: &[u8; 16], child: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(salt);
    hash.update(child.as_bytes());
    let digest = hash.finalize();
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("{ANCHOR_PREFIX}{hex}")
}

/// Whether an update carries the Host's link to a launch, i.e. it is a
/// rerouted child's.
fn is_stamped(update: &Value) -> bool {
    update
        .get("_meta")
        .and_then(|meta| meta.get(PARENT_ID_META))
        .is_some()
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// The prompt a launch shows, or `None` when codex-acp sent none worth
/// showing: its stand-in "Delegated task for <name>" or an encrypted prompt.
fn shown_prompt(link: &ChildLink) -> Option<&str> {
    let task = link.task.trim();
    let stand_in = format!("Delegated task for {}", link.name);
    (!task.is_empty() && task != stand_in && !task.starts_with("gAAAAA")).then_some(task)
}

fn anchor_meta(link: &ChildLink, subagent: Value) -> Value {
    let mut meta = json!({ SUBAGENT_META: subagent });
    if let Some(parent) = &link.parent_anchor {
        meta[PARENT_ID_META] = Value::String(parent.clone());
    }
    meta
}

fn anchor_tool_call(link: &ChildLink) -> AcpEvent {
    let mut input = json!({ "description": link.name });
    if let Some(prompt) = shown_prompt(link) {
        input["prompt"] = Value::String(prompt.to_string());
    }
    AcpEvent::ToolCall {
        tool_call_id: link.anchor_id.clone(),
        title: link.name.clone(),
        kind: "agent".to_string(),
        status: "in_progress".to_string(),
        content: None,
        raw_input: Some(input.to_string()),
        raw_output: None,
        locations: None,
        meta: Some(anchor_meta(link, json!({}))),
        images: None,
    }
}

fn anchor_settled(link: &ChildLink, status: &str) -> AcpEvent {
    let duration_ms = u64::try_from(link.started.elapsed().as_millis()).unwrap_or(u64::MAX);
    AcpEvent::ToolCallUpdate {
        tool_call_id: link.anchor_id.clone(),
        title: None,
        status: Some(status.to_string()),
        content: None,
        raw_input: None,
        raw_output: None,
        raw_output_append: None,
        locations: None,
        meta: Some(anchor_meta(link, json!({ "durationMs": duration_ms }))),
        images: None,
    }
}

/// The session-level step for one frame a session's reader got: `claim` on a
/// `session/update` notification, `None` for anything else. The lock is
/// released before the caller emits.
pub fn claim_dispatch(router: &SharedChildRouter, dispatch: &Dispatch) -> Option<Vec<AcpEvent>> {
    let Dispatch::Notification(msg) = dispatch else {
        return None;
    };
    if msg.method != "session/update" {
        return None;
    }
    router
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .claim(&msg.params)
}

/// The `session/load` replay step: `true` when the frame is a Codex
/// sub-agent's and must be dropped. codex-acp replays each past sub-agent as
/// its announcement, the child's own frames (already rerouted and stamped)
/// and its end report. The link is still recorded, so a late live frame of
/// the same child is recognised, but nothing is emitted: the history parser
/// already shows those launches with their work, and a replayed anchor would
/// be an empty card with no prompt, no children and a duration of about 0.
pub fn consume_replayed(router: &SharedChildRouter, dispatch: &Dispatch) -> bool {
    let Dispatch::Notification(msg) = dispatch else {
        return false;
    };
    if msg.method != "session/update" {
        return false;
    }
    let stamped = msg.params.get("update").is_some_and(is_stamped);
    let claimed = router
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .claim(&msg.params)
        .is_some();
    claimed || stamped
}

/// The connection-level step: first in the handler chain, it reroutes every
/// `session/update` through the router and claims nothing. Rerouting runs
/// for Codex only; the removal of Host-owned keys runs for every agent.
pub struct CodexChildReroute {
    router: SharedChildRouter,
    enabled: bool,
}

impl CodexChildReroute {
    pub fn new(router: SharedChildRouter, enabled: bool) -> Self {
        Self { router, enabled }
    }

    fn apply(&self, params: &mut Value) {
        if self.enabled {
            self.router
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .reroute(params);
        } else if let Some(meta) = params
            .pointer_mut("/update/_meta")
            .and_then(Value::as_object_mut)
        {
            meta.remove(PARENT_ID_META);
            meta.remove(SUBAGENT_META);
        }
    }
}

impl sacp::HandleDispatchFrom<Agent> for CodexChildReroute {
    async fn handle_dispatch_from(
        &mut self,
        mut message: Dispatch,
        _connection: ConnectionTo<Agent>,
    ) -> Result<Handled<Dispatch>, sacp::Error> {
        if let Dispatch::Notification(msg) = &mut message {
            if msg.method == "session/update" {
                self.apply(&mut msg.params);
            }
        }
        Ok(Handled::No {
            message,
            retry: false,
        })
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "CodexChildReroute"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::connection::{
        air_async_task_delta, emit_conversation_update, CodeBuddyLiveState, ToolCallOutputCache,
    };
    use crate::acp::session_state::SessionState;
    use crate::models::agent::AgentType;
    use crate::protocol::events::tests::assert_no_internal_keys;
    use crate::protocol::events::{project_envelope, project_snapshot};
    use crate::web::event_bridge::{emit_with_state, EventEmitter};
    use sacp::schema::SessionUpdate;
    use tokio::sync::RwLock;

    /// Recorded live on 2026-10-19 from codex-acp 1.13.1 over codex 0.156.1,
    /// with the Host advertising `nativeSubagentSessions`: one turn whose
    /// root launches one sub-agent that runs `ls`. Every frame the agent sent
    /// on the connection, in order. Scrubbed: paths are `/work/repo`, no
    /// account, email or key data.
    const LIVE: &str = include_str!("testdata/codex_subagent_live.jsonl");
    const ROOT: &str = "01a1521b-a481-7ec3-b772-6670c440ba4b";
    const CHILD: &str = "01a1521b-b7e5-7d62-aa83-c5fd5a5ea422";

    fn anchor_of(router: &CodexChildRouter, child: &str) -> String {
        router.children[child].anchor_id.clone()
    }

    fn frames() -> Vec<Value> {
        LIVE.lines()
            .map(|l| serde_json::from_str(l).expect("fixture line"))
            .collect()
    }

    /// The live fixture as the root session's reader sees it: every frame
    /// through the handler, then the root's frames through `claim`, then the
    /// unclaimed ones through the typed pipeline. Returns the projected wire
    /// events and the final snapshot.
    async fn drive(frames: Vec<Value>) -> (Vec<Value>, Value, String) {
        drive_as(frames, false).await
    }

    /// `drive`, or with `replay` the `session/load` drain's order: a frame
    /// `consume_replayed` takes is dropped, and every other frame still goes
    /// through the live path, `claim` and the typed pipeline (more than the
    /// drain lets through), so anything of the sub-agent's that slipped past
    /// would show.
    async fn drive_as(frames: Vec<Value>, replay: bool) -> (Vec<Value>, Value, String) {
        let router = CodexChildRouter::shared();
        let state = Arc::new(RwLock::new(SessionState::new(
            "conn-s6".to_string(),
            AgentType::Codex,
            None,
            "win".to_string(),
            None,
        )));
        feed(&state, &router, frames, replay).await;
        let s = state.read().await;
        let wire = s
            .recent_events_after(0)
            .expect("events")
            .iter()
            .filter_map(|env| project_envelope("s-1", Some("/work/repo"), env))
            .map(|e| serde_json::to_value(e).unwrap())
            .collect();
        let anchor = anchor_of(&router.lock().unwrap(), CHILD);
        (
            wire,
            project_snapshot(&s.to_snapshot(), Some("/work/repo")),
            anchor,
        )
    }

    /// The reader's steps for each frame, into `state`.
    async fn feed(
        state: &Arc<RwLock<SessionState>>,
        router: &SharedChildRouter,
        frames: Vec<Value>,
        replay: bool,
    ) {
        let handler = CodexChildReroute::new(Arc::clone(router), true);
        let emitter = EventEmitter::Noop;
        let mut cache = ToolCallOutputCache::default();
        let mut cb = CodeBuddyLiveState::default();
        for mut frame in frames {
            handler.apply(&mut frame["params"]);
            if frame["params"]["sessionId"] != ROOT {
                continue; // no session of this connection claims it
            }
            if replay {
                let dispatch = Dispatch::Notification(
                    sacp::UntypedMessage::new("session/update", frame["params"].clone()).unwrap(),
                );
                if consume_replayed(router, &dispatch) {
                    continue;
                }
            }
            let claimed = router.lock().unwrap().claim(&frame);
            let dispatch = Dispatch::Notification(
                sacp::UntypedMessage::new("session/update", frame["params"].clone()).unwrap(),
            );
            match claimed {
                Some(events) => {
                    for event in events {
                        emit_with_state(state, &emitter, event).await;
                    }
                }
                None if air_async_task_delta(&dispatch).is_some() => {
                    let delta = air_async_task_delta(&dispatch).unwrap();
                    emit_with_state(state, &emitter, AcpEvent::AsyncTask { delta }).await;
                }
                None => {
                    let Ok(update) =
                        serde_json::from_value::<SessionUpdate>(frame["params"]["update"].clone())
                    else {
                        continue;
                    };
                    emit_conversation_update(
                        state,
                        &emitter,
                        AgentType::Codex,
                        update,
                        None,
                        &mut cache,
                        &mut cb,
                    )
                    .await;
                }
            }
        }
    }

    fn of_type<'a>(wire: &'a [Value], kind: &str) -> Vec<&'a Value> {
        wire.iter().filter(|e| e["type"] == kind).collect()
    }

    #[test]
    fn codex_spawn_emits_anchor_tool_call() {
        let mut router = CodexChildRouter::default();
        let spawn = &frames()[1];
        let events = router.claim(spawn).expect("claimed");
        let [AcpEvent::ToolCall {
            tool_call_id,
            title,
            kind,
            status,
            raw_input,
            meta,
            ..
        }] = events.as_slice()
        else {
            panic!("one anchor tool call, got {events:?}");
        };
        let anchor = anchor_of(&router, CHILD);
        assert_eq!(tool_call_id, &anchor);
        assert_eq!(title, "List files");
        assert_eq!(kind, "agent");
        assert_eq!(status, "in_progress");
        // codex-acp's stand-in task is not the prompt the child got.
        let input: Value = serde_json::from_str(raw_input.as_deref().unwrap()).unwrap();
        assert_eq!(input, json!({ "description": "List files" }));
        assert_eq!(meta.as_ref().unwrap(), &json!({ SUBAGENT_META: {} }));
        let link = &router.children[CHILD];
        assert_eq!(link.name, "List files");
        assert_eq!(link.task, "Delegated task for List files");

        // The anchor id is opaque: no part of the child session id is in
        // it, it is the same for the connection's life, and another
        // connection mints another one for the same child.
        assert!(anchor.starts_with("subagent-"));
        assert_eq!(anchor.len(), "subagent-".len() + 16);
        for part in CHILD.split('-') {
            assert!(!anchor.contains(part), "{part} in {anchor}");
        }
        let again = router.claim(spawn).unwrap_or_default();
        let [AcpEvent::ToolCall { tool_call_id, .. }] = again.as_slice() else {
            panic!("anchor");
        };
        assert_eq!(tool_call_id, &anchor);
        let mut other = CodexChildRouter::default();
        other.claim(spawn);
        assert_ne!(anchor_of(&other, CHILD), anchor);

        // A real prompt is kept; an encrypted one is not.
        let mut real = spawn.clone();
        real["params"]["update"]["subagentSessionId"] = json!("c-2");
        real["params"]["update"]["task"] = json!("List the files");
        let events = router.claim(&real).unwrap_or_default();
        let [AcpEvent::ToolCall { raw_input, .. }] = events.as_slice() else {
            panic!("anchor");
        };
        assert!(raw_input
            .as_deref()
            .unwrap()
            .contains("\"prompt\":\"List the files\""));
        real["params"]["update"]["subagentSessionId"] = json!("c-3");
        real["params"]["update"]["task"] = json!("gAAAAABpZ0xxx");
        let events = router.claim(&real).unwrap_or_default();
        let [AcpEvent::ToolCall { raw_input, .. }] = events.as_slice() else {
            panic!("anchor");
        };
        assert!(!raw_input.as_deref().unwrap().contains("prompt"));
    }

    #[tokio::test]
    async fn codex_child_updates_parented_to_anchor() {
        let (wire, snapshot, anchor_id) = drive(frames()).await;
        let anchor = of_type(&wire, "agent.tool_call")
            .into_iter()
            .find(|e| e["payload"]["tool_call_id"] == anchor_id.as_str());
        let anchor = anchor.expect("anchor on the wire");
        assert_eq!(anchor["payload"]["kind"], "agent");
        assert_eq!(anchor["payload"]["title"], "List files");
        assert_eq!(anchor["payload"]["subagent"]["description"], "List files");
        assert!(anchor["payload"].get("parent_id").is_none());
        let anchor_id = anchor_id.as_str();

        // The child's tool call and its result, under the anchor.
        let child_call = of_type(&wire, "agent.tool_call")
            .into_iter()
            .find(|e| e["payload"]["tool_call_id"] == "exec-3a8ce7c4-6b98-4dbe-8ed8-1dc94dda3ca5")
            .expect("child tool call");
        assert_eq!(child_call["payload"]["parent_id"], anchor_id);
        let child_done = of_type(&wire, "agent.tool_call_update")
            .into_iter()
            .find(|e| e["payload"]["tool_call_id"] == "exec-3a8ce7c4-6b98-4dbe-8ed8-1dc94dda3ca5")
            .expect("child tool call update");
        assert_eq!(child_done["payload"]["parent_id"], anchor_id);

        // The child's text is its own, never the reply's.
        let child_text: String = of_type(&wire, "agent.subagent_message_delta")
            .iter()
            .inspect(|e| assert_eq!(e["payload"]["parent_id"], anchor_id))
            .map(|e| e["payload"]["text"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(child_text, "`ls` shows `a.txt` and `b.txt`.");
        let reply: String = of_type(&wire, "agent.message_delta")
            .iter()
            .map(|e| e["payload"]["text"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(reply, "`ls` found `a.txt` and `b.txt`.");

        // The child's usage never reaches the root's.
        let usage: Vec<&Value> = of_type(&wire, "session.usage");
        assert!(usage
            .iter()
            .all(|u| u["payload"]["used"] != 16844 && u["payload"]["used"] != 16890));

        // The snapshot groups the same blocks under the anchor.
        let groups = snapshot["live_message"]["subagents"]
            .as_array()
            .expect("subagents");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["parent_id"], anchor_id);
        let calls = snapshot["tool_calls"].as_array().unwrap();
        let snap_anchor = calls
            .iter()
            .find(|t| t["tool_call_id"] == anchor_id)
            .expect("anchor in snapshot");
        assert_eq!(snap_anchor["kind"], "agent");
    }

    #[tokio::test]
    async fn codex_child_completion_settles_anchor() {
        let mut router = CodexChildRouter::default();
        let all = frames();
        router.claim(&all[1]).expect("spawn");
        let events = router.claim(&all[23]).expect("claimed");
        let [AcpEvent::ToolCallUpdate {
            tool_call_id,
            status,
            meta,
            ..
        }] = events.as_slice()
        else {
            panic!("one settling update, got {events:?}");
        };
        assert_eq!(tool_call_id, &anchor_of(&router, CHILD));
        assert_eq!(status.as_deref(), Some("completed"));
        assert!(meta.as_ref().unwrap()[SUBAGENT_META]["durationMs"].is_u64());

        for (state, expected) in [
            ("cancelled", "failed"),
            ("failed", "failed"),
            ("disconnected", "failed"),
        ] {
            let mut ended = all[23].clone();
            ended["params"]["update"]["state"] = json!(state);
            let events = router.claim(&ended).unwrap_or_default();
            let [AcpEvent::ToolCallUpdate { status, .. }] = events.as_slice() else {
                panic!("{state}: settling update");
            };
            assert_eq!(status.as_deref(), Some(expected), "{state}");
        }
        let mut running = all[23].clone();
        running["params"]["update"]["state"] = json!("running");
        assert_eq!(router.claim(&running).map(|e| e.len()), Some(0));

        // On the wire: the anchor settles with its duration.
        let (wire, _, anchor) = drive(all).await;
        let settled = of_type(&wire, "agent.tool_call_update")
            .into_iter()
            .find(|e| e["payload"]["tool_call_id"] == anchor.as_str())
            .expect("anchor update");
        assert_eq!(settled["payload"]["status"], "completed");
        assert!(settled["payload"]["subagent"]["duration_ms"].is_u64());
    }

    #[test]
    fn unknown_child_session_ignored_not_crash() {
        let mut router = CodexChildRouter::default();
        let all = frames();
        // An end report for a child never announced: claimed, nothing emitted.
        assert_eq!(router.claim(&all[23]).map(|e| e.len()), Some(0));
        // A child's frame before (or without) its announcement stays on its
        // own session, untouched, for no session to claim.
        let mut orphan = all[5].clone();
        let before = orphan.clone();
        router.reroute(&mut orphan["params"]);
        assert_eq!(orphan, before);
        assert!(router.claim(&orphan).is_none());
        // Malformed announcements are claimed and dropped.
        let mut bad = all[1].clone();
        bad["params"]["update"]
            .as_object_mut()
            .unwrap()
            .remove("subagentSessionId");
        assert_eq!(router.claim(&bad).map(|e| e.len()), Some(0));
        router.reroute(&mut bad["params"]);
        assert!(router.children.is_empty());
        // Frames that are not session updates are not read.
        assert!(router
            .claim(&json!({ "params": { "sessionId": ROOT } }))
            .is_none());
        assert!(router.claim(&json!(null)).is_none());
    }

    #[test]
    fn reroute_moves_child_frames_to_root_and_strips_agent_stamps() {
        let mut router = CodexChildRouter::default();
        let all = frames();
        let mut spawn = all[1].clone();
        router.reroute(&mut spawn["params"]);
        assert_eq!(spawn, all[1], "the announcement itself stays on the parent");

        let mut chunk = all[8].clone();
        router.reroute(&mut chunk["params"]);
        assert_eq!(chunk["params"]["sessionId"], ROOT);
        let anchor = anchor_of(&router, CHILD);
        assert_eq!(chunk["params"]["update"]["_meta"][PARENT_ID_META], anchor);
        assert_eq!(
            chunk["params"]["update"]["_meta"]["codex"]["phase"],
            "final_answer"
        );

        // A grandchild announced by the child lands on the root, under the
        // child's anchor.
        let mut nested = all[1].clone();
        nested["params"]["sessionId"] = json!(CHILD);
        nested["params"]["update"]["subagentSessionId"] = json!("grandchild");
        router.reroute(&mut nested["params"]);
        assert_eq!(nested["params"]["sessionId"], ROOT);
        let events = router.claim(&nested).unwrap_or_default();
        let [AcpEvent::ToolCall {
            tool_call_id, meta, ..
        }] = events.as_slice()
        else {
            panic!("nested anchor");
        };
        let grandchild = anchor_of(&router, "grandchild");
        assert_eq!(tool_call_id, &grandchild);
        assert_ne!(grandchild, anchor);
        assert_eq!(meta.as_ref().unwrap()[PARENT_ID_META], anchor);
        let mut deep = all[5].clone();
        deep["params"]["sessionId"] = json!("grandchild");
        router.reroute(&mut deep["params"]);
        assert_eq!(deep["params"]["sessionId"], ROOT);
        assert_eq!(
            deep["params"]["update"]["_meta"][PARENT_ID_META],
            grandchild
        );

        // An agent cannot link or mark anything itself, Codex or not.
        let forged = json!({ "sessionId": ROOT, "update": {
            "sessionUpdate": "tool_call", "toolCallId": "t", "title": "x",
            "_meta": { PARENT_ID_META: "elsewhere", SUBAGENT_META: {} }
        }});
        let mut on_codex = forged.clone();
        router.reroute(&mut on_codex);
        assert_eq!(on_codex["update"]["_meta"], json!({}));
        let other = CodexChildReroute::new(CodexChildRouter::shared(), false);
        let mut on_other = forged.clone();
        other.apply(&mut on_other);
        assert_eq!(on_other["update"]["_meta"], json!({}));
        let mut child_on_other = all[5].clone();
        other.apply(&mut child_on_other["params"]);
        assert_eq!(child_on_other, all[5], "rerouting is Codex's only");
    }

    #[tokio::test]
    async fn no_internal_keys_codex_child() {
        let (wire, snapshot, _) = drive(frames()).await;
        assert!(!wire.is_empty());
        let words = [
            "zwiim/",
            "subagentSessionId",
            "subagent_spawned",
            "sessionUpdate",
            CHILD,
            // The child id's own tail, in case it is ever cut up.
            "c5fd5a5ea422",
        ];
        for event in &wire {
            assert_no_internal_keys(event);
            let text = event.to_string();
            for word in words {
                assert!(!text.contains(word), "`{word}` leaked in {text}");
            }
        }
        assert_no_internal_keys(&snapshot);
        let text = snapshot.to_string();
        for word in words {
            assert!(!text.contains(word), "`{word}` leaked in the snapshot");
        }
    }

    /// M8: a background terminal the child leaves running counts toward the
    /// root's background state, while the child's usage stays its own.
    #[tokio::test]
    async fn codex_child_async_task_reaches_root_background() {
        let mut all = frames();
        let spawned = json!({ "jsonrpc": "2.0", "method": "session/update", "params": {
            "sessionId": CHILD,
            "update": {
                "sessionUpdate": "async_task_spawned",
                "asyncTaskId": format!("{CHILD}:exec-77"),
                "name": "sleep 400",
                "taskType": "shell",
                "showInTranscript": false,
                "canStop": true,
                "toolCallId": "exec-77"
            }
        }});
        all.insert(5, spawned.clone());
        let (wire, snapshot, _) = drive(all.clone()).await;
        let background = &snapshot["background"];
        assert_eq!(background["outstanding"], 1, "{background}");
        assert_eq!(background["tasks"][0]["label"], "sleep 400");
        assert_eq!(background["tasks"][0]["state"], "running");
        assert_eq!(background["tasks"][0]["tool_call_id"], "exec-77");
        let changes = of_type(&wire, "session.background");
        assert!(!changes.is_empty(), "the root saw the child's task");
        for event in changes {
            assert_no_internal_keys(event);
            assert!(!event.to_string().contains("c5fd5a5ea422"));
        }
        assert!(of_type(&wire, "session.usage")
            .iter()
            .all(|u| u["payload"]["used"] != 16844 && u["payload"]["used"] != 16890));

        // The child's stop settles it on the root as well.
        let mut stopped = spawned;
        stopped["params"]["update"] = json!({
            "sessionUpdate": "async_task_state_update",
            "asyncTaskId": format!("{CHILD}:exec-77"),
            "state": "stopped",
            "toolCallId": "exec-77"
        });
        all.insert(6, stopped);
        let (_, snapshot, _) = drive(all).await;
        assert_eq!(snapshot["background"]["outstanding"], 0);
    }

    /// Review Focus 4, Codex-shaped: a client that drops mid-turn and misses
    /// more than a replay batch of the sub-agent's stream gets a snapshot
    /// holding the child's blocks under its launch, and the same history
    /// generation it had, since no turn completed.
    #[tokio::test]
    async fn attach_after_long_codex_subagent_stream_snapshots_live_blocks() {
        use crate::acp::internal_bus::EventBusMetrics;
        use crate::acp::manager::ConnectionManager;
        use crate::web::ws_attach::{handle_attach, ServerMsg, REPLAY_BATCH_THRESHOLD};

        let manager = ConnectionManager::new();
        manager
            .insert_test_connection("conn-rf4", AgentType::Codex, None, EventEmitter::Noop)
            .await;
        let state = manager.get_state("conn-rf4").await.unwrap();
        // Mid-turn: a turn is running.
        state.write().await.status = crate::acp::types::ConnectionStatus::Prompting;
        let router = CodexChildRouter::shared();
        let all = frames();
        // Up to the launch: the client saw this much before it dropped.
        feed(&state, &router, all[..2].to_vec(), false).await;
        let (cursor, generation) = {
            let s = state.read().await;
            (s.event_seq, s.to_snapshot().history_generation)
        };
        // The child's work, its reply streamed three times over, while the
        // turn is still running.
        let mut child: Vec<Value> = all[2..8].to_vec();
        for _ in 0..3 {
            child.extend_from_slice(&all[8..21]);
        }
        feed(&state, &router, child, false).await;
        let missed = state
            .read()
            .await
            .recent_events_after(cursor)
            .unwrap()
            .len();
        assert!(missed > REPLAY_BATCH_THRESHOLD, "only {missed} events");

        let outcome = handle_attach(
            &manager,
            &EventBusMetrics::default(),
            "sub".into(),
            "conn-rf4".into(),
            Some(cursor),
        )
        .await
        .ok()
        .expect("attached");
        let ServerMsg::Snapshot { snapshot, .. } = outcome.initial_msg else {
            panic!("a gap past the batch size snapshots");
        };
        assert_eq!(snapshot.history_generation, generation, "no turn ended");
        let anchor = anchor_of(&router.lock().unwrap(), CHILD);
        let wire = project_snapshot(&snapshot, Some("/work/repo"));
        assert_no_internal_keys(&wire);
        let groups = wire["live_message"]["subagents"]
            .as_array()
            .expect("subagents");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["parent_id"], anchor.as_str());
        let text: String = groups[0]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect();
        assert!(text.ends_with("`ls` shows `a.txt` and `b.txt`."), "{text}");
        let calls = wire["tool_calls"].as_array().unwrap();
        let child_call = calls
            .iter()
            .find(|t| t["tool_call_id"] == "exec-3a8ce7c4-6b98-4dbe-8ed8-1dc94dda3ca5")
            .expect("child call in the snapshot");
        assert_eq!(child_call["parent_id"], anchor.as_str());
        assert_eq!(wire["history_generation"], json!(generation));
    }

    /// I1: on `session/load` codex-acp replays each past sub-agent. Nothing
    /// of it reaches the live view, yet the link is kept, so the same
    /// child's later live frames still land under the same anchor.
    #[tokio::test]
    async fn resume_replay_emits_no_subagent_cards() {
        let (wire, snapshot, anchor) = drive_as(frames(), true).await;
        for event in &wire {
            assert!(
                event["payload"].get("parent_id").is_none(),
                "child event in a resume: {event}"
            );
            assert_ne!(event["payload"]["kind"], "agent", "anchor in a resume");
            assert_ne!(event["payload"]["tool_call_id"], anchor.as_str());
            assert!(!event["type"]
                .as_str()
                .is_some_and(|t| t.starts_with("agent.subagent_")));
        }
        assert!(!wire.iter().any(|e| e.to_string().contains("ls` shows")));
        let live = &snapshot["live_message"];
        assert!(live["subagents"]
            .as_array()
            .is_none_or(|groups| groups.is_empty()));
        let calls = snapshot["tool_calls"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(calls.iter().all(|t| t["kind"] != "agent"));

        // The link survives the drain: a later live frame of the child is
        // still carried under the same anchor.
        let router = CodexChildRouter::shared();
        let handler = CodexChildReroute::new(Arc::clone(&router), true);
        let all = frames();
        let mut spawn = all[1].clone();
        handler.apply(&mut spawn["params"]);
        let dispatch = Dispatch::Notification(
            sacp::UntypedMessage::new("session/update", spawn["params"].clone()).unwrap(),
        );
        assert!(consume_replayed(&router, &dispatch));
        let anchor = anchor_of(&router.lock().unwrap(), CHILD);
        let mut late = all[5].clone();
        handler.apply(&mut late["params"]);
        assert_eq!(late["params"]["sessionId"], ROOT);
        assert_eq!(late["params"]["update"]["_meta"][PARENT_ID_META], anchor);
        // Frames that are not the sub-agent's are left to the drain.
        let other = Dispatch::Notification(
            sacp::UntypedMessage::new("session/update", all[0]["params"].clone()).unwrap(),
        );
        assert!(!consume_replayed(&router, &other));
    }
}
