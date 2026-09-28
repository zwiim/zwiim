//! The Host binary's lifecycle, end to end: one Host per data directory, a
//! stop signal drains and exits, and an unsupervised restart comes back in
//! place. Each Host runs in its own temporary home and data directory on a
//! free port, with file-backed keys.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use zwiim_host::host_lifecycle::{InstanceLock, EXIT_ALREADY_RUNNING, EXIT_DATA_TOO_NEW};
use zwiim_host::local_host::LocalHostInfo;

struct Sandbox {
    _dir: tempfile::TempDir,
    home: PathBuf,
    data: PathBuf,
    port: u16,
}

fn sandbox() -> Sandbox {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let data = dir.path().join("data");
    std::fs::create_dir_all(&home).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    Sandbox {
        _dir: dir,
        home,
        data,
        port,
    }
}

fn host_command(s: &Sandbox) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zwiim-host"));
    cmd.env("HOME", &s.home)
        .env("ZWIIM_DATA_DIR", &s.data)
        .env("ZWIIM_PORT", s.port.to_string())
        .env("ZWIIM_HOST", "127.0.0.1")
        .env("ZWIIM_HOST_KEY_STORE", "file")
        .env("ZWIIM_WORKTREES_DIR", s.home.join("worktrees"))
        .env("ZWIIM_ADAPTER_SETUP", "manual")
        .env_remove("ZWIIM_HOME")
        .env_remove("ZWIIM_TOKEN")
        .env_remove("ZWIIM_SUPERVISED")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd
}

