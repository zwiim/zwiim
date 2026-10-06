//! End-to-end tests of the `/zwiim/v1` boundary through the real router.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum_test::{TestServer, TestWebSocket};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Value};

use super::error::ZwiimError;
use super::events::tests::assert_no_internal_keys;
use crate::acp::connection::ConnectionCommand;
use crate::acp::types::{AcpEvent, PermissionOptionInfo};
use crate::app_state::AppState;
use crate::db::test_helpers::{seed_conversation, seed_folder};
use crate::models::agent::AgentType;
use crate::web::event_bridge::emit_with_state;
use crate::web::router::build_router;
use crate::web::shutdown::ShutdownSignal;

pub(crate) const HOST_TOKEN: &str = "host-token";
pub(crate) const CONN: &str = "conn-secret-7f3a";

const OPS: &[&str] = &[
    "host.hello",
    "host.lan_endpoint",
    "host.claim",
    "host.restart",
    "host.update.status",
    "host.update.check",
    "host.update.apply",
    "host.diagnostics",
    "agent.list",
    "agent.setup",
    "agent.auth_status",
    "agent.sign_in",
    "agent.save_key",
    "agent.remove_key",
    "agent.sign_out",
    "workspace.list",
    "workspace.add",
    "workspace.browse",
    "workspace.create_folder",
    "workspace.open",
    "workspace.tree",
    "workspace.create_worktree",
    "workspace.remove_worktree",
    "workspace.continue",
    "files.read",
    "files.write",
    "files.create",
    "git.status",
    "git.log",
    "git.branches",
    "git.commit",
    "git.diff",
    "git.pr",
    "git.pr_merge",
    "terminal.spawn",
    "terminal.write",
    "terminal.resize",
    "terminal.kill",
    "terminal.list",
    "terminal.snapshot",
    "session.start",
    "session.send",
    "session.cancel",
    "session.stop",
    "session.delete",
    "session.respond",
    "session.options",
    "session.set_options",
    "session.list",
    "session.history",
    "device.pair_invite",
    "device.list",
    "device.revoke",
    "cloud.enroll",
    "cloud.status",
    "lan.status",
    "lan.set",
    "host.keep_awake.status",
    "host.keep_awake.set",
];

pub(crate) fn git(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?}");
}

pub(crate) struct Host {
    pub(crate) _dir: tempfile::TempDir,
    pub(crate) root: String,
    pub(crate) workspace_id: String,
    pub(crate) folder_id: i32,
    pub(crate) state: Arc<AppState>,
    pub(crate) server: TestServer,
}

pub(crate) async fn host(http_transport: bool) -> Host {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
    let data_dir = dir.path().join("data");
    std::fs::create_dir(&data_dir).unwrap();
    let db = crate::db::init_database(&data_dir, "test").await.unwrap();
    let root = repo.canonicalize().unwrap().to_string_lossy().into_owned();
    let folder_id = seed_folder(&db, &root).await;
    let state = Arc::new(AppState::new_for_test(db, data_dir.clone()));
    let workspace_id = crate::workspace::workspace_id_of(&state.db.conn, folder_id)
        .await
        .unwrap()
        .unwrap();
    let app = build_router(
        state.clone(),
        HOST_TOKEN.to_string(),
        data_dir,
        Arc::new(ShutdownSignal::new()),
    );
    let server = if http_transport {
        TestServer::builder().http_transport().build(app).unwrap()
    } else {
        TestServer::new(app).unwrap()
    };
    Host {
        _dir: dir,
        root,
        workspace_id,
        folder_id,
        state,
        server,
    }
}

impl Host {
    async fn call(&self, op: &str, body: Value) -> (StatusCode, Value) {
        let res = self
            .server
            .post(&format!("/zwiim/v1/{op}"))
            .authorization_bearer(HOST_TOKEN)
            .json(&body)
            .await;
        let status = res.status_code();
        let text = res.text();
        let tmp = self._dir.path();
        assert!(
            !text.contains(&self.root)
                && !text.contains(tmp.to_str().unwrap())
                && !text.contains(tmp.canonicalize().unwrap().to_str().unwrap())
                && !text.contains(CONN),
            "{op} leaked a host path or connection id: {text}"
        );
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        assert_no_internal_keys(&value);
        (status, value)
    }

    pub(crate) async fn ok(&self, op: &str, body: Value) -> Value {
        let (status, value) = self.call(op, body).await;
        assert_eq!(status, StatusCode::OK, "{op}: {value}");
        value
    }

    async fn err(&self, op: &str, body: Value) -> (StatusCode, String) {
        let (status, code, details) = self.err_details(op, body).await;
        // A field the Host does not know is always named (R9.1).
        let named_field = details
            .as_ref()
            .is_some_and(|d| d["reason"] == "unknown_field" && d["field"].is_string());
        assert!(details.is_none() || named_field, "{op}: unexpected details {details:?}");
        (status, code)
    }

    /// Like [`Host::err`] for the codes that document a `details` object.
    async fn err_details(&self, op: &str, body: Value) -> (StatusCode, String, Option<Value>) {
        let (status, value) = self.call(op, body).await;
        assert!(!status.is_success(), "{op} unexpectedly succeeded: {value}");
        let e = value["error"].as_object().expect("zwiim error shape");
        assert!(
            e.keys()
                .all(|k| ["code", "message", "details"].contains(&k.as_str())),
            "error carries only code + message (+ details): {value}"
        );
        assert!(e["message"].is_string());
        (
            status,
            e["code"].as_str().unwrap().to_string(),
            e.get("details").cloned(),
        )
    }

    /// A seeded session bound to an in-process live agent connection.
    pub(crate) async fn live_session(&self) -> (String, tokio::sync::mpsc::Receiver<ConnectionCommand>) {
        let conv = seed_conversation(&self.state.db, self.folder_id, AgentType::ClaudeCode).await;
        let rx = self
            .state
            .connection_manager
            .insert_test_connection_live(
                CONN,
                AgentType::ClaudeCode,
                Some(self.root.clone().into()),
                self.state.emitter.clone(),
            )
            .await;
        let st = self.state.connection_manager.get_state(CONN).await.unwrap();
        st.write().await.conversation_id = Some(conv);
        let sid = session_id_of(&self.state, conv).await;
        (sid, rx)
    }

    pub(crate) async fn emit(&self, event: AcpEvent) {
        let (state, emitter) = self
            .state
            .connection_manager
            .get_state_and_emitter(CONN)
            .await
            .unwrap();
        emit_with_state(&state, &emitter, event).await;
    }

    async fn ws(&self) -> TestWebSocket {
        self.ws_as("/zwiim/v1/ws", HOST_TOKEN).await
    }

    async fn ws_as(&self, path: &str, token: &str) -> TestWebSocket {
        let proto = format!(
            "zwiim-events, zwiim-token.{}",
            URL_SAFE_NO_PAD.encode(token)
        );
        self.server
            .get_websocket(path)
            .add_header("sec-websocket-protocol", proto)
            .await
            .into_websocket()
            .await
    }

    /// `op` called with `bearer` instead of the host token.
    async fn call_as(&self, bearer: &str, op: &str, body: Value) -> (StatusCode, Value) {
        let res = self
            .server
            .post(&format!("/zwiim/v1/{op}"))
            .authorization_bearer(bearer)
            .json(&body)
            .await;
        (res.status_code(), res.json::<Value>())
    }
}

pub(crate) fn delta(text: &str) -> AcpEvent {
    AcpEvent::ContentDelta {
        text: text.into(),
        parent_tool_use_id: None,
    }
}

pub(crate) async fn session_id_of(state: &AppState, conversation_id: i32) -> String {
    use sea_orm::EntityTrait;
    crate::db::entities::conversation::Entity::find_by_id(conversation_id)
        .one(&state.db.conn)
        .await
        .unwrap()
        .unwrap()
        .session_id
        .unwrap()
}

async fn recv(ws: &mut TestWebSocket) -> Value {
    let text = tokio::time::timeout(Duration::from_secs(5), ws.receive_text())
        .await
        .expect("frame within 5s");
    assert!(!text.contains(CONN), "frame leaked connection id: {text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_no_internal_keys(&v);
    v
}

async fn assert_silent(ws: &mut TestWebSocket) {
    assert!(
        tokio::time::timeout(Duration::from_millis(200), ws.receive_text())
            .await
            .is_err(),
        "expected no frame"
    );
}

// ─── HTTP ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn every_operation_requires_auth_and_rejects_in_zwiim_shape() {
    let h = host(false).await;
    for op in OPS {
        for req in [
            h.server.post(&format!("/zwiim/v1/{op}")).json(&json!({})),
            h.server
                .post(&format!("/zwiim/v1/{op}"))
                .authorization_bearer("wrong")
                .json(&json!({})),
        ] {
            let res = req.await;
            assert_eq!(res.status_code(), StatusCode::UNAUTHORIZED, "{op}");
            assert_eq!(
                res.json::<Value>(),
                json!({"error": {"code": "unauthenticated", "message": "Missing or invalid credentials"}}),
                "{op}"
            );
        }
    }
    let ws = h.server.get("/zwiim/v1/ws").await;
    assert_eq!(ws.status_code(), StatusCode::UNAUTHORIZED);
    assert_eq!(ws.json::<Value>()["error"]["code"], "unauthenticated");

    // Authenticated: a GET is not an operation, unknown ops are named as such.
    let (status, code) = {
        let res = h
            .server
            .get("/zwiim/v1/host.hello")
            .authorization_bearer(HOST_TOKEN)
            .await;
        (
            res.status_code(),
            res.json::<Value>()["error"]["code"].clone(),
        )
    };
    assert_eq!(
        (status, code),
        (StatusCode::METHOD_NOT_ALLOWED, json!("method_not_allowed"))
    );
    assert_eq!(
        h.err("session.fork", json!({})).await,
        (StatusCode::NOT_FOUND, "unknown_operation".into())
    );
    // Legacy `/api` keeps its own shape.
    let legacy = h.server.post("/api/workspace_list").await;
    assert_eq!(legacy.status_code(), StatusCode::UNAUTHORIZED);
    assert!(serde_json::from_str::<Value>(&legacy.text()).is_err());
}

#[tokio::test]
async fn malformed_requests_are_normalized() {
    let h = host(false).await;
    let wid = h.workspace_id.clone();

    let res = h
        .server
        .post("/zwiim/v1/workspace.open")
        .authorization_bearer(HOST_TOKEN)
        .content_type("application/json")
        .bytes("{not json".into())
        .await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);
    assert_eq!(res.json::<Value>()["error"]["code"], "invalid_request");

    let res = h
        .server
        .post("/zwiim/v1/workspace.open")
        .authorization_bearer(HOST_TOKEN)
        .text(format!("{{\"workspace_id\":\"{wid}\"}}"))
        .await;
    assert_eq!(res.json::<Value>()["error"]["code"], "invalid_request");

    for (op, body) in [
        ("workspace.open", json!({})),
        (
            "workspace.open",
            json!({"workspace_id": wid, "folder_id": 1}),
        ),
        ("files.read", json!({"workspace_id": wid, "path": 3})),
        (
            "terminal.resize",
            json!({"terminal_id": "t", "cols": -1, "rows": 2}),
        ),
        (
            "session.start",
            json!({"workspace_id": wid, "agent": "nope", "blocks": []}),
        ),
        (
            "session.send",
            json!({"session_id": "s", "blocks": [{"type": "resource", "uri": "file:///etc/passwd"}]}),
        ),
        (
            "session.respond",
            json!({"session_id": "s", "request_id": "r", "kind": "telepathy"}),
        ),
    ] {
        assert_eq!(
            h.err(op, body.clone()).await,
            (StatusCode::BAD_REQUEST, "invalid_request".into()),
            "{op} {body}"
        );
    }

    assert_eq!(
        h.err(
            "session.start",
            json!({"workspace_id": wid, "agent": "claude_code", "blocks": []})
        )
        .await,
        (StatusCode::BAD_REQUEST, "invalid_input".into())
    );
    // A traversal is refused without echoing the host path.
    assert_eq!(
        h.err(
            "files.read",
            json!({"workspace_id": wid, "path": "../../etc/passwd"})
        )
        .await
        .1,
        "invalid_input"
    );
    assert_eq!(
        h.err(
            "files.read",
            json!({"workspace_id": wid, "path": h.root.clone() + "/x"})
        )
        .await
        .1,
        "invalid_input"
    );
}

