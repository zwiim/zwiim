use std::path::{Path, PathBuf};
use std::time::Duration;

use sea_orm::EntityTrait;
use serde_json::json;
use tempfile::TempDir;

use super::error::{classify_gh, classify_git};
use super::fake_gh;
use super::run;
use super::*;
use crate::db::service::folder_service;
use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
use crate::db::AppDatabase;

// ── fixtures ────────────────────────────────────────────────────────────

/// `git <args>` in `dir`, isolated from the user's Git config.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository whose own config pins identity, signing, and hooks, so the
/// Harness (which runs with the user's environment) behaves the same on any
/// machine.
fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    for (k, v) in [
        ("user.name", "Fixture"),
        ("user.email", "fixture@example.com"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", "no-hooks"),
        ("pull.rebase", "true"),
    ] {
        git(dir, &["config", k, v]);
    }
}

fn write(dir: &Path, rel: &str, text: &str) {
    std::fs::write(dir.join(rel), text).unwrap();
}

struct Fixture {
    tmp: TempDir,
    db: AppDatabase,
    data: PathBuf,
    root: PathBuf,
    caller: GitCaller,
}

impl Fixture {
    fn host(&self) -> GitHost<'_> {
        GitHost::new(&self.db.conn, &self.data)
    }

    async fn run(&self, op: GitOp) -> Result<GitOutcome, GitError> {
        execute(&self.host(), &self.caller, op).await
    }

    async fn run_gh(&self, gh: &Path, op: GitOp) -> Result<GitOutcome, GitError> {
        let host = GitHost {
            gh: Some(gh),
            ..self.host()
        };
        execute(&host, &self.caller, op).await
    }
}

async fn workspace_id(db: &AppDatabase, id: i32) -> String {
    crate::db::entities::folder::Entity::find_by_id(id)
        .one(&db.conn)
        .await
        .unwrap()
        .unwrap()
        .workspace_id
        .unwrap()
}

/// A workspace over a fresh directory; `git` makes it a repository with one
/// commit.
async fn workspace(git_repo: bool) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("work");
    std::fs::create_dir_all(&root).unwrap();
    if git_repo {
        init_repo(&root);
        write(&root, "README.md", "hello\n");
        git(&root, &["add", "README.md"]);
        git(&root, &["commit", "-q", "-m", "init"]);
    }
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let db = fresh_in_memory_db().await;
    let id = seed_folder(&db, root.to_str().unwrap()).await;
    let caller = GitCaller::new(Some(workspace_id(&db, id).await), Some("s-1".into())).unwrap();
    Fixture {
        tmp,
        db,
        data,
        root,
        caller,
    }
}

/// A bare remote `origin` for `fx`, holding `main`.
fn add_origin(fx: &Fixture) -> PathBuf {
    let bare = fx.tmp.path().join("origin.git");
    git(
        fx.tmp.path(),
        &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
    );
    git(
        &fx.root,
        &["remote", "add", "origin", bare.to_str().unwrap()],
    );
    git(&fx.root, &["push", "-q", "-u", "origin", "main"]);
    bare
}

/// A second clone of `bare`, for making the remote move on.
fn clone_of(fx: &Fixture, bare: &Path, name: &str) -> PathBuf {
    let dir = fx.tmp.path().join(name);
    git(
        fx.tmp.path(),
        &["clone", "-q", bare.to_str().unwrap(), dir.to_str().unwrap()],
    );
    for (k, v) in [("user.name", "Other"), ("user.email", "other@example.com")] {
        git(&dir, &["config", k, v]);
    }
    dir
}

fn code(result: Result<GitOutcome, GitError>) -> GitErrorCode {
    match result {
        Ok(outcome) => panic!("expected an error, got {outcome:?}"),
        Err(e) => e.code,
    }
}

// ── the typed surface ───────────────────────────────────────────────────

#[test]
fn ops_have_a_stable_tagged_json_shape() {
    let cases = [
        (GitOp::Status, json!({"op": "status"})),
        (GitOp::Diff { file: None }, json!({"op": "diff"})),
        (
            GitOp::Diff {
                file: Some("src/a.rs".into()),
            },
            json!({"op": "diff", "file": "src/a.rs"}),
        ),
        (
            GitOp::Log { limit: Some(5) },
            json!({"op": "log", "limit": 5}),
        ),
        (
            GitOp::BranchCreate {
                name: "feat".into(),
                from: Some("origin/main".into()),
            },
            json!({"op": "branch_create", "name": "feat", "from": "origin/main"}),
        ),
        (
            GitOp::BranchRename { name: "fix".into() },
            json!({"op": "branch_rename", "name": "fix"}),
        ),
        (
            GitOp::Switch {
                branch: "main".into(),
            },
            json!({"op": "switch", "branch": "main"}),
        ),
        (
            GitOp::Stage {
                paths: vec!["a".into()],
            },
            json!({"op": "stage", "paths": ["a"]}),
        ),
        (
            GitOp::Commit {
                message: "m".into(),
                paths: Some(vec!["a".into()]),
            },
            json!({"op": "commit", "message": "m", "paths": ["a"]}),
        ),
        (GitOp::Fetch { remote: None }, json!({"op": "fetch"})),
        (
            GitOp::Pull {
                mode: PullMode::FastForwardOnly,
            },
            json!({"op": "pull", "mode": "fast_forward_only"}),
        ),
        (GitOp::Push { remote: None }, json!({"op": "push"})),
        (
            GitOp::PrCreate {
                title: "t".into(),
                body: "b".into(),
                base: None,
                draft: true,
            },
            json!({"op": "pr_create", "title": "t", "body": "b", "draft": true}),
        ),
        (
            GitOp::PrView { number: Some(3) },
            json!({"op": "pr_view", "number": 3}),
        ),
        (GitOp::PrChecks { number: None }, json!({"op": "pr_checks"})),
        (
            GitOp::PrComments { number: None },
            json!({"op": "pr_comments"}),
        ),
        (
            GitOp::PrMerge {
                number: Some(3),
                method: MergeMethod::Squash,
            },
            json!({"op": "pr_merge", "number": 3, "method": "squash"}),
        ),
    ];
    for (op, wire) in cases {
        assert_eq!(serde_json::to_value(&op).unwrap(), wire, "{op:?}");
        assert_eq!(serde_json::from_value::<GitOp>(wire).unwrap(), op);
    }
}

#[test]
fn optional_arguments_default() {
    let op: GitOp = serde_json::from_value(json!({"op": "pull"})).unwrap();
    assert_eq!(
        op,
        GitOp::Pull {
            mode: PullMode::FastForwardOnly
        }
    );
    let op: GitOp = serde_json::from_value(json!({"op": "pr_merge"})).unwrap();
    assert_eq!(
        op,
        GitOp::PrMerge {
            number: None,
            method: MergeMethod::Merge
        }
    );
    let op: GitOp = serde_json::from_value(json!({"op": "pr_create", "title": "t"})).unwrap();
    assert_eq!(
        op,
        GitOp::PrCreate {
            title: "t".into(),
            body: String::new(),
            base: None,
            draft: false
        }
    );
}

#[test]
fn there_is_no_way_to_ask_for_more_than_the_typed_surface() {
    for wire in [
        json!({"op": "push", "force": true}),
        json!({"op": "pr_merge", "admin": true}),
        json!({"op": "pr_merge", "delete_branch": true}),
        json!({"op": "log", "args": ["reset", "--hard"]}),
        json!({"op": "reset"}),
        json!({"op": "clean"}),
        json!({"op": "raw", "args": ["push", "--force"]}),
        json!({"op": "commit"}),
    ] {
        assert!(
            serde_json::from_value::<GitOp>(wire.clone()).is_err(),
            "{wire} parsed"
        );
    }
}

#[test]
fn kind_and_mutates_are_facts_about_each_op() {
    let n = None;
    let table = [
        (GitOp::Status, GitOpKind::Status, false),
        (GitOp::Diff { file: n.clone() }, GitOpKind::Diff, false),
        (GitOp::Log { limit: None }, GitOpKind::Log, false),
        (
            GitOp::BranchCreate {
                name: "b".into(),
                from: None,
            },
            GitOpKind::BranchCreate,
            true,
        ),
        (
            GitOp::BranchRename { name: "b".into() },
            GitOpKind::BranchRename,
            true,
        ),
        (
            GitOp::Switch { branch: "b".into() },
            GitOpKind::Switch,
            true,
        ),
        (GitOp::Stage { paths: vec![] }, GitOpKind::Stage, true),
        (
            GitOp::Commit {
                message: "m".into(),
                paths: None,
            },
            GitOpKind::Commit,
            true,
        ),
        (GitOp::Fetch { remote: None }, GitOpKind::Fetch, true),
        (
            GitOp::Pull {
                mode: PullMode::Merge,
            },
            GitOpKind::Pull,
            true,
        ),
        (GitOp::Push { remote: None }, GitOpKind::Push, true),
        (
            GitOp::PrCreate {
                title: "t".into(),
                body: String::new(),
                base: None,
                draft: false,
            },
            GitOpKind::PrCreate,
            true,
        ),
        (GitOp::PrView { number: None }, GitOpKind::PrView, false),
        (GitOp::PrChecks { number: None }, GitOpKind::PrChecks, false),
        (
            GitOp::PrComments { number: None },
            GitOpKind::PrComments,
            false,
        ),
        (
            GitOp::PrMerge {
                number: None,
                method: MergeMethod::Merge,
            },
            GitOpKind::PrMerge,
            true,
        ),
    ];
    for (op, kind, mutates) in table {
        assert_eq!(op.kind(), kind);
        assert_eq!(op.mutates(), mutates, "{op:?}");
        // The kind's name is the op's wire tag.
        assert_eq!(
            serde_json::to_value(&op).unwrap()["op"],
            json!(kind.as_str())
        );
        assert_eq!(serde_json::to_value(kind).unwrap(), json!(kind.as_str()));
    }
}

