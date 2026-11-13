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
use crate::acp::types::{AcpEvent, PermissionOptionInfo, PromptInputBlock};
use crate::app_state::AppState;
use crate::db::service::folder_service;
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
    "host.update.set_channel",
    "host.diagnostics",
    "host.instructions",
    "host.instructions.set",
    "host.folders",
    "agent.list",
    "agent.setup",
    "agent.auth_status",
    "agent.sign_in",
    "agent.save_key",
    "agent.remove_key",
    "agent.sign_out",
    "workspace.list",
    "workspace.add",
    "workspace.inspect",
    "workspace.browse",
    "workspace.create_folder",
    "workspace.open",
    "workspace.tree",
    "workspace.create_worktree",
    "workspace.remove_worktree",
    "workspace.remove",
    "workspace.group_create",
    "workspace.group_rename",
    "workspace.group_delete",
    "workspace.layout_set",
    "workspace.continue",
    "workspace.resolve_conflicts",
    "files.read",
    "files.write",
    "files.create",
    "files.download",
    "attachments.upload",
    "git.status",
    "git.log",
    "git.branches",
    "git.commit",
    "git.diff",
    "git.pr",
    "git.pr_merge",
    "git.pr_auto_merge",
    "github.repo_available",
    "github.repos",
    "github.auth_status",
    "github.sign_in",
    "github.cancel_sign_in",
    "workspace.clone",
    "workspace.clone_cancel",
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
    "session.revert",
    "session.retry",
    "session.retry_in_new_chat",
    "session.continue_plan",
    "session.fork",
    "session.transcript_remove",
    "session.follow_up",
    "session.follow_up_edit",
    "session.follow_up_remove",
    "session.follow_up_send",
    "settings.get",
    "settings.update",
    "repository.settings.get",
    "repository.settings.update",
    "repository.settings.import",
    "workspace.run_script",
    "workspace.previews",
    "device.pair_invite",
    "device.list",
    "device.revoke",
    "cloud.enroll",
    "cloud.status",
    "lan.status",
    "lan.set",
    "host.keep_awake.status",
    "host.keep_awake.set",
    "browser.tabs.list",
    "browser.tab.open",
    "browser.tab.close",
    "browser.tab.navigate",
    "browser.engine.reset",
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
    /// A worktree of `repo`; sessions and terminals run here, never in `root`.
    pub(crate) wt: String,
    pub(crate) wt_workspace_id: String,
    pub(crate) wt_folder_id: i32,
    pub(crate) state: Arc<AppState>,
    pub(crate) server: TestServer,
    /// A `git init --bare` remote the fake `gh`'s `repo create` pushes to.
    pub(crate) bare: std::path::PathBuf,
}

pub(crate) async fn host(http_transport: bool) -> Host {
    host_with_gh(http_transport, None).await
}

