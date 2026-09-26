use std::sync::Arc;

use axum::{
    extract::{DefaultBodyLimit, Extension},
    http::{StatusCode, Uri},
    middleware::{self, Next},
    response::IntoResponse,
    routing::{any, get, post},
    Json, Router,
};

use crate::web::handlers::files::UPLOAD_MAX_BYTES;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

use super::shutdown::ShutdownSignal;
use super::{auth, handlers, ws};
use crate::app_state::AppState;
use tracing::Instrument;

/// Origins of the local desktop client: the Tauri webview (`tauri://localhost`
/// on macOS/Linux, `http(s)://tauri.localhost` on Windows) and loopback HTTP
/// origins used by its development server.
fn is_local_client_origin(origin: &str) -> bool {
    if matches!(
        origin,
        "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost"
    ) {
        return true;
    }
    let Some(rest) = origin.strip_prefix("http://") else {
        return false;
    };
    let host = match rest.strip_prefix("[::1]") {
        Some(port) => return port.is_empty() || is_port_suffix(port),
        None => rest.split(':').next().unwrap_or_default(),
    };
    matches!(host, "localhost" | "127.0.0.1") && {
        let port = &rest[host.len()..];
        port.is_empty() || is_port_suffix(port)
    }
}

fn is_port_suffix(s: &str) -> bool {
    s.strip_prefix(':')
        .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

pub fn build_router(
    state: Arc<AppState>,
    token: String,
    static_dir: std::path::PathBuf,
    shutdown_signal: Arc<ShutdownSignal>,
) -> Router {
    // Cross-origin callers are limited to the local desktop client (the Tauri
    // webview, or its dev server on loopback). The bundled web UI is served
    // same-origin and needs no CORS.
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            origin.to_str().is_ok_and(is_local_client_origin)
        }))
        .allow_methods(Any)
        .allow_headers(Any);

    let token_for_ws = token.clone();
    let zwiim_v1 = crate::protocol::router(token.clone(), state.db.conn.clone());
    let db_for_api = state.db.conn.clone();
    let db_for_ws = state.db.conn.clone();

    let api = Router::new()
        .route("/health", post(health_check))
        // Debug endpoint: operator-facing snapshot of `EventBusMetrics`
        // (emit volume, lag/eviction counts, attach decision counts).
        // Sits behind the same auth middleware as every other route.
        .route(
            "/debug/event_metrics",
            get(handlers::event_metrics::get_event_metrics),
        )
        // ─── Conversations ───
        .route(
            "/list_conversations",
            post(handlers::conversations::list_conversations),
        )
        .route(
            "/get_conversation",
            post(handlers::conversations::get_conversation),
        )
        .route(
            "/list_all_conversations",
            post(handlers::conversations::list_all_conversations),
        )
        .route(
            "/list_child_conversations",
            post(handlers::conversations::list_child_conversations),
        )
        .route(
            "/get_delegation_settings",
            post(handlers::delegation::get_delegation_settings),
        )
        .route(
            "/set_delegation_settings",
            post(handlers::delegation::set_delegation_settings),
        )
        .route(
            "/get_zwiim_mcp_service_status",
            post(handlers::mcp_service::get_zwiim_mcp_service_status),
        )
        .route(
            "/start_zwiim_mcp_service",
            post(handlers::mcp_service::start_zwiim_mcp_service),
        )
        .route(
            "/set_zwiim_mcp_tool_group",
            post(handlers::mcp_service::set_zwiim_mcp_tool_group),
        )
        .route(
            "/get_feedback_settings",
            post(handlers::feedback::get_feedback_settings),
        )
        .route(
            "/set_feedback_settings",
            post(handlers::feedback::set_feedback_settings),
        )
        .route(
            "/submit_session_feedback",
            post(handlers::feedback::submit_session_feedback),
        )
        .route(
            "/get_question_settings",
            post(handlers::question::get_question_settings),
        )
        .route(
            "/set_question_settings",
            post(handlers::question::set_question_settings),
        )
        .route(
            "/get_session_info_settings",
            post(handlers::session_info::get_session_info_settings),
        )
        .route(
            "/set_session_info_settings",
            post(handlers::session_info::set_session_info_settings),
        )
        .route(
            "/get_folder_conversation",
            post(handlers::conversations::get_folder_conversation),
        )
        .route(
            "/get_folder_conversation_turns",
            post(handlers::conversations::get_folder_conversation_turns),
        )
        .route(
            "/list_opened_tabs",
            post(handlers::conversations::list_opened_tabs),
        )
        .route(
            "/save_opened_tabs",
            post(handlers::conversations::save_opened_tabs),
        )
        .route(
            "/import_local_conversations",
            post(handlers::conversations::import_local_conversations),
        )
        .route(
            "/scan_importable_sessions",
            post(handlers::conversations::scan_importable_sessions),
        )
        .route(
            "/import_selected_sessions",
            post(handlers::conversations::import_selected_sessions),
        )
        .route("/list_folders", post(handlers::conversations::list_folders))
        .route("/get_stats", post(handlers::conversations::get_stats))
        .route(
            "/get_sidebar_data",
            post(handlers::conversations::get_sidebar_data),
        )
        .route(
            "/create_conversation",
            post(handlers::conversations::create_conversation),
        )
        .route(
            "/create_chat_conversation",
            post(handlers::conversations::create_chat_conversation),
        )
        .route(
            "/create_chat_dir",
            post(handlers::conversations::create_chat_dir),
        )
        .route(
            "/update_conversation_status",
            post(handlers::conversations::update_conversation_status),
        )
        .route(
            "/update_conversation_title",
            post(handlers::conversations::update_conversation_title),
        )
        .route(
            "/update_conversation_pinned",
            post(handlers::conversations::update_conversation_pinned),
        )
        .route(
            "/delete_conversation",
            post(handlers::conversations::delete_conversation),
        )
        // ─── Folders ───
        .route(
            "/load_folder_history",
            post(handlers::folders::load_folder_history),
        )
        .route(
            "/list_open_folders",
            post(handlers::folders::list_open_folders),
        )
        .route(
            "/list_open_folder_details",
            post(handlers::folders::list_open_folder_details),
        )
        .route(
            "/list_all_folder_details",
            post(handlers::folders::list_all_folder_details),
        )
        .route("/get_folder", post(handlers::folders::get_folder))
        .route("/open_folder", post(handlers::folders::open_folder))
        .route("/open_in_code", post(handlers::folders::open_in_code))
        .route(
            "/open_worktree_folder",
            post(handlers::folders::open_worktree_folder),
        )
        .route(
            "/resolve_worktree_folder",
            post(handlers::folders::resolve_worktree_folder),
        )
        .route(
            "/open_folder_in_workspace",
            post(handlers::folders::open_folder_in_workspace),
        )
        .route(
            "/open_folder_by_id",
            post(handlers::folders::open_folder_by_id),
        )
        .route(
            "/remove_folder_from_workspace",
            post(handlers::folders::remove_folder_from_workspace),
        )
        .route(
            "/list_folder_groups",
            post(handlers::folders::list_folder_groups),
        )
        .route(
            "/create_folder_group",
            post(handlers::folders::create_folder_group),
        )
        .route(
            "/update_folder_group",
            post(handlers::folders::update_folder_group),
        )
        .route(
            "/delete_folder_group",
            post(handlers::folders::delete_folder_group),
        )
        .route(
            "/apply_sidebar_layout",
            post(handlers::folders::apply_sidebar_layout),
        )
        .route(
            "/set_folder_group",
            post(handlers::folders::set_folder_group),
        )
        .route(
            "/update_folder_color",
            post(handlers::folders::update_folder_color),
        )
        .route(
            "/update_folder_alias",
            post(handlers::folders::update_folder_alias),
        )
        .route(
            "/update_folder_default_agent",
            post(handlers::folders::update_folder_default_agent),
        )
        .route(
            "/list_folder_links",
            post(handlers::folder_links::list_folder_links),
        )
        .route(
            "/preview_folder_links",
            post(handlers::folder_links::preview_folder_links),
        )
        .route(
            "/create_folder_links",
            post(handlers::folder_links::create_folder_links),
        )
        .route(
            "/rename_folder_link",
            post(handlers::folder_links::rename_folder_link),
        )
        .route(
            "/repair_folder_link",
            post(handlers::folder_links::repair_folder_link),
        )
        .route(
            "/remove_folder_link",
            post(handlers::folder_links::remove_folder_link),
        )
        // ─── Canvas ───
        .route(
            "/add_folder_to_history",
            post(handlers::folders::add_folder_to_history),
        )
        .route(
            "/remove_folder_from_history",
            post(handlers::folders::remove_folder_from_history),
        )
        .route(
            "/create_folder_directory",
            post(handlers::folders::create_folder_directory),
        )
        .route("/get_git_branch", post(handlers::folders::get_git_branch))
        .route("/get_git_head", post(handlers::folders::get_git_head))
        .route(
            "/get_home_directory",
            post(handlers::folders::get_home_directory),
        )
        .route(
            "/list_directory_entries",
            post(handlers::folders::list_directory_entries),
        )
        .route(
            "/list_directory_with_files",
            post(handlers::folders::list_directory_with_files),
        )
        .route("/get_file_tree", post(handlers::folders::get_file_tree))
        .route(
            "/list_workspace_files",
            post(handlers::folders::list_workspace_files),
        )
        .route(
            "/start_workspace_state_stream",
            post(handlers::workspace_state::start_workspace_state_stream),
        )
        .route(
            "/stop_workspace_state_stream",
            post(handlers::workspace_state::stop_workspace_state_stream),
        )
        .route(
            "/get_workspace_snapshot",
            post(handlers::workspace_state::get_workspace_snapshot),
        )
        // ─── Window navigation ───
        .route(
            "/open_settings_window",
            post(handlers::folders::open_settings_window),
        )
        .route(
            "/open_commit_window",
            post(handlers::folders::open_commit_window),
        )
        .route(
            "/open_import_sessions_window",
            post(handlers::folders::open_import_sessions_window),
        )
        .route(
            "/open_merge_window",
            post(handlers::folders::open_merge_window),
        )
        .route(
            "/open_stash_window",
            post(handlers::folders::open_stash_window),
        )
        .route(
            "/open_push_window",
            post(handlers::folders::open_push_window),
        )
        // ─── Git (pure) ───
        .route("/git_status", post(handlers::git::git_status))
        .route("/git_init", post(handlers::git::git_init))
        .route("/git_log", post(handlers::git::git_log))
        .route("/git_current_user", post(handlers::git::git_current_user))
        .route("/git_commit_files", post(handlers::git::git_commit_files))
        .route(
            "/git_search_authors",
            post(handlers::git::git_search_authors),
        )
        .route(
            "/git_list_all_branches",
            post(handlers::git::git_list_all_branches),
        )
        .route("/git_list_branches", post(handlers::git::git_list_branches))
        .route(
            "/git_commit_branches",
            post(handlers::git::git_commit_branches),
        )
        .route("/git_show_file", post(handlers::git::git_show_file))
        .route(
            "/git_show_file_base64",
            post(handlers::git::git_show_file_base64),
        )
        .route("/git_diff", post(handlers::git::git_diff))
        .route(
            "/git_diff_with_branch",
            post(handlers::git::git_diff_with_branch),
        )
        .route("/git_show_diff", post(handlers::git::git_show_diff))
        .route("/git_list_remotes", post(handlers::git::git_list_remotes))
        .route("/git_add_remote", post(handlers::git::git_add_remote))
        .route("/git_remove_remote", post(handlers::git::git_remove_remote))
        .route(
            "/git_set_remote_url",
            post(handlers::git::git_set_remote_url),
        )
        .route("/git_new_branch", post(handlers::git::git_new_branch))
        .route("/git_checkout", post(handlers::git::git_checkout))
        .route("/git_reset", post(handlers::git::git_reset))
        .route("/git_merge", post(handlers::git::git_merge))
        .route("/git_rebase", post(handlers::git::git_rebase))
        .route("/git_worktree_add", post(handlers::git::git_worktree_add))
        .route("/git_push_info", post(handlers::git::git_push_info))
        .route(
            "/git_start_pull_merge",
            post(handlers::git::git_start_pull_merge),
        )
        .route(
            "/git_has_merge_head",
            post(handlers::git::git_has_merge_head),
        )
        .route("/git_is_tracked", post(handlers::git::git_is_tracked))
        .route("/git_rollback_file", post(handlers::git::git_rollback_file))
        .route("/git_add_files", post(handlers::git::git_add_files))
        .route(
            "/git_list_conflicts",
            post(handlers::git::git_list_conflicts),
        )
        .route(
            "/git_conflict_file_versions",
            post(handlers::git::git_conflict_file_versions),
        )
        .route(
            "/git_resolve_conflict",
            post(handlers::git::git_resolve_conflict),
        )
        .route(
            "/git_abort_operation",
            post(handlers::git::git_abort_operation),
        )
        .route(
            "/git_continue_operation",
            post(handlers::git::git_continue_operation),
        )
        .route("/git_stash_push", post(handlers::git::git_stash_push))
        .route("/git_stash_pop", post(handlers::git::git_stash_pop))
        .route("/git_stash_list", post(handlers::git::git_stash_list))
        .route("/git_stash_apply", post(handlers::git::git_stash_apply))
        .route("/git_stash_drop", post(handlers::git::git_stash_drop))
        .route("/git_stash_clear", post(handlers::git::git_stash_clear))
        .route("/git_stash_show", post(handlers::git::git_stash_show))
        // ─── Git (remote) ───
        .route("/git_pull", post(handlers::git::git_pull))
        .route("/git_push", post(handlers::git::git_push))
        .route("/git_fetch", post(handlers::git::git_fetch))
        .route(
            "/git_update_branch",
            post(handlers::git::git_update_branch),
        )
        .route("/git_commit", post(handlers::git::git_commit))
        .route("/git_fetch_remote", post(handlers::git::git_fetch_remote))
        .route("/git_delete_branch", post(handlers::git::git_delete_branch))
        .route(
            "/git_remove_worktree",
            post(handlers::git::git_remove_worktree),
        )
        .route(
            "/git_delete_remote_branch",
            post(handlers::git::git_delete_remote_branch),
        )
        .route("/clone_repository", post(handlers::git::clone_repository))
        // ─── Files ───
        .route(
            "/read_file_preview",
            post(handlers::files::read_file_preview),
        )
        .route("/read_file_base64", post(handlers::files::read_file_base64))
        .route(
            "/read_workspace_file_base64",
            post(handlers::files::read_workspace_file_base64),
        )
        .route(
            "/read_file_for_edit",
            post(handlers::files::read_file_for_edit),
        )
        .route(
            "/save_file_content",
            post(handlers::files::save_file_content),
        )
        .route("/save_file_copy", post(handlers::files::save_file_copy))
        .route(
            "/rename_file_tree_entry",
            post(handlers::files::rename_file_tree_entry),
        )
        .route(
            "/move_file_tree_entry",
            post(handlers::files::move_file_tree_entry),
        )
        .route(
            "/delete_file_tree_entry",
            post(handlers::files::delete_file_tree_entry),
        )
        .route(
            "/create_file_tree_entry",
            post(handlers::files::create_file_tree_entry),
        )
        .route(
            "/upload_attachment",
            // `UPLOAD_MAX_BYTES` is the *file payload* limit; the raw
            // multipart body also carries boundary markers, the
            // `Content-Disposition` headers, and the `session_id` field —
            // ~256-512 bytes of overhead. Without this layer, axum's default
            // 2MiB `DefaultBodyLimit` would reject anything bigger before our
            // handler ever sees a chunk. Pad by 64KiB so the handler's own
            // chunk-summing check (in `files.rs`) stays the authoritative
            // size boundary.
            post(handlers::files::upload_attachment)
                .layer(DefaultBodyLimit::max(UPLOAD_MAX_BYTES as usize + 64 * 1024)),
        )
        // ─── Workspace files (web upload/download) ───
        //
        // Issue #179: when Zwiim runs in server mode the user has no
        // native file dialog, so they need HTTP endpoints to move files
        // between the browser and the workspace. The upload handler
        // streams to a same-dir staging file then renames into place;
        // the download handlers stream files and walk-then-zip whole
        // directories. Disable axum's default multipart body limit here:
        // workspace files are user-owned bytes moving between the user's
        // browser and filesystem, not model-context attachments.
        .route(
            "/upload_workspace_file",
            post(handlers::workspace_files::upload_workspace_file)
                .layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/workspace_download_ticket",
            post(handlers::workspace_files::create_download_ticket),
        )
        // ─── Backup & restore ───
        //
        // Export builds an archive and returns a download ticket; restore
        // uploads the archive once (body limit disabled — it can be large),
        // then inspects + stages it by reference. The data swap happens on the
        // next process start; the client follows up with `restart_app`.
        // ─── Configuration sync ───
        //
        // The WebDAV half is runtime-agnostic. Local file transfer is the
        // by-content pair: a browser has no path to name, and the payload is
        // tens of KB, so it travels in the JSON body rather than through the
        // upload-staging machinery above.
        .route(
            "/download_workspace_file",
            post(handlers::workspace_files::download_workspace_file),
        )
        .route(
            "/download_workspace_dir",
            post(handlers::workspace_files::download_workspace_dir),
        )
        // ─── Folder commands ───
        .route(
            "/list_folder_commands",
            post(handlers::folder_commands::list_folder_commands),
        )
        .route(
            "/create_folder_command",
            post(handlers::folder_commands::create_folder_command),
        )
        .route(
            "/update_folder_command",
            post(handlers::folder_commands::update_folder_command),
        )
        .route(
            "/delete_folder_command",
            post(handlers::folder_commands::delete_folder_command),
        )
        .route(
            "/reorder_folder_commands",
            post(handlers::folder_commands::reorder_folder_commands),
        )
        .route(
            "/bootstrap_folder_commands_from_package_json",
            post(handlers::folder_commands::bootstrap_folder_commands_from_package_json),
        )
        // ─── MCP ───
        .route("/mcp_scan_local", post(handlers::mcp::mcp_scan_local))
        .route(
            "/mcp_list_marketplaces",
            post(handlers::mcp::mcp_list_marketplaces),
        )
        .route(
            "/mcp_search_marketplace",
            post(handlers::mcp::mcp_search_marketplace),
        )
        .route(
            "/mcp_get_marketplace_server_detail",
            post(handlers::mcp::mcp_get_marketplace_server_detail),
        )
        .route(
            "/mcp_install_from_marketplace",
            post(handlers::mcp::mcp_install_from_marketplace),
        )
        .route(
            "/mcp_upsert_local_server",
            post(handlers::mcp::mcp_upsert_local_server),
        )
        .route(
            "/mcp_set_server_apps",
            post(handlers::mcp::mcp_set_server_apps),
        )
        .route("/mcp_remove_server", post(handlers::mcp::mcp_remove_server))
        // ─── Version control settings ───
        .route("/detect_git", post(handlers::version_control::detect_git))
        .route(
            "/test_git_path",
            post(handlers::version_control::test_git_path),
        )
        .route(
            "/get_git_settings",
            post(handlers::version_control::get_git_settings),
        )
        .route(
            "/update_git_settings",
            post(handlers::version_control::update_git_settings),
        )
        .route(
            "/get_github_accounts",
            post(handlers::version_control::get_github_accounts),
        )
        .route(
            "/update_github_accounts",
            post(handlers::version_control::update_github_accounts),
        )
        .route(
            "/validate_github_token",
            post(handlers::version_control::validate_github_token),
        )
        .route(
            "/validate_gitlab_token",
            post(handlers::version_control::validate_gitlab_token),
        )
        .route(
            "/validate_gitea_token",
            post(handlers::version_control::validate_gitea_token),
        )
        .route(
            "/save_account_token",
            post(handlers::version_control::save_account_token),
        )
        .route(
            "/get_account_token",
            post(handlers::version_control::get_account_token),
        )
        .route(
            "/delete_account_token",
            post(handlers::version_control::delete_account_token),
        )
        // ─── System settings ───
        .route(
            "/get_system_proxy_settings",
            post(handlers::system_settings::get_system_proxy_settings),
        )
        .route(
            "/get_system_terminal_settings",
            post(handlers::system_settings::get_system_terminal_settings),
        )
        .route(
            "/get_available_terminal_shells",
            post(handlers::system_settings::get_available_terminal_shells),
        )
        .route(
            "/probe_terminal_shell_path",
            post(handlers::system_settings::probe_terminal_shell_path),
        )
        .route(
            "/update_system_proxy_settings",
            post(handlers::system_settings::update_system_proxy_settings),
        )
        .route(
            "/update_system_language_settings",
            post(handlers::system_settings::update_system_language_settings),
        )
        .route(
            "/update_system_terminal_settings",
            post(handlers::system_settings::update_system_terminal_settings),
        )
        // ─── Logging ───
        .route(
            "/get_log_settings",
            post(handlers::logging::get_log_settings),
        )
        .route(
            "/set_log_settings",
            post(handlers::logging::set_log_settings),
        )
        .route("/get_recent_logs", post(handlers::logging::get_recent_logs))
        .route("/list_log_files", post(handlers::logging::list_log_files))
        .route("/read_log_file", post(handlers::logging::read_log_file))
        // ─── ACP ───
        .route(
            "/acp_get_agent_status",
            post(handlers::acp::acp_get_agent_status),
        )
        .route("/acp_list_agents", post(handlers::acp::acp_list_agents))
        .route(
            "/acp_env_diagnostics",
            post(handlers::acp::acp_env_diagnostics),
        )
        .route("/acp_connect", post(handlers::acp::acp_connect))
        .route("/acp_disconnect", post(handlers::acp::acp_disconnect))
        .route(
            "/acp_touch_connection",
            post(handlers::acp::acp_touch_connection),
        )
        .route("/acp_prompt", post(handlers::acp::acp_prompt))
        .route("/acp_preflight", post(handlers::acp::acp_preflight))
        .route("/acp_set_mode", post(handlers::acp::acp_set_mode))
        .route(
            "/acp_set_config_option",
            post(handlers::acp::acp_set_config_option),
        )
        .route(
            "/acp_goal_control",
            post(handlers::acp::acp_goal_control),
        )
        .route(
            "/acp_describe_agent_options",
            post(handlers::acp::acp_describe_agent_options),
        )
        .route("/acp_cancel", post(handlers::acp::acp_cancel))
        .route("/acp_fork", post(handlers::acp::acp_fork))
        .route(
            "/acp_stop_async_task",
            post(handlers::acp::acp_stop_async_task),
        )
        .route(
            "/acp_respond_permission",
            post(handlers::acp::acp_respond_permission),
        )
        .route(
            "/acp_answer_question",
            post(handlers::acp::acp_answer_question),
        )
        .route(
            "/acp_answer_plan_approval",
            post(handlers::acp::acp_answer_plan_approval),
        )
        .route(
            "/acp_list_connections",
            post(handlers::acp::acp_list_connections),
        )
        .route(
            "/acp_get_session_snapshot",
            post(handlers::acp::acp_get_session_snapshot),
        )
        .route(
            "/acp_get_session_snapshot_by_conversation",
            post(handlers::acp::acp_get_session_snapshot_by_conversation),
        )
        .route(
            "/acp_find_connection_for_conversation",
            post(handlers::acp::acp_find_connection_for_conversation),
        )
        .route(
            "/acp_clear_binary_cache",
            post(handlers::acp::acp_clear_binary_cache),
        )
        .route(
            "/acp_scan_leaked_temp",
            post(handlers::acp::acp_scan_leaked_temp),
        )
        .route(
            "/acp_reclaim_leaked_temp",
            post(handlers::acp::acp_reclaim_leaked_temp),
        )
        .route(
            "/acp_update_agent_preferences",
            post(handlers::acp::acp_update_agent_preferences),
        )
        .route(
            "/acp_update_agent_env",
            post(handlers::acp::acp_update_agent_env),
        )
        .route(
            "/acp_update_agent_config",
            post(handlers::acp::acp_update_agent_config),
        )
        .route(
            "/acp_update_hermes_config",
            post(handlers::acp::acp_update_hermes_config),
        )
        .route(
            "/acp_cursor_auth_status",
            post(handlers::acp::acp_cursor_auth_status),
        )
        .route(
            "/acp_cursor_list_models",
            post(handlers::acp::acp_cursor_list_models),
        )
        .route(
            "/acp_qoder_auth_status",
            post(handlers::acp::acp_qoder_auth_status),
        )
        .route(
            "/acp_update_kimi_code_config",
            post(handlers::acp::acp_update_kimi_code_config),
        )
        .route(
            "/acp_fetch_kimi_models",
            post(handlers::acp::acp_fetch_kimi_models),
        )
        .route(
            "/acp_update_pi_config",
            post(handlers::acp::acp_update_pi_config),
        )
        .route(
            "/acp_load_pi_config",
            post(handlers::acp::acp_load_pi_config),
        )
        .route(
            "/acp_load_deepseek_model_catalog",
            post(handlers::acp::acp_load_deepseek_model_catalog),
        )
        .route(
            "/acp_update_deepseek_model_catalog",
            post(handlers::acp::acp_update_deepseek_model_catalog),
        )
        .route(
            "/acp_validate_pi_command",
            post(handlers::acp::acp_validate_pi_command),
        )
        .route(
            "/acp_sync_antigravity_settings",
            post(handlers::acp::acp_sync_antigravity_settings),
        )
        .route(
            "/acp_antigravity_login_start",
            post(handlers::acp::acp_antigravity_login_start),
        )
        .route(
            "/acp_antigravity_login_finish",
            post(handlers::acp::acp_antigravity_login_finish),
        )
        .route(
            "/acp_antigravity_login_cancel",
            post(handlers::acp::acp_antigravity_login_cancel),
        )
        .route(
            "/acp_antigravity_sign_out",
            post(handlers::acp::acp_antigravity_sign_out),
        )
        .route(
            "/acp_pi_project_trust_state",
            post(handlers::acp::acp_pi_project_trust_state),
        )
        .route(
            "/acp_pi_set_project_trust",
            post(handlers::acp::acp_pi_set_project_trust),
        )
        .route(
            "/acp_pi_acknowledge_project_trust",
            post(handlers::acp::acp_pi_acknowledge_project_trust),
        )
        .route(
            "/acp_pi_list_trust_entries",
            post(handlers::acp::acp_pi_list_trust_entries),
        )
        .route(
            "/acp_install_pi_binary",
            post(handlers::acp::acp_install_pi_binary),
        )
        .route(
            "/acp_uninstall_pi_binary",
            post(handlers::acp::acp_uninstall_pi_binary),
        )
        .route(
            "/acp_download_agent_binary",
            post(handlers::acp::acp_download_agent_binary),
        )
        .route(
            "/acp_install_uv_tool",
            post(handlers::acp::acp_install_uv_tool),
        )
        .route(
            "/acp_detect_agent_local_version",
            post(handlers::acp::acp_detect_agent_local_version),
        )
        .route(
            "/acp_prepare_npx_agent",
            post(handlers::acp::acp_prepare_npx_agent),
        )
        .route(
            "/acp_uninstall_agent",
            post(handlers::acp::acp_uninstall_agent),
        )
        .route(
            "/acp_reorder_agents",
            post(handlers::acp::acp_reorder_agents),
        )
        .route(
            "/acp_list_custom_agents",
            post(handlers::acp::acp_list_custom_agents),
        )
        .route(
            "/acp_save_custom_agent",
            post(handlers::acp::acp_save_custom_agent),
        )
        .route(
            "/acp_delete_custom_agent",
            post(handlers::acp::acp_delete_custom_agent),
        )
        .route(
            "/acp_fetch_registry_catalog",
            post(handlers::acp::acp_fetch_registry_catalog),
        )
        .route(
            "/acp_add_registry_agent",
            post(handlers::acp::acp_add_registry_agent),
        )
        .route(
            "/acp_current_platform",
            post(handlers::acp::acp_current_platform),
        )
        .route(
            "/acp_list_agent_skills",
            post(handlers::acp::acp_list_agent_skills),
        )
        .route(
            "/acp_read_agent_skill",
            post(handlers::acp::acp_read_agent_skill),
        )
        .route(
            "/acp_save_agent_skill",
            post(handlers::acp::acp_save_agent_skill),
        )
        .route(
            "/acp_delete_agent_skill",
            post(handlers::acp::acp_delete_agent_skill),
        )
        .route(
            "/opencode_list_plugins",
            post(handlers::acp::opencode_list_plugins),
        )
        .route(
            "/opencode_provider_catalog",
            post(handlers::acp::opencode_provider_catalog),
        )
        .route(
            "/codex_bundled_catalog",
            post(handlers::acp::codex_bundled_catalog),
        )
        .route(
            "/opencode_install_plugins",
            post(handlers::acp::opencode_install_plugins),
        )
        .route(
            "/opencode_uninstall_plugin",
            post(handlers::acp::opencode_uninstall_plugin),
        )
        .route(
            "/codex_request_device_code",
            post(handlers::acp::codex_request_device_code),
        )
        .route(
            "/codex_poll_device_code",
            post(handlers::acp::codex_poll_device_code),
        )
        // ─── Experts ───
        // ─── Science ───
        // ─── Custom skills ───
        // ─── Office tools ───
        // ─── Web-mode port bridge (dev servers on the host, shown in an iframe) ───
        // ─── Project boot ───
        .route(
            "/detect_package_manager",
            post(handlers::project_boot::detect_package_manager),
        )
        .route(
            "/create_shadcn_project",
            post(handlers::project_boot::create_shadcn_project),
        )
        .route(
            "/detect_hyperframes_skills",
            post(handlers::project_boot::detect_hyperframes_skills),
        )
        .route(
            "/install_hyperframes_skills",
            post(handlers::project_boot::install_hyperframes_skills),
        )
        .route(
            "/create_hyperframes_project",
            post(handlers::project_boot::create_hyperframes_project),
        )
        // ─── Web Server ───
        .route(
            "/get_web_server_status",
            post(handlers::web_server::get_web_server_status),
        )
        .route(
            "/get_web_service_config",
            post(handlers::web_server::get_web_service_config),
        )
        .route(
            "/update_web_service_config",
            post(handlers::web_server::update_web_service_config),
        )
        .route(
            "/start_web_server",
            post(handlers::web_server::start_web_server),
        )
        .route(
            "/stop_web_server",
            post(handlers::web_server::stop_web_server),
        )
        .route(
            "/probe_web_service_port",
            post(handlers::web_server::probe_web_service_port),
        )
        // ─── Model Providers ───
        .route(
            "/list_model_providers",
            post(handlers::model_provider::list_model_providers),
        )
        .route(
            "/create_model_provider",
            post(handlers::model_provider::create_model_provider),
        )
        .route(
            "/update_model_provider",
            post(handlers::model_provider::update_model_provider),
        )
        .route(
            "/delete_model_provider",
            post(handlers::model_provider::delete_model_provider),
        )
        // ─── Quick Messages ───
        // ─── Automations ───
        // ─── Token usage dashboard ───
        // ─── Work tasks ───
        // ─── Workspace background ───
        // ─── Pet ───
        // ─── Terminal ───
        .route("/terminal_spawn", post(handlers::terminal::terminal_spawn))
        .route("/terminal_write", post(handlers::terminal::terminal_write))
        .route(
            "/terminal_resize",
            post(handlers::terminal::terminal_resize),
        )
        .route(
            "/terminal_snapshot",
            post(handlers::terminal::terminal_snapshot),
        )
        .route("/terminal_kill", post(handlers::terminal::terminal_kill))
        .route("/terminal_list", post(handlers::terminal::terminal_list))
        // ─── Identity (host id, pairing invitations, trusted devices) ───
        .route("/host_identity", post(handlers::identity::host_identity))
        // Device management stays with the host's operator, never a paired
        // device (matches `device.*` on `/zwiim/v1`).
        .merge(
            Router::new()
                .route(
                    "/pairing_invitation_create",
                    post(handlers::identity::pairing_invitation_create),
                )
                .route(
                    "/trusted_device_list",
                    post(handlers::identity::trusted_device_list),
                )
                .route(
                    "/trusted_device_revoke",
                    post(handlers::identity::trusted_device_revoke),
                )
                .route_layer(middleware::from_fn(auth::require_local_authority)),
        )
        // ─── Workspaces + sessions (Zwiim identities only) ───
        .route("/workspace_list", post(handlers::workspace::workspace_list))
        .route(
            "/workspace_worktree_create",
            post(handlers::workspace::workspace_worktree_create),
        )
        .route(
            "/workspace_worktree_remove",
            post(handlers::workspace::workspace_worktree_remove),
        )
        .route("/session_list", post(handlers::workspace::session_list))
        // Catch-all
        .fallback(api_not_found)
        .layer(middleware::from_fn(move |req, next| {
            auth::require_token(req, next, token.clone(), db_for_api.clone())
        }));

    // Public endpoints — no token required.
    // The login page needs to read the user's preferred language before
    // authenticating so it can render in their chosen locale.
    let public_api = Router::new()
        .route(
            "/get_system_language_settings",
            post(handlers::system_settings::get_system_language_settings),
        )
        .route(
            "/workspace_download/{ticket}",
            get(handlers::workspace_files::consume_download_ticket),
        )
        // An unpaired device has no credential yet; the one-time pairing
        // code in the body is what authorizes this call.
        .route("/pairing_claim", post(handlers::identity::pairing_claim));


    // Wrap every API request in an `http` span (method, path, request id) so a
    // single request's logs — including auth rejections — are correlatable in
    // the viewer. `.instrument()` the downstream future; never `.enter()` across
    // an await, which corrupts the span stack.
    //
    // Layer order: the protected router's `auth::require_token` layer was added
    // (above) BEFORE this `.layer()`, and this layer is applied to the MERGED
    // router, so it is the OUTERMOST layer. The instrumented `next.run(req)`
    // future therefore wraps routing + auth + handler — auth-reject logs land
    // inside the `http` span.
    let api = public_api.merge(api).layer(middleware::from_fn(
        |req: axum::extract::Request, next: Next| async move {
            let method = req.method().clone();
            let path = req.uri().path().to_string();
            let request_id = uuid::Uuid::new_v4();
            let span = tracing::info_span!("http", %method, %path, %request_id);
            next.run(req).instrument(span).await
        },
    ));

    // WebSocket route (auth via Sec-WebSocket-Protocol)
    let ws_route = Router::new()
        .route("/ws/events", get(ws::ws_handler))
        .layer(middleware::from_fn(move |req, next| {
            auth::require_token(req, next, token_for_ws.clone(), db_for_ws.clone())
        }));

    // Static file serving.
    // Next.js static export produces "folder.html" for "/folder" route.
    // We use a middleware to rewrite "/folder" → "/folder.html" before ServeDir.
    let fallback =
        ServeDir::new(&static_dir).fallback(ServeFile::new(static_dir.join("index.html")));

    let static_dir_for_mw = static_dir.clone();
    let html_rewrite = middleware::from_fn(move |req: axum::extract::Request, next: Next| {
        let dir = static_dir_for_mw.clone();
        async move {
            let path = req.uri().path();
            // If path has no extension (not a file) and a .html version exists, rewrite
            if path != "/"
                && !path.contains('.')
                && !path.starts_with("/api")
                && !path.starts_with("/ws")
                && !path.starts_with("/zwiim")
            {
                let html_path = format!("{}.html", path.trim_end_matches('/'));
                let html_file = dir.join(html_path.trim_start_matches('/'));
                if html_file.exists() {
                    // Rebuild URI with .html suffix preserving query string
                    let new_path = if let Some(q) = req.uri().query() {
                        format!("{}?{}", html_path, q)
                    } else {
                        html_path
                    };
                    if let Ok(new_uri) = new_path.parse::<Uri>() {
                        let (mut parts, body) = req.into_parts();
                        parts.uri = new_uri;
                        let req = axum::extract::Request::from_parts(parts, body);
                        return next.run(req).await;
                    }
                }
            }
            next.run(req).await
        }
    });

    Router::new()
        .nest("/api", api)
        .nest("/zwiim/v1", zwiim_v1)
        .merge(ws_route)
        .fallback_service(fallback)
        .layer(html_rewrite)
        .layer(cors)
        .layer(Extension(state))
        .layer(Extension(shutdown_signal))
        // Outermost: compress API JSON and static text assets. Allowlist
        // predicate — binary downloads keep their exact Content-Length (the
        // remote proxy's progress source) and SSE stays unbuffered; see
        // `web::compression`.
        .layer(crate::web::compression::compression_layer())
}

