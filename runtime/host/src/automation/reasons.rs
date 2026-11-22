// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: the outward action kinds, approval rules and the reason/error
//! codes (automation-engine spec 8.1, 11.2). These enums are the single
//! source: the docs table and the SDK union are generated from them.

use serde::{Deserialize, Serialize};

/// Outward action kinds (spec 8.1.1). The list can grow; a new kind is a new
/// variant here and a new entry in the cap table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    SubmitApplication,
    /// Any form send that is none of the named kinds (a booking, a support
    /// ticket, a sign-up).
    SubmitForm,
    PublishPost,
    SendMessage,
    Comment,
    Follow,
    Delete,
    Pay,
    ChangeAccount,
}

impl ActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ActionKind::SubmitApplication => "submit_application",
            ActionKind::SubmitForm => "submit_form",
            ActionKind::PublishPost => "publish_post",
            ActionKind::SendMessage => "send_message",
            ActionKind::Comment => "comment",
            ActionKind::Follow => "follow",
            ActionKind::Delete => "delete",
            ActionKind::Pay => "pay",
            ActionKind::ChangeAccount => "change_account",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "submit_application" => ActionKind::SubmitApplication,
            "submit_form" => ActionKind::SubmitForm,
            "publish_post" => ActionKind::PublishPost,
            "send_message" => ActionKind::SendMessage,
            "comment" => ActionKind::Comment,
            "follow" => ActionKind::Follow,
            "delete" => ActionKind::Delete,
            "pay" => ActionKind::Pay,
            "change_account" => ActionKind::ChangeAccount,
            _ => return None,
        })
    }

    pub fn all() -> &'static [ActionKind] {
        &[
            ActionKind::SubmitApplication,
            ActionKind::SubmitForm,
            ActionKind::PublishPost,
            ActionKind::SendMessage,
            ActionKind::Comment,
            ActionKind::Follow,
            ActionKind::Delete,
            ActionKind::Pay,
            ActionKind::ChangeAccount,
        ]
    }

    /// `pay`, `change_account` and `delete` are always ask (spec 8.1.2): a
    /// playbook that sets one to auto is refused on save.
    pub fn always_ask(self) -> bool {
        matches!(
            self,
            ActionKind::Pay | ActionKind::ChangeAccount | ActionKind::Delete
        )
    }

    /// The per-kind daily cap in spec 10.1.1 [est]; `submit_application`
    /// and `submit_form` use the site cap, so they have no per-kind cap.
    pub fn daily_cap(self) -> Option<i32> {
        match self {
            ActionKind::PublishPost => Some(3),
            ActionKind::Comment | ActionKind::Follow | ActionKind::SendMessage => Some(10),
            ActionKind::SubmitApplication
            | ActionKind::SubmitForm
            | ActionKind::Delete
            | ActionKind::Pay
            | ActionKind::ChangeAccount => None,
        }
    }
}

/// What a playbook allows for one action kind (spec 8.1.2). A kind not set
/// is ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRule {
    Auto,
    Ask,
    Never,
}

impl ApprovalRule {
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalRule::Auto => "auto",
            ApprovalRule::Ask => "ask",
            ApprovalRule::Never => "never",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "auto" => ApprovalRule::Auto,
            "ask" => ApprovalRule::Ask,
            "never" => ApprovalRule::Never,
            _ => return None,
        })
    }
}

/// The engine's decision on a declaration (spec 8.2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclarationDecision {
    /// Auto: the agent may click `submit_target`; dispatch runs at the click.
    Allowed,
    /// Ask: end the item `awaiting_approval`; the tab is parked.
    Pending,
    /// Never: `action_kind_never`.
    Refused,
}

/// Item reason codes (spec 11.2.1), fixed and documented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    LoginExpired,
    Captcha,
    BlockedBySite,
    FormFieldUnanswerable,
    DailyCapHit,
    ApprovalRejected,
    UndeclaredOutwardAction,
    ActionKindNever,
    NavigationOutsidePlaybookSites,
    AlreadyDone,
    CheckFailed,
    PageUnrecognised,
    AgentFailed,
    AgentLimit,
    AgentRateLimited,
    Timeout,
    HostRestarted,
    PageNeedsPerson,
    /// A playbook rule says skip (spec 6.4: incl. detail rules); spec 11.2.1
    /// has no code for it.
    SkippedByRule,
}

