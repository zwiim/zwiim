//! Keep the Mac awake while an agent works, so a long turn is not cut short
//! by idle sleep. While any session has a turn in flight the host holds
//! `caffeinate`; it lets go when every turn has ended, when the operator
//! turns this off, or when the Mac runs on battery below
//! [`MIN_BATTERY_PERCENT`]. The display may still sleep. On by default.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::Notify;

use crate::acp::types::AcpEvent;
use crate::app_state::AppState;
use crate::db::error::DbError;
use crate::db::service::app_metadata_service;
use crate::protocol::error::ZwiimError;

const CONFIG_KEY: &str = "keep_awake";
/// On battery below this, the Mac may sleep even while an agent works.
const MIN_BATTERY_PERCENT: u8 = 10;
/// How often to look again without a turn starting or ending: the battery
/// drains, and a `caffeinate` that died is replaced.
const RECHECK: Duration = Duration::from_secs(60);

/// The setting changed: look again now.
static CHANGED: Notify = Notify::const_new();
/// Whether the host holds `caffeinate` right now.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Whether the operator wants the Mac kept awake. Stored in `app_metadata`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeepAwakeConfig {
    pub enabled: bool,
}

impl Default for KeepAwakeConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

pub async fn config(conn: &DatabaseConnection) -> Result<KeepAwakeConfig, DbError> {
    Ok(app_metadata_service::get_value(conn, CONFIG_KEY)
        .await?
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default())
}

async fn save_config(conn: &DatabaseConnection, config: &KeepAwakeConfig) -> Result<(), DbError> {
    let raw = serde_json::to_string(config).map_err(|e| DbError::Validation(e.to_string()))?;
    app_metadata_service::upsert_value(conn, CONFIG_KEY, &raw).await
}

#[derive(Debug, Serialize)]
pub struct KeepAwakeStatus {
    /// False where the host cannot keep the computer awake (not a Mac).
    pub supported: bool,
    pub enabled: bool,
    /// The Mac is being kept awake right now.
    pub active: bool,
}

/// `host.keep_awake.status`.
pub async fn status(state: &AppState) -> Result<KeepAwakeStatus, ZwiimError> {
    let config = config(&state.db.conn)
        .await
        .map_err(|_| ZwiimError::internal())?;
    Ok(KeepAwakeStatus {
        supported: cfg!(target_os = "macos"),
        enabled: config.enabled,
        active: ACTIVE.load(Ordering::Relaxed),
    })
}

/// `host.keep_awake.set`: turn keeping awake on or off, and remember it.
pub async fn set(state: &AppState, enabled: bool) -> Result<KeepAwakeStatus, ZwiimError> {
    save_config(&state.db.conn, &KeepAwakeConfig { enabled })
        .await
        .map_err(|_| ZwiimError::internal())?;
    tracing::info!(
        "[keep_awake] {}",
        if enabled { "enabled" } else { "disabled" }
    );
    CHANGED.notify_one();
    status(state).await
}

/// Hold `caffeinate` while an agent works, for as long as the host runs.
pub fn spawn(state: Arc<AppState>) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let mut rx = state.acp_event_bus.subscribe();
    tokio::spawn(async move {
        let mut hold: Option<tokio::process::Child> = None;
        loop {
            // A `caffeinate` that exited holds nothing.
            if hold
                .as_mut()
                .is_some_and(|c| !matches!(c.try_wait(), Ok(None)))
            {
                hold = None;
            }
            let wanted = wanted(&state).await;
            if wanted && hold.is_none() {
                hold = start();
            } else if !wanted {
                if let Some(mut child) = hold.take() {
                    let _ = child.kill().await;
                    tracing::info!("[keep_awake] released");
                }
            }
            ACTIVE.store(hold.is_some(), Ordering::Relaxed);

            tokio::select! {
                closed = turn_started_or_ended(&mut rx) => if closed { return },
                _ = CHANGED.notified() => {}
                _ = tokio::time::sleep(RECHECK) => {}
            }
        }
    });
}

/// Waits for an event that can start or end a turn; true once the bus closed.
async fn turn_started_or_ended(
    rx: &mut tokio::sync::broadcast::Receiver<Arc<crate::acp::types::EventEnvelope>>,
) -> bool {
    loop {
        match rx.recv().await {
            Ok(e)
                if matches!(
                    e.payload,
                    AcpEvent::StatusChanged { .. } | AcpEvent::TurnComplete { .. }
                ) =>
            {
                return false
            }
            Ok(_) => {}
            // Missed events may have included one: look again.
            Err(RecvError::Lagged(_)) => return false,
            Err(RecvError::Closed) => return true,
        }
    }
}

async fn wanted(state: &AppState) -> bool {
    let enabled = config(&state.db.conn)
        .await
        .map(|c| c.enabled)
        .unwrap_or(true);
    enabled && turn_in_flight(state).await && !low_battery(&pmset_batt().await)
}

async fn turn_in_flight(state: &AppState) -> bool {
    for info in state.connection_manager.list_connections().await {
        if let Some(s) = state.connection_manager.get_state(&info.id).await {
            if s.read().await.turn_in_flight {
                return true;
            }
        }
    }
    false
}

