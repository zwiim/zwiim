//! The Agent Harness: what Zwiim tells an agent. Provider-neutral. How each
//! agent receives it is provider delivery's job, in the agent connection
//! (`acp::connection`): Claude through its session requests' system prompt,
//! Codex through its developer instructions.
//!
//! Every agent connection, whoever opens it (a Zwiim session, Desktop, a
//! delegated child) and however its session opens (new, load, resume, fork),
//! passes through the same stages:
//!
//! 1. [`gather`]: the Host facts the instructions need, read once per
//!    connection.
//! 2. [`instruct`]: the session's [`SessionInstructions`], immutable for the
//!    connection's life.
//! 3. [`compose_turn`]: what Zwiim adds to each user turn on its way to the
//!    agent: one-shot Host notices, such as [`continuation_notice`]. History
//!    readers take them back out with [`strip_notices`].
//! 4. `observe`: reserved for reacting to turn outcomes. Not built yet.
//!
//! Instructions only guide the agent. What the Host enforces (the working
//! directory and filesystem write roots, permission requests reaching the
//! user, one turn at a time, cancel and stop, secrets kept out of SQLite and
//! logs) is enforced in Host code, so no instruction layer can relax it.

mod system_prompt;

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::acp::types::PromptInputBlock;

pub use system_prompt::{Authority, Layer};

/// The version of the instruction text. Bump it whenever the text rendered
/// for the same facts changes.
pub const INSTRUCTIONS_VERSION: &str = "zwiim-instructions/3";

/// Git facts about a session's working directory. The current branch is
/// left out on purpose, except the placeholder a Host-made worktree starts
/// on: the agent can switch branches mid-session, so it asks Git instead.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitFacts {
    /// The working directory is a linked worktree, not a main checkout.
    pub worktree: bool,
    /// The origin's default branch, e.g. `origin/main`.
    pub default_branch: Option<String>,
    /// The worktree is still on the placeholder branch the Host made it
    /// with, its folder's name (`pune`); `None` once renamed or pushed, and
    /// for any other checkout.
    pub placeholder_branch: Option<String>,
}

/// What the harness knows about a session when it opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessInput {
    pub cwd: PathBuf,
    /// `None` outside a Git checkout.
    pub git: Option<GitFacts>,
}

/// The instructions an agent session runs with, and their identity for logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInstructions {
    pub text: String,
    pub version: &'static str,
    /// Short SHA-256 of `text`.
    pub digest: String,
}

/// Stage 1: read the Host facts for a session working in `cwd`.
pub async fn gather(cwd: &Path) -> HarnessInput {
    HarnessInput {
        cwd: cwd.to_path_buf(),
        git: git_facts(cwd).await,
    }
}

async fn git_facts(cwd: &Path) -> Option<GitFacts> {
    let out = crate::process::tokio_command("git")
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ])
        .current_dir(cwd)
        .output()
        .await
        .ok()
        .filter(|out| out.status.success())?;
    let dirs = String::from_utf8_lossy(&out.stdout);
    let mut dirs = dirs.lines();
    let worktree = matches!((dirs.next(), dirs.next()), (Some(a), Some(b)) if a != b);
    let default_branch = crate::workspace::origin_default_branch(&cwd.to_string_lossy()).await;
    let placeholder_branch = if worktree {
        placeholder_branch(cwd).await
    } else {
        None
    };
    Some(GitFacts {
        worktree,
        default_branch,
        placeholder_branch,
    })
}

/// The branch `cwd` is on, when that is the name the Host gave its folder
/// and it was never pushed.
async fn placeholder_branch(cwd: &Path) -> Option<String> {
    let folder = cwd.file_name()?.to_str()?;
    if !crate::workspace::is_host_worktree_name(folder) {
        return None;
    }
    let out = crate::process::tokio_command("git")
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .current_dir(cwd)
        .output()
        .await
        .ok()
        .filter(|out| out.status.success())?;
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if branch != folder {
        return None;
    }
    // Pushed, as `git_branch_rename` judges it: renaming would orphan the
    // remote branch and its pull request.
    let origin_ref = format!("refs/remotes/origin/{branch}");
    let merge_key = format!("branch.{branch}.merge");
    for args in [
        &["show-ref", "--verify", "--quiet", origin_ref.as_str()][..],
        &["config", "--get", merge_key.as_str()],
    ] {
        let found = crate::process::tokio_command("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .map_or(true, |out| out.status.success());
        if found {
            return None;
        }
    }
    Some(branch)
}

/// Stage 2: the session's instructions.
pub fn instruct(input: &HarnessInput) -> SessionInstructions {
    let text = system_prompt::render(input);
    SessionInstructions {
        digest: digest(&text),
        version: INSTRUCTIONS_VERSION,
        text,
    }
}

fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())[..6]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Opens and closes a notice in a turn. Reserved: it is scrubbed from every
/// prompt at ingress, so a framed notice in a turn is always Zwiim's.
const NOTICE_SEPARATOR: char = '\u{1e}';
const NOTICE_TAG: &str = "[zwiim-notice]";

