//! Client ↔ Host compatibility, carried by `host.hello`.
//!
//! Compatibility is never "same version". The Host speaks protocol
//! `zwiim/<major>` at an integer **revision**, bumped by every additive,
//! client-visible change, and serves clients down to `MIN_CLIENT_REVISION`.
//! Optional features are named **capabilities**: a client that misses one
//! is limited, not broken. The SDK makes the final call (`compat.ts`); the
//! Host's own verdict here only covers what it alone knows, its minimum.
//!
//! The hello body is the one request the Host reads leniently: a future
//! client must always be able to introduce itself, so unknown or malformed
//! fields never fail the handshake.

use serde::{Deserialize, Serialize};

/// Protocol revision this Host speaks. Bump on every additive change a
/// client can observe (new operation, event, field or capability).
///
/// Revision 2 (`session.revert`): the `session.revert` operation puts a
/// session's workspace files and staged changes back to what a user turn
/// started with (`files`; HEAD and ignored files are never touched). It is
/// refused with `turn_in_progress`, `workspace_busy` or
/// `checkpoint_unavailable`, and its result reports `files_restored`,
/// `chat_rewound` and `head_moved`. User turns in `session.history` carry
/// `revertible`. Older clients never call the operation and ignore the field.
///
/// Revision 3 (`session.revert.chat`): `session.revert` with `chat` also takes
/// the conversation back to the agent reply before the turn, keeping the
/// `session_id`; `chat_rewound` says whether it did. `session.options` replies
/// carry `can_rewind_chat` so a client knows in advance whether the agent can.
/// A first turn has no reply before it and is `checkpoint_unavailable`.
///
/// Revision 4 (`agent.subagents`): sub-agent work. A sub-agent's tool calls
/// carry `parent_id` (the tool call that launched it); its text and thinking
/// stream as `agent.subagent_message_delta` / `agent.subagent_thought_delta`
/// and sit in a snapshot's `live_message.subagents`, never in the reply's
/// own content, so an older client that ignores them shows the reply alone.
/// The launching tool call carries `subagent` (description, model, prompt,
/// duration), and on reload `subagent.blocks` holds what the sub-agent did.
///
/// Revision 5 (`session.background`): `session.background` events and a
/// snapshot `background` field report outstanding background tasks and when
/// the session started waiting on them; snapshots and `session.history`
/// carry `history_generation` so a client re-reads history only when it
/// changed. Older clients ignore the new event and fields.
///
/// Revision 6 (background outcome): in `session.history`, the result of a
/// launch that ran in the background carries `background`, how the task
/// ended (`completed`, `failed`, `stopped`, or null while the transcript does
/// not say), and its `output_preview` is the sub-agent's report. Older
/// clients ignore the field.
///
/// Revision 7 (sub-agent totals): a launch's `subagent` carries `tokens` and
/// `tool_uses`, what the sub-agent spent and how many tools it called, once
/// its result says so, live and in `session.history`. Older clients ignore
/// the fields.
///
/// Revision 8 (keep-awake feed): the Host feed publishes
/// `host.keep_awake_changed` with the `host.keep_awake.status` result
/// whenever it changes, and `host.snapshot` carries the latest as
/// `keep_awake`. Older clients ignore both.
///
/// Revision 9 (`github.auth`): `github.auth_status`, `github.sign_in` and
/// `github.cancel_sign_in` sign the Host's `gh` in to GitHub through GitHub's
/// device flow. Each returns the sign-in state (`signed_out`, `signing_in`
/// with the one-time `code` and the `url` to enter it at, `signed_in` with
/// the `login`, or `unavailable` when gh is missing) and, once after a failed
/// attempt, a Zwiim `error` sentence. Paired devices may call them. Older
/// clients never call the operations and keep telling the user to run
/// `gh auth login`.
///
/// Revision 10 (`subagent.started_at`): a launch's `subagent` carries
/// `started_at`, when the sub-agent started (RFC 3339, UTC): the Host's clock
/// when it first saw the launch, live, or the first entry of the sub-agent's
/// transcript in `session.history`. It is absent when the Host cannot tell.
/// A client ticks a running sub-agent's elapsed time from it. Older clients
/// ignore the field.
///
/// Revision 11 (`background.tokens`): a background task in
/// `session.background` carries `tokens` while it is a sub-agent the Host can
/// read: its context tokens so far, counted like the launch's finished
/// `subagent.tokens`. It is absent until the first reading, and the task
/// leaves the list when it finishes, so the finished total is never
/// overwritten. Older clients ignore the field.
///
/// Revision 12 (`workspace.conflicts`): the Host feed publishes
/// `workspace.conflicts_changed` with where a workspace's branch stands
/// against its base (`clean`, `conflicts` with the files a merge would
/// conflict in, or `in_progress` with the merge or rebase stopped on them),
/// or null when there is nothing to compare with; `host.snapshot` carries
/// the latest per workspace as `conflicts`. Older clients ignore both.
pub const PROTOCOL_REVISION: u32 = 12;

/// Oldest client revision this Host serves. Raise only after a release that
/// reported `client_deprecated`-era clients (see the R9 design, §5.5).
pub const MIN_CLIENT_REVISION: u32 = 1;

/// Optional features this Host serves, in Zwiim vocabulary. Clients ignore
/// names they do not know and degrade to "limited" for names they need but
/// do not find.
pub const CAPABILITIES: &[&str] = &[
    "workspace.browse",
    "workspace.worktrees",
    "workspace.continue",
    "files.write",
    "git",
    "git.pr",
    "terminal",
    "terminal.agent",
    "session.options",
    "session.effort_by_model",
    "session.usage",
    "session.commands",
    "host.lan_endpoint",
    "agent.setup",
    "host.update",
    "host.update.channel",
    "host.diagnostics",
    "agent.auth",
    "workspace.publish",
    "workspace.clone",
    "attachments",
    "file_ranges",
    "session.plan_hand_off",
    "session.revert",
    "session.revert.chat",
    "agent.subagents",
    "session.background",
    "github.auth",
    "subagent.started_at",
    "background.tokens",
    "workspace.conflicts",
];

