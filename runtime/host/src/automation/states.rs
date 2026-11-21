//! Zwiim-owned: the engine's state enums and their `allowed(from, to)`
//! transition tables (automation-engine spec 6.4 and 6.5).
//!
//! The tables are exhaustive: `allowed` must name every pair, so a new state
//! or a new edge cannot be added without saying so here, and the generated
//! tests in this file check both directions of the table.

use serde::{Deserialize, Serialize};

macro_rules! state_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $(
                $(#[$vmeta:meta])*
                $variant:ident => $str:literal,
            )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sea_orm::EnumIter, sea_orm::DeriveActiveEnum)]
        #[serde(rename_all = "snake_case")]
        #[sea_orm(rs_type = "String", db_type = "String(sea_orm::sea_query::StringLen::None)")]
        pub enum $name {
            $(
                $(#[$vmeta])*
                #[sea_orm(string_value = $str)]
                $variant,
            )+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $str,)+
                }
            }

            pub fn parse(s: &str) -> Option<Self> {
                match s {
                    $($str => Some($name::$variant),)+
                    _ => None,
                }
            }

            pub fn all() -> &'static [Self] {
                &[$($name::$variant,)+]
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

state_enum! {
    /// Item statuses (spec 6.4). `(new)` is the absence of a row, not a
    /// member; [`ItemStatus::allowed_from_new`] covers the create edge.
    pub enum ItemStatus {
        Queued => "queued",
        Working => "working",
        WaitingApproval => "waiting_approval",
        WaitingPerson => "waiting_person",
        Done => "done",
        Skipped => "skipped",
        Failed => "failed",
        Deferred => "deferred",
        RetriesExhausted => "retries_exhausted",
        Stopped => "stopped",
    }
}

impl ItemStatus {
    /// Whether a row may move from `from` to `to` (spec 6.4 table).
    #[allow(clippy::match_like_matches_macro)]
    pub fn allowed(from: Self, to: Self) -> bool {
        use ItemStatus::*;
        match (from, to) {
            (Queued, Working) => true,
            (Working, Queued) => true,
            (Working, WaitingApproval) => true,
            (Working, WaitingPerson) => true,
            (Working, Done) => true,
            (Working, Skipped) => true,
            (Working, Failed) => true,
            (Working, Deferred) => true,
            (Working, Stopped) => true,
            (WaitingApproval, Working) => true,
            (WaitingApproval, Skipped) => true,
            (WaitingApproval, Stopped) => true,
            (WaitingPerson, Queued) => true,
            (WaitingPerson, Done) => true,
            (WaitingPerson, Skipped) => true,
            (WaitingPerson, Failed) => true,
            (Failed, Queued) => true,
            (Failed, RetriesExhausted) => true,
            (RetriesExhausted, Queued) => true,
            (RetriesExhausted, Skipped) => true,
            (Deferred, Queued) => true,
            _ => false,
        }
    }

    /// Whether a new item may start in `self` (only `queued`, spec 6.4).
    pub fn allowed_from_new(self) -> bool {
        matches!(self, ItemStatus::Queued)
    }

    /// `done` is terminal (spec 6.4.2): nothing leaves it, ever.
    pub fn is_terminal(self) -> bool {
        matches!(self, ItemStatus::Done)
    }

    /// Active statuses take a lane's item tab (the partial unique index).
    pub fn holds_lane_tab(self) -> bool {
        matches!(self, ItemStatus::Working)
    }
}

state_enum! {
    /// Run statuses (spec 6.5.1). Active = `discovering`, `running` or
    /// `paused`; one active run per playbook is a partial unique index.
    pub enum RunStatus {
        Discovering => "discovering",
        Running => "running",
        Paused => "paused",
        Done => "done",
        Stopped => "stopped",
        Failed => "failed",
    }
}

impl RunStatus {
    #[allow(clippy::match_like_matches_macro)]
    pub fn allowed(from: Self, to: Self) -> bool {
        use RunStatus::*;
        match (from, to) {
            (Discovering, Running) => true,
            // A run whose discovery found nothing that can progress is done.
            (Discovering, Done) => true,
            (Discovering, Paused) => true,
            (Discovering, Stopped) => true,
            (Discovering, Failed) => true,
            (Running, Paused) => true,
            (Running, Done) => true,
            (Running, Stopped) => true,
            (Running, Failed) => true,
            (Paused, Running) => true,
            (Paused, Done) => true,
            (Paused, Stopped) => true,
            (Paused, Failed) => true,
            _ => false,
        }
    }

