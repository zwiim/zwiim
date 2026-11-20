//! Zwiim-owned: the automation engine (automation-engine spec 5.2).
//!
//! Two handles:
//!
//! - [`AutomationEngine`]: the queue, the run and item state enums, the
//!   dispatch transaction, the ledgers, caps, approvals, checks and the run
//!   record.
//! - [`PlaybookStore`]: playbooks, site knowledge, item-ID rules,
//!   allow-lists and login metadata.
//!
//! Item and run statuses live in Rust ([`ItemStatus`], [`RunStatus`]) with an
//! exhaustive `allowed(from, to)` table; the database stores their snake_case
//! strings and every transition is a CAS write committed before the next
//! action (spec 6.4).

pub mod agent_gate;
pub mod alerts;
pub mod approvals;
pub mod caps;
pub mod checks;
pub mod declaration;
pub mod engine;
pub mod entities;
pub mod item_runner;
#[cfg(test)]
mod item_runner_tests;
pub mod lanes;
pub mod lockdown;
pub mod pauses;
pub mod playbook_format;
pub mod playbook_store;
pub mod reasons;
pub mod run_views;
pub mod states;
#[cfg(test)]
mod tests;

pub use agent_gate::{ItemAgentGate, DEFAULT_ITEM_AGENTS};
pub use approvals::{Approval, ApprovalAnswer, ApprovalState};
pub use caps::{Caps, Pace};
pub use checks::{Check, CheckResult};
pub use declaration::{Declaration, PreSubmitCheck, Target};
pub use engine::{AutomationEngine, CapSlot, DispatchOutcome, LedgerKey, StartRunError};
pub use item_runner::{ItemOutcome, ItemRef, ItemRunner, SessionResult};
pub use lockdown::{AgentCapabilities, LockdownLevel, SessionKind, SessionProfile};
pub use pauses::{PauseCause, SitePause, TokenBudget};
pub use playbook_format::{validate_playbook, PlaybookDoc, ValidationError};
pub use playbook_store::{Playbook, PlaybookStore, SiteKnowledgeKind};
pub use reasons::{ActionKind, ApprovalRule, DeclarationDecision, ErrorCode, ReasonCode};
pub use states::{ItemStatus, PauseReason, RunError, RunOutcome, RunStatus, RunTrigger};
