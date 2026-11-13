// Modified from upstream Apache-2.0 source; see NOTICE.
//! Built-in browser.
//!
//! A browser tab is a Rust-owned webview that renders an arbitrary web page
//! next to the chat: on macOS / Windows a wry child webview embedded in the
//! workspace window at the bounds of a placeholder element (see Cargo.toml's
//! `browser-child` note for why wry is driven directly rather than through
//! tauri's `add_child`), on Linux (and as
//! the fallback everywhere) an owned top-level window. The frontend only ever
//! talks to it through the `browser_*` commands and the `browser://*` events;
//! the page itself has no Tauri IPC at all — `browser-*` labels are absent from
//! every capability on purpose, so a page script cannot reach any command.
//!
//! Module map:
//! - `types`      — wire types shared with `src/lib/browser/types.ts`
//! - `engine`     — the Host's own Chromium (`chrome-headless-shell`): download,
//!   launch and CDP; compiled in both runtimes
//! - `service`    — `BrowserService`: the Host browser's tabs, one context per
//!   owner, restored after restarts
//! - `snapshot`   — the personal context's cookies, sealed with a keychain key
//! - `store`      — the Host browser's persisted tabs and their owners
//! - `stream`     — `StreamHub`: a tab's screencast as binary frames to its
//!   viewers
//! - `input`      — a viewer's pointer, keys and text, as CDP `Input` calls
//! - `policy`     — pure decisions (scheme allow-list, …)
//! - `agent`      — what an agent may read of a page, and on whose say-so
//! - `eval`       — the one thing an agent may do that is not a named act:
//!   run its own code, which a person approves snippet by snippet
//! - `listener`   — which program is serving a loopback address, for grants
//!   made on one (`http://localhost:3000` names a port, not a site)
//! - `service_url` — reading a dev server's address out of terminal output
//! - `services`   — which of those addresses are live, and telling the
//!   workspace about a new one so it can offer to open it
//! - `blank_page` — the empty tab's own page: `about:blank` in the app's
//!   colours rather than the engine's white
//! - `doc_guest`  — the `zwiim-doc:` guest that shows a local HTML file
//! - `profile`    — the tabs' own data store / directory and their proxy
//! - `downloads`  — destination policy and records for page downloads
//! - `registry`   — tab id → surface + last known state
//! - `surface`    — the enum over the concrete surfaces and their common ops
//! - `surface_child` / `surface_window` — the concrete builders
//! - `channel`    — page → host messages from the isolated-world helper
//! - `confirm`    — the one question `browser_eval` puts to a person, and the
//!   quiet period a refusal buys
//! - `handoff`    — the other direction from `agent`: what a PERSON hands to
//!   a conversation (an element they picked, the console) and how it is
//!   rendered as untrusted page content
//! - `shim`       — per-platform WebKit / WebView2 calls (worlds, eval, snapshot)
//! - `hooks`      — webview callbacks (page load, title) → registry + events
//! - `events`     — state fan-out to the frontend
//! - `smoke`      — dev-only puppet driven by a JSON control file (feature
//!   `browser-smoke`, never in a release build)

// `agent` and `types` are pure data and pure rules — serde and nothing else —
// and they are compiled in BOTH runtimes. The zwiim-mcp plumbing that carries
// the browser tools (`acp::browser_tools`, the broker wire, the companion) is
// shared code, and it has to name the same `PageSnapshot` and the same
// `GrantLevel` the desktop build produces. A second copy of those types for
// the server build would be two wire formats one rename apart from disagreeing
// silently.
//
// Everything below them touches a webview, and so is desktop-only: server mode
// renders a "browser tab" as an iframe in the user's own browser, which this
// process has no handle on at all.
// The terminal that prints a dev server's address runs in both runtimes, so
// the watch that notices it does too — `service_url` is pure text work and
// `services` is a socket probe plus an event; neither touches a webview.
pub mod agent;
pub mod capture;
pub mod console;
pub mod engine;
pub mod eval;
pub mod helper;
pub mod network;
pub mod service_url;
pub mod service;
pub mod services;
pub mod snapshot;
pub mod store;
pub mod stream;
pub mod tools;
pub mod input;
pub mod saving;
pub mod types;
pub mod ui;

#[cfg(feature = "tauri-runtime")]
pub mod blank_page;
#[cfg(feature = "tauri-runtime")]
pub mod channel;
#[cfg(feature = "tauri-runtime")]
pub mod confirm;
#[cfg(feature = "tauri-runtime")]
pub mod doc_guest;
#[cfg(feature = "tauri-runtime")]
pub mod downloads;
#[cfg(feature = "tauri-runtime")]
pub mod events;
#[cfg(feature = "tauri-runtime")]
pub mod handoff;
#[cfg(feature = "tauri-runtime")]
pub mod hooks;
#[cfg(feature = "tauri-runtime")]
pub mod listener;
#[cfg(feature = "tauri-runtime")]
pub mod open_request;
#[cfg(feature = "tauri-runtime")]
pub mod policy;
#[cfg(feature = "tauri-runtime")]
pub mod profile;
#[cfg(feature = "tauri-runtime")]
pub mod registry;
#[cfg(feature = "tauri-runtime")]
pub mod surface;
#[cfg(all(
    feature = "browser-child",
    feature = "tauri-runtime",
    any(target_os = "macos", target_os = "windows")
))]
#[cfg(feature = "tauri-runtime")]
pub mod surface_window;

#[cfg(feature = "tauri-runtime")]
pub mod shim;

#[cfg(feature = "browser-smoke")]
pub mod smoke;

#[cfg(feature = "tauri-runtime")]
pub use doc_guest::DocGuests;
#[cfg(feature = "tauri-runtime")]
pub use downloads::BrowserDownloads;
#[cfg(feature = "tauri-runtime")]
pub use registry::BrowserRegistry;

/// Label prefix of every browser tab webview / window. Nothing under this
/// prefix may ever appear in `capabilities/*.json`.
pub const TAB_LABEL_PREFIX: &str = "browser-";

pub fn tab_label(tab_id: &str) -> String {
    format!("{TAB_LABEL_PREFIX}{tab_id}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn tab_labels_carry_the_prefix() {
        assert_eq!(super::tab_label("abc"), "browser-abc");
    }
}
