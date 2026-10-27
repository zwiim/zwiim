//! `workspace.resolve_conflicts`: an agent turn, written by the Host, that
//! resolves a worktree's conflicts with its base.

use std::path::Path;

use serde::Serialize;

use crate::acp::manager::ConnectionManager;
use crate::acp::types::PromptInputBlock;
use crate::db::AppDatabase;
use crate::git_harness::{self, ConflictCheck, ConflictState, GitHost, PrLifecycle};
use crate::harness::{resolve_conflicts_prompt, ResolveFacts, ResolveStrategy};
use crate::protocol::error::ZwiimError;
use crate::session::{self, AgentLauncher};

/// The conflicts the resolve turn was sent for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolveStarted {
    pub base: String,
    pub files: Vec<String>,
}

/// The `Zwiim-Action` a resolve turn's start checkpoint carries.
pub(crate) const RESOLVE_ACTION: &str = "resolve_conflicts";

/// Whether a resolve turn pushes, given the state of the worktree's pull
/// request: only while one is open, draft or not, so it updates.
pub(crate) fn pushes_to(pr: Option<PrLifecycle>) -> bool {
    pr.is_some_and(|state| !state.is_final())
}

/// Send session `session_id` the Host's resolve text for workspace
/// `workspace_id`'s conflicts with its base, checked against a fresh fetch
/// of that base. `push` says whether the agent pushes the result.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn resolve_conflicts<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    data_dir: &Path,
    workspace_id: &str,
    session_id: &str,
    strategy: ResolveStrategy,
    push: bool,
) -> Result<ResolveStarted, ZwiimError> {
    let folder = super::resolve(&db.conn, workspace_id).await?;
    if folder.parent_id.is_none() {
        return Err(ZwiimError::root_workspace());
    }
    let row = session::resolve(&db.conn, session_id).await?;
    if row.folder_id != folder.id {
        return Err(ZwiimError::not_found("Session not found in this workspace"));
    }
    // Refuse a busy session or worktree before the fetch, not after it.
    if session::row_busy(connections, &row).await {
        return Err(ZwiimError::turn_in_progress());
    }
    if session::other_busy_in_folder(&db.conn, connections, folder.id, row.id).await? {
        return Err(ZwiimError::worktree_busy());
    }

    let root = Path::new(&folder.path);
    let Some(base) = git_harness::conflict_base(root, true, folder.base_branch.as_deref()).await?
    else {
        return Err(ZwiimError::no_conflicts());
    };
    let host = GitHost::new(&db.conn, data_dir);
    if let Err(e) = git_harness::fetch_base(&host, root, &base).await {
        tracing::debug!(
            "[workspace] fetch of {base} before resolving failed: {}",
            e.message
        );
    }
    let (files, in_progress) = match git_harness::check_conflicts(root, &base).await? {
        Some(ConflictCheck {
            state: ConflictState::Conflicts { files },
            ..
        }) => (files, None),
        Some(ConflictCheck {
            state: ConflictState::InProgress { operation, files },
            ..
        }) if !files.is_empty() => (files, Some(operation)),
        _ => return Err(ZwiimError::no_conflicts()),
    };
    // Another session may have started a turn during the fetch.
    if session::other_busy_in_folder(&db.conn, connections, folder.id, row.id).await? {
        return Err(ZwiimError::worktree_busy());
    }

    let text = resolve_conflicts_prompt(&ResolveFacts {
        base: &base,
        files: &files,
        strategy,
        push,
        in_progress: in_progress.as_deref(),
        user_addition: None,
    });
    session::send_tagged(
        db,
        connections,
        launcher,
        session_id,
        vec![PromptInputBlock::Text { text }],
        None,
        Some(RESOLVE_ACTION),
    )
    .await?;
    tracing::info!(
        workspace_id,
        %base,
        files = files.len(),
        "[workspace] resolve conflicts turn sent"
    );
    Ok(ResolveStarted { base, files })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

    use super::*;
    use crate::db::entities::folder;
    use crate::db::test_helpers::fresh_in_memory_db;
    use crate::git_harness::checkpoint;
    use crate::models::agent::AgentType;
    use crate::session::orchestration_tests::{
        end_turn_observed, fixture_git, git_workspace, settle, text, FakeLauncher,
    };

    struct Rig {
        _tmp: tempfile::TempDir,
        db: AppDatabase,
        wid: String,
        dir: PathBuf,
        /// A clone of `origin`, to move its `main` behind the worktree's back.
        upstream: PathBuf,
        data_dir: PathBuf,
    }

    /// A worktree on branch `feature` whose base is `origin/main`. With
    /// `conflict`, `origin/main` (already fetched) changed `a.txt` one way
    /// and `feature` another.
    async fn rig(conflict: bool) -> Rig {
        let db = fresh_in_memory_db().await;
        let (wid, dir) = git_workspace(&db).await;
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin.git");
        fixture_git(
            tmp.path(),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        fixture_git(&dir, &["remote", "add", "origin", origin.to_str().unwrap()]);
        fixture_git(&dir, &["push", "-q", "origin", "main"]);
        let upstream = tmp.path().join("upstream");
        fixture_git(
            tmp.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                upstream.to_str().unwrap(),
            ],
        );
        if conflict {
            commit_a(&upstream, "theirs\n");
            fixture_git(&upstream, &["push", "-q", "origin", "main"]);
        }
        fixture_git(&dir, &["checkout", "-q", "-b", "feature"]);
        commit_a(&dir, "ours\n");
        fixture_git(&dir, &["fetch", "-q", "origin"]);

        let row = folder::Entity::find()
            .filter(folder::Column::WorkspaceId.eq(wid.as_str()))
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        let mut row: folder::ActiveModel = row.into();
        row.base_branch = Set(Some("origin/main".into()));
        row.update(&db.conn).await.unwrap();

        let data_dir = tmp.path().join("data");
        std::fs::create_dir(&data_dir).unwrap();
        Rig {
            _tmp: tmp,
            db,
            wid,
            dir,
            upstream,
            data_dir,
        }
    }

    fn commit_a(dir: &Path, content: &str) {
        std::fs::write(dir.join("a.txt"), content).unwrap();
        fixture_git(dir, &["commit", "-q", "-am", content.trim()]);
    }

    /// A session in the rig's workspace whose first turn has ended.
    async fn idle_session(r: &Rig, mgr: &ConnectionManager, launcher: &FakeLauncher) -> String {
        let sid = crate::session::start(
            &r.db,
            mgr,
            launcher,
            &r.wid,
            AgentType::Codex,
            text("first"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        settle().await;
        end_turn_observed(mgr, "fake-conn-1", "end_turn").await;
        settle().await;
        sid
    }

    async fn resolve(
        r: &Rig,
        mgr: &ConnectionManager,
        launcher: &FakeLauncher,
        sid: &str,
    ) -> Result<ResolveStarted, ZwiimError> {
        resolve_conflicts(
            &r.db,
            mgr,
            launcher,
            &r.data_dir,
            &r.wid,
            sid,
            ResolveStrategy::Merge,
            true,
        )
        .await
    }

    #[test]
    fn pushes_to_open_and_draft_pull_requests() {
        use crate::git_harness::PrLifecycle::*;
        assert!(pushes_to(Some(Open)));
        assert!(pushes_to(Some(Draft)));
        for settled in [None, Some(Merged), Some(Closed)] {
            assert!(!pushes_to(settled), "{settled:?}");
        }
    }

    #[tokio::test]
    async fn resolve_sends_turn_with_host_text() {
        let r = rig(true).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&r, &mgr, &launcher).await;

        let started = resolve(&r, &mgr, &launcher, &sid).await.unwrap();
        settle().await;

        let files = vec!["a.txt".to_string()];
        assert_eq!(
            started,
            ResolveStarted {
                base: "origin/main".into(),
                files: files.clone(),
            }
        );
        let expected = resolve_conflicts_prompt(&ResolveFacts {
            base: "origin/main",
            files: &files,
            strategy: ResolveStrategy::Merge,
            push: true,
            in_progress: None,
            user_addition: None,
        });
        assert_eq!(
            launcher.prompts_for("fake-conn-1"),
            ["first", expected.as_str()]
        );
    }

    #[tokio::test]
    async fn resolve_finishes_a_merge_in_progress() {
        let r = rig(true).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&r, &mgr, &launcher).await;
        // The user started the merge and it stopped on conflicts.
        let merge = std::process::Command::new("git")
            .args(["merge", "-q", "origin/main"])
            .current_dir(&r.dir)
            .output()
            .unwrap();
        assert!(!merge.status.success());
        assert!(r.dir.join(".git/MERGE_HEAD").exists());

        let started = resolve(&r, &mgr, &launcher, &sid).await.unwrap();
        settle().await;

        let files = vec!["a.txt".to_string()];
        assert_eq!(started.files, files);
        let expected = resolve_conflicts_prompt(&ResolveFacts {
            base: "origin/main",
            files: &files,
            strategy: ResolveStrategy::Merge,
            push: true,
            in_progress: Some("merge"),
            user_addition: None,
        });
        assert_eq!(
            launcher.prompts_for("fake-conn-1"),
            ["first", expected.as_str()]
        );
    }

    #[tokio::test]
    async fn resolve_turn_start_checkpoint_carries_action() {
        let r = rig(true).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&r, &mgr, &launcher).await;

        resolve(&r, &mgr, &launcher, &sid).await.unwrap();

        let listed = checkpoint::list(&r.dir, &sid).await.unwrap();
        let actions: Vec<_> = listed.iter().map(|c| c.action.as_deref()).collect();
        assert_eq!(actions.first(), Some(&None), "{actions:?}");
        assert_eq!(actions.last(), Some(&Some(RESOLVE_ACTION)), "{actions:?}");
    }

    #[tokio::test]
    async fn resolve_refuses_clean() {
        let r = rig(false).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&r, &mgr, &launcher).await;

        let err = resolve(&r, &mgr, &launcher, &sid).await.unwrap_err();
        assert_eq!(err.code, "no_conflicts");
        assert_eq!(launcher.prompts_for("fake-conn-1"), ["first"]);
    }

    #[tokio::test]
    async fn resolve_rechecks_after_fetch() {
        let r = rig(true).await;
        // The base is fixed upstream after the worktree last fetched it.
        commit_a(&r.upstream, "ours\n");
        fixture_git(&r.upstream, &["push", "-q", "origin", "main"]);
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&r, &mgr, &launcher).await;

        let err = resolve(&r, &mgr, &launcher, &sid).await.unwrap_err();
        assert_eq!(err.code, "no_conflicts");
        assert_eq!(launcher.prompts_for("fake-conn-1"), ["first"]);
    }

    #[tokio::test]
    async fn resolve_refuses_turn_in_progress() {
        let r = rig(true).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = crate::session::start(
            &r.db,
            &mgr,
            &launcher,
            &r.wid,
            AgentType::Codex,
            text("first"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        settle().await;

        let err = resolve(&r, &mgr, &launcher, &sid).await.unwrap_err();
        assert_eq!(err.code, "turn_in_progress");
        assert_eq!(launcher.prompts_for("fake-conn-1"), ["first"]);

        // The refused resolve leaves nothing behind: the next plain turn's
        // start checkpoint carries no action.
        end_turn_observed(&mgr, "fake-conn-1", "end_turn").await;
        settle().await;
        crate::session::send(&r.db, &mgr, &launcher, &sid, text("second"), None)
            .await
            .unwrap();
        let listed = checkpoint::list(&r.dir, &sid).await.unwrap();
        assert!(listed.len() >= 2, "{listed:?}");
        assert!(listed.iter().all(|c| c.action.is_none()), "{listed:?}");
    }

    /// A merge rewrites the worktree: not while another agent works in it,
    /// and never by two resolve turns at once.
    #[tokio::test]
    async fn resolve_refuses_while_another_session_works_here() {
        let r = rig(true).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&r, &mgr, &launcher).await;
        crate::session::start(
            &r.db,
            &mgr,
            &launcher,
            &r.wid,
            AgentType::Codex,
            text("other"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        settle().await;

        let err = resolve(&r, &mgr, &launcher, &sid).await.unwrap_err();
        assert_eq!(err.code, "turn_in_progress");
        assert_eq!(launcher.prompts_for("fake-conn-1"), ["first"]);

        // Once the other turn ends, the resolve goes through.
        end_turn_observed(&mgr, "fake-conn-2", "end_turn").await;
        settle().await;
        resolve(&r, &mgr, &launcher, &sid).await.unwrap();
    }

    /// A turn another session starts while the base is being fetched still
    /// stops the resolve.
    #[tokio::test]
    async fn resolve_refuses_a_turn_started_during_the_fetch() {
        let r = rig(true).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&r, &mgr, &launcher).await;
        // The fetch's upload-pack waits for `go` after announcing `fetching`.
        let tmp = r.data_dir.parent().unwrap();
        let (fetching, go) = (tmp.join("fetching"), tmp.join("go"));
        let pack = tmp.join("upload-pack.sh");
        std::fs::write(
            &pack,
            format!(
                "#!/bin/sh\ntouch '{}'\nwhile [ ! -e '{}' ]; do sleep 0.05; done\nexec git-upload-pack \"$@\"\n",
                fetching.display(),
                go.display()
            ),
        )
        .unwrap();
        fixture_git(
            &r.dir,
            &["config", "remote.origin.uploadpack", pack.to_str().unwrap()],
        );
        std::process::Command::new("chmod")
            .args(["+x", pack.to_str().unwrap()])
            .status()
            .unwrap();

        let other = async {
            while !fetching.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            crate::session::start(
                &r.db,
                &mgr,
                &launcher,
                &r.wid,
                AgentType::Codex,
                text("other"),
                None,
                &Default::default(),
            )
            .await
            .unwrap();
            settle().await;
            std::fs::write(&go, "").unwrap();
        };
        let (result, ()) = tokio::join!(resolve(&r, &mgr, &launcher, &sid), other);

        assert_eq!(result.unwrap_err().code, "turn_in_progress");
        assert_eq!(launcher.prompts_for("fake-conn-1"), ["first"]);
    }

    #[tokio::test]
    async fn resolve_refuses_a_session_of_another_workspace() {
        let r = rig(true).await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&r, &mgr, &launcher).await;
        let other = crate::session::orchestration_tests::workspace(&r.db).await;

        let err = resolve_conflicts(
            &r.db,
            &mgr,
            &launcher,
            &r.data_dir,
            &other,
            &sid,
            ResolveStrategy::Merge,
            false,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "not_found");
        assert_eq!(launcher.prompts_for("fake-conn-1"), ["first"]);
    }
}