#[test]
fn error_codes_serialize_as_their_names() {
    use GitErrorCode::*;
    for code in [
        NoWorkspace,
        WorkspaceRemoved,
        NotAGitRepository,
        InvalidPath,
        InvalidInput,
        NotFound,
        AlreadyExists,
        NothingToCommit,
        Conflict,
        NonFastForward,
        NoUpstream,
        DetachedHead,
        AuthFailed,
        Network,
        GhUnavailable,
        GhUnauthenticated,
        NotMergeable,
        PrChanged,
        Timeout,
        Failed,
    ] {
        assert_eq!(serde_json::to_value(code).unwrap(), json!(code.as_str()));
    }
    let err = GitError::new(NothingToCommit, "nothing").with_detail("  ");
    assert_eq!(
        serde_json::to_value(&err).unwrap(),
        json!({"code": "nothing_to_commit", "message": "nothing"})
    );
}

#[test]
fn outcomes_are_tagged_by_op() {
    let outcome = GitOutcome::Push(PushOutcome {
        remote: "origin".into(),
        branch: "feat".into(),
        head: "abc".into(),
        set_upstream: true,
    });
    assert_eq!(
        serde_json::to_value(&outcome).unwrap(),
        json!({"op": "push", "remote": "origin", "branch": "feat", "head": "abc", "set_upstream": true})
    );
    let checks = GitOutcome::PrChecks { checks: vec![] };
    assert_eq!(
        serde_json::to_value(&checks).unwrap(),
        json!({"op": "pr_checks", "checks": []})
    );
}

// ── failure classification ──────────────────────────────────────────────

#[test]
fn git_failures_map_to_structured_codes() {
    use GitErrorCode::*;
    let cases = [
        (
            "On branch main\nnothing to commit, working tree clean",
            NothingToCommit,
        ),
        (
            "nothing added to commit but untracked files present",
            NothingToCommit,
        ),
        (
            " ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs",
            NonFastForward,
        ),
        (
            " ! [rejected]        main -> main (non-fast-forward)",
            NonFastForward,
        ),
        (
            "fatal: Not possible to fast-forward, aborting.",
            NonFastForward,
        ),
        ("CONFLICT (content): Merge conflict in a.txt", Conflict),
        (
            "error: Your local changes to the following files would be overwritten by checkout:",
            Conflict,
        ),
        ("fatal: a branch named 'feat' already exists", AlreadyExists),
        ("fatal: invalid reference: nope", NotFound),
        (
            "fatal: 'nope' does not appear to be a git repository",
            NotFound,
        ),
        (
            "There is no tracking information for the current branch.",
            NoUpstream,
        ),
        (
            "fatal: Authentication failed for 'https://github.com/o/r/'",
            AuthFailed,
        ),
        (
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
            AuthFailed,
        ),
        ("git@github.com: Permission denied (publickey).", AuthFailed),
        (
            "fatal: unable to access 'https://x.invalid/': Could not resolve host: x.invalid",
            Network,
        ),
        (
            "fatal: ':(literal)../x' is outside repository at '/w'",
            InvalidPath,
        ),
        ("something unexpected", Failed),
    ];
    for (stderr, want) in cases {
        let err = classify_git("op", stderr);
        assert_eq!(err.code, want, "{stderr}");
        assert_eq!(err.detail.as_deref(), Some(stderr.trim()));
    }
}

#[test]
fn gh_failures_map_to_structured_codes() {
    use GitErrorCode::*;
    let cases = [
        (Some(4), "", GhUnauthenticated),
        (
            Some(1),
            "To get started with GitHub CLI, please run:  gh auth login",
            GhUnauthenticated,
        ),
        (Some(1), "HTTP 401: Bad credentials", AuthFailed),
        (
            Some(1),
            "no pull requests found for branch \"feat\"",
            NotFound,
        ),
        (
            Some(1),
            "GraphQL: Could not resolve to a PullRequest with the number of 9.",
            NotFound,
        ),
        (
            Some(1),
            "a pull request for branch \"feat\" into branch \"main\" already exists",
            AlreadyExists,
        ),
        (
            Some(1),
            "X Pull request o/r#3 is not mergeable: the merge commit cannot be cleanly created.",
            NotMergeable,
        ),
        (Some(1), "error connecting to api.github.com", Network),
        (
            Some(1),
            "dial tcp: lookup api.github.com: no such host",
            Network,
        ),
        (
            Some(1),
            "Post \"https://api.github.com/graphql\": dial tcp: i/o timeout",
            Network,
        ),
        (Some(1), "net/http: TLS handshake timeout", Network),
        (
            Some(1),
            "read tcp 10.0.0.2:5->1.2.3.4:443: read: connection reset by peer",
            Network,
        ),
        (
            Some(1),
            "context deadline exceeded (Client.Timeout exceeded while awaiting headers)",
            Network,
        ),
        (
            Some(1),
            "GraphQL: Head branch was modified. Review and try the merge again. (mergePullRequest)",
            PrChanged,
        ),
        (Some(1), "something unexpected", Failed),
    ];
    for (exit, stderr, want) in cases {
        assert_eq!(classify_gh("op", exit, stderr).code, want, "{stderr}");
    }
}

#[test]
fn details_are_bounded() {
    let long = "x".repeat(10_000);
    let err = classify_git("op", &long);
    assert!(err.detail.unwrap().len() < 2100);
}

// ── workspace scope ─────────────────────────────────────────────────────

#[tokio::test]
async fn work_without_a_workspace_is_no_workspace() {
    for id in [None, Some(String::new()), Some("  ".into())] {
        assert_eq!(
            GitCaller::new(id, Some("s".into())).unwrap_err().code,
            GitErrorCode::NoWorkspace
        );
    }

    let fx = workspace(true).await;
    let unknown = GitCaller::new(Some("no-such-workspace".into()), None).unwrap();
    let err = execute(&fx.host(), &unknown, GitOp::Status)
        .await
        .unwrap_err();
    assert_eq!(err.code, GitErrorCode::NoWorkspace);

    // A chat scratch folder is a folder row but no workspace: it has no id
    // to name, so it cannot reach the Harness.
    let scratch = fx.tmp.path().join("scratch");
    let chat = folder_service::add_chat_folder(&fx.db.conn, scratch.to_str().unwrap())
        .await
        .unwrap();
    let row = crate::db::entities::folder::Entity::find_by_id(chat.id)
        .one(&fx.db.conn)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        GitCaller::new(row.workspace_id, None).unwrap_err().code,
        GitErrorCode::NoWorkspace
    );
}

#[tokio::test]
async fn the_repository_is_the_workspace_s_never_the_caller_s() {
    let a = workspace(true).await;
    let b = workspace(true).await;
    write(&b.root, "only-in-b.txt", "b\n");

    // The same database knows only `a`; `b`'s id resolves nowhere in it.
    let foreign = GitCaller::new(Some(b.caller.workspace_id.clone()), None).unwrap();
    assert_eq!(
        execute(&a.host(), &foreign, GitOp::Status)
            .await
            .unwrap_err()
            .code,
        GitErrorCode::NoWorkspace
    );

    let GitOutcome::Status(status) = a.run(GitOp::Status).await.unwrap() else {
        panic!()
    };
    assert!(status.entries.is_empty(), "{status:?}");
    // The session id is attribution only: another one sees the same
    // workspace.
    let other_session = GitCaller {
        session_id: Some("s-2".into()),
        ..a.caller.clone()
    };
    assert!(execute(&a.host(), &other_session, GitOp::Status)
        .await
        .is_ok());
}

#[tokio::test]
async fn a_removed_worktree_is_workspace_removed() {
    let fx = workspace(true).await;
    let root_id = seed_folder(&fx.db, fx.root.to_str().unwrap()).await;
    let wt = fx.tmp.path().join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    let wt_row =
        folder_service::add_folder_with_parent(&fx.db.conn, wt.to_str().unwrap(), Some(root_id))
            .await
            .unwrap();
    let wid = workspace_id(&fx.db, wt_row.id).await;
    folder_service::soft_delete_folder(&fx.db.conn, wt_row.id)
        .await
        .unwrap();

    let caller = GitCaller::new(Some(wid), None).unwrap();
    for op in [
        GitOp::Status,
        GitOp::Commit {
            message: "m".into(),
            paths: None,
        },
    ] {
        assert_eq!(
            execute(&fx.host(), &caller, op).await.unwrap_err().code,
            GitErrorCode::WorkspaceRemoved
        );
    }
}

#[tokio::test]
async fn a_folder_that_is_not_a_repository_is_not_a_git_repository() {
    let fx = workspace(false).await;
    write(&fx.root, "a.txt", "a\n");
    for op in [
        GitOp::Status,
        GitOp::Diff { file: None },
        GitOp::Log { limit: None },
        GitOp::Stage {
            paths: vec!["a.txt".into()],
        },
        GitOp::Commit {
            message: "m".into(),
            paths: None,
        },
        GitOp::Push { remote: None },
        GitOp::PrView { number: None },
    ] {
        assert_eq!(code(fx.run(op).await), GitErrorCode::NotAGitRepository);
    }
    assert!(
        !fx.root.join(".git").exists(),
        "nothing initialised a repository"
    );
}

