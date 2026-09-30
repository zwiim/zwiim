//! The instructions Zwiim gives every Claude and Codex session, through each
//! agent's own system-prompt channel (never as a conversation message, so no
//! client transcript shows them). Built once per connection from its working
//! directory: whether it is a Git worktree, its branch, and the origin's
//! default branch.

use std::path::Path;

/// What the instructions say about the working directory.
#[derive(Debug, Default, PartialEq, Eq)]
struct Context {
    cwd: String,
    is_git: bool,
    worktree: bool,
    branch: Option<String>,
    /// The origin's default branch, e.g. `origin/main`.
    default_branch: Option<String>,
}

/// The instructions for a session working in `cwd`.
pub async fn for_dir(cwd: &Path) -> String {
    render(&context(cwd).await)
}

async fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = crate::process::tokio_command("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn context(cwd: &Path) -> Context {
    let mut ctx = Context {
        cwd: cwd.to_string_lossy().into_owned(),
        ..Context::default()
    };
    let Some(dirs) = git(
        cwd,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ],
    )
    .await
    else {
        return ctx;
    };
    ctx.is_git = true;
    let mut dirs = dirs.lines();
    ctx.worktree = matches!((dirs.next(), dirs.next()), (Some(a), Some(b)) if a != b);
    ctx.branch = git(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .await
        .filter(|b| !b.is_empty());
    let root = cwd.to_string_lossy();
    ctx.default_branch = crate::workspace::origin_default_branch(&root).await;
    ctx
}

