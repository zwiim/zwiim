//! Host lifecycle: the instance lock, restart when idle, and the drain.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use sea_orm::EntityTrait;
use serde_json::{json, Value};

use super::*;
use crate::acp::connection::ConnectionCommand;
use crate::acp::types::AcpEvent;
use crate::protocol::tests::{host, Host, CONN, HOST_TOKEN};
use crate::web::event_bridge::emit_with_state;

async fn post(h: &Host, op: &str, body: Value) -> (StatusCode, Value) {
    let res = h
        .server
        .post(&format!("/zwiim/v1/{op}"))
        .authorization_bearer(HOST_TOKEN)
        .json(&body)
        .await;
    (res.status_code(), res.json::<Value>())
}

async fn set_turn(h: &Host, running: bool) {
    let st = h.state.connection_manager.get_state(CONN).await.unwrap();
    st.write().await.turn_in_flight = running;
}

async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..100 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

// ─── instance lock ──────────────────────────────────────────────────────

#[test]
fn one_host_per_data_directory() {
    let dir = tempfile::tempdir().unwrap();
    let first = InstanceLock::acquire(dir.path()).unwrap();
    assert_eq!(first.path(), dir.path().join(LOCK_FILE));

    // A second Host is refused and told who holds it.
    match InstanceLock::acquire(dir.path()) {
        Err(LockError::Held { pid }) => assert_eq!(pid, Some(std::process::id())),
        other => panic!("expected Held, got {other:?}"),
    }

    // Released with its holder; a crash releases it the same way (the OS
    // drops the lock with the process).
    drop(first);
    let second = InstanceLock::acquire(dir.path()).unwrap();
    drop(second);
}

#[test]
fn the_lock_creates_a_missing_data_directory() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("fresh").join("data");
    let lock = InstanceLock::acquire(&data).unwrap();
    assert!(lock.path().exists());
}

// ─── lifecycle state ────────────────────────────────────────────────────

#[test]
fn a_stop_overrides_a_pending_restart_but_not_the_reverse() {
    let lc = Lifecycle::default();
    assert_eq!(lc.requested(), None);
    lc.request(ShutdownKind::Restart);
    lc.request(ShutdownKind::Stop);
    assert_eq!(lc.requested(), Some(ShutdownKind::Stop));
    lc.request(ShutdownKind::Restart);
    assert_eq!(lc.requested(), Some(ShutdownKind::Stop));
}

