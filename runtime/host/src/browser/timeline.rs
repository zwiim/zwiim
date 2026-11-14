//! Each session's browser recording (design 10.7): one small JPEG and a line
//! of what the agent did, per step, so a person can look back at what
//! happened while they were away. No continuous video.
//!
//! Layout: `<root>/<session_id>/steps.jsonl`, one [`Step`] per line, oldest
//! first, `<root>/<session_id>/<seq>.jpg` beside it, and the last number
//! given in `last`, all readable by this user only. A session keeps at most [`Limits::max_bytes`] of images
//! and nothing older than [`Limits::max_age`]; the oldest steps go first.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// One recorded step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// From 1, per session, never reused.
    pub seq: u64,
    /// When, in milliseconds since the Unix epoch.
    pub at: i64,
    pub tab_id: String,
    /// What the agent did, in a few words: "Clicked", "Opened https://…".
    pub action: String,
    /// The page the step left the tab on.
    pub url: String,
    /// The image's size.
    pub bytes: u64,
}

/// How much a session's recording keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_bytes: u64,
    pub max_age: Duration,
}

const STEPS: &str = "steps.jsonl";
/// The last step number given, so a trimmed recording never reuses one.
const LAST: &str = "last";

/// The recordings of every session, under `root`.
pub struct Timeline {
    root: PathBuf,
}

impl Timeline {
    pub fn new(root: PathBuf) -> Self {
        Timeline { root }
    }

    /// Session `session_id`'s folder; None for an id that is not a plain
    /// name.
    fn dir(&self, session_id: &str) -> Option<PathBuf> {
        let plain = !session_id.is_empty()
            && session_id.len() <= 64
            && session_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        plain.then(|| self.root.join(session_id))
    }

    /// Record `step` with its image `jpeg`, then trim the session to
    /// `limits`. The step's number and size are filled in.
    pub fn append(
        &self,
        session_id: &str,
        step: Step,
        jpeg: &[u8],
        limits: Limits,
    ) -> std::io::Result<Step> {
        let dir = self.dir(session_id).ok_or_else(bad_id)?;
        std::fs::create_dir_all(&dir)?;
        let last = std::fs::read_to_string(dir.join(LAST))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .or_else(|| read_steps(&dir).last().map(|s| s.seq))
            .unwrap_or(0);
        let now = step.at;
        let step = Step {
            seq: last + 1,
            bytes: jpeg.len() as u64,
            ..step
        };
        write_private(&dir.join(LAST), step.seq.to_string().as_bytes(), false)?;
        write_private(&dir.join(image_name(step.seq)), jpeg, false)?;
        let mut line = serde_json::to_vec(&step)?;
        line.push(b'\n');
        write_private(&dir.join(STEPS), &line, true)?;
        trim(&dir, now, limits)?;
        Ok(step)
    }

    /// Session `session_id`'s steps, newest first: at most `limit` of them,
    /// older than step `before` when given. Steps past `limits` are trimmed
    /// first.
    pub fn steps(
        &self,
        session_id: &str,
        before: Option<u64>,
        limit: usize,
        now: i64,
        limits: Limits,
    ) -> Vec<Step> {
        let Some(dir) = self.dir(session_id) else {
            return Vec::new();
        };
        let _ = trim(&dir, now, limits);
        read_steps(&dir)
            .into_iter()
            .rev()
            .filter(|s| before.is_none_or(|b| s.seq < b))
            .take(limit)
            .collect()
    }

    /// Step `seq`'s image.
    pub fn image(&self, session_id: &str, seq: u64) -> std::io::Result<Vec<u8>> {
        let dir = self.dir(session_id).ok_or_else(bad_id)?;
        std::fs::read(dir.join(image_name(seq)))
    }

