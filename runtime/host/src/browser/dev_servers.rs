//! Zwiim-owned: following each session's dev server in its browser tab
//! (browser design 10.5).
//!
//! The Host scans the processes it started itself, which include the agents
//! and everything they run, for loopback ports that answer HTTP. A new one is
//! matched to a session by its working directory: the worktree it runs in
//! names the session. That session's tab then shows it, by the rule in
//! [`choose`].
//!
//! Scanning rather than reading terminal output: an agent's own shell tool
//! runs `npm run dev` where the Host never sees what it prints.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::listener::listening;
use super::service::{BrowserService, TabInfo};
use crate::db::entities::{conversation, folder};

/// How often the Host looks for new servers.
pub const INTERVAL: Duration = Duration::from_secs(3);
/// How long a port may take to answer an HTTP request.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(1);

/// What a session's tabs do about a new dev server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// A tab already shows it: reload that tab, it was waiting for it.
    Reload(String),
    /// Show it in this blank tab, or in this tab whose server stopped.
    Navigate(String),
    /// Another server of the session is showing; the first one stays.
    Leave,
    /// No tab is free for it: open one.
    Open,
}

/// Where session `tabs` show the new server at `origin`. `stopped` holds the
/// tabs showing a loopback server that no longer answers.
///
/// Tabs on other sites are never taken, and a running server is never
/// replaced by another: an API server starting beside the app does not take
/// over the app's tab.
pub fn choose(tabs: &[TabInfo], origin: &str, stopped: &HashSet<String>) -> Choice {
    if let Some(tab) = tabs
        .iter()
        .find(|t| origin_of(&t.url).as_deref() == Some(origin))
    {
        return Choice::Reload(tab.id.clone());
    }
    if let Some(tab) = tabs
        .iter()
        .find(|t| stopped.contains(&t.id))
        .or_else(|| tabs.iter().find(|t| t.url == "about:blank"))
    {
        return Choice::Navigate(tab.id.clone());
    }
    if tabs.iter().any(|t| is_loopback(&t.url)) {
        return Choice::Leave;
    }
    Choice::Open
}

/// The origin to compare `url` by, with `localhost` and `127.0.0.1` alike.
pub fn origin_of(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    let port = super::listener::loopback_port(&url.origin().ascii_serialization())?;
    Some(format!("{}://localhost:{port}", url.scheme()))
}

/// Whether `url` is a page on this machine.
pub fn is_loopback(url: &str) -> bool {
    origin_of(url).is_some()
}

/// The processes below `root`, which the Host started. macOS only for now.
pub fn descendants(root: u32) -> Vec<u32> {
    let mut found = Vec::new();
    let mut next = vec![root];
    while let Some(pid) = next.pop() {
        for child in sys::children(pid) {
            if !found.contains(&child) {
                found.push(child);
                next.push(child);
            }
        }
    }
    found
}

/// Whether something speaking HTTP answers on loopback `port`.
pub async fn answers_http(port: u16) -> bool {
    let ask = async {
        let mut stream = match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
            Ok(stream) => stream,
            Err(_) => tokio::net::TcpStream::connect(("::1", port)).await.ok()?,
        };
        let request = format!("HEAD / HTTP/1.0\r\nHost: localhost:{port}\r\n\r\n");
        stream.write_all(request.as_bytes()).await.ok()?;
        let mut head = [0u8; 5];
        stream.read_exact(&mut head).await.ok()?;
        Some(&head == b"HTTP/")
    };
    matches!(
        tokio::time::timeout(ANSWER_TIMEOUT, ask).await,
        Ok(Some(true))
    )
}

/// Whether anything listens on loopback `port`.
pub async fn is_up(port: u16) -> bool {
    let connect = async {
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
            || tokio::net::TcpStream::connect(("::1", port)).await.is_ok()
    };
    tokio::time::timeout(ANSWER_TIMEOUT, connect)
        .await
        .unwrap_or(false)
}

