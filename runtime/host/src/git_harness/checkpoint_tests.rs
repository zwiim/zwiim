//! Per-turn checkpoints against real temporary repositories.

use std::path::Path;

use tempfile::TempDir;

use super::checkpoint::{self, CheckpointKey, Phase, Saved, SkipReason};

// ── fixtures ────────────────────────────────────────────────────────────

/// `git <args>` in `dir`, isolated from the user's Git config. Its stdout.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_raw(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git_raw(dir: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
        .output()
        .expect("run git")
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

/// A repository with one commit holding `a.txt` and a `.gitignore`.
fn repo() -> (TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    for (k, v) in [
        ("user.name", "Fixture"),
        ("user.email", "fixture@example.com"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", "no-hooks"),
    ] {
        git(&root, &["config", k, v]);
    }
    write(&root, "a.txt", "one\n");
    write(&root, ".gitignore", "ignored.txt\nnode_modules/\n");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "init"]);
    (tmp, root)
}

fn key(session_id: &str, ordinal: u32) -> CheckpointKey {
    CheckpointKey {
        session_id: session_id.into(),
        ordinal,
    }
}

async fn saved_oid(root: &Path, key: &CheckpointKey, phase: Phase) -> String {
    match checkpoint::save(root, key, phase, None).await {
        Saved::Ref { oid } => oid,
        other => panic!("expected a checkpoint, got {other:?}"),
    }
}

fn tree_files(root: &Path, rev: &str) -> Vec<String> {
    git(root, &["ls-tree", "-r", "--name-only", rev])
        .lines()
        .map(str::to_string)
        .collect()
}

fn zwiim_refs(root: &Path) -> Vec<String> {
    git(
        root,
        &["for-each-ref", "--format=%(refname)", "refs/zwiim/"],
    )
    .lines()
    .map(str::to_string)
    .collect()
}

/// Every file under `root` except `.git`, with its bytes, sorted.
fn worktree_bytes(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let rel = path.strip_prefix(base).unwrap().display().to_string();
                out.push((rel, std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

// ── save ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn save_captures_tracked_untracked_not_ignored() {
    let (_tmp, root) = repo();
    write(&root, "a.txt", "two\n");
    write(&root, "new.txt", "fresh\n");
    write(&root, "dir/nested.txt", "deep\n");
    write(&root, "ignored.txt", "secret\n");
    write(&root, "node_modules/x.js", "dep\n");

    let k = key("s-1", 1);
    let oid = saved_oid(&root, &k, Phase::Start).await;
    assert_eq!(
        zwiim_refs(&root),
        ["refs/zwiim/checkpoints/s-1/1/start"],
        "one ref, in the checkpoint namespace"
    );
    assert_eq!(
        git(&root, &["rev-parse", "refs/zwiim/checkpoints/s-1/1/start"]),
        oid
    );

    let files = tree_files(&root, &oid);
    assert_eq!(files, [".gitignore", "a.txt", "dir/nested.txt", "new.txt"]);
    assert_eq!(git(&root, &["show", &format!("{oid}:a.txt")]), "two");
    // Parentless: the checkpoint never joins any history.
    assert_eq!(git(&root, &["rev-list", "--count", &oid]), "1");

    let listed = checkpoint::list(&root, "s-1").await.unwrap();
    assert_eq!(listed.len(), 1);
    let cp = &listed[0];
    assert_eq!(cp.key, k);
    assert_eq!(cp.phase, Phase::Start);
    assert_eq!(
        cp.head.as_deref(),
        Some(git(&root, &["rev-parse", "HEAD"]).as_str())
    );
    assert_eq!(
        cp.worktree_tree,
        git(&root, &["rev-parse", &format!("{oid}^{{tree}}")])
    );
    assert_eq!(cp.index_tree, git(&root, &["rev-parse", "HEAD^{tree}"]));
    assert_eq!(cp.prompt_fingerprint, None);
}

#[tokio::test]
async fn save_tagged_records_the_action() {
    let (_tmp, root) = repo();
    let k = key("s-act", 1);
    assert!(matches!(
        checkpoint::save_tagged(&root, &k, Phase::Start, None, Some("resolve_conflicts")).await,
        Saved::Ref { .. }
    ));
    saved_oid(&root, &k, Phase::End).await;
    let listed = checkpoint::list(&root, "s-act").await.unwrap();
    assert_eq!(listed[0].action.as_deref(), Some("resolve_conflicts"));
    assert_eq!(listed[1].action, None);
}

#[tokio::test]
async fn save_records_the_prompt_fingerprint() {
    let (_tmp, root) = repo();
    let fp = crate::acp::fork::fingerprint_agent_message("fix the bug");
    let k = key("s-fp", 1);
    assert!(matches!(
        checkpoint::save(&root, &k, Phase::Start, Some(&fp)).await,
        Saved::Ref { .. }
    ));
    saved_oid(&root, &k, Phase::End).await;
    let listed = checkpoint::list(&root, "s-fp").await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].phase, Phase::Start);
    assert_eq!(listed[0].prompt_fingerprint.as_deref(), Some(fp.as_str()));
    assert_eq!(listed[1].phase, Phase::End);
    assert_eq!(listed[1].prompt_fingerprint, None);
}

