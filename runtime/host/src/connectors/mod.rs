//! The connector platform: outside services (remote MCP servers first) that
//! agents reach through the Host, which owns credentials, egress, permissions,
//! approvals and audit. Design: `~/zwiim-plans/2026-11-24-connector-platform-design-r3.md`.
//!
//! Remote MCP over Streamable HTTP is the only connection kind users can
//! create. Local stdio servers stay refused until the Phase 5 sandbox ships;
//! the stdio transport is only ever run against trusted test servers.

#[cfg(any(test, feature = "test-utils"))]
pub mod fake_mcp;
