//! `continue_after_merge`: a worktree whose pull request merged continues on a
//! fresh branch from that pull request's base — same workspace, same sessions —
//! or nothing about it changes.

use sea_orm::ConnectionTrait;

use super::*;
use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};
use crate::git_harness::GitHost;
use crate::models::agent::AgentType;

const REPO_JSON: &str = r#"{"squashMergeAllowed":true,"mergeCommitAllowed":true,"rebaseMergeAllowed":false,"viewerDefaultMergeMethod":"MERGE"}"#;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A merged pull request's worktree: the root repo, a bare `origin`, a
/// worktree started from `origin/<base>` whose branch has one pushed commit,
/// and that branch merged into `<base>` on origin from another clone.
struct Fx {
    tmp: tempfile::TempDir,
    db: AppDatabase,
    connections: ConnectionManager,
    root_wid: String,
    wt: WorkspaceInfo,
    /// The worktree checkout.
    path: PathBuf,
    gh: PathBuf,
    base: String,
    /// The worktree's branch, the pull request's head.
    branch: String,
    feature_head: String,
    merge_commit: String,
}

impl Fx {
    fn hooks(&self) -> PathBuf {
        self.tmp.path().join("hooks")
    }

    /// The fake `gh` now reports pull request `number` for the branch, in
    /// `state`, with head `head`.
    fn answer(&self, number: u64, state: &str, head: &str) {
        self.answer_after(number, state, head, "");
    }

    /// As [`Fx::answer`], running shell `also` first whenever the pull
    /// request is looked up.
    fn answer_after(&self, number: u64, state: &str, head: &str, also: &str) {
        let pr = serde_json::json!({
            "number": number,
            "title": "T",
            "state": state,
            "url": format!("https://github.com/o/r/pull/{number}"),
            "isDraft": false,
            "baseRefName": self.base,
            "headRefName": self.branch,
            "headRefOid": head,
            "mergeStateStatus": "UNKNOWN",
            "statusCheckRollup": [],
            "mergedAt": (state == "MERGED").then_some("2026-10-03T00:00:00Z"),
            "autoMergeRequest": null,
        });
        executable(
            &self.gh,
            &format!(
                "#!/bin/sh\ncase \"$*\" in\n\"pr view {} \"*) {also}\necho '{pr}' ;;\n\"repo view\"*) echo '{REPO_JSON}' ;;\n*) echo 'no pull requests found' >&2; exit 1 ;;\nesac\n",
                self.branch
            ),
        );
    }

    async fn folder(&self) -> folder::Model {
        resolve(&self.db.conn, &self.wt.workspace_id).await.unwrap()
    }

    /// A session in the worktree: `(conversation id, session_id)`.
    async fn session(&self) -> (i32, String) {
        let conv = seed_conversation(&self.db, self.folder().await.id, AgentType::Codex).await;
        let row = conversation::Entity::find_by_id(conv)
            .one(&self.db.conn)
            .await
            .unwrap()
            .unwrap();
        (conv, row.session_id.unwrap())
    }

    /// A live agent for conversation `conv`, mid-turn when `running`.
    async fn attach(&self, connection_id: &str, conv: i32, running: bool) {
        self.connections
            .insert_test_connection(
                connection_id,
                AgentType::Codex,
                Some(self.path.clone()),
                EventEmitter::Noop,
            )
            .await;
        let state = self.connections.get_state(connection_id).await.unwrap();
        let mut state = state.write().await;
        state.conversation_id = Some(conv);
        state.turn_in_flight = running;
    }

    async fn run(&self, number: u64) -> Result<WorkspaceInfo, ZwiimError> {
        self.run_in(&self.wt.workspace_id, number).await
    }

    async fn run_in(&self, workspace_id: &str, number: u64) -> Result<WorkspaceInfo, ZwiimError> {
        self.run_with(workspace_id, number, None).await
    }

    async fn run_with(
        &self,
        workspace_id: &str,
        number: u64,
        prefix: Option<&str>,
    ) -> Result<WorkspaceInfo, ZwiimError> {
        let host = GitHost {
            db: &self.db.conn,
            data_dir: self.tmp.path(),
            gh: Some(&self.gh),
        };
        continue_after_merge(
            &self.db,
            &EventEmitter::Noop,
            &self.connections,
            &host,
            workspace_id,
            number,
            prefix,
        )
        .await
    }