/// A signed-in `gh` (`octo`, orgs `acme` and `labs`) whose `repo create`
/// pushes to the fixture's bare remote. `octo/taken` exists.
pub(crate) const SIGNED_IN_GH: &str = r#"
"api user") echo '{"login":"octo","id":42,"name":"Octo Cat"}';;
"api user/orgs"*) printf 'acme\nlabs\n';;
"api repos/octo/taken"*) echo 1;;
"api repos/"*) echo 'gh: Not Found (HTTP 404)' >&2; exit 1;;
"repo create"*) while [ $# -gt 0 ]; do [ "$1" = --source ] && src=$2; shift; done; git -C "$src" remote add origin '{bare}' && git -C "$src" push -q -u origin HEAD;;
"#;

/// Like [`host`], but with `AppState.gh` set from `body` (a `fake_gh` script
/// body; `None` points at a missing `gh`, so `gh_unavailable`).
pub(crate) async fn host_with_gh(http_transport: bool, body: Option<&str>) -> Host {
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
    let wt_dir = dir.path().join("wt");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt",
            wt_dir.to_str().unwrap(),
        ],
    );
    let wt = wt_dir
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let wt_folder_id = folder_service::add_folder_with_parent(&db.conn, &wt, Some(folder_id))
        .await
        .unwrap()
        .id;
    let bare = dir.path().join("remote.git");
    git(
        dir.path(),
        &["init", "-q", "--bare", bare.to_str().unwrap()],
    );
    let mut app_state = AppState::new_for_test(db, data_dir.clone());
    app_state.gh = Some(match body {
        Some(body) => {
            let body = body.replace("{bare}", bare.to_str().unwrap());
            crate::git_harness::fake_gh(dir.path(), &body).0
        }
        // Never the developer's own gh: missing, so `gh_unavailable`.
        None => dir.path().join("no-gh"),
    });
    let state = Arc::new(app_state);
    let workspace_id = crate::workspace::workspace_id_of(&state.db.conn, folder_id)
        .await
        .unwrap()
        .unwrap();
    let wt_workspace_id = crate::workspace::workspace_id_of(&state.db.conn, wt_folder_id)
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
        wt,
        wt_workspace_id,
        wt_folder_id,
        state,
        server,
        bare,
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
        assert!(
            details.is_none() || named_field,
            "{op}: unexpected details {details:?}"
        );
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
    pub(crate) async fn live_session(
        &self,
    ) -> (String, tokio::sync::mpsc::Receiver<ConnectionCommand>) {
        let conv =
            seed_conversation(&self.state.db, self.wt_folder_id, AgentType::ClaudeCode).await;
        let rx = self
            .state
            .connection_manager
            .insert_test_connection_live(
                CONN,
                AgentType::ClaudeCode,
                Some(self.wt.clone().into()),
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
        h.err("session.teleport", json!({})).await,
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
async fn workspace_groups_organize_without_leaking_rows() {
    let h = host(false).await;
    let root = h.ok("workspace.add", json!({"path": h.root.clone()})).await;
    let workspace_id = root["workspace_id"].as_str().unwrap();

    let group = h
        .ok("workspace.group_create", json!({"name": " backend "}))
        .await;
    assert_eq!(group["name"], "backend");
    let group_id = group["group_id"].as_str().unwrap();
    assert_eq!(group.as_object().unwrap().len(), 2, "{group}");

    let renamed = h
        .ok(
            "workspace.group_rename",
            json!({"group_id": group_id, "name": "services"}),
        )
        .await;
    assert_eq!(renamed["group_id"], group_id);
    assert_eq!(renamed["name"], "services");

    let saved = h
        .ok(
            "workspace.layout_set",
            json!({"entries": [{"kind": "group", "group_id": group_id, "workspaces": [
                {"workspace_id": workspace_id, "worktrees": []},
            ]}]}),
        )
        .await;
    assert_eq!(saved, json!({}), "the feed alone carries layout");

    let deleted = h
        .ok("workspace.group_delete", json!({"group_id": group_id}))
        .await;
    assert_eq!(deleted, json!({}));
    let (status, _) = h
        .err("workspace.group_delete", json!({"group_id": group_id}))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn workspace_layout_refuses_mistakes_by_name() {
    let h = host(false).await;
    let root = h.ok("workspace.add", json!({"path": h.root.clone()})).await;
    let workspace_id = root["workspace_id"].as_str().unwrap();

    let (status, code) = h.err("workspace.group_create", json!({"name": " "})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code, "invalid_input");

    let (status, code) = h
        .err(
            "workspace.layout_set",
            json!({"entries": [
                {"kind": "workspace", "workspace_id": workspace_id, "worktrees": []},
                {"kind": "workspace", "workspace_id": workspace_id, "worktrees": []},
            ]}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code, "invalid_input");

    // `name` is the read shape. A nested stray field is named too.
    for entries in [
        json!([{"kind": "group", "group_id": "g", "name": "n", "workspaces": []}]),
        json!([{"kind": "group", "group_id": "g", "workspaces": [
            {"workspace_id": workspace_id, "worktrees": [], "extra": 1},
        ]}]),
    ] {
        let (status, _) = h
            .err("workspace.layout_set", json!({"entries": entries}))
            .await;
        assert!(status.is_client_error(), "{status}");
    }
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
    assert_eq!(listed.as_array().unwrap().len(), 2, "nothing added");
}

#[tokio::test]
async fn inspect_reports_a_plain_folder_and_the_github_owners() {
    let h = host_with_gh(false, Some(SIGNED_IN_GH)).await;
    let folder = h._dir.path().join("My App");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("a.txt"), "a").unwrap();
    let v = h.ok("workspace.inspect", json!({ "path": folder })).await;
    assert_eq!(v["git"], "none");
    assert_eq!(v["branch"], "main");
    assert_eq!(v["suggested_name"], "My-App");
    assert_eq!(v["first_commit"]["files"], 1);
    assert_eq!(v["first_commit"]["large"], false);
    assert_eq!(v["github"]["available"], true);
    assert_eq!(v["github"]["login"], "octo");
    assert_eq!(
        v["github"]["owners"],
        json!([{"login":"octo","kind":"user"},{"login":"acme","kind":"org"},{"login":"labs","kind":"org"}])
    );
    assert!(!folder.join(".git").exists(), "inspect touches nothing");
}

#[tokio::test]
async fn inspect_reports_a_repository_without_origin_and_its_branch() {
    let h = host_with_gh(false, Some(SIGNED_IN_GH)).await;
    let folder = h._dir.path().join("lib");
    std::fs::create_dir(&folder).unwrap();
    git(&folder, &["init", "-q", "-b", "trunk"]);
    git(&folder, &["commit", "-q", "--allow-empty", "-m", "c"]);
    let v = h.ok("workspace.inspect", json!({ "path": folder })).await;
    assert_eq!(v["git"], "no_origin");
    assert_eq!(v["branch"], "trunk");
    assert_eq!(v["first_commit"], Value::Null);
    assert_eq!(v["github"]["available"], true);
}

#[tokio::test]
async fn inspect_skips_github_for_a_repository_with_origin() {
    let h = host_with_gh(false, Some(SIGNED_IN_GH)).await;
    git(
        std::path::Path::new(&h.root),
        &["remote", "add", "origin", h.bare.to_str().unwrap()],
    );
    let v = h.ok("workspace.inspect", json!({ "path": h.root })).await;
    assert_eq!(v["git"], "origin");
    assert_eq!(v["branch"], Value::Null);
    assert_eq!(v["github"], Value::Null);
}

#[tokio::test]
async fn inspect_says_why_github_is_unavailable() {
    for (body, reason) in [
        (None, "gh_unavailable"),
        (
            Some(
                "\"api user\") echo 'To get started with GitHub CLI, please run:  gh auth login' >&2; exit 4;;",
            ),
            "gh_unauthenticated",
        ),
        (
            Some("\"api user\") echo 'error connecting to api.github.com' >&2; exit 1;;"),
            "github_unreachable",
        ),
    ] {
        let h = host_with_gh(false, body).await;
        let v = h.ok("workspace.inspect", json!({ "path": h.root })).await;
        assert_eq!(
            v["github"],
            json!({ "available": false, "reason": reason }),
            "{body:?}"
        );
    }
}

#[tokio::test]
async fn inspect_validates_like_add() {
    let h = host(false).await;
    assert_eq!(
        h.err("workspace.inspect", json!({ "path": "relative" }))
            .await,
        (StatusCode::BAD_REQUEST, "invalid_input".into())
    );
    assert_eq!(
        h.err("workspace.inspect", json!({})).await,
        (StatusCode::BAD_REQUEST, "invalid_input".into())
    );
    assert_eq!(
        h.err("workspace.inspect", json!({ "home_path": [] })).await,
        (StatusCode::BAD_REQUEST, "invalid_input".into())
    );
}

/// A plain folder (no `.git`) already registered as a root workspace, as if
/// a previous `workspace.list` had already listed it.
async fn listed_plain_root(h: &Host, name: &str) -> String {
    let folder = h._dir.path().join(name);
    std::fs::create_dir(&folder).unwrap();
    let canonical = folder
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let folder_id = seed_folder(&h.state.db, &canonical).await;
    crate::workspace::workspace_id_of(&h.state.db.conn, folder_id)
        .await
        .unwrap()
        .unwrap()
}

/// A repository with no commit yet, already registered as a root workspace.
async fn listed_unborn_root(h: &Host, name: &str) -> String {
    let folder = unborn_repository(h, name);
    let folder_id = seed_folder(&h.state.db, &folder.to_string_lossy()).await;
    crate::workspace::workspace_id_of(&h.state.db.conn, folder_id)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn inspect_and_add_by_workspace_id_for_a_listed_plain_root() {
    let h = host_with_gh(false, Some(SIGNED_IN_GH)).await;
    let workspace_id = listed_plain_root(&h, "plain").await;

    let v = h
        .ok("workspace.inspect", json!({ "workspace_id": workspace_id }))
        .await;
    assert_eq!(v["git"], "none");

    let added = h
        .ok(
            "workspace.add",
            json!({ "workspace_id": workspace_id, "publish": { "owner": "octo", "name": "plain" } }),
        )
        .await;
    assert_eq!(added["is_git_repo"], true);
    assert_eq!(added["workspace_id"], workspace_id);
}

#[tokio::test]
async fn inspect_and_add_by_workspace_id_for_a_listed_unborn_root() {
    let h = host_with_gh(false, Some(SIGNED_IN_GH)).await;
    let workspace_id = listed_unborn_root(&h, "fresh2").await;

    let v = h
        .ok("workspace.inspect", json!({ "workspace_id": workspace_id }))
        .await;
    assert_eq!(v["git"], "no_origin");
    assert_eq!(v["first_commit"]["files"], 1);

    let added = h
        .ok("workspace.add", json!({ "workspace_id": workspace_id }))
        .await;
    assert_eq!(added["is_git_repo"], true);
    assert_eq!(added["workspace_id"], workspace_id);
}

#[tokio::test]
async fn inspect_and_add_by_workspace_id_refuse_a_worktree() {
    let h = host(false).await;
    for op in ["workspace.inspect", "workspace.add"] {
        assert_eq!(
            h.err(op, json!({ "workspace_id": h.wt_workspace_id }))
                .await,
            (StatusCode::BAD_REQUEST, "invalid_input".into()),
            "{op}"
        );
    }
}

#[tokio::test]
async fn inspect_and_add_by_workspace_id_rejects_an_unknown_id() {
    let h = host(false).await;
    for op in ["workspace.inspect", "workspace.add"] {
        assert_eq!(
            h.err(op, json!({ "workspace_id": "nope" })).await,
            (StatusCode::NOT_FOUND, "not_found".into()),
            "{op}"
        );
    }
}

#[tokio::test]
async fn inspect_and_add_refuse_path_with_workspace_id() {
    let h = host(false).await;
    for op in ["workspace.inspect", "workspace.add"] {
        assert_eq!(
            h.err(
                op,
                json!({ "path": h.root, "workspace_id": h.workspace_id }),
            )
            .await,
            (StatusCode::BAD_REQUEST, "invalid_input".into()),
            "{op}"
        );
    }
}

#[tokio::test]
async fn inspect_and_add_by_workspace_id_allowed_for_a_paired_device() {
    let h = host(false).await;
    let workspace_id = listed_plain_root(&h, "plain-device").await;
    let credential = bearer_device(&h, "dev-1").await;

    let res = h
        .server
        .post("/zwiim/v1/workspace.inspect")
        .authorization_bearer(&credential)
        .json(&json!({ "workspace_id": workspace_id }))
        .await;
    assert_eq!(res.status_code(), StatusCode::OK);
    let body: Value = res.json();
    assert_eq!(body["git"], "none");
}

#[tokio::test]
async fn repo_available_checks_the_name_on_github() {
    let h = host_with_gh(false, Some(SIGNED_IN_GH)).await;
    let v = h
        .ok(
            "github.repo_available",
            json!({ "owner": "octo", "name": "free" }),
        )
        .await;
    assert_eq!(v, json!({ "available": true }));
    let v = h
        .ok(
            "github.repo_available",
            json!({ "owner": "octo", "name": "taken" }),
        )
        .await;
    assert_eq!(v, json!({ "available": false }));
    assert_eq!(
        h.err(
            "github.repo_available",
            json!({ "owner": "octo", "name": "a b" })
        )
        .await,
        (StatusCode::BAD_REQUEST, "invalid_input".into())
    );
    assert_eq!(
        h.err(
            "github.repo_available",
            json!({ "owner": "--x", "name": "a" })
        )
        .await,
        (StatusCode::BAD_REQUEST, "invalid_input".into())
    );
}

#[tokio::test]
async fn repo_available_without_gh_is_gh_unavailable() {
    let h = host(false).await;
    assert_eq!(
        h.err(
            "github.repo_available",
            json!({ "owner": "octo", "name": "a" })
        )
        .await,
        (StatusCode::UNPROCESSABLE_ENTITY, "gh_unavailable".into())
    );
}

/// `SIGNED_IN_GH` plus listing and cloning: `octo/app` clones the fixture's
/// bare remote and points `origin` at GitHub.
const CLONING_GH: &str = r#"
"api user") echo '{"login":"octo","id":42,"name":"Octo Cat"}';;
"api user/repos"*) echo '[{"full_name":"octo/app","private":true,"size":5,"default_branch":"main"},{"full_name":"octo/mine","private":false,"size":1}]';;
"repo clone octo/app"*) git clone -q 'file://{bare}' "$4" && git -C "$4" remote set-url origin https://github.com/octo/app.git && git -C "$4" config 'url.file://{bare}.insteadOf' https://github.com/octo/app.git;;
"#;

/// Give the fixture's bare remote a `main` to clone.
fn seed_bare(h: &Host) {
    git(
        std::path::Path::new(&h.root),
        &[
            "push",
            "-q",
            h.bare.to_str().unwrap(),
            "HEAD:refs/heads/main",
        ],
    );
    git(&h.bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);
}

#[tokio::test]
async fn github_repos_lists_a_page_with_opened_roots() {
    let h = host_with_gh(false, Some(CLONING_GH)).await;
    git(
        std::path::Path::new(&h.root),
        &["remote", "add", "origin", "git@github.com:Octo/Mine.git"],
    );
    let v = h.ok("github.repos", json!({})).await;
    assert_eq!(v["next_page"], Value::Null);
    assert_eq!(v["repos"][0]["owner"], "octo");
    assert_eq!(v["repos"][0]["name"], "app");
    assert_eq!(v["repos"][0]["private"], true);
    assert_eq!(v["repos"][0]["size_kb"], 5);
    assert_eq!(v["repos"][0]["workspace_id"], Value::Null);
    assert_eq!(v["repos"][1]["workspace_id"], h.workspace_id.as_str());
    assert_eq!(
        h.err("github.repos", json!({ "page": 0 })).await,
        (StatusCode::BAD_REQUEST, "invalid_input".into())
    );
    // An unknown field (e.g. a host path a client must never send) is a
    // `deny_unknown_fields` rejection, same as every other op.
    assert_eq!(
        h.err("github.repos", json!({ "path": "/" })).await,
        (StatusCode::BAD_REQUEST, "invalid_request".into())
    );
}

#[tokio::test]
async fn github_repos_without_gh_is_gh_unavailable() {
    let h = host(false).await;
    assert_eq!(
        h.err("github.repos", json!({})).await,
        (StatusCode::UNPROCESSABLE_ENTITY, "gh_unavailable".into())
    );
}

#[tokio::test]
async fn workspace_clone_opens_the_repository_then_finds_it_again() {
    let h = host_with_gh(false, Some(CLONING_GH)).await;
    seed_bare(&h);
    let v = h
        .ok("workspace.clone", json!({ "owner": "octo", "name": "app" }))
        .await;
    let clone_id = v["clone_id"].as_str().expect("a clone job").to_string();
    assert_eq!(v.as_object().unwrap().len(), 1, "{v}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let job = loop {
        let job = h
            .state
            .clones
            .list()
            .into_iter()
            .find(|j| j.clone_id == clone_id)
            .unwrap();
        if job.state.finished() {
            break job;
        }
        assert!(std::time::Instant::now() < deadline, "clone never finished");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert_eq!(
        job.state,
        crate::workspace::clone::CloneState::Done,
        "{job:?}"
    );
    let list = h.ok("workspace.list", json!({})).await;
    let ids: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["workspace_id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&job.workspace_id.as_deref().unwrap()));
    assert!(ids.contains(&job.worktree_id.as_deref().unwrap()));
    let again = h
        .ok("workspace.clone", json!({ "owner": "octo", "name": "app" }))
        .await;
    assert_eq!(again, json!({ "workspace_id": job.workspace_id.unwrap() }));
}

#[tokio::test]
async fn workspace_clone_refuses_hostile_targets_and_unknown_fields() {
    let h = host_with_gh(false, Some(CLONING_GH)).await;
    for (owner, name) in [
        ("-o", "app"),
        ("octo", "ext::x"),
        ("octo", "a b"),
        ("https://github.com/o/n", "x"),
    ] {
        assert_eq!(
            h.err("workspace.clone", json!({ "owner": owner, "name": name }))
                .await,
            (StatusCode::BAD_REQUEST, "invalid_input".into()),
            "{owner}/{name}"
        );
    }
    // An unknown field (e.g. a host path a client must never send) is a
    // `deny_unknown_fields` rejection, same as every other op.
    assert_eq!(
        h.err(
            "workspace.clone",
            json!({ "owner": "octo", "name": "app", "path": "/tmp/x" })
        )
        .await,
        (StatusCode::BAD_REQUEST, "invalid_request".into())
    );
}

#[tokio::test]
async fn clone_cancel_of_an_unknown_clone_is_not_found() {
    let h = host(false).await;
    assert_eq!(
        h.err("workspace.clone_cancel", json!({ "clone_id": "c_nope" }))
            .await,
        (StatusCode::NOT_FOUND, "not_found".into())
    );
}

#[tokio::test]
async fn paired_devices_can_list_and_clone() {
    let h = host_with_gh(false, Some(CLONING_GH)).await;
    let device = bearer_device(&h, "dev-1").await;
    let (status, v) = h.call_as(&device, "github.repos", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let (status, v) = h
        .call_as(
            &device,
            "workspace.clone_cancel",
            json!({ "clone_id": "c_nope" }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{v}");
}

/// A `gh` signed out whose login shows gh's code and address, then waits.
const SIGNING_IN_GH: &str = r#""auth status --json hosts") echo '{"hosts":{}}';;
"auth login --web --hostname github.com --git-protocol https") echo '! First copy your one-time code: 36BE-F43F' >&2; echo 'Open this URL to continue in your web browser: https://github.com/login/device' >&2; exec sleep 30;;
"#;

#[tokio::test]
async fn paired_devices_sign_the_host_gh_in_to_github() {
    let h = host_with_gh(false, Some(SIGNING_IN_GH)).await;
    assert_eq!(
        h.ok("github.auth_status", json!({})).await,
        json!({ "state": "signed_out" })
    );
    let device = bearer_device(&h, "dev-1").await;
    let signing_in = json!({
        "state": "signing_in",
        "code": "36BE-F43F",
        "url": "https://github.com/login/device",
    });
    let (status, v) = h.call_as(&device, "github.sign_in", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v, signing_in);
    assert_eq!(h.ok("github.auth_status", json!({})).await, signing_in);
    // A second sign-in is the same one.
    assert_eq!(h.ok("github.sign_in", json!({})).await, signing_in);
    let (status, v) = h.call_as(&device, "github.cancel_sign_in", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v, json!({ "state": "signed_out" }));
}

#[tokio::test]
async fn github_sign_in_reads_signed_in_and_unavailable() {
    let h = host_with_gh(
        false,
        Some(
            r#""auth status --json hosts") echo '{"hosts":{"github.com":[{"state":"success","active":true,"host":"github.com","login":"octo"}]}}';;"#,
        ),
    )
    .await;
    let signed_in = json!({ "state": "signed_in", "login": "octo" });
    assert_eq!(h.ok("github.auth_status", json!({})).await, signed_in);
    assert_eq!(h.ok("github.sign_in", json!({})).await, signed_in);

    let h = host_with_gh(false, None).await;
    for op in [
        "github.auth_status",
        "github.sign_in",
        "github.cancel_sign_in",
    ] {
        assert_eq!(
            h.ok(op, json!({})).await,
            json!({ "state": "unavailable" }),
            "{op}"
        );
    }
}

#[test]
fn clone_is_a_capability() {
    assert!(crate::protocol::compat::CAPABILITIES.contains(&"workspace.clone"));
}

/// A plain folder with something to commit and a secret publishing must
/// never push.
fn plain_folder(h: &Host, name: &str) -> std::path::PathBuf {
    let folder = h._dir.path().join(name);
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("a.txt"), "a").unwrap();
    std::fs::write(folder.join(".gitignore"), "target\n").unwrap();
    std::fs::write(folder.join(".env"), "SECRET=1").unwrap();
    folder.canonicalize().unwrap()
}

/// Git output, without asserting success: some checks (a `user.*` config
/// nothing set) exit non-zero on no match.
fn out(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn git");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[tokio::test]
async fn add_publishes_a_plain_folder_to_a_private_repo() {
    let h = host_with_gh(false, Some(SIGNED_IN_GH)).await;
    let folder = plain_folder(&h, "app");
    let v = h
        .ok(
            "workspace.add",
            json!({ "path": folder, "publish": { "owner": "octo", "name": "app" } }),
        )
        .await;
    assert_eq!(v["kind"], "root");
    assert_eq!(
        out(&folder, &["config", "--local", "user.email"]),
        "42+octo@users.noreply.github.com"
    );
    assert_eq!(
        out(
            &folder,
            &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]
        ),
        "origin/main"
    );
    let pushed = out(&h.bare, &["ls-tree", "-r", "--name-only", "main"]);
    assert_eq!(pushed.lines().collect::<Vec<_>>(), [".gitignore", "a.txt"]);
    let calls = std::fs::read_to_string(h._dir.path().join("gh-calls")).unwrap();
    assert!(
        calls.contains("repo create octo/app --private --source"),
        "{calls}"
    );
    // Raise PR's precondition: the first worktree branches from origin.
    let wid = v["workspace_id"].as_str().unwrap();
    h.ok("workspace.create_worktree", json!({ "workspace_id": wid }))
        .await;
}

#[tokio::test]
async fn add_publishes_a_repository_on_another_branch() {
    let h = host_with_gh(false, Some(SIGNED_IN_GH)).await;
    let folder = h._dir.path().join("lib");
    std::fs::create_dir(&folder).unwrap();
    git(&folder, &["init", "-q", "-b", "trunk"]);
    git(&folder, &["commit", "-q", "--allow-empty", "-m", "mine"]);
    let v = h
        .ok(
            "workspace.add",
            json!({ "path": folder, "publish": { "owner": "acme", "name": "lib" } }),
        )
        .await;
    assert_eq!(
        out(
            &folder,
            &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]
        ),
        "origin/trunk"
    );
    assert_eq!(out(&h.bare, &["log", "--format=%s", "trunk"]), "mine");
    // The fixture's git sets an author by env, not config: the repository
    // had no local identity, so publishing set the account's.
    assert_eq!(
        out(&folder, &["config", "--local", "user.name"]),
        "Octo Cat"
    );
    let wid = v["workspace_id"].as_str().unwrap();
    h.ok("workspace.create_worktree", json!({ "workspace_id": wid }))
        .await;
}

#[tokio::test]
async fn workspace_add_publish_sets_origin_head_without_a_tracking_ref() {
    // The push lands (so `gh` sees success) but by URL, not through the
    // `origin` remote with `-u`: no `refs/remotes/origin/main` for `git
    // remote set-head` to read, so it fails and the fallback must record
    // the branch itself.
    let gh = SIGNED_IN_GH.replace(
        "git -C \"$src\" push -q -u origin HEAD",
        "git -C \"$src\" push -q '{bare}' HEAD",
    );
    let h = host_with_gh(false, Some(&gh)).await;
    let folder = plain_folder(&h, "app");
    let v = h
        .ok(
            "workspace.add",
            json!({ "path": folder, "publish": { "owner": "octo", "name": "app" } }),
        )
        .await;
    assert_eq!(
        out(&folder, &["symbolic-ref", "refs/remotes/origin/HEAD"]),
        "refs/remotes/origin/main"
    );
    assert_eq!(
        out(&folder, &["rev-parse", "refs/remotes/origin/main"]),
        out(&folder, &["rev-parse", "HEAD"])
    );
    // Raise PR's precondition still holds: the first worktree branches from
    // origin.
    let wid = v["workspace_id"].as_str().unwrap();
    h.ok("workspace.create_worktree", json!({ "workspace_id": wid }))
        .await;
}

#[tokio::test]
async fn a_failed_publish_leaves_a_plain_folder_as_it_was() {
    let gh = SIGNED_IN_GH.replace(
        "\"repo create\"*)",
        "\"repo create\"*) echo 'GraphQL: Something went wrong' >&2; exit 1;;\n\"never\")",
    );
    let h = host_with_gh(false, Some(&gh)).await;
    let folder = plain_folder(&h, "app");
    let (_status, code, details) = h
        .err_details(
            "workspace.add",
            json!({ "path": folder, "publish": { "owner": "octo", "name": "app" } }),
        )
        .await;
    assert_eq!(code, "publish_failed");
    assert_eq!(details, Some(json!({ "created_on_github": false })));
    assert!(!folder.join(".git").exists());
    assert_eq!(
        std::fs::read_to_string(folder.join(".gitignore")).unwrap(),
        "target\n"
    );
    let list = h.ok("workspace.list", json!({})).await;
    assert!(!list.to_string().contains("\"app\""), "{list}");
}

#[tokio::test]
async fn a_failed_publish_keeps_a_repositorys_history() {
    let gh = SIGNED_IN_GH.replace(
        "\"repo create\"*)",
        "\"repo create\"*) git remote add origin https://example.invalid/x.git; echo 'Name already exists on this account' >&2; exit 1;;\n\"never\")",
    );
    let h = host_with_gh(false, Some(&gh)).await;
    let folder = h._dir.path().join("lib");
    std::fs::create_dir(&folder).unwrap();
    git(&folder, &["init", "-q", "-b", "main"]);
    git(&folder, &["commit", "-q", "--allow-empty", "-m", "mine"]);
    let (_status, code) = h
        .err(
            "workspace.add",
            json!({ "path": folder, "publish": { "owner": "octo", "name": "lib" } }),
        )
        .await;
    assert_eq!(code, "already_exists");
    assert_eq!(out(&folder, &["log", "--format=%s"]), "mine");
    assert_eq!(out(&folder, &["remote"]), "");
    assert_eq!(
        out(&folder, &["config", "--local", "--get-regexp", "^user\\."]),
        ""
    );
}

#[tokio::test]
async fn a_network_error_on_create_still_checks_whether_it_landed() {
    // `repo create` dies of a network error after GitHub actually made the
    // repository (a dropped response, say); `octo/taken` is the fixture's
    // stand-in for "exists".
    let gh = SIGNED_IN_GH.replace(
        "\"repo create\"*)",
        "\"repo create\"*) echo 'error connecting to api.github.com' >&2; exit 1;;\n\"never\")",
    );
    let h = host_with_gh(false, Some(&gh)).await;
    let folder = plain_folder(&h, "app");
    let (_status, code, details) = h
        .err_details(
            "workspace.add",
            json!({ "path": folder, "publish": { "owner": "octo", "name": "taken" } }),
        )
        .await;
    assert_eq!(code, "publish_failed");
    assert_eq!(details, Some(json!({ "created_on_github": true })));
}

#[tokio::test]
async fn a_network_error_on_create_with_no_repository_is_unreachable() {
    // Offline: no new name fixes this, so it is not `publish_failed`.
    let gh = SIGNED_IN_GH.replace(
        "\"repo create\"*)",
        "\"repo create\"*) echo 'error connecting to api.github.com' >&2; exit 1;;\n\"never\")",
    );
    let h = host_with_gh(false, Some(&gh)).await;
    let folder = plain_folder(&h, "app");
    let (_status, code) = h
        .err(
            "workspace.add",
            json!({ "path": folder, "publish": { "owner": "octo", "name": "app" } }),
        )
        .await;
    assert_eq!(code, "github_unreachable");
    assert!(!folder.join(".git").exists());
}

#[tokio::test]
async fn a_failed_publish_keeps_a_local_user_name() {
    let gh = SIGNED_IN_GH.replace(
        "\"repo create\"*)",
        "\"repo create\"*) echo 'GraphQL: Something went wrong' >&2; exit 1;;\n\"never\")",
    );
    let h = host_with_gh(false, Some(&gh)).await;
    let folder = h._dir.path().join("lib");
    std::fs::create_dir(&folder).unwrap();
    git(&folder, &["init", "-q", "-b", "main"]);
    git(&folder, &["commit", "-q", "--allow-empty", "-m", "mine"]);
    git(&folder, &["config", "--local", "user.name", "Mine"]);
    let (_status, code, _details) = h
        .err_details(
            "workspace.add",
            json!({ "path": folder, "publish": { "owner": "octo", "name": "lib" } }),
        )
        .await;
    assert_eq!(code, "publish_failed");
    assert_eq!(
        out(&folder, &["config", "--local", "--get-regexp", "^user\\."]),
        "user.name Mine"
    );
}

#[tokio::test]
async fn add_with_publish_refuses_a_folder_with_origin() {
    let h = host_with_gh(false, Some(SIGNED_IN_GH)).await;
    git(
        std::path::Path::new(&h.root),
        &["remote", "add", "origin", h.bare.to_str().unwrap()],
    );
    let (_status, code) = h
        .err(
            "workspace.add",
            json!({ "path": h.root, "publish": { "owner": "octo", "name": "x" } }),
        )
        .await;
    assert_eq!(code, "invalid_input");
}

#[tokio::test]
async fn add_with_publish_and_no_gh_touches_nothing() {
    let h = host(false).await;
    let folder = plain_folder(&h, "app");
    let (_status, code) = h
        .err(
            "workspace.add",
            json!({ "path": folder, "publish": { "owner": "octo", "name": "app" } }),
        )
        .await;
    assert_eq!(code, "gh_unavailable");
    assert!(!folder.join(".git").exists());
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
    assert_eq!(list.as_array().unwrap().len(), 3);
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
        crate::workspace::add(&h.state.db, &h.state.emitter, &path, false, None),
    )
    .await;
    assert!(dropped.is_err(), "the add was still waiting");
    drop(held);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let list = h.ok("workspace.list", json!({})).await;
        if list.as_array().unwrap().len() == 3 {
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
    assert_eq!(list.as_array().unwrap().len(), 2, "nothing registered");

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
    // With a commit: an unborn repository is measured (D12).
    git(&dir, &["commit", "-q", "--allow-empty", "-m", "mine"]);
    for i in 0..5_001 {
        std::fs::write(dir.join(format!("f{i}")), "").unwrap();
    }
    let path = dir.canonicalize().unwrap().to_string_lossy().into_owned();
    let added = h.ok("workspace.add", json!({"path": path})).await;
    assert_eq!(added["is_git_repo"], true);
    assert!(!dir.join(".gitignore").exists());
}

/// A repository on `trunk` with no commit yet and one file in it.
fn unborn_repository(h: &Host, name: &str) -> std::path::PathBuf {
    let folder = h._dir.path().join(name);
    std::fs::create_dir(&folder).unwrap();
    git(&folder, &["init", "-q", "-b", "trunk"]);
    std::fs::write(folder.join("a.txt"), "a").unwrap();
    folder.canonicalize().unwrap()
}

#[tokio::test]
async fn workspace_inspect_reads_an_unborn_repository() {
    let h = host_with_gh(false, Some(SIGNED_IN_GH)).await;
    let folder = unborn_repository(&h, "fresh");
    let v = h.ok("workspace.inspect", json!({ "path": folder })).await;
    assert_eq!(v["git"], "no_origin");
    assert_eq!(v["branch"], "trunk");
    assert_eq!(v["first_commit"]["files"], 1);
    assert_eq!(v["first_commit"]["large"], false);
    assert_eq!(v["github"]["available"], true);
    assert!(
        !folder.join(".gitignore").exists(),
        "inspect touches nothing"
    );
}

#[tokio::test]
async fn workspace_add_measures_an_unborn_repository() {
    let h = host(false).await;
    let folder = unborn_repository(&h, "bigfresh");
    for i in 0..5_001 {
        std::fs::write(folder.join(format!("f{i}")), "").unwrap();
    }
    let (status, code, details) = h
        .err_details("workspace.add", json!({ "path": folder }))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::CONFLICT, "large_folder")
    );
    assert_eq!(details.expect("details")["at_least"], true);
    assert!(!folder.join(".gitignore").exists());
    assert_eq!(out(&folder, &["rev-parse", "-q", "--verify", "HEAD"]), "");
    let list = h.ok("workspace.list", json!({})).await;
    assert_eq!(list.as_array().unwrap().len(), 2, "nothing registered");
}

#[tokio::test]
async fn workspace_add_commits_an_unborn_repository() {
    let h = host(false).await;
    let folder = unborn_repository(&h, "fresh");
    let added = h.ok("workspace.add", json!({ "path": folder })).await;
    assert_eq!(added["is_git_repo"], true);
    assert_eq!(
        git_out(&folder, &["log", "--format=%s", "trunk"]),
        "Initial Commit"
    );
    assert_eq!(git_out(&folder, &["branch", "--show-current"]), "trunk");
    assert_eq!(
        git_out(&folder, &["ls-tree", "-r", "--name-only", "HEAD"]),
        ".gitignore\na.txt"
    );
    assert_eq!(out(&folder, &["remote"]), "");
    // A worktree can be made from it now.
    h.ok(
        "workspace.create_worktree",
        json!({ "workspace_id": added["workspace_id"] }),
    )
    .await;
}

#[tokio::test]
async fn workspace_add_commits_and_publishes_an_unborn_repository() {
    let h = host_with_gh(false, Some(SIGNED_IN_GH)).await;
    let folder = unborn_repository(&h, "fresh");
    std::fs::write(folder.join(".env"), "SECRET=1").unwrap();
    h.ok(
        "workspace.add",
        json!({ "path": folder, "publish": { "owner": "octo", "name": "fresh" } }),
    )
    .await;
    assert_eq!(
        git_out(&folder, &["log", "--format=%s", "trunk"]),
        "Initial Commit"
    );
    assert_eq!(out(&folder, &["remote"]), "origin");
    assert_eq!(
        out(&folder, &["log", "-1", "--format=%ae"]),
        "42+octo@users.noreply.github.com"
    );
    assert_eq!(
        out(&h.bare, &["log", "--format=%s", "trunk"]),
        "Initial Commit"
    );
    let pushed = out(&h.bare, &["ls-tree", "-r", "--name-only", "trunk"]);
    assert_eq!(pushed.lines().collect::<Vec<_>>(), [".gitignore", "a.txt"]);
}

#[tokio::test]
async fn workspace_add_failed_publish_restores_an_unborn_repository() {
    // gh adds `origin` and pushes with `-u` before it fails.
    let gh = SIGNED_IN_GH.replace(
        "push -q -u origin HEAD;;",
        "push -q -u origin HEAD; echo 'GraphQL: Something went wrong' >&2; exit 1;;",
    );
    let h = host_with_gh(false, Some(&gh)).await;
    let folder = unborn_repository(&h, "fresh");
    git(&folder, &["add", "a.txt"]);
    std::fs::write(folder.join(".git/zwiim-sentinel"), "mine").unwrap();
    let exclude = std::fs::read(folder.join(".git/info/exclude")).ok();
    let (_status, code, _details) = h
        .err_details(
            "workspace.add",
            json!({ "path": folder, "publish": { "owner": "octo", "name": "fresh" } }),
        )
        .await;
    assert_eq!(code, "publish_failed");
    assert_eq!(out(&folder, &["rev-parse", "-q", "--verify", "HEAD"]), "");
    assert_eq!(out(&folder, &["symbolic-ref", "HEAD"]), "refs/heads/trunk");
    assert_eq!(out(&folder, &["diff", "--cached", "--name-only"]), "a.txt");
    assert_eq!(
        std::fs::read_to_string(folder.join(".git/zwiim-sentinel")).unwrap(),
        "mine"
    );
    assert!(!folder.join(".gitignore").exists());
    assert_eq!(out(&folder, &["remote"]), "");
    assert_eq!(
        out(
            &folder,
            &["config", "--local", "--get-regexp", "^branch\\."]
        ),
        ""
    );
    assert_eq!(
        std::fs::read(folder.join(".git/info/exclude")).ok(),
        exclude
    );
    assert_eq!(out(&folder, &["config", "--local", "user.email"]), "");
    let list = h.ok("workspace.list", json!({})).await;
    assert!(!list.to_string().contains("\"fresh\""), "{list}");
}

#[tokio::test]
async fn workspace_add_registers_an_orphan_branch_without_a_commit() {
    let h = host(false).await;
    let folder = h._dir.path().join("pages");
    std::fs::create_dir(&folder).unwrap();
    git(&folder, &["init", "-q", "-b", "main"]);
    git(&folder, &["commit", "-q", "--allow-empty", "-m", "base"]);
    git(&folder, &["checkout", "-q", "--orphan", "gh-pages"]);
    std::fs::write(folder.join("a.txt"), "a").unwrap();
    let added = h.ok("workspace.add", json!({ "path": folder })).await;
    assert_eq!(added["is_git_repo"], true);
    assert_eq!(git_out(&folder, &["log", "--all", "--format=%s"]), "base");
    assert_eq!(out(&folder, &["rev-parse", "-q", "--verify", "HEAD"]), "");
    assert!(!folder.join(".gitignore").exists());
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
    assert_eq!(list.as_array().unwrap().len(), 2, "nothing registered");
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
        .ok(
            "files.read",
            json!({"workspace_id": wid, "path": "notes.md"}),
        )
        .await;
    h.ok(
        "files.write",
        json!({"workspace_id": wid, "path": "notes.md", "content": "hi\n", "etag": read["etag"]}),
    )
    .await;
    assert_eq!(
        h.ok(
            "files.read",
            json!({"workspace_id": wid, "path": "notes.md"})
        )
        .await["content"],
        "hi\n"
    );
    let tree = h.ok("workspace.tree", json!({"workspace_id": wid})).await;
    assert!(tree.to_string().contains("\"notes.md\""), "{tree}");

    // A terminal never starts in a root workspace, plain or git.
    assert_eq!(
        h.err(
            "terminal.spawn",
            json!({"workspace_id": wid, "shell": "/bin/sh"})
        )
        .await,
        (StatusCode::CONFLICT, "root_workspace".into())
    );

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
        (
            "git.pr_auto_merge",
            json!({"workspace_id": wid, "number": 1, "enabled": false}),
        ),
    ] {
        assert_eq!(
            h.err(op, body).await,
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                "not_a_git_repository".into()
            ),
            "{op}"
        );
    }
    assert_eq!(
        h.ok("workspace.list", json!({}))
            .await
            .as_array()
            .unwrap()
            .len(),
        3,
        "no worktree made"
    );
}