#[tokio::test]
async fn workspace_add_registers_and_revives_by_path() {
    let h = host(false).await;
    let other = h._dir.path().join("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("f.txt"), "x").unwrap();
    let other = other.canonicalize().unwrap();
    let other_str = other.to_string_lossy().into_owned();

    let added = h.ok("workspace.add", json!({"path": other_str})).await;
    let wid = added["workspace_id"].as_str().unwrap().to_string();
    assert_ne!(wid, h.workspace_id);
    assert_eq!(added["name"], "other");
    assert_eq!(added["kind"], "root");
    assert!(added.get("path").is_none());
    assert!(
        !added.to_string().contains(&other_str),
        "path echoed: {added}"
    );
    let list = h.ok("workspace.list", json!({})).await;
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["workspace_id"] == wid.as_str()));

    // Same directory, spelled differently: same workspace.
    let again = h
        .ok(
            "workspace.add",
            json!({"path": format!("{other_str}/../other/")}),
        )
        .await;
    assert_eq!(again["workspace_id"], wid.as_str());
    // An already-known workspace keeps its identity.
    let root = h.ok("workspace.add", json!({"path": h.root.clone()})).await;
    assert_eq!(root["workspace_id"], h.workspace_id.as_str());

    assert_eq!(
        h.err("workspace.add", json!({"path": "relative/dir"}))
            .await,
        (StatusCode::BAD_REQUEST, "invalid_input".into())
    );
    assert_eq!(
        h.err(
            "workspace.add",
            json!({"path": format!("{other_str}/missing")})
        )
        .await,
        (StatusCode::NOT_FOUND, "not_found".into())
    );
    assert_eq!(
        h.err(
            "workspace.add",
            json!({"path": format!("{other_str}/f.txt")})
        )
        .await,
        (StatusCode::NOT_FOUND, "not_found".into())
    );
}

#[tokio::test]
async fn workspace_add_takes_one_form_and_host_paths_only_from_the_host() {
    let h = host(false).await;
    let other = h._dir.path().join("other").to_string_lossy().into_owned();
    std::fs::create_dir(&other).unwrap();
    for body in [
        json!({}),
        json!({"path": other.clone(), "home_path": ["Projects"]}),
        json!({"home_path": []}),
        json!({"home_path": [".."]}),
        json!({"home_path": ["a/b"]}),
    ] {
        assert_eq!(
            h.err("workspace.add", body.clone()).await,
            (StatusCode::BAD_REQUEST, "invalid_input".into()),
            "{body}"
        );
    }

    // A paired device never sends a host path.
    let credential = bearer_device(&h, "dev-1").await;
    let res = h
        .server
        .post("/zwiim/v1/workspace.add")
        .authorization_bearer(&credential)
        .json(&json!({ "path": other }))
        .await;
    assert_eq!(res.status_code(), StatusCode::FORBIDDEN);
    let body: Value = res.json();
    assert_eq!(body["error"]["code"], "permission_denied");
    let listed = h.ok("workspace.list", json!({})).await;
    assert_eq!(listed.as_array().unwrap().len(), 1, "nothing added");
}

fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[tokio::test]
async fn workspace_add_makes_a_plain_folder_a_repository() {
    let h = host(false).await;
    let dir = h._dir.path().join("app");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/a.rs"), "fn main() {}\n").unwrap();
    std::fs::write(dir.join(".env"), "KEY=secret\n").unwrap();
    let path = dir.canonicalize().unwrap().to_string_lossy().into_owned();

    let added = h.ok("workspace.add", json!({"path": path})).await;
    assert_eq!(
        (added["is_git_repo"].clone(), added["branch"].clone()),
        (json!(true), json!("main"))
    );
    assert_eq!(git_out(&dir, &["log", "--format=%s"]), "Initial Commit");
    assert_eq!(
        git_out(&dir, &["ls-tree", "-r", "--name-only", "HEAD"]),
        ".gitignore\nsrc/a.rs"
    );
    assert!(dir.join(".env").exists());

    // A worktree of it holds the committed files.
    let wt = h
        .ok(
            "workspace.create_worktree",
            json!({"workspace_id": added["workspace_id"]}),
        )
        .await;
    let tree = h
        .ok(
            "workspace.tree",
            json!({"workspace_id": wt["workspace_id"]}),
        )
        .await;
    assert!(tree.to_string().contains("\"a.rs\""), "{tree}");
}

#[tokio::test]
async fn workspace_add_leaves_out_a_nested_repository_without_a_commit() {
    let h = host(false).await;
    let dir = h._dir.path().join("with-crate");
    std::fs::create_dir_all(dir.join("crate")).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    git_out(&dir.join("crate"), &["init", "-q"]);
    let path = dir.canonicalize().unwrap().to_string_lossy().into_owned();

    let added = h.ok("workspace.add", json!({"path": path})).await;
    assert_eq!(added["is_git_repo"], json!(true));
    assert_eq!(
        git_out(&dir, &["ls-tree", "-r", "--name-only", "HEAD"]),
        ".gitignore\na.txt"
    );
}

#[tokio::test]
async fn concurrent_adds_of_a_plain_folder_make_one_repository() {
    let h = host(false).await;
    let dir = h._dir.path().join("twice");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    let path = dir.canonicalize().unwrap().to_string_lossy().into_owned();

    let body = json!({"path": path});
    let (a, b, c) = tokio::join!(
        h.ok("workspace.add", body.clone()),
        h.ok("workspace.add", body.clone()),
        h.ok("workspace.add", body.clone()),
    );
    assert_eq!(a["workspace_id"], b["workspace_id"]);
    assert_eq!(a["workspace_id"], c["workspace_id"]);
    assert!([&a, &b, &c].iter().all(|w| w["is_git_repo"] == true));
    assert!(dir.join(".git").is_dir(), "the repository survived");
    assert_eq!(git_out(&dir, &["log", "--format=%s"]), "Initial Commit");
    assert_eq!(
        git_out(&dir, &["ls-tree", "-r", "--name-only", "HEAD"]),
        ".gitignore\na.txt"
    );
    let list = h.ok("workspace.list", json!({})).await;
    assert_eq!(list.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn a_dropped_add_still_finishes_the_repository() {
    let h = host(false).await;
    let dir = h._dir.path().join("dropped");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    let path = dir.canonicalize().unwrap().to_string_lossy().into_owned();

    // The add waits on the lock; its request is dropped meanwhile.
    let held = crate::workspace::repo_init_lock().await;
    let dropped = tokio::time::timeout(
        Duration::from_millis(500),
        crate::workspace::add(&h.state.db, &h.state.emitter, &path, false),
    )
    .await;
    assert!(dropped.is_err(), "the add was still waiting");
    drop(held);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let list = h.ok("workspace.list", json!({})).await;
        if list.as_array().unwrap().len() == 2 {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "never registered");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(git_out(&dir, &["log", "--format=%s"]), "Initial Commit");
}

#[tokio::test]
async fn workspace_add_leaves_an_existing_repository_alone() {
    let h = host(false).await;
    let root = std::path::Path::new(&h.root);
    std::fs::write(root.join("wip.txt"), "x").unwrap();
    let before = (
        git_out(root, &["rev-parse", "HEAD"]),
        git_out(root, &["status", "--porcelain"]),
    );
    h.ok("workspace.add", json!({"path": h.root.clone()})).await;
    let after = (
        git_out(root, &["rev-parse", "HEAD"]),
        git_out(root, &["status", "--porcelain"]),
    );
    assert_eq!(before, after);
    assert!(!root.join(".gitignore").exists());
}

#[tokio::test]
async fn workspace_add_asks_before_a_large_first_commit() {
    let h = host(false).await;
    let dir = h._dir.path().join("big");
    std::fs::create_dir(&dir).unwrap();
    for i in 0..5_001 {
        std::fs::write(dir.join(format!("f{i}")), "").unwrap();
    }
    let path = dir.canonicalize().unwrap().to_string_lossy().into_owned();

    let (status, code, details) = h.err_details("workspace.add", json!({"path": path})).await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::CONFLICT, "large_folder")
    );
    let details = details.expect("details");
    assert!(details["files"].as_u64().unwrap() > 5_000, "{details}");
    assert_eq!(details["at_least"], true);
    assert!(details["bytes"].is_u64());
    assert!(!dir.join(".git").exists() && !dir.join(".gitignore").exists());
    let list = h.ok("workspace.list", json!({})).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "nothing registered");

    let added = h
        .ok("workspace.add", json!({"path": path, "allow_large": true}))
        .await;
    assert_eq!(added["is_git_repo"], true);
    assert_eq!(git_out(&dir, &["log", "--format=%s"]), "Initial Commit");
}

#[tokio::test]
async fn workspace_add_never_asks_for_a_folder_that_has_git() {
    let h = host(false).await;
    let dir = h._dir.path().join("bigrepo");
    std::fs::create_dir(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    for i in 0..5_001 {
        std::fs::write(dir.join(format!("f{i}")), "").unwrap();
    }
    let path = dir.canonicalize().unwrap().to_string_lossy().into_owned();
    let added = h.ok("workspace.add", json!({"path": path})).await;
    assert_eq!(added["is_git_repo"], true);
    assert!(!dir.join(".gitignore").exists());
}

#[tokio::test]
async fn workspace_add_redoes_an_init_the_host_stopped_in() {
    let h = host(false).await;
    let dir = h._dir.path().join("half");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "mine").unwrap();
    // A Host stopped after `git init` and the basic `.gitignore`.
    std::fs::create_dir(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".git/zwiim-init"), "gitignore\n").unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join(".gitignore"), ".env\n").unwrap();
    let path = dir.canonicalize().unwrap().to_string_lossy().into_owned();

    let added = h.ok("workspace.add", json!({"path": path})).await;
    assert_eq!(added["is_git_repo"], true);
    assert_eq!(git_out(&dir, &["log", "--format=%s"]), "Initial Commit");
    assert_eq!(
        git_out(&dir, &["ls-tree", "-r", "--name-only", "HEAD"]),
        ".gitignore\na.txt"
    );
    assert!(!dir.join(".git/zwiim-init").exists());
}

#[tokio::test]
async fn workspace_add_that_cannot_init_registers_nothing() {
    use std::os::unix::fs::PermissionsExt;
    // Root writes through 0o555, so the init would not fail.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let h = host(false).await;
    let dir = h._dir.path().join("locked");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("keep.txt"), "mine").unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let path = dir.canonicalize().unwrap().to_string_lossy().into_owned();

    let got = h.err("workspace.add", json!({"path": path})).await;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        got,
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            "repository_init_failed".into()
        )
    );
    assert!(!dir.join(".git").exists());
    assert_eq!(
        std::fs::read_to_string(dir.join("keep.txt")).unwrap(),
        "mine"
    );
    let list = h.ok("workspace.list", json!({})).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "nothing registered");
}

#[tokio::test]
async fn workspace_browse_refuses_bad_segments() {
    let h = host(false).await;
    for path in [
        json!([".."]),
        json!(["."]),
        json!([""]),
        json!(["/etc"]),
        json!([".ssh"]),
    ] {
        assert_eq!(
            h.err("workspace.browse", json!({ "path": path })).await,
            (StatusCode::BAD_REQUEST, "invalid_input".into()),
            "{path}"
        );
    }
    assert_eq!(
        h.err("workspace.browse", json!({})).await.1,
        "invalid_request"
    );
}

#[tokio::test]
async fn workspace_create_folder_refuses_bad_names_and_parents() {
    // Only refusals here: a good request writes in the real home folder.
    // `workspace_browse` tests the create itself against a temp home.
    let h = host(false).await;
    for body in [
        json!({"path": [], "name": ".."}),
        json!({"path": [], "name": ".hidden"}),
        json!({"path": [], "name": "a/b"}),
        json!({"path": [], "name": "/Users/x"}),
        json!({"path": [], "name": ""}),
        json!({"path": [".."], "name": "new"}),
        json!({"path": ["/etc"], "name": "new"}),
        json!({"path": [".ssh"], "name": "new"}),
    ] {
        assert_eq!(
            h.err("workspace.create_folder", body.clone()).await,
            (StatusCode::BAD_REQUEST, "invalid_input".into()),
            "{body}"
        );
    }
    for body in [
        json!({"path": []}),
        json!({"name": "new"}),
        json!({"path": "/Users/x", "name": "new"}),
        json!({"path": [], "name": "new", "absolute": "/tmp"}),
    ] {
        assert_eq!(
            h.err("workspace.create_folder", body.clone()).await.1,
            "invalid_request",
            "{body}"
        );
    }
}

