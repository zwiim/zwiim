//! Process-runtime + (stubbed) self-update seam.
//!
//! Upstream-derived: `runtime` is carried verbatim from upstream
//! (`src-tauri/src/update/runtime.rs`, v0.31.1; see NOTICE) — it holds pure process /
//! supervise / restart constants and helpers (`self_exe`, `EXIT_RESTART`,
//! `is_supervised`, restart delays) that the Host Runtime bootstrap and the
//! `--supervise` process supervisor both depend on. It is exec-lifecycle
//! infrastructure, not the updater.
//!
//! Zwiim-owned, replacing the upstream minisign in-place self-updater
//! (discarded per CLAUDE.md):
//!
//! - `manifest` + `signature`: the signed release manifest (R9.5).
//! - `install`: versioned Host installs, staging, trial and rollback.
//! - `service`: discover → verify → download → stage → apply → trial.
pub mod install;
pub mod manifest;
pub mod runtime;
pub mod service;
pub mod signature;

#[cfg(test)]
mod tests;