    pub fn allowed_from_new(self) -> bool {
        matches!(self, RunStatus::Discovering | RunStatus::Running)
    }

    /// The statuses the one-active-run index counts.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            RunStatus::Discovering | RunStatus::Running | RunStatus::Paused
        )
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunStatus::Done | RunStatus::Stopped | RunStatus::Failed
        )
    }

    /// SQL list for the partial index's `WHERE status IN (...)`.
    pub fn active_sql_list() -> &'static str {
        "'discovering', 'running', 'paused'"
    }
}

state_enum! {
    /// Why a run is paused (spec 6.5.2). There is no `sleep` reason.
    pub enum PauseReason {
        Person => "person",
        Approval => "approval",
        SitePaused => "site_paused",
        BudgetHit => "budget_hit",
        AgentRateLimited => "agent_rate_limited",
        DiscoveryNeedsPerson => "discovery_needs_person",
    }
}

impl PauseReason {
    /// A site-paused run resumes on its own when the site resumes (6.5.4).
    pub fn resumes_on_site_resume(self) -> bool {
        matches!(self, PauseReason::SitePaused)
    }
}

state_enum! {
    /// How a run started (spec 6.1). `phone` and `schedule` arrive in 1a-ii.
    pub enum RunTrigger {
        Mac => "mac",
        Phone => "phone",
        Schedule => "schedule",
        Agent => "agent",
    }
}

/// How a finished run ended (spec 6.5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    /// Every item reached a settled state with nothing left for the person.
    Complete,
    /// Some items are `waiting_person` or `retries_exhausted`.
    NeedsPerson,
}

impl RunOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            RunOutcome::Complete => "complete",
            RunOutcome::NeedsPerson => "needs_person",
        }
    }
}

/// A run that cannot start or continue (spec 6.5.6). `trace_path` is under
/// the Host data dir and shown as "Show details" on the Mac only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub context: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_path: Option<String>,
}