    /// Remove the worktree, deleting its branch when that is safe.
    async fn remove(&self) -> WorktreeRemoved {
        let host = GitHost {
            db: &self.db.conn,
            data_dir: self.tmp.path(),
            gh: Some(&self.gh),
        };
        remove_worktree(
            &self.db,
            &EventEmitter::Noop,
            &self.connections,
            &TerminalManager::new(),
            &self.wt.workspace_id,
            false,
            Some(&host),
        )
        .await
        .expect("remove")
    }

    fn has_branch(&self) -> bool {
        let root = self.tmp.path().join("repo");
        !git(&root, &["branch", "--list", &self.branch]).is_empty()
    }

    async fn snapshot(&self) -> Snapshot {
        let row = self.folder().await;
        let mut branches: Vec<String> = git(
            &self.path,
            &["branch", "--list", "--format=%(refname:short)"],
        )
        .lines()
        .map(str::to_string)
        .collect();
        branches.sort();
        Snapshot {
            branch: git(&self.path, &["branch", "--show-current"]),
            head: git(&self.path, &["rev-parse", "HEAD"]),
            base_branch: row.base_branch,
            base_commit: row.base_commit,
            alias: row.alias,
            branches,
        }
    }
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    branch: String,
    head: String,
    base_branch: Option<String>,
    base_commit: Option<String>,
    alias: Option<String>,
    branches: Vec<String>,
}

async fn merged_pr(base: &str) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path().to_path_buf();
    let hooks = t.join("hooks");
    std::fs::create_dir(&hooks).unwrap();
    let repo = t.join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    // The Host's own git reads the user's global config: pin what matters.
    for (key, value) in [
        ("user.name", "Fixture"),
        ("user.email", "fixture@example.com"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", hooks.to_str().unwrap()),
    ] {
        git(&repo, &["config", key, value]);
    }
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
    let origin = t.join("origin.git");
    let origin_url = origin.to_str().unwrap();
    git(&t, &["init", "-q", "--bare", "-b", "main", origin_url]);
    git(&repo, &["remote", "add", "origin", origin_url]);
    git(&repo, &["push", "-q", "origin", "main"]);
    if base != "main" {
        git(&repo, &["checkout", "-q", "-b", base]);
        git(
            &repo,
            &["commit", "-q", "--allow-empty", "-m", "stacked base"],
        );
        git(&repo, &["push", "-q", "origin", base]);
        git(&repo, &["checkout", "-q", "main"]);
    }
    git(&repo, &["fetch", "-q", "origin"]);

    let db = fresh_in_memory_db().await;
    let root_id = seed_folder(&db, repo.to_str().unwrap()).await;
    let root_wid = folder::Entity::find_by_id(root_id)
        .one(&db.conn)
        .await
        .unwrap()
        .unwrap()
        .workspace_id
        .unwrap();
    let wt = create_worktree(
        &db,
        &EventEmitter::Noop,
        &t.join("worktrees"),
        &root_wid,
        Some(&format!("origin/{base}")),
        None,
    )
    .await
    .unwrap();
    let path = PathBuf::from(&wt.path);
    let branch = wt.branch.clone().unwrap();
    std::fs::write(path.join("feature.txt"), "feature\n").unwrap();
    git(&path, &["add", "feature.txt"]);
    git(&path, &["commit", "-q", "-m", "feature"]);
    git(&path, &["push", "-q", "origin", &branch]);
    let feature_head = git(&path, &["rev-parse", "HEAD"]);

    // The pull request merges on GitHub: origin's base gets a merge commit.
    let other = t.join("other");
    git(&t, &["clone", "-q", origin_url, other.to_str().unwrap()]);
    git(&other, &["checkout", "-q", base]);
    git(
        &other,
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "Merge #12",
            &format!("origin/{branch}"),
        ],
    );
    git(&other, &["push", "-q", "origin", base]);
    let merge_commit = git(&other, &["rev-parse", "HEAD"]);

    let fx = Fx {
        gh: t.join("gh"),
        tmp,
        db,
        connections: ConnectionManager::new(),
        root_wid,
        wt,
        path,
        base: base.to_string(),
        branch,
        feature_head,
        merge_commit,
    };
    fx.answer(12, "MERGED", &fx.feature_head.clone());
    fx
}

fn details(e: &ZwiimError) -> serde_json::Value {
    e.details.clone().unwrap_or_default()
}