#[tokio::test]
async fn wait_for_request_resolves_with_the_request() {
    let lc = Arc::new(Lifecycle::default());
    let waiter = tokio::spawn({
        let lc = lc.clone();
        async move { lc.wait_for_request().await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!waiter.is_finished());
    lc.request(ShutdownKind::Restart);
    assert_eq!(waiter.await.unwrap(), ShutdownKind::Restart);
    // Already requested: resolves at once.
    assert_eq!(lc.wait_for_request().await, ShutdownKind::Restart);
}

#[tokio::test]
async fn a_drain_refuses_new_operations_and_waits_for_admitted_ones() {
    let lc = Arc::new(Lifecycle::default());
    let admitted = lc.begin_request().expect("admitted before the drain");
    lc.start_draining();
    assert!(lc.begin_request().is_none(), "refused once draining");

    let waiting = tokio::spawn({
        let lc = lc.clone();
        async move { lc.wait_for_requests(Duration::from_secs(5)).await }
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!waiting.is_finished(), "waits for the admitted operation");
    drop(admitted);
    assert_eq!(waiting.await.unwrap(), 0);
}

#[tokio::test]
async fn a_drain_gives_up_on_an_operation_that_never_ends() {
    let lc = Arc::new(Lifecycle::default());
    let _stuck = lc.begin_request().unwrap();
    lc.start_draining();
    assert_eq!(lc.wait_for_requests(Duration::from_millis(30)).await, 1);
}

// ─── host.restart ───────────────────────────────────────────────────────

#[tokio::test]
async fn an_idle_host_restarts_at_once() {
    let h = host(false).await;
    let (status, body) = post(&h, "host.restart", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"state": "restarting", "blockers": {"live_turns": 0, "pending_requests": 0, "terminals": 0, "starting": 0, "session_ids": []}})
    );
    assert_eq!(h.state.lifecycle.requested(), Some(ShutdownKind::Restart));
}

#[tokio::test]
async fn a_running_turn_holds_a_when_idle_restart_until_it_ends() {
    let h = host(false).await;
    let (sid, _rx) = h.live_session().await;
    set_turn(&h, true).await;

    let (status, body) = post(&h, "host.restart", json!({"mode": "when_idle"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["state"], "restart_pending");
    assert_eq!(body["blockers"]["live_turns"], 1);
    assert_eq!(body["blockers"]["session_ids"], json!([sid]));
    assert!(h.state.lifecycle.restart_pending());
    // A client that connects later learns the restart is already asked for.
    let (_, update) = post(&h, "host.update.status", json!({})).await;
    assert_eq!(update["restart_pending"], true);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.state.lifecycle.requested(), None, "nothing interrupted");
    // New work is still accepted while the restart waits.
    assert_eq!(
        post(&h, "workspace.list", json!({})).await.0,
        StatusCode::OK
    );

    // Asking again doesn't start a second watcher or change the answer.
    let (_, again) = post(&h, "host.restart", json!({})).await;
    assert_eq!(again["state"], "restart_pending");

    set_turn(&h, false).await;
    let lc = h.state.lifecycle.clone();
    eventually("the restart", || {
        lc.requested() == Some(ShutdownKind::Restart)
    })
    .await;
}

#[tokio::test]
async fn a_request_waiting_on_the_user_holds_a_when_idle_restart() {
    let h = host(false).await;
    let (_sid, mut rx) = h.live_session().await;
    let (_, _) = post(
        &h,
        "session.send",
        json!({"session_id": _sid, "blocks": [{"type": "text", "text": "hi"}]}),
    )
    .await;
    assert!(matches!(
        rx.recv().await,
        Some(ConnectionCommand::Prompt { .. })
    ));
    h.emit(AcpEvent::PermissionRequest {
        request_id: "perm-1".into(),
        tool_call: json!({"toolCallId": "tc-1", "title": "Run ls"}),
        options: vec![],
        queued: 0,
    })
    .await;
    let (_, body) = post(&h, "host.restart", json!({})).await;
    assert_eq!(body["state"], "restart_pending");
    assert_eq!(body["blockers"]["pending_requests"], 1);
}

#[tokio::test]
async fn an_open_terminal_does_not_hold_a_when_idle_restart() {
    let h = host(false).await;
    let (_, spawned) = post(
        &h,
        "terminal.spawn",
        json!({"workspace_id": h.wt_workspace_id, "shell": "/bin/sh"}),
    )
    .await;
    let tid = spawned["terminal_id"].as_str().unwrap().to_string();

    // Terminals are reported, but only running agents hold the restart.
    let (_, body) = post(&h, "host.restart", json!({})).await;
    assert_eq!(body["state"], "restarting");
    assert_eq!(body["blockers"]["terminals"], 1);
    assert_eq!(h.state.lifecycle.requested(), Some(ShutdownKind::Restart));

    // Don't leave the shell running past the test.
    assert_eq!(
        post(&h, "terminal.kill", json!({"terminal_id": tid}))
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_session_starting_a_prompt_holds_a_when_idle_restart() {
    let h = host(false).await;
    let (sid, _rx) = h.live_session().await;
    let guard = crate::session::session_lock(&sid).lock_owned().await;

    let (_, body) = post(&h, "host.restart", json!({"mode": "when_idle"})).await;
    assert_eq!(body["state"], "restart_pending");
    assert_eq!(body["blockers"]["starting"], 1);
    assert_eq!(body["blockers"]["session_ids"], json!([sid]));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.state.lifecycle.requested(), None, "nothing interrupted");

    drop(guard);
    let lc = h.state.lifecycle.clone();
    eventually("the restart", || {
        lc.requested() == Some(ShutdownKind::Restart)
    })
    .await;
}

#[tokio::test]
async fn a_new_session_being_started_holds_a_when_idle_restart() {
    let h = host(false).await;
    let guard = crate::session::hold_start(&h.wt_workspace_id);

    let (_, body) = post(&h, "host.restart", json!({"mode": "when_idle"})).await;
    assert_eq!(body["state"], "restart_pending");
    assert!(body["blockers"]["starting"].as_u64().unwrap() >= 1);
    // It has no id yet: counted, not listed.
    assert_eq!(body["blockers"]["session_ids"], json!([]));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.state.lifecycle.requested(), None, "nothing interrupted");

    drop(guard);
    let lc = h.state.lifecycle.clone();
    eventually("the restart", || {
        lc.requested() == Some(ShutdownKind::Restart)
    })
    .await;
}

#[tokio::test]
async fn status_reports_blockers() {
    let h = host(false).await;
    let (sid, _rx) = h.live_session().await;
    set_turn(&h, true).await;

    let (status, body) = post(&h, "host.update.status", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["blockers"]["live_turns"], 1);
    assert_eq!(body["blockers"]["session_ids"], json!([sid]));
    assert_eq!(body["restart_pending"], false, "reading arms nothing");
    assert!(!h.state.lifecycle.restart_pending());
}

#[tokio::test]
async fn restart_takes_only_known_modes() {
    let h = host(false).await;
    let (status, body) = post(&h, "host.restart", json!({"mode": "eventually"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_request");
    assert_eq!(h.state.lifecycle.requested(), None);
}

// ─── drain ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_forced_restart_ends_work_visibly_and_keeps_sessions() {
    let h = host(false).await;
    let (sid, mut rx) = h.live_session().await;
    post(
        &h,
        "session.send",
        json!({"session_id": sid, "blocks": [{"type": "text", "text": "hi"}]}),
    )
    .await;
    assert!(matches!(
        rx.recv().await,
        Some(ConnectionCommand::Prompt { .. })
    ));
    h.emit(AcpEvent::PermissionRequest {
        request_id: "perm-1".into(),
        tool_call: json!({"toolCallId": "tc-1", "title": "Run ls"}),
        options: vec![],
        queued: 0,
    })
    .await;
    let (_, spawned) = post(
        &h,
        "terminal.spawn",
        json!({"workspace_id": h.wt_workspace_id, "shell": "/bin/sh"}),
    )
    .await;
    assert!(spawned["terminal_id"].is_string());

    // The agent ends its turn when told to cancel, as a real one does.
    let (state, emitter) = h
        .state
        .connection_manager
        .get_state_and_emitter(CONN)
        .await
        .unwrap();
    let agent = tokio::spawn(async move {
        let mut seen = Vec::new();
        while let Some(cmd) = rx.recv().await {
            match cmd {
                ConnectionCommand::Cancel => {
                    seen.push("cancel");
                    emit_with_state(
                        &state,
                        &emitter,
                        AcpEvent::TurnComplete {
                            session_id: "native".into(),
                            stop_reason: "cancelled".into(),
                            agent_type: "claude_code".into(),
                        },
                    )
                    .await;
                }
                ConnectionCommand::Disconnect => {
                    seen.push("disconnect");
                    return seen;
                }
                _ => seen.push("other"),
            }
        }
        seen
    });

    let (status, body) = post(&h, "host.restart", json!({"mode": "now"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["state"], "restarting");
    assert_eq!(body["blockers"]["live_turns"], 1);
    assert_eq!(body["blockers"]["pending_requests"], 1);
    assert_eq!(body["blockers"]["terminals"], 1);
    assert_eq!(h.state.lifecycle.requested(), Some(ShutdownKind::Restart));

    // What the binary does next.
    let report = drain(&h.state).await;
    assert_eq!(
        report,
        DrainReport {
            cancelled_turns: 1,
            declined_requests: 1,
            terminals_closed: 1,
            agents_stopped: 1,
        }
    );
    assert_eq!(agent.await.unwrap(), ["cancel", "disconnect"]);
    assert!(h.state.web_server_state.shutdown_signal().is_triggered());
    assert!(blockers(&h.state).await.is_idle());

    // New work is refused in the Zwiim shape; the handshake still answers.
    let (status, body) = post(&h, "session.send", json!({"session_id": sid, "blocks": []})).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body,
        json!({"error": {"code": "host_restarting", "message": "The host is restarting", "details": {"retry_after_ms": 3000}}})
    );
    assert_eq!(post(&h, "host.hello", json!({})).await.0, StatusCode::OK);

    // The session is kept for the next process to resume.
    persist(&h.state).await;
    let kept = crate::db::entities::conversation::Entity::find()
        .all(&h.state.db.conn)
        .await
        .unwrap();
    assert!(kept
        .iter()
        .any(|c| c.session_id.as_deref() == Some(sid.as_str())));
}

#[tokio::test]
async fn draining_an_idle_host_is_quick_and_quiet() {
    let h = host(false).await;
    let started = std::time::Instant::now();
    assert_eq!(drain(&h.state).await, DrainReport::default());
    assert!(started.elapsed() < Duration::from_secs(1));
}