impl ReasonCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ReasonCode::LoginExpired => "login_expired",
            ReasonCode::Captcha => "captcha",
            ReasonCode::BlockedBySite => "blocked_by_site",
            ReasonCode::FormFieldUnanswerable => "form_field_unanswerable",
            ReasonCode::DailyCapHit => "daily_cap_hit",
            ReasonCode::ApprovalRejected => "approval_rejected",
            ReasonCode::UndeclaredOutwardAction => "undeclared_outward_action",
            ReasonCode::ActionKindNever => "action_kind_never",
            ReasonCode::NavigationOutsidePlaybookSites => "navigation_outside_playbook_sites",
            ReasonCode::AlreadyDone => "already_done",
            ReasonCode::CheckFailed => "check_failed",
            ReasonCode::PageUnrecognised => "page_unrecognised",
            ReasonCode::AgentFailed => "agent_failed",
            ReasonCode::AgentLimit => "agent_limit",
            ReasonCode::AgentRateLimited => "agent_rate_limited",
            ReasonCode::Timeout => "timeout",
            ReasonCode::HostRestarted => "host_restarted",
            ReasonCode::PageNeedsPerson => "page_needs_person",
            ReasonCode::SkippedByRule => "skipped_by_rule",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "login_expired" => ReasonCode::LoginExpired,
            "captcha" => ReasonCode::Captcha,
            "blocked_by_site" => ReasonCode::BlockedBySite,
            "form_field_unanswerable" => ReasonCode::FormFieldUnanswerable,
            "daily_cap_hit" => ReasonCode::DailyCapHit,
            "approval_rejected" => ReasonCode::ApprovalRejected,
            "undeclared_outward_action" => ReasonCode::UndeclaredOutwardAction,
            "action_kind_never" => ReasonCode::ActionKindNever,
            "navigation_outside_playbook_sites" => ReasonCode::NavigationOutsidePlaybookSites,
            "already_done" => ReasonCode::AlreadyDone,
            "check_failed" => ReasonCode::CheckFailed,
            "page_unrecognised" => ReasonCode::PageUnrecognised,
            "agent_failed" => ReasonCode::AgentFailed,
            "agent_limit" => ReasonCode::AgentLimit,
            "agent_rate_limited" => ReasonCode::AgentRateLimited,
            "timeout" => ReasonCode::Timeout,
            "host_restarted" => ReasonCode::HostRestarted,
            "page_needs_person" => ReasonCode::PageNeedsPerson,
            "skipped_by_rule" => ReasonCode::SkippedByRule,
            _ => return None,
        })
    }

    pub fn all() -> &'static [ReasonCode] {
        &[
            ReasonCode::LoginExpired,
            ReasonCode::Captcha,
            ReasonCode::BlockedBySite,
            ReasonCode::FormFieldUnanswerable,
            ReasonCode::DailyCapHit,
            ReasonCode::ApprovalRejected,
            ReasonCode::UndeclaredOutwardAction,
            ReasonCode::ActionKindNever,
            ReasonCode::NavigationOutsidePlaybookSites,
            ReasonCode::AlreadyDone,
            ReasonCode::CheckFailed,
            ReasonCode::PageUnrecognised,
            ReasonCode::AgentFailed,
            ReasonCode::AgentLimit,
            ReasonCode::AgentRateLimited,
            ReasonCode::Timeout,
            ReasonCode::HostRestarted,
            ReasonCode::PageNeedsPerson,
            ReasonCode::SkippedByRule,
        ]
    }

    /// Side effects an item carries when it ends with this code (11.2.3:
    /// none / partial / complete / unknown).
    pub fn side_effects(self) -> SideEffects {
        match self {
            ReasonCode::AlreadyDone
            | ReasonCode::ActionKindNever
            | ReasonCode::ApprovalRejected
            | ReasonCode::DailyCapHit
            | ReasonCode::NavigationOutsidePlaybookSites
            | ReasonCode::SkippedByRule => SideEffects::None,
            ReasonCode::Captcha
            | ReasonCode::BlockedBySite
            | ReasonCode::LoginExpired
            | ReasonCode::FormFieldUnanswerable
            | ReasonCode::UndeclaredOutwardAction
            | ReasonCode::PageUnrecognised
            | ReasonCode::AgentLimit
            | ReasonCode::AgentRateLimited
            | ReasonCode::AgentFailed => SideEffects::Unknown,
            ReasonCode::CheckFailed => SideEffects::Partial,
            ReasonCode::Timeout | ReasonCode::HostRestarted | ReasonCode::PageNeedsPerson => {
                SideEffects::Unknown
            }
        }
    }
}