#[tokio::test]
async fn a_terminal_starts_in_the_worktree_never_the_root() {
    let h = host(false).await;

    let tid = h
        .ok(
            "terminal.spawn",
            json!({"workspace_id": h.wt_workspace_id, "shell": "/bin/sh"}),
        )
        .await["terminal_id"]
        .as_str()
        .unwrap()
        .to_string();
    // The worktree's folder name only: responses never carry a host path.
    h.ok(
        "terminal.write",
        json!({"terminal_id": tid, "data": "echo \"zw:$(basename \"$PWD\")\"\n"}),
    )
    .await;
    let wt_name = std::path::Path::new(&h.wt)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut seen = false;
    for _ in 0..50 {
        let snap = h.ok("terminal.snapshot", json!({"terminal_id": tid})).await;
        if snap["data"]
            .as_str()
            .unwrap_or_default()
            .contains(&format!("zw:{wt_name}"))
        {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    h.ok("terminal.kill", json!({"terminal_id": tid})).await;
    assert!(seen, "pwd never printed the worktree's folder");
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
    let list = h
        .ok(
            "session.list",
            json!({"workspace_id": h.wt_workspace_id.clone()}),
        )
        .await;
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
async fn session_history_carries_the_history_generation() {
    let h = host(false).await;
    // No agent attached: there is no live generation to report.
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let idle = session_id_of(&h.state, conv).await;
    let history = h.ok("session.history", json!({"session_id": idle})).await;
    assert!(history["history_generation"].is_null(), "{history}");

    // Live: the same generation the session's snapshot carries, and a new
    // one once a turn completes.
    let (sid, _rx) = h.live_session().await;
    let live = h.state.connection_manager.get_state(CONN).await.unwrap();
    let snap_gen = live.read().await.to_snapshot().history_generation;
    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert_eq!(history["history_generation"].as_u64(), Some(snap_gen));

    h.emit(AcpEvent::TurnComplete {
        session_id: "agent-native".into(),
        stop_reason: "end_turn".into(),
        agent_type: "claude_code".into(),
    })
    .await;
    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert!(history["history_generation"].as_u64().unwrap() > snap_gen);
}

/// Save a can't-reach turn of `blocks` on session `sid`.
async fn save_unreachable_turn(h: &Host, sid: &str, blocks: Vec<PromptInputBlock>) {
    let row = crate::session::resolve(&h.state.db.conn, sid)
        .await
        .unwrap();
    crate::db::service::conversation_service::set_unreachable_turn(
        &h.state.db.conn,
        row.id,
        Some(&crate::session_retry::UnreachableTurn {
            message: "Can't reach the agent's server.".into(),
            blocks,
        }),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn session_retry_over_http() {
    let h = host(false).await;
    assert_eq!(
        h.err("session.retry", json!({"session_id": "nope"}))
            .await
            .1,
        "not_found"
    );

    let (sid, mut rx) = h.live_session().await;
    assert_eq!(
        h.err("session.retry", json!({"session_id": sid})).await,
        (StatusCode::CONFLICT, "nothing_to_retry".into())
    );

    let blocks = vec![PromptInputBlock::Text { text: "hi".into() }];
    save_unreachable_turn(&h, &sid, blocks.clone()).await;
    let retried = h.ok("session.retry", json!({"session_id": sid})).await;
    assert_eq!(retried, json!({"session_id": sid}));
    match rx.recv().await {
        Some(ConnectionCommand::Prompt { blocks: sent, .. }) => assert_eq!(sent, blocks),
        _ => panic!("the saved turn reaches the agent"),
    }
    // Sent, so the record is gone.
    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert!(history["last_error"].is_null(), "{history}");
}

/// The op's refusals over HTTP. Its success path starts an agent, which only
/// the orchestration tests can fake (`session_retry::tests`).
#[tokio::test]
async fn session_retry_in_new_chat_over_http() {
    let h = host(false).await;
    assert_eq!(
        h.err("session.retry_in_new_chat", json!({"session_id": "nope"}))
            .await
            .1,
        "not_found"
    );

    let (sid, _rx) = h.live_session().await;
    assert_eq!(
        h.err("session.retry_in_new_chat", json!({"session_id": sid})).await,
        (StatusCode::CONFLICT, "nothing_to_retry".into())
    );

    save_unreachable_turn(&h, &sid, vec![PromptInputBlock::Text { text: "hi".into() }]).await;
    let st = h.state.connection_manager.get_state(CONN).await.unwrap();
    st.write().await.turn_in_flight = true;
    assert_eq!(
        h.err("session.retry_in_new_chat", json!({"session_id": sid}))
            .await
            .1,
        "turn_in_progress"
    );

    // A session in a repository's root workspace has no worktree to start in.
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::Codex).await;
    let root_sid = session_id_of(&h.state, conv).await;
    save_unreachable_turn(&h, &root_sid, vec![PromptInputBlock::Text { text: "hi".into() }]).await;
    assert_eq!(
        h.err("session.retry_in_new_chat", json!({"session_id": root_sid}))
            .await
            .1,
        "root_workspace"
    );
}

#[tokio::test]
async fn history_carries_saved_last_error() {
    let h = host(false).await;
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::Codex).await;
    let sid = session_id_of(&h.state, conv).await;
    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert!(
        history.get("last_error").is_some_and(Value::is_null),
        "no record reads as null: {history}"
    );

    save_unreachable_turn(&h, &sid, vec![PromptInputBlock::Text { text: "hi".into() }]).await;
    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert_eq!(
        history["last_error"],
        json!({"message": "Can't reach the agent's server.", "code": "agent_unreachable"})
    );
    assert_no_internal_keys(&history);
}

/// The newest saved plan comes back in history, titles relativized like the
/// live `agent.progress`. A plan saved as running with no turn running was
/// cut short by a Host that stopped, so it reads as interrupted.
#[tokio::test]
async fn history_carries_saved_plan() {
    use crate::acp::progress::{ProgressStep, StepStatus};
    use crate::session_plans::{PlanRecord, PlanState};

    let h = host(false).await;
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::Codex).await;
    let sid = session_id_of(&h.state, conv).await;
    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert!(
        history.get("progress").is_some_and(Value::is_null),
        "no plan reads as null: {history}"
    );

    let root = crate::session::workspace_dir(&h.state.db.conn, &sid)
        .await
        .expect("workspace dir");
    let record = PlanRecord {
        v: 1,
        plan_id: "7d0f9a3c-0000-4000-8000-000000000001".into(),
        session_id: sid.clone(),
        agent: AgentType::Codex,
        revision: 3,
        state: PlanState::Running,
        steps: vec![
            ProgressStep {
                title: format!("Edit {root}/src/main.rs"),
                status: StepStatus::Completed,
            },
            ProgressStep {
                title: "Run tests".into(),
                status: StepStatus::InProgress,
            },
        ],
        created_at_ms: 1,
        updated_at_ms: 2,
    };
    crate::session_plans::save(std::path::Path::new(&root), &record)
        .await
        .unwrap();

    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert_eq!(
        history["progress"],
        json!({
            "plan_id": record.plan_id,
            "state": "interrupted",
            "agent": "codex",
            "steps": [
                {"title": "Edit src/main.rs", "status": "completed"},
                {"title": "Run tests", "status": "in_progress"},
            ],
            "total": 2,
            "completed": 1,
            "resumable": true,
        })
    );
    assert_no_internal_keys(&history);
}

/// A stopped plan with steps left reads as resumable; once a new session
/// takes it over, history says which one and the plan is no longer offered.
#[tokio::test]
async fn history_reports_resumable_and_continued_in() {
    use crate::acp::progress::{ProgressStep, StepStatus};
    use crate::session_plans::{ContinuedIn, PlanRecord, PlanState};

    let h = host(false).await;
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::Codex).await;
    let sid = session_id_of(&h.state, conv).await;
    let next = seed_conversation(&h.state.db, h.folder_id, AgentType::Codex).await;
    let next_sid = session_id_of(&h.state, next).await;
    let root = crate::session::workspace_dir(&h.state.db.conn, &sid)
        .await
        .expect("workspace dir");
    let root = std::path::Path::new(&root);
    let record = PlanRecord {
        v: 1,
        plan_id: "7d0f9a3c-0000-4000-8000-000000000003".into(),
        session_id: sid.clone(),
        agent: AgentType::Codex,
        revision: 3,
        state: PlanState::Interrupted,
        steps: vec![
            ProgressStep {
                title: "Edit".into(),
                status: StepStatus::Completed,
            },
            ProgressStep {
                title: "Run tests".into(),
                status: StepStatus::Pending,
            },
        ],
        created_at_ms: 1,
        updated_at_ms: 2,
    };
    crate::session_plans::save(root, &record).await.unwrap();

    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert_eq!(history["progress"]["resumable"], json!(true));
    assert!(history["continued_in"].is_null());

    crate::session_plans::save_continued_in(
        root,
        &sid,
        &ContinuedIn {
            session_id: next_sid.clone(),
            plan_id: record.plan_id.clone(),
        },
    )
    .await
    .unwrap();
    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert_eq!(history["progress"]["resumable"], json!(false));
    assert_eq!(history["continued_in"], json!({"session_id": next_sid}));
    assert_no_internal_keys(&history);
}

/// The op's refusals over HTTP, and the link history reports. Its success
/// path starts an agent, which only the orchestration tests can fake
/// (`session_continue::tests`).
#[tokio::test]
async fn session_continue_plan_over_http() {
    use crate::acp::progress::{ProgressStep, StepStatus};
    use crate::session_plans::{PlanLink, PlanRecord, PlanState};

    let unfinished = |sid: &str| PlanRecord {
        v: 1,
        plan_id: "7d0f9a3c-0000-4000-8000-000000000002".into(),
        session_id: sid.to_string(),
        agent: AgentType::ClaudeCode,
        revision: 2,
        state: PlanState::Interrupted,
        steps: vec![ProgressStep {
            title: "Run tests".into(),
            status: StepStatus::Pending,
        }],
        created_at_ms: 1,
        updated_at_ms: 2,
    };
    let h = host(false).await;
    assert_eq!(
        h.err("session.continue_plan", json!({"session_id": "nope"}))
            .await
            .1,
        "not_found"
    );

    let (sid, _rx) = h.live_session().await;
    assert_eq!(
        h.err("session.continue_plan", json!({"session_id": sid})).await,
        (StatusCode::CONFLICT, "nothing_to_continue".into())
    );
    assert_eq!(
        h.err(
            "session.continue_plan",
            json!({"session_id": sid, "agent": "nope"})
        )
        .await
        .1,
        "invalid_request"
    );

    let root = crate::session::workspace_dir(&h.state.db.conn, &sid)
        .await
        .expect("workspace dir");
    let root = std::path::Path::new(&root);
    crate::session_plans::save(root, &unfinished(&sid))
        .await
        .unwrap();
    let st = h.state.connection_manager.get_state(CONN).await.unwrap();
    st.write().await.turn_in_flight = true;
    assert_eq!(
        h.err("session.continue_plan", json!({"session_id": sid}))
            .await
            .1,
        "turn_in_progress"
    );

    // A session in a repository's root workspace has no worktree to start in.
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::Codex).await;
    let root_sid = session_id_of(&h.state, conv).await;
    let root_dir = crate::session::workspace_dir(&h.state.db.conn, &root_sid)
        .await
        .expect("workspace dir");
    crate::session_plans::save(std::path::Path::new(&root_dir), &unfinished(&root_sid))
        .await
        .unwrap();
    assert_eq!(
        h.err("session.continue_plan", json!({"session_id": root_sid}))
            .await
            .1,
        "root_workspace"
    );

    // History names the plan a session continues.
    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert!(history.get("continues").is_some_and(Value::is_null));
    let link = PlanLink {
        session_id: root_sid.clone(),
        plan_id: "7d0f9a3c-0000-4000-8000-000000000002".into(),
        agent: Some(AgentType::Codex),
        completed: 1,
        total: 3,
        title: Some("Fix the leak".into()),
    };
    crate::session_plans::save_link(root, &sid, &link)
        .await
        .unwrap();
    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert_eq!(
        history["continues"],
        json!({
            "session_id": root_sid,
            "plan_id": link.plan_id,
            "agent": "codex",
            "completed": 1,
            "total": 3,
            "title": "Fix the leak",
        })
    );
    assert_no_internal_keys(&history);
}

