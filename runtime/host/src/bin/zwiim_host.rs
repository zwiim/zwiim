use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use zwiim_host::app_state::AppState;
use zwiim_host::web::event_bridge::{EventEmitter, WebEventBroadcaster};
use zwiim_host::web::{
    addresses_for_bind, advertise_host, find_static_dir_standalone, resolve_persisted_server_token,
    WebServerState,
};

fn main() -> ExitCode {
    // Capture our own executable path before anything can rename it (an
    // in-place upgrade swaps the binary mid-run; `current_exe()` would then
    // resolve to a `" (deleted)"` path on Linux). Cheap, single-shot.
    zwiim_host::update::runtime::prime_self_exe();

    // Support --version flag
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    // Release tooling: what this build is, and whether it accepts a signed
    // release manifest (proves the shipped Host trusts the release key).
    if args.iter().any(|a| a == "--release-info") {
        println!("{}", zwiim_host::update::service::release_info());
        return ExitCode::SUCCESS;
    }
    if let Some(i) = args.iter().position(|a| a == "--verify-release") {
        let (Some(manifest), Some(sig)) = (args.get(i + 1), args.get(i + 2)) else {
            eprintln!("usage: zwiim-host --verify-release <release.json> <release.json.sig>");
            return ExitCode::from(2);
        };
        return match zwiim_host::update::service::verify_release_files(
            manifest.as_ref(),
            sig.as_ref(),
        ) {
            Ok(version) => {
                println!("release {version}: signature and manifest verified");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("release rejected: {e}");
                ExitCode::from(1)
            }
        };
    }

    // Resolve and pin `ZWIIM_DATA_DIR` first: the supervisor starts the
    // installed Host version from it, and its worker inherits it (see the
    // longer note below).
    let resolved_data_dir = std::env::var("ZWIIM_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_data_dir());
    let resolved_data_dir = zwiim_host::git_credential::absolutize(&resolved_data_dir);
    std::env::set_var("ZWIIM_DATA_DIR", &resolved_data_dir);

    // `--seed`: Desktop offers the Host it bundles to the installer at every
    // launch, so a Desktop update never downloads the Host a second time.
    if args.iter().any(|a| a == "--seed") {
        if !zwiim_host::update::install::versioned_installs() {
            println!("unchanged");
            return ExitCode::SUCCESS;
        }
        let root = zwiim_host::update::install::root(&resolved_data_dir);
        let exe = zwiim_host::update::runtime::self_exe();
        return match zwiim_host::update::install::seed(&root, &exe, env!("CARGO_PKG_VERSION")) {
            Ok(outcome) => {
                println!("{outcome:?}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("seed failed: {e}");
                ExitCode::from(1)
            }
        };
    }

    // `--supervise`: run as the process supervisor that owns the worker's
    // lifecycle (PID 1 in Docker). It spawns `zwiim-host` without this
    // flag and relaunches it after an in-place upgrade. Never returns.
    if args.iter().any(|a| a == "--supervise") {
        // This mode never reaches init_server(); install a stderr-only
        // subscriber so the supervisor's diagnostics still reach Docker logs.
        let _log_guard = zwiim_host::logging::init::init_stderr_only();
        zwiim_host::supervise::run();
    }

    // When invoked as a git credential helper (by the script written via
    // `git_credential::create_credential_helper_script`), respond to git's
    // credential protocol on stdin and exit. Mirrors the desktop binary's
    // early-exit in `main.rs` so server deployments don't accidentally try
    // to start a second server instance per `git credential` invocation.
    if args.iter().any(|a| a == "--credential-helper") {
        // Subprocess mode, before init_server(): stderr-only subscriber so
        // helper diagnostics aren't dropped, while stdout stays the git
        // credential protocol channel.
        let _log_guard = zwiim_host::logging::init::init_stderr_only();
        zwiim_host::git_credential::run_credential_helper();
        return ExitCode::SUCCESS;
    }

    // PATH initialisation MUST happen before the tokio runtime is created.
    // std::env::set_var is not thread-safe (unsafe in Rust edition 2024);
    // #[tokio::main] would spawn worker threads before we reach this point.
    zwiim_host::process::ensure_node_in_path();
    zwiim_host::process::ensure_user_npm_prefix_in_path();

    // Resolve and pin `ZWIIM_DATA_DIR` before any threads exist.
    //
    // Two things matter here, both single-shot:
    //
    // 1. Absolutize: child processes (notably the credential helper
    //    subprocess invoked by git from inside the user's repo) inherit
    //    the env var and use it via `keyring_store::tokens_file_path` to
    //    find `tokens.json`. A relative `ZWIIM_DATA_DIR=data` would
    //    otherwise resolve against git's CWD, not the server's startup
    //    CWD, and the helper would silently miss the token file even
    //    though we found the database.
    //
    // 2. Fill in the default if unset, so every downstream resolver —
    //    `paths::zwiim_uploads_root`, `paths::zwiim_pets_root`,
    //    the credential subprocess — converges on the same root the
    //    server itself chose for the database. Without this, a default
    //    deployment (env var unset) puts the DB under
    //    `dirs::data_dir()/zwiim` but uploads under `~/.zwiim/uploads`,
    //    splitting the persistent surface across two filesystem roots
    //    and silently breaking single-volume backups, container mounts,
    //    and any `file://` URI in session history that points at an
    //    upload.
    //
    // `std::env::set_var` is not thread-safe (unsafe in Rust edition
    // 2024); doing this before the tokio runtime is built guarantees we
    // are still single-threaded.
    // (Resolved above, before `--supervise`, so the supervisor and its
    // worker agree on the data directory.)

    // Install the logging subscriber now that ZWIIM_DATA_DIR is pinned (the logs
    // dir resolves from it). Doing it here — before the first server diagnostic
    // below — captures even the ZWIIM_HOME warning and the FATAL upload-quota
    // abort. Hold the guard for the whole process so buffered file lines flush
    // on a graceful exit.
    let _log_guard = zwiim_host::logging::init::init_server();

    // `ZWIIM_HOME` overrides `ZWIIM_DATA_DIR` for uploads/pets inside
    // `paths::zwiim_*_root` (legacy `~/.zwiim/` layout). If both are set
    // and resolve to different roots, the database and uploads land on
    // different filesystems — a silent split. Warn loudly so the
    // operator notices before relying on a backup or volume mount that
    // only covers one of them.
    if let Some(home) = std::env::var_os("ZWIIM_HOME").filter(|s| !s.is_empty()) {
        let home_path = zwiim_host::git_credential::absolutize(std::path::Path::new(&home));
        if home_path != resolved_data_dir {
            tracing::warn!(
                "[paths][WARN] ZWIIM_HOME ({}) and ZWIIM_DATA_DIR ({}) point at different roots. \
                 Uploads/pets follow ZWIIM_HOME; the database follows ZWIIM_DATA_DIR. \
                 Unset one or align them to avoid split state.",
                home_path.display(),
                resolved_data_dir.display()
            );
        }
    }

    // Strict-mode quota validation runs before any I/O. Failing fast
    // here means a misconfigured strict deployment never reaches the
    // tokio runtime, never binds a port, and never persists config —
    // the operator sees the FATAL line and a clean exit code 2.
    zwiim_host::web::handlers::files::log_upload_quota_config_at_startup();
    if let Err(err) = zwiim_host::web::handlers::files::validate_upload_quota_config() {
        tracing::error!("[uploads][FATAL] {err}; aborting startup.");
        // Return (don't process::exit) so `_log_guard` drops on the way out and
        // flushes the non-blocking file appender before the process ends.
        return ExitCode::from(2);
    }

    // One Host per data directory, decided before anything opens the
    // database: a second Host exits here, leaving the database, its
    // migrations and the discovery file to the one already running.
    let instance_lock = match zwiim_host::host_lifecycle::InstanceLock::acquire(&resolved_data_dir)
    {
        Ok(lock) => lock,
        Err(zwiim_host::host_lifecycle::LockError::Held { pid }) => {
            let holder = pid.map(|p| format!(" (pid {p})")).unwrap_or_default();
            tracing::info!(
                "[SERVER] Another Host{holder} is already running for {}; exiting.",
                resolved_data_dir.display()
            );
            eprintln!("[SERVER] Another Host{holder} is already running for this data directory.");
            return ExitCode::from(zwiim_host::host_lifecycle::EXIT_ALREADY_RUNNING);
        }
        Err(zwiim_host::host_lifecycle::LockError::Io(e)) => {
            tracing::error!("[SERVER] Could not lock the data directory: {e}; aborting startup.");
            return ExitCode::from(1);
        }
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime");
    let exit = runtime.block_on(async_main());
    // Don't wait on blocking threads a drain has already ended the work of
    // (a PTY reader blocked in `read`, say).
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));

    // `main` returns the worker's exit code so `_log_guard` drops here — after
    // the runtime finishes — flushing any buffered file logs on a clean exit.
    match exit {
        Exit::Code(code) => code,
        Exit::Restart if zwiim_host::update::runtime::is_supervised() => {
            // An update restart switches `current` before the supervisor
            // starts it.
            zwiim_host::update::service::activate_if_armed();
            tracing::info!("[SERVER] Restarting through the supervisor");
            ExitCode::from(zwiim_host::update::runtime::EXIT_RESTART as u8)
        }
        Exit::Restart => {
            tracing::info!("[SERVER] Restarting in place");
            drop(instance_lock);
            drop(_log_guard);
            reexec()
        }
    }
}

/// How the worker ends.
enum Exit {
    Code(ExitCode),
    /// Drained for a restart: relaunch this binary.
    Restart,
}

/// Replace this process with a fresh run of the same executable and
/// arguments (an unsupervised Host restarting). Returns only on failure.
#[cfg(unix)]
fn reexec() -> ExitCode {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(zwiim_host::update::runtime::self_exe())
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("[SERVER] Restart failed: {err}");
    ExitCode::from(1)
}

#[cfg(not(unix))]
fn reexec() -> ExitCode {
    let exe = zwiim_host::update::runtime::self_exe();
    match std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .spawn()
    {
        Ok(_) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("[SERVER] Restart failed: {err}");
            ExitCode::from(1)
        }
    }
}

