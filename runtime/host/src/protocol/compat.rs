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
pub const PROTOCOL_REVISION: u32 = 1;

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
    "host.diagnostics",
    "agent.auth",
    "workspace.publish",
    "attachments",
    "file_ranges",
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
    fn capabilities_are_unique_zwiim_names() {
        let mut seen = std::collections::HashSet::new();
        for c in CAPABILITIES {
            assert!(seen.insert(c), "duplicate capability {c}");
            assert!(!c.contains("acp"), "capabilities use Zwiim vocabulary");
        }
    }
}
