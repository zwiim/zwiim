//! Zwiim protocol errors.
//!
//! Every failure on the `/zwiim/v1` boundary is `{"error": {"code", "message"}}`
//! with a stable snake_case `code`, plus a `details` object for the codes that
//! document one (`workspace_busy`, `workspace_dirty`). Internal errors
//! (`AppCommandError`, `AcpError`, `DbErr`, IO) are translated here: their raw
//! text, `detail`, connection ids and host paths never reach the client —
//! anything not in the stable vocabulary collapses to `internal` and is logged
//! host-side instead. The legacy `/api` error shape is unaffected.

use axum::{
    extract::{rejection::JsonRejection, FromRequest, Request},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::de::DeserializeOwned;

use crate::acp::error::AcpError;
use crate::app_error::{AppCommandError, AppErrorCode};
use crate::git_harness::{GitError, GitErrorCode};

#[derive(Debug, Clone)]
pub struct ZwiimError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    /// Structured, client-safe facts about the failure; never internal ids.
    pub details: Option<serde_json::Value>,
}

impl ZwiimError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn unauthenticated() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "Missing or invalid credentials",
        )
    }

    /// The request body could not be parsed into the operation's schema.
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_input", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn turn_in_progress() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "turn_in_progress",
            "The agent is still working on the previous prompt",
        )
    }

    /// The session exists but has no running agent to take the operation.
    pub fn session_not_live() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "session_not_live",
            "The session has no running agent",
        )
    }

    pub fn agent_not_installed() -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "agent_not_installed",
            "The agent is not installed on this host",
        )
    }

    /// Another account already owns this host.
    pub fn host_owned(owner_email: Option<String>) -> Self {
        let mut e = Self::new(
            StatusCode::CONFLICT,
            "host_owned",
            "This host belongs to another account",
        );
        e.details = Some(serde_json::json!({ "owner_email": owner_email }));
        e
    }

    /// The agent does not offer this session option or value (any more).
    pub fn option_unavailable(option: &str, value: &str) -> Self {
        let mut e = Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "option_unavailable",
            "The agent does not offer this option",
        );
        e.details = Some(serde_json::json!({ "option": option, "value": value }));
        e
    }

    /// Removal refused: agents or terminals are still running in the worktree.
    /// `session_ids` names the sessions whose agents run there.
    pub fn workspace_busy(live_sessions: usize, terminals: usize, session_ids: Vec<String>) -> Self {
        let mut e = Self::new(
            StatusCode::CONFLICT,
            "workspace_busy",
            "Agents or terminals are still running in this workspace",
        );
        e.details = Some(serde_json::json!({
            "live_sessions": live_sessions,
            "terminals": terminals,
            "session_ids": session_ids,
        }));
        e
    }

    /// The Host is draining to stop or restart; retry once it is back.
    pub fn host_restarting() -> Self {
        let mut e = Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "host_restarting",
            "The host is restarting",
        );
        e.details = Some(serde_json::json!({ "retry_after_ms": 3000 }));
        e
    }

    /// No update can be applied now: nothing is staged, or this Host does
    /// not run under its supervisor.
    pub fn update_unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "update_unavailable", message)
    }

    /// Removal refused without `force`: the worktree has uncommitted changes.
    pub fn workspace_dirty(changes: usize) -> Self {
        let mut e = Self::new(
            StatusCode::CONFLICT,
            "workspace_dirty",
            "The workspace has uncommitted changes",
        );
        e.details = Some(serde_json::json!({ "changes": changes }));
        e
    }

    /// The worktree workspace was removed; nothing may run in it again.
    pub fn workspace_removed() -> Self {
        Self::new(
            StatusCode::GONE,
            "workspace_removed",
            "The workspace was removed",
        )
    }

    /// The caller lacks the authority the operation requires.
    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "permission_denied", message)
    }

    /// The host's cryptographic identity has not finished loading (or could
    /// not be loaded); the host log says which.
    pub fn identity_unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "identity_unavailable",
            "The host identity is not available yet",
        )
    }

    /// The resource already exists, or is bound to something else.
    pub fn already_exists(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "already_exists", message)
    }

    /// Zwiim Cloud could not be reached or failed the request. Only
    /// Cloud-facing operations (`cloud.*`) return it; nothing local depends
    /// on the Cloud.
    pub fn cloud_unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, "cloud_unavailable", message)
    }

    pub fn unknown_operation() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "unknown_operation",
            "Unknown protocol operation",
        )
    }

    pub fn method_not_allowed() -> Self {
        Self::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            "Protocol operations are invoked with POST",
        )
    }

    /// Signing an agent in or out did not finish on the host (its Keychain or
    /// the provider's sign-out). The host log says which.
    pub fn agent_auth_failed() -> Self {
        Self::new(
            StatusCode::BAD_GATEWAY,
            "agent_auth_failed",
            "The agent's sign-in could not be changed on the host",
        )
    }

    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Internal host error",
        )
    }
}

