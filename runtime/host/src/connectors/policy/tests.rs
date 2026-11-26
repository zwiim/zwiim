use chrono::{Duration, TimeZone, Utc};

use super::{decide, AllowedBy, AskReason, Decision, DenyReason, Facts, ToolState};
use crate::connectors::grants::{Fallback, Grant, Scope, ToolPolicy, ToolRow};

const HASH: &str = "sha256:now";

fn made() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 25, 12, 0, 0).unwrap()
}

fn grant(rows: &[(ToolPolicy, Option<&str>)]) -> Grant {
    Grant {
        id: 1,
        connector_id: "c1".into(),
        scope: Scope::default(),
        default_policy: Fallback::Deny,
        new_tools: Fallback::Ask,
        created_at: made(),
        tools: rows
            .iter()
            .map(|(policy, hash)| ToolRow {
                tool: "search".into(),
                policy: *policy,
                schema_hash: hash.map(Into::into),
            })
            .collect(),
    }
}

fn tool(first_seen_after_grant: bool) -> ToolState<'static> {
    let offset = if first_seen_after_grant { 1 } else { -1 };
    ToolState {
        name: "search",
        hash: HASH,
        first_seen_at: made() + Duration::hours(offset),
    }
}

fn facts(grant: Option<&Grant>) -> Facts<'_> {
    Facts {
        connector_active: true,
        engine_session: false,
        tool: Some(tool(false)),
        grant,
        session_allowed: None,
    }
}

#[test]
fn each_rule_in_order() {
    let allow = grant(&[(ToolPolicy::Allow, Some(HASH))]);
    let stale = grant(&[(ToolPolicy::Allow, Some("sha256:old"))]);
    let ask = grant(&[(ToolPolicy::Ask, None)]);
    let deny = grant(&[(ToolPolicy::Deny, None)]);
    let none = grant(&[]);

    let cases: Vec<(&str, Facts, Decision)> = vec![
        (
            "inactive beats an allow",
            Facts {
                connector_active: false,
                ..facts(Some(&allow))
            },
            Decision::Deny(DenyReason::Unavailable),
        ),
        (
            "engine session beats an allow",
            Facts {
                engine_session: true,
                ..facts(Some(&allow))
            },
            Decision::Deny(DenyReason::EngineSession),
        ),
        (
            "not exposed beats an allow",
            Facts {
                tool: None,
                ..facts(Some(&allow))
            },
            Decision::Deny(DenyReason::NotExposed),
        ),
        (
            "no grant beats a session allow",
            Facts {
                session_allowed: Some(HASH),
                ..facts(None)
            },
            Decision::Deny(DenyReason::NoGrant),
        ),
        (
            "a deny row beats a session allow",
            Facts {
                session_allowed: Some(HASH),
                ..facts(Some(&deny))
            },
            Decision::Deny(DenyReason::ToolRow),
        ),
        (
            "session allow at this hash",
            Facts {
                session_allowed: Some(HASH),
                ..facts(Some(&none))
            },
            Decision::Allow(AllowedBy::Session),
        ),
        (
            "session allow for an earlier version asks",
            Facts {
                session_allowed: Some("sha256:old"),
                ..facts(Some(&ask))
            },
            Decision::Ask(AskReason::ToolRow),
        ),
        (
            "allow row at this hash",
            facts(Some(&allow)),
            Decision::Allow(AllowedBy::ToolRow),
        ),
        (
            "allow row for a changed tool asks",
            facts(Some(&stale)),
            Decision::Ask(AskReason::ToolChanged),
        ),
        (
            "ask row",
            facts(Some(&ask)),
            Decision::Ask(AskReason::ToolRow),
        ),
        (
            "no row: default policy",
            facts(Some(&none)),
            Decision::Deny(DenyReason::Fallback),
        ),
        (
            "no row, first seen after the grant: new tools",
            Facts {
                tool: Some(tool(true)),
                ..facts(Some(&none))
            },
            Decision::Ask(AskReason::NewTool),
        ),
    ];
    for (what, facts, expected) in cases {
        assert_eq!(decide(facts), expected, "{what}");
    }
}

#[test]
fn fallbacks_never_allow() {
    for default_policy in [Fallback::Ask, Fallback::Deny] {
        for new_tools in [Fallback::Ask, Fallback::Deny] {
            let grant = Grant {
                default_policy,
                new_tools,
                ..grant(&[])
            };
            for new in [false, true] {
                let decision = decide(Facts {
                    tool: Some(tool(new)),
                    ..facts(Some(&grant))
                });
                assert!(!matches!(decision, Decision::Allow(_)), "{decision:?}");
            }
        }
    }
}
