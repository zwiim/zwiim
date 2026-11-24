//! [`McpUpstream`] over stdio framing, on streams Zwiim gives it. rmcp never
//! spawns, waits for or kills a process: whoever opened the streams owns the
//! child and its lifecycle.
//!
//! Compiled for tests only. Stdio servers run arbitrary programs, so no
//! production path may start one before the Phase 5 sandbox launcher exists;
//! until then this transport only ever talks to the trusted fake in
//! `connectors::fake_mcp`. Phase 5 lifts the `cfg` once the launcher owns
//! the child (process group, limits, kill).

use tokio::io::{AsyncRead, AsyncWrite};

use super::McpUpstream;
use crate::connectors::upstream::ConnectorError;

impl McpUpstream {
    /// Connects over one server's stdout (`read`) and stdin (`write`) and
    /// negotiates a version. `open` is called once per attempt: a server that
    /// only speaks a legacy version but answers `server/discover` with
    /// "unsupported version" needs a second, fresh process.
    pub async fn connect_stdio<R, W>(open: impl Fn() -> (R, W)) -> Result<Self, ConnectorError>
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        Self::connect(open).await
    }
}

#[cfg(test)]
mod tests;
