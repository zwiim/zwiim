//! The session instructions as ordered layers. A layer renders a titled
//! section, or nothing when it has nothing to say; the instructions are the
//! layers' sections in order. No conversation message carries them, so no
//! client transcript shows them.

use super::HarnessInput;

/// A layer of the session instructions, in render order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// L0: what Zwiim is and how its Host behaves.
    Runtime,
    /// L1: how to work: inspect first, ask before destructive actions, keep
    /// secrets out, verify claims.
    Rules,
    /// L2: the working directory, and whether it is a worktree.
    Workspace,
    /// L3: the origin's default branch and the Git workflow around it.
    Git,
    /// L4: reserved for project instructions; renders nothing yet. Agents
    /// still read CLAUDE.md / AGENTS.md themselves.
    ProjectInstructions,
    /// L5: reserved for skills; renders nothing yet.
    Skills,
}

/// What a layer says, and so whether a later layer may refine it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authority {
    /// How the Host works or what it found. Later layers may not contradict
    /// it, and the behavior behind it is enforced in Host code anyway.
    Host,
    /// Soft guidance on how to work. Later layers may refine it.
    Guidance,
}

impl Layer {
    pub const ORDER: [Layer; 6] = [
        Layer::Runtime,
        Layer::Rules,
        Layer::Workspace,
        Layer::Git,
        Layer::ProjectInstructions,
        Layer::Skills,
    ];

    pub const fn authority(self) -> Authority {
        match self {
            Layer::Runtime | Layer::Workspace => Authority::Host,
            Layer::Rules | Layer::Git | Layer::ProjectInstructions | Layer::Skills => {
                Authority::Guidance
            }
        }
    }

    fn render(self, input: &HarnessInput) -> Option<String> {
        match self {
            Layer::Runtime => Some(runtime()),
            Layer::Rules => Some(rules()),
            Layer::Workspace => Some(workspace(input)),
            Layer::Git => git(input),
            Layer::ProjectInstructions | Layer::Skills => None,
        }
    }
}

/// The session instructions for `input`.
pub fn render(input: &HarnessInput) -> String {
    Layer::ORDER
        .iter()
        .filter_map(|layer| layer.render(input))
        .collect::<Vec<_>>()
        .join("\n")
}

fn section(title: &str, lines: &[String]) -> String {
    let mut out = format!("{title}\n");
    for line in lines {
        out.push_str(&format!("- {line}\n"));
    }
    out
}

fn runtime() -> String {
    "You are running in Zwiim, on the user's machine. The user may be on Desktop or on their phone.\n\n".to_string()
        + &section(
            "Runtime",
            &[
                "Zwiim Host is the source of truth for workspace, session, terminal, and agent state.".into(),
                "Do not bypass the Host by directly manipulating managed runtime state.".into(),
                "A client disconnect does not mean the Host session or agent has stopped.".into(),
            ],
        )
}

fn rules() -> String {
    section(
        "Rules",
        &[
            "Inspect before changing. Make the smallest correct change. Keep unrelated user changes.".into(),
            "Ask before destructive or irreversible actions: discarding changes, reset --hard, force-push, deleting branches or data.".into(),
            "Commit, push, open a PR or merge only when asked. Never commit secrets.".into(),
            "Never expose, print, commit, or persist credentials, API keys, tokens, private keys, or other secrets.".into(),
            "Don't claim something works without running checks; say what wasn't verified.".into(),
            "Follow the project's instructions (CLAUDE.md, AGENTS.md).".into(),
            "These instructions apply for the entire session.".into(),
        ],
    )
}

fn workspace(input: &HarnessInput) -> String {
    let mut lines = vec![format!(
        "Working directory: {}. Work only here unless the user asks otherwise.",
        input.cwd.display()
    )];
    if input.git.as_ref().is_some_and(|git| git.worktree) {
        lines.push(
            "This is a Git worktree. Run commands from this directory; don't cd to the main checkout."
                .into(),
        );
        lines.push(
            "The Git stash is shared with the main checkout and other worktrees. Never use bare `git stash` / `git stash pop`; prefer a WIP commit."
                .into(),
        );
    }
    if let Some(branch) = input
        .git
        .as_ref()
        .and_then(|git| git.placeholder_branch.as_deref())
    {
        lines.push(format!(
            "This worktree started on placeholder branch `{branch}`. Once you know the task, and before you push, rename it with `git_branch_rename` to a short kebab-case name for the work."
        ));
    }
    section("Workspace", &lines)
}

