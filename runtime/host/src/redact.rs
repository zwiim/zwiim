//! Provider API keys and access tokens out of text the Host passes on: client-facing errors and
//! history, logs, and diagnostics.
//!
//! The Host never puts a key it holds into a message, but vendors do: a refused
//! key comes back in the provider's own error (OpenAI answers a bad key with
//! `Incorrect API key provided: sk-proj-**…wxyz`), and that text travels on as
//! the agent's error. Anything shaped like a key is replaced with
//! `<redacted>`, masked or not.

use std::borrow::Cow;
use std::io::{self, Write};

use tracing::Metadata;
use tracing_subscriber::fmt::MakeWriter;

const REDACTED: &str = "<redacted>";

/// How the credentials vendors issue begin.
const KEY_PREFIXES: &[&str] = &[
    "sk-",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
];

fn next_key(text: &str) -> Option<usize> {
    KEY_PREFIXES.iter().filter_map(|p| text.find(p)).min()
}

/// `text` with every key-shaped word replaced by `<redacted>`.
pub fn scrub_secrets(text: &str) -> Cow<'_, str> {
    if next_key(text).is_none() {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = next_key(rest) {
        // Only at the start of a word: `task-` or `disk-` are not keys.
        let starts_word = rest[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'));
        let len = rest[at..]
            .find(|c: char| !is_key_char(c))
            .unwrap_or(rest.len() - at);
        out.push_str(&rest[..at]);
        let word = &rest[at..at + len];
        if starts_word && looks_like_key(word) {
            out.push_str(REDACTED);
        } else {
            out.push_str(word);
        }
        rest = &rest[at + len..];
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Characters a key (or a vendor's masked echo of one) is made of.
fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '*' | '.')
}

/// A prefix then enough key material to be one; a trailing `.` ends a sentence.
fn looks_like_key(word: &str) -> bool {
    word.trim_end_matches('.').len() >= 8
}

/// A log writer that scrubs each formatted line before it reaches the sink.
pub struct Scrubbing<M>(pub M);

impl<'a, M: MakeWriter<'a>> MakeWriter<'a> for Scrubbing<M> {
    type Writer = ScrubbingWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        ScrubbingWriter(self.0.make_writer())
    }

    fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Self::Writer {
        ScrubbingWriter(self.0.make_writer_for(meta))
    }
}

pub struct ScrubbingWriter<W>(W);

impl<W: Write> Write for ScrubbingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // `fmt` hands over each event as one whole buffer, so a key is never
        // split across two writes.
        match std::str::from_utf8(buf).map(scrub_secrets) {
            Ok(Cow::Owned(clean)) => {
                self.0.write_all(clean.as_bytes())?;
                Ok(buf.len())
            }
            _ => self.0.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_their_masked_echoes_are_redacted() {
        assert_eq!(
            scrub_secrets("Incorrect API key provided: sk-dummy**ocal. You can find your API key"),
            "Incorrect API key provided: <redacted> You can find your API key"
        );
        assert_eq!(
            scrub_secrets("key=sk-proj-AbC_123-xyz and sk-ant-api03-QwErTy"),
            "key=<redacted> and <redacted>"
        );
        assert_eq!(scrub_secrets("(\"sk-1234567890\")"), "(\"<redacted>\")");
    }

    #[test]
    fn access_tokens_are_redacted() {
        assert_eq!(
            scrub_secrets("push failed for ghp_0123456789abcdef and github_pat_11ABC_def"),
            "push failed for <redacted> and <redacted>"
        );
        assert_eq!(
            scrub_secrets("token=gho_0123456789abcdef; slack xoxb-1234-5678"),
            "token=<redacted>; slack <redacted>"
        );
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        let plain = "the task-runner hit disk-full; see sk- for details";
        assert_eq!(scrub_secrets(plain), plain);
        assert_eq!(scrub_secrets("no keys here"), "no keys here");
        assert_eq!(scrub_secrets("sk-short"), "<redacted>");
        assert_eq!(scrub_secrets("sk-a"), "sk-a");
        assert_eq!(scrub_secrets("the ghp_ prefix"), "the ghp_ prefix");
    }

    #[test]
    fn log_lines_are_scrubbed_on_the_way_out() {
        let mut out = ScrubbingWriter(Vec::new());
        out.write_all(b"codex refused key sk-test-0123456789\n")
            .unwrap();
        out.write_all(b"plain line\n").unwrap();
        assert_eq!(
            String::from_utf8(out.0).unwrap(),
            "codex refused key <redacted>\nplain line\n"
        );
    }
}
