//! A pull request as the user follows it: where it stands, its checks, and
//! whether GitHub would merge it now. Built from one `gh pr view --json` call;
//! everything here is a pure mapping of what GitHub reported.

use serde::{Deserialize, Serialize};

use super::op::MergeMethod;

/// The `gh pr view --json` fields [`PrState`] is built from.
pub(super) const PR_STATE_FIELDS: &str = "number,title,state,url,isDraft,baseRefName,headRefName,\
headRefOid,mergeStateStatus,statusCheckRollup,mergedAt,autoMergeRequest";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrState {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub state: PrLifecycle,
    pub base: String,
    pub head: String,
    /// The head commit GitHub has; a merge is pinned to it.
    pub head_sha: String,
    pub merged_at: Option<String>,
    pub checks: PrChecksSummary,
    pub merge: PrMergeInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrLifecycle {
    Open,
    Draft,
    Merged,
    Closed,
}

impl PrLifecycle {
    /// Merged and closed pull requests do not change on their own.
    pub fn is_final(self) -> bool {
        matches!(self, Self::Merged | Self::Closed)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PrChecksSummary {
    pub passed: u32,
    pub failed: u32,
    pub pending: u32,
    pub skipped: u32,
    pub items: Vec<PrCheckItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrCheckItem {
    pub name: String,
    pub workflow: Option<String>,
    pub state: CheckState,
    pub link: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    Passed,
    Failed,
    Pending,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrMergeInfo {
    /// GitHub would merge it now.
    pub allowed: bool,
    pub status: MergeStatus,
    /// Why not, in words for the user; `None` when allowed or settled.
    pub reason: Option<String>,
    /// The method a merge uses unless told otherwise.
    pub method: Option<MergeMethod>,
    /// The methods the repository allows.
    pub methods: Vec<MergeMethod>,
}

/// GitHub's `mergeStateStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeStatus {
    Clean,
    Unstable,
    HasHooks,
    Blocked,
    Behind,
    Dirty,
    Draft,
    Unknown,
}

impl MergeStatus {
    fn parse(raw: &str) -> Self {
        match raw {
            "CLEAN" => Self::Clean,
            "UNSTABLE" => Self::Unstable,
            "HAS_HOOKS" => Self::HasHooks,
            "BLOCKED" => Self::Blocked,
            "BEHIND" => Self::Behind,
            "DIRTY" => Self::Dirty,
            "DRAFT" => Self::Draft,
            _ => Self::Unknown,
        }
    }

    /// GitHub merges now: nothing required is missing. `UNSTABLE` means only
    /// checks that are not required are failing or still running.
    fn allows_merge(self) -> bool {
        matches!(self, Self::Clean | Self::Unstable | Self::HasHooks)
    }
}

/// The repository's merge settings, from `gh repo view --json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RepoMergeSettings {
    #[serde(default)]
    pub squash_merge_allowed: bool,
    #[serde(default)]
    pub merge_commit_allowed: bool,
    #[serde(default)]
    pub rebase_merge_allowed: bool,
    /// `MERGE`, `SQUASH` or `REBASE`: what github.com preselects for the user.
    #[serde(default)]
    pub viewer_default_merge_method: Option<String>,
}

pub(super) const REPO_MERGE_FIELDS: &str =
    "squashMergeAllowed,mergeCommitAllowed,rebaseMergeAllowed,viewerDefaultMergeMethod";

impl RepoMergeSettings {
    /// The allowed methods, in the order a merge prefers them: the user's
    /// github.com default first, then squash, merge, rebase.
    pub fn methods(&self) -> Vec<MergeMethod> {
        let allowed = |m: MergeMethod| match m {
            MergeMethod::Squash => self.squash_merge_allowed,
            MergeMethod::Merge => self.merge_commit_allowed,
            MergeMethod::Rebase => self.rebase_merge_allowed,
        };
        let preferred = match self.viewer_default_merge_method.as_deref() {
            Some("SQUASH") => Some(MergeMethod::Squash),
            Some("MERGE") => Some(MergeMethod::Merge),
            Some("REBASE") => Some(MergeMethod::Rebase),
            _ => None,
        };
        let mut out: Vec<MergeMethod> = preferred.into_iter().filter(|m| allowed(*m)).collect();
        for m in [MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase] {
            if allowed(m) && !out.contains(&m) {
                out.push(m);
            }
        }
        out
    }
}

/// `gh pr view --json PR_STATE_FIELDS`, as GitHub sends it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RawPr {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub is_draft: bool,
    #[serde(default)]
    pub base_ref_name: String,
    #[serde(default)]
    pub head_ref_name: String,
    #[serde(default)]
    pub head_ref_oid: String,
    #[serde(default)]
    pub merge_state_status: String,
    #[serde(default)]
    pub status_check_rollup: Option<Vec<RawCheck>>,
    #[serde(default)]
    pub merged_at: Option<String>,
    #[serde(default)]
    pub auto_merge_request: Option<serde_json::Value>,
}

/// One rollup entry: a CheckRun (`status` + `conclusion`) or a commit
/// StatusContext (`state`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RawCheck {
    #[serde(default, rename = "__typename")]
    pub typename: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub workflow_name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub details_url: Option<String>,
    #[serde(default)]
    pub target_url: Option<String>,
}

impl RawCheck {
    fn check_state(&self) -> CheckState {
        let upper = |s: &Option<String>| s.as_deref().unwrap_or_default().to_ascii_uppercase();
        if self.typename == "StatusContext" || (self.status.is_none() && self.state.is_some()) {
            return match upper(&self.state).as_str() {
                "SUCCESS" => CheckState::Passed,
                "FAILURE" | "ERROR" => CheckState::Failed,
                _ => CheckState::Pending,
            };
        }
        if upper(&self.status) != "COMPLETED" {
            return CheckState::Pending;
        }
        match upper(&self.conclusion).as_str() {
            "SUCCESS" | "NEUTRAL" => CheckState::Passed,
            "SKIPPED" | "STALE" => CheckState::Skipped,
            _ => CheckState::Failed,
        }
    }

    fn item(&self) -> PrCheckItem {
        let non_empty = |s: &Option<String>| s.clone().filter(|s| !s.trim().is_empty());
        PrCheckItem {
            name: non_empty(&self.name)
                .or_else(|| non_empty(&self.context))
                .unwrap_or_else(|| "check".to_string()),
            workflow: non_empty(&self.workflow_name),
            state: self.check_state(),
            link: non_empty(&self.details_url).or_else(|| non_empty(&self.target_url)),
        }
    }
}

pub(super) fn summarize_checks(rollup: &[RawCheck]) -> PrChecksSummary {
    let mut summary = PrChecksSummary::default();
    for check in rollup {
        let item = check.item();
        match item.state {
            CheckState::Passed => summary.passed += 1,
            CheckState::Failed => summary.failed += 1,
            CheckState::Pending => summary.pending += 1,
            CheckState::Skipped => summary.skipped += 1,
        }
        summary.items.push(item);
    }
    summary
}

impl RawPr {
    pub fn lifecycle(&self) -> PrLifecycle {
        match self.state.as_str() {
            "MERGED" => PrLifecycle::Merged,
            "CLOSED" => PrLifecycle::Closed,
            _ if self.is_draft => PrLifecycle::Draft,
            _ => PrLifecycle::Open,
        }
    }

    pub fn merge_status(&self) -> MergeStatus {
        MergeStatus::parse(&self.merge_state_status)
    }

    /// The user-facing state, with `methods` the repository allows.
    pub fn into_state(self, methods: Vec<MergeMethod>) -> PrState {
        let lifecycle = self.lifecycle();
        let status = self.merge_status();
        let allowed = !lifecycle.is_final() && status.allows_merge() && !methods.is_empty();
        let reason = if lifecycle.is_final() || allowed {
            None
        } else if methods.is_empty() && status.allows_merge() {
            Some("The repository allows no merge method".to_string())
        } else {
            Some(blocked_reason(status, &self.base_ref_name))
        };
        PrState {
            checks: summarize_checks(self.status_check_rollup.as_deref().unwrap_or_default()),
            merge: PrMergeInfo {
                allowed,
                status,
                reason,
                method: methods.first().copied(),
                methods,
            },
            number: self.number,
            url: self.url,
            title: self.title,
            state: lifecycle,
            base: self.base_ref_name,
            head: self.head_ref_name,
            head_sha: self.head_ref_oid,
            merged_at: self.merged_at,
        }
    }
}

fn blocked_reason(status: MergeStatus, base: &str) -> String {
    match status {
        MergeStatus::Blocked => "Waiting for required checks or review".to_string(),
        MergeStatus::Behind => format!("Branch is behind {base}"),
        MergeStatus::Dirty => "Merge conflicts".to_string(),
        MergeStatus::Draft => "Draft PR".to_string(),
        _ => "Checking…".to_string(),
    }
}