/// Watch the Host's processes for new dev servers and show each in its
/// session's tab, while `browser.open_dev_servers` is on. `browser` makes
/// the browser service on first need.
///
/// Servers already running at the first scan, or when the setting is turned
/// on, are left alone: only a server that starts while watched is new.
pub async fn follow<F, Fut>(db: DatabaseConnection, browser: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Option<Arc<BrowserService>>>,
{
    let root = std::process::id();
    let mut known: Option<HashSet<(u32, u16)>> = None;
    let mut tick = tokio::time::interval(INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if !crate::settings::browser_open_dev_servers(&db).await {
            known = None;
            continue;
        }
        let found = tokio::task::spawn_blocking(move || listening(&descendants(root)))
            .await
            .unwrap_or_default();
        let now = found.iter().map(|l| (l.pid, l.port)).collect();
        let Some(before) = known.replace(now) else {
            continue;
        };
        for server in found
            .into_iter()
            .filter(|l| !before.contains(&(l.pid, l.port)))
        {
            let Some(dir) = server.workdir else { continue };
            if !answers_http(server.port).await {
                continue;
            }
            let Some(session) = session_of_dir(&db, &dir).await else {
                continue;
            };
            if let Some(service) = browser().await {
                service.show_dev_server(&session, server.port).await;
            }
        }
    }
}

/// The session a server running in `dir` belongs to: the latest top-level
/// session of the deepest worktree holding `dir`.
pub async fn session_of_dir(db: &DatabaseConnection, dir: &str) -> Option<String> {
    let folders = folder::Entity::find()
        .filter(folder::Column::DeletedAt.is_null())
        .all(db)
        .await
        .ok()?;
    let worktree = folders
        .into_iter()
        .filter(|f| Path::new(dir).starts_with(&f.path))
        .max_by_key(|f| f.path.len())?;
    conversation::Entity::find()
        .filter(conversation::Column::FolderId.eq(worktree.id))
        .filter(conversation::Column::DeletedAt.is_null())
        .filter(conversation::Column::ParentId.is_null())
        .filter(conversation::Column::SessionId.is_not_null())
        .order_by_desc(conversation::Column::UpdatedAt)
        .one(db)
        .await
        .ok()??
        .session_id
}

#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::c_void;
    use std::mem::size_of;

    /// `PROC_PPID_ONLY` from `<sys/proc_info.h>`.
    const PROC_PPID_ONLY: u32 = 6;
    const MAX_PIDS: usize = 1024;

    pub fn children(parent: u32) -> Vec<u32> {
        let mut pids = vec![0 as libc::pid_t; MAX_PIDS];
        let bytes = i32::try_from(pids.len() * size_of::<libc::pid_t>()).unwrap_or(i32::MAX);
        // SAFETY: the buffer holds `bytes` bytes, and the call writes at most that.
        let n = unsafe {
            libc::proc_listpids(
                PROC_PPID_ONLY,
                parent,
                pids.as_mut_ptr().cast::<c_void>(),
                bytes,
            )
        };
        let Ok(n) = usize::try_from(n) else {
            return Vec::new();
        };
        pids.truncate(n / size_of::<libc::pid_t>());
        pids.into_iter()
            .filter_map(|p| u32::try_from(p).ok().filter(|p| *p > 0))
            .collect()
    }
}

#[cfg(not(target_os = "macos"))]
mod sys {
    pub fn children(_parent: u32) -> Vec<u32> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::store::TabOwner;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_worktree_folder};
    use crate::models::agent::AgentType;

    fn tab(id: &str, url: &str) -> TabInfo {
        TabInfo {
            id: id.into(),
            owner: TabOwner::Session {
                session_id: "s".into(),
            },
            url: url.into(),
            title: String::new(),
            position: 0,
            loading: false,
            discarded: false,
            crashed: false,
            reload_on_save: false,
            preview: None,
        }
    }

    #[test]
    fn a_new_server_takes_a_free_tab_and_never_a_busy_one() {
        let none = HashSet::new();
        let app = "http://localhost:5173";
        // Already showing it, under either spelling.
        let tabs = [
            tab("a", "https://docs.rs/"),
            tab("b", "http://127.0.0.1:5173/x"),
        ];
        assert_eq!(choose(&tabs, app, &none), Choice::Reload("b".into()));
        // A blank tab is free.
        let tabs = [tab("a", "https://docs.rs/"), tab("b", "about:blank")];
        assert_eq!(choose(&tabs, app, &none), Choice::Navigate("b".into()));
        // A tab whose server stopped is free, before a blank one.
        let tabs = [tab("a", "about:blank"), tab("b", "http://localhost:5174/")];
        let stopped = HashSet::from(["b".to_string()]);
        assert_eq!(choose(&tabs, app, &stopped), Choice::Navigate("b".into()));
        // A running server keeps its tab.
        let tabs = [tab("a", "http://localhost:3000/")];
        assert_eq!(choose(&tabs, app, &none), Choice::Leave);
        // Other sites are the person's, and no tab is free.
        let tabs = [tab("a", "https://docs.rs/")];
        assert_eq!(choose(&tabs, app, &none), Choice::Open);
        assert_eq!(choose(&[], app, &none), Choice::Open);
    }

    #[test]
    fn loopback_origins_compare_alike() {
        assert_eq!(
            origin_of("http://127.0.0.1:5173/a?b").as_deref(),
            Some("http://localhost:5173")
        );
        assert_eq!(
            origin_of("http://[::1]:5173/").as_deref(),
            Some("http://localhost:5173")
        );
        assert_eq!(origin_of("https://example.com/"), None);
        assert_eq!(origin_of("about:blank"), None);
    }

    #[tokio::test]
    async fn only_a_port_that_speaks_http_counts() {
        let http = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http_port = http.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = http.accept().await.unwrap();
            let mut buf = [0u8; 256];
            let _ = s.read(&mut buf).await;
            let _ = s.write_all(b"HTTP/1.0 200 OK\r\n\r\n").await;
        });
        assert!(answers_http(http_port).await);

        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let silent_port = silent.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (s, _) = silent.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
            drop(s);
        });
        assert!(!answers_http(silent_port).await);
    }

    #[tokio::test]
    async fn a_directory_names_the_session_of_its_deepest_worktree() {
        let db = fresh_in_memory_db().await;
        let wt = seed_worktree_folder(&db, "/work/repo-wt").await;
        let conv = seed_conversation(&db, wt, AgentType::ClaudeCode).await;
        let sid = conversation::Entity::find_by_id(conv)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap()
            .session_id;
        assert!(sid.is_some());
        assert_eq!(
            session_of_dir(&db.conn, "/work/repo-wt/clients/web").await,
            sid
        );
        // The root `/work/repo-wt-root` has no session, and a lookalike path
        // is not inside the worktree.
        assert_eq!(session_of_dir(&db.conn, "/work/repo-wt-root").await, None);
        assert_eq!(session_of_dir(&db.conn, "/work/repo-wt2").await, None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_processes_this_one_started_are_found() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let found = descendants(std::process::id());
        let _ = child.kill();
        let _ = child.wait();
        assert!(found.contains(&child.id()));
    }
}
