//! Prompt attachments: files a user attaches to a session prompt.
//!
//! An attachment is stored inside the session's workspace, at
//! `<root>/.zwiim/attachments/<id>/<name>`; the directory is the record. The
//! agent is told the absolute paths in one `<zwiim-attachments>` note
//! appended to the prompt and reads the files with its own tools. The
//! protocol boundary turns that note back into `attachment` blocks, so
//! clients never see it.

use std::path::{Path, PathBuf};

use rand::{distributions::Alphanumeric, Rng};
use serde_json::{json, Value};

use crate::protocol::error::ZwiimError;

/// Largest attachment, decoded.
pub const ATTACHMENT_MAX_BYTES: usize = 10 * 1024 * 1024;

pub const NOTE_OPEN: &str = "<zwiim-attachments>";
pub const NOTE_CLOSE: &str = "</zwiim-attachments>";
const NOTE_INSTRUCTION: &str = "The user attached these files. Read them before proceeding.";

/// Workspace-relative directory holding every attachment.
const DIR: &str = ".zwiim/attachments";
const ID_LEN: usize = 8;
const NAME_MAX_BYTES: usize = 255;

/// The client's file name, safe as one path component: no separators, no
/// control characters, no leading dots. `None` when nothing is left.
pub fn clean_name(raw: &str) -> Option<String> {
    let kept: String = raw
        .chars()
        .filter(|c| !matches!(c, '/' | '\\') && !c.is_control())
        .collect();
    let name = kept.trim().trim_start_matches('.').trim_start();
    if name.is_empty() {
        return None;
    }
    if name.len() <= NAME_MAX_BYTES {
        return Some(name.to_string());
    }
    // Keep the extension; shorten the stem on a char boundary.
    let ext = match name.rfind('.') {
        Some(dot) if name.len() - dot <= 16 => &name[dot..],
        _ => "",
    };
    let mut stem_end = NAME_MAX_BYTES - ext.len();
    while !name.is_char_boundary(stem_end) {
        stem_end -= 1;
    }
    Some(format!("{}{ext}", &name[..stem_end]))
}

/// An attachment id is exactly 8 ASCII letters or digits, so it never forms a
/// path of its own.
pub fn valid_id(id: &str) -> bool {
    id.len() == ID_LEN && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// MIME type from the file extension; `application/octet-stream` when unknown.
pub fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("pdf") => "application/pdf",
        Some("doc") => "application/msword",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("xls") => "application/vnd.ms-excel",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("ppt") => "application/vnd.ms-powerpoint",
        Some("pptx") => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        Some("odt") => "application/vnd.oasis.opendocument.text",
        Some("ods") => "application/vnd.oasis.opendocument.spreadsheet",
        Some("rtf") => "application/rtf",
        Some("zip") => "application/zip",
        Some("gz" | "tgz") => "application/gzip",
        Some("tar") => "application/x-tar",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        Some("html" | "htm") => "text/html",
        Some("css") => "text/css",
        Some("csv") => "text/csv",
        Some("tsv") => "text/tab-separated-values",
        Some("txt" | "log") => "text/plain",
        Some("md" | "markdown") => "text/markdown",
        Some("yaml" | "yml") => "application/yaml",
        Some("toml") => "application/toml",
        Some("js" | "mjs" | "cjs") => "text/javascript",
        Some("ts" | "tsx" | "jsx" | "py" | "rs" | "go" | "java" | "kt" | "swift" | "c" | "h"
            | "cpp" | "hpp" | "rb" | "sh" | "sql") => "text/plain",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("heic") => "image/heic",
        Some("bmp") => "image/bmp",
        Some("tif" | "tiff") => "image/tiff",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("mp4") => "video/mp4",
        Some("mov") => "video/quicktime",
        _ => "application/octet-stream",
    }
}

/// A stored attachment, as `attachments.upload` answers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub attachment_id: String,
    pub name: String,
    pub size: u64,
    pub mime_type: &'static str,
}

fn new_id() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(ID_LEN)
        .map(char::from)
        .collect()
}