/// Taking a plan over changes the old session (its panel stops offering
/// Continue), so every device is told. The success path starts an agent,
/// which this harness cannot fake, so this drives the two steps the handler
/// takes after the new session exists: the old session's bump and the
/// announcement.
#[tokio::test]
async fn continue_emits_session_changed_for_old_session() {
    let h = host(true).await;
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let sid = session_id_of(&h.state, conv).await;
    let (mut ws, _) = h.feed().await;

    crate::db::service::conversation_service::touch(&h.state.db.conn, conv)
        .await
        .unwrap();
    crate::protocol::http::announce_session_changed(&h.state, conv).await;
    let changed = event_is("session.changed", |p| p["session"]["session_id"] == sid.as_str());
    h.host_event(&mut ws, "old session changed", &changed).await;
}

/// The new session is titled after it exists, so the handler announces it
/// again: the announcement reads the row when it is sent and carries the
/// "(continued)" title, not the one the session started with.
#[tokio::test]
async fn continue_announces_the_new_sessions_title() {
    let h = host(true).await;
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::Codex).await;
    let sid = session_id_of(&h.state, conv).await;
    let (mut ws, _) = h.feed().await;

    crate::db::service::conversation_service::update_title(
        &h.state.db.conn,
        conv,
        "Fix the leak (continued)".into(),
    )
    .await
    .unwrap();
    crate::protocol::http::announce_session_changed(&h.state, conv).await;
    let titled = event_is("session.changed", |p| {
        p["session"]["session_id"] == sid.as_str()
            && p["session"]["title"] == "Fix the leak (continued)"
    });
    h.host_event(&mut ws, "new session titled", &titled).await;
}

/// `session.retry_in_new_chat` and `session.continue_plan` announce the old
/// and the new session together, so every device's list shows the new title
/// and where the old chat's plan went. The success path starts an agent,
/// which this harness cannot fake, so this drives the announcement the
/// handlers make once the new session exists.
#[tokio::test]
async fn announcing_two_sessions_tells_every_device_about_both() {
    let h = host(true).await;
    let old = seed_conversation(&h.state.db, h.folder_id, AgentType::Codex).await;
    let old_sid = session_id_of(&h.state, old).await;
    let new = seed_conversation(&h.state.db, h.folder_id, AgentType::Codex).await;
    let new_sid = session_id_of(&h.state, new).await;
    let (mut ws, _) = h.feed().await;

    // The feed only tells devices about a session whose listing changed.
    crate::db::service::conversation_service::touch(&h.state.db.conn, old)
        .await
        .unwrap();
    crate::db::service::conversation_service::update_title(
        &h.state.db.conn,
        new,
        "Fix the leak (continued)".into(),
    )
    .await
    .unwrap();
    crate::protocol::http::announce_sessions_changed(
        &h.state,
        &[old_sid.as_str(), new_sid.as_str()],
    )
    .await;

    // Both arrive, in whichever order the list is sorted.
    let seen = std::cell::RefCell::new(std::collections::HashSet::new());
    let both = event_is("session.changed", |p| {
        let id = p["session"]["session_id"].as_str().unwrap().to_string();
        seen.borrow_mut().insert(id);
        seen.borrow().contains(&old_sid) && seen.borrow().contains(&new_sid)
    });
    h.host_event(&mut ws, "both sessions changed", &both).await;
}

/// The saved message is the agent's own text, kept raw; history scrubs it
/// like the live `agent.error` does, so a host path or a refused key never
/// reaches a client.
#[tokio::test]
async fn history_scrubs_saved_last_error() {
    let h = host(false).await;
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::Codex).await;
    let sid = session_id_of(&h.state, conv).await;
    let raw = "Key sk-proj-abcdefgh1234 refused, see /Users/alice/.codex/config.toml";
    crate::db::service::conversation_service::set_unreachable_turn(
        &h.state.db.conn,
        conv,
        Some(&crate::session_retry::UnreachableTurn {
            message: raw.into(),
            blocks: vec![PromptInputBlock::Text { text: "hi".into() }],
        }),
    )
    .await
    .unwrap();

    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert_eq!(
        history["last_error"]["message"],
        "Key <redacted> refused, see <path>"
    );
    let stored = crate::db::service::conversation_service::unreachable_turn(&h.state.db.conn, conv)
        .await
        .unwrap()
        .expect("record");
    assert_eq!(stored.message, raw, "the stored text stays raw");
}

#[tokio::test]
async fn session_revert_speaks_zwiim_only() {
    let h = host(false).await;
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let sid = session_id_of(&h.state, conv).await;

    // Every user turn says whether it can be reverted; none here can.
    let history = h.ok("session.history", json!({"session_id": sid})).await;
    for turn in history["turns"].as_array().unwrap() {
        if turn["role"] == "user" {
            assert_eq!(turn["revertible"], false, "{turn}");
        }
    }

    let revert = |turn_id: &str, files: bool, chat: bool| {
        json!({"session_id": sid, "turn_id": turn_id, "files": files, "chat": chat})
    };
    assert_eq!(
        h.err("session.revert", revert("turn-404", true, true)).await,
        (StatusCode::CONFLICT, "checkpoint_unavailable".into())
    );
    assert_eq!(
        h.err("session.revert", revert("turn-404", false, false)).await,
        (StatusCode::BAD_REQUEST, "invalid_input".into())
    );
    assert_eq!(
        h.err(
            "session.revert",
            json!({"session_id": "nope", "turn_id": "t", "files": true, "chat": false})
        )
        .await,
        (StatusCode::NOT_FOUND, "not_found".into())
    );
    assert_eq!(
        h.err(
            "session.revert",
            json!({"session_id": sid, "turn_id": "t", "files": true, "chat": false, "ordinal": 1})
        )
        .await,
        (StatusCode::BAD_REQUEST, "invalid_request".into())
    );
    assert_eq!(
        h.err("session.revert", json!({"session_id": sid, "turn_id": "t"}))
            .await,
        (StatusCode::BAD_REQUEST, "invalid_request".into())
    );
}

/// The op's refusals over HTTP. Its success path starts an agent, which only
/// the orchestration tests can fake (`session_fork::tests`).
#[tokio::test]
async fn session_fork_speaks_zwiim_only() {
    let h = host(false).await;
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let sid = session_id_of(&h.state, conv).await;
    let fork = |session_id: &str, target: &str| {
        json!({"session_id": session_id, "turn_id": "turn-404", "target": target})
    };
    assert_eq!(
        h.err("session.fork", fork(&sid, "current")).await,
        (StatusCode::CONFLICT, "fork_unavailable".into())
    );
    assert_eq!(
        h.err("session.fork", fork(&sid, "new_worktree")).await,
        (StatusCode::CONFLICT, "fork_unavailable".into())
    );
    assert_eq!(
        h.err("session.fork", fork("nope", "current")).await,
        (StatusCode::NOT_FOUND, "not_found".into())
    );
    assert_eq!(
        h.err("session.fork", fork(&sid, "elsewhere")).await,
        (StatusCode::BAD_REQUEST, "invalid_request".into())
    );
    assert_eq!(
        h.err(
            "session.fork",
            json!({"session_id": sid, "turn_id": "t", "target": "current", "ordinal": 1})
        )
        .await,
        (StatusCode::BAD_REQUEST, "invalid_request".into())
    );
}

#[tokio::test]
async fn transcript_remove_speaks_zwiim_only() {
    let h = host(false).await;
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let sid = session_id_of(&h.state, conv).await;

    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert!(history["pending_transcript"].is_null(), "{history}");
    h.ok("session.transcript_remove", json!({"session_id": sid})).await;
    assert_eq!(
        h.err("session.transcript_remove", json!({"session_id": "nope"})).await,
        (StatusCode::NOT_FOUND, "not_found".into())
    );
}

/// The chip lives in `session.history`, which devices reread on `session.changed`,
/// and the feed only sends that for a summary that differs.
#[tokio::test]
async fn transcript_remove_tells_every_device() {
    use sea_orm::{ActiveModelTrait, Set};

    let h = host(true).await;
    let source = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let source = session_id_of(&h.state, source).await;
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let sid = session_id_of(&h.state, conv).await;
    crate::db::entities::session_fork::ActiveModel {
        session_id: Set(sid.clone()),
        source_session_id: Set(source),
        source_turn_id: Set("turn-1".into()),
        created_at: Set(chrono::Utc::now()),
        pending_transcript: Set(Some("/nowhere/Transcript of Old work.md".into())),
        transcript_sent: Set(false),
        notice_owed: Set(false),
        pending_options: Set(None),
        worktree_files: Set(None),
    }
    .insert(&h.state.db.conn)
    .await
    .unwrap();
    let (mut ws, _) = h.feed().await;

    h.ok("session.transcript_remove", json!({"session_id": sid})).await;
    let changed = event_is("session.changed", |p| p["session"]["session_id"] == sid.as_str());
    h.host_event(&mut ws, "transcript taken back", &changed).await;
}

#[tokio::test]
async fn history_names_the_session_a_fork_came_from() {
    use sea_orm::{ActiveModelTrait, Set};

    let h = host(false).await;
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let source = session_id_of(&h.state, conv).await;
    crate::db::service::conversation_service::update_title(&h.state.db.conn, conv, "Fix the leak".into())
        .await
        .unwrap();
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let sid = session_id_of(&h.state, conv).await;

    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert!(history["forked_from"].is_null(), "{history}");

    crate::db::entities::session_fork::ActiveModel {
        session_id: Set(sid.clone()),
        source_session_id: Set(source.clone()),
        source_turn_id: Set("turn-1".into()),
        created_at: Set(chrono::Utc::now()),
        pending_transcript: Set(None),
        transcript_sent: Set(false),
        notice_owed: Set(false),
        pending_options: Set(None),
        worktree_files: Set(None),
    }
    .insert(&h.state.db.conn)
    .await
    .unwrap();
    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert_eq!(
        history["forked_from"],
        json!({"session_id": source, "title": "Fix the leak"})
    );

    // A source that is gone keeps its id, for a client to say so, and no title.
    h.ok("session.delete", json!({"session_id": source})).await;
    let history = h.ok("session.history", json!({"session_id": sid})).await;
    assert_eq!(
        history["forked_from"],
        json!({"session_id": source, "title": null})
    );
}