/// `caffeinate -i -s`: no idle sleep, and no system sleep on power. `-w`
/// ends it with the host, however the host ends.
fn start() -> Option<tokio::process::Child> {
    let spawned = crate::process::tokio_command("/usr/bin/caffeinate")
        .args(["-i", "-s", "-w", &std::process::id().to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn();
    match spawned {
        Ok(child) => {
            tracing::info!("[keep_awake] holding while an agent works");
            Some(child)
        }
        Err(e) => {
            tracing::warn!("[keep_awake] caffeinate failed to start: {e}");
            None
        }
    }
}

async fn pmset_batt() -> String {
    let out = crate::process::tokio_command("/usr/bin/pmset")
        .args(["-g", "batt"])
        .kill_on_drop(true)
        .output()
        .await;
    out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// From `pmset -g batt`: running on battery below [`MIN_BATTERY_PERCENT`].
/// A Mac on power, or without a battery, is never low.
fn low_battery(pmset: &str) -> bool {
    if !pmset.contains("'Battery Power'") {
        return false;
    }
    let percent = pmset.lines().find_map(|line| {
        let (before, _) = line.split_once('%')?;
        let digits = before.rsplit(|c: char| !c.is_ascii_digit()).next()?;
        digits.parse::<u8>().ok()
    });
    percent.is_some_and(|p| p < MIN_BATTERY_PERCENT)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::low_battery;
    use crate::protocol::tests::host;

    const ON_BATTERY: &str = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=4653155)\t";
    const ON_POWER: &str = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=4653155)\t";

    #[test]
    fn only_a_mac_on_battery_below_ten_percent_is_low() {
        assert!(low_battery(&format!(
            "{ON_BATTERY}9%; discharging; 0:21 remaining present: true\n"
        )));
        assert!(!low_battery(&format!(
            "{ON_BATTERY}10%; discharging; 0:25 remaining present: true\n"
        )));
        assert!(!low_battery(&format!(
            "{ON_BATTERY}85%; discharging; 6:02 remaining present: true\n"
        )));
        // Charging at 3% is fine: it is on power.
        assert!(!low_battery(&format!(
            "{ON_POWER}3%; charging; 2:10 remaining present: true\n"
        )));
        // A desktop Mac, or pmset unavailable.
        assert!(!low_battery("Now drawing from 'AC Power'\n"));
        assert!(!low_battery(""));
    }

    #[tokio::test]
    async fn keeping_awake_is_on_until_the_operator_turns_it_off() {
        let h = host(false).await;
        let status = h.ok("host.keep_awake.status", json!({})).await;
        assert_eq!(status["enabled"], json!(true));
        assert_eq!(status["supported"], json!(cfg!(target_os = "macos")));

        let status = h.ok("host.keep_awake.set", json!({"enabled": false})).await;
        assert_eq!(status["enabled"], json!(false));
        assert!(!super::config(&h.state.db.conn).await.unwrap().enabled);
        assert_eq!(
            h.ok("host.keep_awake.status", json!({})).await["enabled"],
            json!(false)
        );

        // Operator only.
        let res = h
            .server
            .post("/zwiim/v1/host.keep_awake.set")
            .json(&json!({"enabled": true}))
            .await;
        assert_eq!(res.status_code().as_u16(), 401);
    }

    /// A real `caffeinate`, held for exactly as long as a turn runs.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn the_mac_is_kept_awake_while_a_turn_runs_and_released_after() {
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        use crate::acp::types::{AcpEvent, EventEnvelope};
        use crate::app_state::AppState;
        use crate::models::agent::AgentType;
        use crate::web::event_bridge::EventEmitter;

        async fn held(want: bool) {
            let pattern = format!("caffeinate -i -s -w {}", std::process::id());
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let running = std::process::Command::new("pgrep")
                    .args(["-f", &pattern])
                    .output()
                    .unwrap()
                    .status
                    .success();
                let active = super::ACTIVE.load(std::sync::atomic::Ordering::Relaxed);
                if running == want && active == want {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "caffeinate running: {running}, want {want}"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let state = Arc::new(AppState::new_for_test(db, dir.path().to_path_buf()));
        state
            .connection_manager
            .insert_test_connection("c1", AgentType::Codex, None, EventEmitter::Noop)
            .await;
        let turn = |running: bool| {
            let state = state.clone();
            async move {
                let s = state.connection_manager.get_state("c1").await.unwrap();
                s.write().await.turn_in_flight = running;
                let payload = AcpEvent::TurnComplete {
                    session_id: "s1".into(),
                    stop_reason: "end_turn".into(),
                    agent_type: "codex".into(),
                };
                state.acp_event_bus.send(Arc::new(EventEnvelope {
                    seq: 1,
                    connection_id: "c1".into(),
                    payload,
                }));
            }
        };

        super::spawn(state.clone());
        held(false).await;
        turn(true).await;
        held(true).await;
        turn(false).await;
        held(false).await;

        // Turned off, a running turn does not hold the Mac.
        super::set(&state, false).await.unwrap();
        turn(true).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        held(false).await;
        super::set(&state, true).await.unwrap();
        held(true).await;
        turn(false).await;
        held(false).await;
    }
}