/// Save `bytes` as attachment `name` in workspace `root`. The file appears
/// whole or not at all; the first attachment in a repository keeps `.zwiim/`
/// out of `git status` through the repository's `info/exclude`.
pub async fn store(root: &Path, name: &str, bytes: &[u8]) -> Result<Stored, ZwiimError> {
    if bytes.len() > ATTACHMENT_MAX_BYTES {
        return Err(ZwiimError::attachment_too_large());
    }
    let name = clean_name(name).ok_or_else(ZwiimError::invalid_name)?;
    let base = root.join(DIR);
    tokio::fs::create_dir_all(&base).await.map_err(io_err)?;
    let (id, dir) = loop {
        let id = new_id();
        let dir = base.join(&id);
        match tokio::fs::create_dir(&dir).await {
            Ok(()) => break (id, dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io_err(e)),
        }
    };
    let part = dir.join(".upload.part");
    let written = async {
        tokio::fs::write(&part, bytes).await?;
        tokio::fs::rename(&part, dir.join(&name)).await
    }
    .await;
    if let Err(e) = written {
        let _ = tokio::fs::remove_dir_all(&dir).await;
        return Err(io_err(e));
    }
    crate::commands::folder_links::add_to_git_exclude(root, ".zwiim/").await;
    Ok(Stored {
        attachment_id: id,
        mime_type: mime_for(&name),
        name,
        size: bytes.len() as u64,
    })
}

fn io_err(e: std::io::Error) -> ZwiimError {
    tracing::warn!("[attachments] storage failed: {e}");
    ZwiimError::internal()
}

/// Absolute path of attachment `id` in workspace `root`.
pub fn resolve(root: &Path, id: &str) -> Result<PathBuf, ZwiimError> {
    if !valid_id(id) {
        return Err(ZwiimError::attachment_not_found());
    }
    let dir = root.join(DIR).join(id);
    let entries = std::fs::read_dir(&dir).map_err(|_| ZwiimError::attachment_not_found())?;
    entries
        .filter_map(Result::ok)
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .find(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .ok_or_else(ZwiimError::attachment_not_found)
}

/// The text block telling the agent where the attached files are.
pub fn note(paths: &[PathBuf]) -> String {
    let mut out = format!("{NOTE_OPEN}\n{NOTE_INSTRUCTION}\n");
    for p in paths {
        out.push_str("- ");
        out.push_str(&p.to_string_lossy());
        out.push('\n');
    }
    out.push_str(NOTE_CLOSE);
    out
}

/// User text with its attachment note taken out.
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    /// The text without the note; empty when the user typed nothing.
    pub text: String,
    /// `attachment` blocks, in note order.
    pub attachments: Vec<Value>,
}

/// One note line → its `attachment` block, when it names a file under
/// `.zwiim/attachments/<id>/`. With `root`, the path must be in it and the
/// block carries the file's current `size`.
fn attachment_block(line: &str, root: Option<&str>) -> Option<Value> {
    let path = line.strip_prefix("- ")?.trim_end();
    let marker = format!("/{DIR}/");
    let at = path.rfind(&marker)?;
    let rest = &path[at + marker.len()..];
    let (id, name) = rest.split_once('/')?;
    if !valid_id(id) || name.is_empty() || name.contains('/') {
        return None;
    }
    let size = match root.map(|r| r.trim_end_matches('/')) {
        Some(root) => {
            if &path[..at] != root {
                return None;
            }
            std::fs::metadata(path).ok().filter(|m| m.is_file()).map(|m| m.len())
        }
        None => None,
    };
    let mut block = json!({
        "type": "attachment",
        "attachment_id": id,
        "name": name,
        "mime_type": mime_for(name),
        "path": format!("{DIR}/{id}/{name}"),
    });
    if let Some(size) = size {
        block["size"] = json!(size);
    }
    Some(block)
}

/// Take every attachment note out of `text`. `None` when there is none. Note
/// lines that do not name an attachment stay in the text: user content is
/// never dropped.
pub fn split_note(text: &str, root: Option<&str>) -> Option<Split> {
    if !text.contains(NOTE_OPEN) {
        return None;
    }
    let mut out = String::new();
    let mut attachments = Vec::new();
    let mut rest = text;
    let mut found = false;
    while let Some(open) = rest.find(NOTE_OPEN) {
        let body_start = open + NOTE_OPEN.len();
        let Some(close_len) = rest[body_start..].find(NOTE_CLOSE) else {
            break;
        };
        found = true;
        push_part(&mut out, &rest[..open]);
        let mut kept = Vec::new();
        for line in rest[body_start..body_start + close_len].lines() {
            let line = line.trim();
            if line.is_empty() || line == NOTE_INSTRUCTION {
                continue;
            }
            match attachment_block(line, root) {
                Some(block) => attachments.push(block),
                None => kept.push(line),
            }
        }
        push_part(&mut out, &kept.join("\n"));
        rest = &rest[body_start + close_len + NOTE_CLOSE.len()..];
    }
    if !found {
        return None;
    }
    push_part(&mut out, rest);
    Some(Split {
        text: out,
        attachments,
    })
}