async fn async_main() -> Exit {
    // Sweep stale ACP binary cache trash (rename-aside fallback artifacts).
    // Detached OS thread: cannot block startup, panics are caught and dropped,
    // errors are silenced, no subprocesses spawned.
    std::thread::spawn(|| {
        let _ = std::panic::catch_unwind(|| {
            zwiim_host::acp::binary_cache::migrate_legacy_root();
            zwiim_host::sweep_acp_binary_trash();
            zwiim_host::sweep_acp_scratch_dirs();
        });
    });

    let port: u16 = std::env::var("ZWIIM_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3080);
    // Loopback by default: local clients connect directly, remote clients will
    // arrive through the outbound relay. Exposing the host on the network
    // requires an explicit `ZWIIM_HOST`.
    let host = std::env::var("ZWIIM_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    // ZWIIM_DATA_DIR was already resolved and absolutized in `main()` so
    // all path resolvers across the process see the same root. Read it
    // back rather than re-deriving the default.
    let data_dir =
        PathBuf::from(std::env::var("ZWIIM_DATA_DIR").expect("ZWIIM_DATA_DIR set by main()"));
    let discovery_dir = data_dir.clone();
    let static_dir_env = std::env::var("ZWIIM_STATIC_DIR").ok();

    let static_dir = find_static_dir_standalone(static_dir_env.as_deref());
    let app_version = env!("CARGO_PKG_VERSION");

    tracing::info!("[SERVER] zwiim-host v{}", app_version);
    tracing::info!("[SERVER] Data directory: {}", data_dir.display());
    tracing::info!("[SERVER] Static directory: {}", static_dir.display());

    // Initialize database
    let db = match zwiim_host::db::init_database(&data_dir, app_version).await {
        Ok(db) => db,
        Err(e @ zwiim_host::db::error::DbError::NewerSchema(_)) => {
            // Never run on (or downgrade) data this version cannot read.
            tracing::error!(
                "[SERVER] {e}. Install that version of Zwiim or newer; the data was not changed."
            );
            eprintln!("[SERVER] This data was written by a newer version of Zwiim. Install that version or newer; nothing was changed.");
            return Exit::Code(ExitCode::from(
                zwiim_host::host_lifecycle::EXIT_DATA_TOO_NEW,
            ));
        }
        Err(e) => {
            tracing::error!("[SERVER] Failed to initialize database: {e}");
            return Exit::Code(ExitCode::from(1));
        }
    };

    // Logging phase 2: override the default level from the persisted
    // `logging.level` now that the DB is open. Phase 3 (wiring the emitter)
    // happens once AppState exists, below.
    zwiim_host::logging::init::apply_persisted_level(&db.conn).await;

    // Mint (first boot) or load the persistent host identity.
    let host_id = match zwiim_host::identity::host::ensure_host_id(&db.conn).await {
        Ok(host_id) => {
            tracing::info!("[SERVER] Host id: {host_id}");
            Some(host_id)
        }
        Err(e) => {
            tracing::error!("[SERVER] Failed to initialize host identity: {e}");
            None
        }
    };

    // Resolve the access token *after* the host identity is known so a
    // generated token is kept, in the host's key store beside its keys, and
    // reused across restarts (a self-update restart must not rotate it). An
    // empty/whitespace ZWIIM_TOKEN is treated as unset.
    let token_store = host_id.as_deref().and_then(|host_id| {
        zwiim_host::identity::host_keys::KeyStore::token_from_env(&data_dir, host_id)
            .map_err(|e| tracing::error!("[SERVER] Host token store unavailable: {e}"))
            .ok()
    });
    let mut token_generated = false;
    let token = resolve_persisted_server_token(
        &db.conn,
        token_store.as_ref(),
        std::env::var("ZWIIM_TOKEN").ok(),
        &mut token_generated,
    )
    .await;
    if token_generated {
        // The access token is a bearer credential: it is never printed, since
        // stderr is itself a log under a service manager (journald, Docker).
        // A local client reads it from the owner-only discovery file.
        eprintln!(
            "[SERVER] No ZWIIM_TOKEN set; generated an access token (in {}).",
            zwiim_host::local_host::path(&discovery_dir).display()
        );
        eprintln!("[SERVER] Pin your own by setting the ZWIIM_TOKEN environment variable.");
    }

    // Restore and apply saved system proxy settings before any network operation.
    // reqwest clients (including the LazyLock in check_app_update) cache the proxy
    // config at build time, so this must run before the first one is constructed.
    zwiim_host::init_proxy_from_db(&db.conn).await;

    // Reclaim orphaned chat scratch dirs (pre-send drafts that never bound to a
    // conversation, plus dirs left behind by deleted chat conversations).
    // Background, non-blocking; failures are logged but non-fatal.
    {
        let gc_conn = db.conn.clone();
        let gc_data_dir = data_dir.clone();
        tokio::spawn(async move {
            match zwiim_host::commands::conversations::gc_orphan_chat_dirs_core(
                &gc_conn,
                &gc_data_dir,
            )
            .await
            {
                Ok(n) if n > 0 => {
                    tracing::info!("[SERVER] chat-dir GC: reclaimed {n} orphan scratch dir(s)")
                }
                Ok(_) => {}
                Err(err) => tracing::error!("[SERVER] chat-dir GC failed: {err}"),
            }
        });
    }

    // API keys saved for the agents, read before any agent launches.
    zwiim_host::agent_auth::init(&data_dir).await;

    // Install or confirm the managed Claude/Codex adapters from this build's
    // lock, in the background; offline, installed adapters keep working.
    zwiim_host::adapters::provision_at_start();

    // Create shared broadcaster + internal ACP event bus.
    let broadcaster = Arc::new(WebEventBroadcaster::new());
    let event_bus_metrics = Arc::new(zwiim_host::acp::EventBusMetrics::default());
    let acp_event_bus = Arc::new(zwiim_host::acp::InternalEventBus::new(
        event_bus_metrics.clone(),
    ));
    let emitter = EventEmitter::web_only(broadcaster.clone(), acp_event_bus.clone());

    // Build AppState
    let connection_manager = zwiim_host::app_state::default_connection_manager();
    let (
        delegation_broker,
        delegation_tokens,
        delegation_socket_path,
        feedback_config,
        question_config,
        session_info_config,
        chat_authoring_config,
        browser_tools_config,
    ) = zwiim_host::app_state::build_delegation_stack(
        &connection_manager,
        db.conn.clone(),
        data_dir.clone(),
    );
    let state = Arc::new(AppState {
        db,
        connection_manager,
        terminal_manager: zwiim_host::app_state::default_terminal_manager(),
        event_broadcaster: broadcaster,
        acp_event_bus: acp_event_bus.clone(),
        emitter,
        worktrees_dir: zwiim_host::app_state::default_worktrees_dir()
            .unwrap_or_else(|| data_dir.join("worktrees")),
        repos_dir: zwiim_host::app_state::default_repos_dir()
            .unwrap_or_else(|| data_dir.join("repos")),
        data_dir,
        web_server_state: WebServerState::new(),
        workspace_transfer: Arc::new(
            zwiim_host::workspace_transfer::WorkspaceTransferManager::new_from_env(),
        ),
        delegation_broker: delegation_broker.clone(),
        delegation_tokens: delegation_tokens.clone(),
        delegation_socket_path: delegation_socket_path.clone(),
        feedback_config: feedback_config.clone(),
        question_config: question_config.clone(),
        session_info_config: session_info_config.clone(),
        chat_authoring_config: chat_authoring_config.clone(),
        browser_tools_config: browser_tools_config.clone(),
        system_op_lock: zwiim_host::app_state::default_system_op_lock(),
        pairing_invitations: Default::default(),
        host_feed: Default::default(),
        host_keys: Default::default(),
        device_revocations: Default::default(),
        cloud: Arc::new(zwiim_host::cloud::CloudLink::from_env()),
        lan: Arc::new(zwiim_host::lan::LanLink::from_env()),
        lifecycle: Default::default(),
        gh: None,
        clones: Arc::new(zwiim_host::workspace::clone::CloneJobs::new(
            zwiim_host::workspace::clone::RETENTION,
        )),
           keep_awake: Default::default(),
    });

    // Load (first boot: mint) the host's identity keys off the startup path.
    if let Some(host_id) = host_id.clone() {
        let state = state.clone();
        tokio::spawn(async move {
            use zwiim_host::identity::host_keys::{load_or_create, KeyStore};
            let store = match KeyStore::from_env(&state.data_dir, &host_id) {
                Ok(store) => store,
                Err(e) => return tracing::error!("[SERVER] Host keys unavailable: {e}"),
            };
            match load_or_create(&state.db.conn, &store).await {
                Ok(keys) => {
                    tracing::info!(
                        "[SERVER] Host keys ready ({}): sign {} static {}",
                        store.name(),
                        keys.sign_pubkey(),
                        keys.static_pubkey()
                    );
                    let _ = state.host_keys.set(keys);
                }
                Err(e) => tracing::error!("[SERVER] Host keys unavailable ({}): {e}", store.name()),
            }
        });
    }

    // Follow agents' session options into the per-session preferences that
    // idle respawn and host restart replay.
    zwiim_host::session_options::spawn_tracker(state.clone());
    // Keep each agent's slash commands for its next new session.
    zwiim_host::session_commands::spawn_tracker(state.clone());
    // Close each turn's Git checkpoint when the agent ends the turn.
    zwiim_host::session_checkpoints::spawn_tracker(state.clone());
    // Keep the Mac awake while an agent works.
    zwiim_host::power::spawn(state.clone());

    // Logging phase 3: wire the emitter so the Logs viewer's live tail
    // (`logs://appended`) reaches WS clients.
    if let Some(hub) = zwiim_host::logging::hub::log_hub() {
        hub.set_emitter(state.emitter.clone());
    }

    // Apply persisted delegation settings (depth, enabled) before
    // the listener starts accepting so even the first companion request
    // sees the operator's configured behavior. Cancellation is handled
    // out-of-band via MCP `notifications/cancelled` — no broker-side
    // timeout to apply here.
    zwiim_host::commands::delegation::apply_persisted_config(&state.db.conn, &delegation_broker)
        .await;
    // Same for the live-feedback enable flag, so the first companion launch
    // sees the operator's configured behavior.
    zwiim_host::commands::feedback::apply_persisted_feedback_config(
        &state.db.conn,
        &feedback_config,
    )
    .await;
    // Same for the ask-user-question enable flag.
    zwiim_host::commands::question::apply_persisted_question_config(
        &state.db.conn,
        &question_config,
    )
    .await;
    // Same for the get-session-info enable flag.
    zwiim_host::commands::session_info::apply_persisted_session_info_config(
        &state.db.conn,
        &session_info_config,
    )
    .await;
    // Same for the chat-authoring flags, so the first companion launch knows
    // whether to advertise `create_automation` / `create_work_task`.
    zwiim_host::commands::chat_authoring::apply_persisted_chat_authoring_config(
        &state.db.conn,
        &chat_authoring_config,
    )
    .await;
    // And the browser-tools switch, so the popover reports it truthfully.
    // Server mode never advertises the group (there are no native tabs here),
    // but the flag is one setting shared by both runtimes.
    zwiim_host::commands::browser_tools::apply_persisted_browser_tools_config(
        &state.db.conn,
        &state.browser_tools_config,
    )
    .await;
    // Before accepting connections: keep ACP model terminal fallbacks aligned
    // with the same default-shell preference the built-in terminal uses, and
    // seed the command-color opt-in that every launch env is built from.
    let terminal_shell_config = state.connection_manager.terminal_shell_config();
    zwiim_host::commands::system_settings::apply_persisted_terminal_settings(
        &state.db.conn,
        &terminal_shell_config,
    )
    .await;

    // Spawn the delegation listener so companion processes can round-trip
    // through the broker. Path is PID-scoped, so the listener owns it for
    // the lifetime of the process.
    {
        let listener = zwiim_host::acp::delegation::listener::DelegationListener::new(
            delegation_broker,
            delegation_tokens,
            Arc::new(zwiim_host::acp::manager::ConnectionManagerParentLookup {
                manager: Arc::new(state.connection_manager.clone_ref()),
            }),
            Arc::new(zwiim_host::acp::manager::ConnectionManagerFeedbackLookup {
                manager: Arc::new(state.connection_manager.clone_ref()),
            }),
            Arc::new(zwiim_host::acp::manager::ConnectionManagerQuestionLookup {
                manager: Arc::new(state.connection_manager.clone_ref()),
            }),
            Arc::new(
                zwiim_host::commands::session_info::DbSessionInfoLookup::new(Arc::new(
                    zwiim_host::db::AppDatabase {
                        conn: state.db.conn.clone(),
                    },
                )),
            ),
            Arc::new(zwiim_host::acp::work_task_tools::NoWorkTaskTools),
            Arc::new(zwiim_host::commands::chat_authoring::DbChatAuthoring::new(
                Arc::new(zwiim_host::db::AppDatabase {
                    conn: state.db.conn.clone(),
                }),
                state.emitter.clone(),
                chat_authoring_config.clone(),
            )),
            // No native webviews in this process: what a web user sees in a
            // "browser tab" is an iframe their own browser renders, which
            // nothing here can reach.
            Arc::new(zwiim_host::acp::browser_tools::NoBrowserTabs),
            Arc::new(zwiim_host::acp::git_tools::HostGitTools {
                manager: Arc::new(state.connection_manager.clone_ref()),
                db: Arc::new(zwiim_host::db::AppDatabase {
                    conn: state.db.conn.clone(),
                }),
                data_dir: state.data_dir.clone(),
            }),
        );
        // Bind through the service handle rather than a bare `listener.run`
        // spawn: it keeps the bind error and the accept-loop handle around, so
        // the workspace status indicator can report why the broker socket is
        // down and rebind it without restarting the server.
        let service = zwiim_host::acp::delegation::service::DelegationService::new(
            listener,
            delegation_socket_path.clone(),
        );
        zwiim_host::acp::delegation::service::install(service.clone());
        tokio::spawn(async move {
            if let Err(e) = service.start().await {
                tracing::error!("[delegation] listener failed to start: {e}");
            }
        });
    }

    // Install bundled expert skills into the central store
    // (`~/.zwiim/skills/`). Runs in the background; failures are logged
    // but non-fatal.
    tokio::spawn(async move {
        let report = zwiim_host::commands::experts::ensure_central_experts_installed().await;
        if !report.errors.is_empty() {
            tracing::error!(
                "[Experts] install finished with {} error(s): {:?}",
                report.errors.len(),
                report.errors
            );
        } else {
            tracing::info!(
                "[Experts] install ok: installed={} updated={} pending_review={}",
                report.installed_count,
                report.updated_count,
                report.pending_user_review.len()
            );
        }
    });

    // Spawn the LifecycleSubscriber for cross-connection DB writes. The
    // broker is supplied so TurnComplete on a delegation child resolves the
    // parent's pending `delegate_to_agent` tool_use_id and emits
    // `DelegationCompleted`.
    tokio::spawn(zwiim_host::lifecycle_subscriber_task(
        state.db.conn.clone(),
        state.connection_manager.clone_ref(),
        state.acp_event_bus.clone(),
        Some(state.delegation_broker.clone()),
    ));

    // Spawn the idle sweep so connections abandoned without an explicit
    // disconnect (e.g. browser tab closed, panic survivors) are reaped.
    // Override the 60-second default via `ZWIIM_ACP_IDLE_TIMEOUT_SECS`
    // (set to `0` to disable).
    if let Some(idle_timeout) = zwiim_host::idle_timeout_from_env() {
        tokio::spawn(zwiim_host::idle_sweep_task(
            state.connection_manager.clone_ref(),
            idle_timeout,
            std::time::Duration::from_secs(zwiim_host::SWEEP_INTERVAL_SECS),
        ));
    }

    // Reclaim scratch directories lost track of mid-session. Deliberately NOT
    // gated on `idle_timeout_from_env` like the sweep above: setting
    // `ZWIIM_ACP_IDLE_TIMEOUT_SECS=0` disables idle disconnects, not disk
    // reclamation.
    tokio::spawn(zwiim_host::scratch_sweep_task());

    // Config-sync uploader (mirrors lib.rs setup): sleeps a minute, then
    // compares the configuration's hash every interval and uploads only when
    // it changed. Does nothing at all until a WebDAV endpoint is configured.
    {
        let db_for_sync = state.db.conn.clone();
        let emitter = std::sync::Arc::new(state.emitter.clone());
        tokio::spawn(async move {
            zwiim_host::commands::config_sync::auto_sync::run_auto_sync_loop(
                db_for_sync,
                emitter,
                zwiim_host::commands::config_sync::APP_VERSION.to_string(),
            )
            .await;
        });
    }

    // Label worktree folders registered before aliases were seeded at creation
    // with the branch they have checked out (mirrors lib.rs setup). Background;
    // changed folders are broadcast, so a browser that already fetched its
    // folder list still picks them up.
    {
        let db = zwiim_host::db::AppDatabase {
            conn: state.db.conn.clone(),
        };
        let emitter = state.emitter.clone();
        tokio::spawn(async move {
            let n = zwiim_host::commands::folders::backfill_worktree_folder_aliases(&emitter, &db)
                .await;
            if n > 0 {
                tracing::info!("[folders] labeled {n} worktree folder(s) by branch");
            }
        });
    }

    // Sweep abandoned upload staging files from any prior run before
    // serving the first request. The quota log/validate ran earlier in
    // `main` so strict-mode misconfigurations abort before we touch
    // disk; no second log line here.
    zwiim_host::web::handlers::files::purge_upload_staging().await;

    // Build router
    let shutdown_signal = state.web_server_state.shutdown_signal();
    let router = zwiim_host::web::router::build_router(
        state.clone(),
        token.clone(),
        static_dir,
        shutdown_signal.clone(),
    );

    // Device tunnels on a loopback address, for local verification only
    // (remote devices reach a host through the Cloud relay). Off unless
    // `ZWIIM_TUNNEL_TEST_ADDR` is set.
    if let Some(addr) = std::env::var("ZWIIM_TUNNEL_TEST_ADDR")
        .ok()
        .filter(|s| !s.is_empty())
    {
        let addr: std::net::SocketAddr = match addr.parse() {
            Ok(addr) => addr,
            Err(e) => {
                tracing::error!("[SERVER] Invalid ZWIIM_TUNNEL_TEST_ADDR {addr:?}: {e}");
                return Exit::Code(ExitCode::from(1));
            }
        };
        let tunnel_router =
            zwiim_host::tunnel::host::router(state.clone(), token.clone(), shutdown_signal.clone());
        let (state, shutdown) = (state.clone(), shutdown_signal.clone());
        tokio::spawn(async move {
            if let Err(e) =
                zwiim_host::tunnel::host::serve_test_listener(addr, state, tunnel_router, shutdown)
                    .await
            {
                tracing::error!("[SERVER] Tunnel test listener on {addr} failed: {e}");
            }
        });
    }

    // The outbound Cloud connection, for remote devices. Connects only once
    // the host is enrolled (`cloud.enroll`); local clients never go through
    // it, and nothing here waits on it.
    zwiim_host::cloud::start(
        state.clone(),
        zwiim_host::tunnel::host::router(state.clone(), token.clone(), shutdown_signal.clone()),
        shutdown_signal.clone(),
    );

    // Direct device tunnels on the local network, only if the operator
    // turned LAN access on (`lan.set`). Same tunnel, same device trust as
    // the Cloud relay; no HTTP and no `/api` on that port.
    zwiim_host::lan::start(
        state.clone(),
        zwiim_host::tunnel::host::router(state.clone(), token.clone(), shutdown_signal.clone()),
        shutdown_signal.clone(),
    );

    // Bind
    let addr = format!("{}:{}", host, port);
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(e) => {
            tracing::error!("[SERVER] Failed to bind {}: {}", addr, e);
            return Exit::Code(ExitCode::from(1));
        }
    };

    if let Err(e) = zwiim_host::web::socket_inherit::mark_listener_non_inheritable(&listener) {
        tracing::warn!(
            "[SERVER][WARN] failed to mark listener non-inheritable: {}",
            e
        );
    }

    let local_addr = listener.local_addr().ok();
    let actual_port = local_addr.map(|a| a.port()).unwrap_or(port);
    // `ZWIIM_HOST` may be `localhost` or a bracketed IPv6 (`[::1]`); advertise
    // the concrete IP the socket bound to, not the raw config string.
    let advertised_host = advertise_host(local_addr, &host);

    // Publish runtime state so the settings page (served by us) shows
    // the truth — running on `actual_port` with this token — instead of
    // the placeholder "stopped" that triggers the stale-port banner.
    state.web_server_state.mark_externally_running(
        advertised_host.clone(),
        actual_port,
        token.clone(),
    );
    let addresses = addresses_for_bind(&advertised_host, actual_port);

    // Local discovery for a desktop client on this machine (owner-only file;
    // never logged).
    if let Some(host_id) = host_id {
        let info = zwiim_host::local_host::LocalHostInfo {
            host_id,
            url: zwiim_host::local_host::loopback_url(&advertised_host, actual_port),
            token: token.clone(),
            pid: std::process::id(),
        };
        if let Err(e) = zwiim_host::local_host::write(&discovery_dir, &info) {
            tracing::warn!("[SERVER][WARN] could not write local host discovery file: {e}");
        }
    }

    tracing::info!("[SERVER] Listening on:");
    for addr in &addresses {
        tracing::info!("  {}", addr);
    }

    // A stop signal drains like a restart does, then exits.
    {
        let lifecycle = state.lifecycle.clone();
        tokio::spawn(async move {
            zwiim_host::host_lifecycle::stop_signal().await;
            tracing::info!("[SERVER] Stop requested; draining");
            lifecycle.request(zwiim_host::host_lifecycle::ShutdownKind::Stop);
        });
    }

    // Clones cut short by the last exit leave staging folders behind; swept
    // before the server can accept a request that might start a new one
    // (otherwise a racing clone's staging dir could be deleted mid-clone).
    zwiim_host::workspace::clone::sweep_cloning(&state.repos_dir).await;

    // Start serving; stop accepting once the drain has told clients to go.
    let shutdown = shutdown_signal.clone();
    let mut server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { shutdown.wait().await })
            .await
    });
    // Plain roots from before every workspace was a repository; in the
    // background so serving never waits on a commit.
    {
        let state = state.clone();
        tokio::spawn(async move {
            zwiim_host::workspace::init_plain_roots(&state.db, &state.emitter).await;
        });
    }
    // A version on trial proves its health over the API it just bound.
    zwiim_host::update::service::run_trial(
        zwiim_host::local_host::loopback_url(&advertised_host, actual_port),
        token.clone(),
        state.lifecycle.clone(),
    );
    zwiim_host::update::service::start_background_checks();

    let kind = tokio::select! {
        kind = state.lifecycle.wait_for_request() => kind,
        result = &mut server => {
            // The server stopped on its own: nothing was drained.
            match result {
                Ok(Err(e)) => tracing::error!("[SERVER] Server error: {}", e),
                Err(e) => tracing::error!("[SERVER] Server task failed: {}", e),
                Ok(Ok(())) => tracing::error!("[SERVER] Server stopped unexpectedly"),
            }
            return Exit::Code(ExitCode::from(1));
        }
    };
    zwiim_host::host_lifecycle::drain(&state).await;
    // Let connections close; a client that won't is cut off.
    if tokio::time::timeout(std::time::Duration::from_secs(5), &mut server)
        .await
        .is_err()
    {
        tracing::warn!("[SERVER] Connections still open after the drain; closing them");
        server.abort();
    }
    zwiim_host::host_lifecycle::persist(&state).await;
    // A stop that arrived during a restart's drain wins.
    let kind = state.lifecycle.requested().unwrap_or(kind);
    if kind == zwiim_host::host_lifecycle::ShutdownKind::Stop {
        // A stopped Host is not there to be found; a restarting one rewrites
        // the file when it is back.
        let _ = std::fs::remove_file(zwiim_host::local_host::path(&discovery_dir));
        tracing::info!("[SERVER] Stopped");
        return Exit::Code(ExitCode::SUCCESS);
    }
    Exit::Restart
}

fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .map(|d| d.join("zwiim"))
        .unwrap_or_else(|| PathBuf::from(".zwiim-data"))
}