/// Stage 3: the user's turn as the agent receives it. Host notices queued for
/// the session ride on the turn's last text block, or on a new one when the
/// turn has none. Without notices the turn passes through unchanged.
pub fn compose_turn(
    mut blocks: Vec<PromptInputBlock>,
    notices: &[String],
) -> Vec<PromptInputBlock> {
    if notices.is_empty() {
        return blocks;
    }
    let framed = notices
        .iter()
        .map(|text| format!("{NOTICE_SEPARATOR}{NOTICE_TAG}\n{text}{NOTICE_SEPARATOR}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    let last_text = blocks.iter_mut().rev().find_map(|block| match block {
        PromptInputBlock::Text { text } => Some(text),
        _ => None,
    });
    match last_text {
        Some(text) => {
            text.push_str("\n\n");
            text.push_str(&framed);
        }
        None => blocks.push(PromptInputBlock::Text { text: framed }),
    }
    blocks
}

/// Whether a turn may carry notices. A slash command goes to the agent's CLI
/// as typed, so notices wait for the next turn. Any turn whose first text
/// starts with `/` counts: deferring a notice costs nothing.
pub fn takes_notices(blocks: &[PromptInputBlock]) -> bool {
    let first_text = blocks.iter().find_map(|block| match block {
        PromptInputBlock::Text { text } => Some(text),
        _ => None,
    });
    !first_text.is_some_and(|text| text.trim_start().starts_with('/'))
}

/// Remove every notice, and the whitespace before it, from `text` as an agent
/// recorded it. True if one was removed.
pub fn strip_notices(text: &mut String) -> bool {
    if !text.contains(NOTICE_SEPARATOR) {
        return false;
    }
    let open = format!("{NOTICE_SEPARATOR}{NOTICE_TAG}");
    let mut removed = false;
    while let Some(start) = text.find(&open) {
        let body = start + open.len();
        let Some(len) = text[body..].find(NOTICE_SEPARATOR) else {
            break;
        };
        let end = body + len + NOTICE_SEPARATOR.len_utf8();
        let from = text[..start].trim_end().len();
        text.replace_range(from..end, "");
        removed = true;
    }
    removed
}