#[tokio::test]
async fn paths_cannot_leave_the_workspace() {
    let fx = workspace(true).await;
    let outside = fx.tmp.path().join("outside.txt");
    write(fx.tmp.path(), "outside.txt", "secret\n");
    for path in [
        "../outside.txt",
        outside.to_str().unwrap(),
        "a/../../outside.txt",
        "",
    ] {
        assert_eq!(
            code(
                fx.run(GitOp::Stage {
                    paths: vec![path.into()]
                })
                .await
            ),
            GitErrorCode::InvalidPath,
            "{path}"
        );
        assert_eq!(
            code(
                fx.run(GitOp::Commit {
                    message: "m".into(),
                    paths: Some(vec![path.into()])
                })
                .await
            ),
            GitErrorCode::InvalidPath
        );
    }
    assert_eq!(
        code(
            fx.run(GitOp::Diff {
                file: Some("../outside.txt".into())
            })
            .await
        ),
        GitErrorCode::InvalidPath
    );
    assert_eq!(
        code(fx.run(GitOp::Stage { paths: vec![] }).await),
        GitErrorCode::InvalidPath
    );
}

// ── local operations ────────────────────────────────────────────────────

#[tokio::test]
async fn status_diff_and_log_report_the_workspace() {
    let fx = workspace(true).await;
    write(&fx.root, "README.md", "hello\nworld\n");
    write(&fx.root, "new.txt", "new\n");

    let GitOutcome::Status(status) = fx.run(GitOp::Status).await.unwrap() else {
        panic!()
    };
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(
        status.head.as_deref(),
        Some(git(&fx.root, &["rev-parse", "HEAD"]).as_str())
    );
    assert_eq!(status.upstream, None);
    let mut entries: Vec<_> = status
        .entries
        .iter()
        .map(|e| (e.status.as_str(), e.path.as_str()))
        .collect();
    entries.sort();
    assert_eq!(entries, [("??", "new.txt"), ("M", "README.md")]);

    let GitOutcome::Diff(diff) = fx
        .run(GitOp::Diff {
            file: Some("README.md".into()),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(diff.patch.flatten().unwrap().contains("+world"));

    let GitOutcome::Log(log) = fx.run(GitOp::Log { limit: Some(5) }).await.unwrap() else {
        panic!()
    };
    assert_eq!(log.entries.len(), 1);
    assert_eq!(log.entries[0].message, "init");
}

#[tokio::test]
async fn commit_with_paths_commits_exactly_those_paths() {
    let fx = workspace(true).await;
    write(&fx.root, "a.txt", "a\n");
    write(&fx.root, "b.txt", "b\n");

    let GitOutcome::Stage(staged) = fx
        .run(GitOp::Stage {
            paths: vec!["a.txt".into(), "b.txt".into()],
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(staged.staged, ["a.txt", "b.txt"]);

    let GitOutcome::Commit(commit) = fx
        .run(GitOp::Commit {
            message: "add a".into(),
            paths: Some(vec!["a.txt".into()]),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(commit.files, ["a.txt"]);
    assert_eq!(commit.branch.as_deref(), Some("main"));
    assert_eq!(commit.commit, git(&fx.root, &["rev-parse", "HEAD"]));
    // b.txt is still staged, for a later commit.
    assert_eq!(git(&fx.root, &["diff", "--cached", "--name-only"]), "b.txt");

    // Without paths: everything staged.
    let GitOutcome::Commit(commit) = fx
        .run(GitOp::Commit {
            message: "add b".into(),
            paths: None,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(commit.files, ["b.txt"]);
    assert_eq!(git(&fx.root, &["log", "-1", "--format=%s"]), "add b");
}

#[tokio::test]
async fn commit_paths_include_deletions_and_take_names_literally() {
    let fx = workspace(true).await;
    write(&fx.root, "*.txt", "star\n");
    write(&fx.root, "keep.txt", "keep\n");
    git(&fx.root, &["add", "--", ":(literal)*.txt", "keep.txt"]);
    git(&fx.root, &["commit", "-q", "-m", "files"]);

    // A deletion staged already, and one only in the working tree.
    git(&fx.root, &["rm", "-q", "README.md"]);
    std::fs::remove_file(fx.root.join("*.txt")).unwrap();
    write(&fx.root, "keep.txt", "changed\n");

    let GitOutcome::Commit(commit) = fx
        .run(GitOp::Commit {
            message: "delete".into(),
            paths: Some(vec!["README.md".into(), "*.txt".into()]),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    let mut files = commit.files.clone();
    files.sort();
    assert_eq!(files, ["*.txt", "README.md"]);
    // `*.txt` was a name, not a glob: keep.txt's change is uncommitted.
    assert_eq!(git(&fx.root, &["status", "--porcelain"]), "M keep.txt");
}

#[tokio::test]
async fn an_empty_commit_is_nothing_to_commit() {
    let fx = workspace(true).await;
    let commit = |paths| GitOp::Commit {
        message: "m".into(),
        paths,
    };
    assert_eq!(
        code(fx.run(commit(None)).await),
        GitErrorCode::NothingToCommit
    );
    write(&fx.root, "untracked.txt", "u\n");
    assert_eq!(
        code(fx.run(commit(None)).await),
        GitErrorCode::NothingToCommit
    );
    assert_eq!(
        code(
            fx.run(GitOp::Commit {
                message: "  ".into(),
                paths: None
            })
            .await
        ),
        GitErrorCode::InvalidInput
    );
}

#[tokio::test]
async fn branches_are_created_and_switched() {
    let fx = workspace(true).await;
    let GitOutcome::BranchCreate(created) = fx
        .run(GitOp::BranchCreate {
            name: "feat/x".into(),
            from: Some("main".into()),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(created.branch, "feat/x");
    assert_eq!(git(&fx.root, &["branch", "--show-current"]), "feat/x");
    // New work: it does not track the branch it started from.
    assert!(std::process::Command::new("git")
        .args(["config", "branch.feat/x.merge"])
        .current_dir(&fx.root)
        .output()
        .unwrap()
        .stdout
        .is_empty());

    let create = |name: &str| GitOp::BranchCreate {
        name: name.into(),
        from: None,
    };
    assert_eq!(
        code(fx.run(create("feat/x")).await),
        GitErrorCode::AlreadyExists
    );
    for bad in ["-D", "bad..name", "has space", ""] {
        assert_eq!(
            code(fx.run(create(bad)).await),
            GitErrorCode::InvalidInput,
            "{bad}"
        );
    }
    assert_eq!(
        code(
            fx.run(GitOp::BranchCreate {
                name: "y".into(),
                from: Some("no-such-ref".into())
            })
            .await
        ),
        GitErrorCode::NotFound
    );

    let switch = |branch: &str| GitOp::Switch {
        branch: branch.into(),
    };
    let GitOutcome::Switch(switched) = fx.run(switch("main")).await.unwrap() else {
        panic!()
    };
    assert_eq!(switched.branch, "main");
    assert_eq!(
        code(fx.run(switch("no-such-branch")).await),
        GitErrorCode::NotFound
    );
    assert_eq!(
        code(fx.run(switch("--orphan")).await),
        GitErrorCode::InvalidInput
    );

    // A switch that would overwrite local changes is refused, not forced.
    git(&fx.root, &["switch", "-q", "feat/x"]);
    write(&fx.root, "README.md", "on feat\n");
    git(&fx.root, &["commit", "-q", "-am", "feat"]);
    write(&fx.root, "README.md", "uncommitted\n");
    assert_eq!(code(fx.run(switch("main")).await), GitErrorCode::Conflict);
    assert_eq!(
        std::fs::read_to_string(fx.root.join("README.md")).unwrap(),
        "uncommitted\n"
    );
}

/// A Host-style worktree `pune` of `fx`'s repo on branch `pune`, and a
/// caller in its workspace.
async fn worktree(fx: &Fixture) -> (PathBuf, GitCaller) {
    let root_id = seed_folder(&fx.db, fx.root.to_str().unwrap()).await;
    let wt = fx.tmp.path().join("pune");
    git(
        &fx.root,
        &["worktree", "add", "-q", "-b", "pune", wt.to_str().unwrap()],
    );
    let row =
        folder_service::add_folder_with_parent(&fx.db.conn, wt.to_str().unwrap(), Some(root_id))
            .await
            .unwrap();
    let caller = GitCaller::new(Some(workspace_id(&fx.db, row.id).await), None).unwrap();
    (wt, caller)
}

fn rename(name: &str) -> GitOp {
    GitOp::BranchRename { name: name.into() }
}

#[tokio::test]
async fn a_worktree_branch_is_renamed() {
    let fx = workspace(true).await;
    let (wt, caller) = worktree(&fx).await;

    let GitOutcome::BranchRename(renamed) = execute(&fx.host(), &caller, rename("fix-login"))
        .await
        .unwrap()
    else {
        panic!()
    };

    assert_eq!(renamed.branch, "fix-login");
    assert_eq!(renamed.head, git(&wt, &["rev-parse", "HEAD"]));
    assert_eq!(git(&wt, &["branch", "--show-current"]), "fix-login");
    assert_eq!(branches(&fx.root, "pune"), "");
}

/// The local branch named `name`, or nothing.
fn branches(dir: &Path, name: &str) -> String {
    git(
        dir,
        &["branch", "--list", "--format=%(refname:short)", name],
    )
}

#[tokio::test]
async fn a_root_workspace_branch_is_not_renamed() {
    let fx = workspace(true).await;

    assert_eq!(
        code(fx.run(rename("trunk")).await),
        GitErrorCode::InvalidInput
    );
    assert_eq!(git(&fx.root, &["branch", "--show-current"]), "main");
}

#[tokio::test]
async fn a_detached_worktree_has_no_branch_to_rename() {
    let fx = workspace(true).await;
    let (wt, caller) = worktree(&fx).await;
    git(&wt, &["switch", "-q", "--detach"]);

    let result = execute(&fx.host(), &caller, rename("fix")).await;

    assert_eq!(code(result), GitErrorCode::DetachedHead);
    assert_eq!(branches(&fx.root, "pune"), "pune");
}

#[tokio::test]
async fn a_pushed_branch_is_not_renamed() {
    let fx = workspace(true).await;
    add_origin(&fx);
    let (wt, caller) = worktree(&fx).await;
    // With an upstream.
    git(&wt, &["push", "-q", "-u", "origin", "pune"]);
    let e = execute(&fx.host(), &caller, rename("fix"))
        .await
        .unwrap_err();
    assert_eq!(e.code, GitErrorCode::InvalidInput);
    assert!(e.message.contains("pushed"), "{}", e.message);
    // `origin/pune` alone, without an upstream, is pushed too.
    git(&wt, &["branch", "--unset-upstream"]);
    assert_eq!(
        code(execute(&fx.host(), &caller, rename("fix")).await),
        GitErrorCode::InvalidInput
    );
    assert_eq!(git(&wt, &["branch", "--show-current"]), "pune");
}

#[tokio::test]
async fn a_branch_rename_refuses_an_invalid_name() {
    let fx = workspace(true).await;
    let (wt, caller) = worktree(&fx).await;
    for bad in ["-D", "bad..name", "has space", ""] {
        assert_eq!(
            code(execute(&fx.host(), &caller, rename(bad)).await),
            GitErrorCode::InvalidInput,
            "{bad}"
        );
    }
    assert_eq!(git(&wt, &["branch", "--show-current"]), "pune");
}

#[tokio::test]
async fn a_branch_rename_refuses_a_taken_name() {
    let fx = workspace(true).await;
    let (wt, caller) = worktree(&fx).await;
    git(
        &fx.root,
        &["update-ref", "refs/remotes/origin/shipped", "HEAD"],
    );
    git(&fx.root, &["tag", "v1"]);
    // A branch, a remote branch only, a tag, a case-only twin, its own name.
    for taken in ["main", "shipped", "v1", "Main", "pune"] {
        assert_eq!(
            code(execute(&fx.host(), &caller, rename(taken)).await),
            GitErrorCode::AlreadyExists,
            "{taken}"
        );
    }
    assert_eq!(git(&wt, &["branch", "--show-current"]), "pune");
}

// ── remote operations ───────────────────────────────────────────────────

#[tokio::test]
async fn push_sets_the_upstream_once_and_never_forces() {
    let fx = workspace(true).await;
    let bare = add_origin(&fx);

    fx.run(GitOp::BranchCreate {
        name: "feat".into(),
        from: Some("origin/main".into()),
    })
    .await
    .unwrap();
    write(&fx.root, "f.txt", "f\n");
    fx.run(GitOp::Commit {
        message: "f".into(),
        paths: Some(vec!["f.txt".into()]),
    })
    .await
    .unwrap();

    let GitOutcome::Push(pushed) = fx.run(GitOp::Push { remote: None }).await.unwrap() else {
        panic!()
    };
    assert_eq!(
        (pushed.remote.as_str(), pushed.branch.as_str()),
        ("origin", "feat")
    );
    assert!(pushed.set_upstream);
    assert_eq!(git(&bare, &["rev-parse", "feat"]), pushed.head);
    // main on the remote is untouched.
    assert_eq!(
        git(&bare, &["rev-parse", "main"]),
        git(&fx.root, &["rev-parse", "main"])
    );
    let GitOutcome::Status(status) = fx.run(GitOp::Status).await.unwrap() else {
        panic!()
    };
    assert_eq!(status.upstream.as_deref(), Some("origin/feat"));

    write(&fx.root, "f.txt", "f2\n");
    fx.run(GitOp::Commit {
        message: "f2".into(),
        paths: Some(vec!["f.txt".into()]),
    })
    .await
    .unwrap();
    let GitOutcome::Push(again) = fx.run(GitOp::Push { remote: None }).await.unwrap() else {
        panic!()
    };
    assert!(!again.set_upstream);

    // Someone else moves the remote branch: the push is refused, not forced.
    let other = clone_of(&fx, &bare, "other");
    git(&other, &["switch", "-q", "feat"]);
    write(&other, "o.txt", "o\n");
    git(&other, &["add", "o.txt"]);
    git(&other, &["commit", "-q", "-m", "other"]);
    git(&other, &["push", "-q"]);
    write(&fx.root, "f.txt", "f3\n");
    git(&fx.root, &["commit", "-q", "-am", "f3"]);
    let theirs = git(&bare, &["rev-parse", "feat"]);
    assert_eq!(
        code(fx.run(GitOp::Push { remote: None }).await),
        GitErrorCode::NonFastForward
    );
    assert_eq!(git(&bare, &["rev-parse", "feat"]), theirs);

    // Fast-forward-only pull refuses diverged history; a merge pull merges.
    assert_eq!(
        code(
            fx.run(GitOp::Pull {
                mode: PullMode::FastForwardOnly
            })
            .await
        ),
        GitErrorCode::NonFastForward
    );
    let GitOutcome::Pull(pulled) = fx
        .run(GitOp::Pull {
            mode: PullMode::Merge,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(pulled.updated);
    assert!(fx.root.join("o.txt").exists());
    assert!(fx.run(GitOp::Push { remote: None }).await.is_ok());
}

#[tokio::test]
async fn fetch_and_fast_forward_pull() {
    let fx = workspace(true).await;
    let bare = add_origin(&fx);
    let other = clone_of(&fx, &bare, "other");
    write(&other, "o.txt", "o\n");
    git(&other, &["add", "o.txt"]);
    git(&other, &["commit", "-q", "-m", "other"]);
    git(&other, &["push", "-q"]);

    let GitOutcome::Fetch(fetched) = fx.run(GitOp::Fetch { remote: None }).await.unwrap() else {
        panic!()
    };
    assert_eq!(fetched.remote, "origin");
    let GitOutcome::Status(status) = fx.run(GitOp::Status).await.unwrap() else {
        panic!()
    };
    assert_eq!((status.ahead, status.behind), (0, 1));

    let GitOutcome::Pull(pulled) = fx
        .run(GitOp::Pull {
            mode: PullMode::FastForwardOnly,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(pulled.updated);
    assert_eq!(pulled.head, git(&other, &["rev-parse", "HEAD"]));
    let GitOutcome::Pull(again) = fx
        .run(GitOp::Pull {
            mode: PullMode::FastForwardOnly,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(!again.updated);

    assert_eq!(
        code(
            fx.run(GitOp::Fetch {
                remote: Some("nope".into())
            })
            .await
        ),
        GitErrorCode::NotFound
    );
    assert_eq!(
        code(
            fx.run(GitOp::Fetch {
                remote: Some("--upload-pack=touch pwned".into())
            })
            .await
        ),
        GitErrorCode::InvalidInput
    );
}

#[tokio::test]
async fn a_conflicting_merge_pull_is_conflict() {
    let fx = workspace(true).await;
    let bare = add_origin(&fx);
    let other = clone_of(&fx, &bare, "other");
    write(&other, "README.md", "theirs\n");
    git(&other, &["commit", "-q", "-am", "theirs"]);
    git(&other, &["push", "-q"]);
    write(&fx.root, "README.md", "ours\n");
    git(&fx.root, &["commit", "-q", "-am", "ours"]);

    assert_eq!(
        code(
            fx.run(GitOp::Pull {
                mode: PullMode::Merge
            })
            .await
        ),
        GitErrorCode::Conflict
    );
}

#[tokio::test]
async fn pull_and_push_need_a_branch_and_pull_an_upstream() {
    let fx = workspace(true).await;
    add_origin(&fx);
    fx.run(GitOp::BranchCreate {
        name: "local-only".into(),
        from: None,
    })
    .await
    .unwrap();
    assert_eq!(
        code(
            fx.run(GitOp::Pull {
                mode: PullMode::FastForwardOnly
            })
            .await
        ),
        GitErrorCode::NoUpstream
    );

    git(&fx.root, &["switch", "-q", "--detach"]);
    assert_eq!(
        code(fx.run(GitOp::Push { remote: None }).await),
        GitErrorCode::DetachedHead
    );
    assert_eq!(
        code(
            fx.run(GitOp::Pull {
                mode: PullMode::Merge
            })
            .await
        ),
        GitErrorCode::DetachedHead
    );
}

#[tokio::test]
async fn a_repository_without_a_remote_has_nothing_to_push_to() {
    let fx = workspace(true).await;
    assert_eq!(
        code(fx.run(GitOp::Push { remote: None }).await),
        GitErrorCode::NotFound
    );
}

// ── GitHub ──────────────────────────────────────────────────────────────

const REPO_JSON: &str = r#"{"squashMergeAllowed":true,"mergeCommitAllowed":true,"rebaseMergeAllowed":false,"viewerDefaultMergeMethod":"MERGE"}"#;

/// `gh pr view --json` for the PR-state fields, escaped for a `format!`ed
/// fake `gh` body.
fn pr_state_json(number: u64, state: &str, merge: &str, head: &str, auto: bool) -> String {
    json!({
        "number": number,
        "title": "T",
        "state": state,
        "url": format!("https://github.com/o/r/pull/{number}"),
        "isDraft": false,
        "baseRefName": "main",
        "headRefName": "feat",
        "headRefOid": head,
        "mergeStateStatus": merge,
        "statusCheckRollup": [],
        "mergedAt": if state == "MERGED" { json!("2026-10-03T00:00:00Z") } else { json!(null) },
        "autoMergeRequest": if auto { json!({"mergeMethod": "SQUASH"}) } else { json!(null) },
    })
    .to_string()
}

const PR_JSON: &str = r#"{"number":7,"title":"T","state":"OPEN","url":"https://github.com/o/r/pull/7","isDraft":false,"baseRefName":"main","headRefName":"feat","mergeable":"MERGEABLE","reviewDecision":"","author":{"login":"me"},"body":"b"}"#;

#[tokio::test]
async fn gh_must_be_installed_and_logged_in() {
    let fx = workspace(true).await;
    let missing = fx.tmp.path().join("no-gh");
    assert_eq!(
        code(fx.run_gh(&missing, GitOp::PrView { number: None }).await),
        GitErrorCode::GhUnavailable
    );

    let (gh, _) = fake_gh(
        fx.tmp.path(),
        "*) echo 'To get started with GitHub CLI, please run:  gh auth login' >&2; exit 4 ;;",
    );
    for op in [
        GitOp::PrView { number: None },
        GitOp::PrChecks { number: None },
        GitOp::PrMerge {
            number: Some(1),
            method: MergeMethod::Merge,
        },
    ] {
        assert_eq!(
            code(fx.run_gh(&gh, op).await),
            GitErrorCode::GhUnauthenticated
        );
    }
}

const ACCOUNT_GH: &str = r#"
"api user") echo '{"login":"octo","id":42,"name":"Octo Cat"}';;
"api user/orgs"*) printf 'acme\nlabs\n';;
"api repos/octo/taken"*) echo 1;;
"api repos/octo/locked"*) echo 'HTTP 401: Bad credentials' >&2; exit 1;;
"api repos/"*) echo 'gh: Not Found (HTTP 404)' >&2; exit 1;;
"repo create"*) exit 0;;
"#;

#[tokio::test]
async fn github_account_reads_the_signed_in_user() {
    let dir = TempDir::new().unwrap();
    let (gh, calls) = fake_gh(dir.path(), ACCOUNT_GH);
    let cwd = dir.path().to_str().unwrap();
    let account = github_account(Some(&gh), cwd).await.unwrap();
    assert_eq!(
        account,
        GithubAccount {
            login: "octo".into(),
            id: 42,
            name: Some("Octo Cat".into())
        }
    );
    let owners = github_owners(Some(&gh), cwd, &account).await.unwrap();
    let listed: Vec<_> = owners.iter().map(|o| (o.login.as_str(), o.kind)).collect();
    assert_eq!(
        listed,
        [
            ("octo", GithubOwnerKind::User),
            ("acme", GithubOwnerKind::Org),
            ("labs", GithubOwnerKind::Org)
        ]
    );
    let log = std::fs::read_to_string(calls).unwrap();
    assert!(
        log.contains("api user/orgs --paginate --jq .[].login | prompt=1"),
        "{log}"
    );
}

#[tokio::test]
async fn github_owners_is_the_login_alone_when_orgs_fail() {
    let dir = TempDir::new().unwrap();
    let (gh, _) = fake_gh(
        dir.path(),
        "\"api user/orgs\"*) echo 'HTTP 403' >&2; exit 1;;",
    );
    let account = GithubAccount {
        login: "octo".into(),
        id: 42,
        name: None,
    };
    let owners = github_owners(Some(&gh), dir.path().to_str().unwrap(), &account)
        .await
        .unwrap();
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0].login, "octo");
}

#[tokio::test]
async fn github_repo_exists_is_false_only_on_not_found() {
    let dir = TempDir::new().unwrap();
    let (gh, _) = fake_gh(dir.path(), ACCOUNT_GH);
    let cwd = dir.path().to_str().unwrap();
    assert!(github_repo_exists(Some(&gh), cwd, "octo", "taken")
        .await
        .unwrap());
    assert!(!github_repo_exists(Some(&gh), cwd, "octo", "free")
        .await
        .unwrap());
    let err = github_repo_exists(Some(&gh), cwd, "octo", "locked")
        .await
        .unwrap_err();
    assert_eq!(err.code, GitErrorCode::AuthFailed);
}

#[tokio::test]
async fn github_repo_create_makes_a_private_repo_from_the_folder() {
    let dir = TempDir::new().unwrap();
    let (gh, calls) = fake_gh(dir.path(), ACCOUNT_GH);
    let path = dir.path().to_str().unwrap();
    github_repo_create(Some(&gh), path, "acme", "app")
        .await
        .unwrap();
    let log = std::fs::read_to_string(calls).unwrap();
    assert!(
        log.contains(&format!(
            "repo create acme/app --private --source {path} --remote origin --push | prompt=1"
        )),
        "{log}"
    );
}

#[tokio::test]
async fn github_account_without_gh_is_gh_unavailable() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("no-gh");
    let err = github_account(Some(&missing), dir.path().to_str().unwrap())
        .await
        .unwrap_err();
    assert_eq!(err.code, GitErrorCode::GhUnavailable);
}

#[tokio::test]
async fn pull_requests_are_created_viewed_and_followed() {
    let fx = workspace(true).await;
    git(&fx.root, &["switch", "-q", "-c", "feat"]);
    let merged_flag = fx.tmp.path().join("merged");
    let open = pr_state_json(7, "OPEN", "CLEAN", "abc123", false);
    let merged = pr_state_json(7, "MERGED", "UNKNOWN", "abc123", false);
    let (gh, calls) = fake_gh(
        fx.tmp.path(),
        &format!(
            r#"
"pr create"*) echo 'Creating pull request'; echo 'https://github.com/o/r/pull/7' ;;
"pr view"*autoMergeRequest*) if [ -f '{flag}' ]; then echo '{merged}'; else echo '{open}'; fi ;;
"pr view"*comments*) echo '{{"number":7,"comments":[{{"author":{{"login":"rev"}},"body":"nit","createdAt":"t","url":"u"}}],"reviews":[{{"author":{{"login":"rev"}},"state":"CHANGES_REQUESTED","body":"fix","submittedAt":"t"}}]}}' ;;
"pr view"*) echo '{PR_JSON}' ;;
"pr checks"*) echo '[{{"name":"ci","state":"IN_PROGRESS","bucket":"pending","link":"l","workflow":"w"}}]'; exit 8 ;;
api*) echo '[{{"user":{{"login":"rev"}},"path":"a.rs","line":null,"original_line":3,"body":"here","created_at":"t","html_url":"h"}}]' ;;
"repo view"*) echo '{REPO_JSON}' ;;
"pr merge"*) touch '{flag}'; echo merged ;;
*) echo "unexpected: $*" >&2; exit 1 ;;"#,
            flag = merged_flag.display(),
        ),
    );

    let GitOutcome::PrCreate(created) = fx
        .run_gh(
            &gh,
            GitOp::PrCreate {
                title: "Title".into(),
                body: "Body".into(),
                base: Some("main".into()),
                draft: true,
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(created.number, Some(7));
    assert_eq!(created.url, "https://github.com/o/r/pull/7");

    let GitOutcome::PrView(pr) = fx
        .run_gh(&gh, GitOp::PrView { number: None })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!((pr.number, pr.author.as_deref()), (7, Some("me")));
    assert_eq!(pr.review_decision, None);
    // gh's camelCase in, the Harness's snake_case out.
    let wire = serde_json::to_value(&pr).unwrap();
    assert_eq!(wire["head_ref_name"], "feat");
    assert_eq!(wire["is_draft"], false);

    // Pending checks exit 8; the checks are still the answer.
    let GitOutcome::PrChecks { checks } = fx
        .run_gh(&gh, GitOp::PrChecks { number: Some(7) })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(checks[0].bucket, "pending");

    let GitOutcome::PrComments(comments) = fx
        .run_gh(&gh, GitOp::PrComments { number: None })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(comments.number, 7);
    assert_eq!(comments.comments[0].author.as_deref(), Some("rev"));
    assert_eq!(comments.reviews[0].state, "CHANGES_REQUESTED");
    assert_eq!(comments.review_comments[0].line, Some(3));

    let GitOutcome::PrMerge(merged) = fx
        .run_gh(
            &gh,
            GitOp::PrMerge {
                number: None,
                method: MergeMethod::Squash,
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(merged.number, 7);

    let log = std::fs::read_to_string(calls).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert!(lines.iter().all(|l| l.ends_with("prompt=1")), "{log}");
    assert!(
        lines[0].starts_with("pr create --title Title --body Body --head feat --base main --draft")
    );
    assert!(lines
        .iter()
        .any(|l| l.starts_with("api repos/{owner}/{repo}/pulls/7/comments")));
    // The merge pins the number and the head it checked, and asks for
    // nothing more.
    let merge = lines.iter().find(|l| l.starts_with("pr merge")).unwrap();
    assert!(
        merge.starts_with("pr merge 7 --squash --match-head-commit abc123 |"),
        "{merge}"
    );
    assert!(!log.contains("--admin") && !log.contains("--auto") && !log.contains("-d "));
    assert_eq!(merged.method, MergeMethod::Squash);
}

#[tokio::test]
async fn gh_failures_are_structured() {
    let fx = workspace(true).await;
    let (gh, _) = fake_gh(
        fx.tmp.path(),
        &format!(
            r#"
"pr view 3 "*) echo '{open}' ;;
"pr view"*) echo 'no pull requests found for branch "main"' >&2; exit 1 ;;
"pr checks"*) echo "no checks reported on the 'main' branch" >&2; exit 1 ;;
"repo view"*) echo '{REPO_JSON}' ;;
"pr merge"*) echo 'X Pull request o/r#3 is not mergeable: the merge commit cannot be cleanly created.' >&2; exit 1 ;;"#,
            open = pr_state_json(3, "OPEN", "CLEAN", "abc", false),
        ),
    );
    assert_eq!(
        code(fx.run_gh(&gh, GitOp::PrView { number: None }).await),
        GitErrorCode::NotFound
    );
    let GitOutcome::PrChecks { checks } = fx
        .run_gh(&gh, GitOp::PrChecks { number: None })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(checks.is_empty());
    assert_eq!(
        code(
            fx.run_gh(
                &gh,
                GitOp::PrMerge {
                    number: Some(3),
                    method: MergeMethod::Merge
                }
            )
            .await
        ),
        GitErrorCode::NotMergeable
    );
    // A pull request needs a branch to come from.
    git(&fx.root, &["switch", "-q", "--detach"]);
    assert_eq!(
        code(
            fx.run_gh(
                &gh,
                GitOp::PrCreate {
                    title: "t".into(),
                    body: String::new(),
                    base: None,
                    draft: false
                }
            )
            .await
        ),
        GitErrorCode::DetachedHead
    );
}

