// Modified from upstream Apache-2.0 source; see NOTICE.
// The ACP connection driver (`acp::connection`) wraps the enormous
// `run_connection` future in a `block_on(async move { … })` frame whose type
// layout nests deep enough to blow rustc's default query depth of 128 (it
// overflowed by ~130 when computing the async block's layout). This is a
// compile-time type-recursion knob, unrelated to any runtime limit — bump it
// so the giant future's layout resolves. See the big-stack thread in
// `acp/connection.rs` for the sibling *runtime* mitigation of the same frame.
#![recursion_limit = "256"]

pub mod acp;
pub mod adapters;
pub mod agent_auth;
pub mod attachments;
pub mod agent_images;
pub mod acp_transcript;
pub use acp::{
    idle_sweep_task, idle_timeout_from_env, lifecycle_subscriber_task, SWEEP_INTERVAL_SECS,
};
pub use acp::scratch_dir::scratch_sweep_task;
pub use network::proxy::init_proxy_from_db;
mod app_error;
pub mod app_state;
pub mod commands;
pub mod conductor_import;
/// Built-in browser. Only its wire types and its grant rules compile in server
/// mode — see `browser/mod.rs` for why those two, and only those two.
pub mod browser;
pub mod cloud;
pub mod db;
pub mod downloads;
pub mod folder_links;
pub mod git_credential;
pub mod git_harness;
pub mod git_repo;
pub mod harness;
pub mod history_cache;
pub mod host_feed;
pub mod conflict_watch;
pub mod pr_watch;
pub mod host_lifecycle;
pub mod identity;
pub mod intern;
pub mod power;
pub mod lan;
pub mod keyring_store;
pub mod local_host;
pub mod logging;
pub mod models;
mod network;
pub mod parsers;
pub mod paths;
pub mod process;
pub mod protocol;
pub mod redact;
pub mod follow_ups;
pub mod session;
pub mod session_checkpoints;
pub mod session_commands;
pub mod session_options;
pub mod session_continue;
pub mod session_fork;
pub mod session_plans;
pub mod session_retry;
pub mod session_summary;
pub mod repository_settings;
pub mod settings;
pub mod supervise;
mod terminal;
pub mod tunnel;
pub mod turn_timings;
pub mod unreachable;
pub mod update;
pub mod web;
pub mod workspace;
pub mod workspace_browse;
pub mod workspace_diff;
pub mod workspace_state;
pub mod workspace_transfer;

/// Sweep stale ACP binary cache trash created by the rename-aside fallback in
/// `acp::binary_cache::clear_agent_cache`. Safe to call any time; intended to
/// be invoked once at startup from a detached OS thread. Does not block, does
/// not panic, errors are silently dropped.
pub fn sweep_acp_binary_trash() {
    crate::acp::binary_cache::sweep_trash();
}

/// Reclaim per-launch ACP scratch directories left by a previous run — the
/// crash/force-quit backstop for the in-session sweep. Same contract as
/// [`sweep_acp_binary_trash`]: safe any time, intended for a detached startup
/// thread, never panics.
///
/// Deletes only directories whose recorded owner is positively confirmed dead
/// (or is this process and not live — see `acp::scratch_dir`), and never leaves
/// Zwiim's own `zwiim-acp/` subtree, so a peer Zwiim instance's work and any
/// other application's temp files are both out of reach by construction.
pub fn sweep_acp_scratch_dirs() {
    crate::acp::scratch_dir::sweep_foreign_orphans();
    crate::acp::scratch_dir::sweep_own_orphans();
}