async fn health_check() -> impl IntoResponse {
    // Include the running version so the upgrade UI can confirm — using only a
    // local signal — that a restart actually landed on the new version (and
    // wasn't auto-rolled-back by the supervisor) without depending on the
    // remote update manifest.
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn api_not_found(uri: axum::http::Uri) -> impl IntoResponse {
    let command = uri.path().trim_start_matches('/');
    tracing::info!("[WEB] Unimplemented API endpoint: {}", command);
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "code": "not_implemented",
            "message": format!("API endpoint '{}' is not available in web mode", command),
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::build_router;
    use axum::http::StatusCode;
    use crate::app_state::AppState;
    use crate::db::test_helpers::fresh_in_memory_db;
    use crate::web::shutdown::ShutdownSignal;
    use axum_test::TestServer;
    use std::sync::Arc;

    #[test]
    fn only_local_desktop_origins_are_allowed() {
        use super::is_local_client_origin as ok;
        for origin in [
            "tauri://localhost",
            "http://tauri.localhost",
            "https://tauri.localhost",
            "http://localhost:1420",
            "http://127.0.0.1:1420",
            "http://[::1]:5173",
            "http://localhost",
        ] {
            assert!(ok(origin), "{origin}");
        }
        for origin in [
            "https://evil.example",
            "http://localhost.evil.example",
            "http://127.0.0.1.evil.example:80",
            "http://localhost:80@evil.example",
            "http://192.168.1.5:1420",
            "null",
        ] {
            assert!(!ok(origin), "{origin}");
        }
    }

    #[tokio::test]
    async fn cors_preflight_allows_tauri_and_rejects_foreign_origins() {
        let server = health_test_server("t").await;
        let preflight = |origin: &'static str| {
            server
                .method(axum::http::Method::OPTIONS, "/zwiim/v1/host.hello")
                .add_header("origin", origin)
                .add_header("access-control-request-method", "POST")
        };
        let res = preflight("tauri://localhost").await;
        assert_eq!(
            res.header("access-control-allow-origin"),
            "tauri://localhost"
        );
        let res = preflight("https://evil.example").await;
        assert!(res.maybe_header("access-control-allow-origin").is_none());
    }

    async fn health_test_server(token: &str) -> TestServer {
        let db = fresh_in_memory_db().await;
        let data_dir = tempfile::tempdir().expect("tempdir").keep();
        let state = Arc::new(AppState::new_for_test(db, data_dir.clone()));
        let router = build_router(
            state,
            token.to_string(),
            data_dir,
            Arc::new(ShutdownSignal::new()),
        );
        TestServer::new(router).expect("test server")
    }

    /// `/health` is the one local signal the runtime exposes for "is it up, and
    /// which version answered". It sits behind the same bearer-token layer as
    /// every other route, so a valid token returns 200 with the running version.
    #[tokio::test]
    async fn health_returns_ok_and_version_with_a_valid_token() {
        let server = health_test_server("test-token").await;

        let response = server
            .post("/api/health")
            .authorization_bearer("test-token")
            .await;

        assert_eq!(response.status_code(), StatusCode::OK);
        let body: serde_json::Value = response.json();
        assert_eq!(body["status"], "ok");
        assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    }

    /// The core-only router still fails closed: `/health` without a token is
    /// rejected by `auth::require_token`, proving the auth layer is wired onto
    /// the extracted surface.
    #[tokio::test]
    async fn health_without_a_token_is_unauthorized() {
        let server = health_test_server("test-token").await;

        let response = server.post("/api/health").expect_failure().await;

        assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
    }
}