/// A fake `gh` whose `pr view` answers `view` before any merge and `after`
/// once `pr merge` ran.
fn merge_gh(fx: &Fixture, view: &str, after: &str) -> (PathBuf, PathBuf) {
    let flag = fx.tmp.path().join("merge-ran");
    let _ = std::fs::remove_file(&flag);
    let _ = std::fs::remove_file(fx.tmp.path().join("gh-calls"));
    fake_gh(
        fx.tmp.path(),
        &format!(
            r#"
"pr view"*) if [ -f '{flag}' ]; then echo '{after}'; else echo '{view}'; fi ;;
"repo view"*) echo '{REPO_JSON}' ;;
"pr merge"*--disable-auto*) echo off ;;
"pr merge"*) touch '{flag}' ;;
*) echo "unexpected: $*" >&2; exit 1 ;;"#,
            flag = flag.display(),
        ),
    )
}

async fn merge_with(fx: &Fixture, gh: &Path, head: &str) -> Result<PrMerged, GitError> {
    let host = GitHost {
        gh: Some(gh),
        ..fx.host()
    };
    merge_pr(&host, &fx.caller, 5, head, None).await
}

#[tokio::test]
async fn a_merge_is_pinned_to_the_head_the_user_saw() {
    let fx = workspace(true).await;
    let open = pr_state_json(5, "OPEN", "CLEAN", "new-head", false);
    let (gh, calls) = merge_gh(&fx, &open, &open);
    let err = merge_with(&fx, &gh, "old-head").await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::PrChanged);
    let log = std::fs::read_to_string(&calls).unwrap();
    assert!(!log.contains("pr merge"), "{log}");

    // An empty head is refused before GitHub is asked anything.
    let err = merge_with(&fx, &gh, " ").await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::InvalidInput);
}

