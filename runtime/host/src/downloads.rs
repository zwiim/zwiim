//! Downloads in flight, so every client can show their progress: an agent's
//! adapter, the browser engine, a Host update (which brings `gh`). Whatever
//! downloads takes a [`Progress`] from [`Downloads::begin`] and passes it to
//! each [`crate::adapters::install::Fetch`]; the Host feed publishes the list
//! as `download.changed` / `download.removed`.
//!
//! One entry per thing the person would name, not per file: an adapter's
//! hundred packages are one "Claude Code".

use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::watch;

/// Progress is announced at most this often per download.
const NOTIFY_EVERY: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Download {
    pub download_id: String,
    /// What is downloading, as the person knows it: "Claude Code",
    /// "Browser engine 157.0.8103.78", "Zwiim 0.6.0".
    pub label: String,
    /// Bytes received so far.
    pub received: u64,
    /// Bytes in all, when known.
    pub total: Option<u64>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    list: Vec<Download>,
}

pub struct Downloads {
    inner: Mutex<Inner>,
    /// Bumped on every announced change; the host feed watches it.
    notify: watch::Sender<u64>,
}

/// The Host's downloads.
pub fn global() -> Arc<Downloads> {
    static GLOBAL: LazyLock<Arc<Downloads>> = LazyLock::new(Downloads::new);
    GLOBAL.clone()
}

impl Downloads {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::default(),
            notify: watch::Sender::new(0),
        })
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.notify.subscribe()
    }

    /// Every download in flight, oldest first.
    pub fn list(&self) -> Vec<Download> {
        self.inner.lock().unwrap().list.clone()
    }

    /// Start a download of `files` files named `label`. It is listed until
    /// the returned [`Progress`] is dropped. Its total is known only for a
    /// single file.
    pub fn begin(self: &Arc<Self>, label: impl Into<String>, files: usize) -> Progress {
        let download_id = {
            let mut inner = self.inner.lock().unwrap();
            inner.next += 1;
            let download_id = format!("download-{}", inner.next);
            inner.list.push(Download {
                download_id: download_id.clone(),
                label: label.into(),
                received: 0,
                total: None,
            });
            download_id
        };
        self.changed();
        Progress {
            to: Some(Tracked {
                downloads: self.clone(),
                download_id,
                single: files == 1,
                last: Mutex::new(Instant::now()),
            }),
        }
    }

    fn changed(&self) {
        self.notify.send_modify(|v| *v = v.wrapping_add(1));
    }

    fn update(&self, download_id: &str, f: impl FnOnce(&mut Download)) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(d) = inner.list.iter_mut().find(|d| d.download_id == download_id) {
            f(d);
        }
    }
}

/// A download's progress, reported by the fetches that make it up.
pub struct Progress {
    to: Option<Tracked>,
}

struct Tracked {
    downloads: Arc<Downloads>,
    download_id: String,
    single: bool,
    last: Mutex<Instant>,
}

impl Progress {
    /// Progress nobody sees: small files (a manifest), tests.
    pub fn none() -> Progress {
        Progress { to: None }
    }

    /// A file of `size` bytes starts (`None` when the server did not say).
    pub fn file(&self, size: Option<u64>) {
        let Some(t) = &self.to else { return };
        if !t.single {
            return;
        }
        t.downloads.update(&t.download_id, |d| d.total = size);
        t.downloads.changed();
    }

    /// `bytes` more arrived.
    pub fn received(&self, bytes: u64) {
        let Some(t) = &self.to else { return };
        t.downloads.update(&t.download_id, |d| d.received += bytes);
        let mut last = t.last.lock().unwrap();
        if last.elapsed() >= NOTIFY_EVERY {
            *last = Instant::now();
            t.downloads.changed();
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        let Some(t) = &self.to else { return };
        t.downloads
            .inner
            .lock()
            .unwrap()
            .list
            .retain(|d| d.download_id != t.download_id);
        t.downloads.changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_is_listed_until_it_ends() {
        let downloads = Downloads::new();
        let mut rx = downloads.subscribe();
        let one = downloads.begin("Browser engine 157", 1);
        assert!(rx.has_changed().unwrap());
        rx.borrow_and_update();
        one.file(Some(100));
        one.received(40);
        assert_eq!(
            downloads.list(),
            vec![Download {
                download_id: "download-1".into(),
                label: "Browser engine 157".into(),
                received: 40,
                total: Some(100),
            }]
        );
        drop(one);
        assert!(downloads.list().is_empty());
        assert!(rx.has_changed().unwrap());
    }

    #[test]
    fn many_files_count_bytes_but_no_total() {
        let downloads = Downloads::new();
        let many = downloads.begin("Claude Code", 3);
        many.file(Some(10));
        many.received(10);
        many.file(Some(20));
        many.received(5);
        let list = downloads.list();
        assert_eq!((list[0].received, list[0].total), (15, None));
    }

    #[test]
    fn progress_is_announced_at_most_every_quarter_second() {
        let downloads = Downloads::new();
        let one = downloads.begin("Zwiim 0.6.0", 1);
        let mut rx = downloads.subscribe();
        rx.borrow_and_update();
        one.received(1);
        one.received(1);
        assert!(!rx.has_changed().unwrap());
        assert_eq!(downloads.list()[0].received, 2);
        *one.to.as_ref().unwrap().last.lock().unwrap() -= NOTIFY_EVERY;
        one.received(1);
        assert!(rx.has_changed().unwrap());
    }

    #[test]
    fn unreported_progress_lists_nothing() {
        let p = Progress::none();
        p.file(Some(1));
        p.received(1);
    }
}
