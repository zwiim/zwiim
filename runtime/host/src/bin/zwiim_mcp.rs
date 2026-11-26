// Modified from upstream Apache-2.0 source; see NOTICE.
//! `zwiim-mcp` — the per-launch stdio MCP companion that an agent CLI runs
//! to surface Zwiim's tools to its LLM: the multi-agent delegation tools
//! (`delegate_to_agent` etc.), `check_user_feedback` (pull the user's mid-turn
//! steering notes), `ask_user_question` (block on a multiple-choice card), and
//! `get_session_info` (resolve a referenced session by id), plus the
//! chat-authoring tools (`create_automation` / `create_work_task`) and
//! `update_progress` (show the user the agent's steps), gated by the
//! `--features` groups (`delegation` / `feedback` / `ask` / `sessions` /
//! `tasks` / `automations` / `taskboard` / `progress`).
//!
//! The agent's MCP config (injected by Zwiim via `load_mcp_servers_for_agent`)
//! spawns this binary with three required flags:
//!
//!   zwiim-mcp \
//!     --parent-connection-id <uuid> \
//!     --socket-path <abs path> \
//!     --token <ephemeral secret>
//!
//! All three are required and the binary exits early if any is missing.
//! `--custom-agents` optionally carries the `custom:<id>` slugs registered
//! in the parent, so `delegate_to_agent`'s schema can offer them as targets;
//! `--disabled-agents` optionally names the built-ins to drop from that
//! schema so only agents enabled in settings are advertised.
//! `--connector` makes the launch a connector proxy instead: it serves the
//! tools of the one connector its token is bound to, and none of its own.
//! Everything heavyweight lives in the library so it's unit-testable without
//! spawning a process: the MCP protocol (both eras, on rmcp) in
//! `zwiim_host::acp::delegation::mcp_server`, and the tools (schemas, UDS
//! round-trips, cancellation tracking) in
//! `zwiim_host::acp::delegation::{companion, transport}`.

use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use zwiim_host::acp::delegation::companion::{
    drain_and_cancel_all, CompanionContext, CompanionFeatures, InflightCalls, parse_csv_list,
};
use zwiim_host::acp::delegation::mcp_server;
use zwiim_host::acp::delegation::parent_watcher::{wait_for_parent_exit, DEFAULT_POLL_INTERVAL};

