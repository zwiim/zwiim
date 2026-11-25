//! `connection.*` through the real router, against the fake MCP server over
//! HTTP, with secrets in memory.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{json, Value};

use super::{bearer_device, host, Host};
use crate::connectors::credentials::MemorySecrets;
use crate::connectors::egress::SystemResolver;
use crate::connectors::fake_mcp::{tool, Era, FakeMcp, HttpServer};
use crate::connectors::runtime::{Connectors, McpFactory};

const KEY: &str = "k3y-9d1e";

/// A change to a `connection.create` body.
type Edit = fn(&mut Value);

async fn connected_host() -> (Host, Arc<MemorySecrets>) {
    let h = host(false).await;
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

async fn created(h: &Host, server: &HttpServer) -> String {
    let body = ok(h, "connection.create", create_body(&server.url())).await;
    body["connection"]["service_connection_id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn a_server_added_by_url_is_connected_listed_and_its_tools_shown() {
    let fake = FakeMcp::new(Era::Modern);
    let server = fake.serve_http().await;
    let (h, secrets) = connected_host().await;
    let body = ok(&h, "connection.create", create_body(&server.url())).await;
    assert_eq!(body["validation_error"], Value::Null);
    let connection = &body["connection"];
    assert_eq!(connection["name"], "Fake");
    assert_eq!(connection["label"], "Work");
    assert_eq!(connection["slug"], "fake_work");
    assert_eq!(connection["status"], "active");
    assert_eq!(connection["credentials"], "keychain");
    assert_eq!(connection["tool_count"], 1);
    assert_eq!(
        connection["endpoints"],
        json!([{ "origin": server.url().trim_end_matches("/mcp"), "credentials": true }])
    );
    assert!(connection["last_validated_at"].is_string());
    let uid = connection["service_connection_id"].as_str().unwrap();
    assert_eq!(secrets.names(), [format!("connector:{uid}:api_key")]);

    let list = ok(&h, "connection.list", json!({})).await;
    assert_eq!(list["connections"], json!([connection]));

    let tools = ok(
        &h,
        "connection.tools",
        json!({ "service_connection_id": uid }),
    )
    .await;
    assert_eq!(tools["tools"][0]["name"], "echo");
    assert_eq!(tools["tools"][0]["input_schema"]["type"], "object");

    fake.set_tools(vec![tool("search", "Searches")]);
    let body = ok(
        &h,
        "connection.validate",
        json!({ "service_connection_id": uid }),
    )
    .await;
    assert_eq!(
        body["changes"],
        json!({ "added": ["search"], "changed": [], "removed": ["echo"] })
    );
}

#[tokio::test]
async fn a_new_key_reconnects_the_same_connection() {
    let fake = FakeMcp::new(Era::Modern);
    let server = fake.serve_http().await;
    let (h, secrets) = connected_host().await;
    let id = created(&h, &server).await;
    let new_key = "n3w-k3y-77";
    let body = ok(
        &h,
        "connection.set_secret",
        json!({ "service_connection_id": id, "secret": new_key }),
    )
    .await;
    assert!(!body.to_string().contains(new_key), "the new key came back");
    assert_eq!(body["validation_error"], Value::Null);
    assert_eq!(body["connection"]["service_connection_id"], id.as_str());
    assert_eq!(body["connection"]["status"], "active");
    assert_eq!(body["connection"]["label"], "Work");
    let last = fake.received().pop().unwrap();
    assert_eq!(
        last.header("authorization"),
        Some(format!("Bearer {new_key}").as_str())
    );
    assert_eq!(secrets.names().len(), 1);

    let (status, code) = h
        .err(
            "connection.set_secret",
            json!({ "service_connection_id": id, "secret": " " }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::BAD_REQUEST, "invalid_input")
    );
    ok(
        &h,
        "connection.revoke",
        json!({ "service_connection_id": id }),
    )
    .await;
    let (status, code) = h
        .err(
            "connection.set_secret",
            json!({ "service_connection_id": id, "secret": new_key }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::CONFLICT, "connection_unavailable")
    );
}

#[tokio::test]
async fn a_server_that_cannot_be_reached_is_kept_as_failed() {
    let (h, _) = connected_host().await;
    let body = ok(
        &h,
        "connection.create",
        create_body("http://127.0.0.1:9/mcp"),
    )
    .await;
    assert_eq!(body["connection"]["status"], "error");
    assert_eq!(body["validation_error"]["code"], "connection_failed");
    assert!(body["connection"]["error"].is_string());
    let uid = body["connection"]["service_connection_id"]
        .as_str()
        .unwrap();
    let (status, code) = h
        .err(
            "connection.validate",
            json!({ "service_connection_id": uid }),
        )
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
        let (status, code) = h.err("connection.create", body).await;
        assert_eq!(
            (status, code.as_str()),
            (StatusCode::BAD_REQUEST, "invalid_input"),
            "{what}"
        );
    }
    let mut body = create_body(url);
    body["extra"] = json!(1);
    let (_, code) = h.err("connection.create", body).await;
    assert_eq!(code, "invalid_request");

    secrets.broken.store(true, Ordering::SeqCst);
    let (status, code) = h.err("connection.create", create_body(url)).await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, "keychain_unavailable")
    );
    let list = ok(&h, "connection.list", json!({})).await;
    assert_eq!(list["connections"], json!([]));
}

#[tokio::test]
async fn revoking_makes_the_connection_unusable_and_retries_a_locked_keychain() {
    let fake = FakeMcp::new(Era::Modern);
    let server = fake.serve_http().await;
    let (h, secrets) = connected_host().await;
    let uid = created(&h, &server).await;
    let one = json!({ "service_connection_id": uid });

    secrets.broken.store(true, Ordering::SeqCst);
    let body = ok(&h, "connection.revoke", one.clone()).await;
    assert_eq!(body["connection"]["status"], "revoking");
    assert!(body["connection"]["error"].is_string());
    let (status, code) = h.err("connection.validate", one.clone()).await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::CONFLICT, "connection_unavailable")
    );

    secrets.broken.store(false, Ordering::SeqCst);
    let body = ok(&h, "connection.revoke", one.clone()).await;
    assert_eq!(body["connection"]["status"], "revoked");
    assert_eq!(body["connection"]["error"], Value::Null);
    assert!(secrets.names().is_empty());
    // Revoking again is fine.
    ok(&h, "connection.revoke", one).await;
}

#[tokio::test]
async fn unknown_connections_are_not_found() {
    let (h, _) = connected_host().await;
    for op in [
        "connection.validate",
        "connection.revoke",
        "connection.tools",
    ] {
        let (status, code) = h.err(op, json!({ "service_connection_id": "nope" })).await;
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
        .call_as(&device, "connection.create", create_body(&server.url()))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "permission_denied");
    let (status, body) = h
        .call_as(
            &device,
            "connection.set_secret",
            json!({ "service_connection_id": uid, "secret": "k" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "permission_denied");

    let (status, body) = h.call_as(&device, "connection.list", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["connections"].as_array().unwrap().len(), 1);
    let (status, body) = h
        .call_as(
            &device,
            "connection.revoke",
            json!({ "service_connection_id": uid }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["connection"]["status"], "revoked");
}
