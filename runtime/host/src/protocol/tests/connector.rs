//! `connector.*` through the real router, against the fake MCP server over
//! HTTP, with secrets in memory.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{json, Value};

use super::{bearer_device, event_is, host, Host};
use crate::connectors::credentials::MemorySecrets;
use crate::connectors::egress::SystemResolver;
use crate::connectors::fake_mcp::{tool, Era, FakeMcp, HttpServer};
use crate::connectors::runtime::{CallSession, Connectors, McpFactory, NoCards};

const KEY: &str = "k3y-9d1e";

/// A change to a `connector.create` body.
type Edit = fn(&mut Value);

async fn connected_host() -> (Host, Arc<MemorySecrets>) {
    connected_host_over(false).await
}

/// With `http_transport`, the Host also serves its sockets.
async fn connected_host_over(http_transport: bool) -> (Host, Arc<MemorySecrets>) {
    let h = host(http_transport).await;
    let secrets = Arc::new(MemorySecrets::default());
    let service = Connectors::new(
        h.state.db.conn.clone(),
        Arc::new(McpFactory::new(Arc::new(SystemResolver))),
        secrets.clone(),
    );
    assert!(h.state.connectors.set(Arc::new(service)).is_ok());
    (h, secrets)
}

fn create_body(url: &str) -> Value {
    json!({
        "name": "Fake",
        "url": url,
        "auth": { "kind": "bearer" },
        "network": "local",
        "label": "Work",
        "secret": KEY,
    })
}

/// `op` answered, with the key never in what came back.
async fn ok(h: &Host, op: &str, body: Value) -> Value {
    let value = h.ok(op, body).await;
    assert!(!value.to_string().contains(KEY), "{op} echoed the key");
    value
}

/// A session that may run every tool the connector lists now.
async fn allowed(h: &Host, id: &str) -> CallSession {
    let grant = ok(
        h,
        "connector.grant.set",
        json!({ "connector_id": id, "default_policy": "ask", "new_tools": "ask" }),
    )
    .await;
    ok(
        h,
        "connector.grant.allow_all",
        json!({ "grant_id": grant["grant"]["grant_id"] }),
    )
    .await;
    CallSession {
        session_id: "s1".into(),
        workspace_id: None,
        agent: "codex".into(),
        engine: false,
        cards: Arc::new(NoCards),
    }
}

async fn created(h: &Host, server: &HttpServer) -> String {
    let body = ok(h, "connector.create", create_body(&server.url())).await;
    body["connector"]["connector_id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn a_server_added_by_url_is_connected_listed_and_its_tools_shown() {
    // Legacy over HTTP reports no tool changes of its own, so the
    // validation below is what sees the new list.
    let fake = FakeMcp::new(Era::Legacy("2025-06-18"));
    let server = fake.serve_http().await;
    let (h, secrets) = connected_host().await;
    let body = ok(&h, "connector.create", create_body(&server.url())).await;
    assert_eq!(body["validation_error"], Value::Null);
    let connector = &body["connector"];
    assert_eq!(connector["name"], "Fake");
    assert_eq!(connector["label"], "Work");
    assert_eq!(connector["slug"], "fake_work");
    assert_eq!(connector["status"], "active");
    assert_eq!(connector["credentials"], "keychain");
    assert_eq!(connector["tool_count"], 1);
    assert_eq!(
        connector["endpoints"],
        json!([{ "origin": server.url().trim_end_matches("/mcp"), "credentials": true }])
    );
    assert!(connector["last_validated_at"].is_string());
    let uid = connector["connector_id"].as_str().unwrap();
    assert_eq!(secrets.names(), [format!("connector:{uid}:api_key")]);

    let list = ok(&h, "connector.list", json!({})).await;
    assert_eq!(list["connectors"], json!([connector]));

    let tools = ok(&h, "connector.tools", json!({ "connector_id": uid })).await;
    assert_eq!(tools["tools"][0]["name"], "echo");
    assert_eq!(tools["tools"][0]["input_schema"]["type"], "object");

    fake.set_tools(vec![tool("search", "Searches")]);
    let body = ok(&h, "connector.validate", json!({ "connector_id": uid })).await;
    assert_eq!(
        body["changes"],
        json!({ "added": ["search"], "changed": [], "removed": ["echo"] })
    );
}

#[tokio::test]
async fn a_new_key_reconnects_the_same_connector() {
    let fake = FakeMcp::new(Era::Modern);
    let server = fake.serve_http().await;
    let (h, secrets) = connected_host().await;
    let id = created(&h, &server).await;
    let new_key = "n3w-k3y-77";
    let body = ok(
        &h,
        "connector.set_secret",
        json!({ "connector_id": id, "secret": new_key }),
    )
    .await;
    assert!(!body.to_string().contains(new_key), "the new key came back");
    assert_eq!(body["validation_error"], Value::Null);
    assert_eq!(body["connector"]["connector_id"], id.as_str());
    assert_eq!(body["connector"]["status"], "active");
    assert_eq!(body["connector"]["label"], "Work");
    let last = fake.received().pop().unwrap();
    assert_eq!(
        last.header("authorization"),
        Some(format!("Bearer {new_key}").as_str())
    );
    assert_eq!(secrets.names().len(), 1);

    let (status, code) = h
        .err(
            "connector.set_secret",
            json!({ "connector_id": id, "secret": " " }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::BAD_REQUEST, "invalid_input")
    );
    ok(&h, "connector.revoke", json!({ "connector_id": id })).await;
    let (status, code) = h
        .err(
            "connector.set_secret",
            json!({ "connector_id": id, "secret": new_key }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::CONFLICT, "connector_unavailable")
    );
}

#[tokio::test]
async fn a_server_that_cannot_be_reached_is_kept_as_failed() {
    let (h, _) = connected_host().await;
    let body = ok(
        &h,
        "connector.create",
        create_body("http://127.0.0.1:9/mcp"),
    )
    .await;
    assert_eq!(body["connector"]["status"], "error");
    assert_eq!(body["validation_error"]["code"], "connection_failed");
    assert!(body["connector"]["error"].is_string());
    let uid = body["connector"]["connector_id"].as_str().unwrap();
    let (status, code) = h
        .err("connector.validate", json!({ "connector_id": uid }))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::BAD_GATEWAY, "connection_failed")
    );
}