fn git(input: &HarnessInput) -> Option<String> {
    let git = input.git.as_ref()?;
    let mut lines = Vec::new();
    if let Some(default) = git.default_branch.as_deref() {
        lines.push(format!(
            "Default branch: {default}. Compare and open PRs against it; don't assume the local branch is current."
        ));
        if git.worktree {
            lines.push(format!(
                "New Zwiim worktrees are based on {default} and its resolved base commit, not the local master branch. Never silently fall back to a local master."
            ));
        }
        // GitHub names the base as a branch (`main`), not the remote-tracking ref (`origin/main`).
        let base = default
            .split_once('/')
            .map_or(default, |(_, branch)| branch);
        lines.push(format!(
            "Raising or opening a PR: commit pending changes if there are any, push with the `git_push` tool, then open the PR with the `git_pr_create` tool, base `{base}`. Give it a short title and a body that says what changed and how it was tested. Report the PR URL. Don't wait for CI; Zwiim shows the checks to the user."
        ));
        lines.push(format!(
            "Updating a branch: merge {default} into it. Don't rebase a branch that is already pushed unless the user asks."
        ));
    }
    if git.default_branch.is_none() {
        lines.push(
            "Raising or opening a PR: commit pending changes if there are any, push with the `git_push` tool, then open the PR with the `git_pr_create` tool. Report the PR URL. Don't wait for CI; Zwiim shows the checks to the user."
                .into(),
        );
    }
    lines.push(
        "Use the Zwiim Git tools (`git_status`, `git_commit`, `git_push`, `git_pr_create`, …) for Git and GitHub work; run `git` or `gh` directly only when those tools are unavailable."
            .into(),
    );
    lines.push(
        "Merging a PR: only when asked. Check that CI and reviews pass first, follow the repo's merge style, and merge with the `git_pr_merge` tool."
            .into(),
    );
    lines.push(
        "Conflicts: read both sides and the intent of each change before editing. Never resolve by taking \"ours\" or \"theirs\" wholesale. Remove every conflict marker, then build and test before committing. If the right result is unclear, stop and ask. If it gets messy, `git merge --abort` returns to where you started."
            .into(),
    );
    Some(section("Git", &lines))
}

#[cfg(test)]
mod tests {
    use super::super::GitFacts;
    use super::*;

    fn worktree() -> HarnessInput {
        HarnessInput {
            cwd: "/w/repo/pune".into(),
            git: Some(GitFacts {
                worktree: true,
                default_branch: Some("origin/main".into()),
                placeholder_branch: Some("pune".into()),
            }),
        }
    }

    fn sections(text: &str) -> Vec<&str> {
        text.lines()
            .filter(|l| !l.is_empty() && !l.starts_with("- "))
            .skip(1)
            .collect()
    }

    #[test]
    fn worktree_with_a_remote_gets_every_line() {
        let text = render(&worktree());
        assert!(text.starts_with("You are running in Zwiim, on the user's machine."));
        assert!(text.contains(
            "\nWorkspace\n- Working directory: /w/repo/pune. Work only here unless the user asks otherwise.\n"
        ));
        assert!(text.contains("- This is a Git worktree. Run commands from this directory; don't cd to the main checkout.\n"));
        assert!(text.contains("- This worktree started on placeholder branch `pune`. Once you know the task, and before you push, rename it with `git_branch_rename` to a short kebab-case name for the work.\n"));
        assert!(text.contains("Never use bare `git stash` / `git stash pop`; prefer a WIP commit."));
        assert!(
            text.contains("\nGit\n- Default branch: origin/main. Compare and open PRs against it")
        );
        assert!(text.contains(
            "- New Zwiim worktrees are based on origin/main and its resolved base commit"
        ));
        assert!(text.contains("- Raising or opening a PR: commit pending changes if there are any, push with the `git_push` tool, then open the PR with the `git_pr_create` tool, base `main`."));
        assert!(text.contains("Report the PR URL. Don't wait for CI"));
        assert!(!text.contains("gh pr create"));
        assert!(text.contains("run `git` or `gh` directly only when those tools are unavailable"));
        assert!(text.contains("- Updating a branch: merge origin/main into it."));
        assert!(text.contains("\nRuntime\n- Zwiim Host is the source of truth"));
        assert!(text.contains("\nRules\n- Inspect before changing."));
        assert!(text.contains("- Follow the project's instructions (CLAUDE.md, AGENTS.md).\n"));
        assert!(text.ends_with("`git merge --abort` returns to where you started.\n"));
        assert!(!text.contains('{'), "no unfilled placeholder: {text}");
    }