impl IntoResponse for ZwiimError {
    fn into_response(self) -> Response {
        let mut error = serde_json::json!({ "code": self.code, "message": self.message });
        if let Some(details) = self.details {
            error["details"] = details;
        }
        let body = serde_json::json!({ "error": error });
        (self.status, Json(body)).into_response()
    }
}

impl From<AppCommandError> for ZwiimError {
    fn from(e: AppCommandError) -> Self {
        let message = scrub_paths(&crate::redact::scrub_secrets(&e.message));
        match e.code {
            AppErrorCode::InvalidInput => Self::invalid_input(message),
            AppErrorCode::NotFound => Self::not_found(message),
            AppErrorCode::AlreadyExists => {
                Self::new(StatusCode::CONFLICT, "already_exists", message)
            }
            AppErrorCode::TurnInProgress => Self::turn_in_progress(),
            AppErrorCode::WorkspaceRemoved => Self::workspace_removed(),
            AppErrorCode::PermissionDenied => Self::permission_denied(message),
            AppErrorCode::NotAGitRepository => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "not_a_git_repository",
                "The workspace is not a git repository",
            ),
            _ => {
                tracing::warn!(
                    code = ?e.code,
                    message = %e.message,
                    detail = ?e.detail,
                    "[zwiim] internal error translated to `internal`"
                );
                Self::internal()
            }
        }
    }
}

impl From<GitError> for ZwiimError {
    fn from(e: GitError) -> Self {
        // Harness messages are Zwiim's own words; `gh`'s raw text stays in
        // `detail`, which never leaves the Host.
        let message = scrub_paths(&crate::redact::scrub_secrets(&e.message));
        match e.code {
            GitErrorCode::NoWorkspace => Self::not_found("No such workspace"),
            GitErrorCode::WorkspaceRemoved => Self::workspace_removed(),
            GitErrorCode::NotAGitRepository => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "not_a_git_repository",
                "The workspace is not a git repository",
            ),
            GitErrorCode::InvalidInput => Self::invalid_input(message),
            GitErrorCode::NotFound => Self::not_found(message),
            GitErrorCode::NotMergeable => {
                let mut err = Self::new(StatusCode::CONFLICT, "not_mergeable", message.clone());
                err.details = Some(serde_json::json!({ "reason": message }));
                err
            }
            GitErrorCode::PrChanged => Self::new(StatusCode::CONFLICT, "pr_changed", message),
            GitErrorCode::GhUnavailable => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "gh_unavailable",
                "GitHub CLI is not installed on the Host",
            ),
            GitErrorCode::GhUnauthenticated | GitErrorCode::AuthFailed => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "gh_unauthenticated",
                "GitHub CLI on the Host is not logged in",
            ),
            GitErrorCode::Network | GitErrorCode::Timeout => Self::new(
                StatusCode::BAD_GATEWAY,
                "github_unreachable",
                "GitHub could not be reached",
            ),
            _ => {
                tracing::warn!(
                    code = e.code.as_str(),
                    message = %e.message,
                    "[zwiim] git error translated to `internal`"
                );
                Self::internal()
            }
        }
    }
}

impl From<AcpError> for ZwiimError {
    fn from(e: AcpError) -> Self {
        match e {
            AcpError::TurnInProgress => Self::turn_in_progress(),
            AcpError::SdkNotInstalled(_) => Self::agent_not_installed(),
            AcpError::ConnectionNotFound(_) | AcpError::ProcessExited => Self::session_not_live(),
            other => {
                tracing::warn!(error = %other, "[zwiim] agent error translated to `internal`");
                Self::internal()
            }
        }
    }
}