#[tokio::test]
async fn a_merge_github_would_not_do_now_is_refused_before_gh_runs() {
    let fx = workspace(true).await;
    for (status, state, why) in [
        ("BLOCKED", "OPEN", "Waiting for required checks or review"),
        ("BEHIND", "OPEN", "Branch is behind main"),
        ("DIRTY", "OPEN", "Merge conflicts"),
        ("CLEAN", "MERGED", "already merged"),
        ("CLEAN", "CLOSED", "closed"),
    ] {
        let view = pr_state_json(5, state, status, "h", false);
        let (gh, calls) = merge_gh(&fx, &view, &view);
        let err = merge_with(&fx, &gh, "h").await.unwrap_err();
        assert_eq!(err.code, GitErrorCode::NotMergeable, "{status} {state}");
        assert!(err.message.contains(why), "{}", err.message);
        let log = std::fs::read_to_string(&calls).unwrap();
        assert!(!log.contains("pr merge"), "{log}");
    }
}

#[tokio::test]
async fn a_merge_runs_now_with_the_preferred_method_and_head() {
    let fx = workspace(true).await;
    let (gh, calls) = merge_gh(
        &fx,
        &pr_state_json(5, "OPEN", "UNSTABLE", "h1", false),
        &pr_state_json(5, "MERGED", "UNKNOWN", "h1", false),
    );
    let merged = merge_with(&fx, &gh, "h1").await.unwrap();
    // The repository's github.com default (merge commit) wins over squash.
    assert_eq!(merged.method, MergeMethod::Merge);
    assert_eq!(merged.merged_at.as_deref(), Some("2026-10-03T00:00:00Z"));
    let log = std::fs::read_to_string(&calls).unwrap();
    let merge: Vec<&str> = log.lines().filter(|l| l.starts_with("pr merge")).collect();
    assert_eq!(merge.len(), 1, "{log}");
    assert!(
        merge[0].starts_with("pr merge 5 --merge --match-head-commit h1 |"),
        "{log}"
    );
}