#[tokio::test]
async fn save_leaves_head_index_worktree_identical() {
    let (_tmp, root) = repo();
    write(&root, "staged.txt", "staged\n");
    git(&root, &["add", "staged.txt"]);
    // A staged change, then a different unstaged edit of the same file.
    write(&root, "a.txt", "staged edit\n");
    git(&root, &["add", "a.txt"]);
    write(&root, "a.txt", "worktree edit\n");
    write(&root, "untracked.txt", "u\n");
    write(&root, "ignored.txt", "secret\n");
    // `status` may refresh the index; settle it before taking the bytes.
    let status_before = git(
        &root,
        &["status", "--porcelain=v2", "--untracked-files=all"],
    );
    let head_before = git(&root, &["rev-parse", "HEAD"]);
    let index_path = root.join(".git/index");
    let index_before = std::fs::read(&index_path).unwrap();
    let files_before = worktree_bytes(&root);

    let k = key("s-2", 1);
    let oid = saved_oid(&root, &k, Phase::Start).await;

    assert_eq!(
        std::fs::read(&index_path).unwrap(),
        index_before,
        "index bytes"
    );
    assert_eq!(worktree_bytes(&root), files_before, "worktree bytes");
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), head_before, "HEAD");
    assert_eq!(
        git(
            &root,
            &["status", "--porcelain=v2", "--untracked-files=all"]
        ),
        status_before
    );
    assert!(!root.join(".git/index.lock").exists());

    let cp = checkpoint::list(&root, "s-2").await.unwrap().remove(0);
    // The index tree holds what was staged; the worktree tree what was on disk.
    assert_eq!(
        git(&root, &["show", &format!("{}:a.txt", cp.index_tree)]),
        "staged edit"
    );
    assert_eq!(
        git(&root, &["show", &format!("{}:staged.txt", cp.index_tree)]),
        "staged"
    );
    assert!(!tree_files(&root, &cp.index_tree).contains(&"untracked.txt".to_string()));
    assert_eq!(
        git(&root, &["show", &format!("{oid}:a.txt")]),
        "worktree edit"
    );
    assert!(tree_files(&root, &oid).contains(&"untracked.txt".to_string()));
    assert!(!tree_files(&root, &oid).contains(&"ignored.txt".to_string()));
}

#[tokio::test]
async fn save_works_before_the_first_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    git(&root, &["init", "-q", "-b", "main"]);
    write(&root, "first.txt", "1\n");
    let oid = saved_oid(&root, &key("s-unborn", 1), Phase::Start).await;
    assert_eq!(tree_files(&root, &oid), ["first.txt"]);
    let cp = checkpoint::list(&root, "s-unborn").await.unwrap().remove(0);
    assert_eq!(cp.head, None);
    assert!(!root.join(".git/index").exists(), "no index was created");
}

