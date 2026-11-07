// Modified from upstream Apache-2.0 source; see NOTICE.
//! Shared link engine for skills in the central store.
//!
//! Skills live in `~/.zwiim/skills/<id>/`. Enabling one for an ACP agent means
//! creating a symbolic link (or Windows junction) from the agent's skill
//! directory into the central copy, so "enabled" is purely "does a link exist".
//! Custom skills use these primitives and DTOs.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::models::agent::AgentType;

const CENTRAL_DIR_NAME: &str = ".zwiim";
const CENTRAL_SKILLS_SUBDIR: &str = "skills";

// ─── Types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExpertLinkState {
    NotLinked,
    LinkedToZwiim,
    LinkedElsewhere,
    BlockedByRealDirectory,
    Broken,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertInstallStatus {
    pub expert_id: String,
    pub agent_type: AgentType,
    pub state: ExpertLinkState,
    pub link_path: String,
    pub target_path: Option<String>,
    pub expected_target_path: String,
    pub copy_mode: bool,
}

/// A single enable/disable request for one (skill, agent) pair, used by the
/// batch `*_apply_links` commands. `expert_id` is the central-store id — for
/// office tools it carries the office skill id (mirroring how
/// `ExpertInstallStatus.expert_id` already doubles as the office skill id).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkOp {
    pub expert_id: String,
    pub agent_type: AgentType,
    pub enable: bool,
}

/// Per-op outcome of a batch apply. A failed op never aborts the rest of the
/// batch; the caller inspects `ok`/`error` per entry and re-fetches the
/// authoritative snapshot afterwards.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkOpResult {
    pub expert_id: String,
    pub agent_type: AgentType,
    pub ok: bool,
    /// Present on a successful enable; `None` for disables and failures.
    pub status: Option<ExpertInstallStatus>,
    pub error: Option<String>,
}

// ─── Paths ──────────────────────────────────────────────────────────────

fn home_dir_or_default() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn central_experts_dir() -> PathBuf {
    home_dir_or_default()
        .join(CENTRAL_DIR_NAME)
        .join(CENTRAL_SKILLS_SUBDIR)
}

// ─── Link operations ────────────────────────────────────────────────────

#[cfg(unix)]
pub(crate) fn create_link_raw(src: &Path, dst: &Path) -> io::Result<bool> {
    std::os::unix::fs::symlink(src, dst).map(|_| false)
}

#[cfg(windows)]
pub(crate) fn create_link_raw(src: &Path, dst: &Path) -> io::Result<bool> {
    match junction::create(src, dst) {
        Ok(_) => Ok(false),
        Err(junction_err) => {
            // Fall back to recursive copy when junction creation is not
            // possible (e.g. cross-volume, permission denied). Mark the
            // returned status with copy_mode = true so the UI can warn
            // the user that upgrades won't propagate automatically.
            copy_dir_recursive(src, dst).map_err(|copy_err| {
                io::Error::other(format!(
                    "junction failed ({junction_err}); copy fallback failed ({copy_err})"
                ))
            })?;
            Ok(true)
        }
    }
}

#[cfg(windows)]
fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ft.is_file() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Best-effort human-readable link target. On Windows, `fs::read_link`
/// does not resolve junctions in all stdlib versions — prefer the
/// `junction` crate when the path is a reparse point.
pub(crate) fn read_link_target(path: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if path_is_reparse_point(path) {
            if let Ok(target) = junction::get_target(path) {
                return Some(target);
            }
        }
    }
    fs::read_link(path).ok()
}

pub(crate) fn path_is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// On Windows a junction is *not* a symlink — it is a directory reparse
/// point. `symlink_metadata` reports it as a directory. So we also need to
/// ask the OS whether the directory is a reparse point.
#[cfg(windows)]
fn path_is_reparse_point(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    fs::symlink_metadata(path)
        .map(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn path_is_reparse_point(_path: &Path) -> bool {
    false
}

/// Equality check for two already-canonicalized paths. On Windows the
/// filesystem is case-insensitive but `Path` comparison is not — canonical
/// forms can still differ in drive-letter case or user-supplied casing.
fn paths_equivalent(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    #[cfg(windows)]
    {
        let a_s = a.as_os_str().to_string_lossy();
        let b_s = b.as_os_str().to_string_lossy();
        a_s.eq_ignore_ascii_case(b_s.as_ref())
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Resolve a path while following symlinks and Windows junctions.
/// Returns `None` if the path does not exist or cannot be resolved (e.g.
/// dangling link).
fn resolve_real_path(path: &Path) -> Option<PathBuf> {
    fs::canonicalize(path).ok()
}

pub(crate) fn classify_link(link_path: &Path, expected_target: &Path) -> ExpertLinkState {
    // No entry at all (not even a dangling link) → not linked.
    let meta = match fs::symlink_metadata(link_path) {
        Ok(m) => m,
        Err(_) => return ExpertLinkState::NotLinked,
    };

    let is_link_like = meta.file_type().is_symlink() || path_is_reparse_point(link_path);
    if !is_link_like {
        // A real directory (or file) sits where we'd put our link.
        // This also covers Windows copy-mode fallback, where we could not
        // create a junction and fell back to `copy_dir_recursive`. We still
        // surface it as BlockedByRealDirectory so callers
        // treat it as "needs user attention" (the copy will not track
        // central-store updates and must be re-linked explicitly).
        return ExpertLinkState::BlockedByRealDirectory;
    }

    // `fs::canonicalize` transparently follows both symlinks and Windows
    // junctions, so comparing the two canonical forms is the single
    // source of truth for "does this link point at our central store?".
    // We intentionally do *not* rely on `fs::read_link`'s string output
    // for equality — on Windows junctions its output format is
    // stdlib-version-dependent and often fails to round-trip through
    // `canonicalize` cleanly.
    let resolved_link = resolve_real_path(link_path);
    let resolved_expected = resolve_real_path(expected_target);

    match (resolved_link, resolved_expected) {
        (None, _) => ExpertLinkState::Broken,
        (Some(l), Some(e)) if paths_equivalent(&l, &e) => ExpertLinkState::LinkedToZwiim,
        _ => ExpertLinkState::LinkedElsewhere,
    }
}
