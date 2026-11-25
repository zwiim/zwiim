//! Validating connections against the fake MCP server over real HTTP,
//! through the stored records, the credential store and egress.

use std::collections::HashMap;
use std::sync::Arc;

use super::{Connectors, McpFactory, UpstreamFactory, ValidateError};
use crate::connectors::credentials::{save_secrets, MemorySecrets};
use crate::connectors::definition::{from_user, Runtime, UserAuth, UserConnector};
use crate::connectors::egress::{NetworkClass, SystemResolver};
use crate::connectors::fake_mcp::{tool, Era, FakeMcp, HttpServer};
use crate::connectors::store::{
    self, create_connection, save_definition, ConnectionStatus, CredentialStore,
};
use crate::connectors::upstream::ShutdownReason;
use crate::db::test_helpers::fresh_in_memory_db;
use crate::db::AppDatabase;

struct Setup {
    db: AppDatabase,
    connectors: Connectors,
    connection: String,
    _server: Option<HttpServer>,
}

/// A stored connection to `url` with bearer key `key` saved, if any.
async fn setup_at(url: &str, key: Option<&str>) -> Setup {
    let db = fresh_in_memory_db().await;
    let input = UserConnector {
        name: "Fake".into(),
        url: url.into(),
        auth: UserAuth::Bearer,
        network: NetworkClass::Local,
    };
    let def = from_user(input, chrono::Utc::now()).unwrap();
    save_definition(&db.conn, &def).await.unwrap();
    let connection = create_connection(&db.conn, &def.id, "Work", CredentialStore::Keychain)
        .await
        .unwrap()
        .id;
    let secrets = Arc::new(MemorySecrets::default());
    if let Some(key) = key {
        let values = HashMap::from([("api_key".to_string(), key.to_string())]);
        save_secrets(&*secrets, &connection, &def.auth, &values).unwrap();
    }
    let connectors = Connectors::new(
        db.conn.clone(),
        Arc::new(McpFactory::new(Arc::new(SystemResolver))),
        secrets,
    );
    Setup {
        db,
        connectors,
        connection,
        _server: None,
    }
}

async fn setup(fake: &FakeMcp) -> Setup {
    let server = fake.serve_http().await;
    let mut setup = setup_at(&server.url(), Some("k3y")).await;
    setup._server = Some(server);
    setup
}

async fn status(setup: &Setup) -> (ConnectionStatus, Option<String>) {
    let c = store::connection(&setup.db.conn, &setup.connection)
        .await
        .unwrap()
        .unwrap();
    (c.status, c.last_error)
}

#[tokio::test]
async fn validating_lists_and_records_the_tools_and_activates_the_connection() {
    for era in [Era::Modern, Era::Legacy("2025-06-18")] {
        let fake = FakeMcp::new(era);
        let setup = setup(&fake).await;
        let changes = setup.connectors.validate(&setup.connection).await.unwrap();
        assert_eq!(changes.added, ["echo"], "{era:?}");
        assert_eq!(status(&setup).await, (ConnectionStatus::Active, None));
        let tools = store::tools(&setup.db.conn, &setup.connection)
            .await
            .unwrap();
        assert_eq!(tools[0].name, "echo");
        // The key went out as the definition says, to the server's origin.
        let sent = fake.received();
        assert!(sent
            .iter()
            .all(|r| r.header("authorization") == Some("Bearer k3y")));
    }
}

#[tokio::test]
async fn validating_again_reconnects_and_reports_changes() {
    let fake = FakeMcp::new(Era::DualEra);
    let setup = setup(&fake).await;
    setup.connectors.validate(&setup.connection).await.unwrap();
    fake.set_tools(vec![tool("search", "Searches")]);
    let changes = setup.connectors.validate(&setup.connection).await.unwrap();
    assert_eq!(changes.added, ["search"]);
    assert_eq!(changes.removed, ["echo"]);
    let discovers = fake
        .received_methods()
        .iter()
        .filter(|m| *m == "server/discover")
        .count();
    assert!(discovers >= 2, "one connection per validation");
}