#[tokio::test]
async fn busy_repo_skips_with_reason() {
    // A conflicted merge.
    let (_tmp, root) = repo();
    git(&root, &["checkout", "-q", "-b", "side"]);
    write(&root, "a.txt", "side\n");
    git(&root, &["commit", "-qam", "side"]);
    git(&root, &["checkout", "-q", "main"]);
    write(&root, "a.txt", "main\n");
    git(&root, &["commit", "-qam", "main"]);
    assert!(!git_raw(&root, &["merge", "side"]).status.success());
    let k = key("s-busy", 1);
    assert_eq!(
        checkpoint::save(&root, &k, Phase::Start, None).await,
        Saved::Skipped(SkipReason::Busy("merge"))
    );
    // Unmerged entries alone still count.
    std::fs::remove_file(root.join(".git/MERGE_HEAD")).unwrap();
    assert_eq!(
        checkpoint::save(&root, &k, Phase::Start, None).await,
        Saved::Skipped(SkipReason::Busy("conflict"))
    );
    assert!(zwiim_refs(&root).is_empty());

    // A rebase, a cherry-pick, a revert and a held index lock.
    for (marker, dir, reason) in [
        ("rebase-merge", true, "rebase"),
        ("rebase-apply", true, "rebase"),
        ("CHERRY_PICK_HEAD", false, "cherry_pick"),
        ("REVERT_HEAD", false, "revert"),
        ("index.lock", false, "index_lock"),
    ] {
        let (_tmp, root) = repo();
        let path = root.join(".git").join(marker);
        if dir {
            std::fs::create_dir_all(&path).unwrap();
        } else {
            std::fs::write(&path, "").unwrap();
        }
        assert_eq!(
            checkpoint::save(&root, &k, Phase::Start, None).await,
            Saved::Skipped(SkipReason::Busy(reason)),
            "{marker}"
        );
        assert!(zwiim_refs(&root).is_empty(), "{marker}");
        assert!(path.exists(), "{marker} left alone");
    }
}

#[tokio::test]
async fn non_git_skips_without_git_call() {
    // A plain directory inside a repository: any `git` run there would find
    // the enclosing repository and write a ref into it.
    let (_tmp, root) = repo();
    let plain = root.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    write(&plain, "f.txt", "x\n");

    let k = key("s-plain", 1);
    for phase in [Phase::Start, Phase::End] {
        assert_eq!(
            checkpoint::save(&plain, &k, phase, None).await,
            Saved::Skipped(SkipReason::NotGit)
        );
    }
    assert_eq!(
        checkpoint::next_ordinal(&plain, "s-plain").await,
        Err(SkipReason::NotGit)
    );
    assert_eq!(
        checkpoint::list(&plain, "s-plain").await,
        Err(SkipReason::NotGit)
    );
    checkpoint::cap(&plain, "s-plain", 0).await;
    checkpoint::gc_session(&plain, "s-plain").await;
    assert!(
        zwiim_refs(&root).is_empty(),
        "nothing reached the enclosing repo"
    );
}

#[tokio::test]
async fn linked_worktrees_do_not_bleed() {
    let (tmp, root) = repo();
    let wt = tmp.path().join("wt");
    git(
        &root,
        &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()],
    );
    write(&root, "only-main.txt", "m\n");
    write(&wt, "only-wt.txt", "w\n");
    write(&wt, "a.txt", "wt edit\n");

    let main_oid = saved_oid(&root, &key("s-main", 1), Phase::Start).await;
    let wt_oid = saved_oid(&wt, &key("s-wt", 1), Phase::Start).await;

    let main_files = tree_files(&root, &main_oid);
    let wt_files = tree_files(&root, &wt_oid);
    assert!(main_files.contains(&"only-main.txt".to_string()));
    assert!(!main_files.contains(&"only-wt.txt".to_string()));
    assert!(wt_files.contains(&"only-wt.txt".to_string()));
    assert!(!wt_files.contains(&"only-main.txt".to_string()));
    assert_eq!(git(&root, &["show", &format!("{main_oid}:a.txt")]), "one");
    assert_eq!(git(&root, &["show", &format!("{wt_oid}:a.txt")]), "wt edit");

    // Refs are shared repo-wide; the session id keeps them apart.
    assert_eq!(
        zwiim_refs(&root),
        [
            "refs/zwiim/checkpoints/s-main/1/start",
            "refs/zwiim/checkpoints/s-wt/1/start"
        ]
    );
    assert_eq!(checkpoint::list(&wt, "s-wt").await.unwrap().len(), 1);
    assert_eq!(checkpoint::list(&wt, "s-main").await.unwrap().len(), 1);
    // The worktree's index was its own, and is untouched.
    assert_eq!(git(&wt, &["status", "--porcelain"]).lines().count(), 2);
}