#[tokio::test]
async fn a_merge_queued_for_later_is_turned_off_and_refused() {
    let fx = workspace(true).await;
    let (gh, calls) = merge_gh(
        &fx,
        &pr_state_json(5, "OPEN", "CLEAN", "h1", false),
        &pr_state_json(5, "OPEN", "BLOCKED", "h1", true),
    );
    let err = merge_with(&fx, &gh, "h1").await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::NotMergeable);
    assert!(err.message.contains("queued"), "{}", err.message);
    let log = std::fs::read_to_string(&calls).unwrap();
    assert!(log.contains("pr merge 5 --disable-auto"), "{log}");

    // The agent tool takes the same guard.
    let (gh, calls) = merge_gh(
        &fx,
        &pr_state_json(5, "OPEN", "CLEAN", "h1", false),
        &pr_state_json(5, "OPEN", "BLOCKED", "h1", true),
    );
    let err = fx
        .run_gh(
            &gh,
            GitOp::PrMerge {
                number: Some(5),
                method: MergeMethod::Squash,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, GitErrorCode::NotMergeable);
    let log = std::fs::read_to_string(&calls).unwrap();
    assert!(log.contains("--match-head-commit h1"), "{log}");
    assert!(log.contains("pr merge 5 --disable-auto"), "{log}");
}

#[tokio::test]
async fn pr_state_follows_the_branch_and_no_pr_is_none() {
    let fx = workspace(true).await;
    let (gh, _) = fake_gh(
        fx.tmp.path(),
        &format!(
            r#"
"pr view feat "*) echo '{open}' ;;
"pr view"*) echo 'no pull requests found for branch "other"' >&2; exit 1 ;;
"repo view"*) echo '{REPO_JSON}' ;;"#,
            open = pr_state_json(9, "OPEN", "BLOCKED", "h", false),
        ),
    );
    let host = GitHost {
        gh: Some(&gh),
        ..fx.host()
    };
    let pr = pr_state(&host, &fx.caller, "feat").await.unwrap().unwrap();
    assert_eq!(
        (pr.number, pr.state, pr.head_sha.as_str()),
        (9, PrLifecycle::Open, "h")
    );
    assert!(!pr.merge.allowed);
    assert_eq!(
        pr.merge.methods,
        vec![MergeMethod::Merge, MergeMethod::Squash]
    );
    assert_eq!(pr_state(&host, &fx.caller, "other").await.unwrap(), None);
    assert_eq!(
        pr_state(&host, &fx.caller, "--web").await.unwrap_err().code,
        GitErrorCode::InvalidInput
    );
}

// ── PR state mapping ────────────────────────────────────────────────────

fn raw_pr(
    state: &str,
    merge: &str,
    draft: bool,
    rollup: serde_json::Value,
) -> super::pr_state::RawPr {
    serde_json::from_value(json!({
        "number": 1, "state": state, "isDraft": draft, "baseRefName": "main",
        "headRefName": "feat", "headRefOid": "h", "mergeStateStatus": merge,
        "statusCheckRollup": rollup,
    }))
    .unwrap()
}

#[test]
fn merge_state_maps_to_allowed_and_a_reason() {
    let methods = || vec![MergeMethod::Squash];
    let cases = [
        ("CLEAN", true, None),
        ("UNSTABLE", true, None),
        ("HAS_HOOKS", true, None),
        (
            "BLOCKED",
            false,
            Some("Waiting for required checks or review"),
        ),
        ("BEHIND", false, Some("Branch is behind main")),
        ("DIRTY", false, Some("Merge conflicts")),
        ("DRAFT", false, Some("Draft PR")),
        ("UNKNOWN", false, Some("Checking…")),
        ("SOMETHING_NEW", false, Some("Checking…")),
    ];
    for (status, allowed, reason) in cases {
        let pr = raw_pr("OPEN", status, false, json!([])).into_state(methods());
        assert_eq!(pr.merge.allowed, allowed, "{status}");
        assert_eq!(pr.merge.reason.as_deref(), reason, "{status}");
        assert_eq!(pr.merge.method, Some(MergeMethod::Squash));
    }
    let wire =
        serde_json::to_value(raw_pr("OPEN", "HAS_HOOKS", false, json!([])).into_state(methods()))
            .unwrap();
    assert_eq!(wire["merge"]["status"], "has_hooks");
    assert_eq!(wire["merge"]["method"], "squash");
    assert_eq!(wire["state"], "open");

    // Settled pull requests are never mergeable and need no reason.
    for state in ["MERGED", "CLOSED"] {
        let pr = raw_pr(state, "CLEAN", false, json!([])).into_state(methods());
        assert!(!pr.merge.allowed && pr.merge.reason.is_none(), "{state}");
        assert!(pr.state.is_final());
    }
    let draft = raw_pr("OPEN", "DRAFT", true, json!([])).into_state(methods());
    assert_eq!(draft.state, PrLifecycle::Draft);
    let none = raw_pr("OPEN", "CLEAN", false, json!([])).into_state(vec![]);
    assert!(!none.merge.allowed);
    assert_eq!(
        none.merge.reason.as_deref(),
        Some("The repository allows no merge method")
    );
}

