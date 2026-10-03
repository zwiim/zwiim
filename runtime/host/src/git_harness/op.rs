//! The typed operations and their outcomes.

use serde::{Deserialize, Serialize};

use crate::commands::folders::GitLogResult;
use crate::workspace_diff::WorkspaceDiff;

/// One Git or GitHub operation in the caller's workspace.
///
/// Deliberately scoped: everything routine, nothing that discards work.
/// Paths are relative to the workspace root and taken literally (no globs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum GitOp {
    /// The current branch, its upstream, and changed files.
    Status,
    /// What changed against the workspace's base; with `file`, that file's
    /// patch.
    Diff {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<String>,
    },
    /// Recent commits on the current branch, newest first.
    Log {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },
    /// Create branch `name` (from `from`, else `HEAD`) and switch to it.
    BranchCreate {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
    },
    /// Switch to an existing branch. Refuses when local changes would be
    /// overwritten.
    Switch { branch: String },
    /// Stage `paths`, including their deletions.
    Stage { paths: Vec<String> },
    /// Commit. With `paths`, exactly those paths (staging them first) and
    /// nothing else that is staged; without, everything staged.
    Commit {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        paths: Option<Vec<String>>,
    },
    /// Fetch `remote` (else the default remote).
    Fetch {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote: Option<String>,
    },
    /// Bring the current branch up to date with its upstream.
    Pull {
        #[serde(default)]
        mode: PullMode,
    },
    /// Push the current branch, setting its upstream on first push. Never
    /// forces.
    Push {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote: Option<String>,
    },
    /// Open a pull request for the current branch.
    PrCreate {
        title: String,
        #[serde(default)]
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
        #[serde(default)]
        draft: bool,
    },
    /// A pull request (`number`, else the current branch's).
    PrView {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        number: Option<u64>,
    },
    /// A pull request's CI checks.
    PrChecks {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        number: Option<u64>,
    },
    /// A pull request's conversation, reviews, and review comments.
    PrComments {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        number: Option<u64>,
    },
    /// Merge a pull request now. Keeps the head branch.
    PrMerge {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        number: Option<u64>,
        #[serde(default)]
        method: MergeMethod,
    },
}

/// The operation's name, without its arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitOpKind {
    Status,
    Diff,
    Log,
    BranchCreate,
    Switch,
    Stage,
    Commit,
    Fetch,
    Pull,
    Push,
    PrCreate,
    PrView,
    PrChecks,
    PrComments,
    PrMerge,
}

impl GitOpKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Diff => "diff",
            Self::Log => "log",
            Self::BranchCreate => "branch_create",
            Self::Switch => "switch",
            Self::Stage => "stage",
            Self::Commit => "commit",
            Self::Fetch => "fetch",
            Self::Pull => "pull",
            Self::Push => "push",
            Self::PrCreate => "pr_create",
            Self::PrView => "pr_view",
            Self::PrChecks => "pr_checks",
            Self::PrComments => "pr_comments",
            Self::PrMerge => "pr_merge",
        }
    }
}

impl GitOp {
    pub fn kind(&self) -> GitOpKind {
        match self {
            Self::Status => GitOpKind::Status,
            Self::Diff { .. } => GitOpKind::Diff,
            Self::Log { .. } => GitOpKind::Log,
            Self::BranchCreate { .. } => GitOpKind::BranchCreate,
            Self::Switch { .. } => GitOpKind::Switch,
            Self::Stage { .. } => GitOpKind::Stage,
            Self::Commit { .. } => GitOpKind::Commit,
            Self::Fetch { .. } => GitOpKind::Fetch,
            Self::Pull { .. } => GitOpKind::Pull,
            Self::Push { .. } => GitOpKind::Push,
            Self::PrCreate { .. } => GitOpKind::PrCreate,
            Self::PrView { .. } => GitOpKind::PrView,
            Self::PrChecks { .. } => GitOpKind::PrChecks,
            Self::PrComments { .. } => GitOpKind::PrComments,
            Self::PrMerge { .. } => GitOpKind::PrMerge,
        }
    }