/// How much of an outward action may have happened (11.2.3 docs column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffects {
    None,
    Partial,
    Complete,
    Unknown,
}

impl SideEffects {
    pub fn as_str(self) -> &'static str {
        match self {
            SideEffects::None => "none",
            SideEffects::Partial => "partial",
            SideEffects::Complete => "complete",
            SideEffects::Unknown => "unknown",
        }
    }
}

/// The meaning and the person's fix for a reason code; docs and the SDK
/// read these, so the table lives beside the enum (spec 11.2.3).
pub struct ReasonInfo {
    pub code: ReasonCode,
    pub meaning: &'static str,
    pub retried: bool,
    pub fix: &'static str,
}

/// Every reason code's docs row, in spec 11.2.1 order.
pub fn reason_infos() -> &'static [ReasonInfo] {
    use ReasonCode::*;
    &[
        ReasonInfo {
            code: LoginExpired,
            meaning: "The site sent us to its login page.",
            retried: false,
            fix: "Sign in to the site on the Mac, then Resume site.",
        },
        ReasonInfo {
            code: Captcha,
            meaning: "A CAPTCHA frame appeared.",
            retried: false,
            fix: "Clear the CAPTCHA in a playbook tab on the Mac, then Resume site.",
        },
        ReasonInfo {
            code: BlockedBySite,
            meaning: "The site showed a block or rate-limit page.",
            retried: false,
            fix: "Wait, then Resume site; the site may be unsupported.",
        },
        ReasonInfo {
            code: FormFieldUnanswerable,
            meaning: "The form asked something the playbook cannot answer.",
            retried: false,
            fix: "Add the answer on the playbook, then Save and retry.",
        },
        ReasonInfo {
            code: DailyCapHit,
            meaning: "The day's cap for this site or kind is spent.",
            retried: false,
            fix: "Wait for the local day to turn over, or lower the cap.",
        },
        ReasonInfo {
            code: ApprovalRejected,
            meaning: "The person skipped this item at approval.",
            retried: false,
            fix: "Nothing; it stays on the playbook's skip list.",
        },
        ReasonInfo {
            code: UndeclaredOutwardAction,
            meaning: "An outward click was attempted without a declaration.",
            retried: true,
            fix: "Nothing; the item retries and the agent declares first.",
        },
        ReasonInfo {
            code: ActionKindNever,
            meaning: "The playbook sets this action kind to never.",
            retried: false,
            fix: "Change the rule on the playbook if this kind should be allowed.",
        },
        ReasonInfo {
            code: NavigationOutsidePlaybookSites,
            meaning: "The item went to a domain not in the playbook's sites.",
            retried: false,
            fix: "Allow the domain on the playbook, then redo the item.",
        },
        ReasonInfo {
            code: AlreadyDone,
            meaning: "This item was already sent by an earlier run.",
            retried: false,
            fix: "Nothing; it is skipped on purpose.",
        },
        ReasonInfo {
            code: CheckFailed,
            meaning: "The item's checks did not pass after it went out.",
            retried: false,
            fix: "Open the item and check the site; the item may need a redo on Mac.",
        },
        ReasonInfo {
            code: PageUnrecognised,
            meaning: "The page did not match the item's URL.",
            retried: true,
            fix: "Nothing; the item retries from its first step.",
        },
        ReasonInfo {
            code: AgentFailed,
            meaning: "The agent working the item failed.",
            retried: true,
            fix: "Nothing; the item retries, up to 3 times.",
        },
        ReasonInfo {
            code: AgentLimit,
            meaning: "An engine session hit its time or tool-call limit.",
            retried: true,
            fix: "Nothing; the item retries, up to 3 times.",
        },
        ReasonInfo {
            code: AgentRateLimited,
            meaning: "The agent's provider rate-limited us.",
            retried: true,
            fix: "Wait; the run backs off and resumes.",
        },
        ReasonInfo {
            code: Timeout,
            meaning: "A step or session timed out.",
            retried: true,
            fix: "Nothing; the item retries.",
        },
        ReasonInfo {
            code: HostRestarted,
            meaning: "The Host restarted mid-item.",
            retried: true,
            fix: "Nothing; the item restarts from its first step.",
        },
        ReasonInfo {
            code: PageNeedsPerson,
            meaning: "The site's page asked something only a person can answer.",
            retried: false,
            fix: "Answer it in the playbook tab on the Mac, then Resume.",
        },
        ReasonInfo {
            code: SkippedByRule,
            meaning: "A playbook rule says this item is not one to work.",
            retried: false,
            fix: "Nothing; edit the rule if it skipped one you want.",
        },
    ]
}