#[tokio::test]
async fn a_plain_folder_is_a_full_workspace_without_git() {
    let h = host(false).await;
    let plain = h._dir.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    let plain = plain.canonicalize().unwrap().to_string_lossy().into_owned();

    // Registered before repositories were made on add (or skipped as large).
    let id = crate::db::test_helpers::seed_folder(&h.state.db, &plain).await;
    let wid = crate::workspace::workspace_id_of(&h.state.db.conn, id)
        .await
        .unwrap()
        .unwrap();
    let list = h.ok("workspace.list", json!({})).await;
    let listed = |id: &str| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|w| w["workspace_id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(listed(&wid)["is_git_repo"], false);
    assert_eq!(listed(&h.workspace_id)["is_git_repo"], true);
    assert_eq!(listed(&h.workspace_id)["branch"], "main");

    // Files: create, read, write, list.
    h.ok(
        "files.create",
        json!({"workspace_id": wid, "path": "", "name": "notes.md", "kind": "file"}),
    )
    .await;
    let read = h
        .ok("files.read", json!({"workspace_id": wid, "path": "notes.md"}))
        .await;
    h.ok(
        "files.write",
        json!({"workspace_id": wid, "path": "notes.md", "content": "hi\n", "etag": read["etag"]}),
    )
    .await;
    assert_eq!(
        h.ok("files.read", json!({"workspace_id": wid, "path": "notes.md"}))
            .await["content"],
        "hi\n"
    );
    let tree = h.ok("workspace.tree", json!({"workspace_id": wid})).await;
    assert!(tree.to_string().contains("\"notes.md\""), "{tree}");

    // A terminal starts in the folder.
    let tid = h
        .ok(
            "terminal.spawn",
            json!({"workspace_id": wid, "shell": "/bin/sh"}),
        )
        .await["terminal_id"]
        .as_str()
        .unwrap()
        .to_string();
    // The folder's name only: responses never carry a host path.
    h.ok(
        "terminal.write",
        json!({"terminal_id": tid, "data": "echo \"zw:$(basename \"$PWD\")\"\n"}),
    )
    .await;
    let mut seen = false;
    for _ in 0..50 {
        let snap = h.ok("terminal.snapshot", json!({"terminal_id": tid})).await;
        if snap["data"].as_str().unwrap_or_default().contains("zw:plain") {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    h.ok("terminal.kill", json!({"terminal_id": tid})).await;
    assert!(seen, "pwd never printed the folder");

    // Git features say so, in Zwiim's shape.
    for (op, body) in [
        ("workspace.create_worktree", json!({"workspace_id": wid})),
        ("git.branches", json!({"workspace_id": wid})),
        ("git.status", json!({"workspace_id": wid})),
        ("git.diff", json!({"workspace_id": wid})),
        ("git.log", json!({"workspace_id": wid, "limit": 5})),
        (
            "git.pr_merge",
            json!({"workspace_id": wid, "number": 1, "head_sha": "abc"}),
        ),
    ] {
        assert_eq!(
            h.err(op, body).await,
            (StatusCode::UNPROCESSABLE_ENTITY, "not_a_git_repository".into()),
            "{op}"
        );
    }
    assert_eq!(
        h.ok("workspace.list", json!({})).await.as_array().unwrap().len(),
        2,
        "no worktree made"
    );
}

#[tokio::test]
async fn session_list_and_history_use_public_ids_only() {
    let h = host(false).await;
    let wid = h.workspace_id.clone();

    assert_eq!(
        h.ok("session.list", json!({"workspace_id": wid})).await,
        json!([])
    );
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let sid = session_id_of(&h.state, conv).await;
    let list = h.ok("session.list", json!({"workspace_id": wid})).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["session_id"], sid.as_str());
    assert_eq!(list[0]["workspace_id"], wid.as_str());
    assert_eq!(list[0]["agent"], "claude_code");
    assert_eq!(list[0]["live"], false);

    // Attaching an agent flips `live`; the session keeps its id.
    let (live_sid, _rx) = h.live_session().await;
    let list = h.ok("session.list", json!({"workspace_id": wid})).await;
    let live = list
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == live_sid.as_str())
        .unwrap();
    assert_eq!(live["live"], true);

    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert_eq!(history["session_id"], sid.as_str());
    assert!(history["turns"].is_array());

    assert_eq!(
        h.err("session.list", json!({"workspace_id": "nope"})).await,
        (StatusCode::NOT_FOUND, "not_found".into())
    );
    assert_eq!(
        h.err("session.history", json!({"session_id": "nope"}))
            .await,
        (StatusCode::NOT_FOUND, "not_found".into())
    );
    assert_eq!(
        h.err(
            "session.history",
            json!({"session_id": sid, "conversation_id": conv})
        )
        .await,
        (StatusCode::BAD_REQUEST, "invalid_request".into())
    );
}

#[tokio::test]
async fn workspace_files_git_resolve_workspace_id_only() {
    let h = host(false).await;
    let wid = h.workspace_id.clone();

    let hello = h.ok("host.hello", json!({})).await;
    assert_eq!(hello["protocol"], "zwiim/1");
    assert!(hello["host_id"].as_str().is_some_and(|s| !s.is_empty()));

    let list = h.ok("workspace.list", json!({})).await;
    assert_eq!(list[0]["workspace_id"], wid.as_str());

    let open = h.ok("workspace.open", json!({"workspace_id": wid})).await;
    assert_eq!(open["workspace_id"], wid.as_str());
    assert_eq!(open["kind"], "root");
    assert!(open.get("path").is_none());

    let created = h
        .ok(
            "files.create",
            json!({"workspace_id": wid, "path": "", "name": "src", "kind": "dir"}),
        )
        .await;
    assert_eq!(created["path"], "src");
    h.ok(
        "files.create",
        json!({"workspace_id": wid, "path": "src", "name": "a.txt", "kind": "file"}),
    )
    .await;
    let read = h
        .ok(
            "files.read",
            json!({"workspace_id": wid, "path": "src/a.txt"}),
        )
        .await;
    assert_eq!(read["path"], "src/a.txt");
    let saved = h
        .ok(
            "files.write",
            json!({"workspace_id": wid, "path": "src/a.txt", "content": "hello\n", "etag": read["etag"]}),
        )
        .await;
    assert_ne!(saved["etag"], read["etag"]);
    let stale = h
        .call(
            "files.write",
            json!({"workspace_id": wid, "path": "src/a.txt", "content": "x", "etag": read["etag"]}),
        )
        .await;
    assert!(!stale.0.is_success(), "stale etag must be refused");
    assert_eq!(
        h.ok(
            "files.read",
            json!({"workspace_id": wid, "path": "src/a.txt"})
        )
        .await["content"],
        "hello\n"
    );

    let tree = h.ok("workspace.tree", json!({"workspace_id": wid})).await;
    assert!(tree.to_string().contains("\"src/a.txt\""), "{tree}");
    let sub = h
        .ok(
            "workspace.tree",
            json!({"workspace_id": wid, "path": "src"}),
        )
        .await;
    assert_eq!(sub[0]["path"], "src/a.txt");
    assert_eq!(
        h.err(
            "workspace.tree",
            json!({"workspace_id": wid, "path": "../"})
        )
        .await
        .1,
        "invalid_input"
    );

    let status = h
        .ok(
            "git.status",
            json!({"workspace_id": wid, "show_all_untracked": true}),
        )
        .await;
    assert!(status.to_string().contains("src/a.txt"), "{status}");
    let commit = h
        .ok(
            "git.commit",
            json!({"workspace_id": wid, "message": "add a", "files": ["src/a.txt"]}),
        )
        .await;
    assert_eq!(commit["committed_files"], 1);
    let log = h
        .ok("git.log", json!({"workspace_id": wid, "limit": 5}))
        .await;
    assert_eq!(log["entries"][0]["message"], "add a");

    // Every workspace op resolves `workspace_id`, never a raw folder id.
    for op in [
        "workspace.open",
        "workspace.tree",
        "git.status",
        "git.log",
        "terminal.spawn",
    ] {
        assert_eq!(
            h.err(op, json!({"workspace_id": h.folder_id.to_string()}))
                .await
                .1,
            "not_found",
            "{op}"
        );
    }
    assert_eq!(
        h.err("files.read", json!({"workspace_id": "nope", "path": "a"}))
            .await
            .1,
        "not_found"
    );
    assert_eq!(
        h.err(
            "git.commit",
            json!({"workspace_id": "nope", "message": "m", "files": []})
        )
        .await
        .1,
        "not_found"
    );
}

#[tokio::test]
async fn pull_request_ops_answer_in_zwiim_shape() {
    let h = host(false).await;
    let wid = h.workspace_id.clone();
    // The default branch has no pull request to follow.
    assert_eq!(
        h.ok("git.pr", json!({"workspace_id": wid})).await,
        json!({"pr": null, "error": null})
    );
    assert_eq!(
        h.ok("git.pr", json!({"workspace_id": wid, "refresh": true})).await,
        json!({"pr": null, "error": null})
    );
    assert_eq!(
        h.err("git.pr", json!({"workspace_id": "nope"})).await.1,
        "not_found"
    );
    // A merge names the head it was reviewed at.
    assert_eq!(
        h.err("git.pr_merge", json!({"workspace_id": wid, "number": 1})).await,
        (StatusCode::BAD_REQUEST, "invalid_request".into())
    );
    assert_eq!(
        h.err(
            "git.pr_merge",
            json!({"workspace_id": wid, "number": 1, "head_sha": " "})
        )
        .await,
        (StatusCode::BAD_REQUEST, "invalid_input".into())
    );
    assert_eq!(
        h.err(
            "git.pr_merge",
            json!({"workspace_id": wid, "number": 1, "head_sha": "a", "method": "fast_forward"})
        )
        .await,
        (StatusCode::BAD_REQUEST, "invalid_request".into())
    );
}

#[test]
fn git_errors_keep_their_codes_and_never_gh_s_words() {
    use crate::git_harness::{GitError, GitErrorCode::*};
    let cases = [
        (NotMergeable, StatusCode::CONFLICT, "not_mergeable"),
        (PrChanged, StatusCode::CONFLICT, "pr_changed"),
        (GhUnavailable, StatusCode::UNPROCESSABLE_ENTITY, "gh_unavailable"),
        (GhUnauthenticated, StatusCode::UNPROCESSABLE_ENTITY, "gh_unauthenticated"),
        (Network, StatusCode::BAD_GATEWAY, "github_unreachable"),
        (Timeout, StatusCode::BAD_GATEWAY, "github_unreachable"),
        (NotFound, StatusCode::NOT_FOUND, "not_found"),
        (NoWorkspace, StatusCode::NOT_FOUND, "not_found"),
        (WorkspaceRemoved, StatusCode::GONE, "workspace_removed"),
        (Failed, StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    ];
    for (code, status, wire) in cases {
        let err = ZwiimError::from(
            GitError::new(code, "Pull request #3 cannot be merged: Merge conflicts")
                .with_detail("gh: secret ghp_abc123 at /Users/me/repo"),
        );
        assert_eq!((err.status, err.code), (status, wire), "{code:?}");
        assert!(!err.message.contains("ghp_") && !err.message.contains("/Users"));
    }
    let err = ZwiimError::from(GitError::new(NotMergeable, "Pull request #3 cannot be merged: Merge conflicts"));
    assert_eq!(
        err.details,
        Some(json!({"reason": "Pull request #3 cannot be merged: Merge conflicts"}))
    );
}

#[tokio::test]
async fn terminal_operations_are_addressed_by_terminal_id() {
    let h = host(false).await;
    let wid = h.workspace_id.clone();
    let spawned = h
        .ok(
            "terminal.spawn",
            json!({"workspace_id": wid, "shell": "/bin/sh"}),
        )
        .await;
    let tid = spawned["terminal_id"].as_str().unwrap().to_string();
    assert_eq!(spawned.as_object().unwrap().len(), 1);

    let list = h.ok("terminal.list", json!({})).await;
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["terminal_id"] == tid.as_str() && t["workspace_id"] == wid.as_str()));

    h.ok(
        "terminal.resize",
        json!({"terminal_id": tid, "cols": 100, "rows": 30}),
    )
    .await;
    h.ok(
        "terminal.write",
        json!({"terminal_id": tid, "data": "echo zw-$((40+2))\n"}),
    )
    .await;
    let mut seen = false;
    for _ in 0..50 {
        let snap = h.ok("terminal.snapshot", json!({"terminal_id": tid})).await;
        if snap["data"].as_str().unwrap_or_default().contains("zw-42") {
            assert_eq!(snap["alive"], true);
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(seen, "terminal output never appeared");

    h.ok("terminal.kill", json!({"terminal_id": tid})).await;
    for op in ["terminal.kill", "terminal.write"] {
        let body = json!({"terminal_id": tid, "data": "x"});
        let body = if op == "terminal.kill" {
            json!({"terminal_id": tid})
        } else {
            body
        };
        assert_eq!(h.err(op, body).await.1, "not_found", "{op}");
    }
}

#[tokio::test]
async fn session_operations_over_http() {
    let h = host(false).await;
    let wid = h.workspace_id.clone();

    assert_eq!(
        h.err("session.start", json!({"workspace_id": "nope", "agent": "claude_code", "blocks": [{"type": "text", "text": "hi"}]})).await.1,
        "not_found"
    );
    assert_eq!(
        h.err(
            "session.send",
            json!({"session_id": "nope", "blocks": [{"type": "text", "text": "hi"}]})
        )
        .await
        .1,
        "not_found"
    );

    // An idle session: cancel is a no-op, nothing is pending to answer.
    let idle_conv = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let idle = session_id_of(&h.state, idle_conv).await;
    h.ok("session.cancel", json!({"session_id": idle})).await;
    assert_eq!(
        h.err("session.respond", json!({"session_id": idle, "request_id": "r", "kind": "permission", "answer": {"option_id": "a"}})).await,
        (StatusCode::CONFLICT, "session_not_live".into())
    );

    // A live session: send, respond, cancel reach its agent.
    let (sid, mut rx) = h.live_session().await;
    let sent = h
        .ok("session.send", json!({"session_id": sid, "blocks": [{"type": "text", "text": "hi"}], "client_message_id": "m1"}))
        .await;
    assert_eq!(sent, json!({"session_id": sid}));
    assert!(matches!(
        rx.recv().await,
        Some(ConnectionCommand::Prompt { .. })
    ));
    assert_eq!(
        h.err(
            "session.send",
            json!({"session_id": sid, "blocks": [{"type": "text", "text": "again"}]})
        )
        .await,
        (StatusCode::CONFLICT, "turn_in_progress".into())
    );

    h.emit(AcpEvent::PermissionRequest {
        request_id: "perm-1".into(),
        tool_call: json!({"toolCallId": "tc-1", "title": "Run ls"}),
        options: vec![PermissionOptionInfo {
            option_id: "allow".into(),
            name: "Allow".into(),
            kind: "allow_once".into(),
            meta: None,
        }],
        queued: 0,
    })
    .await;
    assert_eq!(
        h.err("session.respond", json!({"session_id": sid, "request_id": "perm-2", "kind": "permission", "answer": {"option_id": "allow"}})).await.1,
        "not_found"
    );
    h.ok("session.respond", json!({"session_id": sid, "request_id": "perm-1", "kind": "permission", "answer": {"option_id": "allow"}}))
        .await;
    match rx.recv().await {
        Some(ConnectionCommand::RespondPermission {
            request_id,
            option_id,
        }) => {
            assert_eq!(
                (request_id.as_str(), option_id.as_str()),
                ("perm-1", "allow")
            );
        }
        _ => panic!("expected RespondPermission"),
    }

    h.ok("session.cancel", json!({"session_id": sid})).await;
    assert!(matches!(rx.recv().await, Some(ConnectionCommand::Cancel)));

    // Delete: refused mid-turn; an idle session goes, and stays gone.
    assert_eq!(
        h.err("session.delete", json!({"session_id": sid})).await,
        (StatusCode::CONFLICT, "turn_in_progress".into())
    );
    h.ok("session.delete", json!({"session_id": idle})).await;
    let listed = h.ok("session.list", json!({"workspace_id": wid})).await;
    assert!(listed.as_array().unwrap().iter().all(|s| s["session_id"] != idle.as_str()));
    assert!(listed.as_array().unwrap().iter().any(|s| s["session_id"] == sid.as_str()));
    assert_eq!(h.err("session.delete", json!({"session_id": idle})).await.1, "not_found");
    assert_eq!(h.err("session.history", json!({"session_id": idle})).await.1, "not_found");
}

#[tokio::test]
async fn session_stop_over_http() {
    let h = host(false).await;
    let wid = h.workspace_id.clone();

    // A session with no agent is already stopped.
    let idle_conv = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let idle = session_id_of(&h.state, idle_conv).await;
    assert_eq!(h.ok("session.stop", json!({"session_id": idle})).await, json!({}));
    assert_eq!(h.err("session.stop", json!({"session_id": "nope"})).await.1, "not_found");

    // Mid-turn, a permission waiting: the agent is told to cancel, ends its
    // turn, and is then disconnected.
    let (sid, mut rx) = h.live_session().await;
    h.ok("session.send", json!({"session_id": sid, "blocks": [{"type": "text", "text": "hi"}]}))
        .await;
    assert!(matches!(rx.recv().await, Some(ConnectionCommand::Prompt { .. })));
    h.emit(AcpEvent::PermissionRequest {
        request_id: "perm-1".into(),
        tool_call: json!({"toolCallId": "tc-1", "title": "Run ls"}),
        options: vec![],
        queued: 0,
    })
    .await;
    let (state, emitter) = h.state.connection_manager.get_state_and_emitter(CONN).await.unwrap();
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
    h.ok("session.stop", json!({"session_id": sid})).await;
    assert_eq!(agent.await.unwrap(), ["cancel", "disconnect"]);

    // The session and its transcript stay; it is no longer live.
    let list = h.ok("session.list", json!({"workspace_id": wid})).await;
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == sid.as_str())
        .expect("kept");
    assert_eq!(row["live"], false);
    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert!(history["turns"].is_array());
    assert_eq!(
        h.err("session.respond", json!({"session_id": sid, "request_id": "perm-1", "kind": "permission", "answer": {"option_id": "allow"}})).await.1,
        "session_not_live"
    );
}

fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

#[tokio::test]
async fn worktree_create_diff_and_guarded_removal() {
    let h = host(false).await;
    let wid = h.workspace_id.clone();
    let text = |s: &str| json!([{"type": "text", "text": s}]);

    // Create: the host picks branch and path; only public fields come back.
    let wt = h
        .ok("workspace.create_worktree", json!({"workspace_id": wid}))
        .await;
    assert_eq!(
        keys(&wt),
        [
            "alias",
            "base_branch",
            "base_commit",
            "branch",
            "display_path",
            "is_git_repo",
            "kind",
            "name",
            "parent_workspace_id",
            "removed",
            "workspace_id"
        ]
    );
    assert_eq!(wt["display_path"], Value::Null);
    assert_eq!(wt["is_git_repo"], true);
    let wt_id = wt["workspace_id"].as_str().unwrap().to_string();
    let branch = wt["branch"].as_str().unwrap().to_string();
    assert_eq!(
        (wt["kind"].clone(), wt["removed"].clone()),
        (json!("worktree"), json!(false))
    );
    assert_eq!(wt["parent_workspace_id"], wid.as_str());
    // A city the host chose: one plain word, nothing for clients to parse.
    assert!(
        !branch.is_empty() && branch.bytes().all(|b| b.is_ascii_lowercase()),
        "{branch}"
    );
    assert_eq!(wt["base_branch"], "main");
    assert_eq!(wt["base_commit"].as_str().unwrap().len(), 40);
    let listed = h.ok("workspace.list", json!({})).await;
    assert!(listed.as_array().unwrap().iter().any(|w| w == &wt));

    // The base is a branch the host lists; anything else is refused.
    let bases = h.ok("git.branches", json!({"workspace_id": wid})).await;
    assert_eq!(keys(&bases), ["default", "local", "remote"]);
    assert_eq!(bases["default"], "main");
    assert!(bases["local"].as_array().unwrap().contains(&json!("main")));
    let from_main = h
        .ok(
            "workspace.create_worktree",
            json!({"workspace_id": wid, "base": "main"}),
        )
        .await;
    assert_eq!(from_main["base_branch"], "main");
    assert_eq!(from_main["base_commit"], wt["base_commit"]);
    assert_eq!(
        h.err(
            "workspace.create_worktree",
            json!({"workspace_id": wid, "base": "HEAD~1"})
        )
        .await
        .1,
        "invalid_input"
    );

    // The client never names a path or branch, and only roots have worktrees.
    for (op, body) in [
        (
            "workspace.create_worktree",
            json!({"workspace_id": wid, "path": "/tmp/x"}),
        ),
        (
            "workspace.create_worktree",
            json!({"workspace_id": wid, "branch": "main"}),
        ),
        (
            "workspace.remove_worktree",
            json!({"workspace_id": wt_id, "path": "/tmp/x"}),
        ),
        ("git.diff", json!({"workspace_id": wt_id, "base": "HEAD~1"})),
    ] {
        assert_eq!(
            h.err(op, body).await,
            (StatusCode::BAD_REQUEST, "invalid_request".into()),
            "{op}"
        );
    }
    assert_eq!(
        h.err("workspace.create_worktree", json!({"workspace_id": wt_id}))
            .await
            .1,
        "invalid_input"
    );
    assert_eq!(
        h.err(
            "workspace.create_worktree",
            json!({"workspace_id": h.folder_id.to_string()})
        )
        .await
        .1,
        "not_found"
    );
    assert_eq!(
        h.err("workspace.remove_worktree", json!({"workspace_id": wid}))
            .await
            .1,
        "invalid_input"
    );

    // Diff: an untracked file in the worktree is an added file; the root is untouched.
    let empty = h.ok("git.diff", json!({"workspace_id": wt_id})).await;
    assert_eq!(
        empty,
        json!({"base_commit": wt["base_commit"], "files": [], "truncated": false})
    );
    h.ok(
        "files.create",
        json!({"workspace_id": wt_id, "path": "", "name": "new.txt", "kind": "file"}),
    )
    .await;
    let read = h
        .ok(
            "files.read",
            json!({"workspace_id": wt_id, "path": "new.txt"}),
        )
        .await;
    h.ok(
        "files.write",
        json!({"workspace_id": wt_id, "path": "new.txt", "content": "one\ntwo\n", "etag": read["etag"]}),
    )
    .await;
    let d = h.ok("git.diff", json!({"workspace_id": wt_id})).await;
    assert_eq!(
        d["files"],
        json!([{"path": "new.txt", "change": "added", "additions": 2, "deletions": 0,
                "binary": false, "too_large": false, "untracked": true}])
    );
    let one = h
        .ok(
            "git.diff",
            json!({"workspace_id": wt_id, "file": "new.txt"}),
        )
        .await;
    assert!(one["patch"].as_str().unwrap().contains("+two"), "{one}");
    assert_eq!(
        h.err(
            "git.diff",
            json!({"workspace_id": wt_id, "file": "missing.txt"})
        )
        .await
        .1,
        "not_found"
    );
    assert_eq!(
        h.ok("git.diff", json!({"workspace_id": wid})).await["files"],
        json!([])
    );
    assert!(!std::path::Path::new(&h.root).join("new.txt").exists());

    // Busy: a live agent blocks removal, even forced; an open terminal never does.
    let row = crate::workspace::resolve(&h.state.db.conn, &wt_id)
        .await
        .unwrap();
    let conv = seed_conversation(&h.state.db, row.id, AgentType::ClaudeCode).await;
    let sid = session_id_of(&h.state, conv).await;
    let _rx = h
        .state
        .connection_manager
        .insert_test_connection_live(
            CONN,
            AgentType::ClaudeCode,
            Some(row.path.clone().into()),
            h.state.emitter.clone(),
        )
        .await;
    h.state.connection_manager.get_state(CONN).await.unwrap().write().await.conversation_id =
        Some(conv);
    let tid = h
        .ok(
            "terminal.spawn",
            json!({"workspace_id": wt_id, "shell": "/bin/sh"}),
        )
        .await["terminal_id"]
        .clone();
    for force in [false, true] {
        assert_eq!(
            h.err_details(
                "workspace.remove_worktree",
                json!({"workspace_id": wt_id, "force": force})
            )
            .await,
            (
                StatusCode::CONFLICT,
                "workspace_busy".into(),
                Some(json!({"live_sessions": 1, "terminals": 0, "session_ids": [sid]}))
            ),
            "force={force}"
        );
    }
    h.ok("session.stop", json!({"session_id": sid})).await;
    let terminal_ids = |list: Value| -> Vec<Value> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|t| t["terminal_id"].clone())
            .collect()
    };

    // Dirty: refused without force, discarded with it; the branch stays.
    // A refused removal leaves the terminal running; a removal kills it.
    assert_eq!(
        h.err_details("workspace.remove_worktree", json!({"workspace_id": wt_id}))
            .await,
        (
            StatusCode::CONFLICT,
            "workspace_dirty".into(),
            Some(json!({"changes": 1}))
        )
    );
    assert!(terminal_ids(h.ok("terminal.list", json!({})).await).contains(&tid));
    let removed = h
        .ok(
            "workspace.remove_worktree",
            json!({"workspace_id": wt_id, "force": true}),
        )
        .await;
    assert_eq!(
        removed,
        json!({"workspace_id": wt_id, "parent_workspace_id": wid, "kept_sessions": 1})
    );
    assert!(!terminal_ids(h.ok("terminal.list", json!({})).await).contains(&tid));
    assert!(!std::path::Path::new(&row.path).exists());
    let branches = std::process::Command::new("git")
        .args(["branch", "--list", &branch])
        .current_dir(&h.root)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&branches.stdout).contains(&branch));

    // Removed: still listed and readable, never runnable, never moved to the root.
    let listed = h.ok("workspace.list", json!({})).await;
    let gone = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["workspace_id"] == wt_id.as_str())
        .expect("removed worktree with sessions stays listed");
    assert_eq!(
        (gone["removed"].clone(), gone["branch"].clone()),
        (json!(true), Value::Null)
    );
    let sessions = h.ok("session.list", json!({"workspace_id": wt_id})).await;
    assert_eq!(sessions[0]["session_id"], sid.as_str());
    assert_eq!(sessions[0]["workspace_id"], wt_id.as_str());
    assert_eq!(
        h.ok("session.list", json!({"workspace_id": wid})).await,
        json!([])
    );
    h.ok("session.history", json!({"session_id": sid})).await;
    for (op, body) in [
        (
            "session.send",
            json!({"session_id": sid, "blocks": text("hi")}),
        ),
        (
            "session.start",
            json!({"workspace_id": wt_id, "agent": "claude_code", "blocks": text("hi")}),
        ),
        ("terminal.spawn", json!({"workspace_id": wt_id})),
        ("workspace.open", json!({"workspace_id": wt_id})),
        (
            "files.read",
            json!({"workspace_id": wt_id, "path": "new.txt"}),
        ),
        ("git.diff", json!({"workspace_id": wt_id})),
        ("workspace.remove_worktree", json!({"workspace_id": wt_id})),
    ] {
        assert_eq!(
            h.err(op, body).await,
            (StatusCode::GONE, "workspace_removed".into()),
            "{op}"
        );
    }
    assert!(h.state.connection_manager.get_state(CONN).await.is_none());
}

