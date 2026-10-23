//! Parsed session history, kept while the files it was read from are
//! unchanged.
//!
//! A client that reconnects, reopens a session or loads a second view of it
//! asks for the history again. Parsing a long transcript is the expensive
//! part of that request, and when no turn has completed since the last read
//! the answer is the same, so it is served from here instead.
//!
//! An entry is keyed by the transcript's path and stamped with the length and
//! modification time of every file the parse read (the transcript first, then
//! the parser's side files, see `AgentParser::transcript_files`). Any change
//! to any of them, a file appearing or a file going away is a miss and a
//! fresh parse. The cache holds at most [`CAPACITY`] sessions and drops the
//! least recently used one past that.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::models::ConversationDetail;
use crate::parsers::{AgentParser, ParseError};

/// Sessions kept. A detail can be several MB for a long session, so this is
/// sized for the handful a user switches between, not for every session.
pub const CAPACITY: usize = 8;

/// Length and modification time of one file; `None` when it is missing.
type FileStamp = Option<(u64, Option<SystemTime>)>;

/// The stamps of every file a parse read, in the parser's order.
#[derive(Debug, Clone, PartialEq)]
struct Stamp(Vec<(PathBuf, FileStamp)>);

impl Stamp {
    fn of(files: &[PathBuf]) -> Self {
        Stamp(files.iter().map(|p| (p.clone(), file_stamp(p))).collect())
    }
}

fn file_stamp(path: &Path) -> FileStamp {
    let meta = fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

struct Entry {
    path: PathBuf,
    stamp: Stamp,
    detail: ConversationDetail,
}

/// A bounded least-recently-used map from transcript path to its parse.
pub struct HistoryCache {
    capacity: usize,
    /// Most recently used last.
    entries: Mutex<VecDeque<Entry>>,
}

impl HistoryCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: Mutex::new(VecDeque::new()),
        }
    }

    /// The parse of `files` (the transcript first): the cached one when
    /// every file is as it was, otherwise `parse()`'s, which is kept only if
    /// no file changed while it ran.
    pub fn get_or_parse(
        &self,
        files: &[PathBuf],
        parse: impl FnOnce() -> Result<ConversationDetail, ParseError>,
    ) -> Result<ConversationDetail, ParseError> {
        let Some(path) = files.first() else {
            return parse();
        };
        let before = Stamp::of(files);
        if before.0[0].1.is_none() {
            return parse();
        }
        if let Some(hit) = self.lookup(path, &before) {
            return Ok(hit);
        }
        let detail = parse()?;
        if Stamp::of(files) == before {
            self.store(path.clone(), before, detail.clone());
        }
        Ok(detail)
    }

    fn lookup(&self, path: &Path, stamp: &Stamp) -> Option<ConversationDetail> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let at = entries.iter().position(|e| e.path == path)?;
        if entries[at].stamp != *stamp {
            entries.remove(at);
            return None;
        }
        let entry = entries.remove(at)?;
        let detail = entry.detail.clone();
        entries.push_back(entry);
        Some(detail)
    }

    fn store(&self, path: PathBuf, stamp: Stamp, detail: ConversationDetail) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|e| e.path != path);
        entries.push_back(Entry {
            path,
            stamp,
            detail,
        });
        while entries.len() > self.capacity {
            entries.pop_front();
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

/// The Host's one cache.
fn shared() -> &'static HistoryCache {
    static CACHE: OnceLock<HistoryCache> = OnceLock::new();
    CACHE.get_or_init(|| HistoryCache::new(CAPACITY))
}

