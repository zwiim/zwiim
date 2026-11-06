//! Zwiim-owned: the earlier-chat summary a "Retry in new chat" sends along.
//!
//! When the agent couldn't reach its server, the new chat starts with the
//! failed message plus a Markdown file of the old chat. The file is built
//! mechanically from the old session's history (no agent, since the agent
//! can't reach its server): each turn's user message and the agent's final
//! reply, with everything in between collapsed to a count, capped per
//! message and per file.
//!
//! A stopped plan continued in a new chat gets the same kind of file
//! ([`SummaryKind::PlanStopped`]), but there the agent works fine, so each
//! tool call is listed instead of counted. So is a fork of a session whose
//! agent cannot fork ([`SummaryKind::Forked`]), cut at the forked turn.

use crate::acp::types::PromptInputBlock;
use crate::attachments::split_note;
use crate::models::message::{ContentBlock, MessageTurn, TurnRole};

/// The summary's file name, as the new chat's attachment.
pub const SUMMARY_FILE: &str = "earlier-chat-summary.md";

/// The file name of the old chat when a stopped plan is continued, as the
/// new chat's attachment.
pub const PREVIOUS_SESSION_FILE: &str = "previous-session.md";

const TITLE: &str = "# Earlier chat";
const INTRO: &str = "This chat continues an earlier one that stopped because the agent couldn't reach its server. The last message below is the one that failed; it is repeated in the new chat.";
const PLAN_STOPPED_INTRO: &str = "This chat continues an earlier one whose tasks stopped before they were finished. The tasks are in tasks.md.";
const FORKED_INTRO: &str = "This chat branches off an earlier chat. Below is that chat up to the point it was forked; anything that happened after that point is not part of this chat.";
const OMITTED: &str = "_(earlier turns omitted)_";
const CUT: &str = "… [cut]";
/// Characters kept of any one message.
const MESSAGE_MAX_CHARS: usize = 10_000;
/// Characters the whole file may hold.
const FILE_MAX_CHARS: usize = 200_000;

/// Why the old chat is being summarized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryKind {
    /// The agent couldn't reach its server; the failed message is repeated.
    Unreachable,
    /// A plan stopped before it was finished; every tool call is listed.
    PlanStopped,
    /// A session was forked here; every tool call is listed.
    Forked,
}

/// One turn of the old chat: what the user sent and what the agent did.
#[derive(Debug, Clone, Default)]
pub struct SummaryTurn {
    pub user: Vec<ContentBlock>,
    pub agent: Vec<ContentBlock>,
    /// The error the turn ended with, if any.
    pub error: Option<String>,
    /// Whether the user stopped the turn.
    pub stopped: bool,
}

/// `turns` grouped into user turns, each with the agent turns that answered
/// it. History keeps no stop or error marks, so none are set.
pub fn turns_from_history(turns: &[MessageTurn]) -> Vec<SummaryTurn> {
    let mut out: Vec<SummaryTurn> = Vec::new();
    for turn in turns {
        match turn.role {
            TurnRole::User => out.push(SummaryTurn {
                user: turn.blocks.clone(),
                ..Default::default()
            }),
            TurnRole::Assistant => match out.last_mut() {
                Some(last) => last.agent.extend(turn.blocks.iter().cloned()),
                None => out.push(SummaryTurn {
                    agent: turn.blocks.clone(),
                    ..Default::default()
                }),
            },
            TurnRole::System => {}
        }
    }
    out
}

/// `history` without its trailing turns that are the `failed` message: some
/// agents write the user message to their transcript before failing to send
/// it, once per failed try. Marks and system turns after it go too.
pub fn without_failed_turns<'a>(
    history: &'a [MessageTurn],
    failed: &[PromptInputBlock],
) -> &'a [MessageTurn] {
    let failed_text = failed_user_text(failed);
    let failed_message = render_failed(failed).into_iter().next();
    let mut turns = history;
    while let Some((last, earlier)) = turns.split_last() {
        let is_failed = match last.role {
            TurnRole::System => true,
            TurnRole::User => {
                user_text(&last.blocks) == failed_text
                    && (!failed_text.is_empty() || render_user(&last.blocks) == failed_message)
            }
            TurnRole::Assistant => false,
        };
        if !is_failed {
            break;
        }
        turns = earlier;
    }
    turns
}

/// `history` up to and including `turn_id`, or `None` when it isn't there.
pub fn through_turn<'a>(history: &'a [MessageTurn], turn_id: &str) -> Option<&'a [MessageTurn]> {
    let at = history.iter().position(|t| t.id == turn_id)?;
    Some(&history[..=at])
}