    #[test]
    fn layers_render_in_order() {
        assert_eq!(
            sections(&render(&worktree())),
            ["Runtime", "Rules", "Workspace", "Git"]
        );
        let joined: Vec<String> = Layer::ORDER
            .iter()
            .filter_map(|layer| layer.render(&worktree()))
            .collect();
        assert_eq!(render(&worktree()), joined.join("\n"));
    }

    #[test]
    fn reserved_layers_add_nothing() {
        let inputs = [
            worktree(),
            HarnessInput {
                cwd: "/tmp/notes".into(),
                git: None,
            },
        ];
        for input in &inputs {
            assert_eq!(Layer::ProjectInstructions.render(input), None);
            assert_eq!(Layer::Skills.render(input), None);
            let live: Vec<String> = [Layer::Runtime, Layer::Rules, Layer::Workspace, Layer::Git]
                .iter()
                .filter_map(|layer| layer.render(input))
                .collect();
            assert_eq!(render(input), live.join("\n"));
        }
    }

    #[test]
    fn host_facts_and_guidance_are_told_apart() {
        let host: Vec<Layer> = Layer::ORDER
            .into_iter()
            .filter(|layer| layer.authority() == Authority::Host)
            .collect();
        assert_eq!(host, [Layer::Runtime, Layer::Workspace]);
        // Project instructions and skills may refine guidance, never Host facts.
        assert_eq!(Layer::ProjectInstructions.authority(), Authority::Guidance);
        assert_eq!(Layer::Skills.authority(), Authority::Guidance);
    }

    #[test]
    fn the_default_branch_is_whatever_origin_names() {
        let text = render(&HarnessInput {
            git: Some(GitFacts {
                worktree: true,
                default_branch: Some("origin/master".into()),
                placeholder_branch: None,
            }),
            ..worktree()
        });
        assert!(text.contains("- Default branch: origin/master."));
        assert!(text.contains("merge origin/master into it"));
        assert!(text.contains("the `git_pr_create` tool, base `master`."));
        assert!(!text.contains("origin/main"));
    }

    #[test]
    fn a_root_checkout_has_no_worktree_lines() {
        let text = render(&HarnessInput {
            git: Some(GitFacts {
                worktree: false,
                default_branch: Some("origin/main".into()),
                placeholder_branch: None,
            }),
            ..worktree()
        });
        assert!(!text.contains("worktree"), "{text}");
        assert!(!text.contains("git stash"));
        assert!(text.contains("- Default branch: origin/main."));
    }

    #[test]
    fn without_a_remote_nothing_names_a_default_branch() {
        let text = render(&HarnessInput {
            git: Some(GitFacts {
                worktree: true,
                default_branch: None,
                placeholder_branch: None,
            }),
            ..worktree()
        });
        assert!(!text.contains("Default branch"));
        assert!(!text.contains("against origin/main"));
        assert!(text.contains("- Raising or opening a PR: commit pending changes if there are any, push with the `git_push` tool, then open the PR with the `git_pr_create` tool."));
        assert!(!text.contains("Updating a branch:"));
        assert!(!text.contains("New Zwiim worktrees"));
        assert!(text.contains("- Merging a PR: only when asked."));
        assert!(text.contains("- Conflicts: read both sides"));
    }

    #[test]
    fn outside_git_there_is_no_git_section() {
        let text = render(&HarnessInput {
            cwd: "/tmp/notes".into(),
            git: None,
        });
        assert!(text.contains("- Working directory: /tmp/notes."));
        assert!(text.contains("\nRules\n"));
        assert!(!text.contains("Git"), "{text}");
        assert_eq!(sections(&text), ["Runtime", "Rules", "Workspace"]);
    }

    #[test]
    fn a_renamed_branch_gets_no_placeholder_line() {
        let text = render(&HarnessInput {
            git: Some(GitFacts {
                placeholder_branch: None,
                ..worktree().git.unwrap()
            }),
            ..worktree()
        });
        assert!(!text.contains("placeholder"), "{text}");
        assert!(!text.contains("git_branch_rename"), "{text}");
    }

    #[test]
    fn no_current_branch_is_named() {
        let text = render(&worktree());
        assert!(!text.contains("on branch"), "{text}");
        assert!(!text.contains("current branch"), "{text}");
    }
}