#[tokio::test]
async fn refused_creations_say_why_and_leave_nothing() {
    let (h, secrets) = connected_host().await;
    let url = "http://127.0.0.1:9/mcp";
    let edits: [(&str, Edit); 5] = [
        ("no key", |b| b["secret"] = Value::Null),
        ("key for an open server", |b| {
            b["auth"] = json!({ "kind": "none" })
        }),
        ("public plain http", |b| b["network"] = json!("public")),
        ("blank label", |b| b["label"] = json!(" ")),
        ("bad header", |b| {
            b["auth"] = json!({ "kind": "header", "header": "bad header" })
        }),
    ];
    for (what, edit) in edits {
        let mut body = create_body(url);
        edit(&mut body);
        let (status, code) = h.err("connector.create", body).await;
        assert_eq!(
            (status, code.as_str()),
            (StatusCode::BAD_REQUEST, "invalid_input"),
            "{what}"
        );
    }
    let mut body = create_body(url);
    body["extra"] = json!(1);
    let (_, code) = h.err("connector.create", body).await;
    assert_eq!(code, "invalid_request");

    secrets.broken.store(true, Ordering::SeqCst);
    let (status, code) = h.err("connector.create", create_body(url)).await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, "keychain_unavailable")
    );
    let list = ok(&h, "connector.list", json!({})).await;
    assert_eq!(list["connectors"], json!([]));
}

#[tokio::test]
async fn revoking_makes_the_connector_unusable_and_retries_a_locked_keychain() {
    let fake = FakeMcp::new(Era::Modern);
    let server = fake.serve_http().await;
    let (h, secrets) = connected_host().await;
    let uid = created(&h, &server).await;
    let one = json!({ "connector_id": uid });

    secrets.broken.store(true, Ordering::SeqCst);
    let body = ok(&h, "connector.revoke", one.clone()).await;
    assert_eq!(body["connector"]["status"], "revoking");
    assert!(body["connector"]["error"].is_string());
    let (status, code) = h.err("connector.validate", one.clone()).await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::CONFLICT, "connector_unavailable")
    );

    secrets.broken.store(false, Ordering::SeqCst);
    let body = ok(&h, "connector.revoke", one.clone()).await;
    assert_eq!(body["connector"]["status"], "revoked");
    assert_eq!(body["connector"]["error"], Value::Null);
    assert!(secrets.names().is_empty());
    // Revoking again is fine.
    ok(&h, "connector.revoke", one).await;
}