/// Append `part`, trimmed, separated from what is already there by a blank line.
fn push_part(out: &mut String, part: &str) {
    let part = part.trim();
    if part.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(part);
}

/// A session title never shows the note: the text before it.
pub fn cut_title(text: &str) -> &str {
    match text.find(NOTE_OPEN) {
        Some(at) => text[..at].trim_end(),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_name_keeps_one_safe_component() {
        assert_eq!(clean_name("report.pdf").as_deref(), Some("report.pdf"));
        assert_eq!(clean_name("../x").as_deref(), Some("x"));
        assert_eq!(clean_name("a/b\\c").as_deref(), Some("abc"));
        assert_eq!(clean_name("\u{0}\n.env").as_deref(), Some("env"));
        assert_eq!(
            clean_name("Screenshot 2026-10-07 at 10.50.11 PM.png").as_deref(),
            Some("Screenshot 2026-10-07 at 10.50.11 PM.png")
        );
        assert_eq!(clean_name("résumé.pdf").as_deref(), Some("résumé.pdf"));
        assert_eq!(clean_name("..."), None);
        assert_eq!(clean_name(" / "), None);
    }

    #[test]
    fn clean_name_shortens_long_names_and_keeps_the_extension() {
        let long = format!("{}.xlsx", "é".repeat(200));
        let name = clean_name(&long).unwrap();
        assert!(name.len() <= NAME_MAX_BYTES);
        assert!(name.ends_with(".xlsx"));
    }

    #[test]
    fn ids_are_eight_alphanumerics() {
        assert!(valid_id("k3F9a2Qx"));
        assert!(!valid_id("../../etc"));
        assert!(!valid_id("short"));
        assert!(!valid_id("k3F9a2Q-"));
        assert!(!valid_id("k3F9a2Qxy"));
        assert!(valid_id(&new_id()));
    }

    #[test]
    fn mime_follows_the_extension() {
        assert_eq!(mime_for("a.pdf"), "application/pdf");
        assert_eq!(mime_for("A.XLSX"), "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet");
        assert_eq!(mime_for("a.docx"), "application/vnd.openxmlformats-officedocument.wordprocessingml.document");
        assert_eq!(mime_for("a.csv"), "text/csv");
        assert_eq!(mime_for("a.html"), "text/html");
        assert_eq!(mime_for("a.png"), "image/png");
        assert_eq!(mime_for("a.zip"), "application/zip");
        assert_eq!(mime_for("a.unknown"), "application/octet-stream");
        assert_eq!(mime_for("noext"), "application/octet-stream");
    }

    #[tokio::test]
    async fn store_writes_the_file_and_resolve_finds_it() {
        let dir = tempfile::tempdir().unwrap();
        let stored = store(dir.path(), "../report.pdf", b"%PDF-1.7").await.unwrap();
        assert!(valid_id(&stored.attachment_id));
        assert_eq!(stored.name, "report.pdf");
        assert_eq!(stored.size, 8);
        assert_eq!(stored.mime_type, "application/pdf");
        let path = resolve(dir.path(), &stored.attachment_id).unwrap();
        assert_eq!(
            path,
            dir.path()
                .join(".zwiim/attachments")
                .join(&stored.attachment_id)
                .join("report.pdf")
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"%PDF-1.7");
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(leftovers.len(), 1);
    }

    #[tokio::test]
    async fn store_accepts_the_limit_and_refuses_one_byte_more() {
        let dir = tempfile::tempdir().unwrap();
        let at_limit = vec![0u8; ATTACHMENT_MAX_BYTES];
        assert!(store(dir.path(), "a.bin", &at_limit).await.is_ok());
        let over = vec![0u8; ATTACHMENT_MAX_BYTES + 1];
        let err = store(dir.path(), "b.bin", &over).await.unwrap_err();
        assert_eq!(err.code, "attachment_too_large");
    }

    #[tokio::test]
    async fn store_refuses_an_empty_name() {
        let dir = tempfile::tempdir().unwrap();
        let err = store(dir.path(), "..", b"x").await.unwrap_err();
        assert_eq!(err.code, "invalid_name");
    }

    #[test]
    fn resolve_refuses_bad_and_unknown_ids() {
        let dir = tempfile::tempdir().unwrap();
        for id in ["../../etc", "short", "k3F9a2Q-", "k3F9a2Qx"] {
            assert_eq!(resolve(dir.path(), id).unwrap_err().code, "attachment_not_found");
        }
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }

    #[tokio::test]
    async fn store_keeps_attachments_out_of_git_status_in_every_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init"]);
        let wt = tmp.path().join("wt");
        git(&repo, &["worktree", "add", "-q", "-b", "feature", wt.to_str().unwrap()]);

        store(&wt, "a.csv", b"a,b").await.unwrap();
        store(&wt, "b.csv", b"a,b").await.unwrap();
        store(&repo, "c.csv", b"a,b").await.unwrap();

        assert_eq!(git(&wt, &["status", "--porcelain"]), "");
        assert_eq!(git(&repo, &["status", "--porcelain"]), "");
        let exclude = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.lines().filter(|l| *l == "/.zwiim/").count(), 1);
    }

    #[test]
    fn note_round_trips_through_split() {
        let root = tempfile::tempdir().unwrap();
        let root_str = root.path().to_str().unwrap();
        let a = root.path().join(".zwiim/attachments/k3F9a2Qx/Screenshot 2026 at 10.50 PM.png");
        let b = root.path().join(".zwiim/attachments/Zp81QaLm/résumé.pdf");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap();
        std::fs::write(&a, b"png!").unwrap();
        let text = format!("summarize this\n\n{}", note(&[a, b]));

        let split = split_note(&text, Some(root_str)).unwrap();
        assert_eq!(split.text, "summarize this");
        assert_eq!(
            split.attachments,
            vec![
                json!({
                    "type": "attachment",
                    "attachment_id": "k3F9a2Qx",
                    "name": "Screenshot 2026 at 10.50 PM.png",
                    "mime_type": "image/png",
                    "path": ".zwiim/attachments/k3F9a2Qx/Screenshot 2026 at 10.50 PM.png",
                    "size": 4,
                }),
                // Gone from disk: no size.
                json!({
                    "type": "attachment",
                    "attachment_id": "Zp81QaLm",
                    "name": "résumé.pdf",
                    "mime_type": "application/pdf",
                    "path": ".zwiim/attachments/Zp81QaLm/résumé.pdf",
                }),
            ]
        );
        assert!(!serde_json::to_string(&split.attachments).unwrap().contains(root_str));
    }

    #[test]
    fn split_keeps_lines_that_name_no_attachment() {
        let text = format!(
            "{NOTE_OPEN}\n{NOTE_INSTRUCTION}\n- /r/.zwiim/attachments/k3F9a2Qx/a.pdf\n- /etc/passwd\nhello\n{NOTE_CLOSE}"
        );
        let split = split_note(&text, Some("/r")).unwrap();
        assert_eq!(split.text, "- /etc/passwd\nhello");
        assert_eq!(split.attachments.len(), 1);
        // Outside the session's workspace: not an attachment.
        let split = split_note(&text, Some("/other")).unwrap();
        assert_eq!(split.attachments.len(), 0);
    }

    #[test]
    fn split_leaves_a_notice_after_the_note_in_place() {
        let text = format!(
            "{}\n\n\u{1e}[zwiim-notice]\nBranch changed\u{1e}",
            note(&[PathBuf::from("/r/.zwiim/attachments/k3F9a2Qx/a.pdf")])
        );
        let split = split_note(&text, Some("/r")).unwrap();
        assert!(!split.text.contains(NOTE_OPEN));
        assert!(split.text.starts_with("\u{1e}[zwiim-notice]"));
        assert_eq!(split.attachments.len(), 1);
    }

    #[test]
    fn split_is_none_without_a_complete_note() {
        assert_eq!(split_note("plain text", None), None);
        assert_eq!(split_note(&format!("{NOTE_OPEN} unterminated"), None), None);
    }

    #[test]
    fn titles_stop_before_the_note() {
        let text = format!("look at this\n\n{}", note(&[PathBuf::from("/r/x")]));
        assert_eq!(cut_title(&text), "look at this");
        assert_eq!(cut_title(&note(&[PathBuf::from("/r/x")])), "");
        assert_eq!(cut_title("plain"), "plain");
    }
}