#[tokio::test]
async fn workspace_continue_refuses_before_asking_github() {
    let h = host(false).await;
    let wid = h.workspace_id.clone();
    let wt = h
        .ok("workspace.create_worktree", json!({"workspace_id": wid}))
        .await;
    let wt_id = wt["workspace_id"].as_str().unwrap().to_string();

    // Clients offer Continue only to a Host that serves it.
    let hello = h.ok("host.hello", json!({})).await;
    assert!(hello["capabilities"]
        .as_array()
        .is_some_and(|c| c.iter().any(|v| v == "workspace.continue")));

    // Only a worktree continues; the client names the pull request and
    // nothing else.
    assert_eq!(
        h.err("workspace.continue", json!({"workspace_id": wid, "number": 1}))
            .await,
        (StatusCode::BAD_REQUEST, "invalid_input".into())
    );
    assert_eq!(
        h.err(
            "workspace.continue",
            json!({"workspace_id": h.folder_id.to_string(), "number": 1})
        )
        .await,
        (StatusCode::NOT_FOUND, "not_found".into())
    );
    for body in [
        json!({"workspace_id": wt_id}),
        json!({"workspace_id": wt_id, "number": 1, "branch": "main"}),
    ] {
        assert_eq!(
            h.err("workspace.continue", body).await,
            (StatusCode::BAD_REQUEST, "invalid_request".into())
        );
    }

    // Uncommitted work stays where it is.
    h.ok(
        "files.create",
        json!({"workspace_id": wt_id, "path": "", "name": "new.txt", "kind": "file"}),
    )
    .await;
    let (status, code, _) = h
        .err_details("workspace.continue", json!({"workspace_id": wt_id, "number": 1}))
        .await;
    assert_eq!((status, code.as_str()), (StatusCode::CONFLICT, "workspace_dirty"));
    let after = h.ok("workspace.list", json!({})).await;
    assert!(after.as_array().unwrap().iter().any(|w| w == &wt));
}

