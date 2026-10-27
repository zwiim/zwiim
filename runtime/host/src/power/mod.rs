//! Keeping the host reachable while the user is away: the computer must not
//! idle-sleep, and after any sleep the host gets back online fast.
//!
//! [`policy`] decides, a [`backend::PowerBackend`] per platform holds and
//! reads the power state, and [`config`] remembers the operator's choice.

pub mod backend;
pub mod config;
#[cfg(unix)]
mod lid_helper;
mod macos;
pub mod policy;
pub mod supervisor;
pub mod wake;

use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

use crate::app_state::AppState;
use crate::protocol::error::ZwiimError;
use backend::{Capabilities, LidSupport, PowerBackend, PowerSnapshot, PowerSource};
use config::KeepAwakeConfig;
use policy::{Blocker, Decision, Mode, Warning};
use supervisor::Supervisor;

/// The supervisor this host runs, once [`spawn`] started it.
static RUNNING: OnceLock<Arc<Supervisor>> = OnceLock::new();

/// This platform's backend, detected once.
pub async fn backend() -> Arc<dyn PowerBackend> {
    static BACKEND: tokio::sync::OnceCell<Arc<dyn PowerBackend>> =
        tokio::sync::OnceCell::const_new();
    BACKEND.get_or_init(detect).await.clone()
}

#[cfg(target_os = "macos")]
async fn detect() -> Arc<dyn PowerBackend> {
    Arc::new(macos::MacBackend::detect().await)
}

#[cfg(not(target_os = "macos"))]
async fn detect() -> Arc<dyn PowerBackend> {
    Arc::new(backend::UnsupportedBackend)
}

/// Keep the host reachable for as long as it runs.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let supervisor = Supervisor::new(backend().await);
        if RUNNING.set(supervisor.clone()).is_err() {
            tracing::warn!("[power] already running");
            return;
        }
        supervisor.run(state.clone());
        wake::spawn(state);
    });
}

/// Look again now: the setting changed, or the computer woke.
pub fn nudge() {
    if let Some(supervisor) = RUNNING.get() {
        supervisor.nudge();
    }
}