/// Render the reason-code docs page from the enums; the checked-in file is
/// this output, so a hand edit fails the drift test (spec 11.2.3).
pub fn reason_codes_markdown() -> String {
    let mut out = String::new();
    out.push_str("# Automation reason codes\n\n");
    out.push_str("Generated from `runtime/host/src/automation/reasons.rs`. Do not edit by hand;\n");
    out.push_str("the drift test `scripts/automation-docs.mjs` fails on a hand edit.\n\n");
    out.push_str("| Code | Meaning | Retried | Side effects | The person's fix |\n");
    out.push_str("|---|---|---|---|---|\n");
    for info in reason_infos() {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            info.code.as_str(),
            info.meaning,
            if info.retried { "yes" } else { "no" },
            info.code.side_effects_str(),
            info.fix,
        ));
    }
    out.push_str("\n## Protocol error codes\n\n");
    out.push_str("| Code | Meaning |\n|---|---|\n");
    for code in ErrorCode::all() {
        out.push_str(&format!(
            "| `{}` | {} |\n",
            code.as_str(),
            error_meaning(*code)
        ));
    }
    out
}

fn error_meaning(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::RunActive => {
            "The playbook already has an active run; `details.run_id` names it."
        }
        ErrorCode::ActionKindRequiresAsk => {
            "This action kind must be ask; a playbook cannot set it to auto."
        }
        ErrorCode::BudgetUnsupportedForAgent => {
            "This agent reports no token usage, so it cannot have a budget."
        }
        ErrorCode::ApprovalNotPending => {
            "The approval was already answered; `details.state` says how."
        }
        ErrorCode::SiteNotPaused => "Resume site was asked for a site that is not paused.",
        ErrorCode::RunNotPaused => "Resume run was asked for a run that is not paused.",
        ErrorCode::PlaybookInvalid => {
            "The playbook failed validation; `details` lists line, field, problem, fix."
        }
        ErrorCode::RunStartRequiresMac => "1a-i runs start from the Mac only.",
        ErrorCode::ScheduleNeedsShowMeRun => "A schedule needs a completed show-me run first.",
        ErrorCode::PlaybooksDisabled => "Playbooks are off in Settings > Labs.",
        ErrorCode::RunNotRunning => "The run is not running.",
        ErrorCode::SiteRiskNotAccepted => {
            "A site of the playbook has no risk step yet; `details.sites` lists them."
        }
        ErrorCode::NoOpenQuestion => "The item is not waiting on a question.",
        ErrorCode::ItemNotWaiting => "The item is not waiting on the person.",
        ErrorCode::InputInUse => "A step still uses this input.",
        ErrorCode::FileTooLarge => "The file is over 50 MB.",
        ErrorCode::FileNotFound => "That file is not on this computer.",
    }
}

impl ReasonCode {
    /// Side effects of a reason, from the enum (11.2.3).
    pub fn side_effects_str(self) -> &'static str {
        self.side_effects().as_str()
    }
}

