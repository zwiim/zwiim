// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: the engine sessions that are live right now (automation
//! spec 5.6, 7.1.4, 6.2).
//!
//! The Host opens an entry when it launches an engine session and closes it
//! when the session ends. The session's one structured answer arrives through
//! `playbook_item_result`, checked here against the session's kind, and goes
//! to whoever launched it. Every tool call the session makes is counted, so
//! the launcher can hold the session to its limits.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Deserialize;
use tokio::sync::{oneshot, watch};

use crate::automation::lockdown::SessionKind;

/// What a session returned through `playbook_item_result`, by kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionReport {
    Item(ItemReport),
    /// Discovery: the candidates read off the list pages (spec 6.2).
    Discovery(Vec<Candidate>),
    /// A judgment batch: one decision per candidate (spec 6.2.3).
    Judgment(Vec<Judgment>),
}

/// An item session's end (spec 7.1.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemReport {
    pub status: ItemReportStatus,
    /// Tips the agent proposes; the person approves them first (spec 7.4).
    pub tips: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemReportStatus {
    /// A playbook rule says skip; `rule` quotes it.
    Skipped { rule: String },
    /// The agent could not do the item.
    Failed { note: String },
    /// The item continues on another site (spec 6.9).
    HandOff { url: String },
}

/// One item found by discovery.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Candidate {
    pub url: String,
    #[serde(default)]
    pub title: String,
    /// The list text the judgment batch reads.
    #[serde(default)]
    pub text: String,
}

/// A judgment batch's decision on one candidate, by its URL.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Judgment {
    pub url: String,
    pub keep: bool,
    /// The rule a skip follows.
    #[serde(default)]
    pub rule: Option<String>,
}

/// The `playbook_item_result` arguments as the agent sends them.
#[derive(Debug, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum WireReport {
    Skipped {
        rule: String,
        #[serde(default)]
        tips: Vec<String>,
    },
    Failed {
        note: String,
        #[serde(default)]
        tips: Vec<String>,
    },
    HandOff {
        url: String,
        #[serde(default)]
        tips: Vec<String>,
    },
    Listed {
        candidates: Vec<Candidate>,
    },
    Judged {
        decisions: Vec<Judgment>,
    },
}

/// The refusal for a result the session's kind does not take.
fn wrong_status(kind: SessionKind) -> String {
    let allowed = match kind {
        SessionKind::Item => "skipped, failed or hand_off",
        SessionKind::Discovery => "listed",
        SessionKind::JudgmentBatch => "judged",
        SessionKind::DraftBatch => "nothing in this build",
    };
    format!(
        "Refused: invalid_result; a {} session ends with status {allowed}.",
        kind.as_str()
    )
}

/// Read a result for a session of `kind`, or the refusal the agent is told.
pub fn parse_report(kind: SessionKind, args: &serde_json::Value) -> Result<SessionReport, String> {
    let wire: WireReport = serde_json::from_value(args.clone())
        .map_err(|e| format!("Refused: invalid_result; {e}. Fix the arguments and call again."))?;
    let item = |status, tips| SessionReport::Item(ItemReport { status, tips });
    match (kind, wire) {
        (SessionKind::Item, WireReport::Skipped { rule, tips }) => {
            Ok(item(ItemReportStatus::Skipped { rule }, tips))
        }
        (SessionKind::Item, WireReport::Failed { note, tips }) => {
            Ok(item(ItemReportStatus::Failed { note }, tips))
        }
        (SessionKind::Item, WireReport::HandOff { url, tips }) => {
            Ok(item(ItemReportStatus::HandOff { url }, tips))
        }
        (SessionKind::Discovery, WireReport::Listed { candidates }) => {
            Ok(SessionReport::Discovery(candidates))
        }
        (SessionKind::JudgmentBatch, WireReport::Judged { decisions }) => {
            Ok(SessionReport::Judgment(decisions))
        }
        _ => Err(wrong_status(kind)),
    }
}

/// What the launcher holds while its session runs.
pub struct SessionWatch {
    /// The session's one result.
    pub report: oneshot::Receiver<SessionReport>,
    /// Tool calls made so far.
    pub calls: watch::Receiver<u32>,
}

struct Live {
    kind: SessionKind,
    report: Option<oneshot::Sender<SessionReport>>,
    calls: watch::Sender<u32>,
}

/// The live engine sessions, by connection id.
#[derive(Default)]
pub struct EngineSessions {
    live: Mutex<HashMap<String, Live>>,
}

