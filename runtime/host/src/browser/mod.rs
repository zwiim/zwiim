// Modified from upstream Apache-2.0 source; see NOTICE.
//! Built-in browser.
//!
//! The Host runs its own Chromium (`chrome-headless-shell`) and owns every
//! tab; clients view a tab through its screencast and steer it through the
//! Zwiim protocol (browser design 4).
//!
//! Module map:
//! - `types`      — wire types shared with the tools and clients
//! - `engine`     — the Host's own Chromium: download, launch and CDP
//! - `service`    — `BrowserService`: the Host browser's tabs, one context per
//!   owner, restored after restarts
//! - `snapshot`   — the personal context's cookies, sealed with a keychain key
//! - `store`      — the Host browser's persisted tabs and their owners
//! - `stream`     — `StreamHub`: a tab's screencast as binary frames to its
//!   viewers
//! - `input`      — a viewer's pointer, keys and text, as CDP `Input` calls
//! - `ui`         — what a page asks of a person (dialogs, sign-in, files)
//! - `helper`     — the isolated-world helper: selects, date pickers, menus,
//!   find in page
//! - `tools`      — the agent browser tools over CDP
//! - `saving`     — where downloads and printed pages go
//! - `policy`     — pure decisions (scheme allow-list, administrator rules, …)
//! - `governor`   — the Host browser's memory budget, ladder and priority
//! - `sites`      — which sites a tab's page loads may reach (site policy)
//! - `dev_servers` — a session's new dev server, shown in its tab
//! - `agent`      — what an agent may read of a page, and on whose say-so
//! - `eval`       — the one thing an agent may do that is not a named act:
//!   run its own code, which a person approves snippet by snippet
//! - `capture`, `console`, `network` — screenshots, console and network logs
//! - `listener`   — which program is serving a loopback address
//! - `service_url` — reading a dev server's address out of terminal output
//! - `services`   — which of those addresses are live
//! - `handoff`    — the other direction from `agent`: what a PERSON hands to
//!   a conversation (an element they picked, the console) and how it is
//!   rendered as untrusted page content

pub mod agent;
pub mod capture;
pub mod console;
pub mod dev_servers;
pub mod engine;
pub mod eval;
pub mod governor;
pub mod handoff;
pub mod helper;
pub mod network;
pub mod service_url;
pub mod service;
pub mod sites;
pub mod services;
pub mod snapshot;
pub mod store;
pub mod stream;
pub mod tools;
pub mod input;
pub mod listener;
pub mod policy;
pub mod saving;
pub mod types;
pub mod ui;