#[tokio::test]
async fn unknown_connectors_are_not_found() {
    let (h, _) = connected_host().await;
    for op in [
        "connector.validate",
        "connector.revoke",
        "connector.tools",
        "connector.executions",
    ] {
        let (status, code) = h.err(op, json!({ "connector_id": "nope" })).await;
        assert_eq!(
            (status, code.as_str()),
            (StatusCode::NOT_FOUND, "not_found"),
            "{op}"
        );
    }
}

#[tokio::test]
async fn a_paired_device_lists_and_revokes_but_adds_nothing() {
    let fake = FakeMcp::new(Era::Modern);
    let server = fake.serve_http().await;
    let (h, _) = connected_host().await;
    let uid = created(&h, &server).await;
    let device = bearer_device(&h, "dev-1").await;

    let (status, body) = h
        .call_as(&device, "connector.create", create_body(&server.url()))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "permission_denied");
    let (status, body) = h
        .call_as(
            &device,
            "connector.set_secret",
            json!({ "connector_id": uid, "secret": "k" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "permission_denied");

    let (status, body) = h.call_as(&device, "connector.list", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["connectors"].as_array().unwrap().len(), 1);
    let (status, body) = h
        .call_as(&device, "connector.revoke", json!({ "connector_id": uid }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["connector"]["status"], "revoked");
}

/// Runs grant op `op`, which makes every proxy list its tools again.
async fn changed(h: &Host, id: &str, op: &str, body: Value) -> Value {
    let connectors = h.state.connectors().await;
    let before = connectors.runtime(id).revision();
    let result = ok(h, op, body).await;
    assert!(connectors.runtime(id).revision() > before, "{op}");
    result
}

#[tokio::test]
async fn grants_say_which_sessions_may_use_a_connector_and_each_tool() {
    let fake = FakeMcp::new(Era::Legacy("2025-06-18"));
    let server = fake.serve_http().await;
    let (h, _) = connected_host().await;
    let id = created(&h, &server).await;
    let one = json!({ "connector_id": id });

    let body = changed(
        &h,
        &id,
        "connector.grant.set",
        json!({ "connector_id": id, "default_policy": "ask", "new_tools": "deny" }),
    )
    .await;
    let grant = &body["grant"];
    assert_eq!(grant["workspace_id"], Value::Null);
    assert_eq!(grant["agent"], Value::Null);
    assert_eq!(grant["default_policy"], "ask");
    assert_eq!(grant["new_tools"], "deny");
    let grant_id = grant["grant_id"].clone();

    let body = changed(
        &h,
        &id,
        "connector.tool_policy.set",
        json!({ "grant_id": grant_id, "tool": "echo", "policy": "allow" }),
    )
    .await;
    assert_eq!(
        body["grant"]["tools"],
        json!([{ "name": "echo", "policy": "allow", "changed": false }])
    );

    // The tool changes: its allow no longer holds, until given again.
    fake.set_tools(vec![tool("echo", "Echoes, now differently")]);
    ok(&h, "connector.validate", one.clone()).await;
    let list = ok(&h, "connector.grant.list", one.clone()).await;
    assert_eq!(list["grants"][0]["tools"][0]["changed"], true);
    let body = changed(
        &h,
        &id,
        "connector.grant.allow_all",
        json!({ "grant_id": grant_id }),
    )
    .await;
    assert_eq!(body["grant"]["tools"][0]["changed"], false);

    let scoped = changed(
        &h,
        &id,
        "connector.grant.set",
        json!({ "connector_id": id, "workspace_id": h.workspace_id,
                "agent": "codex", "default_policy": "deny", "new_tools": "deny" }),
    )
    .await;
    assert_eq!(scoped["grant"]["workspace_id"], h.workspace_id.as_str());
    assert_eq!(scoped["grant"]["agent"], "codex");
    let list = ok(&h, "connector.grant.list", one.clone()).await;
    assert_eq!(list["grants"].as_array().unwrap().len(), 2);

    changed(
        &h,
        &id,
        "connector.grant.remove",
        json!({ "grant_id": scoped["grant"]["grant_id"] }),
    )
    .await;
    let list = ok(&h, "connector.grant.list", one).await;
    assert_eq!(list["grants"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn grant_changes_that_cannot_hold_are_refused() {
    let fake = FakeMcp::new(Era::Modern);
    let server = fake.serve_http().await;
    let (h, _) = connected_host().await;
    let id = created(&h, &server).await;
    let set = |extra: Value| {
        let mut body = json!({ "connector_id": id, "default_policy": "ask",
                               "new_tools": "ask" });
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        body
    };

    let (_, code) = h
        .err(
            "connector.grant.set",
            set(json!({ "default_policy": "allow" })),
        )
        .await;
    assert_eq!(code, "invalid_request", "a fallback never allows");
    let (status, code) = h
        .err(
            "connector.grant.set",
            set(json!({ "workspace_id": h.wt_workspace_id })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::BAD_REQUEST, "invalid_input")
    );
    let (_, code) = h
        .err(
            "connector.grant.set",
            set(json!({ "workspace_id": "nope" })),
        )
        .await;
    assert_eq!(code, "not_found");
    let (_, code) = h
        .err(
            "connector.grant.set",
            set(json!({ "connector_id": "nope" })),
        )
        .await;
    assert_eq!(code, "not_found");

    let grant = ok(&h, "connector.grant.set", set(json!({}))).await;
    let grant_id = grant["grant"]["grant_id"].clone();
    let (_, code) = h
        .err(
            "connector.tool_policy.set",
            json!({ "grant_id": grant_id, "tool": "drop_db", "policy": "allow" }),
        )
        .await;
    assert_eq!(code, "not_found");
    ok(
        &h,
        "connector.grant.remove",
        json!({ "grant_id": grant_id }),
    )
    .await;
    let (_, code) = h
        .err("connector.grant.remove", json!({ "grant_id": grant_id }))
        .await;
    assert_eq!(code, "not_found");

    let device = bearer_device(&h, "dev-1").await;
    for (op, body) in [
        ("connector.grant.set", set(json!({}))),
        ("connector.grant.remove", json!({ "grant_id": 1 })),
        (
            "connector.tool_policy.set",
            json!({ "grant_id": 1, "tool": "echo", "policy": "allow" }),
        ),
        ("connector.grant.allow_all", json!({ "grant_id": 1 })),
    ] {
        let (status, body) = h.call_as(&device, op, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{op}: {body}");
    }
    let (status, _) = h
        .call_as(
            &device,
            "connector.grant.list",
            json!({ "connector_id": id }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "a paired device may look");
}

#[tokio::test]
async fn a_connectors_calls_are_listed_newest_first_without_their_arguments() {
    let fake = FakeMcp::new(Era::Legacy("2025-06-18"));
    let server = fake.serve_http().await;
    let (h, _) = connected_host().await;
    let id = created(&h, &server).await;
    let connectors = h.state.connectors().await;
    let session = allowed(&h, &id).await;
    let secret = json!({ "text": "ship the token sk-live-42" });
    let arguments = secret.as_object().unwrap().clone();
    for _ in 0..3 {
        assert!(
            !connectors
                .call(&session, &id, "echo", arguments.clone())
                .await
                .is_error
        );
    }
    assert!(
        connectors
            .call(&session, &id, "missing", arguments)
            .await
            .is_error
    );

    let device = bearer_device(&h, "dev-1").await;
    let (status, body) = h
        .call_as(
            &device,
            "connector.executions",
            json!({ "connector_id": id, "limit": 2 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body.to_string().contains("sk-live-42"), "{body}");
    let first = body["executions"].as_array().unwrap();
    assert_eq!(first.len(), 2);
    let newest = &first[0];
    assert_eq!(newest["tool"], "missing");
    assert_eq!(newest["state"], "rejected");
    assert_eq!(newest["reason"], "not_exposed");
    assert_eq!(newest["dispatched_at"], Value::Null);
    assert!(newest["detail"]
        .as_str()
        .unwrap()
        .contains("nothing was sent"));
    let cursor = body["next_cursor"].clone();
    assert!(cursor.is_string());

    let body = ok(
        &h,
        "connector.executions",
        json!({ "connector_id": id, "cursor": cursor }),
    )
    .await;
    let rest = body["executions"].as_array().unwrap();
    assert_eq!(rest.len(), 2);
    assert_eq!(body["next_cursor"], Value::Null);
    let oldest = &rest[1];
    assert_eq!(oldest["state"], "succeeded");
    assert_eq!(oldest["arg_keys"], json!(["text"]));
    assert!(oldest["dispatched_at"].is_string());
    assert!(oldest["upstream_ms"].is_i64() && oldest["overhead_ms"].is_i64());
    assert_eq!(oldest["detail"], Value::Null);

    let (status, code) = h
        .err(
            "connector.executions",
            json!({ "connector_id": id, "cursor": "nope" }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::BAD_REQUEST, "invalid_input")
    );
}

#[tokio::test]
async fn each_call_is_published_as_it_starts_and_as_it_ends() {
    let fake = FakeMcp::new(Era::Legacy("2025-06-18"));
    let server = fake.serve_http().await;
    let (h, _) = connected_host_over(true).await;
    let id = created(&h, &server).await;
    let (mut feed, snapshot) = h.feed().await;
    assert!(
        !snapshot.to_string().contains("execution"),
        "not in the snapshot"
    );

    let connectors = h.state.connectors().await;
    let secret = json!({ "text": "ship the token sk-live-42" });
    let arguments = secret.as_object().unwrap().clone();
    let session = allowed(&h, &id).await;
    assert!(
        !connectors
            .call(&session, &id, "echo", arguments)
            .await
            .is_error
    );

    let started = h
        .host_event(
            &mut feed,
            "the call started",
            event_is("connector.execution", |p| {
                p["execution"]["state"] == "received"
            }),
        )
        .await;
    assert_eq!(started["event"]["payload"]["connector_id"], id.as_str());
    let execution_id = started["event"]["payload"]["execution"]["execution_id"].clone();
    let ended = h
        .host_event(
            &mut feed,
            "the call ended",
            event_is("connector.execution", |p| {
                p["execution"]["execution_id"] == execution_id
            }),
        )
        .await;
    assert_eq!(ended["event"]["payload"]["execution"]["state"], "succeeded");
    assert!(!ended.to_string().contains("sk-live-42"), "{ended}");
}

#[tokio::test]
async fn a_call_that_asks_is_answered_through_its_session() {
    use crate::acp::delegation::listener::ConnectorSessionLookup;

    let fake = FakeMcp::new(Era::Legacy("2025-06-18"));
    let server = fake.serve_http().await;
    let (h, _) = connected_host().await;
    let id = created(&h, &server).await;
    let (sid, _rx) = h.live_session().await;
    // Granted to the session's root workspace and agent; tools ask.
    ok(
        &h,
        "connector.grant.set",
        json!({
            "connector_id": id,
            "workspace_id": h.workspace_id,
            "agent": "claude_code",
            "default_policy": "ask",
            "new_tools": "ask",
        }),
    )
    .await;
    let session = crate::acp::connector_cards::ConnectorSessions::new(
        h.state.db.conn.clone(),
        h.state.connection_manager.clone_ref(),
    )
    .session(super::CONN, std::path::Path::new(&h.wt), false)
    .await
    .unwrap();
    assert_eq!(
        session.workspace_id.as_deref(),
        Some(h.workspace_id.as_str())
    );

    let connectors = h.state.connectors().await;
    let call = {
        let (connectors, id) = (connectors.clone(), id.clone());
        let arguments = json!({ "text": "hi" }).as_object().unwrap().clone();
        tokio::spawn(async move { connectors.call(&session, &id, "echo", arguments).await })
    };
    let request = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let state = h
                .state
                .connection_manager
                .get_state(super::CONN)
                .await
                .unwrap();
            let snapshot = state.read().await.to_snapshot();
            let request = crate::protocol::events::project_snapshot(&snapshot, None)
                ["pending_request"]
                .clone();
            if !request.is_null() {
                return request;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the card is shown");
    assert_eq!(request["kind"], "connector_approval");
    let request_id = request["request_id"].clone();
    let binding = request["binding_hash"].clone();
    let respond = |decision: &str, binding: Value| {
        json!({
            "session_id": sid,
            "request_id": request_id,
            "kind": "connector_approval",
            "answer": { "decision": decision, "binding_hash": binding },
        })
    };

    let device = bearer_device(&h, "dev-1").await;
    let (status, body) = h
        .call_as(
            &device,
            "session.respond",
            respond("always", binding.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "permission_denied");
    let (status, body) = h
        .call_as(
            &device,
            "session.respond",
            respond("once", json!("sha256:other")),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "approval_mismatch");
    let (status, body) = h
        .call_as(&device, "session.respond", respond("once", binding.clone()))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let output = tokio::time::timeout(std::time::Duration::from_secs(5), call)
        .await
        .unwrap()
        .unwrap();
    assert!(!output.is_error, "{output:?}");
    let (status, body) = h
        .call_as(&device, "session.respond", respond("deny", binding))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "already_resolved");
    assert_eq!(body["error"]["details"]["decided_by"], "device");

    let page = ok(&h, "connector.executions", json!({ "connector_id": id })).await;
    assert_eq!(page["executions"][0]["state"], "succeeded");
}