    /// Whether the operation changes any state: the working tree, the index,
    /// refs (fetch updates remote-tracking refs), a remote, or GitHub. A fact
    /// about the operation, not a decision about it.
    pub fn mutates(&self) -> bool {
        !matches!(
            self.kind(),
            GitOpKind::Status
                | GitOpKind::Diff
                | GitOpKind::Log
                | GitOpKind::PrView
                | GitOpKind::PrChecks
                | GitOpKind::PrComments
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullMode {
    /// Only when the branch can fast-forward; a diverged branch is
    /// `non_fast_forward`.
    #[default]
    FastForwardOnly,
    /// Merge the upstream in; a conflicting merge is `conflict`, with the
    /// merge left in progress for the agent to resolve.
    Merge,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeMethod {
    #[default]
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    pub(super) fn flag(self) -> &'static str {
        match self {
            Self::Merge => "--merge",
            Self::Squash => "--squash",
            Self::Rebase => "--rebase",
        }
    }
}

/// What an operation did or found.
#[derive(Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum GitOutcome {
    Status(StatusReport),
    Diff(WorkspaceDiff),
    Log(GitLogResult),
    BranchCreate(BranchOutcome),
    Switch(BranchOutcome),
    Stage(StageOutcome),
    Commit(CommitOutcome),
    Fetch(FetchOutcome),
    Pull(PullOutcome),
    Push(PushOutcome),
    PrCreate(PrCreated),
    PrView(PrSummary),
    PrChecks { checks: Vec<PrCheck> },
    PrComments(PrComments),
    PrMerge(PrMerged),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StatusReport {
    /// `None` on a detached `HEAD`.
    pub branch: Option<String>,
    /// `HEAD`'s commit; `None` before the first commit.
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub entries: Vec<StatusEntry>,
}

/// One changed path, in `git status --porcelain` terms (`M`, `A`, `D`, `??`,
/// `UU`, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusEntry {
    pub status: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BranchOutcome {
    pub branch: String,
    pub head: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StageOutcome {
    /// Every staged path afterwards, not only the ones just staged.
    pub staged: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommitOutcome {
    pub commit: String,
    pub branch: Option<String>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FetchOutcome {
    pub remote: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PullOutcome {
    pub branch: String,
    pub head: String,
    /// `false` when the branch was already up to date.
    pub updated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PushOutcome {
    pub remote: String,
    pub branch: String,
    pub head: String,
    /// The push also made `remote/branch` the branch's upstream.
    pub set_upstream: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrCreated {
    pub number: Option<u64>,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(deserialize = "camelCase"))]
pub struct PrSummary {
    pub number: u64,
    pub title: String,
    /// `OPEN`, `CLOSED`, or `MERGED`.
    pub state: String,
    pub url: String,
    #[serde(default)]
    pub is_draft: bool,
    #[serde(default)]
    pub base_ref_name: String,
    #[serde(default)]
    pub head_ref_name: String,
    /// `MERGEABLE`, `CONFLICTING`, or `UNKNOWN`.
    #[serde(default)]
    pub mergeable: String,
    /// `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED`, or empty.
    #[serde(default)]
    pub review_decision: Option<String>,
    #[serde(default, deserialize_with = "login")]
    pub author: Option<String>,
    #[serde(default)]
    pub body: String,
}

/// One CI check, as `gh pr checks` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrCheck {
    pub name: String,
    /// The provider's state (`SUCCESS`, `FAILURE`, `IN_PROGRESS`, …).
    #[serde(default)]
    pub state: String,
    /// `pass`, `fail`, `pending`, `skipping`, or `cancel`.
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub workflow: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PrComments {
    pub number: u64,
    /// The pull request's conversation.
    pub comments: Vec<PrComment>,
    /// Submitted reviews and their summaries.
    pub reviews: Vec<PrReview>,
    /// Comments on specific lines of the diff.
    pub review_comments: Vec<PrReviewComment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(deserialize = "camelCase"))]
pub struct PrComment {
    #[serde(default, deserialize_with = "login")]
    pub author: Option<String>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(deserialize = "camelCase"))]
pub struct PrReview {
    #[serde(default, deserialize_with = "login")]
    pub author: Option<String>,
    /// `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, …
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrReviewComment {
    pub author: Option<String>,
    pub path: String,
    pub line: Option<u64>,
    pub body: String,
    pub created_at: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrMerged {
    pub number: u64,
    pub method: MergeMethod,
}

/// `{"login": "…"}` → `Some("…")`; GitHub sends `null` for deleted users.
fn login<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    struct Actor {
        login: Option<String>,
    }
    Ok(Option::<Actor>::deserialize(d)?.and_then(|a| a.login))
}