/// Saving the new base fails from now on.
async fn fail_base_saves(fx: &Fx) {
    fx.db
        .conn
        .execute_unprepared(
            "CREATE TRIGGER fail_base BEFORE UPDATE OF base_commit ON folder \
             BEGIN SELECT RAISE(ABORT, 'disk full'); END;",
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn continues_on_a_fresh_branch_from_the_merged_prs_base() {
    let fx = merged_pr("main").await;
    let (_, session_id) = fx.session().await;

    let info = fx.run(12).await.expect("continue");

    // `pune` continues as placeholder `pune-v1`.
    let branch = git(&fx.path, &["branch", "--show-current"]);
    assert_eq!(branch, format!("{}-v1", fx.branch));
    let head = git(&fx.path, &["rev-parse", "HEAD"]);
    assert_eq!(head, fx.merge_commit);
    assert_eq!(head, git(&fx.path, &["rev-parse", "origin/main"]));

    let row = fx.folder().await;
    assert_eq!(row.base_branch.as_deref(), Some("origin/main"));
    assert_eq!(row.base_commit.as_deref(), Some(head.as_str()));
    assert_eq!(info.workspace_id, fx.wt.workspace_id);
    assert_eq!(info.branch.as_deref(), Some(branch.as_str()));
    assert_eq!(info.base_branch.as_deref(), Some("origin/main"));
    assert_eq!(info.base_commit.as_deref(), Some(head.as_str()));
    // The name was the branch's, so it follows the branch.
    assert_eq!(fx.wt.alias.as_deref(), Some(fx.branch.as_str()));
    assert_eq!(row.alias.as_deref(), Some(branch.as_str()));
    assert_eq!(info.alias.as_deref(), Some(branch.as_str()));

    let sessions =
        crate::session::list_for_workspace(&fx.db.conn, &fx.connections, &fx.wt.workspace_id)
            .await
            .unwrap();
    let ids: Vec<_> = sessions.into_iter().map(|s| s.session_id).collect();
    assert_eq!(ids, vec![session_id]);

    // The merged branch is kept as it was.
    assert_eq!(
        git(
            &fx.path,
            &["rev-parse", &format!("refs/heads/{}", fx.branch)]
        ),
        fx.feature_head
    );
    assert!(folders::git_status(row.path.clone(), None)
        .await
        .unwrap()
        .is_empty());
    assert!(crate::workspace_diff::diff(&row, None)
        .await
        .unwrap()
        .files
        .is_empty());
}

#[tokio::test]
async fn a_continued_name_skips_one_in_use() {
    let fx = merged_pr("main").await;
    git(&fx.path, &["branch", &format!("{}-v1", fx.branch)]);

    let info = fx.run(12).await.expect("continue");

    assert_eq!(info.branch, Some(format!("{}-v2", fx.branch)));
}

#[tokio::test]
async fn a_name_git_refuses_is_skipped_on_continue() {
    let fx = merged_pr("main").await;
    // `pune-v1/x` is free-looking to the name check, but blocks `pune-v1`.
    git(&fx.path, &["branch", &format!("{}-v1/x", fx.branch)]);

    let info = fx.run(12).await.expect("continue");

    assert_eq!(info.branch, Some(format!("{}-v2", fx.branch)));
}

#[tokio::test]
async fn the_folders_name_plays_no_part() {
    let mut fx = merged_pr("main").await;
    let hex = fx.path.with_file_name("3276b212");
    git(
        &fx.tmp.path().join("repo"),
        &[
            "worktree",
            "move",
            fx.path.to_str().unwrap(),
            hex.to_str().unwrap(),
        ],
    );
    let mut row = fx.folder().await.into_active_model();
    row.path = Set(hex.to_string_lossy().into_owned());
    row.update(&fx.db.conn).await.unwrap();
    fx.path = hex;

    let info = fx.run(12).await.expect("continue");

    let branch = format!("{}-v1", fx.branch);
    assert_eq!(info.branch.as_deref(), Some(branch.as_str()));
    assert_eq!(git(&fx.path, &["branch", "--show-current"]), branch);
}

#[tokio::test]
async fn a_name_the_user_chose_is_kept() {
    let fx = merged_pr("main").await;
    folders::update_folder_alias_core(&fx.db, fx.folder().await.id, Some("Payments".into()))
        .await
        .unwrap();

    let info = fx.run(12).await.expect("continue");

    assert_ne!(info.branch.as_deref(), Some(fx.branch.as_str()));
    assert_eq!(info.alias.as_deref(), Some("Payments"));
    assert_eq!(fx.folder().await.alias.as_deref(), Some("Payments"));
}

#[tokio::test]
async fn a_stacked_pr_continues_from_its_own_base() {
    let fx = merged_pr("branch-a").await;

    fx.run(12).await.expect("continue");

    let head = git(&fx.path, &["rev-parse", "HEAD"]);
    assert_eq!(head, git(&fx.path, &["rev-parse", "origin/branch-a"]));
    assert_ne!(head, git(&fx.path, &["rev-parse", "origin/main"]));
    let row = fx.folder().await;
    assert_eq!(row.base_branch.as_deref(), Some("origin/branch-a"));
    assert_eq!(row.base_commit.as_deref(), Some(head.as_str()));
}

#[tokio::test]
async fn another_sessions_running_turn_refuses() {
    let fx = merged_pr("main").await;
    let (x, _) = fx.session().await;
    let (y, y_session) = fx.session().await;
    fx.attach("conn-x", x, false).await;
    fx.attach("conn-y", y, true).await;
    let before = fx.snapshot().await;

    let err = fx.run(12).await.expect_err("busy");

    assert_eq!(err.code, "workspace_busy");
    assert_eq!(
        details(&err),
        serde_json::json!({ "live_sessions": 1, "terminals": 0, "session_ids": [y_session] })
    );
    assert_eq!(fx.snapshot().await, before);
}

#[tokio::test]
async fn the_callers_own_running_turn_refuses() {
    let fx = merged_pr("main").await;
    let (x, x_session) = fx.session().await;
    fx.attach("conn-x", x, true).await;
    let before = fx.snapshot().await;

    let err = fx.run(12).await.expect_err("busy");

    assert_eq!(err.code, "workspace_busy");
    assert_eq!(details(&err)["session_ids"], serde_json::json!([x_session]));
    assert_eq!(fx.snapshot().await, before);
}

#[tokio::test]
async fn idle_sessions_do_not_block() {
    let fx = merged_pr("main").await;
    let (idle, _) = fx.session().await;
    fx.attach("conn-idle", idle, false).await;
    let _not_live = fx.session().await;

    fx.run(12).await.expect("idle sessions do not block");
    assert_eq!(git(&fx.path, &["rev-parse", "HEAD"]), fx.merge_commit);
}

#[tokio::test]
async fn a_dirty_worktree_refuses() {
    let fx = merged_pr("main").await;
    std::fs::write(fx.path.join("scratch.txt"), "wip\n").unwrap();
    let before = fx.snapshot().await;

    let err = fx.run(12).await.expect_err("dirty");

    assert_eq!(err.code, "workspace_dirty");
    assert_eq!(details(&err), serde_json::json!({ "changes": 1 }));
    assert_eq!(fx.snapshot().await, before);
    assert!(fx.path.join("scratch.txt").exists());
}

#[tokio::test]
async fn a_file_saved_while_github_answers_refuses() {
    let fx = merged_pr("main").await;
    let before = fx.snapshot().await;
    // A terminal or editor is not held off by the gate.
    let scratch = fx.path.join("scratch.txt");
    fx.answer_after(
        12,
        "MERGED",
        &fx.feature_head,
        &format!("echo wip > '{}'", scratch.display()),
    );

    let err = fx.run(12).await.expect_err("dirty");

    assert_eq!(err.code, "workspace_dirty");
    assert_eq!(fx.snapshot().await, before);
    assert!(scratch.exists());
}

#[tokio::test]
async fn a_commit_made_while_fetching_refuses() {
    let fx = merged_pr("main").await;
    let before = fx.snapshot().await;
    // The fetch's ref update is the window: a commit lands on the branch then
    // (once: the commit's own ref update must not commit again).
    let marker = fx.tmp.path().join("hook-fired");
    executable(
        &fx.hooks().join("reference-transaction"),
        &format!(
            "#!/bin/sh\n[ \"$1\" = committed ] || exit 0\n[ -e '{m}' ] && exit 0\ntouch '{m}'\ngit -C '{p}' commit -q --allow-empty -m late\n",
            m = marker.display(),
            p = fx.path.display()
        ),
    );

    let err = fx.run(12).await.expect_err("unmerged commit");

    assert!(marker.exists(), "the hook ran");
    assert_eq!(err.code, "head_not_merged");
    let after = fx.snapshot().await;
    assert_eq!(after.branch, before.branch);
    assert_eq!(after.base_branch, before.base_branch);
    assert_eq!(after.base_commit, before.base_commit);
    assert_eq!(after.branches, before.branches);
    assert_eq!(git(&fx.path, &["log", "-1", "--format=%s"]), "late");
}

#[tokio::test]
async fn head_ahead_of_the_merged_pr_refuses() {
    let fx = merged_pr("main").await;
    git(
        &fx.path,
        &["commit", "-q", "--allow-empty", "-m", "after merge"],
    );
    let head = git(&fx.path, &["rev-parse", "HEAD"]);
    let before = fx.snapshot().await;

    let err = fx.run(12).await.expect_err("unmerged commits");

    assert_eq!(err.code, "head_not_merged");
    assert_eq!(
        details(&err),
        serde_json::json!({ "head": head, "pr_head": fx.feature_head })
    );
    assert_eq!(fx.snapshot().await, before);
}

#[tokio::test]
async fn an_unmerged_pr_refuses() {
    let fx = merged_pr("main").await;
    let before = fx.snapshot().await;

    for (state, name) in [("OPEN", "open"), ("CLOSED", "closed")] {
        fx.answer(12, state, &fx.feature_head);
        let err = fx.run(12).await.expect_err(state);
        assert_eq!(err.code, "pr_not_merged", "{state}");
        assert_eq!(
            details(&err),
            serde_json::json!({ "number": 12, "state": name })
        );
        assert_eq!(fx.snapshot().await, before);
    }
}

#[tokio::test]
async fn a_different_pr_number_refuses() {
    let fx = merged_pr("main").await;
    let before = fx.snapshot().await;

    let err = fx.run(13).await.expect_err("different pull request");

    assert_eq!(err.code, "pr_changed");
    assert_eq!(fx.snapshot().await, before);
}

#[tokio::test]
async fn a_failed_fetch_changes_nothing() {
    let fx = merged_pr("main").await;
    let missing = fx.tmp.path().join("missing.git");
    git(
        &fx.path,
        &["remote", "set-url", "origin", missing.to_str().unwrap()],
    );
    let before = fx.snapshot().await;

    let err = fx.run(12).await.expect_err("fetch fails");

    assert_eq!(err.code, "not_found");
    assert_eq!(fx.snapshot().await, before);
}

#[tokio::test]
async fn a_failed_branch_create_changes_nothing() {
    let fx = merged_pr("main").await;
    let git_dir = PathBuf::from(git(&fx.path, &["rev-parse", "--absolute-git-dir"]));
    let lock = git_dir.join("index.lock");
    std::fs::write(&lock, "").unwrap();
    let before = fx.snapshot().await;

    let result = fx.run(12).await;
    std::fs::remove_file(&lock).unwrap();

    let err = result.expect_err("switch fails");
    assert_eq!(err.code, "internal");
    // It failed at the switch, after fetching.
    assert_eq!(
        git(&fx.path, &["rev-parse", "refs/remotes/origin/main"]),
        fx.merge_commit
    );
    assert_eq!(fx.snapshot().await, before);
}

#[tokio::test]
async fn a_root_workspace_is_refused() {
    let fx = merged_pr("main").await;

    let err = fx.run_in(&fx.root_wid, 12).await.expect_err("root");

    assert_eq!(err.code, "invalid_input");
}

#[tokio::test]
async fn every_session_gets_the_notice_once() {
    let fx = merged_pr("main").await;
    let (a, _) = fx.session().await;
    let (b, _) = fx.session().await;
    let (c, _) = fx.session().await;
    fx.attach("conn-a", a, false).await;
    fx.attach("conn-b", b, false).await;
    let root_id = resolve(&fx.db.conn, &fx.root_wid).await.unwrap().id;
    let elsewhere = seed_conversation(&fx.db, root_id, AgentType::Codex).await;

    fx.run(12).await.expect("continue");

    for conv in [a, b, c] {
        let notices = fx.connections.pending_notices(conv);
        assert_eq!(notices.len(), 1, "conversation {conv}: {notices:?}");
        assert!(notices[0].contains("#12"), "{}", notices[0]);
        assert!(notices[0].contains("git_branch_rename"), "{}", notices[0]);
    }
    assert!(fx.connections.pending_notices(elsewhere).is_empty());
}

#[tokio::test]
async fn a_failed_baseline_save_switches_back() {
    let fx = merged_pr("main").await;
    let (conv, _) = fx.session().await;
    let before = fx.snapshot().await;
    fail_base_saves(&fx).await;

    let err = fx.run(12).await.expect_err("base save fails");

    assert_eq!(err.code, "internal");
    assert!(err.message.contains(&fx.branch), "{}", err.message);
    assert_eq!(fx.snapshot().await, before);
    assert!(fx.connections.pending_notices(conv).is_empty());
}

#[tokio::test]
async fn a_moved_new_branch_is_kept_when_switching_back() {
    let fx = merged_pr("main").await;
    let before = fx.snapshot().await;
    fail_base_saves(&fx).await;
    // A commit lands on the new branch right after Continue switches to it
    // (once: the switch back must not commit again).
    let marker = fx.tmp.path().join("hook-fired");
    executable(
        &fx.hooks().join("post-checkout"),
        &format!(
            "#!/bin/sh\n[ -e '{m}' ] || {{ touch '{m}'; git commit --allow-empty -qm window; }}\nexit 0\n",
            m = marker.display()
        ),
    );

    let err = fx.run(12).await.expect_err("base save fails");

    assert_eq!(err.code, "internal");
    assert!(marker.exists(), "the hook ran");
    let after = fx.snapshot().await;
    assert_eq!(after.branch, fx.branch);
    assert_eq!(after.head, before.head);
    assert_eq!(after.base_commit, before.base_commit);
    let kept: Vec<_> = after
        .branches
        .iter()
        .filter(|b| !before.branches.contains(b))
        .collect();
    assert_eq!(kept.len(), 1, "{:?}", after.branches);
    assert_eq!(
        git(&fx.path, &["log", "-1", "--format=%s", kept[0].as_str()]),
        "window"
    );
}

#[tokio::test]
async fn a_branch_prefix_goes_in_front_of_the_continued_branch_once() {
    let fx = merged_pr("main").await;

    fx.run_with(&fx.wt.workspace_id, 12, Some("ashish"))
        .await
        .expect("continue");
    assert_eq!(
        git(&fx.path, &["branch", "--show-current"]),
        format!("ashish/{}-v1", fx.branch)
    );
    // The folder keeps its city.
    assert_eq!(
        fx.path.file_name().unwrap().to_str(),
        Some(fx.branch.as_str())
    );
}

#[tokio::test]
async fn removing_deletes_a_fully_pushed_branch() {
    let fx = merged_pr("main").await;

    let removed = fx.remove().await;
    assert_eq!(removed.branch.as_deref(), Some(fx.branch.as_str()));
    assert!(removed.branch_deleted);
    assert_eq!(removed.branch_kept_reason, None);
    assert!(!fx.has_branch());
}

#[tokio::test]
async fn removing_deletes_a_merged_branch_whose_remote_copy_is_gone() {
    let fx = merged_pr("main").await;
    // GitHub deleted the head branch after merging; the root never fetched
    // the merge, so the commit is on no remote branch it knows.
    git(&fx.path, &["push", "-q", "origin", "--delete", &fx.branch]);

    let removed = fx.remove().await;
    assert!(removed.branch_deleted, "{:?}", removed.branch_kept_reason);
    assert!(!fx.has_branch());
}

#[tokio::test]
async fn removing_keeps_an_unpushed_branch_and_says_why() {
    let fx = merged_pr("main").await;
    std::fs::write(fx.path.join("more.txt"), "more\n").unwrap();
    git(&fx.path, &["add", "more.txt"]);
    git(&fx.path, &["commit", "-q", "-m", "more"]);

    let removed = fx.remove().await;
    assert!(!removed.branch_deleted);
    assert_eq!(
        removed.branch_kept_reason.as_deref(),
        Some(format!("{} has commits that aren't pushed", fx.branch).as_str())
    );
    assert!(fx.has_branch());
}

#[tokio::test]
async fn removing_keeps_a_branch_github_cannot_vouch_for() {
    let fx = merged_pr("main").await;
    git(&fx.path, &["push", "-q", "origin", "--delete", &fx.branch]);
    fx.answer(12, "OPEN", &fx.feature_head.clone());

    let removed = fx.remove().await;
    assert!(!removed.branch_deleted);
    assert!(removed
        .branch_kept_reason
        .unwrap()
        .contains("has commits that aren't pushed"));
    assert!(fx.has_branch());
}

#[tokio::test]
async fn the_base_branch_is_never_deleted() {
    let fx = merged_pr("main").await;
    let host = GitHost {
        db: &fx.db.conn,
        data_dir: fx.tmp.path(),
        gh: Some(&fx.gh),
    };
    let repo = fx.tmp.path().join("repo");

    let kept = delete_branch_if_safe(
        &host,
        repo.to_str().unwrap(),
        &fx.root_wid,
        "main",
        Some("origin/main"),
    )
    .await;
    assert_eq!(kept, Err("main is the base branch".to_string()));
    git(&repo, &["rev-parse", "--verify", "refs/heads/main"]);
}
