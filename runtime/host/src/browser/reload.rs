//! Reload on save (design 10.5): a session tab whose dev server has no hot
//! reload loads again once files in its worktree stop changing. Opt-in per
//! tab; the watch goes with the tab or when turned off.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Quiet time after the last change before the tab reloads.
pub const SETTLE: Duration = Duration::from_millis(300);

/// Folders whose changes are not the person's edits: Git's own, Zwiim's
/// (the tab's downloads land there), and installed packages.
const IGNORED: &[&str] = &[".git", ".zwiim", "node_modules"];

/// Whether a change at `path`, under `root`, is one to reload for.
pub fn counts(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    !rel.components()
        .any(|c| matches!(c, Component::Normal(name) if IGNORED.iter().any(|i| name == *i)))
}

/// A worktree being watched for one tab. Dropping it stops the watch.
pub struct ReloadWatch {
    _watcher: RecommendedWatcher,
    task: JoinHandle<()>,
}

impl Drop for ReloadWatch {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Watch `root`, and call `reload` once [`SETTLE`] after each burst of
/// changes that [`counts`].
pub fn watch(root: PathBuf, reload: impl Fn() + Send + 'static) -> notify::Result<ReloadWatch> {
    let (tx, mut rx) = mpsc::unbounded_channel::<()>();
    let watched = root.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        if event.paths.iter().any(|p| counts(&watched, p)) {
            let _ = tx.send(());
        }
    })?;
    watcher.watch(&root, RecursiveMode::Recursive)?;
    let task = tokio::spawn(async move {
        while rx.recv().await.is_some() {
            // Wait until changes stop for SETTLE.
            loop {
                match tokio::time::timeout(SETTLE, rx.recv()).await {
                    Ok(Some(())) => continue,
                    Ok(None) => return,
                    Err(_) => break,
                }
            }
            reload();
        }
    });
    Ok(ReloadWatch {
        _watcher: watcher,
        task,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn edits_count_but_git_zwiim_and_packages_do_not() {
        let root = Path::new("/w");
        assert!(counts(root, Path::new("/w/src/app.js")));
        assert!(counts(root, Path::new("/w/index.html")));
        for skipped in [
            "/w/.git/index",
            "/w/.zwiim/browser/a/report.pdf",
            "/w/node_modules/x/y.js",
            "/w/web/node_modules/z.js",
        ] {
            assert!(!counts(root, Path::new(skipped)), "{skipped}");
        }
        assert!(!counts(root, Path::new("/elsewhere/a.js")));
    }

    #[tokio::test]
    async fn a_burst_of_saves_reloads_once_after_they_settle() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let reloads = Arc::new(AtomicUsize::new(0));
        let counted = reloads.clone();
        let watch = watch(root.clone(), move || {
            counted.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        // Let the watcher start before writing.
        tokio::time::sleep(Duration::from_millis(200)).await;
        for i in 0..5 {
            std::fs::write(root.join("app.js"), format!("v{i}")).unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while reloads.load(Ordering::SeqCst) == 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(SETTLE * 2).await;
        assert_eq!(reloads.load(Ordering::SeqCst), 1);

        // Git's own writes don't reload.
        std::fs::write(root.join(".git").join("index"), "x").unwrap();
        tokio::time::sleep(SETTLE * 3).await;
        assert_eq!(reloads.load(Ordering::SeqCst), 1);

        // Turned off, nothing more.
        drop(watch);
        std::fs::write(root.join("app.js"), "after").unwrap();
        tokio::time::sleep(SETTLE * 3).await;
        assert_eq!(reloads.load(Ordering::SeqCst), 1);
    }
}