/// What the operator chose; a field left out stays as it was.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeepAwakePatch {
    pub mode: Option<Mode>,
    pub on_battery: Option<bool>,
    pub lid_closed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KeepAwakeStatus {
    /// False where the host cannot keep the computer awake.
    pub supported: bool,
    pub mode: Mode,
    pub on_battery: bool,
    pub lid_closed: LidClosedStatus,
    /// The computer is being kept awake right now.
    pub active: bool,
    /// Why it is not, although the setting asks for it.
    pub blocked_by: Option<Blocker>,
    pub warnings: Vec<Warning>,
    pub power: PowerStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LidClosedStatus {
    pub support: LidSupport,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PowerStatus {
    pub source: PowerSource,
    pub battery_percent: Option<u8>,
}

/// `host.keep_awake.status`, read fresh.
pub async fn status(state: &AppState) -> Result<KeepAwakeStatus, ZwiimError> {
    let backend = backend().await;
    let (config, power, decision) = supervisor::evaluate(backend.as_ref(), state).await;
    let (active, lid_held) = match RUNNING.get() {
        Some(s) => (
            s.active_now(decision.hold_idle).await,
            s.lid_now(decision.hold_lid).await,
        ),
        None => (false, false),
    };
    let decision = supervisor::with_lid_warning(decision, lid_held);
    Ok(status_of(
        backend.capabilities(),
        &config,
        &power,
        decision,
        active,
    ))
}

/// The status a reader sees, from one evaluation and whether a hold is in
/// place.
fn status_of(
    caps: Capabilities,
    config: &KeepAwakeConfig,
    power: &PowerSnapshot,
    decision: Decision,
    active: bool,
) -> KeepAwakeStatus {
    KeepAwakeStatus {
        supported: caps.idle,
        mode: config.mode,
        on_battery: config.on_battery,
        lid_closed: LidClosedStatus {
            support: caps.lid,
            enabled: config.lid_closed,
        },
        active,
        blocked_by: decision.blocked_by,
        warnings: decision.warnings,
        power: PowerStatus {
            source: power.source,
            battery_percent: power.battery_percent,
        },
    }
}

/// `host.keep_awake.set`: change the setting, remember it, and act on it now.
pub async fn set(state: &AppState, patch: KeepAwakePatch) -> Result<KeepAwakeStatus, ZwiimError> {
    let mut c = config::load(&state.db.conn)
        .await
        .map_err(|_| ZwiimError::internal())?;
    c.mode = patch.mode.unwrap_or(c.mode);
    c.on_battery = patch.on_battery.unwrap_or(c.on_battery);
    c.lid_closed = patch.lid_closed.unwrap_or(c.lid_closed);
    config::save(&state.db.conn, &c)
        .await
        .map_err(|_| ZwiimError::internal())?;
    tracing::info!(
        "[power] keep_awake set mode={:?} on_battery={} lid_closed={}",
        c.mode,
        c.on_battery,
        c.lid_closed
    );
    nudge();
    status(state).await
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use crate::protocol::tests::{host, Host, HOST_TOKEN};

    async fn direct(h: &Host, op: &str, body: Value) -> (u16, Value) {
        let res = h
            .server
            .post(&format!("/zwiim/v1/{op}"))
            .authorization_bearer(HOST_TOKEN)
            .json(&body)
            .await;
        (res.status_code().as_u16(), res.json())
    }

    #[tokio::test]
    async fn keep_awake_defaults_to_always_and_remembers_changes() {
        let h = host(false).await;
        let status = h.ok("host.keep_awake.status", json!({})).await;
        assert_eq!(status["mode"], json!("always"));
        assert_eq!(status["on_battery"], json!(false));
        assert_eq!(status["lid_closed"]["enabled"], json!(false));
        assert_eq!(status["supported"], json!(cfg!(target_os = "macos")));
        assert!(
            ["ac", "battery", "unknown"].contains(&status["power"]["source"].as_str().unwrap()),
            "{status}"
        );
        assert!(status.get("enabled").is_none(), "{status}");

        let status = h.ok("host.keep_awake.set", json!({"mode": "off"})).await;
        assert_eq!(status["mode"], json!("off"));
        assert_eq!(
            super::config::load(&h.state.db.conn).await.unwrap().mode,
            super::Mode::Off
        );

        let status = h
            .ok("host.keep_awake.set", json!({"on_battery": true}))
            .await;
        assert_eq!(status["mode"], json!("off"));
        assert_eq!(status["on_battery"], json!(true));

        // Operator only.
        let res = h
            .server
            .post("/zwiim/v1/host.keep_awake.set")
            .json(&json!({"mode": "always"}))
            .await;
        assert_eq!(res.status_code().as_u16(), 401);
    }

    #[tokio::test]
    async fn set_refuses_unknown_fields_and_keeps_config() {
        let h = host(false).await;
        h.ok("host.keep_awake.set", json!({"mode": "while_working"}))
            .await;
        let before = super::config::load(&h.state.db.conn).await.unwrap();
        for body in [json!({"enabled": true}), json!({"mode": "sometimes"})] {
            let (code, value) = direct(&h, "host.keep_awake.set", body.clone()).await;
            assert_eq!(code, 400, "{body}: {value}");
            assert_eq!(
                value["error"]["code"],
                json!("invalid_request"),
                "{body}: {value}"
            );
        }
        assert_eq!(super::config::load(&h.state.db.conn).await.unwrap(), before);
    }

    #[tokio::test]
    async fn status_never_names_the_os_tool() {
        let h = host(false).await;
        let text = h.ok("host.keep_awake.status", json!({})).await.to_string();
        for word in ["caffeinate", "pmset", "ioreg", "IOPM"] {
            assert!(!text.contains(word), "{word} in {text}");
        }
    }
}
