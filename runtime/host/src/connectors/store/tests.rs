use sea_orm::{ActiveModelTrait, EntityTrait, Set};

use super::{definition, save_definition, StoreError};
use crate::connectors::definition::{from_user, Runtime, UserAuth, UserConnector};
use crate::connectors::egress::NetworkClass;
use crate::db::entities::connector_definition;
use crate::db::test_helpers::fresh_in_memory_db;

fn user_definition(url: &str) -> crate::connectors::definition::ConnectorDefinition {
    let input = UserConnector {
        name: "Linear".into(),
        url: url.into(),
        auth: UserAuth::Bearer,
        network: NetworkClass::Public,
    };
    from_user(input, chrono::Utc::now()).unwrap()
}

#[tokio::test]
async fn a_definition_is_stored_and_read_back_with_its_columns() {
    let db = fresh_in_memory_db().await;
    let def = user_definition("https://mcp.linear.app/mcp");
    save_definition(&db.conn, &def).await.unwrap();

    assert_eq!(
        definition(&db.conn, &def.id).await.unwrap(),
        Some(def.clone())
    );
    assert_eq!(definition(&db.conn, "custom_none").await.unwrap(), None);
    let row = connector_definition::Entity::find_by_id(def.id.clone())
        .one(&db.conn)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.zcd_version, 1);
    assert_eq!(row.trust, "user");
    assert_eq!(row.runtime_kind, "mcp_http");
    assert_eq!(row.hash, def.hash());
}

#[tokio::test]
async fn saving_again_replaces_the_record_and_keeps_its_creation_time() {
    let db = fresh_in_memory_db().await;
    let mut def = user_definition("https://mcp.linear.app/mcp");
    save_definition(&db.conn, &def).await.unwrap();
    let created = connector_definition::Entity::find_by_id(def.id.clone())
        .one(&db.conn)
        .await
        .unwrap()
        .unwrap()
        .created_at;

    def.display.name = "Linear (work)".into();
    def.runtime = Runtime::McpHttp {
        url: "https://mcp.linear.app/v2/mcp".into(),
        protocol: None,
    };
    save_definition(&db.conn, &def).await.unwrap();
    let row = connector_definition::Entity::find_by_id(def.id.clone())
        .one(&db.conn)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.created_at, created);
    assert_eq!(row.hash, def.hash());
    assert_eq!(definition(&db.conn, &def.id).await.unwrap(), Some(def));
}

#[tokio::test]
async fn an_invalid_definition_is_never_written() {
    let db = fresh_in_memory_db().await;
    let mut def = user_definition("https://mcp.linear.app/mcp");
    def.runtime = Runtime::McpStdio {
        rest: serde_json::Map::new(),
    };
    let err = save_definition(&db.conn, &def).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Definition(e) if e.code() == "stdio_requires_sandbox"),
        "{err:?}"
    );
    assert_eq!(definition(&db.conn, &def.id).await.unwrap(), None);
}

/// A record that no longer passes the rules (written by hand, or before a
/// rule existed) is refused when read, not used.
#[tokio::test]
async fn a_stored_record_that_fails_validation_is_refused_on_read() {
    let db = fresh_in_memory_db().await;
    let def = user_definition("https://mcp.linear.app/mcp");
    save_definition(&db.conn, &def).await.unwrap();
    let mut json = serde_json::to_value(&def).unwrap();
    json["runtime"] = serde_json::json!({ "kind": "mcp_stdio", "args": [] });
    let row = connector_definition::ActiveModel {
        id: Set(def.id.clone()),
        definition_json: Set(json.to_string()),
        ..Default::default()
    };
    row.update(&db.conn).await.unwrap();

    let err = definition(&db.conn, &def.id).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Definition(e) if e.code() == "stdio_requires_sandbox"),
        "{err:?}"
    );
}