fn wait_exit(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

fn discovery(data: &Path) -> Option<LocalHostInfo> {
    let bytes = std::fs::read(zwiim_host::local_host::path(data)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn hello(info: &LocalHostInfo) -> Option<Value> {
    let res = reqwest::Client::new()
        .post(format!("{}/zwiim/v1/host.hello", info.url))
        .bearer_auth(&info.token)
        .json(&json!({}))
        .send()
        .await
        .ok()?;
    res.status().is_success().then_some(())?;
    res.json().await.ok()
}

/// Wait for the Host to publish itself and answer the handshake.
async fn wait_ready(data: &Path, child: &mut Child) -> LocalHostInfo {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("host exited before it was ready: {status}");
        }
        if let Some(info) = discovery(data) {
            if hello(&info).await.is_some() {
                return info;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("host never became ready");
}

fn terminate(child: &Child) {
    // SAFETY: signalling our own child process.
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
}

#[test]
fn a_second_host_exits_without_touching_the_database() {
    let s = sandbox();
    let _running = InstanceLock::acquire(&s.data).unwrap();

    let mut child = host_command(&s).spawn().unwrap();
    let status = wait_exit(&mut child, Duration::from_secs(30)).expect("exits by itself");
    assert_eq!(status.code(), Some(i32::from(EXIT_ALREADY_RUNNING)));

    // No database, no migrations, no discovery file of its own.
    let entries: Vec<String> = std::fs::read_dir(&s.data)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !entries
            .iter()
            .any(|name| name.ends_with(".db") || name.contains(".db-")),
        "database touched: {entries:?}"
    );
    assert!(discovery(&s.data).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_signal_drains_and_exits_cleanly() {
    let s = sandbox();
    let mut child = host_command(&s).spawn().unwrap();
    wait_ready(&s.data, &mut child).await;

    // While it runs, a second Host is refused.
    let mut second = host_command(&s).spawn().unwrap();
    let status = wait_exit(&mut second, Duration::from_secs(30)).expect("exits by itself");
    assert_eq!(status.code(), Some(i32::from(EXIT_ALREADY_RUNNING)));
    assert!(
        discovery(&s.data).is_some(),
        "the running Host's file is left alone"
    );

    terminate(&child);
    let status = wait_exit(&mut child, Duration::from_secs(30)).expect("stops after draining");
    assert_eq!(status.code(), Some(0), "{status}");
    assert!(
        discovery(&s.data).is_none(),
        "a stopped Host is not discoverable"
    );
    // The data directory is free for the next Host.
    InstanceLock::acquire(&s.data).expect("lock released");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unsupervised_restart_comes_back_in_place() {
    let s = sandbox();
    let mut child = host_command(&s).spawn().unwrap();
    let before = wait_ready(&s.data, &mut child).await;
    let host_id = hello(&before).await.unwrap()["host_id"].clone();
    let written = || {
        std::fs::metadata(zwiim_host::local_host::path(&s.data))
            .and_then(|m| m.modified())
            .ok()
    };
    let first_written = written();

    let res: Value = reqwest::Client::new()
        .post(format!("{}/zwiim/v1/host.restart", before.url))
        .bearer_auth(&before.token)
        .json(&json!({"mode": "when_idle"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res["state"], "restarting", "{res}");

    // The re-executed Host republishes itself when it is back.
    let deadline = Instant::now() + Duration::from_secs(60);
    let after = loop {
        assert!(Instant::now() < deadline, "host never came back");
        assert!(
            child.try_wait().unwrap().is_none(),
            "the process exited instead of restarting"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        if written() == first_written {
            continue;
        }
        if let Some(info) = discovery(&s.data) {
            if let Some(h) = hello(&info).await {
                assert_eq!(h["host_id"], host_id, "same identity");
                break info;
            }
        }
    };
    // Same process (re-executed in place), same token, same data.
    assert_eq!(after.pid, before.pid);
    assert_eq!(after.token, before.token);

    terminate(&child);
    let status = wait_exit(&mut child, Duration::from_secs(30)).expect("stops");
    assert_eq!(status.code(), Some(0));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_supervised_restart_is_relaunched_by_the_supervisor() {
    let s = sandbox();
    let mut supervisor = host_command(&s)
        .arg("--supervise")
        .env("ZWIIM_RESTART_DELAY_MS", "100")
        .spawn()
        .unwrap();
    let before = wait_ready(&s.data, &mut supervisor).await;
    assert_ne!(before.pid, supervisor.id(), "the worker is a child");

    let res: Value = reqwest::Client::new()
        .post(format!("{}/zwiim/v1/host.restart", before.url))
        .bearer_auth(&before.token)
        .json(&json!({"mode": "now"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res["state"], "restarting", "{res}");

    // A new worker, same Host.
    let deadline = Instant::now() + Duration::from_secs(60);
    let after = loop {
        assert!(Instant::now() < deadline, "worker never came back");
        assert!(
            supervisor.try_wait().unwrap().is_none(),
            "the supervisor exited"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        match discovery(&s.data) {
            Some(info) if info.pid != before.pid && hello(&info).await.is_some() => break info,
            _ => {}
        }
    };
    assert_eq!(after.token, before.token);

    // Stopping the supervisor drains the worker and ends both.
    terminate(&supervisor);
    let status = wait_exit(&mut supervisor, Duration::from_secs(30)).expect("stops");
    assert_eq!(status.code(), Some(0));
    assert!(discovery(&s.data).is_none());
    InstanceLock::acquire(&s.data).expect("lock released");
}

#[tokio::test(flavor = "multi_thread")]
async fn data_from_a_newer_zwiim_is_refused_untouched() {
    use sea_orm::ConnectionTrait;
    let s = sandbox();
    // A data directory a newer Zwiim has migrated past this binary.
    let db = zwiim_host::db::init_database(&s.data, "test")
        .await
        .unwrap();
    db.conn
        .execute_unprepared(
            "INSERT INTO seaql_migrations (version, applied_at) VALUES ('m29990101_000001_from_the_future', 0)",
        )
        .await
        .unwrap();
    db.conn
        .execute_unprepared("PRAGMA wal_checkpoint(TRUNCATE)")
        .await
        .unwrap();
    db.conn.close().await.unwrap();
    let file = s.data.join("zwiim.db");
    let before = std::fs::read(&file).unwrap();

    let mut child = host_command(&s).spawn().unwrap();
    let status = wait_exit(&mut child, Duration::from_secs(60)).expect("exits by itself");
    assert_eq!(status.code(), Some(i32::from(EXIT_DATA_TOO_NEW)));
    assert_eq!(std::fs::read(&file).unwrap(), before, "not a byte changed");
    assert!(discovery(&s.data).is_none(), "never served");
    assert!(
        zwiim_host::db::schema_guard::snapshots(&s.data).is_empty(),
        "no snapshot of data it cannot read"
    );
}