#[tokio::test]
async fn two_sessions_never_collide() {
    let (_tmp, root) = repo();
    for sid in ["s-a", "s-b"] {
        assert_eq!(checkpoint::next_ordinal(&root, sid).await, Ok(1));
    }
    write(&root, "a.txt", "for a\n");
    let a1 = saved_oid(&root, &key("s-a", 1), Phase::Start).await;
    write(&root, "a.txt", "for b\n");
    let b1 = saved_oid(&root, &key("s-b", 1), Phase::Start).await;
    saved_oid(&root, &key("s-a", 1), Phase::End).await;
    saved_oid(&root, &key("s-a", 2), Phase::Start).await;

    assert_eq!(checkpoint::next_ordinal(&root, "s-a").await, Ok(3));
    assert_eq!(checkpoint::next_ordinal(&root, "s-b").await, Ok(2));
    // A session id that is a prefix of another stays separate.
    assert_eq!(checkpoint::next_ordinal(&root, "s").await, Ok(1));

    let a = checkpoint::list(&root, "s-a").await.unwrap();
    let shape: Vec<_> = a.iter().map(|c| (c.key.ordinal, c.phase)).collect();
    assert_eq!(
        shape,
        [(1, Phase::Start), (1, Phase::End), (2, Phase::Start)]
    );
    assert!(a.iter().all(|c| c.key.session_id == "s-a"));
    let b = checkpoint::list(&root, "s-b").await.unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(git(&root, &["show", &format!("{a1}:a.txt")]), "for a");
    assert_eq!(git(&root, &["show", &format!("{b1}:a.txt")]), "for b");
}

#[tokio::test]
async fn rejects_session_ids_that_are_not_ref_safe() {
    let (_tmp, root) = repo();
    for bad in ["", "a/b", "..", "a b", "x~1", "a:b", "*"] {
        assert!(
            matches!(
                checkpoint::save(&root, &key(bad, 1), Phase::Start, None).await,
                Saved::Skipped(SkipReason::Failed(_))
            ),
            "{bad:?}"
        );
    }
    assert!(zwiim_refs(&root).is_empty());
}

#[tokio::test]
async fn save_never_replaces_a_stored_checkpoint() {
    let (_tmp, root) = repo();
    let k = key("s-once", 1);
    let first = saved_oid(&root, &k, Phase::Start).await;
    write(&root, "a.txt", "changed\n");
    assert!(
        matches!(
            checkpoint::save(&root, &k, Phase::Start, None).await,
            Saved::Skipped(SkipReason::Failed(_))
        ),
        "a second save of the same turn and phase is refused"
    );
    assert_eq!(
        git(
            &root,
            &["rev-parse", "refs/zwiim/checkpoints/s-once/1/start"]
        ),
        first
    );
}

#[tokio::test]
async fn unreadable_refs_are_an_error_not_turn_one() {
    let (_tmp, root) = repo();
    saved_oid(&root, &key("s-r", 1), Phase::Start).await;
    // Git cannot read the refs at all: guessing "no checkpoints yet" would
    // hand out turn 1 again.
    let packed = root.join(".git/packed-refs");
    std::fs::create_dir_all(&packed).unwrap();
    assert!(matches!(
        checkpoint::next_ordinal(&root, "s-r").await,
        Err(SkipReason::Failed(_))
    ));
    assert!(matches!(
        checkpoint::list(&root, "s-r").await,
        Err(SkipReason::Failed(_))
    ));
    assert!(matches!(
        checkpoint::save(&root, &key("s-r", 1), Phase::End, None).await,
        Saved::Skipped(SkipReason::Failed(_))
    ));
    std::fs::remove_dir(&packed).unwrap();
    assert_eq!(checkpoint::next_ordinal(&root, "s-r").await, Ok(2));
    assert_eq!(checkpoint::list(&root, "s-r").await.unwrap().len(), 1);
}