/// Protocol error codes in the `ZwiimError` envelope (spec 11.2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    RunActive,
    ActionKindRequiresAsk,
    BudgetUnsupportedForAgent,
    ApprovalNotPending,
    SiteNotPaused,
    RunNotPaused,
    PlaybookInvalid,
    RunStartRequiresMac,
    ScheduleNeedsShowMeRun,
    /// Any `playbook.*` / `run.*` / `site.*` while Playbooks are off in Labs.
    PlaybooksDisabled,
    /// `run.pause` asked about a run that is not running.
    RunNotRunning,
    /// `run.start` / `run.try` on a playbook with a site whose risk the
    /// person has not accepted (spec 14.5).
    SiteRiskNotAccepted,
    /// `run.answer` for an item that waits on no question.
    NoOpenQuestion,
    /// `run.resolve` for an item that is not waiting on the person.
    ItemNotWaiting,
    /// `playbook.file.remove` for an input a step still names.
    InputInUse,
    /// `playbook.file.set` with a file over 50 MB.
    FileTooLarge,
    /// `playbook.file.set` with a path that is not a file.
    FileNotFound,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::RunActive => "run_active",
            ErrorCode::ActionKindRequiresAsk => "action_kind_requires_ask",
            ErrorCode::BudgetUnsupportedForAgent => "budget_unsupported_for_agent",
            ErrorCode::ApprovalNotPending => "approval_not_pending",
            ErrorCode::SiteNotPaused => "site_not_paused",
            ErrorCode::RunNotPaused => "run_not_paused",
            ErrorCode::PlaybookInvalid => "playbook_invalid",
            ErrorCode::RunStartRequiresMac => "run_start_requires_mac",
            ErrorCode::ScheduleNeedsShowMeRun => "schedule_needs_show_me_run",
            ErrorCode::PlaybooksDisabled => "playbooks_disabled",
            ErrorCode::RunNotRunning => "run_not_running",
            ErrorCode::SiteRiskNotAccepted => "site_risk_not_accepted",
            ErrorCode::NoOpenQuestion => "no_open_question",
            ErrorCode::ItemNotWaiting => "item_not_waiting",
            ErrorCode::InputInUse => "input_in_use",
            ErrorCode::FileTooLarge => "file_too_large",
            ErrorCode::FileNotFound => "file_not_found",
        }
    }

    pub fn all() -> &'static [ErrorCode] {
        &[
            ErrorCode::RunActive,
            ErrorCode::ActionKindRequiresAsk,
            ErrorCode::BudgetUnsupportedForAgent,
            ErrorCode::ApprovalNotPending,
            ErrorCode::SiteNotPaused,
            ErrorCode::RunNotPaused,
            ErrorCode::PlaybookInvalid,
            ErrorCode::RunStartRequiresMac,
            ErrorCode::ScheduleNeedsShowMeRun,
            ErrorCode::PlaybooksDisabled,
            ErrorCode::RunNotRunning,
            ErrorCode::SiteRiskNotAccepted,
            ErrorCode::NoOpenQuestion,
            ErrorCode::ItemNotWaiting,
            ErrorCode::InputInUse,
            ErrorCode::FileTooLarge,
            ErrorCode::FileNotFound,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_code_is_in_the_sdk_error_union() {
        // The SDK's `HostErrorCode` is written by hand; this keeps it from
        // missing a code the Host sends (spec 11.2).
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../clients/sdk/src/types/protocol.ts"
        );
        let sdk = std::fs::read_to_string(path).expect("read the SDK protocol types");
        let start = sdk
            .find("export type HostErrorCode =")
            .expect("HostErrorCode");
        // Members are `| "code"` lines; the union ends at the first line
        // ending in `;` (doc comments in between may hold semicolons).
        let members: Vec<&str> = sdk[start..]
            .lines()
            .skip(1)
            .map(str::trim)
            .take_while(|l| l.starts_with('|') || l.starts_with("/*") || l.starts_with('*'))
            .filter_map(|l| l.strip_prefix("| "))
            .map(|l| l.trim_end_matches(';').trim_matches('"'))
            .collect();
        for code in ErrorCode::all() {
            assert!(
                members.contains(&code.as_str()),
                "`{}` is missing from HostErrorCode in clients/sdk/src/types/protocol.ts",
                code.as_str()
            );
        }
    }

    /// The SDK's `ItemReasonCode` is written by hand too; every item reason
    /// the Host records is in it, the rule skip included.
    #[test]
    fn every_reason_code_is_in_the_sdk_item_reason_union() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../clients/sdk/src/types/protocol.ts"
        );
        let sdk = std::fs::read_to_string(path).expect("read the SDK protocol types");
        let start = sdk
            .find("export type ItemReasonCode =")
            .expect("ItemReasonCode");
        let members: Vec<&str> = sdk[start..]
            .lines()
            .skip(1)
            .map(str::trim)
            .take_while(|l| l.starts_with('|'))
            .filter_map(|l| l.strip_prefix("| "))
            .map(|l| l.trim_end_matches(';').trim_matches('"'))
            .collect();
        for code in ReasonCode::all() {
            assert!(
                members.contains(&code.as_str()),
                "`{}` is missing from ItemReasonCode in clients/sdk/src/types/protocol.ts",
                code.as_str()
            );
        }
        assert_eq!(
            ReasonCode::parse("skipped_by_rule"),
            Some(ReasonCode::SkippedByRule)
        );
    }

    #[test]
    fn always_ask_kinds_never_go_auto_and_names_round_trip() {
        for &kind in ActionKind::all() {
            assert_eq!(ActionKind::parse(kind.as_str()), Some(kind));
            if kind.always_ask() {
                assert!(matches!(
                    kind,
                    ActionKind::Pay | ActionKind::ChangeAccount | ActionKind::Delete
                ));
            }
        }
        for &rule in &[ApprovalRule::Auto, ApprovalRule::Ask, ApprovalRule::Never] {
            assert_eq!(ApprovalRule::parse(rule.as_str()), Some(rule));
        }
    }

    #[test]
    fn a_general_form_send_is_its_own_kind_under_the_site_cap() {
        assert_eq!(
            ActionKind::parse("submit_form"),
            Some(ActionKind::SubmitForm)
        );
        assert!(ActionKind::all().contains(&ActionKind::SubmitForm));
        assert!(!ActionKind::SubmitForm.always_ask());
        assert_eq!(ActionKind::SubmitForm.daily_cap(), None);
    }

    /// The docs table is generated from the enums; this pins that every code
    /// has a row and the table is in the enum's order (spec 11.2.3 drift
    /// check). The checked-in `docs/automation/reason-codes.md` is rendered
    /// from this table by `reason_codes_markdown`.
    #[test]
    fn every_reason_code_has_exactly_one_docs_row() {
        let infos = reason_infos();
        assert_eq!(infos.len(), ReasonCode::all().len());
        for (info, code) in infos.iter().zip(ReasonCode::all()) {
            assert_eq!(info.code, *code, "row order must follow the enum");
        }
    }

    // The checked-in docs are byte-checked by `scripts/automation-docs.mjs`
    // (a CI drift check, since a Rust unit test cannot write the file through
    // its crate boundary). Here we pin that the render is stable and lists
    // every code.
    #[test]
    fn the_generated_table_is_stable_and_complete() {
        let once = reason_codes_markdown();
        assert_eq!(once, reason_codes_markdown(), "the render must be stable");
        for code in ReasonCode::all() {
            assert!(
                once.contains(&format!("`{}`", code.as_str())),
                "{} missing from the docs render",
                code.as_str()
            );
        }
        for code in ErrorCode::all() {
            assert!(
                once.contains(&format!("`{}`", code.as_str())),
                "{} missing from the docs render",
                code.as_str()
            );
        }
    }

    #[test]
    fn code_names_round_trip() {
        for &code in ReasonCode::all() {
            assert_eq!(ReasonCode::parse(code.as_str()), Some(code));
        }
    }

    #[test]
    fn spec_errors_are_listed_once_each() {
        let mut seen = std::collections::HashSet::new();
        for &code in ErrorCode::all() {
            assert!(seen.insert(code.as_str()), "{} duplicated", code.as_str());
        }
        assert_eq!(seen.len(), ErrorCode::all().len());
    }

    #[test]
    fn per_kind_caps_match_spec_10_1_1() {
        assert_eq!(ActionKind::PublishPost.daily_cap(), Some(3));
        for k in [
            ActionKind::Comment,
            ActionKind::Follow,
            ActionKind::SendMessage,
        ] {
            assert_eq!(k.daily_cap(), Some(10));
        }
        assert_eq!(ActionKind::SubmitApplication.daily_cap(), None);
    }
}