/// What an agent is told after its workspace continued past merged pull
/// request `number` onto `branch`, cut from `base_branch` at `base_commit`.
pub fn continuation_notice(
    number: u64,
    base_branch: &str,
    branch: &str,
    previous: &str,
    base_commit: &str,
) -> String {
    let short = base_commit.get(..12).unwrap_or(base_commit);
    format!(
        "Zwiim: pull request #{number} was merged. This workspace now works on a new branch \
         `{branch}`, started from `{base_branch}` at {short}. `{branch}` is a placeholder: \
         rename it with `git_branch_rename` before you push. The previous branch `{previous}` \
         is kept. Earlier changes are part of the merged pull request; build on the new base."
    )
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn instructions_carry_their_version_and_digest() {
        let made = instruct(&worktree());
        assert_eq!(made.version, INSTRUCTIONS_VERSION);
        assert_eq!(made.text, system_prompt::render(&worktree()));
        assert_eq!(made, instruct(&worktree()), "deterministic");
        assert_eq!(made.digest.len(), 12);
        assert!(made.digest.bytes().all(|b| b.is_ascii_hexdigit()));
        let other = instruct(&HarnessInput {
            cwd: "/w/repo/def".into(),
            ..worktree()
        });
        assert_ne!(made.digest, other.digest);
    }

    /// Pins the text for one input. If this fails because the text changed
    /// on purpose, bump `INSTRUCTIONS_VERSION` and update the digest.
    #[test]
    fn the_version_names_the_text() {
        assert_eq!(
            (
                instruct(&worktree()).version,
                instruct(&worktree()).digest.as_str()
            ),
            ("zwiim-instructions/3", "3229a2a01c2d")
        );
    }

    #[test]
    fn compose_turn_passes_the_turn_through() {
        let blocks = vec![
            PromptInputBlock::Text {
                text: "hello".into(),
            },
            PromptInputBlock::Image {
                data: "AAAA".into(),
                mime_type: "image/png".into(),
                uri: None,
            },
        ];
        assert_eq!(compose_turn(blocks.clone(), &[]), blocks);
        assert!(compose_turn(Vec::new(), &[]).is_empty());
    }

    #[test]
    fn notices_ride_on_the_last_text_block_and_strip_back_out() {
        let blocks = vec![PromptInputBlock::Text {
            text: "fix it".into(),
        }];
        let notice = continuation_notice(
            12,
            "origin/main",
            "zwiim/abcd1234",
            "zwiim/old",
            "0123456789ab",
        );
        let sent = compose_turn(blocks.clone(), std::slice::from_ref(&notice));
        assert_eq!(sent.len(), 1);
        let PromptInputBlock::Text { text } = &sent[0] else {
            panic!("a text block");
        };
        assert!(text.starts_with("fix it") && text.contains(&notice));
        let mut back = text.clone();
        assert!(strip_notices(&mut back));
        assert_eq!(back.trim_end(), "fix it");
        let mut plain = "fix it".to_string();
        assert!(!strip_notices(&mut plain));
        assert_eq!(compose_turn(blocks.clone(), &[]), blocks);

        // No text block to ride on: the notice becomes its own.
        let image = vec![PromptInputBlock::Image {
            data: "AAAA".into(),
            mime_type: "image/png".into(),
            uri: None,
        }];
        let sent = compose_turn(image.clone(), &["n".into(), "m".into()]);
        assert_eq!(sent[..1], image[..]);
        let PromptInputBlock::Text { text } = &sent[1] else {
            panic!("a text block");
        };
        let mut back = text.clone();
        assert!(strip_notices(&mut back));
        assert!(back.is_empty(), "both notices go: {back:?}");
    }

    #[test]
    fn a_slash_command_turn_takes_no_notice() {
        assert!(!takes_notices(&[PromptInputBlock::Text {
            text: " /review".into()
        }]));
        assert!(takes_notices(&[PromptInputBlock::Text {
            text: "review".into()
        }]));
    }

    #[test]
    fn a_slash_command_after_an_image_takes_no_notice() {
        let image = PromptInputBlock::Image {
            data: "AAAA".into(),
            mime_type: "image/png".into(),
            uri: None,
        };
        assert!(!takes_notices(&[
            image.clone(),
            PromptInputBlock::Text {
                text: "/review".into()
            },
        ]));
        assert!(takes_notices(&[image]));
    }

    #[test]
    fn the_notice_says_what_happened() {
        let n = continuation_notice(
            12,
            "origin/main",
            "zwiim/abcd1234",
            "zwiim/old",
            "0123456789abcdef",
        );
        for part in [
            "#12",
            "merged",
            "origin/main",
            "zwiim/abcd1234",
            "zwiim/old",
            "0123456789ab",
            "placeholder",
            "git_branch_rename",
        ] {
            assert!(n.contains(part), "{part}");
        }
        assert!(!n.contains("0123456789abc"), "short commit only");
    }

    /// The harness decides what to say, never how a provider receives it.
    #[test]
    fn no_provider_or_transport_types_in_the_harness() {
        let sources = [
            include_str!("harness.rs"),
            include_str!("harness/system_prompt.rs"),
        ];
        for source in sources {
            let code = source.split("#[cfg(test)]").next().unwrap();
            for banned in [
                "sacp",
                "ContentBlock",
                "AgentType",
                "crate::acp::connection",
                "CODEX_CONFIG",
                "_meta",
            ] {
                assert!(!code.contains(banned), "harness mentions {banned}");
            }
        }
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
    async fn gather_reads_the_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
        let plain = dir.path().join("plain");
        std::fs::create_dir(&plain).unwrap();

        let outside = gather(&plain).await;
        assert_eq!(outside.cwd, plain);
        assert_eq!(outside.git, None);
        assert_eq!(
            gather(&repo).await.git,
            Some(GitFacts {
                worktree: false,
                default_branch: None,
                placeholder_branch: None,
            })
        );

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
        let input = gather(&wt).await;
        assert_eq!(
            input.git,
            Some(GitFacts {
                worktree: true,
                default_branch: Some("origin/main".into()),
                placeholder_branch: None,
            })
        );
        // The branch the worktree is on is a Git fact the agent asks for.
        assert!(!instruct(&input).text.contains("zwiim/abc"));

        // A Host-made worktree, still on the branch its folder was named with.
        let pune = dir.path().join("pune");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "pune",
                pune.to_str().unwrap(),
            ],
        );
        let input = gather(&pune).await;
        assert_eq!(
            input.git.as_ref().unwrap().placeholder_branch.as_deref(),
            Some("pune")
        );
        assert!(instruct(&input).text.contains("placeholder branch `pune`"));
        // Renamed: no longer a placeholder.
        run(&pune, &["branch", "-m", "fix-login"]);
        assert_eq!(gather(&pune).await.git.unwrap().placeholder_branch, None);
        // A main checkout named and branched after a city is not a worktree.
        let agra = dir.path().join("agra");
        std::fs::create_dir(&agra).unwrap();
        run(&agra, &["init", "-q", "-b", "agra"]);
        run(&agra, &["commit", "-q", "--allow-empty", "-m", "base"]);
        assert_eq!(gather(&agra).await.git.unwrap().placeholder_branch, None);
    }

    #[tokio::test]
    async fn a_pushed_city_branch_is_no_placeholder() {
        // Worktrees made before placeholders pushed under their city name;
        // renaming such a branch would orphan its pull request.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
        for city in ["pune", "surat"] {
            let path = dir.path().join(city);
            run(
                &repo,
                &["worktree", "add", "-q", "-b", city, path.to_str().unwrap()],
            );
        }

        // On the origin.
        run(&repo, &["update-ref", "refs/remotes/origin/pune", "HEAD"]);
        assert_eq!(
            gather(&dir.path().join("pune"))
                .await
                .git
                .unwrap()
                .placeholder_branch,
            None
        );
        // Tracking a remote branch.
        run(&repo, &["config", "branch.surat.merge", "refs/heads/surat"]);
        assert_eq!(
            gather(&dir.path().join("surat"))
                .await
                .git
                .unwrap()
                .placeholder_branch,
            None
        );
    }
}