/// Replace host filesystem paths in a pass-through message with `<path>`.
pub(crate) fn scrub_paths(message: &str) -> String {
    message
        .split(' ')
        .map(|word| {
            let token = word
                .trim_start_matches(['\'', '"', '`', '(', '['])
                .trim_end_matches(['\'', '"', '`', ')', ']', ',', ';', ':', '.']);
            if looks_like_host_path(token) {
                word.replace(token, "<path>")
            } else {
                word.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn looks_like_host_path(token: &str) -> bool {
    let b = token.as_bytes();
    (b.len() > 1 && b[0] == b'/')
        || token.starts_with("~/")
        || (b.len() > 2
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'\\' | b'/'))
}

/// `axum::Json` whose rejection is a Zwiim `invalid_request` instead of axum's
/// plain-text body.
pub struct ZJson<T>(pub T);

impl<T, S> FromRequest<S> for ZJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ZwiimError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(json_rejection(rejection)),
        }
    }
}

/// The field name in serde's "unknown field `x`, expected …" message.
fn unknown_field(serde_message: &str) -> Option<&str> {
    let rest = serde_message.split_once("unknown field `")?.1;
    rest.split_once('`').map(|(name, _)| name)
}

fn json_rejection(rejection: JsonRejection) -> ZwiimError {
    let message = match &rejection {
        JsonRejection::JsonDataError(e) => {
            // "Failed to deserialize the JSON body into the target type: <serde>"
            let text = e.body_text();
            let serde_part = text.split_once(": ").map_or(text.as_str(), |(_, s)| s);
            // A field this Host does not know usually means a newer client:
            // name it so the SDK can say "this Host needs an update".
            if let Some(field) = unknown_field(serde_part) {
                let mut err = ZwiimError::invalid_request(format!("Invalid request: {serde_part}"));
                err.details = Some(serde_json::json!({"reason": "unknown_field", "field": field}));
                return err;
            }
            format!("Invalid request: {serde_part}")
        }
        JsonRejection::JsonSyntaxError(_) => "Request body is not valid JSON".to_string(),
        JsonRejection::MissingJsonContentType(_) => {
            "Expected Content-Type: application/json".to_string()
        }
        _ => "Invalid request body".to_string(),
    };
    ZwiimError::invalid_request(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_host_paths_but_keeps_message() {
        assert_eq!(
            scrub_paths("Failed to open '/Users/a/repo/x.rs': denied"),
            "Failed to open '<path>': denied"
        );
        assert_eq!(
            scrub_paths("Path must be relative"),
            "Path must be relative"
        );
        assert_eq!(scrub_paths(r"bad C:\work\x"), "bad <path>");
    }

    #[test]
    fn names_unknown_fields() {
        assert_eq!(
            unknown_field("unknown field `since`, expected one of `a`, `b` at line 1 column 9"),
            Some("since")
        );
        assert_eq!(unknown_field("missing field `session_id`"), None);
    }

    #[test]
    fn internal_codes_collapse_and_hide_detail() {
        let db = AppCommandError::new(AppErrorCode::DatabaseError, "UNIQUE constraint failed")
            .with_detail("sqlx: conversation.session_id");
        let e = ZwiimError::from(db);
        assert_eq!(
            (e.code, e.status),
            ("internal", StatusCode::INTERNAL_SERVER_ERROR)
        );
        assert!(!e.message.contains("UNIQUE"));

        let e = ZwiimError::from(AcpError::ConnectionNotFound("conn-123".into()));
        assert_eq!(e.code, "session_not_live");
        assert!(!e.message.contains("conn-123"));

        let e = ZwiimError::from(AcpError::Protocol("sacp: -32603 at /tmp/x".into()));
        assert_eq!(e.code, "internal");
        assert!(!e.message.contains("sacp"));

        assert_eq!(
            ZwiimError::from(AcpError::SdkNotInstalled("claude is not installed".into())).code,
            "agent_not_installed"
        );
        assert_eq!(
            ZwiimError::from(AcpError::TurnInProgress).code,
            "turn_in_progress"
        );
    }
}