/// The last non-empty text of the last agent turn in `history`, trimmed.
pub fn last_reply(history: &[MessageTurn]) -> Option<String> {
    let turn = history
        .iter()
        .rev()
        .find(|t| matches!(t.role, TurnRole::Assistant))?;
    turn.blocks.iter().rev().find_map(|b| match b {
        ContentBlock::Text { text } if !text.trim().is_empty() => Some(text.trim().to_string()),
        _ => None,
    })
}

/// The summary file of `turns`, by `agent` (its display name), ending with
/// the `failed` message when there is one.
pub fn build(
    turns: &[SummaryTurn],
    agent: &str,
    failed: &[PromptInputBlock],
    kind: SummaryKind,
) -> String {
    let failed_text = failed_user_text(failed);
    let failed_message = render_failed(failed).into_iter().next();
    let mut turns = turns;
    // Some agents write the user message to their transcript before failing
    // to send it, once per failed try: show it once, as the failed message.
    while let Some((last, earlier)) = turns.split_last() {
        // With no typed text to match (an image-only message), the whole
        // rendered message must match, or any unanswered empty-text turn would.
        let same_message = user_text(&last.user) == failed_text
            && (!failed_text.is_empty() || render_user(&last.user) == failed_message);
        if kind != SummaryKind::Unreachable || !(agent_lines(last, kind).is_empty() && same_message)
        {
            break;
        }
        turns = earlier;
    }

    let header = match kind {
        SummaryKind::Unreachable => format!("{TITLE}\n{INTRO}"),
        SummaryKind::PlanStopped => format!("{TITLE}\n\n{PLAN_STOPPED_INTRO}"),
        SummaryKind::Forked => format!("{TITLE}\n\n{FORKED_INTRO}"),
    };
    // A stopped plan or a fork has no failed message to repeat.
    let failed_section = (kind == SummaryKind::Unreachable)
        .then(|| section("## You (failed to send)", &render_failed(failed)));
    let sections: Vec<String> = turns.iter().map(|t| render_turn(t, agent, kind)).collect();

    let mut parts = vec![header];
    // Header, the failed section with its separator, and the final newline.
    let fixed = chars(&parts[0]) + failed_section.as_ref().map_or(0, |f| chars(f) + 2) + 1;
    let total: usize = sections.iter().map(|s| chars(s) + 2).sum();
    if fixed + total <= FILE_MAX_CHARS || sections.len() <= 1 {
        parts.extend(sections);
    } else {
        // Keep the first turn and as many of the newest as fit, marking the gap.
        let first = &sections[0];
        let mut budget =
            FILE_MAX_CHARS.saturating_sub(fixed + chars(first) + 2 + chars(OMITTED) + 2);
        let mut tail = Vec::new();
        for s in sections[1..].iter().rev() {
            let cost = chars(s) + 2;
            if cost > budget {
                break;
            }
            budget -= cost;
            tail.push(s.clone());
        }
        tail.reverse();
        parts.push(first.clone());
        parts.push(OMITTED.to_string());
        parts.extend(tail);
    }
    parts.extend(failed_section);
    let mut out = parts.join("\n\n");
    out.push('\n');
    out
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// `text`, cut to the per-message cap.
pub(crate) fn capped(text: &str) -> String {
    if chars(text) <= MESSAGE_MAX_CHARS {
        return text.to_string();
    }
    let kept: String = text.chars().take(MESSAGE_MAX_CHARS).collect();
    format!("{kept}{CUT}")
}

fn section(heading: &str, lines: &[String]) -> String {
    let mut out = heading.to_string();
    for line in lines {
        out.push_str("\n\n");
        out.push_str(line);
    }
    out
}

/// An attachment's name as the user knows it: notes carry it percent-encoded.
/// A name that does not decode to UTF-8 stays as stored.
fn display_name(name: &str) -> String {
    percent_encoding::percent_decode_str(name)
        .decode_utf8()
        .map_or_else(|_| name.to_string(), |decoded| decoded.into_owned())
}

/// User text with its attachment note taken out, and the files it named.
fn text_parts(text: &str) -> (String, Vec<String>) {
    match split_note(text, None) {
        Some(split) => {
            let files = split
                .attachments
                .iter()
                .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                .map(|n| format!("[file: {}]", display_name(n)))
                .collect();
            (split.text, files)
        }
        None => (text.trim().to_string(), Vec::new()),
    }
}

/// The text the user typed, notes and attachments left out.
fn user_text(blocks: &[ContentBlock]) -> String {
    let texts: Vec<String> = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text_parts(text).0),
            _ => None,
        })
        .filter(|t| !t.is_empty())
        .collect();
    texts.join("\n\n")
}

