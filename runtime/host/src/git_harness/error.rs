//! Structured failures. Subprocess output only ever travels as a scrubbed,
//! bounded `detail`; the `code` is the contract.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::app_error::{AppCommandError, AppErrorCode};

/// The longest `detail` carried, in bytes.
const MAX_DETAIL: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitErrorCode {
    /// The caller named no workspace, or one that does not exist.
    NoWorkspace,
    /// The workspace is a removed worktree; nothing runs in it again.
    WorkspaceRemoved,
    NotAGitRepository,
    /// A path is absolute, escapes the workspace, or is empty.
    InvalidPath,
    /// Any other malformed argument (a branch name that reads as an option,
    /// an empty commit message, …).
    InvalidInput,
    /// A branch, ref, remote, or pull request that does not exist.
    NotFound,
    AlreadyExists,
    NothingToCommit,
    /// Local changes or a merge conflict stand in the way.
    Conflict,
    /// The remote has commits this branch lacks.
    NonFastForward,
    /// The current branch has no upstream to pull from.
    NoUpstream,
    /// `HEAD` is not on a branch.
    DetachedHead,
    /// Git or GitHub refused the user's credentials, or has none.
    AuthFailed,
    Network,
    /// `gh` is not installed.
    GhUnavailable,
    /// `gh` is installed but not logged in.
    GhUnauthenticated,
    /// The pull request cannot be merged as it stands.
    NotMergeable,
    /// The pull request's head moved since the caller looked at it.
    PrChanged,
    /// The repository has "Allow auto-merge" off on GitHub.
    AutoMergeDisabled,
    /// The operation ran past its time limit and was stopped.
    Timeout,
    /// Anything else Git or `gh` refused; see `detail`.
    Failed,
}

