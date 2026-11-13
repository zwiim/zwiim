//! The browser engine: a `chrome-headless-shell` child process the Host
//! downloads, starts and talks CDP to (design 5). Clients never see it; they
//! see the tabs `BrowserService` reports.
//!
//! - `install` — the pinned download, its hash check and unpacking
//! - `launch`  — starting it with the CDP pipe on fd 3 and 4
//! - `cdp`     — the CDP connection: calls, replies and events

pub mod cdp;
pub mod install;
#[cfg(unix)]
pub mod launch;