// ─── WebSocket ───────────────────────────────────────────────────────────

#[tokio::test]
async fn ws_subscribe_snapshot_live_replay_and_epoch() {
    let h = host(true).await;
    let (sid, _rx) = h.live_session().await;
    h.emit(delta("before ")).await;

    let mut ws = h.ws().await;
    ws.send_json(&json!({"op": "session.subscribe", "subscription_id": "a", "session_id": sid}))
        .await;
    let snap = recv(&mut ws).await;
    assert_eq!(snap["type"], "snapshot");
    assert_eq!(snap["session_id"], sid.as_str());
    let epoch = snap["epoch"].as_str().unwrap().to_string();
    let mut keys: Vec<_> = snap["snapshot"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "commands",
            "last_error",
            "live_message",
            "options",
            "pending_request",
            "pending_user_message",
            "status",
            "tool_calls",
            "turn_usage",
            "usage"
        ]
    );
    let base = snap["event_seq"].as_u64().unwrap();

    // Live projection; internal-only events are not forwarded.
    h.emit(AcpEvent::SessionStarted {
        session_id: "agent-native".into(),
    })
    .await;
    h.emit(delta("hello")).await;
    let ev = recv(&mut ws).await;
    assert_eq!(ev["type"], "event");
    assert_eq!(ev["event"]["type"], "agent.message_delta");
    assert_eq!(ev["event"]["payload"], json!({"text": "hello"}));
    assert_eq!(ev["event"]["session_id"], sid.as_str());
    assert!(ev["event"]["seq"].as_u64().unwrap() > base);
    // The agent's context report streams live, as reported.
    h.emit(AcpEvent::UsageUpdate { used: Some(1_500), size: Some(200_000) }).await;
    let ev = recv(&mut ws).await;
    assert_eq!(ev["event"]["type"], "session.usage");
    assert_eq!(ev["event"]["payload"], json!({"used": 1_500, "size": 200_000}));
    ws.close().await;

    // Reconnect with a cursor on the same epoch → replay of the gap.
    h.emit(delta(" world")).await;
    let mut ws = h.ws().await;
    ws.send_json(&json!({"op": "session.subscribe", "subscription_id": "b", "session_id": sid, "since_seq": base, "epoch": epoch}))
        .await;
    let replay = recv(&mut ws).await;
    assert_eq!(replay["type"], "replay", "{replay}");
    assert_eq!(replay["epoch"], epoch.as_str());
    let texts: Vec<_> = replay["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "agent.message_delta")
        .map(|e| e["payload"]["text"].clone())
        .collect();
    assert_eq!(texts, [json!("hello"), json!(" world")]);

    // Same cursor, wrong or missing epoch → snapshot.
    for extra in [json!({"epoch": "0000000000000000"}), json!({})] {
        let mut msg = json!({"op": "session.subscribe", "subscription_id": "c", "session_id": sid, "since_seq": base});
        msg.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        ws.send_json(&msg).await;
        let f = recv(&mut ws).await;
        assert_eq!(f["type"], "snapshot", "{f}");
    }
}

#[tokio::test]
async fn ws_detach_session_not_live_and_errors() {
    let h = host(true).await;
    let (sid, _rx) = h.live_session().await;
    let mut ws = h.ws().await;

    ws.send_json(&json!({"op": "session.subscribe", "subscription_id": "a", "session_id": sid}))
        .await;
    assert_eq!(recv(&mut ws).await["type"], "snapshot");
    ws.send_json(&json!({"op": "session.detach", "subscription_id": "a"}))
        .await;
    ws.send_json(&json!({"op": "ping"})).await;
    assert_eq!(recv(&mut ws).await, json!({"type": "pong"}));
    h.emit(delta("unseen")).await;
    assert_silent(&mut ws).await;

    // The agent goes away under a live subscription.
    ws.send_json(&json!({"op": "session.subscribe", "subscription_id": "b", "session_id": sid}))
        .await;
    assert_eq!(recv(&mut ws).await["type"], "snapshot");
    h.state.connection_manager.disconnect(CONN).await.unwrap();
    // The forwarder's broadcast closes once the connection's state drops.
    let st = h.state.connection_manager.get_state(CONN).await;
    assert!(st.is_none());
    let f = recv(&mut ws).await;
    assert_eq!(
        f,
        json!({"type": "detached", "subscription_id": "b", "reason": "session_not_live"})
    );

    // Subscribing to a session with no live agent.
    ws.send_json(&json!({"op": "session.subscribe", "subscription_id": "c", "session_id": sid}))
        .await;
    assert_eq!(
        recv(&mut ws).await,
        json!({"type": "detached", "subscription_id": "c", "reason": "session_not_live"})
    );
    ws.send_json(&json!({"op": "session.subscribe", "subscription_id": "d", "session_id": "nope"}))
        .await;
    let f = recv(&mut ws).await;
    assert_eq!(f["type"], "error");
    assert_eq!(f["error"]["code"], "not_found");
    ws.send_json(&json!({"op": "attach", "connection_id": CONN}))
        .await;
    let f = recv(&mut ws).await;
    assert_eq!(f["error"]["code"], "invalid_request");
}

#[tokio::test]
async fn ws_subscription_keeps_session_alive() {
    let h = host(true).await;
    let (sid, _rx) = h.live_session().await;
    let st = h.state.connection_manager.get_state(CONN).await.unwrap();
    let stale = chrono::Utc::now() - chrono::Duration::hours(1);
    st.write().await.last_activity_at = stale;

    let mut ws = h.ws().await;
    ws.send_json(&json!({"op": "session.subscribe", "subscription_id": "a", "session_id": sid}))
        .await;
    recv(&mut ws).await;
    assert!(
        st.read().await.last_activity_at > stale,
        "subscribe touches"
    );

    st.write().await.last_activity_at = stale;
    ws.send_json(&json!({"op": "ping"})).await;
    recv(&mut ws).await;
    assert!(st.read().await.last_activity_at > stale, "ping touches");

    // The idle sweep spares the watched session.
    h.state
        .connection_manager
        .sweep_idle(Duration::from_secs(60))
        .await;
    assert!(h.state.connection_manager.get_state(CONN).await.is_some());
}

#[tokio::test]
async fn ws_terminal_output_and_exit() {
    let h = host(true).await;
    let spawned = h
        .ok(
            "terminal.spawn",
            json!({"workspace_id": h.workspace_id, "shell": "/bin/sh"}),
        )
        .await;
    let tid = spawned["terminal_id"].as_str().unwrap().to_string();

    let mut ws = h.ws().await;
    ws.send_json(&json!({"op": "terminal.subscribe", "terminal_id": tid}))
        .await;
    ws.send_json(&json!({"op": "ping"})).await;
    // Output emitted before the ping reply may or may not arrive first.
    h.ok(
        "terminal.write",
        json!({"terminal_id": tid, "data": "echo zw-$((40+2)); exit\n"}),
    )
    .await;

    let mut output = String::new();
    let mut exited = false;
    for _ in 0..200 {
        let f = recv(&mut ws).await;
        if f["type"] == "pong" {
            continue;
        }
        assert_eq!(f["type"], "terminal");
        let ev = &f["event"];
        assert_eq!(ev["terminal_id"], tid.as_str());
        match ev["type"].as_str().unwrap() {
            "terminal.output" => {
                assert!(ev["seq"].is_u64());
                output.push_str(ev["payload"]["data"].as_str().unwrap());
            }
            "terminal.exit" => {
                exited = true;
                break;
            }
            other => panic!("unexpected {other}"),
        }
    }
    assert!(output.contains("zw-42"), "{output}");
    assert!(exited, "terminal.exit not delivered");
}

#[tokio::test]
async fn terminal_spawn_agent_params_are_checked() {
    let h = host(true).await;
    let w = h.workspace_id.clone();
    for (body, code) in [
        (json!({"workspace_id": w, "agent": "codex", "shell": "/bin/sh"}), "invalid_request"),
        (json!({"workspace_id": w, "agent": "codex", "initial_command": "ls"}), "invalid_request"),
        (json!({"workspace_id": w, "command": "/mcp"}), "invalid_request"),
        (json!({"workspace_id": w, "agent": "gemini", "command": "/mcp"}), "invalid_request"),
        (json!({"workspace_id": w, "agent": "codex", "command": "mcp"}), "invalid_input"),
        (json!({"workspace_id": w, "agent": "codex", "command": "/"}), "invalid_input"),
        (json!({"workspace_id": w, "agent": "codex", "command": "/mcp\r"}), "invalid_input"),
        (json!({"workspace_id": w, "agent": "codex", "command": "/x\u{1b}[A"}), "invalid_input"),
        (json!({"workspace_id": w, "agent": "codex", "command": format!("/{}", "a".repeat(200))}), "invalid_input"),
    ] {
        assert_eq!(
            h.err("terminal.spawn", body.clone()).await,
            (StatusCode::BAD_REQUEST, code.to_string()),
            "{body}"
        );
    }
}

/// A terminal running `script` under `/bin/sh`, and the Host's manager.
async fn sh_terminal(h: &Host, script: &str) -> (crate::terminal::manager::TerminalManager, String) {
    let tid = h
        .ok(
            "terminal.spawn",
            json!({"workspace_id": h.workspace_id, "shell": "/bin/sh", "initial_command": script}),
        )
        .await["terminal_id"]
        .as_str()
        .unwrap()
        .to_string();
    (h.state.terminal_manager.clone_ref(), tid)
}

