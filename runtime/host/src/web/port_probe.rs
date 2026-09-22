use std::time::Duration;

use serde::Serialize;
use tokio::net::TcpStream;
use tokio::time::timeout;

/// Cap the probe at 200 ms so the settings page never visibly stalls.
/// On localhost a real listener answers in microseconds; only a hung
/// stack would even approach this bound.
const PROBE_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortState {
    /// `connect` was refused — no LISTENing socket on this port.
    Free,
    /// `connect` succeeded — some process holds a LISTEN socket.
    Occupied,
    /// Probe was inconclusive (timeout, unexpected I/O error). Treat
    /// like `Occupied` for warning purposes; bind may still succeed.
    Unknown,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebServicePortProbe {
    pub port: u16,
    pub state: PortState,
}

/// Determine whether `port` on loopback is currently being LISTENed on.
/// Probes via `connect` rather than `bind`: a `connect` succeeds iff
/// some process accepted the SYN, while `bind` would race with our
/// own subsequent server start.
pub async fn probe_port(port: u16) -> PortState {
    match timeout(PROBE_TIMEOUT, TcpStream::connect(("127.0.0.1", port))).await {
        Ok(Ok(_)) => PortState::Occupied,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => PortState::Free,
        _ => PortState::Unknown,
    }
}