impl GitErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoWorkspace => "no_workspace",
            Self::WorkspaceRemoved => "workspace_removed",
            Self::NotAGitRepository => "not_a_git_repository",
            Self::InvalidPath => "invalid_path",
            Self::InvalidInput => "invalid_input",
            Self::NotFound => "not_found",
            Self::AlreadyExists => "already_exists",
            Self::NothingToCommit => "nothing_to_commit",
            Self::Conflict => "conflict",
            Self::NonFastForward => "non_fast_forward",
            Self::NoUpstream => "no_upstream",
            Self::DetachedHead => "detached_head",
            Self::AuthFailed => "auth_failed",
            Self::Network => "network",
            Self::GhUnavailable => "gh_unavailable",
            Self::GhUnauthenticated => "gh_unauthenticated",
            Self::NotMergeable => "not_mergeable",
            Self::PrChanged => "pr_changed",
            Self::AutoMergeDisabled => "auto_merge_disabled",
            Self::Timeout => "timeout",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitError {
    pub code: GitErrorCode,
    pub message: String,
    /// What Git or `gh` said, secrets scrubbed and truncated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl GitError {
    pub fn new(code: GitErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: &str) -> Self {
        let detail = bounded(detail.trim());
        self.detail = (!detail.is_empty()).then_some(detail);
        self
    }

    /// Map a Host command error from the reused Git helpers.
    pub(super) fn from_app(e: AppCommandError) -> Self {
        let code = match e.code {
            AppErrorCode::NotAGitRepository => GitErrorCode::NotAGitRepository,
            AppErrorCode::WorkspaceRemoved => GitErrorCode::WorkspaceRemoved,
            AppErrorCode::NotFound => GitErrorCode::NotFound,
            AppErrorCode::InvalidInput => GitErrorCode::InvalidInput,
            AppErrorCode::AlreadyExists => GitErrorCode::AlreadyExists,
            AppErrorCode::AuthenticationFailed => GitErrorCode::AuthFailed,
            AppErrorCode::NetworkError => GitErrorCode::Network,
            _ => GitErrorCode::Failed,
        };
        let err = Self::new(code, e.message);
        match e.detail {
            Some(d) => err.with_detail(&d),
            None => err,
        }
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for GitError {}

fn bounded(text: &str) -> String {
    let text = crate::redact::scrub_secrets(text);
    if text.len() <= MAX_DETAIL {
        return text.into_owned();
    }
    let mut end = MAX_DETAIL;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Classify a failed `git` run from its stderr. `op` names it in the message.
pub(super) fn classify_git(op: &str, stderr: &str) -> GitError {
    let lower = stderr.to_lowercase();
    let code = if lower.contains("nothing to commit")
        || lower.contains("nothing added to commit")
        || lower.contains("no changes added to commit")
    {
        GitErrorCode::NothingToCommit
    } else if lower.contains("non-fast-forward")
        || lower.contains("[rejected]")
        || lower.contains("fetch first")
        || lower.contains("not possible to fast-forward")
        || lower.contains("diverging branches")
    {
        GitErrorCode::NonFastForward
    } else if lower.contains("conflict")
        || lower.contains("would be overwritten")
        || lower.contains("you have unmerged paths")
        || lower.contains("needs merge")
        || lower.contains("unmerged files")
    {
        GitErrorCode::Conflict
    } else if lower.contains("already exists") {
        GitErrorCode::AlreadyExists
    } else if lower.contains("outside repository") || lower.contains("is beyond a symbolic link") {
        GitErrorCode::InvalidPath
    } else if lower.contains("there is no tracking information")
        || lower.contains("no upstream configured")
        || lower.contains("has no upstream branch")
    {
        GitErrorCode::NoUpstream
    } else if lower.contains("invalid reference")
        || lower.contains("not a valid object name")
        || lower.contains("did not match any")
        || lower.contains("couldn't find remote ref")
        || lower.contains("does not appear to be a git repository")
        || lower.contains("no such remote")
        || lower.contains("not a valid ref")
    {
        GitErrorCode::NotFound
    } else if lower.contains("not a valid branch name") || lower.contains("is not a valid") {
        GitErrorCode::InvalidInput
    } else {
        // The Host's own remote classifier: credentials and connectivity.
        let app = crate::commands::folders::classify_remote_git_error(op, stderr.as_bytes());
        match app.code {
            AppErrorCode::AuthenticationFailed => GitErrorCode::AuthFailed,
            AppErrorCode::NetworkError => GitErrorCode::Network,
            _ if lower.contains("permission denied (publickey")
                || lower.contains("host key verification failed") =>
            {
                GitErrorCode::AuthFailed
            }
            _ => GitErrorCode::Failed,
        }
    };
    GitError::new(code, format!("git {op}: {}", describe(code))).with_detail(stderr)
}

/// Classify a failed `gh` run from its exit code and output.
pub(super) fn classify_gh(op: &str, exit: Option<i32>, stderr: &str) -> GitError {
    let lower = stderr.to_lowercase();
    let code = if exit == Some(4)
        || lower.contains("gh auth login")
        || lower.contains("not logged into")
        || lower.contains("authentication required")
    {
        GitErrorCode::GhUnauthenticated
    } else if lower.contains("bad credentials") || lower.contains("http 401") {
        GitErrorCode::AuthFailed
    } else if lower.contains("no pull requests found")
        || lower.contains("could not resolve to a pullrequest")
        || lower.contains("no open pull requests")
        || lower.contains("http 404")
    {
        GitErrorCode::NotFound
    } else if lower.contains("already exists") {
        GitErrorCode::AlreadyExists
    } else if lower.contains("head branch was modified") {
        GitErrorCode::PrChanged
    } else if lower.contains("auto merge is not allowed")
        || lower.contains("auto-merge is not allowed")
    {
        GitErrorCode::AutoMergeDisabled
    } else if lower.contains("not mergeable")
        || lower.contains("merge conflict")
        || lower.contains("is not mergeable")
        || lower.contains("base branch policy prohibits")
        || lower.contains("required status check")
    {
        GitErrorCode::NotMergeable
    } else if lower.contains("could not resolve host")
        || lower.contains("no such host")
        || lower.contains("connection refused")
        || lower.contains("network is unreachable")
        || lower.contains("error connecting to")
        || lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("tls handshake")
        || lower.contains("connection reset")
        || lower.contains("unexpected eof")
    {
        GitErrorCode::Network
    } else if lower.contains("none of the git remotes")
        || lower.contains("not a git repository")
        || lower.contains("no git remotes")
    {
        GitErrorCode::NotFound
    } else {
        GitErrorCode::Failed
    };
    GitError::new(code, format!("gh {op}: {}", describe(code))).with_detail(stderr)
}

fn describe(code: GitErrorCode) -> &'static str {
    match code {
        GitErrorCode::NoWorkspace => "no workspace",
        GitErrorCode::WorkspaceRemoved => "the workspace was removed",
        GitErrorCode::NotAGitRepository => "not a Git repository",
        GitErrorCode::InvalidPath => "a path is outside the workspace",
        GitErrorCode::InvalidInput => "invalid argument",
        GitErrorCode::NotFound => "not found",
        GitErrorCode::AlreadyExists => "already exists",
        GitErrorCode::NothingToCommit => "nothing to commit",
        GitErrorCode::Conflict => "conflicting changes",
        GitErrorCode::NonFastForward => "the remote has commits this branch does not",
        GitErrorCode::NoUpstream => "the branch has no upstream",
        GitErrorCode::DetachedHead => "HEAD is not on a branch",
        GitErrorCode::AuthFailed => "authentication failed",
        GitErrorCode::Network => "network error",
        GitErrorCode::GhUnavailable => "the GitHub CLI (gh) is not installed",
        GitErrorCode::GhUnauthenticated => "the GitHub CLI is not logged in; run `gh auth login`",
        GitErrorCode::NotMergeable => "the pull request cannot be merged",
        GitErrorCode::PrChanged => "the pull request changed; review the new commits",
        GitErrorCode::AutoMergeDisabled => "auto-merge is off for this repository on GitHub",
        GitErrorCode::Timeout => "timed out",
        GitErrorCode::Failed => "failed",
    }
}
