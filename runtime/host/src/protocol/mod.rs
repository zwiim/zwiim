//! Zwiim Protocol V1 — the client-facing boundary at `/zwiim/v1`.
//!
//! Operations are `POST /zwiim/v1/<op>` with a JSON body; session and
//! terminal events stream over `GET /zwiim/v1/ws`. The protocol is a thin
//! translation over the host runtime: clients address everything by
//! `workspace_id` / `session_id` / `terminal_id`, and internal row ids,
//! agent connection ids, ACP session ids and host paths never cross it.
//! Authentication is the host's bearer check (`web::auth::require_token`).
//! Device management and `host.claim` additionally require local host
//! authority (`web::auth::require_local_authority`); `device.pair` is the one
//! unauthenticated operation, authorized by its one-time pairing code.

pub mod compat;
pub mod error;
pub mod events;
pub mod http;
pub mod ws;

#[cfg(test)]
pub(crate) mod tests;

use axum::{
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use sea_orm::DatabaseConnection;

use crate::web::auth;
use error::ZwiimError;

/// Replace the plain-text bodies axum and the shared auth middleware produce
/// for rejected requests with the Zwiim error shape.
async fn normalize_rejections(req: axum::extract::Request, next: Next) -> Response {
    let res = next.run(req).await;
    let is_json = res
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if is_json {
        return res;
    }
    match res.status() {
        StatusCode::UNAUTHORIZED => ZwiimError::unauthenticated().into_response(),
        StatusCode::FORBIDDEN => {
            ZwiimError::permission_denied("This operation requires local host authority")
                .into_response()
        }
        StatusCode::METHOD_NOT_ALLOWED => ZwiimError::method_not_allowed().into_response(),
        StatusCode::PAYLOAD_TOO_LARGE => {
            ZwiimError::invalid_request("Request body is too large").into_response()
        }
        _ => res,
    }
}

/// Operations a draining Host still answers: the handshake, so a client can
/// tell the Host is there, and the restart request itself (a drain waits for
/// admitted operations, and must not wait for the one that started it).
const ANSWERED_WHILE_DRAINING: [&str; 4] = [
    "host.hello",
    "host.restart",
    "host.update.apply",
    "host.update.status",
];

/// Refuse new operations once the Host drains, and count the admitted ones
/// so the drain can wait for them.
async fn drain_gate(req: axum::extract::Request, next: Next) -> Response {
    let Some(state) = req
        .extensions()
        .get::<std::sync::Arc<crate::app_state::AppState>>()
        .cloned()
    else {
        return next.run(req).await;
    };
    let op = req.uri().path().rsplit('/').next().unwrap_or_default();
    if ANSWERED_WHILE_DRAINING.contains(&op) {
        return next.run(req).await;
    }
    match state.lifecycle.begin_request() {
        Some(_admitted) => next.run(req).await,
        None => ZwiimError::host_restarting().into_response(),
    }
}

async fn unknown_operation() -> ZwiimError {
    ZwiimError::unknown_operation()
}

/// The `/zwiim/v1` router. Nested by `web::router::build_router`, which
/// supplies the `AppState` and `ShutdownSignal` extensions.
pub fn router(token: String, db: DatabaseConnection) -> Router {
    // Operations only the host's operator may perform, never a paired device.
    let local_authority = Router::new()
        .route("/host.claim", post(http::host_claim))
        .route("/device.pair_invite", post(http::device_pair_invite))
        .route("/device.list", post(http::device_list))
        .route("/device.revoke", post(http::device_revoke))
        .route("/cloud.enroll", post(http::cloud_enroll))
        .route("/cloud.status", post(http::cloud_status))
        .route("/lan.status", post(http::lan_status))
        .route("/lan.set", post(http::lan_set))
        .route("/host.restart", post(http::host_restart))
        .route(
            "/host.keep_awake.status",
            post(http::host_keep_awake_status),
        )
        .route("/host.keep_awake.set", post(http::host_keep_awake_set))
        .route("/agent.setup", post(http::agent_setup))
        .route("/agent.sign_in", post(http::agent_sign_in))
        .route("/agent.save_key", post(http::agent_save_key))
        .route("/agent.remove_key", post(http::agent_remove_key))
        .route("/agent.sign_out", post(http::agent_sign_out))
        .route_layer(middleware::from_fn(auth::require_local_authority));

    let authenticated = Router::new()
        .route("/host.hello", post(http::host_hello))
        .route("/host.lan_endpoint", post(http::host_lan_endpoint))
        .route("/host.update.status", post(http::host_update_status))
        .route("/host.diagnostics", post(http::host_diagnostics))
        .route("/host.update.check", post(http::host_update_check))
        .route("/host.update.apply", post(http::host_update_apply))
        .route("/agent.list", post(http::agent_list))
        .route("/agent.auth_status", post(http::agent_auth_status))
        .route("/workspace.list", post(http::workspace_list))
        .route("/workspace.add", post(http::workspace_add))
        .route("/workspace.inspect", post(http::workspace_inspect))
        .route("/workspace.browse", post(http::workspace_browse))
        .route(
            "/workspace.create_folder",
            post(http::workspace_create_folder),
        )
        .route("/workspace.open", post(http::workspace_open))
        .route("/workspace.tree", post(http::workspace_tree))
        .route(
            "/workspace.create_worktree",
            post(http::workspace_create_worktree),
        )
        .route(
            "/workspace.remove_worktree",
            post(http::workspace_remove_worktree),
        )
        .route("/workspace.continue", post(http::workspace_continue))
        .route("/files.read", post(http::files_read))
        .route("/files.write", post(http::files_write))
        .route("/files.create", post(http::files_create))
        .route("/git.status", post(http::git_status))
        .route("/git.log", post(http::git_log))
        .route("/git.branches", post(http::git_branches))
        .route("/git.commit", post(http::git_commit))
        .route("/git.diff", post(http::git_diff))
        .route("/git.pr", post(http::git_pr))
        .route("/git.pr_merge", post(http::git_pr_merge))
        .route("/github.repo_available", post(http::github_repo_available))
        .route("/terminal.spawn", post(http::terminal_spawn))
        .route("/terminal.write", post(http::terminal_write))
        .route("/terminal.resize", post(http::terminal_resize))
        .route("/terminal.kill", post(http::terminal_kill))
        .route("/terminal.list", post(http::terminal_list))
        .route("/terminal.snapshot", post(http::terminal_snapshot))
        .route("/session.start", post(http::session_start))
        .route("/session.send", post(http::session_send))
        .route("/session.cancel", post(http::session_cancel))
        .route("/session.stop", post(http::session_stop))
        .route("/session.delete", post(http::session_delete))
        .route("/session.options", post(http::session_options))
        .route("/session.set_options", post(http::session_set_options))
        .route("/session.respond", post(http::session_respond))
        .route("/session.list", post(http::session_list))
        .route("/session.history", post(http::session_history))
        .route("/ws", get(ws::ws_handler))
        .merge(local_authority)
        .fallback(unknown_operation)
        .layer(middleware::from_fn(move |req, next| {
            auth::require_token(req, next, token.clone(), db.clone())
        }));

    // An unpaired device has no credential; the one-time code in the body is
    // what authorizes pairing.
    Router::new()
        .route("/device.pair", post(http::device_pair))
        .merge(authenticated)
        .layer(middleware::from_fn(drain_gate))
        .layer(middleware::from_fn(normalize_rejections))
}
