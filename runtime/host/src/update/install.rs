//! Zwiim-owned stub for the in-place self-updater.
//!
//! Replaces the upstream minisign-verified download/swap installer (discarded per
//! CLAUDE.md — see `third_party/codeg/PROVENANCE.md`). This build ships no
//! self-update path, so:
//!
//! - nothing is ever staged (`upgrade_staged` / `take_upgrade_staged` are
//!   always `false`) — the supervisor treats every launch as a plain restart
//!   and never puts a worker on rollback probation;
//! - there is nothing to roll back to (`rollback` reports unsupported,
//!   `rollback_available` is `false`);
//! - no release asset is published for this runtime (`asset_basename` is
//!   `None`).
//!
//! A real Zwiim updater is a later slice; keep this seam minimal until then.

use crate::app_error::{AppCommandError, AppErrorCode};

/// Whether a freshly-swapped upgrade is awaiting its trial window. Always
/// `false`: this build never stages an upgrade.
pub fn upgrade_staged() -> bool {
    false
}

/// Consume the staged-upgrade marker (returning whether one was present).
/// Always `false`: nothing is ever staged.
pub fn take_upgrade_staged() -> bool {
    false
}

/// Whether a previous bundle is available to revert to. Always `false`: this
/// build performs no in-place swaps, so no `.bak` bundle exists.
pub fn rollback_available() -> bool {
    false
}

/// Revert to the previous bundle. Unsupported in this build — there is no
/// installer and nothing to roll back to.
pub fn rollback() -> Result<(), AppCommandError> {
    Err(AppCommandError::new(
        AppErrorCode::DependencyMissing,
        "self-update / rollback is not available in this build",
    ))
}