#[tokio::test]
async fn an_unreachable_service_marks_the_connection_failed() {
    // A port nothing listens on: bind, note it, close.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let setup = setup_at(&format!("http://127.0.0.1:{port}/mcp"), Some("k3y")).await;
    let err = setup
        .connectors
        .validate(&setup.connection)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "connection_failed", "{err}");
    let (state, message) = status(&setup).await;
    assert_eq!(state, ConnectionStatus::Error);
    assert!(!message.unwrap().contains("k3y"));
    assert!(store::tools(&setup.db.conn, &setup.connection)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_missing_key_fails_the_connection_and_a_locked_store_changes_nothing() {
    let fake = FakeMcp::new(Era::Modern);
    let server = fake.serve_http().await;
    let missing = setup_at(&server.url(), None).await;
    let err = missing
        .connectors
        .validate(&missing.connection)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "credential_missing");
    assert_eq!(status(&missing).await.0, ConnectionStatus::Error);

    let locked = setup_at(&server.url(), Some("k3y")).await;
    let broken = Connectors::new(
        locked.db.conn.clone(),
        Arc::new(McpFactory::new(Arc::new(SystemResolver))),
        Arc::new(MemorySecrets {
            broken: true,
            ..Default::default()
        }),
    );
    let err = broken.validate(&locked.connection).await.unwrap_err();
    assert_eq!(err.code(), "credential_store_unavailable");
    assert_eq!(status(&locked).await, (ConnectionStatus::Validating, None));
    assert!(fake.received().is_empty(), "nothing was sent");
}

#[tokio::test]
async fn a_changed_definition_must_be_confirmed_before_anything_is_sent() {
    let fake = FakeMcp::new(Era::Modern);
    let setup = setup(&fake).await;
    let conn = store::connection(&setup.db.conn, &setup.connection)
        .await
        .unwrap()
        .unwrap();
    let mut def = store::definition(&setup.db.conn, &conn.definition_id)
        .await
        .unwrap()
        .unwrap();
    let Runtime::McpHttp { url, .. } = &def.runtime else {
        unreachable!()
    };
    def.runtime = Runtime::McpHttp {
        url: format!("{url}/v2"),
        protocol: None,
    };
    save_definition(&setup.db.conn, &def).await.unwrap();

    let err = setup
        .connectors
        .validate(&setup.connection)
        .await
        .unwrap_err();
    assert!(matches!(err, ValidateError::DefinitionChanged), "{err}");
    assert!(fake.received().is_empty());
}

#[tokio::test]
async fn a_revoked_or_blocked_connection_is_never_contacted() {
    let fake = FakeMcp::new(Era::Modern);
    let setup = setup(&fake).await;
    setup
        .connectors
        .runtime(&setup.connection)
        .shutdown_all(ShutdownReason::Revoked)
        .await;
    let err = setup
        .connectors
        .validate(&setup.connection)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "connection_unavailable");

    let other = setup_at("http://127.0.0.1:9/mcp", Some("k")).await;
    for status in [ConnectionStatus::Revoking, ConnectionStatus::Revoked] {
        store::set_status(&other.db.conn, &other.connection, status, None)
            .await
            .unwrap();
        let err = other
            .connectors
            .validate(&other.connection)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "connection_unavailable");
    }
    assert!(fake.received().is_empty());
}

#[tokio::test]
async fn the_factory_refuses_local_servers() {
    let setup = setup_at("http://127.0.0.1:9/mcp", None).await;
    let conn = store::connection(&setup.db.conn, &setup.connection)
        .await
        .unwrap()
        .unwrap();
    let mut def = store::definition(&setup.db.conn, &conn.definition_id)
        .await
        .unwrap()
        .unwrap();
    def.runtime = Runtime::McpStdio {
        rest: serde_json::Map::new(),
    };
    let factory = McpFactory::new(Arc::new(SystemResolver));
    let err = factory.open(&def, Vec::new(), None).await.err().unwrap();
    assert!(err.to_string().contains("sandbox"), "{err}");
}