// ── gc ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn gc_session_removes_only_that_session() {
    let (_tmp, root) = repo();
    let oid = saved_oid(&root, &key("s-gone", 1), Phase::Start).await;
    saved_oid(&root, &key("s-gone", 1), Phase::End).await;
    saved_oid(&root, &key("s-gone", 2), Phase::Start).await;
    saved_oid(&root, &key("s-kept", 1), Phase::Start).await;
    saved_oid(&root, &key("s-gone-too-long", 1), Phase::Start).await;
    git(&root, &["update-ref", "refs/zwiim/other/s-gone", &oid]);
    git(&root, &["update-ref", "refs/heads/keep", "HEAD"]);
    git(&root, &["tag", "v1"]);

    checkpoint::gc_session(&root, "s-gone").await;

    assert!(checkpoint::list(&root, "s-gone").await.unwrap().is_empty());
    assert_eq!(checkpoint::list(&root, "s-kept").await.unwrap().len(), 1);
    assert_eq!(
        checkpoint::list(&root, "s-gone-too-long")
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        zwiim_refs(&root),
        [
            "refs/zwiim/checkpoints/s-gone-too-long/1/start",
            "refs/zwiim/checkpoints/s-kept/1/start",
            "refs/zwiim/other/s-gone"
        ]
    );
    assert!(
        git_raw(&root, &["rev-parse", "--verify", "refs/heads/keep"])
            .status
            .success()
    );
    assert!(git_raw(&root, &["rev-parse", "--verify", "refs/tags/v1"])
        .status
        .success());
}

#[tokio::test]
async fn cap_keeps_last_50() {
    let (_tmp, root) = repo();
    let start = saved_oid(&root, &key("s-cap", 1), Phase::Start).await;
    let end = saved_oid(&root, &key("s-cap", 1), Phase::End).await;
    for n in 2..=55 {
        git(
            &root,
            &[
                "update-ref",
                &format!("refs/zwiim/checkpoints/s-cap/{n}/start"),
                &start,
            ],
        );
        if n % 2 == 0 {
            git(
                &root,
                &[
                    "update-ref",
                    &format!("refs/zwiim/checkpoints/s-cap/{n}/end"),
                    &end,
                ],
            );
        }
    }
    saved_oid(&root, &key("s-other", 1), Phase::Start).await;

    checkpoint::cap(&root, "s-cap", 50).await;

    let mut turns: Vec<u32> = checkpoint::list(&root, "s-cap")
        .await
        .unwrap()
        .iter()
        .map(|c| c.key.ordinal)
        .collect();
    turns.dedup();
    assert_eq!(turns, (6..=55).collect::<Vec<_>>());
    let refs = zwiim_refs(&root);
    assert!(!refs.iter().any(|r| r.contains("/s-cap/5/")));
    assert!(refs.contains(&"refs/zwiim/checkpoints/s-cap/6/end".to_string()));
    assert!(refs.contains(&"refs/zwiim/checkpoints/s-cap/55/start".to_string()));
    assert_eq!(checkpoint::list(&root, "s-other").await.unwrap().len(), 1);
    assert_eq!(checkpoint::next_ordinal(&root, "s-cap").await, Ok(56));

    // Under the cap: nothing to do.
    checkpoint::cap(&root, "s-cap", 50).await;
    assert_eq!(zwiim_refs(&root), refs);
}

#[tokio::test]
async fn drop_from_forgets_later_turns_only() {
    let (_tmp, root) = repo();
    let start = saved_oid(&root, &key("s-drop", 1), Phase::Start).await;
    saved_oid(&root, &key("s-drop", 1), Phase::End).await;
    for n in 2..=4 {
        for phase in ["start", "end"] {
            git(
                &root,
                &[
                    "update-ref",
                    &format!("refs/zwiim/checkpoints/s-drop/{n}/{phase}"),
                    &start,
                ],
            );
        }
    }
    let restore_point = "refs/zwiim/checkpoints/s-drop/revert-1700000000000";
    git(&root, &["update-ref", restore_point, &start]);
    saved_oid(&root, &key("s-other", 3), Phase::Start).await;

    checkpoint::drop_from(&root, "s-drop", 3).await;

    let mut turns: Vec<u32> = checkpoint::list(&root, "s-drop")
        .await
        .unwrap()
        .iter()
        .map(|c| c.key.ordinal)
        .collect();
    turns.dedup();
    assert_eq!(turns, [1, 2]);
    assert!(zwiim_refs(&root).contains(&restore_point.to_string()));
    assert_eq!(checkpoint::list(&root, "s-other").await.unwrap().len(), 1);

    // From the first turn: every turn goes, the restore point stays.
    checkpoint::drop_from(&root, "s-drop", 1).await;
    assert!(checkpoint::list(&root, "s-drop").await.unwrap().is_empty());
    assert!(zwiim_refs(&root).contains(&restore_point.to_string()));

    // Not a repository, or nothing to drop: a no-op.
    let plain = _tmp.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    checkpoint::drop_from(&plain, "s-drop", 1).await;
    checkpoint::drop_from(&root, "s-none", 1).await;
}

