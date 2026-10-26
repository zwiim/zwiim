//! The platform seam: what a power backend can do, what it reports, and the
//! holds it hands out. Holds are tied to the host process: if the host dies,
//! the OS drops them.

use futures::future::BoxFuture;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerSource {
    Ac,
    Battery,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Thermal {
    Nominal,
    Fair,
    Serious,
    Critical,
}

/// Whether, and how, the backend can keep the computer awake with its lid
/// closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LidSupport {
    /// No lid: a desktop computer.
    NoLid,
    /// A lid, and no way to keep it from sleeping the computer.
    Unavailable,
    /// Possible once the operator approves a helper.
    NeedsSetup,
    Available,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    /// Can prevent idle sleep.
    pub idle: bool,
    pub lid: LidSupport,
    /// Can report the power source and battery level.
    pub battery: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PowerSnapshot {
    pub source: PowerSource,
    pub battery_percent: Option<u8>,
    pub lid_closed: Option<bool>,
    pub thermal: Option<Thermal>,
}

impl PowerSnapshot {
    pub fn unknown() -> Self {
        Self {
            source: PowerSource::Unknown,
            battery_percent: None,
            lid_closed: None,
            thermal: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerError(pub String);

impl std::fmt::Display for PowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A hold on the computer; dropping it releases the hold.
pub trait HoldGuard: Send {
    /// False once the hold vanished on its own, so it can be replaced.
    fn is_alive(&mut self) -> bool;
}

pub trait PowerBackend: Send + Sync + 'static {
    fn capabilities(&self) -> Capabilities;
    fn snapshot(&self) -> BoxFuture<'_, PowerSnapshot>;
    /// Acquire an idle-sleep hold; dropping the guard releases it.
    fn hold_idle(&self, reason: &str) -> Result<Box<dyn HoldGuard>, PowerError>;
    /// Acquire a lid-close hold (only when capabilities say `Available`).
    fn hold_lid(&self) -> Result<Box<dyn HoldGuard>, PowerError>;
}

/// A platform the host cannot keep awake.
pub struct UnsupportedBackend;

impl PowerBackend for UnsupportedBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            idle: false,
            lid: LidSupport::NoLid,
            battery: false,
        }
    }

    fn snapshot(&self) -> BoxFuture<'_, PowerSnapshot> {
        Box::pin(async { PowerSnapshot::unknown() })
    }

    fn hold_idle(&self, _reason: &str) -> Result<Box<dyn HoldGuard>, PowerError> {
        Err(PowerError("not supported on this platform".into()))
    }

    fn hold_lid(&self) -> Result<Box<dyn HoldGuard>, PowerError> {
        Err(PowerError("not supported on this platform".into()))
    }
}
