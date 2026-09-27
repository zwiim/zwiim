//! Zwiim-owned workspace and session endpoints. Everything crossing this
//! boundary is addressed by `workspace_id` / `session_id`; raw folder and
//! conversation row ids, connection ids, and client-chosen paths never do.

use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::protocol::error::ZwiimError;
use crate::session::{self, SessionInfo};
use crate::workspace::{self, WorkspaceInfo, WorktreeRemoved};

pub async fn workspace_list(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<WorkspaceInfo>>, AppCommandError> {
    Ok(Json(workspace::list(&state.db.conn).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct WorktreeCreateParams {
    /// Root workspace to branch from. No path or branch: the host picks both.
    pub workspace_id: String,
}

pub async fn workspace_worktree_create(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<WorktreeCreateParams>,
) -> Result<Json<WorkspaceInfo>, AppCommandError> {
    Ok(Json(
        workspace::create_worktree(
            &state.db,
            &state.emitter,
            &state.worktrees_dir,
            &params.workspace_id,
            None,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WorktreeRemoveParams {
    pub workspace_id: String,
    /// Discard uncommitted changes in the checkout. Never overrides the
    /// running agent/terminal guard.
    #[serde(default)]
    pub force: bool,
}

pub async fn workspace_worktree_remove(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<WorktreeRemoveParams>,
) -> Result<Json<WorktreeRemoved>, ZwiimError> {
    Ok(Json(
        workspace::remove_worktree(
            &state.db,
            &state.emitter,
            &state.connection_manager,
            &state.terminal_manager,
            &params.workspace_id,
            params.force,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionListParams {
    pub workspace_id: String,
}

pub async fn session_list(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<SessionListParams>,
) -> Result<Json<Vec<SessionInfo>>, AppCommandError> {
    Ok(Json(
        session::list_for_workspace(
            &state.db.conn,
            &state.connection_manager,
            &params.workspace_id,
        )
        .await?,
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::StatusCode;
    use axum_test::TestServer;
    use serde_json::{json, Value};

    use crate::app_state::AppState;
    use crate::web::router::build_router;
    use crate::web::shutdown::ShutdownSignal;

    const HOST_TOKEN: &str = "host-token";

    fn git(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .expect("spawn git");
        assert!(out.status.success(), "git {args:?}");
    }

    #[tokio::test]
    async fn workspace_endpoints_speak_only_zwiim_identities() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
        let data_dir = dir.path().join("data");
        std::fs::create_dir(&data_dir).unwrap();

        let db = crate::db::init_database(&data_dir, "test").await.unwrap();
        crate::db::test_helpers::seed_folder(&db, repo.to_str().unwrap()).await;
        let state = Arc::new(AppState::new_for_test(db, data_dir.clone()));
        let state_db = state.db.conn.clone();
        let server = TestServer::new(build_router(
            state,
            HOST_TOKEN.to_string(),
            data_dir.clone(),
            Arc::new(ShutdownSignal::new()),
        ))
        .unwrap();

        let unauth = server.post("/api/workspace_list").await;
        assert_eq!(unauth.status_code(), StatusCode::UNAUTHORIZED);

        let list: Value = server
            .post("/api/workspace_list")
            .authorization_bearer(HOST_TOKEN)
            .await
            .json();
        let root = &list.as_array().unwrap()[0];
        assert!(root.get("id").is_none() && root.get("folderId").is_none());
        let root_wid = root["workspace_id"].as_str().unwrap().to_string();

        // The client cannot choose where a managed worktree lives.
        let rejected = server
            .post("/api/workspace_worktree_create")
            .authorization_bearer(HOST_TOKEN)
            .json(&json!({ "workspace_id": root_wid, "path": "/tmp/elsewhere" }))
            .await;
        assert!(rejected.status_code().is_client_error());

        let created = server
            .post("/api/workspace_worktree_create")
            .authorization_bearer(HOST_TOKEN)
            .json(&json!({ "workspace_id": root_wid }))
            .await;
        assert_eq!(created.status_code(), StatusCode::OK);
        let wt: Value = created.json();
        assert_eq!(wt["kind"], "worktree");
        assert_eq!(wt["parent_workspace_id"], root_wid.as_str());
        assert!(wt.get("path").is_none(), "absolute host paths stay internal");
        let wt_wid = wt["workspace_id"].as_str().unwrap();
        let wt_folder = crate::workspace::resolve(&state_db, wt_wid).await.unwrap();
        assert!(std::path::Path::new(&wt_folder.path).starts_with(data_dir.join("worktrees")));

        let sessions: Value = server
            .post("/api/session_list")
            .authorization_bearer(HOST_TOKEN)
            .json(&json!({ "workspace_id": wt_wid }))
            .await
            .json();
        assert_eq!(sessions, json!([]));

        let removed = server
            .post("/api/workspace_worktree_remove")
            .authorization_bearer(HOST_TOKEN)
            .json(&json!({ "workspace_id": wt_wid }))
            .await;
        assert_eq!(removed.status_code(), StatusCode::OK);
        let removed: Value = removed.json();
        assert_eq!(removed["parent_workspace_id"], root_wid.as_str());
        assert_eq!(removed["kept_sessions"], 0);

        // A removed worktree with no sessions has nothing left to read.
        let gone = server
            .post("/api/session_list")
            .authorization_bearer(HOST_TOKEN)
            .json(&json!({ "workspace_id": wt_wid }))
            .await;
        assert_eq!(gone.status_code(), StatusCode::OK);
        assert_eq!(gone.json::<Value>(), json!([]));
    }
}