#[tokio::test]
async fn changes_summarize_the_turn() {
    let (_tmp, root) = repo();
    let turn = key("s-changes", 1);
    saved_oid(&root, &turn, Phase::Start).await;
    assert_eq!(checkpoint::changes(&root, &turn).await, None, "no end yet");

    write(&root, "a.txt", "uno\n");
    write(&root, "b.txt", "two\nthree\n");
    write(&root, "ignored.txt", "never\n");
    std::fs::remove_file(root.join(".gitignore")).unwrap();
    write(&root, ".gitignore", "ignored.txt\n");
    saved_oid(&root, &turn, Phase::End).await;

    let changes = checkpoint::changes(&root, &turn).await.expect("changes");
    assert_eq!(
        changes.files,
        [
            ("M".to_string(), ".gitignore".to_string()),
            ("M".to_string(), "a.txt".to_string()),
            ("A".to_string(), "b.txt".to_string()),
        ]
    );
    assert_eq!(changes.file_count, 3);
    // a.txt: one line replaced; b.txt: two added; .gitignore: one removed.
    assert_eq!((changes.added, changes.deleted), (3, 2));
    assert!(!changes.truncated);
}

#[tokio::test]
async fn changes_cap_the_file_list() {
    let (_tmp, root) = repo();
    let turn = key("s-many", 1);
    saved_oid(&root, &turn, Phase::Start).await;
    for i in 0..201 {
        write(&root, &format!("many/f{i:03}.txt"), "x\n");
    }
    saved_oid(&root, &turn, Phase::End).await;

    let changes = checkpoint::changes(&root, &turn).await.expect("changes");
    assert_eq!(changes.file_count, 201);
    assert_eq!(changes.files.len(), 200);
    assert_eq!(changes.added, 201);
    assert!(changes.truncated);
}

// ── restore ─────────────────────────────────────────────────────────────

/// The start checkpoint of `key`, read back.
async fn start_of(root: &Path, key: &CheckpointKey) -> checkpoint::Checkpoint {
    checkpoint::list(root, &key.session_id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.key == *key && c.phase == Phase::Start)
        .expect("start checkpoint")
}

/// The staged state, as `git ls-files --stage` prints it.
fn staged(root: &Path) -> String {
    git(root, &["ls-files", "--stage"])
}

#[tokio::test]
async fn revert_restores_files_to_turn_start() {
    let (_tmp, root) = repo();
    // Before the turn: an unstaged edit, a staged new file, an untracked one.
    write(&root, "a.txt", "two\n");
    write(&root, "b.txt", "b\n");
    git(&root, &["add", "b.txt"]);
    write(&root, "c.txt", "c\n");
    let turn = key("s-restore", 1);
    saved_oid(&root, &turn, Phase::Start).await;
    let files_before = worktree_bytes(&root);
    let staged_before = staged(&root);
    let head_before = git(&root, &["rev-parse", "HEAD"]);

    // The turn edits, deletes, stages and adds.
    write(&root, "a.txt", "three\n");
    std::fs::remove_file(root.join("b.txt")).unwrap();
    git(&root, &["add", "-A", "b.txt"]);
    write(&root, "c.txt", "changed\n");
    write(&root, "d.txt", "d\n");
    git(&root, &["add", "d.txt"]);

    let cp = start_of(&root, &turn).await;
    let restored = checkpoint::restore(&root, &cp).await.expect("restore");
    assert!(!restored.head_moved);
    assert_eq!(worktree_bytes(&root), files_before);
    assert_eq!(staged(&root), staged_before);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), head_before);
    // `c.txt` is untracked again, `b.txt` staged again.
    assert_eq!(
        git(&root, &["status", "--porcelain"]),
        // (The fixture trims the first line's leading space.)
        "M a.txt\nA  b.txt\n?? c.txt"
    );
}

