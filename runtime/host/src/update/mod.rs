//! Process-runtime + (stubbed) self-update seam.
//!
//! Codeg-derived provenance: `runtime` is carried verbatim from Codeg
//! (`src-tauri/src/update/runtime.rs`, v0.31.1) — it holds pure process /
//! supervise / restart constants and helpers (`self_exe`, `EXIT_RESTART`,
//! `is_supervised`, restart delays) that the Host Runtime bootstrap and the
//! `--supervise` process supervisor both depend on. It is exec-lifecycle
//! infrastructure, not the updater.
//!
//! `install` is a Zwiim-owned STUB that replaces Codeg's minisign in-place
//! self-updater. Per CLAUDE.md the Codeg updater is discarded and a Zwiim
//! updater is a later slice; until then this build ships no self-update path:
//! no upgrade is ever staged and there is nothing to roll back. Keeping the
//! seam (rather than deleting it) lets the supervisor and bootstrap compile
//! and behave correctly (plain restart, never auto-rollback).
pub mod install;
pub mod runtime;
