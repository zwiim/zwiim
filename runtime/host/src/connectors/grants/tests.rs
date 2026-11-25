use serde_json::json;

use super::{
    allow_all, grant_for, grants, remove_grant, set_grant, set_tool_policy, Fallback, GrantError,
    Scope, ToolPolicy,
};
use crate::connectors::definition::{from_user, UserAuth, UserConnector};
use crate::connectors::egress::NetworkClass;
use crate::connectors::store::{
    create_connection, save_definition, save_tool_snapshot, CredentialStore,
};
use crate::connectors::upstream::ToolDefinition;
use crate::db::test_helpers::fresh_in_memory_db;
use crate::db::AppDatabase;

fn tool(name: &str, description: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        title: None,
        description: Some(description.into()),
        input_schema: json!({ "type": "object" }),
        output_schema: None,
        annotations: None,
    }
}

/// A connection listing `search` and `delete`.
async fn connected() -> (AppDatabase, String) {
    let db = fresh_in_memory_db().await;
    let input = UserConnector {
        name: "Linear".into(),
        url: "https://mcp.linear.app/mcp".into(),
        auth: UserAuth::Bearer,
        network: NetworkClass::Public,
    };
    let def = from_user(input, chrono::Utc::now()).unwrap();
    save_definition(&db.conn, &def).await.unwrap();
    let id = create_connection(&db.conn, &def.id, "Work", CredentialStore::Keychain)
        .await
        .unwrap()
        .id;
    let listed = [tool("search", "Searches"), tool("delete", "Deletes")];
    save_tool_snapshot(&db.conn, &id, &listed).await.unwrap();
    (db, id)
}

fn scope(workspace_id: Option<&str>, agent: Option<&str>) -> Scope {
    Scope {
        workspace_id: workspace_id.map(Into::into),
        agent: agent.map(Into::into),
    }
}

#[tokio::test]
async fn a_scope_has_one_grant_whose_fallbacks_can_change() {
    let (db, id) = connected().await;
    let first = set_grant(
        &db.conn,
        &id,
        Scope::default(),
        Fallback::Ask,
        Fallback::Ask,
    )
    .await
    .unwrap();
    set_tool_policy(&db.conn, first.id, "search", Some(ToolPolicy::Deny))
        .await
        .unwrap();
    let again = set_grant(
        &db.conn,
        &id,
        Scope::default(),
        Fallback::Deny,
        Fallback::Ask,
    )
    .await
    .unwrap();
    assert_eq!(again.id, first.id);
    assert_eq!(again.created_at, first.created_at);
    assert_eq!(again.default_policy, Fallback::Deny);
    assert_eq!(again.tools.len(), 1, "its rows are kept");
    set_grant(
        &db.conn,
        &id,
        scope(Some("ws1"), None),
        Fallback::Ask,
        Fallback::Ask,
    )
    .await
    .unwrap();
    assert_eq!(grants(&db.conn, &id).await.unwrap().len(), 2);

    let missing = set_grant(
        &db.conn,
        "nope",
        Scope::default(),
        Fallback::Ask,
        Fallback::Ask,
    )
    .await
    .unwrap_err();
    assert!(matches!(missing, GrantError::ConnectionNotFound(_)));
}

#[tokio::test]
async fn the_most_specific_grant_applies() {
    let (db, id) = connected().await;
    let mut ids = Vec::new();
    for (workspace, agent) in [
        (None, None),
        (None, Some("codex")),
        (Some("ws1"), None),
        (Some("ws1"), Some("codex")),
    ] {
        let grant = set_grant(
            &db.conn,
            &id,
            scope(workspace, agent),
            Fallback::Ask,
            Fallback::Ask,
        )
        .await
        .unwrap();
        ids.push(grant.id);
    }
    let pick = |workspace: Option<&'static str>, agent: &'static str| {
        let db = &db;
        let id = &id;
        async move {
            grant_for(&db.conn, id, workspace, agent)
                .await
                .unwrap()
                .map(|g| g.id)
        }
    };
    assert_eq!(pick(Some("ws1"), "codex").await, Some(ids[3]));
    assert_eq!(pick(Some("ws1"), "claude_code").await, Some(ids[2]));
    assert_eq!(pick(Some("ws2"), "codex").await, Some(ids[1]));
    assert_eq!(pick(None, "claude_code").await, Some(ids[0]));

    remove_grant(&db.conn, ids[0]).await.unwrap();
    assert_eq!(
        pick(Some("ws2"), "claude_code").await,
        None,
        "no grant: deny"
    );
    assert!(matches!(
        remove_grant(&db.conn, ids[0]).await,
        Err(GrantError::GrantNotFound(_))
    ));
}

#[tokio::test]
async fn an_allow_is_pinned_to_the_tool_as_it_is_now() {
    let (db, id) = connected().await;
    let grant = set_grant(
        &db.conn,
        &id,
        Scope::default(),
        Fallback::Ask,
        Fallback::Ask,
    )
    .await
    .unwrap();
    let hash = tool("search", "Searches").hash();
    let grant = set_tool_policy(&db.conn, grant.id, "search", Some(ToolPolicy::Allow))
        .await
        .unwrap();
    let row = grant.row("search").unwrap();
    assert_eq!(row.policy, ToolPolicy::Allow);
    assert_eq!(row.schema_hash.as_deref(), Some(hash.as_str()));

    let grant = set_tool_policy(&db.conn, grant.id, "search", Some(ToolPolicy::Ask))
        .await
        .unwrap();
    assert_eq!(grant.row("search").unwrap().schema_hash, None);
    let grant = set_tool_policy(&db.conn, grant.id, "search", None)
        .await
        .unwrap();
    assert!(grant.row("search").is_none());

    let unknown = set_tool_policy(&db.conn, grant.id, "drop_db", Some(ToolPolicy::Allow))
        .await
        .unwrap_err();
    assert!(matches!(unknown, GrantError::ToolNotFound(_)));
}

#[tokio::test]
async fn allowing_all_current_tools_pins_each_and_keeps_denials() {
    let (db, id) = connected().await;
    let grant = set_grant(
        &db.conn,
        &id,
        Scope::default(),
        Fallback::Ask,
        Fallback::Deny,
    )
    .await
    .unwrap();
    set_tool_policy(&db.conn, grant.id, "delete", Some(ToolPolicy::Deny))
        .await
        .unwrap();
    let grant = allow_all(&db.conn, grant.id).await.unwrap();
    let search = grant.row("search").unwrap();
    assert_eq!(search.policy, ToolPolicy::Allow);
    assert_eq!(
        search.schema_hash.as_deref(),
        Some(tool("search", "Searches").hash().as_str())
    );
    assert_eq!(grant.row("delete").unwrap().policy, ToolPolicy::Deny);
}