#[tokio::test]
async fn revert_keeps_ignored_files() {
    let (_tmp, root) = repo();
    write(&root, "ignored.txt", "before\n");
    let turn = key("s-ignored", 1);
    saved_oid(&root, &turn, Phase::Start).await;

    write(&root, "ignored.txt", "after\n");
    write(&root, "node_modules/pkg/index.js", "x\n");
    write(&root, "a.txt", "turn\n");

    let cp = start_of(&root, &turn).await;
    checkpoint::restore(&root, &cp).await.expect("restore");
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "one\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("ignored.txt")).unwrap(),
        "after\n"
    );
    assert!(root.join("node_modules/pkg/index.js").exists());
}

#[tokio::test]
async fn revert_removes_new_untracked_and_is_recoverable() {
    let (_tmp, root) = repo();
    let turn = key("s-recover", 1);
    saved_oid(&root, &turn, Phase::Start).await;

    write(&root, "new.txt", "new\n");
    write(&root, "new_dir/inner.txt", "inner\n");
    write(&root, "a.txt", "turn\n");

    let cp = start_of(&root, &turn).await;
    checkpoint::restore(&root, &cp).await.expect("restore");
    assert!(!root.join("new.txt").exists());
    assert!(!root.join("new_dir").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "one\n"
    );

    // The state before the restore is kept under the session, apart from
    // its turns.
    let recovery: Vec<String> = zwiim_refs(&root)
        .into_iter()
        .filter(|r| {
            r.strip_prefix("refs/zwiim/checkpoints/s-recover/revert-")
                .is_some_and(|ms| !ms.is_empty() && ms.bytes().all(|b| b.is_ascii_digit()))
        })
        .collect();
    assert_eq!(recovery.len(), 1, "{:?}", zwiim_refs(&root));
    let files = tree_files(&root, &recovery[0]);
    assert!(files.contains(&"new.txt".to_string()), "{files:?}");
    assert!(
        files.contains(&"new_dir/inner.txt".to_string()),
        "{files:?}"
    );
    assert_eq!(
        git(&root, &["show", &format!("{}:a.txt", recovery[0])]),
        "turn"
    );
    // Not a turn: the turns and the next turn number are unchanged.
    assert_eq!(checkpoint::list(&root, "s-recover").await.unwrap().len(), 1);
    assert_eq!(checkpoint::next_ordinal(&root, "s-recover").await, Ok(2));
    // Deleting the session deletes it too.
    checkpoint::gc_session(&root, "s-recover").await;
    assert!(zwiim_refs(&root).is_empty());
}

#[tokio::test]
async fn revert_with_moved_head_keeps_commits() {
    let (_tmp, root) = repo();
    let turn = key("s-head", 1);
    saved_oid(&root, &turn, Phase::Start).await;
    let files_before = worktree_bytes(&root);

    write(&root, "a.txt", "committed by the turn\n");
    git(&root, &["commit", "-qam", "turn commit"]);
    let moved = git(&root, &["rev-parse", "HEAD"]);

    let cp = start_of(&root, &turn).await;
    let restored = checkpoint::restore(&root, &cp).await.expect("restore");
    assert!(restored.head_moved);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), moved, "HEAD untouched");
    assert_eq!(git(&root, &["log", "-1", "--format=%s"]), "turn commit");
    assert_eq!(worktree_bytes(&root), files_before);
    // The commit's change now shows as undone, staged, against HEAD.
    assert_eq!(git(&root, &["status", "--porcelain"]), "M  a.txt");
}

#[tokio::test]
async fn restore_refuses_a_busy_repo() {
    let (_tmp, root) = repo();
    let turn = key("s-busy-restore", 1);
    saved_oid(&root, &turn, Phase::Start).await;
    let cp = start_of(&root, &turn).await;
    write(&root, "a.txt", "turn\n");
    std::fs::write(root.join(".git/index.lock"), "").unwrap();

    let err = checkpoint::restore(&root, &cp).await.unwrap_err();
    assert_eq!(err.code, super::GitErrorCode::Conflict);
    assert_eq!(err.detail.as_deref(), Some("index_lock"));
    // Nothing changed and nothing was saved.
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "turn\n"
    );
    assert_eq!(zwiim_refs(&root).len(), 1);
}