fn render(ctx: &Context) -> String {
    let default = ctx.default_branch.as_deref();
    let mut workspace = vec![format!(
        "Working directory: {}. Work only here unless the user asks otherwise.",
        ctx.cwd
    )];
    if ctx.worktree {
        let on_branch = ctx
            .branch
            .as_deref()
            .map(|b| format!(" on branch {b}"))
            .unwrap_or_default();
        workspace.push(format!(
            "This is a Git worktree{on_branch}. Run commands from this directory; don't cd to the main checkout."
        ));
        workspace.push(
            "The Git stash is shared with the main checkout and other worktrees. Never use bare `git stash` / `git stash pop`; prefer a WIP commit."
                .into(),
        );
    }
    if let Some(default) = default {
        workspace.push(format!(
            "Default branch: {default}. Compare and open PRs against it; don't assume the local branch is current."
        ));
        if ctx.worktree {
            workspace.push(format!(
                "New Zwiim worktrees are based on {default} and its resolved base commit, not the local master branch. Never silently fall back to a local master."
            ));
        }
    }

    let mut out = String::from(
        "You are running in Zwiim, on the user's machine. The user may be on Desktop or on their phone.\n\nWorkspace\n",
    );
    for line in workspace {
        out.push_str(&format!("- {line}\n"));
    }
    out.push_str(
        "\nRuntime
- Zwiim Host is the source of truth for workspace, session, terminal, and agent state.
- Do not bypass the Host by directly manipulating managed runtime state.
- A client disconnect does not mean the Host session or agent has stopped.

Rules
- Inspect before changing. Make the smallest correct change. Keep unrelated user changes.
- Ask before destructive or irreversible actions: discarding changes, reset --hard, force-push, deleting branches or data.
- Commit, push, open a PR or merge only when asked. Never commit secrets.
- Never expose, print, commit, or persist credentials, API keys, tokens, private keys, or other secrets.
- Don't claim something works without running checks; say what wasn't verified.
- Follow the project's instructions (CLAUDE.md, AGENTS.md).
- These instructions apply for the entire session.
",
    );
    if !ctx.is_git {
        return out;
    }

    out.push_str("\nGit\n");
    if let Some(default) = default {
        out.push_str(&format!(
            "- Pull requests: first `git fetch`, then check the diff against {default}. Push the branch, then open the PR against {default} with `gh pr create` if it is available. Give it a short title and a body that says what changed and how it was tested. Report the PR URL.
- Updating a branch: merge {default} into it. Don't rebase a branch that is already pushed unless the user asks.
"
        ));
    }
    out.push_str(
        "- Merging a PR: only when asked. Check that CI and reviews pass first, and follow the repo's merge style.
- Conflicts: read both sides and the intent of each change before editing. Never resolve by taking \"ours\" or \"theirs\" wholesale. Remove every conflict marker, then build and test before committing. If the right result is unclear, stop and ask. If it gets messy, `git merge --abort` returns to where you started.
",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree() -> Context {
        Context {
            cwd: "/w/repo/abc".into(),
            is_git: true,
            worktree: true,
            branch: Some("zwiim/abc".into()),
            default_branch: Some("origin/main".into()),
        }
    }

    #[test]
    fn worktree_with_a_remote_gets_every_line() {
        let text = render(&worktree());
        assert!(text.starts_with("You are running in Zwiim, on the user's machine."));
        assert!(text.contains(
            "\nWorkspace\n- Working directory: /w/repo/abc. Work only here unless the user asks otherwise.\n"
        ));
        assert!(text.contains("- This is a Git worktree on branch zwiim/abc. Run commands"));
        assert!(text.contains("Never use bare `git stash` / `git stash pop`"));
        assert!(text.contains("- Default branch: origin/main. Compare and open PRs against it"));
        assert!(text.contains(
            "- New Zwiim worktrees are based on origin/main and its resolved base commit"
        ));
        assert!(text.contains("check the diff against origin/main. Push the branch, then open the PR against origin/main"));
        assert!(text.contains("- Updating a branch: merge origin/main into it."));
        assert!(text.contains("\nRuntime\n- Zwiim Host is the source of truth"));
        assert!(text.contains("\nRules\n- Inspect before changing."));
        assert!(text.contains("`git merge --abort` returns to where you started.\n"));
        assert!(!text.contains('{'), "no unfilled placeholder: {text}");
        let sections: Vec<_> = text
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with("- "))
            .collect();
        assert_eq!(sections[1..], ["Workspace", "Runtime", "Rules", "Git"]);
    }

    #[test]
    fn the_default_branch_is_whatever_origin_names() {
        let text = render(&Context {
            default_branch: Some("origin/master".into()),
            ..worktree()
        });
        assert!(text.contains("- Default branch: origin/master."));
        assert!(text.contains("merge origin/master into it"));
        assert!(!text.contains("origin/main"));
    }

    #[test]
    fn a_root_checkout_has_no_worktree_lines() {
        let text = render(&Context {
            worktree: false,
            branch: Some("main".into()),
            ..worktree()
        });
        assert!(!text.contains("worktree"), "{text}");
        assert!(!text.contains("git stash"));
        assert!(text.contains("- Default branch: origin/main."));
    }

    #[test]
    fn without_a_remote_nothing_names_a_default_branch() {
        let text = render(&Context {
            default_branch: None,
            ..worktree()
        });
        assert!(!text.contains("Default branch"));
        assert!(!text.contains("Pull requests:"));
        assert!(!text.contains("Updating a branch:"));
        assert!(!text.contains("New Zwiim worktrees"));
        assert!(text.contains("- Merging a PR: only when asked."));
        assert!(text.contains("- Conflicts: read both sides"));
    }

    #[test]
    fn outside_git_there_is_no_git_section() {
        let text = render(&Context {
            cwd: "/tmp/notes".into(),
            ..Context::default()
        });
        assert!(text.contains("- Working directory: /tmp/notes."));
        assert!(text.contains("\nRules\n"));
        assert!(!text.contains("Git"), "{text}");
    }

    fn run(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    #[tokio::test]
    async fn context_reads_the_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
        let plain = dir.path().join("plain");
        std::fs::create_dir(&plain).unwrap();

        assert!(!context(&plain).await.is_git);
        let root = context(&repo).await;
        assert!(root.is_git && !root.worktree);
        assert_eq!(root.branch.as_deref(), Some("main"));
        assert_eq!(root.default_branch, None);

        let repo_str = repo.to_string_lossy().into_owned();
        run(&repo, &["remote", "add", "origin", &repo_str]);
        run(&repo, &["fetch", "-q", "origin"]);
        let wt = dir.path().join("wt");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "zwiim/abc",
                wt.to_str().unwrap(),
            ],
        );
        let ctx = context(&wt).await;
        assert!(ctx.is_git && ctx.worktree);
        assert_eq!(ctx.branch.as_deref(), Some("zwiim/abc"));
        assert_eq!(ctx.default_branch.as_deref(), Some("origin/main"));
    }
}
