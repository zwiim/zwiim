//! The worktree's `.zwiim/` folder: user uploads and agent images. Created
//! with the worktree, excluded from Git before it exists, so it is visible in
//! the file tree but can never be staged.

use std::path::Path;

use crate::commands::folder_links;

/// Files the user attached to prompts.
pub(crate) const ATTACHMENTS: &str = ".zwiim/attachments";
/// Images agents captured or generated.
pub(crate) const IMAGES: &str = ".zwiim/images";

/// Best-effort: a failure is logged and the worktree is still created.
/// `attachments::store` and `agent_images::save` create the folders lazily
/// for worktrees made before this existed.
pub(super) async fn prepare(worktree: &Path) {
    folder_links::add_to_git_exclude(worktree, ".zwiim/").await;
    for dir in [ATTACHMENTS, IMAGES] {
        if let Err(e) = tokio::fs::create_dir_all(worktree.join(dir)).await {
            tracing::warn!("[worktree] could not create {dir} in {}: {e}", worktree.display());
        }
    }
}