#[tokio::test]
async fn agent_command_is_pasted_once_output_settles_then_entered_when_shown() {
    let h = host(true).await;
    // The terminal echoes what it is given; `cat` repeats a line once entered.
    let (terminals, tid) = sh_terminal(&h, r"printf '\033[?2004hready'; cat").await;
    super::http::run_when_settled(terminals.clone_ref(), tid.clone(), "/zw-run".into());
    let mut seen = String::new();
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        seen = terminals.snapshot(&tid).data;
        if seen.matches("/zw-run").count() >= 2 {
            break;
        }
    }
    let at = seen.find("/zw-run").unwrap_or_else(|| panic!("not pasted: {seen:?}"));
    assert!(seen.find("ready").is_some_and(|r| r < at), "{seen:?}");
    assert!(seen.contains("\x1b[200~/zw-run\x1b[201~"), "pasted, bracketed: {seen:?}");
    assert_eq!(seen.matches("/zw-run").count(), 2, "entered once: {seen:?}");
    h.ok("terminal.kill", json!({"terminal_id": tid})).await;
}

#[tokio::test]
async fn agent_command_gets_no_enter_when_the_program_does_not_show_it() {
    let h = host(true).await;
    // Like a CLI asking a question of its own: input is taken, nothing shown.
    let (terminals, tid) = sh_terminal(&h, r"stty -echo; printf '\033[?2004hready'; cat").await;
    super::http::run_when_settled(terminals.clone_ref(), tid.clone(), "/zw-run".into());
    tokio::time::sleep(Duration::from_secs(5)).await;
    let seen = terminals.snapshot(&tid).data;
    assert!(seen.contains("ready"), "{seen:?}");
    // Entered, `cat` would have printed the line.
    assert!(!seen.contains("/zw-run"), "{seen:?}");
    h.ok("terminal.kill", json!({"terminal_id": tid})).await;
}

#[tokio::test]
async fn agent_command_is_not_pasted_until_the_program_takes_pastes() {
    let h = host(true).await;
    // Output went quiet, but no CLI is reading yet: the tty would echo it.
    let (terminals, tid) = sh_terminal(&h, "printf ready; cat").await;
    super::http::run_when_settled(terminals.clone_ref(), tid.clone(), "/zw-run".into());
    tokio::time::sleep(Duration::from_secs(2)).await;
    let seen = terminals.snapshot(&tid).data;
    assert!(seen.contains("ready"), "{seen:?}");
    assert!(!seen.contains("/zw-run"), "{seen:?}");
    h.ok("terminal.kill", json!({"terminal_id": tid})).await;
}

// ─── host state feed ─────────────────────────────────────────────────────

impl Host {
    /// A socket subscribed to the host feed, and its snapshot frame.
    async fn feed(&self) -> (TestWebSocket, Value) {
        let mut ws = self.ws().await;
        ws.send_json(&json!({"op": "host.subscribe"})).await;
        let snapshot = self.recv_clean(&mut ws).await;
        assert_eq!(snapshot["type"], "host.snapshot", "{snapshot}");
        (ws, snapshot)
    }

    /// Next frame; also asserts no host path crosses the socket.
    async fn recv_clean(&self, ws: &mut TestWebSocket) -> Value {
        let frame = recv(ws).await;
        let text = frame.to_string();
        let tmp = self._dir.path();
        assert!(
            !text.contains(tmp.to_str().unwrap())
                && !text.contains(tmp.canonicalize().unwrap().to_str().unwrap()),
            "frame leaked a host path: {text}"
        );
        frame
    }

    /// Skip `host.event` frames until one matches; returns it.
    async fn host_event(
        &self,
        ws: &mut TestWebSocket,
        what: &str,
        pred: impl Fn(&Value) -> bool,
    ) -> Value {
        for _ in 0..200 {
            let frame = self.recv_clean(ws).await;
            assert_eq!(frame["type"], "host.event", "{frame}");
            let event = &frame["event"];
            assert!(event["seq"].is_u64() && event["type"].is_string() && event["payload"].is_object());
            if pred(event) {
                return frame;
            }
        }
        panic!("no {what}");
    }
}

fn event_is<'a>(kind: &'a str, check: impl Fn(&Value) -> bool + 'a) -> impl Fn(&Value) -> bool + 'a {
    move |e| e["type"] == kind && check(&e["payload"])
}

