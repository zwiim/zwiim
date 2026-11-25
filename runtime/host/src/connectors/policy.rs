//! Whether one tool call may run (connector design r3 §5.1). [`decide`] is
//! pure: it reads only the facts it is given, and only user-made rows can
//! allow. Annotations, descriptions, risk labels, agent mode and anything
//! a server or agent sends are never among them.
//!
//! Precedence, first match wins:
//! 1. Connection not active → deny.
//! 2. Engine session → deny (until playbooks declare connectors).
//! 3. Tool not exposed, or no longer listed → deny.
//! 4. No grant → deny.
//! 5. A `deny` row → deny.
//! 6. Allowed for this session, or an `allow` row, at the tool's current
//!    hash → allow.
//! 7. An `allow` row for another hash (the tool changed) → ask.
//! 8. An `ask` row → ask.
//! 9. The grant's `new_tools` for a tool first seen after it was made,
//!    else its `default_policy` → ask or deny.
//!
//! r3 checks "no grant" last; here it comes before session allows, so
//! removing a grant also ends what a session was allowed under it. With no
//! grant there are no rows, so nothing else moves.

use chrono::{DateTime, Utc};

use super::grants::{Fallback, Grant, ToolPolicy};

/// A tool as the session sees it now.
#[derive(Clone, Copy, Debug)]
pub struct ToolState<'a> {
    pub name: &'a str,
    pub hash: &'a str,
    pub first_seen_at: DateTime<Utc>,
}

/// Everything [`decide`] reads.
#[derive(Clone, Copy, Debug)]
pub struct Facts<'a> {
    pub connection_active: bool,
    pub engine_session: bool,
    /// `None` when the tool isn't exposed to the session or isn't listed.
    pub tool: Option<ToolState<'a>>,
    /// The grant that applies to the session (`grants::grant_for`).
    pub grant: Option<&'a Grant>,
    /// The hash this session already allowed the tool at, if it did.
    pub session_allowed: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow(AllowedBy),
    Ask(AskReason),
    Deny(DenyReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllowedBy {
    ToolRow,
    Session,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AskReason {
    ToolRow,
    /// An `allow` was given for an earlier version of the tool.
    ToolChanged,
    Fallback,
    NewTool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DenyReason {
    Unavailable,
    EngineSession,
    NotExposed,
    NoGrant,
    ToolRow,
    Fallback,
    NewTool,
}

impl DenyReason {
    /// The line for the agent; nothing was sent.
    pub fn line(self) -> &'static str {
        match self {
            Self::Unavailable => "This connection is unavailable; nothing was sent.",
            Self::EngineSession => "Automation sessions can't use connections; nothing was sent.",
            Self::NotExposed => "This tool isn't available in this session; nothing was sent.",
            Self::NoGrant => "This session isn't allowed to use this connection; nothing was sent.",
            Self::ToolRow | Self::Fallback | Self::NewTool => {
                "The user doesn't allow this tool here; nothing was sent."
            }
        }
    }
}

pub fn decide(facts: Facts<'_>) -> Decision {
    if !facts.connection_active {
        return Decision::Deny(DenyReason::Unavailable);
    }
    if facts.engine_session {
        return Decision::Deny(DenyReason::EngineSession);
    }
    let Some(tool) = facts.tool else {
        return Decision::Deny(DenyReason::NotExposed);
    };
    let Some(grant) = facts.grant else {
        return Decision::Deny(DenyReason::NoGrant);
    };
    let row = grant.row(tool.name);
    if row.is_some_and(|row| row.policy == ToolPolicy::Deny) {
        return Decision::Deny(DenyReason::ToolRow);
    }
    if facts.session_allowed == Some(tool.hash) {
        return Decision::Allow(AllowedBy::Session);
    }
    if let Some(row) = row {
        match row.policy {
            ToolPolicy::Allow if row.schema_hash.as_deref() == Some(tool.hash) => {
                return Decision::Allow(AllowedBy::ToolRow)
            }
            ToolPolicy::Allow => return Decision::Ask(AskReason::ToolChanged),
            ToolPolicy::Ask => return Decision::Ask(AskReason::ToolRow),
            ToolPolicy::Deny => unreachable!("denied above"),
        }
    }
    let new = tool.first_seen_at > grant.created_at;
    match (
        new,
        if new {
            grant.new_tools
        } else {
            grant.default_policy
        },
    ) {
        (true, Fallback::Ask) => Decision::Ask(AskReason::NewTool),
        (true, Fallback::Deny) => Decision::Deny(DenyReason::NewTool),
        (false, Fallback::Ask) => Decision::Ask(AskReason::Fallback),
        (false, Fallback::Deny) => Decision::Deny(DenyReason::Fallback),
    }
}

#[cfg(test)]
mod tests;