#[test]
fn the_rollup_maps_check_runs_and_status_contexts() {
    let rollup = json!([
        {"__typename": "CheckRun", "name": "build", "workflowName": "CI", "status": "COMPLETED", "conclusion": "SUCCESS", "detailsUrl": "https://ci/1"},
        {"__typename": "CheckRun", "name": "lint", "status": "COMPLETED", "conclusion": "FAILURE"},
        {"__typename": "CheckRun", "name": "e2e", "status": "IN_PROGRESS", "conclusion": ""},
        {"__typename": "CheckRun", "name": "queued", "status": "QUEUED"},
        {"__typename": "CheckRun", "name": "docs", "status": "COMPLETED", "conclusion": "SKIPPED"},
        {"__typename": "CheckRun", "name": "neutral", "status": "COMPLETED", "conclusion": "NEUTRAL"},
        {"__typename": "CheckRun", "name": "cancel", "status": "COMPLETED", "conclusion": "CANCELLED"},
        {"__typename": "CheckRun", "name": "timeout", "status": "COMPLETED", "conclusion": "TIMED_OUT"},
        {"__typename": "StatusContext", "context": "ci/legacy", "state": "SUCCESS", "targetUrl": "https://legacy"},
        {"__typename": "StatusContext", "context": "deploy", "state": "PENDING"},
        {"__typename": "StatusContext", "context": "sec", "state": "ERROR"},
    ]);
    let checks = raw_pr("OPEN", "UNSTABLE", false, rollup)
        .into_state(vec![])
        .checks;
    assert_eq!(
        (checks.passed, checks.failed, checks.pending, checks.skipped),
        (3, 4, 3, 1)
    );
    assert_eq!(checks.items[0].workflow.as_deref(), Some("CI"));
    assert_eq!(checks.items[0].link.as_deref(), Some("https://ci/1"));
    assert_eq!(checks.items[8].name, "ci/legacy");
    assert_eq!(checks.items[8].link.as_deref(), Some("https://legacy"));
    assert_eq!(checks.items[2].state, CheckState::Pending);
    // No rollup at all is no checks.
    let pr: super::pr_state::RawPr =
        serde_json::from_value(json!({"number": 2, "statusCheckRollup": null})).unwrap();
    assert_eq!(pr.into_state(vec![]).checks.items.len(), 0);
}

#[test]
fn merge_methods_prefer_the_user_s_default() {
    use super::pr_state::RepoMergeSettings;
    let settings = |sq, mc, rb, def: Option<&str>| RepoMergeSettings {
        squash_merge_allowed: sq,
        merge_commit_allowed: mc,
        rebase_merge_allowed: rb,
        viewer_default_merge_method: def.map(str::to_string),
    };
    use MergeMethod::*;
    assert_eq!(
        settings(true, true, true, None).methods(),
        vec![Squash, Merge, Rebase]
    );
    assert_eq!(
        settings(true, true, true, Some("REBASE")).methods(),
        vec![Rebase, Squash, Merge]
    );
    // A default the repository no longer allows is skipped.
    assert_eq!(
        settings(true, false, false, Some("MERGE")).methods(),
        vec![Squash]
    );
    assert!(settings(false, false, false, None).methods().is_empty());
}

// ── time limits and cancellation ────────────────────────────────────────

/// A shell that starts a long-lived grandchild, records its pid, and waits.
#[cfg(unix)]
fn sleeper(dir: &Path) -> (tokio::process::Command, PathBuf) {
    let pidfile = dir.join("grandchild.pid");
    let mut cmd = tokio::process::Command::new("sh");
    cmd.args([
        "-c",
        &format!("sleep 60 & echo $! > '{}'; wait", pidfile.display()),
    ]);
    (cmd, pidfile)
}