#[tokio::test]
async fn host_feed_snapshot_equals_the_list_operations() {
    let h = host(true).await;
    let wt = h
        .ok("workspace.create_worktree", json!({"workspace_id": h.workspace_id}))
        .await;
    seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let tid = h
        .ok(
            "terminal.spawn",
            json!({"workspace_id": h.workspace_id, "shell": "/bin/sh"}),
        )
        .await["terminal_id"]
        .clone();

    let (_ws, snapshot) = h.feed().await;
    let workspaces = h.ok("workspace.list", json!({})).await;
    assert_eq!(snapshot["workspaces"], workspaces);
    let mut sessions = Vec::new();
    for w in workspaces.as_array().unwrap() {
        let list = h
            .ok("session.list", json!({"workspace_id": w["workspace_id"]}))
            .await;
        sessions.extend(list.as_array().unwrap().iter().cloned());
    }
    assert_eq!(snapshot["sessions"], Value::Array(sessions));
    assert_eq!(snapshot["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["terminals"], h.ok("terminal.list", json!({})).await);
    assert_eq!(snapshot["terminals"][0]["terminal_id"], tid);
    // No remote, so no pull requests; the map is still there for a late joiner.
    assert_eq!(snapshot["prs"], json!({}));
    assert!(snapshot["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["workspace_id"] == wt["workspace_id"]));
    assert!(snapshot["epoch"].is_string());
    assert!(snapshot["seq"].is_u64());
    h.ok("terminal.kill", json!({"terminal_id": tid})).await;
}

#[tokio::test]
async fn host_feed_reaches_every_subscribed_socket_only() {
    let h = host(true).await;
    let (mut a, snap_a) = h.feed().await;
    let (mut b, snap_b) = h.feed().await;
    let mut idle = h.ws().await;
    assert_eq!(snap_a, snap_b);

    // An HTTP mutation.
    let wt = h
        .ok("workspace.create_worktree", json!({"workspace_id": h.workspace_id}))
        .await;
    let wid = wt["workspace_id"].clone();
    let created = event_is("workspace.changed", |p| p["workspace"]["workspace_id"] == wid);
    let fa = h.host_event(&mut a, "worktree on A", &created).await;
    let fb = h.host_event(&mut b, "worktree on B", &created).await;
    assert_eq!(fa, fb, "both sockets receive the same event");
    assert_eq!(fa["epoch"], snap_a["epoch"]);
    assert_eq!(fa["event"]["payload"]["workspace"], wt);

    // A terminal, spawned and killed.
    let tid = h
        .ok(
            "terminal.spawn",
            json!({"workspace_id": h.workspace_id, "shell": "/bin/sh"}),
        )
        .await["terminal_id"]
        .clone();
    let spawned = event_is("terminal.changed", |p| p["terminal"]["terminal_id"] == tid);
    h.host_event(&mut a, "terminal on A", &spawned).await;
    h.host_event(&mut b, "terminal on B", &spawned).await;
    h.ok("terminal.kill", json!({"terminal_id": tid})).await;
    let removed = event_is("terminal.removed", |p| p["terminal_id"] == tid);
    h.host_event(&mut a, "terminal removal on A", &removed).await;
    h.host_event(&mut b, "terminal removal on B", &removed).await;

    // A socket that never subscribed hears none of it.
    assert_silent(&mut idle).await;
}

#[tokio::test]
async fn host_feed_branch_and_diff_changes_reach_both_sockets() {
    let h = host(true).await;
    let (mut a, _) = h.feed().await;
    let (mut b, _) = h.feed().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let wid = h.workspace_id.clone();

    std::fs::write(std::path::Path::new(&h.root).join("new.txt"), "x").unwrap();
    let diff = event_is("workspace.diff_changed", |p| p["workspace_id"] == wid.as_str());
    let fa = h.host_event(&mut a, "diff on A", &diff).await;
    assert_eq!(fa["event"]["payload"], json!({"workspace_id": wid}));
    h.host_event(&mut b, "diff on B", &diff).await;

    git(std::path::Path::new(&h.root), &["checkout", "-q", "-b", "feature"]);
    let branch = event_is("workspace.changed", |p| {
        p["workspace"]["workspace_id"] == wid.as_str() && p["workspace"]["branch"] == "feature"
    });
    h.host_event(&mut a, "branch on A", &branch).await;
    h.host_event(&mut b, "branch on B", &branch).await;
}

#[tokio::test]
async fn host_feed_carries_legacy_mutations() {
    let h = host(true).await;
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let sid = session_id_of(&h.state, conv).await;
    let (mut a, _) = h.feed().await;
    let (mut b, _) = h.feed().await;

    let res = h
        .server
        .post("/api/update_conversation_title")
        .authorization_bearer(HOST_TOKEN)
        .json(&json!({"conversationId": conv, "title": "From legacy"}))
        .await;
    assert_eq!(res.status_code(), StatusCode::OK, "{}", res.text());
    let renamed = event_is("session.changed", |p| {
        p["session"]["session_id"] == sid.as_str() && p["session"]["title"] == "From legacy"
    });
    let fa = h.host_event(&mut a, "rename on A", &renamed).await;
    h.host_event(&mut b, "rename on B", &renamed).await;
    assert_eq!(fa["event"]["payload"]["session"]["workspace_id"], h.workspace_id.as_str());
}

#[tokio::test]
async fn host_feed_invalid_subscribe_and_resubscribe() {
    let h = host(true).await;
    let mut ws = h.ws().await;
    ws.send_json(&json!({"op": "host.subscribe", "since_seq": 3})).await;
    let err = recv(&mut ws).await;
    assert_eq!(err["type"], "error");
    assert_eq!(err["error"]["code"], "invalid_request");
    assert_silent(&mut ws).await;

    // The socket still works; subscribing again restarts from a snapshot.
    ws.send_json(&json!({"op": "host.subscribe"})).await;
    let first = recv(&mut ws).await;
    assert_eq!(first["type"], "host.snapshot");
    ws.send_json(&json!({"op": "host.subscribe"})).await;
    let second = recv(&mut ws).await;
    assert_eq!(second["type"], "host.snapshot");
    assert_eq!(first, second);
}

/// Claude's native options as `claude-agent-acp` 0.79 advertises them.
fn claude_options(mode: &str, model: &str) -> Vec<crate::acp::types::SessionConfigOptionInfo> {
    let value = |v: &str, n: &str, k: Option<&str>| json!({"value": v, "name": n, "description": null, "kind": k});
    serde_json::from_value(json!([
        {"id": "mode", "name": "Mode", "description": null, "category": "mode", "kind": {"type": "select", "current_value": mode, "groups": [], "options": [
            value("default", "Manual", Some("standard")),
            value("acceptEdits", "Accept edits", Some("standard")),
            value("plan", "Plan", Some("plan")),
            value("bypassPermissions", "Bypass permissions", Some("full_access")),
        ]}},
        {"id": "model", "name": "Model", "description": null, "category": "model", "kind": {"type": "select", "current_value": model, "groups": [], "options": [
            value("opus[1m]", "Opus 5", None),
            value("haiku", "Haiku 4.5", None),
        ]}},
        {"id": "effort", "name": "Effort", "description": null, "category": "thought_level", "kind": {"type": "select", "current_value": "high", "groups": [], "options": [
            value("low", "Low", None),
            value("high", "High", None),
        ]}},
    ]))
    .unwrap()
}

#[tokio::test]
async fn host_claim_and_owner() {
    let h = host(false).await;
    assert_eq!(h.ok("host.hello", json!({})).await["owner"], Value::Null);
    let alice = json!({"user_id": "u-alice", "email": "alice@example.com"});
    assert_eq!(h.ok("host.claim", alice.clone()).await["owner"], alice);
    let (status, code, details) = h
        .err_details("host.claim", json!({"user_id": "u-bob", "email": "bob@example.com"}))
        .await;
    assert_eq!((status, code.as_str()), (StatusCode::CONFLICT, "host_owned"));
    assert_eq!(details.unwrap()["owner_email"], "alice@example.com");
    assert_eq!(h.ok("host.hello", json!({})).await["owner"], alice);
    assert_eq!(
        h.err("host.claim", json!({"user_id": " "})).await.1,
        "invalid_input"
    );
    assert_eq!(
        h.err("host.claim", json!({"user_id": "u", "access_token": "x"})).await.1,
        "invalid_request"
    );
}

#[tokio::test]
async fn agent_list_reports_availability_only() {
    let h = host(false).await;
    let list = h.ok("agent.list", json!({})).await;
    let agents = list.as_array().unwrap();
    assert!(agents.len() >= 10);
    for a in agents {
        let available = a["available"].as_bool().unwrap();
        assert_eq!(available, a["reason"].is_null(), "{a}");
        assert!(a["name"].is_string());
        assert!(a["options"].is_null(), "no catalog before any session: {a}");
        assert!(a["commands"].is_null(), "no commands before any session: {a}");
        assert_eq!(a["effort_by_model"], json!({}), "no model has run yet: {a}");
    }
    assert!(agents.iter().any(|a| a["agent"] == "claude_code"));
}

#[tokio::test]
async fn agent_list_marks_claude_and_codex_supported_and_host_managed() {
    let h = host(false).await;
    let list = h.ok("agent.list", json!({})).await;
    for a in list.as_array().unwrap() {
        let managed = a["agent"] == "claude_code" || a["agent"] == "codex";
        if managed {
            assert_eq!(a["support"], "supported", "{a}");
            let adapter = &a["adapter"];
            assert!(adapter["locked_version"].is_string(), "{a}");
            for key in ["installed", "authenticated", "usable"] {
                assert!(adapter[key].is_boolean(), "{key}: {a}");
            }
            assert!(adapter["setup"]["state"].is_string(), "{a}");
            // Installed or not, availability and the adapter agree.
            if !adapter["installed"].as_bool().unwrap() {
                assert_eq!(a["available"], false, "{a}");
            }
        } else {
            assert_eq!(a["support"], "experimental", "{a}");
            assert!(a["adapter"].is_null(), "{a}");
        }
    }
}

#[tokio::test]
async fn agent_setup_only_sets_up_host_managed_adapters() {
    let h = host(false).await;
    let (status, code) = h.err("agent.setup", json!({"agent": "gemini"})).await;
    assert_eq!((status, code.as_str()), (StatusCode::BAD_REQUEST, "invalid_request"));
    let (status, code) = h.err("agent.setup", json!({"agent": "claude_code", "version": "9.9.9"})).await;
    assert_eq!((status, code.as_str()), (StatusCode::BAD_REQUEST, "invalid_request"));
}

#[tokio::test]
async fn agent_sign_in_status_reaches_every_client_and_never_a_key() {
    let h = host(false).await;
    let codex_home = tempfile::tempdir().unwrap();
    // Saving clears a plaintext key Codex wrote to its own home: a temp one.
    temp_env::async_with_vars([("CODEX_HOME", Some(codex_home.path()))], async {
        let key = "sk-test-protocol-0001";
        let saved = h.ok("agent.save_key", json!({"agent": "codex", "key": key})).await;
        assert_eq!(saved["agent"], "codex");
        assert_eq!(saved["saved_key"], true, "{saved}");
        assert!(!saved.to_string().contains(key), "{saved}");

        let all = h.ok("agent.auth_status", json!({})).await;
        let agents: Vec<_> = all.as_array().unwrap().iter().map(|s| s["agent"].clone()).collect();
        assert_eq!(agents, [json!("claude_code"), json!("codex")]);
        assert!(!all.to_string().contains(key), "{all}");
        let mut fields: Vec<_> = all[1].as_object().unwrap().keys().cloned().collect();
        fields.sort();
        assert_eq!(fields, ["agent", "connected", "email", "method", "plan", "saved_key", "sign_in"]);

        // A paired device reads the status too.
        let credential = bearer_device(&h, "dev-1").await;
        let (status, body) = h.call_as(&credential, "agent.auth_status", json!({})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body[1]["saved_key"], true);

        let removed = h.ok("agent.remove_key", json!({"agent": "codex"})).await;
        assert_eq!(removed["saved_key"], false, "{removed}");
    })
    .await;
}

#[tokio::test]
async fn a_rejected_key_is_never_echoed() {
    let h = host(false).await;
    let (status, value) = h
        .call("agent.save_key", json!({"agent": "claude_code", "key": "sk-proj-not-anthropic"}))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["code"], "invalid_input");
    assert!(!value.to_string().contains("not-anthropic"), "{value}");
    // A key of the wrong type is a schema error that does not quote it either.
    let (status, value) = h.call("agent.save_key", json!({"agent": "codex", "key": ["sk-test-in-array"]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!value.to_string().contains("sk-test-in-array"), "{value}");
    let (status, code) = h.err("agent.sign_in", json!({"agent": "gemini"})).await;
    assert_eq!((status, code.as_str()), (StatusCode::BAD_REQUEST, "invalid_request"));
}

#[tokio::test]
async fn an_unsigned_unsupervised_host_reports_that_it_cannot_update() {
    let h = host(false).await;
    let status = h.ok("host.update.status", json!({})).await;
    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(status["channel"], "stable");
    assert_eq!(status["supported"], false, "{status}");
    // Test builds carry no release key.
    assert_eq!(status["unsupported_reason"], "unsigned_build", "{status}");
    assert_eq!(status["restart_required"], false);
    assert_eq!(status["activity"]["state"], "idle");
    // Checking never reaches the network without a release key.
    let checked = h.ok("host.update.check", json!({})).await;
    assert_eq!(checked["supported"], false);
    assert!(checked["last_checked"].is_null(), "{checked}");
    let (status, code) = h.err("host.update.apply", json!({})).await;
    assert_eq!((status, code.as_str()), (StatusCode::CONFLICT, "update_unavailable"));
    let (status, code) = h.err("host.update.apply", json!({"mode": "later"})).await;
    assert_eq!((status, code.as_str()), (StatusCode::BAD_REQUEST, "invalid_request"));
    let hello = h.ok("host.hello", json!({})).await;
    assert!(hello["capabilities"].as_array().unwrap().contains(&json!("host.update")));
}

#[tokio::test]
async fn session_options_live_and_idle() {
    let h = host(false).await;
    let (sid, mut rx) = h.live_session().await;
    let st = h.state.connection_manager.get_state(CONN).await.unwrap();
    st.write().await.config_options = Some(claude_options("default", "opus[1m]"));

    let read = h.ok("session.options", json!({"session_id": sid})).await;
    let options = &read["options"];
    assert_eq!(options["model"]["current"], "opus[1m]");
    assert_eq!(options["effort"]["current"], "high");
    assert_eq!(options["mode"]["current"], "normal");
    assert_eq!(options["mode"]["available"], json!(["normal", "plan", "unattended"]));
    assert!(!read.to_string().contains("bypassPermissions"));

    // Unattended is the agent's native full-access mode, applied between turns.
    let apply = async {
        match rx.recv().await {
            Some(ConnectionCommand::SetConfigOption { config_id, value_id }) => {
                assert_eq!((config_id.as_str(), value_id.as_str()), ("mode", "bypassPermissions"));
                st.write().await.config_options =
                    Some(claude_options("bypassPermissions", "opus[1m]"));
            }
            _ => panic!("expected SetConfigOption"),
        }
    };
    let (set, ()) = tokio::join!(
        h.ok("session.set_options", json!({"session_id": sid, "mode": "unattended"})),
        apply
    );
    assert_eq!(set["options"]["mode"]["current"], "unattended");
    assert_eq!(set["options"]["mode"]["label"], "Bypass permissions");

    let (status, code, details) = h
        .err_details("session.set_options", json!({"session_id": sid, "model": "bogus"}))
        .await;
    assert_eq!((status, code.as_str()), (StatusCode::UNPROCESSABLE_ENTITY, "option_unavailable"));
    assert_eq!(details.unwrap(), json!({"option": "model", "value": "bogus"}));
    assert_eq!(
        h.err("session.set_options", json!({"session_id": sid})).await.1,
        "invalid_input"
    );
    assert_eq!(
        h.err("session.set_options", json!({"session_id": sid, "mode": "yolo"})).await.1,
        "invalid_request"
    );

    // Between turns only.
    st.write().await.status = crate::acp::types::ConnectionStatus::Prompting;
    assert_eq!(
        h.err("session.set_options", json!({"session_id": sid, "effort": "low"})).await,
        (StatusCode::CONFLICT, "turn_in_progress".into())
    );

    // Idle: nothing known about the agent yet.
    let idle_conv = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let idle = session_id_of(&h.state, idle_conv).await;
    assert_eq!(h.ok("session.options", json!({"session_id": idle})).await["options"], Value::Null);
    assert_eq!(
        h.err_details("session.set_options", json!({"session_id": idle, "mode": "plan"})).await.1,
        "option_unavailable"
    );
    // Once the agent has advertised its options, an idle session's change is
    // kept for its next respawn.
    crate::db::service::app_metadata_service::upsert_value(
        &h.state.db.conn,
        "zwiim.agent_options:claude_code",
        &serde_json::to_string(&claude_options("default", "opus[1m]")).unwrap(),
    )
    .await
    .unwrap();
    let set = h
        .ok("session.set_options", json!({"session_id": idle, "mode": "plan", "model": "haiku"}))
        .await;
    assert_eq!(set["options"]["mode"]["current"], "plan");
    assert_eq!(set["options"]["model"]["current"], "haiku");
    let preferred = crate::session_options::preferred_values(&h.state.db.conn, &idle).await;
    assert_eq!(preferred.get("mode").map(String::as_str), Some("plan"));
    assert_eq!(preferred.get("model").map(String::as_str), Some("haiku"));
    let back = h
        .ok("session.set_options", json!({"session_id": idle, "mode": "normal"}))
        .await;
    assert_eq!(back["options"]["mode"]["current"], "normal");
    let listed = h.ok("agent.list", json!({})).await;
    let claude = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agent"] == "claude_code")
        .unwrap()
        .clone();
    assert_eq!(claude["options"]["mode"]["current"], "normal");

    // Effort belongs to the model: once Opus (with effort) and Haiku (without)
    // have run here, New Session sees each model's own, and the Host refuses
    // Haiku with an effort instead of letting the agent drop it.
    let opus_effort = claude_options("default", "opus[1m]").into_iter().find(|o| o.id == "effort");
    crate::db::service::app_metadata_service::upsert_value(
        &h.state.db.conn,
        "zwiim.agent_model_effort:claude_code",
        &serde_json::to_string(&json!({"opus[1m]": opus_effort, "haiku": null})).unwrap(),
    )
    .await
    .unwrap();
    let listed = h.ok("agent.list", json!({})).await;
    let claude = listed.as_array().unwrap().iter().find(|a| a["agent"] == "claude_code").unwrap().clone();
    assert_eq!(claude["effort_by_model"]["haiku"], Value::Null);
    assert_eq!(claude["effort_by_model"]["opus[1m]"]["values"][1]["value"], "high");
    assert!(claude["options"]["effort"].is_object(), "the agent-wide effort stays");
    let start = |model: &str, effort: Option<&str>| {
        let mut options = json!({"model": model});
        if let Some(e) = effort {
            options["effort"] = json!(e);
        }
        json!({"workspace_id": h.workspace_id, "agent": "claude_code", "blocks": [{"type": "text", "text": "hi"}], "options": options})
    };
    let (status, code, details) = h.err_details("session.start", start("haiku", Some("low"))).await;
    assert_eq!((status, code.as_str()), (StatusCode::UNPROCESSABLE_ENTITY, "option_unavailable"));
    assert_eq!(details, Some(json!({"option": "effort", "value": "low"})));

    // An idle session moved to Haiku drops Opus's effort instead of keeping
    // it for a model that has none, and gets Opus's own back on Opus.
    let set = h
        .ok("session.set_options", json!({"session_id": idle, "model": "opus[1m]", "effort": "low"}))
        .await;
    assert_eq!(set["options"]["effort"]["current"], "low");
    let on_haiku = h.ok("session.set_options", json!({"session_id": idle, "model": "haiku"})).await;
    assert_eq!(on_haiku["options"]["model"]["current"], "haiku");
    assert_eq!(on_haiku["options"]["effort"], Value::Null);
    let preferred = crate::session_options::preferred_values(&h.state.db.conn, &idle).await;
    assert!(!preferred.contains_key("effort"), "{preferred:?}");
    assert_eq!(
        h.err_details("session.set_options", json!({"session_id": idle, "effort": "low"})).await.1,
        "option_unavailable"
    );
    let on_opus = h.ok("session.set_options", json!({"session_id": idle, "model": "opus[1m]"})).await;
    assert_eq!(on_opus["options"]["effort"]["values"][0]["value"], "low");
    assert_eq!(h.ok("session.options", json!({"session_id": idle})).await["options"]["model"]["current"], "opus[1m]");
}

// ─── devices ─────────────────────────────────────────────────────────────

fn device_key(byte: u8) -> String {
    URL_SAFE_NO_PAD.encode([byte; 32])
}

async fn pair_by_key(h: &Host, device_id: &str, key: u8) -> (StatusCode, Value) {
    let invite = h.ok("device.pair_invite", json!({})).await;
    let res = h
        .server
        .post("/zwiim/v1/device.pair")
        .json(&json!({
            "code": invite["code"],
            "device_id": device_id,
            "device_name": "Phone",
            "device_pubkey": device_key(key),
        }))
        .await;
    (res.status_code(), res.json::<Value>())
}

/// A device paired the legacy way, holding a bearer credential.
async fn bearer_device(h: &Host, device_id: &str) -> String {
    crate::identity::trusted_devices::register(&h.state.db.conn, device_id, "Laptop")
        .await
        .unwrap()
}

#[tokio::test]
async fn device_pair_invite_list_and_revoke() {
    let h = host(false).await;
    let host_id = h.ok("host.hello", json!({})).await["host_id"].clone();
    let static_pubkey = h.state.host_keys.get().unwrap().static_pubkey();

    // The v2 QR payload: no addresses, no private material.
    let invite = h.ok("device.pair_invite", json!({})).await;
    let mut fields: Vec<_> = invite.as_object().unwrap().keys().cloned().collect();
    fields.sort();
    assert_eq!(fields, ["code", "expires_at", "host_id", "host_static_pubkey", "v"]);
    assert_eq!(invite["v"], 2);
    assert_eq!(invite["host_id"], host_id);
    assert_eq!(invite["host_static_pubkey"], static_pubkey);

    // `device.pair` needs no credential: the code authorizes it.
    let pair = |code: &Value, device_id: &str, key: u8| {
        h.server.post("/zwiim/v1/device.pair").json(&json!({
            "code": code,
            "device_id": device_id,
            "device_name": "Phone",
            "device_pubkey": device_key(key),
        }))
    };
    let res = pair(&invite["code"], "dev-1", 1).await;
    assert_eq!(res.status_code(), StatusCode::OK);
    assert_eq!(res.json::<Value>(), json!({"host_id": host_id, "device_id": "dev-1"}));

    // Reused code.
    let res = pair(&invite["code"], "dev-2", 2).await;
    assert_eq!(res.status_code(), StatusCode::FORBIDDEN);
    assert_eq!(res.json::<Value>()["error"]["code"], "permission_denied");
    // Unknown code.
    let res = pair(&json!("not-a-code"), "dev-2", 2).await;
    assert_eq!(res.json::<Value>()["error"]["code"], "permission_denied");
    // Malformed.
    let res = h
        .server
        .post("/zwiim/v1/device.pair")
        .json(&json!({"code": "x", "device_id": "dev-2", "device_pubkey": device_key(2), "secret": 1}))
        .await;
    assert_eq!(res.json::<Value>()["error"]["code"], "invalid_request");

    let listed = h.ok("device.list", json!({})).await;
    let devices = listed["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 1);
    let d = &devices[0];
    assert_eq!(d["device_id"], "dev-1");
    assert_eq!(d["device_name"], "Phone");
    assert_eq!(d["status"], "trusted");
    assert_eq!(d["device_pubkey"], device_key(1));
    assert!(d["revoked_at"].is_null());
    assert!(d.get("id").is_none() && d.get("credential_hash").is_none(), "{d}");

    assert_eq!(h.ok("device.revoke", json!({"device_id": "dev-1"})).await, json!({}));
    let d = &h.ok("device.list", json!({})).await["devices"][0];
    assert_eq!(d["status"], "revoked");
    assert!(d["revoked_at"].is_string());
    assert_eq!(
        crate::identity::trusted_devices::authenticate_key(&h.state.db.conn, &device_key(1))
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        h.err("device.revoke", json!({"device_id": "nope"})).await,
        (StatusCode::NOT_FOUND, "not_found".into())
    );
    assert_eq!(h.err("device.revoke", json!({})).await.1, "invalid_request");
}

#[tokio::test]
async fn device_key_conflicts_and_repairing() {
    let h = host(false).await;
    assert_eq!(pair_by_key(&h, "dev-1", 1).await.0, StatusCode::OK);
    // One key, one device.
    let (status, body) = pair_by_key(&h, "dev-2", 1).await;
    assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("already_exists")));
    // A revoked device is trusted again only by pairing again.
    h.ok("device.revoke", json!({"device_id": "dev-1"})).await;
    assert_eq!(pair_by_key(&h, "dev-1", 1).await.0, StatusCode::OK);
    assert_eq!(h.ok("device.list", json!({})).await["devices"][0]["status"], "trusted");
}

#[tokio::test]
async fn a_paired_device_cannot_manage_devices_or_claim_the_host() {
    let h = host(false).await;
    let credential = bearer_device(&h, "dev-1").await;

    // Normal operations stay available to a trusted device…
    let (status, _) = h.call_as(&credential, "workspace.list", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    // …and it is recorded as seen.
    let d = &h.ok("device.list", json!({})).await["devices"][0];
    assert!(d["last_seen_at"].is_string(), "{d}");

    // Local-authority operations are refused in the Zwiim shape.
    for (op, body) in [
        ("device.pair_invite", json!({})),
        ("device.list", json!({})),
        ("device.revoke", json!({"device_id": "dev-1"})),
        ("host.claim", json!({"user_id": "u-mallory"})),
        ("host.restart", json!({"mode": "now"})),
        ("agent.setup", json!({"agent": "claude_code"})),
        ("agent.sign_in", json!({"agent": "codex"})),
        ("agent.save_key", json!({"agent": "codex", "key": "sk-test-device-0001"})),
        ("agent.remove_key", json!({"agent": "codex"})),
        ("agent.sign_out", json!({"agent": "codex"})),
    ] {
        let (status, body) = h.call_as(&credential, op, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{op}");
        assert_eq!(
            body,
            json!({"error": {"code": "permission_denied", "message": "This operation requires local host authority"}}),
            "{op}"
        );
    }
    // Nothing changed.
    assert_eq!(h.state.lifecycle.requested(), None);
    assert_eq!(h.ok("device.list", json!({})).await["devices"][0]["status"], "trusted");
    assert_eq!(h.ok("host.hello", json!({})).await["owner"], Value::Null);

    // The legacy `/api` device management is host-only too.
    for route in ["pairing_invitation_create", "trusted_device_list", "trusted_device_revoke"] {
        let res = h
            .server
            .post(&format!("/api/{route}"))
            .authorization_bearer(&credential)
            .json(&json!({"deviceId": "dev-1"}))
            .await;
        assert_eq!(res.status_code(), StatusCode::FORBIDDEN, "{route}");
    }
    let res = h
        .server
        .post("/api/trusted_device_list")
        .authorization_bearer(HOST_TOKEN)
        .await;
    assert_eq!(res.status_code(), StatusCode::OK);

    // Unknown and revoked devices do not authenticate at all.
    assert_eq!(h.call_as("unknown-credential", "workspace.list", json!({})).await.0, StatusCode::UNAUTHORIZED);
    h.ok("device.revoke", json!({"device_id": "dev-1"})).await;
    let (status, body) = h.call_as(&credential, "workspace.list", json!({})).await;
    assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::UNAUTHORIZED, Some("unauthenticated")));
}

async fn expect_revoked_close(ws: &mut TestWebSocket) {
    use axum_test::WsMessage;
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.receive_message())
            .await
            .expect("close within 5s");
        match msg {
            WsMessage::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), crate::web::auth::WS_CLOSE_DEVICE_REVOKED);
                assert_eq!(frame.reason.as_str(), "device_revoked");
                return;
            }
            WsMessage::Close(None) => panic!("closed without the revocation code"),
            _ => {}
        }
    }
}