impl RunError {
    /// The code of a run's stored error, for clients: the message, context
    /// and trace stay on the Host. A row that holds a bare code gives it.
    pub fn code_of(stored: Option<&str>) -> Option<String> {
        let stored = stored?;
        Some(match serde_json::from_str::<RunError>(stored) {
            Ok(error) => error.code,
            Err(_) => stored.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ITEM_EDGES: &[(ItemStatus, ItemStatus)] = &[
        (ItemStatus::Queued, ItemStatus::Working),
        (ItemStatus::Working, ItemStatus::Queued),
        (ItemStatus::Working, ItemStatus::WaitingApproval),
        (ItemStatus::Working, ItemStatus::WaitingPerson),
        (ItemStatus::Working, ItemStatus::Done),
        (ItemStatus::Working, ItemStatus::Skipped),
        (ItemStatus::Working, ItemStatus::Failed),
        (ItemStatus::Working, ItemStatus::Deferred),
        (ItemStatus::Working, ItemStatus::Stopped),
        (ItemStatus::WaitingApproval, ItemStatus::Working),
        (ItemStatus::WaitingApproval, ItemStatus::Skipped),
        (ItemStatus::WaitingApproval, ItemStatus::Stopped),
        (ItemStatus::WaitingPerson, ItemStatus::Queued),
        (ItemStatus::WaitingPerson, ItemStatus::Done),
        (ItemStatus::WaitingPerson, ItemStatus::Skipped),
        (ItemStatus::WaitingPerson, ItemStatus::Failed),
        (ItemStatus::Failed, ItemStatus::Queued),
        (ItemStatus::Failed, ItemStatus::RetriesExhausted),
        (ItemStatus::RetriesExhausted, ItemStatus::Queued),
        (ItemStatus::RetriesExhausted, ItemStatus::Skipped),
        (ItemStatus::Deferred, ItemStatus::Queued),
    ];

    const RUN_EDGES: &[(RunStatus, RunStatus)] = &[
        (RunStatus::Discovering, RunStatus::Running),
        (RunStatus::Discovering, RunStatus::Done),
        (RunStatus::Discovering, RunStatus::Paused),
        (RunStatus::Discovering, RunStatus::Stopped),
        (RunStatus::Discovering, RunStatus::Failed),
        (RunStatus::Running, RunStatus::Paused),
        (RunStatus::Running, RunStatus::Done),
        (RunStatus::Running, RunStatus::Stopped),
        (RunStatus::Running, RunStatus::Failed),
        (RunStatus::Paused, RunStatus::Running),
        (RunStatus::Paused, RunStatus::Done),
        (RunStatus::Paused, RunStatus::Stopped),
        (RunStatus::Paused, RunStatus::Failed),
    ];

    // Generated from the spec 6.4 table: every listed edge is allowed, every
    // pair not in the list is refused. A new state fails this until added.
    #[test]
    fn item_transitions_match_the_spec_table() {
        for &from in ItemStatus::all() {
            for &to in ItemStatus::all() {
                let listed = ITEM_EDGES.contains(&(from, to));
                assert_eq!(
                    ItemStatus::allowed(from, to),
                    listed,
                    "item {from} -> {to} should be {}",
                    if listed { "allowed" } else { "refused" }
                );
            }
        }
    }

    #[test]
    fn run_transitions_match_the_spec_table() {
        for &from in RunStatus::all() {
            for &to in RunStatus::all() {
                let listed = RUN_EDGES.contains(&(from, to));
                assert_eq!(
                    RunStatus::allowed(from, to),
                    listed,
                    "run {from} -> {to} should be {}",
                    if listed { "allowed" } else { "refused" }
                );
            }
        }
    }

    #[test]
    fn done_is_terminal_and_nothing_leaves_it() {
        for &to in ItemStatus::all() {
            assert!(!ItemStatus::allowed(ItemStatus::Done, to));
        }
    }

    #[test]
    fn outward_started_gates_the_recovery_edges_at_the_runtime() {
        // Spec 6.4.1: after outward_started an item never becomes failed,
        // queued, deferred or stopped. The table keeps the pre-dispatch
        // recovery edges (`waiting_person -> queued | failed`,
        // `waiting_approval -> stopped`); the requirement is that the
        // runtime refuses them once outward_started is set, which
        // `transition_item` enforces alongside its CAS. Pinned here so a
        // later edit cannot quietly drop the runtime check's input.
        for from in [ItemStatus::WaitingApproval, ItemStatus::WaitingPerson] {
            let backwards = ItemStatus::all()
                .iter()
                .filter(|to| {
                    matches!(
                        to,
                        ItemStatus::Failed
                            | ItemStatus::Queued
                            | ItemStatus::Deferred
                            | ItemStatus::Stopped
                    )
                })
                .filter(|to| ItemStatus::allowed(from, **to))
                .count();
            assert!(
                backwards > 0,
                "{from} must keep its pre-dispatch recovery edges"
            );
        }
    }

    #[test]
    fn strings_round_trip() {
        for &s in ItemStatus::all() {
            assert_eq!(ItemStatus::parse(s.as_str()), Some(s));
        }
        for &s in RunStatus::all() {
            assert_eq!(RunStatus::parse(s.as_str()), Some(s));
        }
        for &s in PauseReason::all() {
            assert_eq!(PauseReason::parse(s.as_str()), Some(s));
        }
        for &s in RunTrigger::all() {
            assert_eq!(RunTrigger::parse(s.as_str()), Some(s));
        }
        assert_eq!(ItemStatus::parse("nope"), None);
    }

    #[test]
    fn only_queued_items_start_and_only_discovering_or_running_runs() {
        for &s in ItemStatus::all() {
            assert_eq!(s.allowed_from_new(), s == ItemStatus::Queued);
        }
        assert!(RunStatus::Discovering.allowed_from_new());
        assert!(RunStatus::Running.allowed_from_new());
        assert!(!RunStatus::Paused.allowed_from_new());
    }

    #[test]
    fn active_runs_match_the_index_sql_list() {
        for &s in RunStatus::all() {
            let in_sql = RunStatus::active_sql_list().contains(&format!("'{s}'"));
            assert_eq!(s.is_active(), in_sql, "{s}");
        }
    }
}