impl EngineSessions {
    fn live(&self) -> std::sync::MutexGuard<'_, HashMap<String, Live>> {
        self.live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Start tracking a session the Host launched.
    pub fn open(&self, connection_id: &str, kind: SessionKind) -> SessionWatch {
        let (report_tx, report) = oneshot::channel();
        let (calls_tx, calls) = watch::channel(0);
        self.live().insert(
            connection_id.to_string(),
            Live {
                kind,
                report: Some(report_tx),
                calls: calls_tx,
            },
        );
        SessionWatch { report, calls }
    }

    /// Stop tracking it; a later result or call is refused.
    pub fn close(&self, connection_id: &str) {
        self.live().remove(connection_id);
    }

    /// Count one tool call of the session.
    pub fn tool_called(&self, connection_id: &str) {
        if let Some(live) = self.live().get(connection_id) {
            live.calls.send_modify(|n| *n += 1);
        }
    }

    /// Take the session's result, once. The error is what the agent is told.
    pub fn report(&self, connection_id: &str, args: &serde_json::Value) -> Result<(), String> {
        let mut live = self.live();
        let Some(session) = live.get_mut(connection_id) else {
            return Err("Refused: no_session; this session has ended, so stop.".to_string());
        };
        let report = parse_report(session.kind, args)?;
        let Some(tx) = session.report.take() else {
            return Err(
                "Refused: already_reported; this session already returned its result, so stop."
                    .to_string(),
            );
        };
        // The launcher may have given up waiting (limits); the result is
        // then dropped, and the session is ending anyway.
        let _ = tx.send(report);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_kind_takes_only_its_own_result() {
        let skipped = json!({ "status": "skipped", "rule": "no relocation", "tips": ["t"] });
        assert_eq!(
            parse_report(SessionKind::Item, &skipped).unwrap(),
            SessionReport::Item(ItemReport {
                status: ItemReportStatus::Skipped {
                    rule: "no relocation".into()
                },
                tips: vec!["t".into()],
            })
        );
        let listed = json!({ "status": "listed", "candidates": [{ "url": "https://a.test/j/1" }] });
        assert_eq!(
            parse_report(SessionKind::Discovery, &listed).unwrap(),
            SessionReport::Discovery(vec![Candidate {
                url: "https://a.test/j/1".into(),
                title: String::new(),
                text: String::new(),
            }])
        );
        let judged = json!({ "status": "judged", "decisions": [{ "url": "u", "keep": false, "rule": "r" }] });
        assert!(matches!(
            parse_report(SessionKind::JudgmentBatch, &judged),
            Ok(SessionReport::Judgment(_))
        ));

        // Crossed kinds, unknown fields and draft batches are refused with
        // a line the agent can act on.
        for (kind, args) in [
            (SessionKind::Item, &listed),
            (SessionKind::Discovery, &skipped),
            (SessionKind::JudgmentBatch, &listed),
            (SessionKind::DraftBatch, &judged),
        ] {
            let refusal = parse_report(kind, args).unwrap_err();
            assert!(refusal.starts_with("Refused: invalid_result"), "{refusal}");
        }
        let extra = json!({ "status": "failed", "note": "n", "sent": true });
        assert!(parse_report(SessionKind::Item, &extra).is_err());
    }

    #[tokio::test]
    async fn a_result_reaches_the_launcher_once() {
        let sessions = EngineSessions::default();
        let watch = sessions.open("conn-1", SessionKind::Item);
        let failed = json!({ "status": "failed", "note": "no form" });
        sessions.report("conn-1", &failed).unwrap();
        let again = sessions.report("conn-1", &failed).unwrap_err();
        assert!(again.starts_with("Refused: already_reported"), "{again}");
        assert_eq!(
            watch.report.await.unwrap(),
            SessionReport::Item(ItemReport {
                status: ItemReportStatus::Failed {
                    note: "no form".into()
                },
                tips: vec![],
            })
        );
        // A bad result does not use up the one answer.
        let watch = sessions.open("conn-2", SessionKind::Discovery);
        assert!(sessions.report("conn-2", &failed).is_err());
        sessions
            .report("conn-2", &json!({ "status": "listed", "candidates": [] }))
            .unwrap();
        assert_eq!(
            watch.report.await.unwrap(),
            SessionReport::Discovery(vec![])
        );
    }

    #[test]
    fn calls_are_counted_until_the_session_closes() {
        let sessions = EngineSessions::default();
        let watch = sessions.open("conn-1", SessionKind::Item);
        sessions.tool_called("conn-1");
        sessions.tool_called("conn-1");
        sessions.tool_called("other");
        assert_eq!(*watch.calls.borrow(), 2);
        sessions.close("conn-1");
        sessions.tool_called("conn-1");
        assert_eq!(*watch.calls.borrow(), 2);
        let ended = sessions
            .report("conn-1", &json!({ "status": "failed", "note": "n" }))
            .unwrap_err();
        assert!(ended.starts_with("Refused: no_session"), "{ended}");
    }
}