struct Args {
    parent_connection_id: String,
    socket_path: String,
    token: String,
    /// Optional PID of the Zwiim / zwiim-host process that owns this
    /// session. When set, zwiim-mcp exits as soon as the parent is gone so
    /// orphaned companions don't keep the binary file locked (Windows
    /// upgrade failure) or hold open a UDS / pipe nobody will ever read
    /// from. Omitted by older parents — backward compatible.
    parent_pid: Option<u32>,
    /// Comma-joined tool groups to expose (e.g.
    /// `delegation,feedback,ask,sessions,automations,taskboard`). Omitted by parents that predate
    /// feature gating; see `CompanionFeatures::parse` (defaults to
    /// delegation-only).
    features: Option<String>,
    /// Comma-joined `custom:<id>` slugs of the custom ACP agents registered
    /// in the parent at injection time, appended to `delegate_to_agent`'s
    /// `agent_type` enum. Omitted when the parent has none (the embedded
    /// builtin-only schema is served unchanged).
    custom_agents: Option<String>,
    /// Comma-joined wire slugs of the built-in agents the user has disabled
    /// in settings, removed from `delegate_to_agent`'s `agent_type` enum so
    /// only launchable targets are advertised. Omitted when nothing is
    /// disabled (disabled customs are simply left out of `--custom-agents`).
    disabled_agents: Option<String>,
    /// Comma-joined exact tool names, passed only for automation engine
    /// sessions; narrows the groups to that list.
    tools: Option<String>,
    /// Serve the token's connector as a connector proxy.
    connector: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut parent_connection_id = None;
    let mut socket_path = None;
    let mut token = None;
    let mut parent_pid = None;
    let mut features = None;
    let mut custom_agents = None;
    let mut disabled_agents = None;
    let mut tools = None;
    let mut connector = false;

    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--parent-connection-id" => {
                parent_connection_id = Some(
                    iter.next()
                        .ok_or_else(|| "--parent-connection-id requires a value".to_string())?,
                );
            }
            "--socket-path" => {
                socket_path = Some(
                    iter.next()
                        .ok_or_else(|| "--socket-path requires a value".to_string())?,
                );
            }
            "--token" => {
                token = Some(
                    iter.next()
                        .ok_or_else(|| "--token requires a value".to_string())?,
                );
            }
            "--parent-pid" => {
                let raw = iter
                    .next()
                    .ok_or_else(|| "--parent-pid requires a value".to_string())?;
                parent_pid = Some(
                    raw.parse::<u32>()
                        .map_err(|e| format!("--parent-pid must be a u32: {e}"))?,
                );
            }
            "--features" => {
                features = Some(
                    iter.next()
                        .ok_or_else(|| "--features requires a value".to_string())?,
                );
            }
            "--custom-agents" => {
                custom_agents = Some(
                    iter.next()
                        .ok_or_else(|| "--custom-agents requires a value".to_string())?,
                );
            }
            "--disabled-agents" => {
                disabled_agents = Some(
                    iter.next()
                        .ok_or_else(|| "--disabled-agents requires a value".to_string())?,
                );
            }
            "--tools" => {
                tools = Some(
                    iter.next()
                        .ok_or_else(|| "--tools requires a value".to_string())?,
                );
            }
            "--connector" => connector = true,
            "--help" | "-h" => {
                println!(
                    "zwiim-mcp --parent-connection-id <uuid> --socket-path <path> --token <secret> [--parent-pid <pid>] [--features delegation,feedback,ask,sessions,tasks,progress] [--custom-agents custom:<id>,...] [--disabled-agents <agent>,...] [--tools <tool>,...] [--connector]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }
    Ok(Args {
        parent_connection_id: parent_connection_id
            .ok_or_else(|| "missing --parent-connection-id".to_string())?,
        socket_path: socket_path.ok_or_else(|| "missing --socket-path".to_string())?,
        token: token.ok_or_else(|| "missing --token".to_string())?,
        parent_pid,
        features,
        custom_agents,
        disabled_agents,
        tools,
        connector,
    })
}

/// Split an optional comma-joined arg value into its non-empty entries.
fn parse_csv(raw: Option<&str>) -> Vec<String> {
    raw.map(|csv| {
        csv.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    // Stderr-only subscriber: stdout is the JSON-RPC protocol channel, and
    // concurrent mcp processes share no log file. No hub/buffer/emitter.
    let _log_guard = zwiim_host::logging::init::init_mcp();

    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "zwiim-mcp: {e}");
            return ExitCode::from(2);
        }
    };
    let ctx = CompanionContext {
        parent_connection_id: args.parent_connection_id,
        socket_path: args.socket_path,
        token: args.token,
        features: CompanionFeatures::parse(args.features.as_deref()),
        custom_agents: parse_csv(args.custom_agents.as_deref()),
        disabled_agents: parse_csv(args.disabled_agents.as_deref()),
        only_tools: parse_csv_list(args.tools.as_deref()),
    };

    let inflight = Arc::new(InflightCalls::new());

    // Optional parent-PID watchdog. Composed as a separate future so the
    // server can be raced against it via `tokio::select!`; when no PID was
    // provided we substitute a never-ready future, which tokio's branch
    // evaluation skips for free.
    let watchdog: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
        match args.parent_pid {
            Some(pid) => Box::pin(wait_for_parent_exit(pid, DEFAULT_POLL_INTERVAL)),
            None => Box::pin(std::future::pending()),
        };

    let server: std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>>>> =
        if args.connector {
            Box::pin(mcp_server::serve_connector(
                ctx.socket_path.clone(),
                ctx.token.clone(),
                tokio::io::stdin(),
                tokio::io::stdout(),
            ))
        } else {
            Box::pin(mcp_server::serve(
                ctx.clone(),
                inflight.clone(),
                tokio::io::stdin(),
                tokio::io::stdout(),
            ))
        };
    tokio::select! {
        // Bias toward parent-exit detection: if the watchdog fires while a
        // call is in flight we want to bail rather than finish a round-trip
        // whose response no one will read.
        biased;
        () = watchdog => {
            // Best-effort: cancel every in-flight delegation BEFORE we
            // hard-exit so the broker doesn't park each pending row
            // on `rx.await` waiting for a TurnComplete it can never
            // deliver. cancel_by_parent on the Zwiim main side is the
            // ultimate backstop, but firing the explicit cancels here
            // closes the window between MCP shutdown and parent ACP
            // disconnect detection on the Zwiim side.
            drain_and_cancel_all(&ctx, &inflight, "parent process exited").await;
            let _ = writeln!(
                std::io::stderr(),
                "zwiim-mcp: parent process exited, shutting down"
            );
            // Hard exit on purpose: `tokio::io::stdin()` parks a
            // blocking worker thread that the runtime can't cancel,
            // so returning normally would keep the process alive
            // until the parent agent CLI also closes stdin — defeating
            // the watchdog. The agent CLI sees the stdout pipe close
            // and tears down its MCP client cleanly.
            std::process::exit(0);
        }
        served = server => {
            // The agent closed stdin (or the stream broke). Drain pending
            // delegations before returning so the broker can resolve them
            // immediately.
            drain_and_cancel_all(&ctx, &inflight, "companion stdio closed").await;
            if let Err(e) = served {
                let _ = writeln!(std::io::stderr(), "zwiim-mcp: {e}");
                return ExitCode::from(1);
            }
        }
    }
    ExitCode::SUCCESS
}