fn failed_user_text(blocks: &[PromptInputBlock]) -> String {
    let texts: Vec<String> = blocks
        .iter()
        .filter_map(|b| match b {
            PromptInputBlock::Text { text } => Some(text_parts(text).0),
            _ => None,
        })
        .filter(|t| !t.is_empty())
        .collect();
    texts.join("\n\n")
}

/// A user message as one capped message: its text, then its attachments by name.
fn user_message(text: String, files: Vec<String>) -> Option<String> {
    let mut parts = Vec::new();
    if !text.is_empty() {
        parts.push(text);
    }
    parts.extend(files);
    (!parts.is_empty()).then(|| capped(&parts.join("\n\n")))
}

fn render_user(blocks: &[ContentBlock]) -> Option<String> {
    let mut texts = Vec::new();
    let mut files = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text } => {
                let (t, f) = text_parts(text);
                if !t.is_empty() {
                    texts.push(t);
                }
                files.extend(f);
            }
            ContentBlock::Image { .. } => files.push("[image]".to_string()),
            _ => {}
        }
    }
    user_message(texts.join("\n\n"), files)
}

fn render_failed(blocks: &[PromptInputBlock]) -> Vec<String> {
    let mut texts = Vec::new();
    let mut files = Vec::new();
    for block in blocks {
        match block {
            PromptInputBlock::Text { text } => {
                let (t, f) = text_parts(text);
                if !t.is_empty() {
                    texts.push(t);
                }
                files.extend(f);
            }
            PromptInputBlock::Image { .. } => files.push("[image]".to_string()),
            PromptInputBlock::ResourceLink { name, .. } => {
                files.push(format!("[file: {}]", display_name(name)))
            }
            PromptInputBlock::Resource { uri, .. } => {
                let name = uri.rsplit('/').next().unwrap_or(uri);
                files.push(format!("[file: {}]", display_name(name)));
            }
        }
    }
    user_message(texts.join("\n\n"), files)
        .into_iter()
        .collect()
}

/// The agent's key messages of `turn`. `Unreachable` keeps the elided marker,
/// the final reply (or `[Stopped]`) and the error it ended with. `PlanStopped`
/// and `Forked` keep every reply and tool call in order, then `[Stopped]` and
/// the error.
fn agent_lines(turn: &SummaryTurn, kind: SummaryKind) -> Vec<String> {
    match kind {
        SummaryKind::Unreachable => unreachable_agent_lines(turn),
        SummaryKind::PlanStopped | SummaryKind::Forked => plan_stopped_agent_lines(turn),
    }
}

fn unreachable_agent_lines(turn: &SummaryTurn) -> Vec<String> {
    // Assistant messages and tool calls count; thinking and results don't.
    let counted: Vec<Option<&str>> = turn
        .agent
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } if !text.trim().is_empty() => Some(Some(text.as_str())),
            ContentBlock::ToolUse { .. } | ContentBlock::ImageGeneration { .. } => Some(None),
            _ => None,
        })
        .collect();
    let reply = if turn.stopped {
        None
    } else {
        counted.iter().rev().find_map(|t| *t)
    };
    let elided = counted.len() - usize::from(reply.is_some());

    let mut lines = Vec::new();
    match elided {
        0 => {}
        1 => lines.push("[1 message elided]".to_string()),
        n => lines.push(format!("[{n} messages elided]")),
    }
    if turn.stopped {
        lines.push("[Stopped]".to_string());
    } else if let Some(reply) = reply {
        lines.push(capped(reply.trim()));
    }
    if let Some(error) = &turn.error {
        // Cap the error first so the closing bracket survives.
        lines.push(format!("[Error: {}]", capped(error)));
    }
    lines
}

/// ImageGeneration blocks are left out on purpose.
fn plan_stopped_agent_lines(turn: &SummaryTurn) -> Vec<String> {
    let mut lines: Vec<String> = turn
        .agent
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } if !text.trim().is_empty() => Some(capped(text.trim())),
            ContentBlock::ToolUse {
                tool_name,
                input_preview,
                ..
            } => Some(capped(&match input_preview.as_deref().map(str::trim) {
                Some(preview) if !preview.is_empty() => format!("{tool_name} {preview}"),
                _ => tool_name.clone(),
            })),
            _ => None,
        })
        .collect();
    if turn.stopped {
        lines.push("[Stopped]".to_string());
    }
    if let Some(error) = &turn.error {
        lines.push(format!("[Error: {}]", capped(error)));
    }
    lines
}