#[tokio::test]
async fn sessions_and_terminals_never_start_in_a_root() {
    let h = host(false).await;
    assert_eq!(
        h.err(
            "session.start",
            json!({"workspace_id": h.workspace_id, "agent": "claude_code", "blocks": [{"type": "text", "text": "hi"}]})
        )
        .await,
        (StatusCode::CONFLICT, "root_workspace".into())
    );
    assert_eq!(
        h.err("terminal.spawn", json!({"workspace_id": h.workspace_id}))
            .await,
        (StatusCode::CONFLICT, "root_workspace".into())
    );
    assert!(h
        .ok("terminal.list", json!({}))
        .await
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_session_that_ran_in_a_root_cannot_continue_there() {
    let h = host(false).await;
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let sid = session_id_of(&h.state, conv).await;
    assert_eq!(
        h.err(
            "session.send",
            json!({"session_id": sid, "blocks": [{"type": "text", "text": "hi"}]})
        )
        .await,
        (StatusCode::CONFLICT, "root_workspace".into())
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
    assert_eq!(stale.0, StatusCode::CONFLICT, "{}", stale.1);
    assert_eq!(stale.1["error"]["code"], "file_changed", "{}", stale.1);
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
        h.ok("git.pr", json!({"workspace_id": wid, "refresh": true}))
            .await,
        json!({"pr": null, "error": null})
    );
    assert_eq!(
        h.err("git.pr", json!({"workspace_id": "nope"})).await.1,
        "not_found"
    );
    // A merge names the head it was reviewed at.
    assert_eq!(
        h.err("git.pr_merge", json!({"workspace_id": wid, "number": 1}))
            .await,
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
    // Auto-merge says on or off, and turning it on names the head.
    assert_eq!(
        h.err("git.pr_auto_merge", json!({"workspace_id": wid, "number": 1}))
            .await,
        (StatusCode::BAD_REQUEST, "invalid_request".into())
    );
    assert_eq!(
        h.err(
            "git.pr_auto_merge",
            json!({"workspace_id": wid, "number": 1, "enabled": true})
        )
        .await,
        (StatusCode::BAD_REQUEST, "invalid_input".into())
    );
    assert_eq!(
        h.err(
            "git.pr_auto_merge",
            json!({"workspace_id": "nope", "number": 1, "enabled": false})
        )
        .await
        .1,
        "not_found"
    );
}

#[test]
fn git_errors_keep_their_codes_and_never_gh_s_words() {
    use crate::git_harness::{GitError, GitErrorCode::*};
    let cases = [
        (NotMergeable, StatusCode::CONFLICT, "not_mergeable"),
        (PrChanged, StatusCode::CONFLICT, "pr_changed"),
        (AutoMergeDisabled, StatusCode::CONFLICT, "auto_merge_disabled"),
        (
            GhUnavailable,
            StatusCode::UNPROCESSABLE_ENTITY,
            "gh_unavailable",
        ),
        (
            GhUnauthenticated,
            StatusCode::UNPROCESSABLE_ENTITY,
            "gh_unauthenticated",
        ),
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
    let err = ZwiimError::from(GitError::new(
        NotMergeable,
        "Pull request #3 cannot be merged: Merge conflicts",
    ));
    assert_eq!(
        err.details,
        Some(json!({"reason": "Pull request #3 cannot be merged: Merge conflicts"}))
    );
}

#[tokio::test]
async fn terminal_operations_are_addressed_by_terminal_id() {
    let h = host(false).await;
    let wid = h.wt_workspace_id.clone();
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
    let tab = |id: &str, session_id: &str| crate::browser::store::TabRecord {
        id: id.into(),
        owner: crate::browser::store::TabOwner::Session {
            session_id: session_id.into(),
        },
        url: "http://localhost:5173/".into(),
        title: String::new(),
        position: 0,
        discarded: true,
    };
    crate::browser::store::insert(&h.state.db.conn, &tab("t-idle", &idle))
        .await
        .unwrap();
    crate::browser::store::insert(&h.state.db.conn, &tab("t-other", &sid))
        .await
        .unwrap();
    h.ok("session.delete", json!({"session_id": idle})).await;
    // Its browser tabs go with it; other sessions' stay.
    let tabs = crate::browser::store::load(&h.state.db.conn).await.unwrap();
    assert_eq!(
        tabs.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        vec!["t-other"]
    );
    let listed = h
        .ok(
            "session.list",
            json!({"workspace_id": h.wt_workspace_id.clone()}),
        )
        .await;
    assert!(listed
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["session_id"] != idle.as_str()));
    assert!(listed
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["session_id"] == sid.as_str()));
    assert_eq!(
        h.err("session.delete", json!({"session_id": idle})).await.1,
        "not_found"
    );
    assert_eq!(
        h.err("session.history", json!({"session_id": idle}))
            .await
            .1,
        "not_found"
    );
}

#[tokio::test]
async fn session_stop_over_http() {
    let h = host(false).await;

    // A session with no agent is already stopped.
    let idle_conv = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let idle = session_id_of(&h.state, idle_conv).await;
    assert_eq!(
        h.ok("session.stop", json!({"session_id": idle})).await,
        json!({})
    );
    assert_eq!(
        h.err("session.stop", json!({"session_id": "nope"})).await.1,
        "not_found"
    );

    // Mid-turn, a permission waiting: the agent is told to cancel, ends its
    // turn, and is then disconnected.
    let (sid, mut rx) = h.live_session().await;
    h.ok(
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
    h.ok("session.stop", json!({"session_id": sid})).await;
    assert_eq!(agent.await.unwrap(), ["cancel", "disconnect"]);

    // The session and its transcript stay; it is no longer live.
    let list = h
        .ok(
            "session.list",
            json!({"workspace_id": h.wt_workspace_id.clone()}),
        )
        .await;
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
    h.state
        .connection_manager
        .get_state(CONN)
        .await
        .unwrap()
        .write()
        .await
        .conversation_id = Some(conv);
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
        json!({
            "workspace_id": wt_id,
            "parent_workspace_id": wid,
            "kept_sessions": 1,
            "branch": branch,
            "branch_deleted": false,
            "branch_kept_reason": null,
        })
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
async fn workspace_remove_archives_its_worktrees_and_re_add_revives_it() {
    let h = host(false).await;
    let wid = h.workspace_id.clone();
    let made = h
        .ok("workspace.create_worktree", json!({"workspace_id": wid}))
        .await;
    let made_id = made["workspace_id"].as_str().unwrap().to_string();
    let listed_ids = |list: Value| -> Vec<String> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|w| w["workspace_id"].as_str().unwrap().to_string())
            .collect()
    };

    // Only a repository is removed this way; a worktree is archived.
    assert_eq!(
        h.err("workspace.remove", json!({"workspace_id": made_id}))
            .await
            .1,
        "invalid_input"
    );
    assert_eq!(
        h.err(
            "workspace.remove",
            json!({"workspace_id": wid, "path": "/tmp/x"})
        )
        .await,
        (StatusCode::BAD_REQUEST, "invalid_request".into())
    );

    // Busy: a live agent in the root refuses it, even forced.
    let conv = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let sid = session_id_of(&h.state, conv).await;
    let _rx = h
        .state
        .connection_manager
        .insert_test_connection_live(
            CONN,
            AgentType::ClaudeCode,
            Some(h.root.clone().into()),
            h.state.emitter.clone(),
        )
        .await;
    h.state
        .connection_manager
        .get_state(CONN)
        .await
        .unwrap()
        .write()
        .await
        .conversation_id = Some(conv);
    for force in [false, true] {
        assert_eq!(
            h.err_details(
                "workspace.remove",
                json!({"workspace_id": wid, "force": force})
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

    // Dirty: a change in any worktree refuses it before anything is archived.
    std::fs::write(std::path::Path::new(&h.wt).join("draft.txt"), "x").unwrap();
    assert_eq!(
        h.err_details("workspace.remove", json!({"workspace_id": wid}))
            .await,
        (
            StatusCode::CONFLICT,
            "workspace_dirty".into(),
            Some(json!({"changes": 1}))
        )
    );
    let live = listed_ids(h.ok("workspace.list", json!({})).await);
    for id in [&wid, &h.wt_workspace_id, &made_id] {
        assert!(live.contains(id), "{id} still live");
    }

    // Forced: every worktree archived, the root gone, its files untouched.
    let tid = h
        .ok(
            "terminal.spawn",
            json!({"workspace_id": made_id, "shell": "/bin/sh"}),
        )
        .await["terminal_id"]
        .clone();
    let removed = h
        .ok(
            "workspace.remove",
            json!({"workspace_id": wid, "force": true}),
        )
        .await;
    assert_eq!(
        removed,
        json!({"workspace_id": wid, "archived_worktrees": [h.wt_workspace_id, made_id]})
    );
    assert!(!h
        .ok("terminal.list", json!({}))
        .await
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["terminal_id"] == tid));
    assert!(!std::path::Path::new(&h.wt).exists());
    assert!(std::path::Path::new(&h.root).join(".git").exists());
    let after = listed_ids(h.ok("workspace.list", json!({})).await);
    for id in [&wid, &h.wt_workspace_id, &made_id] {
        assert!(!after.contains(id), "{id} still listed");
    }
    assert_eq!(
        h.err("workspace.open", json!({"workspace_id": wid}))
            .await
            .1,
        "not_found"
    );

    // Added again: the same workspace, its sessions back with it.
    let again = h.ok("workspace.add", json!({"path": h.root.clone()})).await;
    assert_eq!(again["workspace_id"], wid.as_str());
    assert_eq!(again["kind"], "root");
    assert!(listed_ids(h.ok("workspace.list", json!({})).await).contains(&wid));
    let sessions = h.ok("session.list", json!({"workspace_id": wid})).await;
    assert_eq!(sessions[0]["session_id"], sid.as_str());
}

#[tokio::test]
async fn workspace_resolve_conflicts_refuses_before_sending() {
    let h = host(false).await;
    let wid = h.workspace_id.clone();
    let wt = h
        .ok("workspace.create_worktree", json!({"workspace_id": wid}))
        .await;
    let wt_id = wt["workspace_id"].as_str().unwrap().to_string();

    let hello = h.ok("host.hello", json!({})).await;
    assert!(hello["capabilities"]
        .as_array()
        .is_some_and(|c| c.iter().any(|v| v == "workspace.resolve_conflicts")));

    // Only a worktree has a base to resolve against.
    assert_eq!(
        h.err(
            "workspace.resolve_conflicts",
            json!({"workspace_id": wid, "session_id": "nope"})
        )
        .await,
        (StatusCode::CONFLICT, "root_workspace".into())
    );
    assert_eq!(
        h.err(
            "workspace.resolve_conflicts",
            json!({"workspace_id": wt_id, "session_id": "nope"})
        )
        .await,
        (StatusCode::NOT_FOUND, "not_found".into())
    );
    for body in [
        json!({"workspace_id": wt_id}),
        json!({"workspace_id": wt_id, "session_id": "nope", "strategy": "squash"}),
        json!({"workspace_id": wt_id, "session_id": "nope", "text": "hi"}),
    ] {
        assert_eq!(
            h.err("workspace.resolve_conflicts", body).await,
            (StatusCode::BAD_REQUEST, "invalid_request".into())
        );
    }
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
        h.err(
            "workspace.continue",
            json!({"workspace_id": wid, "number": 1})
        )
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
        .err_details(
            "workspace.continue",
            json!({"workspace_id": wt_id, "number": 1}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::CONFLICT, "workspace_dirty")
    );
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
            "background",
            "commands",
            "history_generation",
            "last_error",
            "live_message",
            "options",
            "pending_request",
            "pending_user_message",
            "progress",
            "reconnecting",
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
    h.emit(AcpEvent::UsageUpdate {
        used: Some(1_500),
        size: Some(200_000),
    })
    .await;
    let ev = recv(&mut ws).await;
    assert_eq!(ev["event"]["type"], "session.usage");
    assert_eq!(
        ev["event"]["payload"],
        json!({"used": 1_500, "size": 200_000})
    );
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
async fn ws_history_rewind_sends_subscribers_a_fresh_snapshot() {
    let h = host(true).await;
    let (sid, _rx) = h.live_session().await;
    let mut ws = h.ws().await;
    ws.send_json(&json!({"op": "session.subscribe", "subscription_id": "a", "session_id": sid}))
        .await;
    let snap = recv(&mut ws).await;
    assert_eq!(snap["type"], "snapshot");
    let epoch = snap["epoch"].as_str().unwrap().to_string();
    let base = snap["event_seq"].as_u64().unwrap();
    h.emit(delta("old")).await;
    assert_eq!(recv(&mut ws).await["event"]["type"], "agent.message_delta");

    // The chat was rewound: the subscriber gets the session afresh, which is
    // what makes it reload the transcript, and no event of its own.
    h.emit(AcpEvent::HistoryRewound).await;
    let fresh = recv(&mut ws).await;
    assert_eq!(fresh["type"], "snapshot", "{fresh}");
    assert_eq!(fresh["subscription_id"], "a");
    assert_eq!(fresh["session_id"], sid.as_str());
    assert_eq!(fresh["epoch"], epoch.as_str());
    let rewound = fresh["event_seq"].as_u64().unwrap();
    assert!(rewound > base + 1, "covers the rewind itself");
    h.emit(delta("new")).await;
    let ev = recv(&mut ws).await;
    assert_eq!(ev["event"]["payload"], json!({"text": "new"}));
    assert!(ev["event"]["seq"].as_u64().unwrap() > rewound);
    ws.close().await;

    // A client that was away across the rewind is not replayed the gap: its
    // transcript is no longer the session's.
    let mut ws = h.ws().await;
    ws.send_json(&json!({"op": "session.subscribe", "subscription_id": "b", "session_id": sid, "since_seq": base, "epoch": epoch}))
        .await;
    let f = recv(&mut ws).await;
    assert_eq!(f["type"], "snapshot", "{f}");
    // After the rewind, a gap is replayed as before.
    let mut ws = h.ws().await;
    ws.send_json(&json!({"op": "session.subscribe", "subscription_id": "c", "session_id": sid, "since_seq": rewound, "epoch": epoch}))
        .await;
    let f = recv(&mut ws).await;
    assert_eq!(f["type"], "replay", "{f}");
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
            json!({"workspace_id": h.wt_workspace_id, "shell": "/bin/sh"}),
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
        (
            json!({"workspace_id": w, "agent": "codex", "shell": "/bin/sh"}),
            "invalid_request",
        ),
        (
            json!({"workspace_id": w, "agent": "codex", "initial_command": "ls"}),
            "invalid_request",
        ),
        (
            json!({"workspace_id": w, "command": "/mcp"}),
            "invalid_request",
        ),
        (
            json!({"workspace_id": w, "agent": "gemini", "command": "/mcp"}),
            "invalid_request",
        ),
        (
            json!({"workspace_id": w, "agent": "codex", "command": "mcp"}),
            "invalid_input",
        ),
        (
            json!({"workspace_id": w, "agent": "codex", "command": "/"}),
            "invalid_input",
        ),
        (
            json!({"workspace_id": w, "agent": "codex", "command": "/mcp\r"}),
            "invalid_input",
        ),
        (
            json!({"workspace_id": w, "agent": "codex", "command": "/x\u{1b}[A"}),
            "invalid_input",
        ),
        (
            json!({"workspace_id": w, "agent": "codex", "command": format!("/{}", "a".repeat(200))}),
            "invalid_input",
        ),
    ] {
        assert_eq!(
            h.err("terminal.spawn", body.clone()).await,
            (StatusCode::BAD_REQUEST, code.to_string()),
            "{body}"
        );
    }
}

/// A terminal running `script` under `/bin/sh`, and the Host's manager.
async fn sh_terminal(
    h: &Host,
    script: &str,
) -> (crate::terminal::manager::TerminalManager, String) {
    let tid = h
        .ok(
            "terminal.spawn",
            json!({"workspace_id": h.wt_workspace_id, "shell": "/bin/sh", "initial_command": script}),
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
    let at = seen
        .find("/zw-run")
        .unwrap_or_else(|| panic!("not pasted: {seen:?}"));
    assert!(seen.find("ready").is_some_and(|r| r < at), "{seen:?}");
    assert!(
        seen.contains("\x1b[200~/zw-run\x1b[201~"),
        "pasted, bracketed: {seen:?}"
    );
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
            assert!(
                event["seq"].is_u64() && event["type"].is_string() && event["payload"].is_object()
            );
            if pred(event) {
                return frame;
            }
        }
        panic!("no {what}");
    }
}

fn event_is<'a>(
    kind: &'a str,
    check: impl Fn(&Value) -> bool + 'a,
) -> impl Fn(&Value) -> bool + 'a {
    move |e| e["type"] == kind && check(&e["payload"])
}

#[tokio::test]
async fn host_feed_snapshot_equals_the_list_operations() {
    let h = host(true).await;
    let wt = h
        .ok(
            "workspace.create_worktree",
            json!({"workspace_id": h.workspace_id}),
        )
        .await;
    seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let tid = h
        .ok(
            "terminal.spawn",
            json!({"workspace_id": wt["workspace_id"], "shell": "/bin/sh"}),
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
    assert_eq!(
        snapshot["terminals"],
        h.ok("terminal.list", json!({})).await
    );
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
        .ok(
            "workspace.create_worktree",
            json!({"workspace_id": h.workspace_id}),
        )
        .await;
    let wid = wt["workspace_id"].clone();
    let created = event_is("workspace.changed", |p| {
        p["workspace"]["workspace_id"] == wid
    });
    let fa = h.host_event(&mut a, "worktree on A", &created).await;
    let fb = h.host_event(&mut b, "worktree on B", &created).await;
    assert_eq!(fa, fb, "both sockets receive the same event");
    assert_eq!(fa["epoch"], snap_a["epoch"]);
    assert_eq!(fa["event"]["payload"]["workspace"], wt);

    // A terminal, spawned and killed.
    let tid = h
        .ok(
            "terminal.spawn",
            json!({"workspace_id": wid, "shell": "/bin/sh"}),
        )
        .await["terminal_id"]
        .clone();
    let spawned = event_is("terminal.changed", |p| p["terminal"]["terminal_id"] == tid);
    h.host_event(&mut a, "terminal on A", &spawned).await;
    h.host_event(&mut b, "terminal on B", &spawned).await;
    h.ok("terminal.kill", json!({"terminal_id": tid})).await;
    let removed = event_is("terminal.removed", |p| p["terminal_id"] == tid);
    h.host_event(&mut a, "terminal removal on A", &removed)
        .await;
    h.host_event(&mut b, "terminal removal on B", &removed)
        .await;

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
    let diff = event_is("workspace.diff_changed", |p| {
        p["workspace_id"] == wid.as_str()
    });
    let fa = h.host_event(&mut a, "diff on A", &diff).await;
    assert_eq!(fa["event"]["payload"], json!({"workspace_id": wid}));
    h.host_event(&mut b, "diff on B", &diff).await;

    git(
        std::path::Path::new(&h.root),
        &["checkout", "-q", "-b", "feature"],
    );
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
    assert_eq!(
        fa["event"]["payload"]["session"]["workspace_id"],
        h.workspace_id.as_str()
    );
}

#[tokio::test]
async fn host_feed_snapshot_carries_the_layout() {
    let h = host(true).await;
    let group = h
        .ok("workspace.group_create", json!({"name": "backend"}))
        .await;

    let (_ws, snapshot) = h.feed().await;

    let entries = snapshot["layout"]["entries"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e["group_id"] == group["group_id"] && e["name"] == "backend"),
        "{snapshot}"
    );
    assert!(
        entries
            .iter()
            .any(|e| e["workspace_id"] == json!(h.workspace_id)),
        "{snapshot}"
    );
}

#[tokio::test]
async fn host_feed_snapshot_carries_keep_awake() {
    let h = host(true).await;
    let (mut ws, snapshot) = h.feed().await;
    assert_eq!(snapshot.get("keep_awake"), Some(&Value::Null), "{snapshot}");

    let status = crate::power::status(&h.state).await.unwrap();
    h.state.keep_awake.send_replace(Some(status));
    let changed = event_is("host.keep_awake_changed", |p| p["mode"] == "always");
    h.host_event(&mut ws, "keep-awake change", &changed).await;
}

#[tokio::test]
async fn host_feed_invalid_subscribe_and_resubscribe() {
    let h = host(true).await;
    let mut ws = h.ws().await;
    ws.send_json(&json!({"op": "host.subscribe", "since_seq": 3}))
        .await;
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
async fn settings_round_trip_and_refuse_what_they_do_not_know() {
    let h = host(false).await;
    let empty = h.ok("settings.get", json!({})).await;
    assert_eq!(empty["values"]["defaults.agent"], Value::Null);
    assert_eq!(empty["values"]["defaults.mode"], Value::Null);
    let set = h
        .ok(
            "settings.update",
            json!({"values": {"defaults.agent": "codex", "defaults.claude_code.model": "haiku"}}),
        )
        .await;
    assert_eq!(set["values"]["defaults.agent"], "codex");
    assert_eq!(set["values"]["defaults.claude_code.model"], "haiku");
    let cleared = h
        .ok("settings.update", json!({"values": {"defaults.agent": null}}))
        .await;
    assert_eq!(cleared["values"]["defaults.agent"], Value::Null);
    assert_eq!(cleared["values"]["defaults.claude_code.model"], "haiku");
    for bad in [
        json!({"values": {"defaults.nope": "x"}}),
        json!({"values": {"defaults.agent": "nobody"}}),
        json!({"values": {"defaults.mode": "bypassPermissions"}}),
        json!({"values": {"defaults.codex.effort": 3}}),
    ] {
        assert_eq!(h.err("settings.update", bad.clone()).await.1, "invalid_input", "{bad}");
    }
    // Nothing is written when one value is refused.
    h.err(
        "settings.update",
        json!({"values": {"defaults.mode": "plan", "defaults.nope": "x"}}),
    )
    .await;
    assert_eq!(h.ok("settings.get", json!({})).await["values"]["defaults.mode"], Value::Null);
    assert_eq!(h.err("settings.update", json!({"values": {}, "x": 1})).await.1, "invalid_request");
}

#[tokio::test]
async fn advanced_settings_take_effect_on_the_host() {
    let h = host(false).await;
    let empty = h.ok("settings.get", json!({})).await;
    assert_eq!(empty["values"]["terminal.scrollback"], Value::Null);
    assert_eq!(empty["values"]["git.push_sets_upstream"], Value::Null);
    assert!(crate::settings::push_sets_upstream(&h.state.db.conn).await, "on until turned off");
    let set = h
        .ok(
            "settings.update",
            json!({"values": {"terminal.scrollback": 4 * 1024 * 1024, "git.push_sets_upstream": false}}),
        )
        .await;
    assert_eq!(set["values"]["terminal.scrollback"], 4 * 1024 * 1024);
    assert!(!crate::settings::push_sets_upstream(&h.state.db.conn).await);
    for bad in [
        json!({"values": {"terminal.scrollback": 0}}),
        json!({"values": {"terminal.scrollback": "4MB"}}),
        json!({"values": {"git.push_sets_upstream": "no"}}),
    ] {
        assert_eq!(h.err("settings.update", bad).await.1, "invalid_input");
    }
    h.ok("settings.update", json!({"values": {"terminal.scrollback": null}})).await;
}

#[tokio::test]
async fn folders_are_where_the_host_keeps_things() {
    let h = host(false).await;
    // The one op whose answer is host paths, so it skips `call`'s leak check.
    let res = h
        .server
        .post("/zwiim/v1/host.folders")
        .authorization_bearer(HOST_TOKEN)
        .json(&json!({}))
        .await;
    assert_eq!(res.status_code(), StatusCode::OK, "{}", res.text());
    let f: Value = res.json();
    assert_eq!(f["data"], h.state.data_dir.to_string_lossy().as_ref());
    assert_eq!(f["worktrees"], h.state.worktrees_dir.to_string_lossy().as_ref());
    assert_eq!(f["repositories"], h.state.repos_dir.to_string_lossy().as_ref());
}

#[tokio::test]
async fn prompt_settings_keep_the_users_words() {
    let h = host(false).await;
    let pull_request = "Draft PRs only.\n\n1. Title them `area: change`.";
    let set = h
        .ok(
            "settings.update",
            json!({"values": {
                "prompts.pull_request": pull_request,
                "prompts.branch_name": "Start with the ticket number.",
            }}),
        )
        .await;
    assert_eq!(set["values"]["prompts.pull_request"], pull_request);
    assert!(set["values"].get("prompts.general").is_none());
    let prefs = crate::settings::user_preferences(&h.state.db.conn).await;
    assert_eq!(prefs.pull_request.as_deref(), Some(pull_request));
    for bad in [
        json!({"values": {"prompts.pull_request": "  "}}),
        json!({"values": {"prompts.general": "x"}}),
        json!({"values": {"prompts.resolve_conflicts": "x"}}),
        json!({"values": {"prompts.branch_name": "x".repeat(crate::settings::MAX_PROMPT + 1)}}),
        json!({"values": {"prompts.code_review": "x"}}),
    ] {
        assert_eq!(h.err("settings.update", bad).await.1, "invalid_input");
    }
    let cleared = h
        .ok("settings.update", json!({"values": {"prompts.pull_request": null}}))
        .await;
    assert_eq!(cleared["values"]["prompts.pull_request"], Value::Null);
    assert_eq!(cleared["values"]["prompts.branch_name"], "Start with the ticket number.");
}

#[tokio::test]
async fn git_settings_name_new_branches_and_delete_them_on_remove() {
    let h = host(false).await;
    let wid = h.workspace_id.clone();
    for bad in [
        json!({"values": {"git.branch_prefix": "none"}}),
        json!({"values": {"git.branch_prefix_custom": "a b"}}),
        json!({"values": {"git.delete_branch_on_remove": "yes"}}),
    ] {
        assert_eq!(
            h.err("settings.update", bad.clone()).await.1,
            "invalid_input",
            "{bad}"
        );
    }
    let set = h
        .ok(
            "settings.update",
            json!({"values": {
                "git.branch_prefix": "custom",
                "git.branch_prefix_custom": "team/ashish/",
                "git.delete_branch_on_remove": true,
            }}),
        )
        .await;
    assert_eq!(set["values"]["git.branch_prefix"], "custom");
    assert_eq!(set["values"]["git.delete_branch_on_remove"], true);

    let root = std::path::Path::new(&h.root);
    let origin = root.parent().unwrap().join("origin.git");
    git(root, &["init", "-q", "--bare", origin.to_str().unwrap()]);
    git(root, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git(root, &["push", "-q", "origin", "main"]);
    git(root, &["remote", "set-head", "origin", "main"]);

    let wt = h
        .ok("workspace.create_worktree", json!({"workspace_id": wid}))
        .await;
    let branch = wt["branch"].as_str().unwrap().to_string();
    let city = branch.strip_prefix("team/ashish/").expect(&branch);
    let row = crate::workspace::resolve(&h.state.db.conn, wt["workspace_id"].as_str().unwrap())
        .await
        .unwrap();
    assert!(row.path.ends_with(&format!("/{city}")), "{}", row.path);
    git(
        std::path::Path::new(&row.path),
        &["push", "-q", "origin", &branch],
    );

    let removed = h
        .ok(
            "workspace.remove_worktree",
            json!({"workspace_id": wt["workspace_id"]}),
        )
        .await;
    assert_eq!(removed["branch"], branch.as_str());
    assert_eq!(removed["branch_deleted"], true);
    assert_eq!(removed["branch_kept_reason"], Value::Null);
    assert_eq!(git_out(root, &["branch", "--list", &branch]), "");
}

#[cfg(unix)]
#[tokio::test]
async fn repository_scripts_run_around_a_worktrees_life() {
    let h = host(false).await;
    let wid = h.workspace_id.clone();
    let none = h
        .ok("repository.settings.get", json!({"workspace_id": wid}))
        .await;
    assert_eq!(
        none,
        json!({"values": {"scripts.setup": null, "scripts.run": null, "scripts.archive": null, "files.copy": null, "env": null, "preview.urls": null}})
    );
    for bad in [
        json!({"workspace_id": wid, "values": {"scripts.deploy": "x"}}),
        json!({"workspace_id": wid, "values": {"scripts.setup": "  "}}),
        json!({"workspace_id": wid, "values": {"files.copy": ["#x"]}}),
        json!({"workspace_id": wid, "values": {"env": {"ZWIIM_ROOT_PATH": "x"}}}),
        json!({"workspace_id": wid, "values": {"env": {"A": 1}}}),
        json!({"workspace_id": wid, "values": {"scripts.setup": "x".repeat(crate::repository_settings::MAX_SCRIPT + 1)}}),
    ] {
        assert_eq!(h.err("repository.settings.update", bad).await.1, "invalid_input");
    }
    assert_eq!(
        h.err("repository.settings.update", json!({"workspace_id": wid, "values": {}, "x": 1}))
            .await
            .1,
        "invalid_request"
    );
    assert_eq!(
        h.err("repository.settings.get", json!({"workspace_id": "nope"})).await.1,
        "not_found"
    );

    // Only the files the patterns pick are copied into a new worktree.
    let root = std::path::Path::new(&h.root);
    std::fs::write(root.join(".git/info/exclude"), "*.local\n.env\n").unwrap();
    std::fs::write(root.join("keys.local"), "k").unwrap();
    std::fs::write(root.join(".env"), "e").unwrap();
    let set = h
        .ok(
            "repository.settings.update",
            json!({"workspace_id": wid, "values": {
                "scripts.setup": "echo \"$API_URL\" > setup-ran",
                "scripts.archive": "echo archiving",
                "files.copy": ["*.local"],
                "env": {"API_URL": "http://localhost:4000"},
            }}),
        )
        .await;
    assert_eq!(set["values"]["scripts.setup"], "echo \"$API_URL\" > setup-ran");
    assert_eq!(set["values"]["env"], json!({"API_URL": "http://localhost:4000"}));

    // The setup script starts in a terminal of the new worktree, without
    // holding up the reply.
    let wt = h
        .ok("workspace.create_worktree", json!({"workspace_id": wid}))
        .await;
    let wt_id = wt["workspace_id"].as_str().unwrap().to_string();
    let wt_path = crate::workspace::resolve(&h.state.db.conn, &wt_id).await.unwrap().path;
    let wt_path = std::path::Path::new(&wt_path);
    assert!(wt_path.join("keys.local").exists() && !wt_path.join(".env").exists());
    // The setup script sees the repository's variables.
    let mut ran = String::new();
    for _ in 0..100 {
        ran = std::fs::read_to_string(wt_path.join("setup-ran")).unwrap_or_default();
        if !ran.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(ran, "http://localhost:4000\n");
    let list = h.ok("terminal.list", json!({})).await;
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["title"] == "Setup" && t["workspace_id"] == wt_id.as_str()), "{list}");
    // A worktree reads and changes its repository's.
    let through_worktree = h
        .ok("repository.settings.get", json!({"workspace_id": wt_id}))
        .await;
    assert_eq!(through_worktree, set);

    // The run script runs on asking, in the worktree's one `Run` terminal,
    // with the worktree's own port; its terminals have it too.
    let wait_for = |name: &'static str| async move {
        for _ in 0..100 {
            let read = std::fs::read_to_string(wt_path.join(name)).unwrap_or_default();
            if !read.is_empty() {
                return read;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        String::new()
    };
    let run = |script: &str| json!({"workspace_id": wt_id, "script": script});
    let (_, code, details) = h.err_details("workspace.run_script", run("run")).await;
    assert_eq!((code.as_str(), details), ("script_not_set", Some(json!({"script": "run"}))));
    assert_eq!(
        h.err("workspace.run_script", json!({"workspace_id": wid, "script": "setup"})).await.1,
        "root_workspace"
    );
    assert_eq!(h.err("workspace.run_script", run("deploy")).await.1, "invalid_request");
    h.ok(
        "repository.settings.update",
        json!({"workspace_id": wid, "values": {"scripts.run": "echo \"$ZWIIM_PORT\" > run-port; sleep 30"}}),
    )
    .await;
    let first = h.ok("workspace.run_script", run("run")).await;
    let port = wait_for("run-port").await;
    assert!(port.trim().parse::<u16>().unwrap() >= 40_000, "{port}");
    let again = h.ok("workspace.run_script", run("run")).await;
    assert_ne!(first["terminal_id"], again["terminal_id"]);
    h.ok(
        "terminal.spawn",
        json!({"workspace_id": wt_id, "initial_command": "echo \"$ZWIIM_PORT\" > term-port"}),
    )
    .await;
    assert_eq!(wait_for("term-port").await, port);
    // Setup again replaces the first `Setup` terminal too.
    std::fs::remove_file(wt_path.join("setup-ran")).unwrap();
    let setup = h.ok("workspace.run_script", run("setup")).await;
    assert_eq!(wait_for("setup-ran").await, "http://localhost:4000\n");
    let list = h.ok("terminal.list", json!({})).await;
    let titled = |title: &str| {
        list.as_array()
            .unwrap()
            .iter()
            .filter(|t| t["title"] == title && t["workspace_id"] == wt_id.as_str())
            .map(|t| t["terminal_id"].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(titled("Run"), vec![again["terminal_id"].clone()], "{list}");
    assert_eq!(titled("Setup"), vec![setup["terminal_id"].clone()], "{list}");

    // Its previews: the repository's URLs, with the worktree's ports.
    h.ok(
        "repository.settings.update",
        json!({"workspace_id": wid, "values": {"preview.urls": [{"name": "App", "url": "http://localhost:$((ZWIIM_PORT + 1))"}]}}),
    )
    .await;
    let app = port.trim().parse::<u16>().unwrap() + 1;
    assert_eq!(
        h.ok("workspace.previews", json!({"workspace_id": wt_id})).await,
        json!({"previews": [{"name": "App", "url": format!("http://localhost:{app}")}]})
    );
    assert_eq!(
        h.err("workspace.previews", json!({"workspace_id": wid})).await.1,
        "root_workspace"
    );

    let removed = h
        .ok("workspace.remove_worktree", json!({"workspace_id": wt_id, "force": true}))
        .await;
    assert_eq!(
        removed["archive_script"],
        json!({"exit_code": 0, "timed_out": false, "output": "archiving"})
    );
}

#[tokio::test]
async fn agent_list_shows_the_host_defaults_it_offers() {
    let h = host(false).await;
    crate::db::service::app_metadata_service::upsert_value(
        &h.state.db.conn,
        "zwiim.agent_options:claude_code",
        &serde_json::to_string(&claude_options("default", "opus[1m]")).unwrap(),
    )
    .await
    .unwrap();
    let opus_effort = claude_options("default", "opus[1m]")
        .into_iter()
        .find(|o| o.id == "effort");
    crate::db::service::app_metadata_service::upsert_value(
        &h.state.db.conn,
        "zwiim.agent_model_effort:claude_code",
        &serde_json::to_string(&json!({"opus[1m]": opus_effort, "haiku": null})).unwrap(),
    )
    .await
    .unwrap();
    let claude = |list: Value| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|a| a["agent"] == "claude_code")
            .unwrap()
            .clone()
    };
    let before = claude(h.ok("agent.list", json!({})).await);
    assert_eq!(before["default"], false);
    assert_eq!(before["defaults"], json!({"model": null, "effort": null, "mode": null}));

    h.ok(
        "settings.update",
        json!({"values": {
            "defaults.agent": "claude_code",
            "defaults.mode": "plan",
            "defaults.claude_code.effort": "low",
        }}),
    )
    .await;
    let listed = h.ok("agent.list", json!({})).await;
    let codex = listed.as_array().unwrap().iter().find(|a| a["agent"] == "codex").unwrap();
    assert_eq!(codex["default"], false);
    assert_eq!(codex["defaults"]["mode"], Value::Null, "no catalog, nothing offered");
    let now = claude(listed);
    assert_eq!(now["default"], true);
    assert_eq!(now["defaults"], json!({"model": null, "effort": "low", "mode": "plan"}));
    assert_eq!(now["options"]["mode"]["current"], "plan");
    assert_eq!(now["options"]["effort"]["current"], "low");
    assert_eq!(now["effort_by_model"]["opus[1m]"]["current"], "low");

    // Haiku offers no effort: the default effort does not apply on it, and a
    // model the agent does not offer is ignored.
    h.ok("settings.update", json!({"values": {"defaults.claude_code.model": "haiku"}}))
        .await;
    let on_haiku = claude(h.ok("agent.list", json!({})).await);
    assert_eq!(on_haiku["defaults"], json!({"model": "haiku", "effort": null, "mode": "plan"}));
    assert_eq!(on_haiku["options"]["model"]["current"], "haiku");
    assert_eq!(on_haiku["options"]["effort"], Value::Null);
    h.ok("settings.update", json!({"values": {"defaults.claude_code.model": "sonnet"}}))
        .await;
    let dropped = claude(h.ok("agent.list", json!({})).await);
    assert_eq!(dropped["defaults"]["model"], Value::Null);
    assert_eq!(dropped["options"]["model"]["current"], "opus[1m]");
}

#[tokio::test]
async fn host_claim_and_owner() {
    let h = host(false).await;
    assert_eq!(h.ok("host.hello", json!({})).await["owner"], Value::Null);
    let alice = json!({"user_id": "u-alice", "email": "alice@example.com"});
    assert_eq!(h.ok("host.claim", alice.clone()).await["owner"], alice);
    let (status, code, details) = h
        .err_details(
            "host.claim",
            json!({"user_id": "u-bob", "email": "bob@example.com"}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::CONFLICT, "host_owned")
    );
    assert_eq!(details.unwrap()["owner_email"], "alice@example.com");
    assert_eq!(h.ok("host.hello", json!({})).await["owner"], alice);
    assert_eq!(
        h.err("host.claim", json!({"user_id": " "})).await.1,
        "invalid_input"
    );
    assert_eq!(
        h.err("host.claim", json!({"user_id": "u", "access_token": "x"}))
            .await
            .1,
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
        assert!(
            a["commands"].is_null(),
            "no commands before any session: {a}"
        );
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
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
    let (status, code) = h
        .err(
            "agent.setup",
            json!({"agent": "claude_code", "version": "9.9.9"}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
}

#[tokio::test]
async fn agent_sign_in_status_reaches_every_client_and_never_a_key() {
    let h = host(false).await;
    let codex_home = tempfile::tempdir().unwrap();
    // Saving clears a plaintext key Codex wrote to its own home: a temp one.
    temp_env::async_with_vars([("CODEX_HOME", Some(codex_home.path()))], async {
        let key = "sk-test-protocol-0001";
        let saved = h
            .ok("agent.save_key", json!({"agent": "codex", "key": key}))
            .await;
        assert_eq!(saved["agent"], "codex");
        assert_eq!(saved["saved_key"], true, "{saved}");
        assert!(!saved.to_string().contains(key), "{saved}");

        let all = h.ok("agent.auth_status", json!({})).await;
        let agents: Vec<_> = all
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["agent"].clone())
            .collect();
        assert_eq!(agents, [json!("claude_code"), json!("codex")]);
        assert!(!all.to_string().contains(key), "{all}");
        let mut fields: Vec<_> = all[1].as_object().unwrap().keys().cloned().collect();
        fields.sort();
        assert_eq!(
            fields,
            [
                "agent",
                "connected",
                "email",
                "method",
                "plan",
                "saved_key",
                "sign_in"
            ]
        );

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
        .call(
            "agent.save_key",
            json!({"agent": "claude_code", "key": "sk-proj-not-anthropic"}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["code"], "invalid_input");
    assert!(!value.to_string().contains("not-anthropic"), "{value}");
    // A key of the wrong type is a schema error that does not quote it either.
    let (status, value) = h
        .call(
            "agent.save_key",
            json!({"agent": "codex", "key": ["sk-test-in-array"]}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!value.to_string().contains("sk-test-in-array"), "{value}");
    let (status, code) = h.err("agent.sign_in", json!({"agent": "gemini"})).await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
}

fn own_channel() -> &'static str {
    if env!("CARGO_PKG_VERSION").contains('-') {
        "beta"
    } else {
        "stable"
    }
}

#[tokio::test]
async fn an_unsigned_unsupervised_host_reports_that_it_cannot_update() {
    let h = host(false).await;
    let status = h.ok("host.update.status", json!({})).await;
    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    // Until one is chosen, a Host follows its own version's channel.
    assert_eq!(status["channel"], own_channel());
    assert_eq!(status["supported"], false, "{status}");
    // Test builds carry no release key.
    assert_eq!(status["unsupported_reason"], "unsigned_build", "{status}");
    assert_eq!(status["restart_required"], false);
    assert_eq!(status["restart_pending"], false);
    assert_eq!(status["activity"]["state"], "idle");
    // Checking never reaches the network without a release key.
    let checked = h.ok("host.update.check", json!({})).await;
    assert_eq!(checked["supported"], false);
    assert!(checked["last_checked"].is_null(), "{checked}");
    let (status, code) = h.err("host.update.apply", json!({})).await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::CONFLICT, "update_unavailable")
    );
    let (status, code) = h.err("host.update.apply", json!({"mode": "later"})).await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
    for body in [
        json!({"channel": "nightly"}),
        json!({}),
        json!({"channel": "beta", "extra": 1}),
    ] {
        let (status, code) = h.err("host.update.set_channel", body.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (StatusCode::BAD_REQUEST, "invalid_request"),
            "{body}"
        );
    }
    let hello = h.ok("host.hello", json!({})).await;
    let capabilities = hello["capabilities"].as_array().unwrap();
    assert!(capabilities.contains(&json!("host.update")));
    assert!(capabilities.contains(&json!("host.update.channel")));
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
    assert_eq!(
        options["mode"]["available"],
        json!(["normal", "plan", "unattended"])
    );
    assert!(!read.to_string().contains("bypassPermissions"));

    // Unattended is the agent's native full-access mode, applied between turns.
    let apply = async {
        match rx.recv().await {
            Some(ConnectionCommand::SetConfigOption {
                config_id,
                value_id,
            }) => {
                assert_eq!(
                    (config_id.as_str(), value_id.as_str()),
                    ("mode", "bypassPermissions")
                );
                st.write().await.config_options =
                    Some(claude_options("bypassPermissions", "opus[1m]"));
            }
            _ => panic!("expected SetConfigOption"),
        }
    };
    let (set, ()) = tokio::join!(
        h.ok(
            "session.set_options",
            json!({"session_id": sid, "mode": "unattended"})
        ),
        apply
    );
    assert_eq!(set["options"]["mode"]["current"], "unattended");
    assert_eq!(set["options"]["mode"]["label"], "Bypass permissions");

    let (status, code, details) = h
        .err_details(
            "session.set_options",
            json!({"session_id": sid, "model": "bogus"}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, "option_unavailable")
    );
    assert_eq!(
        details.unwrap(),
        json!({"option": "model", "value": "bogus"})
    );
    assert_eq!(
        h.err("session.set_options", json!({"session_id": sid}))
            .await
            .1,
        "invalid_input"
    );
    assert_eq!(
        h.err(
            "session.set_options",
            json!({"session_id": sid, "mode": "yolo"})
        )
        .await
        .1,
        "invalid_request"
    );

    // Between turns only.
    st.write().await.status = crate::acp::types::ConnectionStatus::Prompting;
    assert_eq!(
        h.err(
            "session.set_options",
            json!({"session_id": sid, "effort": "low"})
        )
        .await,
        (StatusCode::CONFLICT, "turn_in_progress".into())
    );

    // Idle: nothing known about the agent yet.
    let idle_conv = seed_conversation(&h.state.db, h.folder_id, AgentType::ClaudeCode).await;
    let idle = session_id_of(&h.state, idle_conv).await;
    assert_eq!(
        h.ok("session.options", json!({"session_id": idle})).await["options"],
        Value::Null
    );
    assert_eq!(
        h.err_details(
            "session.set_options",
            json!({"session_id": idle, "mode": "plan"})
        )
        .await
        .1,
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
        .ok(
            "session.set_options",
            json!({"session_id": idle, "mode": "plan", "model": "haiku"}),
        )
        .await;
    assert_eq!(set["options"]["mode"]["current"], "plan");
    assert_eq!(set["options"]["model"]["current"], "haiku");
    let preferred = crate::session_options::preferred_values(&h.state.db.conn, &idle).await;
    assert_eq!(preferred.get("mode").map(String::as_str), Some("plan"));
    assert_eq!(preferred.get("model").map(String::as_str), Some("haiku"));
    let back = h
        .ok(
            "session.set_options",
            json!({"session_id": idle, "mode": "normal"}),
        )
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
    let opus_effort = claude_options("default", "opus[1m]")
        .into_iter()
        .find(|o| o.id == "effort");
    crate::db::service::app_metadata_service::upsert_value(
        &h.state.db.conn,
        "zwiim.agent_model_effort:claude_code",
        &serde_json::to_string(&json!({"opus[1m]": opus_effort, "haiku": null})).unwrap(),
    )
    .await
    .unwrap();
    let listed = h.ok("agent.list", json!({})).await;
    let claude = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agent"] == "claude_code")
        .unwrap()
        .clone();
    assert_eq!(claude["effort_by_model"]["haiku"], Value::Null);
    assert_eq!(
        claude["effort_by_model"]["opus[1m]"]["values"][1]["value"],
        "high"
    );
    assert!(
        claude["options"]["effort"].is_object(),
        "the agent-wide effort stays"
    );
    let start = |model: &str, effort: Option<&str>| {
        let mut options = json!({"model": model});
        if let Some(e) = effort {
            options["effort"] = json!(e);
        }
        json!({"workspace_id": h.workspace_id, "agent": "claude_code", "blocks": [{"type": "text", "text": "hi"}], "options": options})
    };
    let (status, code, details) = h
        .err_details("session.start", start("haiku", Some("low")))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, "option_unavailable")
    );
    assert_eq!(details, Some(json!({"option": "effort", "value": "low"})));

    // An idle session moved to Haiku drops Opus's effort instead of keeping
    // it for a model that has none, and gets Opus's own back on Opus.
    let set = h
        .ok(
            "session.set_options",
            json!({"session_id": idle, "model": "opus[1m]", "effort": "low"}),
        )
        .await;
    assert_eq!(set["options"]["effort"]["current"], "low");
    let on_haiku = h
        .ok(
            "session.set_options",
            json!({"session_id": idle, "model": "haiku"}),
        )
        .await;
    assert_eq!(on_haiku["options"]["model"]["current"], "haiku");
    assert_eq!(on_haiku["options"]["effort"], Value::Null);
    let preferred = crate::session_options::preferred_values(&h.state.db.conn, &idle).await;
    assert!(!preferred.contains_key("effort"), "{preferred:?}");
    assert_eq!(
        h.err_details(
            "session.set_options",
            json!({"session_id": idle, "effort": "low"})
        )
        .await
        .1,
        "option_unavailable"
    );
    let on_opus = h
        .ok(
            "session.set_options",
            json!({"session_id": idle, "model": "opus[1m]"}),
        )
        .await;
    assert_eq!(on_opus["options"]["effort"]["values"][0]["value"], "low");
    assert_eq!(
        h.ok("session.options", json!({"session_id": idle})).await["options"]["model"]["current"],
        "opus[1m]"
    );
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
    assert_eq!(
        fields,
        ["code", "expires_at", "host_id", "host_static_pubkey", "v"]
    );
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
    assert_eq!(
        res.json::<Value>(),
        json!({"host_id": host_id, "device_id": "dev-1"})
    );

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
    assert!(
        d.get("id").is_none() && d.get("credential_hash").is_none(),
        "{d}"
    );

    assert_eq!(
        h.ok("device.revoke", json!({"device_id": "dev-1"})).await,
        json!({})
    );
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
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("already_exists"))
    );
    // A revoked device is trusted again only by pairing again.
    h.ok("device.revoke", json!({"device_id": "dev-1"})).await;
    assert_eq!(pair_by_key(&h, "dev-1", 1).await.0, StatusCode::OK);
    assert_eq!(
        h.ok("device.list", json!({})).await["devices"][0]["status"],
        "trusted"
    );
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
        ("host.update.set_channel", json!({"channel": "beta"})),
        ("agent.setup", json!({"agent": "claude_code"})),
        ("agent.sign_in", json!({"agent": "codex"})),
        (
            "agent.save_key",
            json!({"agent": "codex", "key": "sk-test-device-0001"}),
        ),
        ("agent.remove_key", json!({"agent": "codex"})),
        ("agent.sign_out", json!({"agent": "codex"})),
        ("browser.tabs.list", json!({})),
        ("browser.tab.open", json!({"owner": {"kind": "personal"}})),
        ("browser.tab.close", json!({"tab_id": "t"})),
        ("browser.tab.navigate", json!({"tab_id": "t", "action": "reload"})),
        ("browser.engine.reset", json!({})),
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
    assert_eq!(
        h.ok("device.list", json!({})).await["devices"][0]["status"],
        "trusted"
    );
    assert_eq!(h.ok("host.hello", json!({})).await["owner"], Value::Null);

    // The legacy `/api` device management is host-only too.
    for route in [
        "pairing_invitation_create",
        "trusted_device_list",
        "trusted_device_revoke",
    ] {
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
    assert_eq!(
        h.call_as("unknown-credential", "workspace.list", json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    h.ok("device.revoke", json!({"device_id": "dev-1"})).await;
    let (status, body) = h.call_as(&credential, "workspace.list", json!({})).await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("unauthenticated"))
    );
}

async fn expect_revoked_close(ws: &mut TestWebSocket) {
    use axum_test::WsMessage;
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.receive_message())
            .await
            .expect("close within 5s");
        match msg {
            WsMessage::Close(Some(frame)) => {
                assert_eq!(
                    u16::from(frame.code),
                    crate::web::auth::WS_CLOSE_DEVICE_REVOKED
                );
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
            format!(
                "zwiim-events, zwiim-token.{}",
                URL_SAFE_NO_PAD.encode(&credential)
            ),
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
    assert_eq!(
        hello["min_client_revision"],
        super::compat::MIN_CLIENT_REVISION
    );
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
    assert_eq!(
        res.json::<Value>()["revision"],
        super::compat::PROTOCOL_REVISION
    );
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
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
    assert_eq!(
        details,
        Some(json!({"reason": "unknown_field", "field": "from_a_newer_client"}))
    );
}

#[tokio::test]
async fn host_instructions_round_trip() {
    let h = host(false).await;
    assert_eq!(
        h.ok("host.instructions", json!({})).await,
        json!({"host": {"general": null, "resolve_conflicts": null}, "workspaces": {}})
    );
    h.ok(
        "host.instructions.set",
        json!({"scope": "host", "general": "Be brief."}),
    )
    .await;
    let saved = h
        .ok(
            "host.instructions.set",
            json!({"scope": "workspace", "workspace_id": h.workspace_id, "resolve_conflicts": "Keep ours."}),
        )
        .await;
    assert_eq!(saved["host"]["general"], "Be brief.");
    assert_eq!(
        saved["workspaces"][&h.workspace_id]["resolve_conflicts"],
        "Keep ours."
    );
    assert_eq!(h.ok("host.instructions", json!({})).await, saved);
    // A worktree has no texts of its own; it uses its root's.
    let (status, code) = h
        .err(
            "host.instructions.set",
            json!({"scope": "workspace", "workspace_id": h.wt_workspace_id, "general": "x"}),
        )
        .await;
    assert_eq!((status, code.as_str()), (StatusCode::NOT_FOUND, "not_found"));
    let (status, code) = h
        .err(
            "host.instructions.set",
            json!({"scope": "host", "general": "x".repeat(8 * 1024 + 1)}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, "too_large")
    );
}

#[tokio::test]
async fn diagnostics_report_versions_and_state_without_secrets() {
    let h = host(false).await;
    let d = h.ok("host.diagnostics", json!({})).await;
    assert_eq!(d["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(d["channel"], own_channel());
    assert_eq!(
        d["protocol"]["revision"],
        crate::protocol::compat::PROTOCOL_REVISION
    );
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
    assert!(
        !text.contains(&h.state.data_dir.to_string_lossy().to_string()),
        "paths leak: {text}"
    );
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[tokio::test]
async fn attachments_upload_into_the_workspace_and_download_back() {
    let h = host(false).await;
    let wid = h.wt_workspace_id.clone();

    let up = h
        .ok(
            "attachments.upload",
            json!({"workspace_id": wid, "name": "../Q3 report.pdf", "data": b64(b"%PDF-1.7")}),
        )
        .await;
    let id = up["attachment_id"].as_str().unwrap().to_string();
    assert!(crate::attachments::valid_id(&id));
    assert_eq!(
        up,
        json!({"attachment_id": id, "name": "Q3 report.pdf", "size": 8, "mime_type": "application/pdf"})
    );
    let rel = format!(".zwiim/attachments/{id}/Q3 report.pdf");
    assert_eq!(
        std::fs::read(std::path::Path::new(&h.wt).join(&rel)).unwrap(),
        b"%PDF-1.7"
    );

    let down = h
        .ok("files.download", json!({"workspace_id": wid, "path": rel}))
        .await;
    assert_eq!(
        down,
        json!({"name": "Q3 report.pdf", "mime_type": "application/pdf", "size": 8, "data": b64(b"%PDF-1.7")})
    );

    assert_eq!(
        h.err("attachments.upload", json!({"workspace_id": wid, "name": "a.txt", "data": "%%%"}))
            .await
            .1,
        "invalid_request"
    );
    assert_eq!(
        h.err("attachments.upload", json!({"workspace_id": wid, "name": "..", "data": b64(b"x")}))
            .await
            .1,
        "invalid_name"
    );
    assert_eq!(
        h.err("attachments.upload", json!({"workspace_id": "nope", "name": "a", "data": b64(b"x")}))
            .await
            .1,
        "not_found"
    );
}

#[tokio::test]
async fn attachments_take_ten_mebibytes_and_refuse_more() {
    let h = host(false).await;
    let wid = h.wt_workspace_id.clone();
    let max = crate::attachments::ATTACHMENT_MAX_BYTES;

    let up = h
        .ok(
            "attachments.upload",
            json!({"workspace_id": wid, "name": "big.bin", "data": b64(&vec![7u8; max])}),
        )
        .await;
    assert_eq!(up["size"], max);
    assert_eq!(
        h.err(
            "attachments.upload",
            json!({"workspace_id": wid, "name": "big.bin", "data": b64(&vec![7u8; max + 1])})
        )
        .await,
        (StatusCode::PAYLOAD_TOO_LARGE, "attachment_too_large".into())
    );

    // Downloads share the cap.
    std::fs::write(std::path::Path::new(&h.wt).join("huge.bin"), vec![0u8; max + 1]).unwrap();
    assert_eq!(
        h.err("files.download", json!({"workspace_id": wid, "path": "huge.bin"}))
            .await
            .1,
        "file_too_large"
    );
}

#[tokio::test]
async fn files_download_stays_inside_the_workspace() {
    let h = host(false).await;
    let wid = h.wt_workspace_id.clone();
    let outside = h._dir.path().join("secret.txt");
    std::fs::write(&outside, "secret").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, std::path::Path::new(&h.wt).join("link.txt")).unwrap();

    for path in ["../secret.txt", outside.to_str().unwrap(), "link.txt", "missing.txt", ""] {
        let (status, _) = h
            .err("files.download", json!({"workspace_id": wid, "path": path}))
            .await;
        assert!(status.is_client_error(), "{path}: {status}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn files_download_guard_keeps_a_link_away_from_hidden_files() {
    use std::os::unix::fs::symlink;
    let h = host(false).await;
    let wid = h.wt_workspace_id.clone();
    let wt = std::path::Path::new(&h.wt);
    std::fs::write(wt.join(".env"), "secret").unwrap();
    std::fs::create_dir(wt.join(".keys")).unwrap();
    std::fs::write(wt.join(".keys/a"), "secret").unwrap();
    std::fs::write(wt.join("plain.txt"), "plain").unwrap();
    symlink(".env", wt.join("env.txt")).unwrap();
    symlink(".keys", wt.join("keys")).unwrap();
    symlink("plain.txt", wt.join("same.txt")).unwrap();
    let guarded = |path: &str| json!({"workspace_id": wid, "path": path, "guard_hidden": true});

    for path in ["env.txt", "keys/a"] {
        let (status, _) = h.err("files.download", guarded(path)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    // The client decides what a path may name; the guard is about what a link
    // turns it into. So a hidden path asked for by name still reads.
    for path in ["plain.txt", "same.txt", ".env", ".keys/a"] {
        h.ok("files.download", guarded(path)).await;
    }
    // Every other caller keeps following links.
    h.ok("files.download", json!({"workspace_id": wid, "path": "env.txt"})).await;
}

#[tokio::test]
async fn files_download_reads_ranges_past_the_whole_file_cap() {
    let h = host(false).await;
    let wid = h.wt_workspace_id.clone();
    let max = crate::attachments::ATTACHMENT_MAX_BYTES;
    let range_max = crate::protocol::http::RANGE_MAX_BYTES as usize;
    let bytes: Vec<u8> = (0..max + 10).map(|i| (i % 251) as u8).collect();
    std::fs::write(std::path::Path::new(&h.wt).join("big.bin"), &bytes).unwrap();
    let size = bytes.len() as u64;
    let range = |offset: Option<u64>, length: Option<u64>| {
        let mut body = json!({"workspace_id": wid, "path": "big.bin"});
        if let Some(o) = offset { body["offset"] = json!(o); }
        if let Some(l) = length { body["length"] = json!(l); }
        body
    };

    // The whole file is still refused past 10 MiB.
    assert_eq!(h.err("files.download", range(None, None)).await.1, "file_too_large");

    // A small range, from the start.
    let r = h.ok("files.download", range(Some(0), Some(4))).await;
    assert_eq!(r, json!({"name": "big.bin", "mime_type": "application/octet-stream", "size": size, "offset": 0, "data": b64(&bytes[..4])}));

    // `length` alone starts at 0.
    let r = h.ok("files.download", range(None, Some(3))).await;
    assert_eq!((r["offset"].clone(), r["data"].clone()), (json!(0), json!(b64(&bytes[..3]))));

    // No length, or a huge one, is clamped to 8 MiB.
    for length in [None, Some(u64::MAX)] {
        let r = h.ok("files.download", range(Some(3), length)).await;
        assert_eq!(r["data"], json!(b64(&bytes[3..3 + range_max])), "{length:?}");
    }

    // The tail, the exact end, and past the end.
    let r = h.ok("files.download", range(Some(size - 2), Some(10))).await;
    assert_eq!(r["data"], json!(b64(&bytes[bytes.len() - 2..])));
    let r = h.ok("files.download", range(Some(size), Some(10))).await;
    assert_eq!((r["data"].clone(), r["size"].clone()), (json!(""), json!(size)));
    assert_eq!(
        h.err("files.download", range(Some(size + 1), None)).await,
        (StatusCode::BAD_REQUEST, "invalid_input".into())
    );
}

#[tokio::test]
async fn ranged_reads_keep_the_workspace_jail() {
    let h = host(false).await;
    let wid = h.wt_workspace_id.clone();
    let outside = h._dir.path().join("secret.txt");
    std::fs::write(&outside, "secret").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, std::path::Path::new(&h.wt).join("link.txt")).unwrap();
    for path in ["../secret.txt", outside.to_str().unwrap(), "link.txt", "missing.txt", "", "."] {
        let (status, _) = h
            .err("files.download", json!({"workspace_id": wid, "path": path, "offset": 0, "length": 4}))
            .await;
        assert!(status.is_client_error(), "{path}: {status}");
    }
}

#[tokio::test]
async fn the_host_offers_file_ranges() {
    let h = host(false).await;
    let hello = h.ok("host.hello", json!({})).await;
    assert!(hello["capabilities"].as_array().is_some_and(|c| c.iter().any(|v| v == "file_ranges")));
}

#[tokio::test]
async fn attachment_blocks_reach_the_agent_as_one_note() {
    let h = host(false).await;
    let wid = h.wt_workspace_id.clone();
    let (sid, mut rx) = h.live_session().await;

    assert_eq!(
        h.err(
            "session.send",
            json!({"session_id": sid, "blocks": [{"type": "attachment", "attachment_id": "k3F9a2Qx"}]})
        )
        .await,
        (StatusCode::NOT_FOUND, "attachment_not_found".into())
    );
    assert_eq!(
        h.err(
            "session.send",
            json!({"session_id": sid, "blocks": [{"type": "attachment", "attachment_id": "../../etc"}]})
        )
        .await
        .1,
        "attachment_not_found"
    );

    let mut ids = Vec::new();
    for name in ["a.pdf", "b.xlsx"] {
        let up = h
            .ok("attachments.upload", json!({"workspace_id": wid, "name": name, "data": b64(b"x")}))
            .await;
        ids.push(up["attachment_id"].as_str().unwrap().to_string());
    }
    h.ok(
        "session.send",
        json!({"session_id": sid, "blocks": [
            {"type": "attachment", "attachment_id": ids[0]},
            {"type": "text", "text": "compare these"},
            {"type": "attachment", "attachment_id": ids[1]},
        ]}),
    )
    .await;
    let Some(ConnectionCommand::Prompt { blocks, .. }) = rx.recv().await else {
        panic!("no prompt");
    };
    let texts: Vec<_> = blocks
        .iter()
        .map(|b| match b {
            PromptInputBlock::Text { text } => text.clone(),
            other => panic!("unexpected block {other:?}"),
        })
        .collect();
    assert_eq!(texts.len(), 2);
    assert_eq!(texts[0], "compare these");
    let wt = std::path::Path::new(&h.wt);
    assert_eq!(
        texts[1],
        crate::attachments::note(&[
            wt.join(format!(".zwiim/attachments/{}/a.pdf", ids[0])),
            wt.join(format!(".zwiim/attachments/{}/b.xlsx", ids[1])),
        ])
    );
}

#[tokio::test]
async fn an_attachment_alone_is_a_prompt() {
    let h = host(false).await;
    let (sid, mut rx) = h.live_session().await;
    let up = h
        .ok(
            "attachments.upload",
            json!({"workspace_id": h.wt_workspace_id, "name": "data.csv", "data": b64(b"a,b")}),
        )
        .await;
    h.ok(
        "session.send",
        json!({"session_id": sid, "blocks": [{"type": "attachment", "attachment_id": up["attachment_id"]}]}),
    )
    .await;
    let Some(ConnectionCommand::Prompt { blocks, .. }) = rx.recv().await else {
        panic!("no prompt");
    };
    assert!(matches!(&blocks[..], [PromptInputBlock::Text { text }] if text.starts_with(crate::attachments::NOTE_OPEN)));
}

/// The follow-ups queued on session `sid`, as `session.list` shows them.
async fn follow_ups_of(h: &Host, sid: &str) -> Value {
    let list = h
        .ok("session.list", json!({"workspace_id": h.wt_workspace_id.clone()}))
        .await;
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == sid)
        .expect("listed")
        .clone();
    row.get("follow_ups").cloned().unwrap_or(json!([]))
}

fn prompt_text(cmd: Option<ConnectionCommand>) -> String {
    match cmd {
        Some(ConnectionCommand::Prompt { blocks, .. }) => match &blocks[..] {
            [PromptInputBlock::Text { text }, ..] => text.clone(),
            other => panic!("prompt without text: {other:?}"),
        },
        _ => panic!("no prompt"),
    }
}

#[tokio::test]
async fn follow_up_goes_out_at_once_to_an_idle_session() {
    let h = host(false).await;
    let (sid, mut rx) = h.live_session().await;
    let sent = h
        .ok(
            "session.follow_up",
            json!({"session_id": sid, "blocks": [{"type": "text", "text": "next"}]}),
        )
        .await;
    assert_eq!(sent, json!({"outcome": "sent"}));
    assert_eq!(prompt_text(rx.recv().await), "next");
    assert_eq!(follow_ups_of(&h, &sid).await, json!([]));

    assert_eq!(
        h.err("session.follow_up", json!({"session_id": sid, "blocks": []}))
            .await
            .1,
        "invalid_input"
    );
    assert_eq!(
        h.err(
            "session.follow_up",
            json!({"session_id": "nope", "blocks": [{"type": "text", "text": "x"}]})
        )
        .await
        .1,
        "not_found"
    );
}

#[tokio::test]
async fn follow_ups_queue_behind_a_running_turn_and_can_be_changed() {
    let h = host(false).await;
    let (sid, mut rx) = h.live_session().await;
    let st = h.state.connection_manager.get_state(CONN).await.unwrap();
    st.write().await.turn_in_flight = true;

    for (text, image) in [("first", false), ("second", true), ("third", false)] {
        let mut blocks = vec![json!({"type": "text", "text": text})];
        if image {
            blocks.push(json!({"type": "image", "data": "AAAA", "mime_type": "image/png"}));
        }
        let queued = h
            .ok("session.follow_up", json!({"session_id": sid, "blocks": blocks}))
            .await;
        assert_eq!(queued, json!({"outcome": "queued"}));
    }
    assert!(rx.try_recv().is_err(), "nothing reaches the busy agent");
    let queue = follow_ups_of(&h, &sid).await;
    let texts: Vec<_> = queue.as_array().unwrap().iter().map(|f| f["text"].clone()).collect();
    assert_eq!(texts, [json!("first"), json!("second"), json!("third")]);
    assert_eq!(queue[1]["attachments"], 1);
    let id = |i: usize| queue[i]["follow_up_id"].as_str().unwrap().to_string();

    h.ok(
        "session.follow_up_edit",
        json!({"session_id": sid, "follow_up_id": id(0), "text": "first, edited"}),
    )
    .await;
    // Text is all the first one has, so clearing it would leave nothing.
    assert_eq!(
        h.err(
            "session.follow_up_edit",
            json!({"session_id": sid, "follow_up_id": id(0), "text": "  "})
        )
        .await
        .1,
        "invalid_input"
    );
    assert_eq!(
        h.err(
            "session.follow_up_edit",
            json!({"session_id": sid, "follow_up_id": "gone", "text": "x"})
        )
        .await
        .1,
        "not_found"
    );
    h.ok("session.follow_up_remove", json!({"session_id": sid, "follow_up_id": id(2)}))
        .await;
    // Already gone is fine: another device may have sent or removed it.
    h.ok("session.follow_up_remove", json!({"session_id": sid, "follow_up_id": id(2)}))
        .await;

    // Sending one while the turn runs puts it first.
    let moved = h
        .ok("session.follow_up_send", json!({"session_id": sid, "follow_up_id": id(1)}))
        .await;
    assert_eq!(moved, json!({"outcome": "queued"}));
    let queue = follow_ups_of(&h, &sid).await;
    let texts: Vec<_> = queue.as_array().unwrap().iter().map(|f| f["text"].clone()).collect();
    assert_eq!(texts, [json!("second"), json!("first, edited")]);

    // Idle, it goes out now.
    st.write().await.turn_in_flight = false;
    let sent = h
        .ok("session.follow_up_send", json!({"session_id": sid, "follow_up_id": id(0)}))
        .await;
    assert_eq!(sent, json!({"outcome": "sent"}));
    assert_eq!(prompt_text(rx.recv().await), "first, edited");
    let queue = follow_ups_of(&h, &sid).await;
    assert_eq!(queue.as_array().unwrap().len(), 1);
    assert_eq!(
        h.err("session.follow_up_send", json!({"session_id": sid, "follow_up_id": id(0)}))
            .await
            .1,
        "not_found"
    );
}

#[tokio::test]
async fn the_next_follow_up_goes_out_when_a_turn_ends() {
    let h = host(false).await;
    crate::follow_ups::spawn_tracker(h.state.clone());
    let (sid, mut rx) = h.live_session().await;
    let st = h.state.connection_manager.get_state(CONN).await.unwrap();
    st.write().await.turn_in_flight = true;
    for text in ["one", "two"] {
        h.ok(
            "session.follow_up",
            json!({"session_id": sid, "blocks": [{"type": "text", "text": text}]}),
        )
        .await;
    }

    // A stopped turn holds the queue.
    h.emit(AcpEvent::TurnComplete {
        session_id: "agent-native".into(),
        stop_reason: "cancelled".into(),
        agent_type: "claude_code".into(),
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(rx.try_recv().is_err(), "held after a stop");
    assert_eq!(follow_ups_of(&h, &sid).await.as_array().unwrap().len(), 2);

    st.write().await.turn_in_flight = true;
    h.emit(AcpEvent::TurnComplete {
        session_id: "agent-native".into(),
        stop_reason: "end_turn".into(),
        agent_type: "claude_code".into(),
    })
    .await;
    let next = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("the queue drains");
    assert_eq!(prompt_text(next), "one");
    for _ in 0..50 {
        if follow_ups_of(&h, &sid).await.as_array().unwrap().len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let queue = follow_ups_of(&h, &sid).await;
    assert_eq!(queue[0]["text"], "two");
    assert_eq!(queue.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_steer_the_agent_cannot_take_goes_through_the_queue() {
    let h = host(false).await;
    let (sid, mut rx) = h.live_session().await;
    let steer = |text: &str| json!({"session_id": sid, "blocks": [{"type": "text", "text": text}], "steer": true});

    // Idle: there is no turn to steer, so it starts one.
    assert_eq!(h.ok("session.follow_up", steer("now")).await, json!({"outcome": "sent"}));
    assert_eq!(prompt_text(rx.recv().await), "now");

    // Running, on an agent that cannot take messages mid-turn: it waits.
    let st = h.state.connection_manager.get_state(CONN).await.unwrap();
    st.write().await.turn_in_flight = true;
    assert_eq!(h.ok("session.follow_up", steer("later")).await, json!({"outcome": "queued"}));
    assert!(rx.try_recv().is_err());
    assert_eq!(follow_ups_of(&h, &sid).await[0]["text"], "later");
}

#[tokio::test]
async fn a_steer_goes_into_the_running_turn() {
    use crate::acp::connection::SteerOutcome;
    let h = host(true).await;
    let (sid, mut rx) = h.live_session().await;
    let st = h.state.connection_manager.get_state(CONN).await.unwrap();
    {
        let mut s = st.write().await;
        s.turn_in_flight = true;
        s.native_steering_available = true;
    }
    let mut ws = h.ws().await;
    ws.send_json(&json!({"op": "session.subscribe", "subscription_id": "a", "session_id": sid}))
        .await;
    assert_eq!(recv(&mut ws).await["type"], "snapshot");

    // The agent takes the first; its turn has settled by the second.
    let agent = tokio::spawn(async move {
        let mut outcomes = [SteerOutcome::Injected, SteerOutcome::PromptRequired].into_iter();
        let mut sent = vec![];
        while let Some(outcome) = outcomes.next() {
            match rx.recv().await {
                Some(ConnectionCommand::Steer { blocks, reply }) => {
                    sent.push(blocks);
                    let _ = reply.send(Ok(outcome));
                }
                _ => panic!("expected a steer"),
            }
        }
        sent
    });
    let steer = |text: &str| json!({"session_id": sid, "blocks": [{"type": "text", "text": text}], "steer": true});
    assert_eq!(
        h.ok("session.follow_up", steer("use the cache")).await,
        json!({"outcome": "steered"})
    );
    let ev = recv(&mut ws).await;
    assert_eq!(ev["event"]["type"], "agent.steered", "{ev}");
    assert_eq!(
        ev["event"]["payload"]["blocks"],
        json!([{"type": "text", "text": "use the cache"}])
    );
    assert_eq!(follow_ups_of(&h, &sid).await, json!([]));

    // The turn ended before the agent took it: it keeps its place in line.
    assert_eq!(
        h.ok("session.follow_up", steer("and tests")).await,
        json!({"outcome": "queued"})
    );
    assert_eq!(follow_ups_of(&h, &sid).await[0]["text"], "and tests");
    let sent = agent.await.unwrap();
    assert!(matches!(&sent[0][..], [PromptInputBlock::Text { text }] if text == "use the cache"));
}

// ─── browser ─────────────────────────────────────────────────────────────

/// Give the Host a browser on a fake engine.
async fn fake_browser(h: &Host) -> crate::browser::engine::fake::FakeEngine {
    use crate::browser::engine::{fake::FakeEngine, manager::EngineManager};
    let fake = FakeEngine::default();
    let service = crate::browser::service::BrowserService::start(
        EngineManager::new(fake.start_fn()),
        h.state.db.conn.clone(),
        None,
    )
    .await
    .unwrap();
    assert!(h.state.browser.set(service).is_ok());
    fake
}

#[tokio::test]
async fn browser_tabs_over_http() {
    let h = host(false).await;
    let fake = fake_browser(&h).await;
    assert_eq!(
        h.ok("browser.tabs.list", json!({})).await,
        json!({"tabs": [], "engine": {"state": "stopped"}})
    );

    let conv = seed_conversation(&h.state.db, h.wt_folder_id, AgentType::ClaudeCode).await;
    let sid = session_id_of(&h.state, conv).await;
    let owner = json!({"kind": "session", "session_id": sid});
    let tab = h
        .ok(
            "browser.tab.open",
            json!({"owner": owner, "url": "http://localhost:5173/"}),
        )
        .await;
    let tab_id = tab["tab_id"].as_str().unwrap().to_string();
    assert_eq!(tab["owner"], owner);
    assert_eq!(tab["url"], "http://localhost:5173/");
    assert_eq!(tab["discarded"], false);
    let personal = h
        .ok("browser.tab.open", json!({"owner": {"kind": "personal"}}))
        .await;
    assert_eq!(personal["url"], "about:blank");

    let listed = h.ok("browser.tabs.list", json!({"session_id": sid})).await;
    assert_eq!(listed["tabs"].as_array().unwrap().len(), 1);
    assert_eq!(listed["tabs"][0]["tab_id"], tab_id.as_str());
    assert_eq!(listed["engine"]["state"], "running");
    assert!(listed["engine"]["version"].is_string());
    assert_eq!(
        h.ok("browser.tabs.list", json!({})).await["tabs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    h.ok(
        "browser.tab.navigate",
        json!({"tab_id": tab_id, "url": "https://example.com/"}),
    )
    .await;
    assert!(fake
        .calls("Page.navigate")
        .iter()
        .any(|c| c["url"] == "https://example.com/"));
    h.ok(
        "browser.tab.navigate",
        json!({"tab_id": tab_id, "action": "reload"}),
    )
    .await;
    assert_eq!(fake.calls("Page.reload").len(), 1);

    for body in [
        json!({"tab_id": tab_id}),
        json!({"tab_id": tab_id, "url": "https://example.com/", "action": "back"}),
        json!({"tab_id": tab_id, "action": "sideways"}),
    ] {
        assert_eq!(
            h.err("browser.tab.navigate", body).await,
            (StatusCode::BAD_REQUEST, "invalid_request".into())
        );
    }
    assert_eq!(
        h.err(
            "browser.tab.navigate",
            json!({"tab_id": tab_id, "url": "file:///etc/passwd"}),
        )
        .await,
        (StatusCode::UNPROCESSABLE_ENTITY, "unsupported_url".into())
    );
    assert_eq!(
        h.err(
            "browser.tab.open",
            json!({"owner": {"kind": "session", "session_id": "nope"}}),
        )
        .await,
        (StatusCode::NOT_FOUND, "not_found".into())
    );

    h.ok("browser.tab.close", json!({"tab_id": tab_id})).await;
    assert_eq!(
        h.err("browser.tab.close", json!({"tab_id": tab_id})).await,
        (StatusCode::NOT_FOUND, "tab_not_found".into())
    );
    assert_eq!(
        h.err(
            "browser.tab.navigate",
            json!({"tab_id": tab_id, "action": "stop"})
        )
        .await,
        (StatusCode::NOT_FOUND, "tab_not_found".into())
    );
    assert_eq!(
        h.ok("browser.engine.reset", json!({})).await["engine"]["state"],
        "running"
    );
}