    /// Drop session `session_id`'s recording.
    pub fn remove(&self, session_id: &str) {
        if let Some(dir) = self.dir(session_id) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn bad_id() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a session id")
}

fn image_name(seq: u64) -> String {
    format!("{seq}.jpg")
}

/// The steps in `dir`, oldest first; a line that does not read is skipped.
fn read_steps(dir: &Path) -> Vec<Step> {
    std::fs::read_to_string(dir.join(STEPS))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Drop the steps in `dir` older than `limits.max_age`, then the oldest
/// until the images fit `limits.max_bytes`.
fn trim(dir: &Path, now: i64, limits: Limits) -> std::io::Result<()> {
    let steps = read_steps(dir);
    let oldest = now - limits.max_age.as_millis() as i64;
    let mut total: u64 = steps.iter().map(|s| s.bytes).sum();
    let mut kept = Vec::with_capacity(steps.len());
    let mut dropped = false;
    for step in steps {
        if step.at < oldest || total > limits.max_bytes {
            total -= step.bytes;
            let _ = std::fs::remove_file(dir.join(image_name(step.seq)));
            dropped = true;
        } else {
            kept.push(step);
        }
    }
    if !dropped {
        return Ok(());
    }
    let mut body = Vec::new();
    for step in &kept {
        body.extend(serde_json::to_vec(step)?);
        body.push(b'\n');
    }
    write_private(&dir.join(STEPS), &body, false)
}

/// Write `bytes` to `path`, or add them to its end, readable by this user
/// only.
fn write_private(path: &Path, bytes: &[u8], append: bool) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true);
    if append {
        options.append(true);
    } else {
        options.write(true).truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 24 * 60 * 60 * 1000;

    fn step(action: &str, at: i64) -> Step {
        Step {
            seq: 0,
            at,
            tab_id: "t1".into(),
            action: action.into(),
            url: "https://a.example/".into(),
            bytes: 0,
        }
    }
    const ROOMY: Limits = Limits {
        max_bytes: 1 << 20,
        max_age: Duration::from_secs(7 * 24 * 60 * 60),
    };

    #[test]
    fn steps_are_kept_newest_first_with_their_images() {
        let dir = tempfile::tempdir().unwrap();
        let timeline = Timeline::new(dir.path().to_path_buf());
        for (i, action) in ["Opened https://a.example/", "Clicked", "Typed into a field"]
            .iter()
            .enumerate()
        {
            let step = timeline
                .append("s1", step(action, 1000), &[i as u8; 10], ROOMY)
                .unwrap();
            assert_eq!(step.seq, i as u64 + 1);
        }
        let steps = timeline.steps("s1", None, 10, 1000, ROOMY);
        let actions: Vec<_> = steps.iter().map(|s| s.action.as_str()).collect();
        assert_eq!(
            actions,
            ["Typed into a field", "Clicked", "Opened https://a.example/"]
        );
        let older = timeline.steps("s1", Some(3), 1, 1000, ROOMY);
        assert_eq!(older[0].seq, 2);
        assert_eq!(timeline.image("s1", 2).unwrap(), vec![1u8; 10]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("s1/2.jpg"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(timeline.steps("other", None, 10, 1000, ROOMY).is_empty());
        assert!(timeline.append("../x", step("a", 0), &[], ROOMY).is_err());

        timeline.remove("s1");
        assert!(!dir.path().join("s1").exists());
    }

    #[test]
    fn the_oldest_steps_go_past_the_size_or_the_age() {
        let dir = tempfile::tempdir().unwrap();
        let timeline = Timeline::new(dir.path().to_path_buf());
        let small = Limits {
            max_bytes: 25,
            ..ROOMY
        };
        for _ in 0..3 {
            timeline
                .append("s1", step("Clicked", 0), &[0; 10], small)
                .unwrap();
        }
        let seqs: Vec<_> = timeline
            .steps("s1", None, 10, 0, small)
            .iter()
            .map(|s| s.seq)
            .collect();
        assert_eq!(seqs, [3, 2]);
        assert!(timeline.image("s1", 1).is_err());

        // A week later, both are past their age.
        let later = 8 * DAY;
        assert!(timeline.steps("s1", None, 10, later, small).is_empty());
        assert!(timeline.image("s1", 3).is_err());
        // Numbers are never reused.
        let next = timeline
            .append("s1", step("Clicked", later), &[0; 10], small)
            .unwrap();
        assert_eq!(next.seq, 4);
    }
}
