use std::path::{Path, PathBuf};
use std::sync::Arc;

use sacp::schema::{McpServer, McpServerStdio};

use super::{end, inject};
use crate::acp::connection::DelegationInjection;
use crate::acp::connection::{test_delegation_injection, TestAllAgentsAvailable};
use crate::connectors::credentials::MemorySecrets;
use crate::connectors::definition::{from_user, UserAuth, UserConnector};
use crate::connectors::egress::{NetworkClass, SystemResolver};
use crate::connectors::grants::{self, Fallback, Scope};
use crate::connectors::runtime::{Connectors, McpFactory};
use crate::connectors::store::{self, ConnectorStatus, CredentialStore};
use crate::connectors::upstream::ToolDefinition;
use crate::models::agent::AgentType;

fn binary() -> Option<PathBuf> {
    Some(PathBuf::from("/opt/zwiim/zwiim-mcp"))
}

fn injection() -> DelegationInjection {
    test_delegation_injection(Arc::new(TestAllAgentsAvailable))
}

/// An active connector with one tool that every session may ask to use,
/// on the Host's connectors given to `injection`. Returns its id and slug.
async fn granted(injection: &DelegationInjection) -> (String, String) {
    let db = crate::db::test_helpers::fresh_in_memory_db().await;
    let def = from_user(
        UserConnector {
            name: "Fake".into(),
            url: "http://127.0.0.1:9/mcp".into(),
            auth: UserAuth::Bearer,
            network: NetworkClass::Local,
        },
        chrono::Utc::now(),
    )
    .unwrap();
    store::save_definition(&db.conn, &def).await.unwrap();
    let connector = store::create_connector(&db.conn, &def.id, "Work", CredentialStore::Keychain)
        .await
        .unwrap();
    store::set_status(&db.conn, &connector.id, ConnectorStatus::Active, None)
        .await
        .unwrap();
    let echo = ToolDefinition {
        name: "echo".into(),
        title: None,
        description: None,
        input_schema: serde_json::json!({ "type": "object" }),
        output_schema: None,
        annotations: None,
    };
    store::save_tool_snapshot(&db.conn, &connector.id, &[echo])
        .await
        .unwrap();
    grants::set_grant(
        &db.conn,
        &connector.id,
        Scope::default(),
        Fallback::Ask,
        Fallback::Ask,
    )
    .await
    .unwrap();
    let connectors = Connectors::new(
        db.conn.clone(),
        Arc::new(McpFactory::new(Arc::new(SystemResolver))),
        Arc::new(MemorySecrets::default()),
    );
    assert!(injection.connectors.set(Arc::new(connectors)).is_ok());
    (connector.id, connector.slug)
}

fn stdio(server: &McpServer) -> &McpServerStdio {
    match server {
        McpServer::Stdio(s) => s,
        other => panic!("a stdio server, not {other:?}"),
    }
}

fn token_of(server: &McpServerStdio) -> String {
    let at = server.args.iter().position(|a| a == "--token").unwrap();
    server.args[at + 1].clone()
}

#[tokio::test]
async fn a_session_gets_one_proxy_per_connector_it_may_use() {
    let injection = injection();
    let (id, slug) = granted(&injection).await;
    let mut servers = Vec::new();
    let injected = launch(&injection, &mut servers, false, binary).await;
    assert_eq!(injected, [slug.clone()]);
    let [server] = &servers[..] else {
        panic!("one proxy: {servers:?}");
    };
    let server = stdio(server);
    assert_eq!(server.name, slug);
    assert_eq!(server.command, PathBuf::from("/opt/zwiim/zwiim-mcp"));
    assert!(server.args.iter().any(|a| a == "--connector"));

    let token = token_of(server);
    let entry = injection.tokens.lookup(&token).await.expect("registered");
    assert_eq!(entry.parent_connection_id, "c1");
    assert_eq!(entry.connector.as_deref(), Some(id.as_str()));
    assert!(entry.engine.is_none());

    end(&injection, "c1").await;
    assert!(
        injection.tokens.lookup(&token).await.is_none(),
        "the token goes with the agent connection"
    );
}

/// The proxies a Codex session outside every workspace gets.
async fn launch(
    injection: &DelegationInjection,
    servers: &mut Vec<McpServer>,
    engine: bool,
    locate: fn() -> Option<PathBuf>,
) -> Vec<String> {
    inject(
        servers,
        injection,
        "c1",
        Path::new("/nowhere"),
        AgentType::Codex,
        engine,
        locate,
    )
    .await
}

#[tokio::test]
async fn some_sessions_get_no_proxies() {
    let injection = injection();
    let mut servers = Vec::new();
    assert!(
        launch(&injection, &mut servers, false, binary)
            .await
            .is_empty(),
        "the Host's connectors aren't made yet"
    );
    granted(&injection).await;
    assert!(
        launch(&injection, &mut servers, true, binary)
            .await
            .is_empty(),
        "engine sessions"
    );
    assert!(
        launch(&injection, &mut servers, false, || None)
            .await
            .is_empty(),
        "no zwiim-mcp binary"
    );
    assert!(servers.is_empty());
}

#[tokio::test]
async fn a_users_own_server_of_the_same_name_keeps_it() {
    let injection = injection();
    let (_, slug) = granted(&injection).await;
    let mut servers = vec![McpServer::Stdio(McpServerStdio::new(
        slug.clone(),
        "/usr/bin/true",
    ))];
    let injected = launch(&injection, &mut servers, false, binary).await;
    assert!(injected.is_empty());
    assert_eq!(servers.len(), 1);
    assert_eq!(stdio(&servers[0]).command, PathBuf::from("/usr/bin/true"));
}
