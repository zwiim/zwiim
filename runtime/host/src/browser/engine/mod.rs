//! The browser engine: a `chrome-headless-shell` child process the Host
//! downloads, starts and talks CDP to (design 5). Clients never see it; they
//! see the tabs `BrowserService` reports.
//!
//! - `install` — the pinned download, its hash check and unpacking

pub mod install;