fn render_turn(turn: &SummaryTurn, agent: &str, kind: SummaryKind) -> String {
    // A fork's transcript is read as a chat, not as this agent's own past.
    let (user_heading, agent_heading) = match kind {
        SummaryKind::Forked => ("## User".to_string(), "## Assistant".to_string()),
        _ => ("## You".to_string(), format!("## {agent}")),
    };
    let mut sections = Vec::new();
    if let Some(user) = render_user(&turn.user) {
        sections.push(section(&user_heading, &[user]));
    }
    let lines = agent_lines(turn, kind);
    if !lines.is_empty() {
        sections.push(section(&agent_heading, &lines));
    }
    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachments::note;
    use std::path::PathBuf;

    const AGENT: &str = "Codex CLI";

    fn text(t: &str) -> ContentBlock {
        ContentBlock::Text { text: t.into() }
    }

    fn tool_use() -> ContentBlock {
        ContentBlock::ToolUse {
            tool_use_id: Some("t1".into()),
            tool_name: "Bash".into(),
            input_preview: Some("ls".into()),
            status: None,
            meta: None,
        }
    }

    fn tool_result() -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: Some("t1".into()),
            output_preview: Some("a.txt".into()),
            is_error: false,
            agent_stats: None,
            images: Vec::new(),
        }
    }

    fn thinking() -> ContentBlock {
        ContentBlock::Thinking { text: "hmm".into() }
    }

    fn turn(user: &str, agent: Vec<ContentBlock>) -> SummaryTurn {
        SummaryTurn {
            user: vec![text(user)],
            agent,
            ..Default::default()
        }
    }

    fn failed(t: &str) -> Vec<PromptInputBlock> {
        vec![PromptInputBlock::Text { text: t.into() }]
    }

    #[test]
    fn starts_with_title_and_intro() {
        let out = build(
            &[turn("hi", vec![text("hello")])],
            AGENT,
            &failed("again"),
            SummaryKind::Unreachable,
        );
        let mut lines = out.lines();
        assert_eq!(lines.next(), Some("# Earlier chat"));
        assert_eq!(
            lines.next(),
            Some(
                "This chat continues an earlier one that stopped because the agent couldn't reach its server. The last message below is the one that failed; it is repeated in the new chat."
            )
        );
        assert!(out.contains("## You\n\nhi"));
        assert!(out.contains("## Codex CLI\n\nhello"));
    }

    #[test]
    fn keeps_final_reply_and_counts_elided() {
        let busy = turn(
            "go",
            vec![
                text("a"),
                tool_use(),
                text("b"),
                thinking(),
                tool_result(),
                text("c"),
            ],
        );
        let out = build(&[busy], AGENT, &failed("next"), SummaryKind::Unreachable);
        assert!(
            out.contains("## Codex CLI\n\n[3 messages elided]\n\nc"),
            "{out}"
        );
        assert!(!out.contains("\n\na\n"));
        assert!(!out.contains("\n\nb\n"));

        let single = build(
            &[turn("go", vec![text("only")])],
            AGENT,
            &failed("next"),
            SummaryKind::Unreachable,
        );
        assert!(single.contains("## Codex CLI\n\nonly"));
        assert!(!single.contains("elided"));

        let two = build(
            &[turn("go", vec![tool_use(), text("done")])],
            AGENT,
            &failed("next"),
            SummaryKind::Unreachable,
        );
        assert!(two.contains("[1 message elided]\n\ndone"), "{two}");
    }

    #[test]
    fn cuts_long_message() {
        let long = "x".repeat(10_001);
        let out = build(
            &[turn("go", vec![text(&long)])],
            AGENT,
            &failed("next"),
            SummaryKind::Unreachable,
        );
        assert!(out.contains(&format!("{}… [cut]", "x".repeat(10_000))));
        assert!(!out.contains(&"x".repeat(10_001)));
    }

    #[test]
    fn stopped_turn_shows_stopped() {
        let mut stopped = turn("go", vec![tool_use(), text("partial reply")]);
        stopped.stopped = true;
        let out = build(&[stopped], AGENT, &failed("next"), SummaryKind::Unreachable);
        assert!(out.contains("## You\n\ngo"));
        assert!(
            out.contains("## Codex CLI\n\n[2 messages elided]\n\n[Stopped]"),
            "{out}"
        );
        assert!(!out.contains("partial reply"));
    }

    #[test]
    fn turn_error_is_kept() {
        let mut errored = turn("go", vec![text("reply")]);
        errored.error = Some("x".into());
        let out = build(&[errored], AGENT, &failed("next"), SummaryKind::Unreachable);
        assert!(out.contains("reply\n\n[Error: x]"), "{out}");
    }

    #[test]
    fn failed_turn_is_last_and_marked() {
        let out = build(
            &[turn("hi", vec![text("hello")])],
            AGENT,
            &failed("please retry"),
            SummaryKind::Unreachable,
        );
        assert!(
            out.trim_end()
                .ends_with("## You (failed to send)\n\nplease retry"),
            "{out}"
        );
        assert_eq!(out.matches("please retry").count(), 1);
    }

    #[test]
    fn failed_message_already_in_history_is_not_repeated() {
        let turns = vec![turn("hi", vec![text("hello")]), turn("fix the bug", vec![])];
        let out = build(
            &turns,
            AGENT,
            &failed("fix the bug"),
            SummaryKind::Unreachable,
        );
        assert_eq!(out.matches("fix the bug").count(), 1, "{out}");
        assert!(out.contains("## You (failed to send)\n\nfix the bug"));
        assert!(out.contains("## You\n\nhi"));

        // An earlier turn with the same text that the agent answered stays.
        let answered = vec![turn("fix the bug", vec![text("done")])];
        let out = build(
            &answered,
            AGENT,
            &failed("fix the bug"),
            SummaryKind::Unreachable,
        );
        assert_eq!(out.matches("fix the bug").count(), 2);
    }

    #[test]
    fn image_only_failed_message_keeps_other_unanswered_turns() {
        let image = || ContentBlock::Image {
            data: "aGk=".into(),
            mime_type: "image/png".into(),
            uri: None,
        };
        let path = PathBuf::from("/r/.zwiim/attachments/k3F9a2Qx/a.txt");
        let file_only = SummaryTurn {
            user: vec![text(&note(&[path]))],
            ..Default::default()
        };
        let copy = SummaryTurn {
            user: vec![image()],
            ..Default::default()
        };
        let turns = vec![turn("hi", vec![text("hello")]), file_only, copy];
        let failed = vec![PromptInputBlock::Image {
            data: "aGk=".into(),
            mime_type: "image/png".into(),
            uri: None,
        }];
        let out = build(&turns, AGENT, &failed, SummaryKind::Unreachable);
        assert!(out.contains("[file: a.txt]"), "{out}");
        assert_eq!(out.matches("[image]").count(), 1, "{out}");
        assert!(out.contains("## You (failed to send)\n\n[image]"), "{out}");
    }

    #[test]
    fn file_only_failed_message_is_shown_once() {
        // The transcript's copy of a file-only message is its attachment note.
        let path = PathBuf::from("/r/.zwiim/attachments/k3F9a2Qx/a.txt");
        let copy = SummaryTurn {
            user: vec![text(&note(&[path]))],
            ..Default::default()
        };
        let turns = vec![turn("hi", vec![text("hello")]), copy];
        let failed = vec![PromptInputBlock::ResourceLink {
            uri: "file:///r/.zwiim/attachments/k3F9a2Qx/a.txt".into(),
            name: "a.txt".into(),
            mime_type: None,
            description: None,
        }];
        let out = build(&turns, AGENT, &failed, SummaryKind::Unreachable);
        assert_eq!(out.matches("[file: a.txt]").count(), 1, "{out}");
    }

    #[test]
    fn failed_resource_names_show_decoded() {
        let failed = vec![
            PromptInputBlock::ResourceLink {
                uri: "file:///r/my%20file.txt".into(),
                name: "my%20file%C3%A9.txt".into(),
                mime_type: None,
                description: None,
            },
            PromptInputBlock::Resource {
                uri: "file:///r/other%20one.txt".into(),
                mime_type: None,
                text: None,
                blob: None,
            },
        ];
        let out = build(&[], AGENT, &failed, SummaryKind::Unreachable);
        assert!(out.contains("[file: my fileé.txt]"), "{out}");
        assert!(out.contains("[file: other one.txt]"), "{out}");
    }

    #[test]
    fn long_error_keeps_its_closing_bracket() {
        let mut errored = turn("go", vec![text("reply")]);
        errored.error = Some("e".repeat(MESSAGE_MAX_CHARS + 50));
        let out = build(&[errored], AGENT, &failed("next"), SummaryKind::Unreachable);
        assert!(
            out.contains(&format!(
                "[Error: {}… [cut]]",
                "e".repeat(MESSAGE_MAX_CHARS)
            )),
            "{}",
            &out[out.len().saturating_sub(120)..]
        );
    }

    #[test]
    fn attachment_names_show_decoded() {
        let path = PathBuf::from("/r/.zwiim/attachments/k3F9a2Qx/my%20file%C3%A9.txt");
        let bad = PathBuf::from("/r/.zwiim/attachments/k3F9a2Qy/bad%FF.txt");
        let t = SummaryTurn {
            user: vec![text(&format!("look\n\n{}", note(&[path, bad])))],
            agent: vec![text("seen")],
            ..Default::default()
        };
        let out = build(&[t], AGENT, &failed("next"), SummaryKind::Unreachable);
        assert!(out.contains("[file: my fileé.txt]"), "{out}");
        assert!(out.contains("[file: bad%FF.txt]"), "{out}");
    }

    #[test]
    fn repeated_failed_message_in_history_is_shown_once() {
        // Retry in the same chat failed too, then Retry in new chat: history
        // ends with two unanswered copies of the failed message.
        let turns = vec![
            turn("hi", vec![text("hello")]),
            turn("fix the bug", vec![]),
            turn("fix the bug", vec![]),
        ];
        let out = build(
            &turns,
            AGENT,
            &failed("fix the bug"),
            SummaryKind::Unreachable,
        );
        assert_eq!(out.matches("fix the bug").count(), 1, "{out}");
        assert!(out.contains("## You (failed to send)\n\nfix the bug"));
        assert!(out.contains("## You\n\nhi"));
    }

    #[test]
    fn file_cap_counts_trailing_newline() {
        // Turns that fill the file to exactly the cap before the final
        // newline: the newline must not push it one past.
        let fail = failed("the failed one");
        let base = chars(&build(&[], AGENT, &fail, SummaryKind::Unreachable));
        let mut turns = Vec::new();
        let mut used = base;
        let mut i = 0;
        loop {
            i += 1;
            if FILE_MAX_CHARS + 1 - used <= MESSAGE_MAX_CHARS {
                // Tune the last turn so the untrimmed file is cap + 1.
                let q = format!("question {i}");
                let prefix = chars(&render_turn(
                    &turn(&q, vec![text("r")]),
                    AGENT,
                    SummaryKind::Unreachable,
                )) + 2
                    - 1;
                let k = FILE_MAX_CHARS + 1 - used - prefix;
                assert!(k <= MESSAGE_MAX_CHARS, "{k}");
                let last = turn(&q, vec![text(&"r".repeat(k))]);
                used += chars(&render_turn(&last, AGENT, SummaryKind::Unreachable)) + 2;
                turns.push(last);
                break;
            }
            let t = turn(&format!("question {i}"), vec![text(&"r".repeat(9_000))]);
            used += chars(&render_turn(&t, AGENT, SummaryKind::Unreachable)) + 2;
            turns.push(t);
        }
        assert_eq!(used, FILE_MAX_CHARS + 1);
        let out = build(&turns, AGENT, &fail, SummaryKind::Unreachable);
        assert!(
            out.chars().count() <= FILE_MAX_CHARS,
            "{}",
            out.chars().count()
        );
        assert!(out.ends_with("the failed one\n"));
    }

    #[test]
    fn file_cap_keeps_first_turn_and_marks_gap() {
        let turns: Vec<SummaryTurn> = (1..=50)
            .map(|i| turn(&format!("question {i}"), vec![text(&"r".repeat(9_000))]))
            .collect();
        let out = build(
            &turns,
            AGENT,
            &failed("the failed one"),
            SummaryKind::Unreachable,
        );
        assert!(out.chars().count() <= 200_000, "{}", out.chars().count());
        let first = out.find("question 1\n").expect("first turn kept");
        let gap = out.find("_(earlier turns omitted)_").expect("gap marked");
        let newest = out.find("question 50\n").expect("newest turn kept");
        let last = out.find("## You (failed to send)").expect("failed turn");
        assert!(first < gap && gap < newest && newest < last);
        assert!(!out.contains("question 2\n"));
    }

    #[test]
    fn earlier_attachments_listed_by_name() {
        let path = PathBuf::from("/r/.zwiim/attachments/k3F9a2Qx/a.txt");
        let user = vec![
            text(&format!("look at these\n\n{}", note(&[path]))),
            ContentBlock::Image {
                data: "aGk=".into(),
                mime_type: "image/png".into(),
                uri: None,
            },
        ];
        let t = SummaryTurn {
            user,
            agent: vec![text("seen")],
            ..Default::default()
        };
        let out = build(&[t], AGENT, &failed("next"), SummaryKind::Unreachable);
        assert!(out.contains("look at these"));
        assert!(out.contains("[file: a.txt]"), "{out}");
        assert!(out.contains("[image]"));
        assert!(!out.contains("zwiim-attachments"));
        assert!(!out.contains("/r/.zwiim"));
    }

    fn history_turn(role: TurnRole, blocks: Vec<ContentBlock>) -> MessageTurn {
        MessageTurn {
            id: "t".into(),
            role,
            blocks,
            timestamp: chrono::Utc::now(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
            agent_message_id: None,
        }
    }

    fn tool(name: &str, preview: Option<&str>) -> ContentBlock {
        ContentBlock::ToolUse {
            tool_use_id: None,
            tool_name: name.into(),
            input_preview: preview.map(Into::into),
            status: None,
            meta: None,
        }
    }

    fn plan_stopped(turns: &[SummaryTurn]) -> String {
        build(turns, AGENT, &[], SummaryKind::PlanStopped)
    }

    #[test]
    fn unreachable_kind_is_unchanged() {
        let busy = turn("go", vec![text("a"), tool_use(), text("c")]);
        let out = build(&[busy], AGENT, &failed("next"), SummaryKind::Unreachable);
        assert_eq!(
            out,
            "# Earlier chat\nThis chat continues an earlier one that stopped because the agent couldn't reach its server. The last message below is the one that failed; it is repeated in the new chat.\n\n## You\n\ngo\n\n## Codex CLI\n\n[2 messages elided]\n\nc\n\n## You (failed to send)\n\nnext\n"
        );
        // Empty `failed` still gets its (empty) section, as before.
        let out = build(&[], AGENT, &[], SummaryKind::Unreachable);
        assert!(out.ends_with("\n\n## You (failed to send)\n"), "{out}");
    }

    #[test]
    fn plan_stopped_has_its_own_intro() {
        let out = plan_stopped(&[turn("hi", vec![text("hello")])]);
        assert!(out.starts_with("# Earlier chat\n\n"), "{out}");
        assert!(out.contains(
            "This chat continues an earlier one whose tasks stopped before they were finished. The tasks are in tasks.md."
        ));
        assert!(!out.contains("couldn't reach its server"));
    }

    #[test]
    fn plan_stopped_lists_tool_calls() {
        let t = turn(
            "fix it",
            vec![
                tool("Edit", Some("transport/lan.ts")),
                tool("Bash", Some("npm test -- lan")),
                text("Fixing it now."),
            ],
        );
        let out = plan_stopped(&[t]);
        assert!(out.contains("Edit transport/lan.ts\n"), "{out}");
        assert!(out.contains("Bash npm test -- lan\n"), "{out}");
        assert!(out.contains("Fixing it now."), "{out}");
        assert!(!out.contains("elided"), "{out}");
    }

    #[test]
    fn plan_stopped_tool_without_preview() {
        let out = plan_stopped(&[turn("go", vec![tool("Read", None)])]);
        assert!(out.contains("## Codex CLI\n\nRead\n"), "{out}");
    }

    #[test]
    fn plan_stopped_keeps_replies_in_order_with_stop_and_error() {
        let mut t = turn(
            "go",
            vec![text("first"), tool("Bash", Some("ls")), text("second")],
        );
        t.stopped = true;
        t.error = Some("boom".into());
        let out = plan_stopped(&[t]);
        assert!(
            out.contains("first\n\nBash ls\n\nsecond\n\n[Stopped]\n\n[Error: boom]"),
            "{out}"
        );
    }

    #[test]
    fn plan_stopped_caps_a_tool_line() {
        let long = "x".repeat(MESSAGE_MAX_CHARS + 1);
        let out = plan_stopped(&[turn("go", vec![tool("Bash", Some(&long))])]);
        assert!(!out.contains(&long));
        assert!(out.contains("… [cut]"));
    }

    #[test]
    fn empty_failed_omits_failed_section() {
        let out = plan_stopped(&[turn("hi", vec![text("hello")])]);
        assert!(!out.contains("You (failed to send)"));
        assert!(out.ends_with("hello\n"), "{out}");
    }

    #[test]
    fn plan_stopped_keeps_file_cap() {
        let turns: Vec<SummaryTurn> = (1..=300)
            .map(|i| turn(&format!("question {i}"), vec![text(&"r".repeat(2_000))]))
            .collect();
        let out = plan_stopped(&turns);
        assert!(
            out.chars().count() <= FILE_MAX_CHARS,
            "{}",
            out.chars().count()
        );
        assert!(out.contains("_(earlier turns omitted)_"));
        assert!(out.contains("question 300\n"));
    }

    #[test]
    fn last_reply_is_last_assistant_text() {
        let history = vec![
            history_turn(TurnRole::User, vec![text("a")]),
            history_turn(
                TurnRole::Assistant,
                vec![text("x"), tool_use(), text("  y \n"), text("  ")],
            ),
            history_turn(TurnRole::User, vec![text("b")]),
        ];
        assert_eq!(last_reply(&history), Some("y".to_string()));
    }

    #[test]
    fn last_reply_none_without_assistant() {
        let history = vec![history_turn(TurnRole::User, vec![text("a")])];
        assert_eq!(last_reply(&history), None);
        assert_eq!(last_reply(&[]), None);
    }

    #[test]
    fn turns_from_history_groups_agent_turns_under_user() {
        let mk = |role: TurnRole, blocks: Vec<ContentBlock>| MessageTurn {
            id: "t".into(),
            role,
            blocks,
            timestamp: chrono::Utc::now(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
            agent_message_id: None,
        };
        let turns = turns_from_history(&[
            mk(TurnRole::User, vec![text("hi")]),
            mk(TurnRole::Assistant, vec![text("a")]),
            mk(TurnRole::System, vec![text("sys")]),
            mk(TurnRole::Assistant, vec![text("b")]),
            mk(TurnRole::User, vec![text("again")]),
        ]);
        assert_eq!(turns.len(), 2);
        assert_eq!(user_text(&turns[0].agent), "a\n\nb");
        assert!(turns[1].agent.is_empty());
    }

    fn history() -> Vec<MessageTurn> {
        [
            ("u1", TurnRole::User, "first"),
            ("a1", TurnRole::Assistant, "one"),
            ("u2", TurnRole::User, "second"),
            ("a2", TurnRole::Assistant, "two"),
            ("u3", TurnRole::User, "third"),
            ("a3", TurnRole::Assistant, "three"),
        ]
        .map(|(id, role, body)| MessageTurn {
            id: id.into(),
            role,
            blocks: vec![text(body)],
            timestamp: Default::default(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
            agent_message_id: None,
        })
        .to_vec()
    }

    #[test]
    fn a_fork_summary_stops_at_the_turn_it_forks() {
        let history = history();
        let kept = through_turn(&history, "a2").unwrap();
        let out = build(&turns_from_history(kept), AGENT, &[], SummaryKind::Forked);

        assert!(out.contains("second") && out.contains("two"), "{out}");
        assert!(!out.contains("third") && !out.contains("three"), "{out}");
    }

    #[test]
    fn a_fork_of_the_last_turn_keeps_everything() {
        let history = history();
        assert_eq!(through_turn(&history, "a3").unwrap().len(), history.len());
    }

    #[test]
    fn an_unknown_turn_cuts_nothing() {
        assert!(through_turn(&history(), "gone").is_none());
    }

    #[test]
    fn a_fork_summary_says_it_is_a_fork_and_lists_tool_calls() {
        let t = turn("go", vec![tool("Bash", Some("ls")), text("done")]);
        let out = build(&[t], AGENT, &[], SummaryKind::Forked);

        assert!(out.starts_with("# Earlier chat\n\n"), "{out}");
        assert!(out.contains("branches off an earlier chat"), "{out}");
        assert!(out.contains("Bash"), "{out}");
        assert!(!out.contains("failed to send"), "{out}");
    }

    #[test]
    fn a_fork_summary_labels_both_sides_of_the_chat() {
        let t = turn("go", vec![text("done")]);
        let out = build(&[t], AGENT, &[], SummaryKind::Forked);

        assert!(out.contains("## User\n\ngo"), "{out}");
        assert!(out.contains("## Assistant\n\ndone"), "{out}");
        assert!(!out.contains(AGENT), "{out}");
    }

    #[test]
    fn a_fork_summary_keeps_an_unanswered_last_turn() {
        let out = build(&[turn("go", vec![])], AGENT, &[], SummaryKind::Forked);
        assert!(out.contains("go"), "{out}");
    }
}