/// `parser.get_conversation(conversation_id)`, through the cache when the
/// parser names the files it reads.
pub fn get_conversation(
    parser: &dyn AgentParser,
    conversation_id: &str,
) -> Result<ConversationDetail, ParseError> {
    let files = parser.transcript_files(conversation_id);
    shared().get_or_parse(&files, || parser.get_conversation(conversation_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ConversationSummary, MessageTurn};
    use std::cell::Cell;
    use std::io::Write;

    fn detail(title: &str) -> ConversationDetail {
        let summary: ConversationSummary = serde_json::from_value(serde_json::json!({
            "id": "s",
            "agent_type": "codex",
            "folder_path": null,
            "folder_name": null,
            "title": title,
            "started_at": "2026-10-19T00:00:00Z",
            "ended_at": null,
            "message_count": 0,
            "model": null,
            "git_branch": null,
        }))
        .expect("summary");
        ConversationDetail {
            summary,
            turns: Vec::<MessageTurn>::new(),
            session_stats: None,
            transcript_watermark: None,
        }
    }

    fn append(path: &Path, text: &str) {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn history_cache_hit_when_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        append(&path, "{\"a\":1}\n");
        let cache = HistoryCache::new(2);
        let parses = Cell::new(0);
        let parse = || {
            parses.set(parses.get() + 1);
            Ok(detail("first"))
        };
        let files = vec![path.clone()];
        cache.get_or_parse(&files, parse).unwrap();
        let again = cache
            .get_or_parse(&files, || {
                parses.set(parses.get() + 1);
                Ok(detail("second"))
            })
            .unwrap();
        assert_eq!(parses.get(), 1, "the second read is a hit");
        assert_eq!(again.summary.title.as_deref(), Some("first"));
    }

    #[test]
    fn history_cache_miss_when_file_grows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        append(&path, "{\"a\":1}\n");
        let cache = HistoryCache::new(2);
        let files = vec![path.clone()];
        cache.get_or_parse(&files, || Ok(detail("first"))).unwrap();
        append(&path, "{\"b\":2}\n");
        let parses = Cell::new(0);
        let next = cache
            .get_or_parse(&files, || {
                parses.set(parses.get() + 1);
                Ok(detail("grown"))
            })
            .unwrap();
        assert_eq!(parses.get(), 1, "a grown transcript is parsed again");
        assert_eq!(next.summary.title.as_deref(), Some("grown"));
        // And the new parse is what the next read gets.
        let hit = cache.get_or_parse(&files, || Ok(detail("third"))).unwrap();
        assert_eq!(hit.summary.title.as_deref(), Some("grown"));
    }

    #[test]
    fn history_cache_miss_when_a_side_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let side = dir.path().join("agent-1.jsonl");
        append(&path, "{}\n");
        let cache = HistoryCache::new(2);
        let files = vec![path.clone(), side.clone()];
        cache.get_or_parse(&files, || Ok(detail("first"))).unwrap();
        // A side file that appears.
        append(&side, "{}\n");
        let next = cache.get_or_parse(&files, || Ok(detail("side"))).unwrap();
        assert_eq!(next.summary.title.as_deref(), Some("side"));
        // A side file that grows.
        append(&side, "{}\n");
        let next = cache.get_or_parse(&files, || Ok(detail("grew"))).unwrap();
        assert_eq!(next.summary.title.as_deref(), Some("grew"));
    }

    #[test]
    fn history_cache_is_bounded_and_skips_unknown_files() {
        let dir = tempfile::tempdir().unwrap();
        let cache = HistoryCache::new(2);
        let paths: Vec<PathBuf> = (0..3)
            .map(|i| {
                let p = dir.path().join(format!("{i}.jsonl"));
                append(&p, "{}\n");
                p
            })
            .collect();
        for p in &paths {
            cache
                .get_or_parse(std::slice::from_ref(p), || Ok(detail("x")))
                .unwrap();
        }
        assert_eq!(cache.len(), 2);
        // The oldest was dropped: reading it parses again.
        let parses = Cell::new(0);
        cache
            .get_or_parse(std::slice::from_ref(&paths[0]), || {
                parses.set(parses.get() + 1);
                Ok(detail("x"))
            })
            .unwrap();
        assert_eq!(parses.get(), 1);

        // No files named, or a transcript that is missing: never cached.
        cache.get_or_parse(&[], || Ok(detail("x"))).unwrap();
        let missing = vec![dir.path().join("gone.jsonl")];
        cache.get_or_parse(&missing, || Ok(detail("x"))).unwrap();
        assert_eq!(cache.len(), 2);
        // A failed parse is not cached either.
        let fresh = vec![paths[1].clone()];
        let cache = HistoryCache::new(2);
        assert!(cache
            .get_or_parse(&fresh, || Err(ParseError::ConversationNotFound("s".into())))
            .is_err());
        assert_eq!(cache.len(), 0);
    }
}