#[tokio::test]
async fn revoking_a_device_closes_its_open_sockets() {
    let h = host(true).await;
    let credential = bearer_device(&h, "dev-1").await;
    let other = bearer_device(&h, "dev-2").await;

    let mut zwiim_ws = h.ws_as("/zwiim/v1/ws", &credential).await;
    let mut legacy_ws = h.ws_as("/ws/events", &credential).await;
    let mut other_ws = h.ws_as("/zwiim/v1/ws", &other).await;
    let mut host_ws = h.ws().await;
    // Both sockets are live before the revocation.
    zwiim_ws.send_json(&json!({"op": "ping"})).await;
    recv(&mut zwiim_ws).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    h.ok("device.revoke", json!({"device_id": "dev-1"})).await;
    expect_revoked_close(&mut zwiim_ws).await;
    expect_revoked_close(&mut legacy_ws).await;

    // Other devices and the host keep their sockets.
    for ws in [&mut other_ws, &mut host_ws] {
        ws.send_json(&json!({"op": "ping"})).await;
        assert_eq!(recv(ws).await["type"], "pong");
    }

    // The revoked device cannot open a new one.
    let res = h
        .server
        .get("/zwiim/v1/ws")
        .add_header(
            "sec-websocket-protocol",
            format!("zwiim-events, zwiim-token.{}", URL_SAFE_NO_PAD.encode(&credential)),
        )
        .await;
    assert_eq!(res.status_code(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn pairing_invites_wait_for_the_host_identity() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::init_database(dir.path(), "test").await.unwrap();
    let mut state = AppState::new_for_test(db, dir.path().to_path_buf());
    state.host_keys = Default::default();
    let app = build_router(
        Arc::new(state),
        HOST_TOKEN.to_string(),
        dir.path().to_path_buf(),
        Arc::new(ShutdownSignal::new()),
    );
    let server = TestServer::new(app).unwrap();
    let res = server
        .post("/zwiim/v1/device.pair_invite")
        .authorization_bearer(HOST_TOKEN)
        .json(&json!({}))
        .await;
    assert_eq!(res.status_code(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(res.json::<Value>()["error"]["code"], "identity_unavailable");
}

#[tokio::test]
async fn hello_carries_the_compatibility_handshake() {
    let h = host(false).await;

    // An older client says nothing: every field is still served, no verdict.
    let hello = h.ok("host.hello", json!({})).await;
    assert_eq!(hello["protocol"], "zwiim/1");
    assert_eq!(hello["revision"], super::compat::PROTOCOL_REVISION);
    assert_eq!(hello["min_client_revision"], super::compat::MIN_CLIENT_REVISION);
    assert!(hello["capabilities"]
        .as_array()
        .is_some_and(|c| c.iter().any(|v| v == "session.usage")));
    assert_eq!(hello["compat"], Value::Null);

    // A client introduces itself; unknown fields never fail the handshake.
    let hello = h
        .ok(
            "host.hello",
            json!({"client": {"app": "mobile", "version": "9.9.9", "revision": 99,
                   "capabilities": ["from.the.future"], "brand_new": {"x": 1}},
                   "top_level_new": true}),
        )
        .await;
    assert_eq!(hello["compat"], "compatible");

    // No body at all is the same as `{}`.
    let res = h
        .server
        .post("/zwiim/v1/host.hello")
        .authorization_bearer(HOST_TOKEN)
        .await;
    assert_eq!(res.status_code(), StatusCode::OK);
    assert_eq!(res.json::<Value>()["revision"], super::compat::PROTOCOL_REVISION);
}

#[tokio::test]
async fn unknown_request_fields_are_named() {
    let h = host(false).await;
    let (status, code, details) = h
        .err_details(
            "workspace.open",
            json!({"workspace_id": h.workspace_id, "from_a_newer_client": 1}),
        )
        .await;
    assert_eq!((status, code.as_str()), (StatusCode::BAD_REQUEST, "invalid_request"));
    assert_eq!(
        details,
        Some(json!({"reason": "unknown_field", "field": "from_a_newer_client"}))
    );
}

#[tokio::test]
async fn diagnostics_report_versions_and_state_without_secrets() {
    let h = host(false).await;
    let d = h.ok("host.diagnostics", json!({})).await;
    assert_eq!(d["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(d["channel"], "stable");
    assert_eq!(d["protocol"]["revision"], crate::protocol::compat::PROTOCOL_REVISION);
    assert_eq!(d["update"]["supported"], false, "{d}");
    assert_eq!(d["update"]["unsupported_reason"], "unsigned_build");
    assert_eq!(d["database"]["schema"]["state"], "current", "{d}");
    for adapter in ["claude", "codex"] {
        let a = &d["adapters"][adapter];
        assert!(a["locked_version"].is_string(), "{d}");
        assert!(a["authenticated"].is_boolean(), "{d}");
    }
    let text = d.to_string();
    for secret in [HOST_TOKEN, CONN, "sk-", "token\":", "api_key", "password"] {
        assert!(!text.contains(secret), "{secret} in diagnostics: {text}");
    }
    assert!(!text.contains(&h.state.data_dir.to_string_lossy().to_string()), "paths leak: {text}");
}