#[cfg(unix)]
async fn grandchild(pidfile: &Path) -> i32 {
    for _ in 0..100 {
        if let Ok(pid) = std::fs::read_to_string(pidfile) {
            if let Ok(pid) = pid.trim().parse() {
                return pid;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the grandchild never started");
}

#[cfg(unix)]
async fn assert_gone(pid: i32) {
    for _ in 0..100 {
        // SAFETY: signal 0 only probes for existence.
        if unsafe { libc::kill(pid, 0) } != 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    unsafe { libc::kill(pid, libc::SIGKILL) };
    panic!("process {pid} outlived the call");
}

#[cfg(unix)]
#[tokio::test]
async fn a_timed_out_run_kills_its_whole_process_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let (cmd, pidfile) = sleeper(tmp.path());
    let started = std::time::Instant::now();
    let pid_wait = tokio::spawn({
        let pidfile = pidfile.clone();
        async move { grandchild(&pidfile).await }
    });
    let err = run::run(cmd, Duration::from_millis(500), "sleeper")
        .await
        .err()
        .unwrap();
    assert_eq!(err.code, GitErrorCode::Timeout);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_gone(pid_wait.await.unwrap()).await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_cancelled_run_kills_its_whole_process_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let (cmd, pidfile) = sleeper(tmp.path());
    let call = run::run(cmd, Duration::from_secs(120), "sleeper");
    let cancelled = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::select! {
            _ = call => panic!("the sleeper finished"),
            pid = grandchild(&pidfile) => pid,
        }
    })
    .await
    .unwrap();
    // `call` was dropped when `select!` returned.
    assert_gone(cancelled).await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_finished_run_leaves_its_group_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let pidfile = tmp.path().join("daemon.pid");
    let mut cmd = tokio::process::Command::new("sh");
    // A daemon that closes the call's pipes, like a credential cache.
    cmd.args([
        "-c",
        &format!(
            "sleep 60 >/dev/null 2>&1 </dev/null & echo $! > '{}'",
            pidfile.display()
        ),
    ]);
    let out = run::run(cmd, Duration::from_secs(10), "daemon")
        .await
        .unwrap();
    assert!(out.status.success());
    let pid = grandchild(&pidfile).await;
    // SAFETY: probe, then clean up the daemon this test started.
    assert_eq!(unsafe { libc::kill(pid, 0) }, 0, "the daemon was killed");
    unsafe { libc::kill(pid, libc::SIGKILL) };
}

// ── conflicts ───────────────────────────────────────────────────────────

/// `git <args>` in `dir` that may fail; whether it succeeded.
fn git_may_fail(dir: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("run git")
        .status
        .success()
}

fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

/// A workspace with `origin`, whose `main` holds `a.txt` and `b.txt`, and a
/// second clone for moving `origin/main` on.
async fn conflict_fixture() -> (Fixture, PathBuf) {
    let fx = workspace(true).await;
    write(&fx.root, "a.txt", "base\n");
    write(&fx.root, "b.txt", "base\n");
    commit_all(&fx.root, "base");
    let bare = add_origin(&fx);
    let other = clone_of(&fx, &bare, "other");
    (fx, other)
}

/// Commit `text` to `rel` in `other`, push it, and fetch it into `fx`.
fn upstream_edit(fx: &Fixture, other: &Path, rel: &str, text: &str) {
    write(other, rel, text);
    commit_all(other, "upstream");
    git(other, &["push", "-q"]);
    git(&fx.root, &["fetch", "-q"]);
}

/// A workspace whose `main` and `origin/main` both changed `a.txt`.
async fn text_conflict() -> Fixture {
    let (fx, other) = conflict_fixture().await;
    write(&fx.root, "a.txt", "mine\n");
    commit_all(&fx.root, "mine");
    upstream_edit(&fx, &other, "a.txt", "theirs\n");
    fx
}

async fn check(fx: &Fixture) -> ConflictCheck {
    check_conflicts(&fx.root, "origin/main")
        .await
        .unwrap()
        .expect("origin/main resolves")
}

#[tokio::test]
async fn conflicts_clean_when_base_merges() {
    let (fx, other) = conflict_fixture().await;
    write(&fx.root, "a.txt", "mine\n");
    commit_all(&fx.root, "mine");
    upstream_edit(&fx, &other, "b.txt", "theirs\n");

    let found = check(&fx).await;
    assert_eq!(found.state, ConflictState::Clean);
    assert_eq!(found.base, "origin/main");
    assert_eq!(found.base_sha, git(&fx.root, &["rev-parse", "origin/main"]));
    assert_eq!(found.head_sha, git(&fx.root, &["rev-parse", "HEAD"]));
}

#[tokio::test]
async fn conflicts_list_text_conflict_files() {
    let fx = text_conflict().await;
    assert_eq!(
        check(&fx).await.state,
        ConflictState::Conflicts {
            files: vec!["a.txt".into()]
        }
    );
}

#[tokio::test]
async fn conflicts_list_delete_vs_edit() {
    let (fx, other) = conflict_fixture().await;
    git(&fx.root, &["rm", "-q", "b.txt"]);
    git(&fx.root, &["commit", "-q", "-m", "drop b"]);
    upstream_edit(&fx, &other, "b.txt", "theirs\n");
    assert_eq!(
        check(&fx).await.state,
        ConflictState::Conflicts {
            files: vec!["b.txt".into()]
        }
    );
}

#[tokio::test]
async fn conflicts_report_merge_in_progress() {
    let fx = text_conflict().await;
    assert!(!git_may_fail(&fx.root, &["merge", "-q", "origin/main"]));
    assert_eq!(
        check(&fx).await.state,
        ConflictState::InProgress {
            operation: "merge".into(),
            files: vec!["a.txt".into()]
        }
    );
}

#[tokio::test]
async fn conflicts_base_is_the_remote_tracking_ref() {
    // A local branch and a tag named like the base must not stand in for it.
    let fx = text_conflict().await;
    git(&fx.root, &["branch", "-q", "origin/main", "HEAD~1"]);
    git(&fx.root, &["tag", "origin/main", "HEAD~1"]);
    let found = check(&fx).await;
    assert_eq!(
        found.state,
        ConflictState::Conflicts {
            files: vec!["a.txt".into()]
        }
    );
    assert_eq!(
        found.base_sha,
        git(&fx.root, &["rev-parse", "refs/remotes/origin/main"])
    );
    assert_eq!(
        conflict_base(&fx.root, false, None).await.unwrap(),
        Some("origin/main".into())
    );
}

#[tokio::test]
async fn conflicts_none_without_base() {
    let fx = workspace(true).await;
    assert_eq!(check_conflicts(&fx.root, "origin/main").await.unwrap(), None);
}

#[tokio::test]
async fn conflicts_report_unmerged_without_operation() {
    let (fx, other) = conflict_fixture().await;
    write(&fx.root, "a.txt", "mine\n");
    git(&fx.root, &["stash", "-q"]);
    upstream_edit(&fx, &other, "a.txt", "theirs\n");
    git(&fx.root, &["merge", "-q", "--ff-only", "origin/main"]);
    assert!(!git_may_fail(&fx.root, &["stash", "pop", "-q"]));
    assert_eq!(
        check(&fx).await.state,
        ConflictState::InProgress {
            operation: "unmerged".into(),
            files: vec!["a.txt".into()]
        }
    );
}

#[tokio::test]
async fn conflicts_check_does_not_touch_worktree() {
    let fx = text_conflict().await;
    write(&fx.root, "README.md", "edited\n");
    write(&fx.root, "new.txt", "untracked\n");
    git(&fx.root, &["add", "new.txt"]);
    write(&fx.root, "loose.txt", "untracked\n");
    let state = |fx: &Fixture| {
        (
            git(&fx.root, &["status", "--porcelain=v1", "-uall"]),
            git(&fx.root, &["rev-parse", "HEAD"]),
            std::fs::read(fx.root.join(".git/index")).unwrap(),
        )
    };
    let before = state(&fx);
    assert!(matches!(
        check(&fx).await.state,
        ConflictState::Conflicts { .. }
    ));
    assert_eq!(state(&fx), before);
}

#[tokio::test]
async fn conflicts_fetch_base_updates_the_tracking_ref() {
    let (fx, other) = conflict_fixture().await;
    write(&other, "a.txt", "theirs\n");
    commit_all(&other, "upstream");
    git(&other, &["push", "-q"]);
    fetch_base(&fx.host(), &fx.root, "origin/main").await.unwrap();
    assert_eq!(
        git(&fx.root, &["rev-parse", "origin/main"]),
        git(&other, &["rev-parse", "HEAD"])
    );
    assert!(fetch_base(&fx.host(), &fx.root, "nowhere/main")
        .await
        .is_err());
}

#[tokio::test]
async fn conflicts_base_of_a_root_is_its_upstream() {
    let (fx, _) = conflict_fixture().await;
    assert_eq!(
        conflict_base(&fx.root, false, None).await.unwrap(),
        Some("origin/main".into())
    );
    git(&fx.root, &["branch", "-q", "--unset-upstream"]);
    assert_eq!(conflict_base(&fx.root, false, None).await.unwrap(), None);
}

#[tokio::test]
async fn conflicts_base_of_a_worktree_is_its_remote_base_branch() {
    let (fx, _) = conflict_fixture().await;
    let base = |b: Option<&'static str>| conflict_base(&fx.root, true, b);
    assert_eq!(base(Some("main")).await.unwrap(), Some("origin/main".into()));
    assert_eq!(
        base(Some("origin/main")).await.unwrap(),
        Some("origin/main".into())
    );
    assert_eq!(base(Some("gone")).await.unwrap(), None);
    assert_eq!(base(None).await.unwrap(), None);
}

#[tokio::test]
async fn conflicts_base_needs_a_remote() {
    let fx = workspace(true).await;
    assert_eq!(conflict_base(&fx.root, false, None).await.unwrap(), None);
    assert_eq!(
        conflict_base(&fx.root, true, Some("main")).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn push_refuses_unmerged_index() {
    let fx = text_conflict().await;
    git(&fx.root, &["checkout", "-q", "-b", "feat"]);
    assert!(!git_may_fail(&fx.root, &["merge", "-q", "origin/main"]));
    let bare = fx.tmp.path().join("origin.git");

    assert_eq!(
        code(fx.run(GitOp::Push { remote: None }).await),
        GitErrorCode::Conflict
    );
    assert!(!git_may_fail(
        &bare,
        &["rev-parse", "-q", "--verify", "feat"]
    ));

    // Unmerged entries with no merge running block it too.
    git(&fx.root, &["merge", "--abort"]);
    write(&fx.root, "a.txt", "dirty\n");
    git(&fx.root, &["stash", "-q"]);
    git(&fx.root, &["reset", "-q", "--hard", "origin/main"]);
    assert!(!git_may_fail(&fx.root, &["stash", "pop", "-q"]));
    assert_eq!(
        code(fx.run(GitOp::Push { remote: None }).await),
        GitErrorCode::Conflict
    );
    assert!(!git_may_fail(
        &bare,
        &["rev-parse", "-q", "--verify", "feat"]
    ));
}

#[tokio::test]
async fn push_allows_committed_marker_fixture() {
    let (fx, _) = conflict_fixture().await;
    let bare = fx.tmp.path().join("origin.git");
    git(&fx.root, &["checkout", "-q", "-b", "feat"]);
    write(
        &fx.root,
        "fixture.txt",
        "<<<<<<< ours\nmine\n=======\ntheirs\n>>>>>>> theirs\n",
    );
    commit_all(&fx.root, "marker fixture");

    let GitOutcome::Push(pushed) = fx.run(GitOp::Push { remote: None }).await.unwrap() else {
        panic!()
    };
    assert_eq!(git(&bare, &["rev-parse", "feat"]), pushed.head);
}

#[tokio::test]
async fn marker_scan_reads_resolved_file_contents() {
    let (fx, _) = conflict_fixture().await;
    let pair = "<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> origin/main\n";
    // Committed before the turn: still flagged, the scan reads contents.
    write(&fx.root, "a.txt", pair);
    commit_all(&fx.root, "markers in the start tree");
    // Outside the resolved files: ignored.
    write(&fx.root, "b.txt", pair);
    // An opening marker with no closing one is not a leftover conflict.
    write(&fx.root, "c.txt", "<<<<<<< just text\n");
    // A file the resolution deleted is skipped.
    let files: Vec<String> = ["a.txt", "c.txt", "gone.txt"]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(
        leftover_markers(&fx.root, &files).await.unwrap(),
        vec!["a.txt".to_string()]
    );

    // The marker length follows the `conflict-marker-size` attribute.
    write(&fx.root, "d.txt", pair);
    write(
        &fx.root,
        "e.txt",
        "<<<<<<<<<< HEAD\nmine\n==========\ntheirs\n>>>>>>>>>> origin/main\n",
    );
    write(
        &fx.root,
        ".gitattributes",
        "d.txt conflict-marker-size=10\ne.txt conflict-marker-size=10\n",
    );
    let files: Vec<String> = ["d.txt", "e.txt"].into_iter().map(String::from).collect();
    assert_eq!(
        leftover_markers(&fx.root, &files).await.unwrap(),
        vec!["e.txt".to_string()]
    );
}

#[tokio::test]
async fn marker_scan_skips_paths_outside_the_worktree() {
    let (fx, _) = conflict_fixture().await;
    let pair = "<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> origin/main\n";
    write(&fx.root, "a.txt", pair);
    // A path leaving the worktree is skipped without failing the rest.
    let files: Vec<String> = ["../x", "/etc/passwd", "a.txt"]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(
        leftover_markers(&fx.root, &files).await.unwrap(),
        vec!["a.txt".to_string()]
    );
}

// ── boundary ────────────────────────────────────────────────────────────

/// The Harness serves every agent integration and depends on none: no
/// agent protocol, tool bridge, provider, or approval vocabulary.
#[test]
fn the_harness_knows_nothing_about_agent_integrations() {
    let sources = [
        ("git_harness.rs", include_str!("../git_harness.rs")),
        ("op.rs", include_str!("op.rs")),
        ("error.rs", include_str!("error.rs")),
        ("run.rs", include_str!("run.rs")),
        ("local.rs", include_str!("local.rs")),
        ("remote.rs", include_str!("remote.rs")),
        ("github.rs", include_str!("github.rs")),
        ("pr_state.rs", include_str!("pr_state.rs")),
        ("conflicts.rs", include_str!("conflicts.rs")),
    ];
    for (name, source) in sources {
        let code = source.split("#[cfg(test)]").next().unwrap().to_lowercase();
        for banned in [
            "acp",
            "sacp",
            "agent_client_protocol",
            "mcp",
            "claude",
            "codex",
            "tokenentry",
            "brokermessage",
            "elicitation",
            "permissionoption",
            "requestpermission",
            "crate::protocol",
            "crate::session",
        ] {
            assert!(!code.contains(banned), "{name} mentions {banned}");
        }
    }
}