/// What a client says about itself. Every field is optional so a partial or
/// newer introduction still parses.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ClientInfo {
    /// `desktop` | `mobile` | anything a future client calls itself.
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub build: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub revision: Option<u32>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct HelloParams {
    #[serde(default)]
    client: Option<ClientInfo>,
}

/// Parse a hello body without ever failing: empty, `{}`, malformed or
/// unknown-shaped bodies all mean "the client did not say".
pub fn parse_hello_client(body: &[u8]) -> Option<ClientInfo> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    serde_json::from_slice::<HelloParams>(body)
        .ok()
        .and_then(|p| p.client)
}

/// The Host's verdict for a declared client. `None` when the client did not
/// declare a revision (an older client): the SDK then decides on its own.
pub fn client_verdict(client: Option<&ClientInfo>) -> Option<&'static str> {
    let revision = client?.revision?;
    Some(if revision < MIN_CLIENT_REVISION {
        "update_required"
    } else {
        "compatible"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_body_is_lenient() {
        assert_eq!(parse_hello_client(b""), None);
        assert_eq!(parse_hello_client(b"  \n"), None);
        assert_eq!(parse_hello_client(b"{}"), None);
        assert_eq!(parse_hello_client(b"not json"), None);
        assert_eq!(parse_hello_client(br#"{"client": 5}"#), None);
        let c = parse_hello_client(
            br#"{"client":{"app":"mobile","version":"0.2.0","revision":1,
                "capabilities":["session.usage","from.the.future"],"new_field":true},
                "also_new":1}"#,
        )
        .unwrap();
        assert_eq!(c.app.as_deref(), Some("mobile"));
        assert_eq!(c.revision, Some(1));
        assert_eq!(c.capabilities.len(), 2);
    }

    #[test]
    fn verdict_follows_minimum_revision_only() {
        assert_eq!(client_verdict(None), None);
        let undeclared = ClientInfo::default();
        assert_eq!(client_verdict(Some(&undeclared)), None);
        let at_min = ClientInfo {
            revision: Some(MIN_CLIENT_REVISION),
            ..Default::default()
        };
        assert_eq!(client_verdict(Some(&at_min)), Some("compatible"));
        let newer = ClientInfo {
            revision: Some(PROTOCOL_REVISION + 5),
            ..Default::default()
        };
        assert_eq!(client_verdict(Some(&newer)), Some("compatible"));
        if MIN_CLIENT_REVISION > 0 {
            let old = ClientInfo {
                revision: Some(MIN_CLIENT_REVISION - 1),
                ..Default::default()
            };
            assert_eq!(client_verdict(Some(&old)), Some("update_required"));
        }
    }

    #[test]
    fn plan_hand_off_is_served() {
        assert!(CAPABILITIES.contains(&"session.plan_hand_off"));
    }

    #[test]
    fn revert_is_served() {
        assert!(CAPABILITIES.contains(&"session.revert"));
    }

    #[test]
    fn revert_chat_is_served() {
        assert!(CAPABILITIES.contains(&"session.revert.chat"));
    }

    #[test]
    fn subagents_are_served() {
        assert!(CAPABILITIES.contains(&"agent.subagents"));
        // Sub-agent events and fields are a revision bump.
        const _: () = assert!(PROTOCOL_REVISION >= 4);
    }

    #[test]
    fn background_is_served() {
        assert!(CAPABILITIES.contains(&"session.background"));
        const _: () = assert!(PROTOCOL_REVISION >= 5);
        // How a background launch ended is a field of revision 6.
        const _: () = assert!(PROTOCOL_REVISION >= 6);
    }

    #[test]
    fn subagent_totals_are_a_revision() {
        // A launch's `subagent.tokens` and `subagent.tool_uses`.
        const _: () = assert!(PROTOCOL_REVISION >= 7);
    }

    #[test]
    fn the_keep_awake_feed_is_a_revision() {
        // `host.keep_awake_changed` and the snapshot's `keep_awake`.
        const _: () = assert!(PROTOCOL_REVISION >= 8);
    }

    #[test]
    fn github_sign_in_is_served() {
        assert!(CAPABILITIES.contains(&"github.auth"));
        const _: () = assert!(PROTOCOL_REVISION >= 9);
    }

    #[test]
    fn subagent_start_is_served() {
        assert!(CAPABILITIES.contains(&"subagent.started_at"));
        const _: () = assert!(PROTOCOL_REVISION >= 10);
    }

    #[test]
    fn background_tokens_are_served() {
        assert!(CAPABILITIES.contains(&"background.tokens"));
        const _: () = assert!(PROTOCOL_REVISION >= 11);
    }

    #[test]
    fn conflicts_are_served() {
        assert!(CAPABILITIES.contains(&"workspace.conflicts"));
        // `workspace.conflicts_changed` and the snapshot's `conflicts`.
        assert_eq!(PROTOCOL_REVISION, 12);
    }

    #[test]
    fn capabilities_are_unique_zwiim_names() {
        let mut seen = std::collections::HashSet::new();
        for c in CAPABILITIES {
            assert!(seen.insert(c), "duplicate capability {c}");
            assert!(!c.contains("acp"), "capabilities use Zwiim vocabulary");
        }
    }
}