// ── unwind ──────────────────────────────────────────────────────────────

/// A turn that commits once, its start checkpoint and the HEAD it ended at.
async fn committing_turn(root: &Path, session_id: &str) -> (checkpoint::Checkpoint, String) {
    let turn = key(session_id, 1);
    saved_oid(root, &turn, Phase::Start).await;
    write(root, "a.txt", "committed by the turn\n");
    git(root, &["commit", "-qam", "turn commit"]);
    (
        start_of(root, &turn).await,
        git(root, &["rev-parse", "HEAD"]),
    )
}

#[tokio::test]
async fn unwind_moves_head_back_when_unpushed_past_the_upstream() {
    let (tmp, root) = repo();
    let remote = tmp.path().join("remote.git");
    git(
        tmp.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git(
        &root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&root, &["push", "-q", "-u", "origin", "main"]);
    let start = git(&root, &["rev-parse", "HEAD"]);
    let (cp, end) = committing_turn(&root, "s-unwind-up").await;

    let moved = checkpoint::unwind_unpushed(&root, &cp, Some(&end)).await;
    assert_eq!(moved, Ok(true));
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), start);
    // Soft: the turn's change stays, staged.
    assert_eq!(git(&root, &["status", "--porcelain"]), "M  a.txt");
}

#[tokio::test]
async fn unwind_keeps_head_once_the_turn_is_pushed() {
    let (tmp, root) = repo();
    let remote = tmp.path().join("remote.git");
    git(
        tmp.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git(
        &root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&root, &["push", "-q", "-u", "origin", "main"]);
    let (cp, end) = committing_turn(&root, "s-unwind-pushed").await;
    git(&root, &["push", "-q"]);

    let moved = checkpoint::unwind_unpushed(&root, &cp, Some(&end)).await;
    assert_eq!(moved, Ok(false));
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), end);
}

#[tokio::test]
async fn unwind_keeps_head_when_the_start_is_not_an_ancestor() {
    let (_tmp, root) = repo();
    let (cp, _) = committing_turn(&root, "s-unwind-rewrite").await;
    // History rewritten under the turn: the start commit is gone from it.
    git(&root, &["checkout", "-q", "--orphan", "other"]);
    git(&root, &["commit", "-qm", "unrelated"]);
    let now = git(&root, &["rev-parse", "HEAD"]);

    let moved = checkpoint::unwind_unpushed(&root, &cp, Some(&now)).await;
    assert_eq!(moved, Ok(false));
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), now);
}

#[tokio::test]
async fn unwind_moves_a_detached_head_back() {
    let (_tmp, root) = repo();
    git(&root, &["checkout", "-q", "--detach"]);
    let start = git(&root, &["rev-parse", "HEAD"]);
    let (cp, end) = committing_turn(&root, "s-unwind-detached").await;

    // No branch, so no upstream: nothing was pushed.
    let moved = checkpoint::unwind_unpushed(&root, &cp, Some(&end)).await;
    assert_eq!(moved, Ok(true));
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), start);
    assert!(
        git_raw(&root, &["symbolic-ref", "-q", "HEAD"])
            .status
            .code()
            == Some(1)
    );
}

#[tokio::test]
async fn abort_operation_aborts_a_rebase_in_progress() {
    let (_tmp, root) = repo();
    git(&root, &["checkout", "-q", "-b", "side"]);
    write(&root, "a.txt", "side\n");
    git(&root, &["commit", "-qam", "side"]);
    git(&root, &["checkout", "-q", "main"]);
    write(&root, "a.txt", "main\n");
    git(&root, &["commit", "-qam", "main"]);
    git(&root, &["checkout", "-q", "side"]);
    let side = git(&root, &["rev-parse", "HEAD"]);
    let turn = key("s-abort-rebase", 1);
    saved_oid(&root, &turn, Phase::Start).await;
    assert!(!git_raw(&root, &["rebase", "-q", "main"]).status.success());
    assert!(root.join(".git/rebase-merge").exists());

    let cp = start_of(&root, &turn).await;
    checkpoint::abort_operation(&root, &cp)
        .await
        .expect("abort");
    assert!(!root.join(".git/rebase-merge").exists());
    assert_eq!(git(&root, &["symbolic-ref", "--short", "HEAD"]), "side");
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), side);
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
}
