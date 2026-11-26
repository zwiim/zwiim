use sea_orm::{ActiveModelTrait, EntityTrait, Set};

use super::{
    connector, connectors, create_connector, definition, endpoint_rules, save_definition,
    save_tool_snapshot, set_status, tools, ConnectorStatus, CredentialStore, SnapshotChanges,
    StoreError,
};
use crate::connectors::definition::{from_user, Runtime, UserAuth, UserConnector};
use crate::connectors::egress::NetworkClass;
use crate::connectors::upstream::ToolDefinition;
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

async fn stored(db: &crate::db::AppDatabase, name: &str) -> String {
    let mut def = user_definition("https://mcp.linear.app/mcp");
    def.display.name = name.into();
    save_definition(&db.conn, &def).await.unwrap();
    def.id
}

#[tokio::test]
async fn a_connector_starts_validating_with_its_definitions_endpoints() {
    let db = fresh_in_memory_db().await;
    let def_id = stored(&db, "Linear").await;
    let def = definition(&db.conn, &def_id).await.unwrap().unwrap();

    let made = create_connector(&db.conn, &def_id, " Work ", CredentialStore::Keychain)
        .await
        .unwrap();
    assert_eq!(made.label, "Work");
    assert_eq!(made.slug, "linear_work");
    assert_eq!(made.status, ConnectorStatus::Validating);
    assert_eq!(made.store, CredentialStore::Keychain);
    assert_eq!(made.definition_hash, def.hash());
    assert_eq!(
        connector(&db.conn, &made.id).await.unwrap(),
        Some(made.clone())
    );

    let rules = endpoint_rules(&db.conn, &made.id).await.unwrap();
    assert_eq!(rules, def.endpoint_rules().unwrap());
    assert!(rules[0].matches(&url::Url::parse("https://mcp.linear.app/mcp").unwrap()));
}

#[tokio::test]
async fn labels_are_unique_per_definition_and_slugs_unique_everywhere() {
    let db = fresh_in_memory_db().await;
    let linear = stored(&db, "Linear").await;
    let other = stored(&db, "Linear").await;
    let keychain = CredentialStore::Keychain;

    let work = create_connector(&db.conn, &linear, "Work", keychain)
        .await
        .unwrap();
    let err = create_connector(&db.conn, &linear, "Work", keychain)
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::LabelTaken(_)), "{err:?}");
    let home = create_connector(&db.conn, &linear, "Home", keychain)
        .await
        .unwrap();
    assert_eq!(home.slug, "linear_home");
    // Another definition with the same name: same label, its own slug.
    let again = create_connector(&db.conn, &other, "Work", keychain)
        .await
        .unwrap();
    assert_eq!(again.slug, "linear_work_2");

    let all: Vec<String> = connectors(&db.conn)
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(all, [work.id, home.id, again.id]);
}

#[tokio::test]
async fn slugs_are_short_plain_names() {
    let db = fresh_in_memory_db().await;
    let def = stored(&db, "Ünïcode ✨ Tools: GitHub Enterprise Server").await;
    let made = create_connector(&db.conn, &def, "My work account", CredentialStore::File)
        .await
        .unwrap();
    assert_eq!(made.slug, "n_code_tools_github_enterprise");
    assert!(made.slug.len() <= 32);
    assert_eq!(made.store, CredentialStore::File);

    let symbols = stored(&db, "✨").await;
    let made = create_connector(&db.conn, &symbols, "✨", CredentialStore::Keychain)
        .await
        .unwrap();
    assert_eq!(made.slug, "connector");
}

#[tokio::test]
async fn a_connector_needs_a_stored_definition_and_a_label() {
    let db = fresh_in_memory_db().await;
    let err = create_connector(&db.conn, "custom_none", "Work", CredentialStore::Keychain)
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::DefinitionNotFound(_)), "{err:?}");
    let def = stored(&db, "Linear").await;
    for label in ["", "   ", &"x".repeat(41)] {
        let err = create_connector(&db.conn, &def, label, CredentialStore::Keychain)
            .await
            .unwrap_err();
        assert!(
            matches!(err, StoreError::InvalidLabel),
            "{label:?}: {err:?}"
        );
    }
    assert!(connectors(&db.conn).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_revoked_connector_never_becomes_usable_again() {
    let db = fresh_in_memory_db().await;
    let def = stored(&db, "Linear").await;
    let made = create_connector(&db.conn, &def, "Work", CredentialStore::Keychain)
        .await
        .unwrap();

    let failed = set_status(&db.conn, &made.id, ConnectorStatus::Error, Some("401"))
        .await
        .unwrap();
    assert_eq!(failed.last_error.as_deref(), Some("401"));
    assert_eq!(failed.last_validated_at, None);
    let active = set_status(&db.conn, &made.id, ConnectorStatus::Active, None)
        .await
        .unwrap();
    assert_eq!(active.last_error, None);
    assert!(active.last_validated_at.is_some());

    set_status(&db.conn, &made.id, ConnectorStatus::Revoking, None)
        .await
        .unwrap();
    for status in [ConnectorStatus::Active, ConnectorStatus::Validating] {
        let err = set_status(&db.conn, &made.id, status, None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, StoreError::InvalidTransition { .. }),
            "{err:?}"
        );
    }
    set_status(&db.conn, &made.id, ConnectorStatus::Revoked, None)
        .await
        .unwrap();
    let err = set_status(&db.conn, &made.id, ConnectorStatus::Active, None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, StoreError::InvalidTransition { .. }),
        "{err:?}"
    );
    let err = set_status(&db.conn, "missing", ConnectorStatus::Active, None)
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::ConnectorNotFound(_)), "{err:?}");
}

/// The connector keeps the hash the user confirmed, even after its
/// definition changes, so the change can be noticed.
#[tokio::test]
async fn a_connector_keeps_the_hash_it_was_confirmed_with() {
    let db = fresh_in_memory_db().await;
    let def_id = stored(&db, "Linear").await;
    let made = create_connector(&db.conn, &def_id, "Work", CredentialStore::Keychain)
        .await
        .unwrap();
    let mut def = definition(&db.conn, &def_id).await.unwrap().unwrap();
    def.runtime = Runtime::McpHttp {
        url: "https://mcp.linear.app/v2/mcp".into(),
        protocol: None,
    };
    save_definition(&db.conn, &def).await.unwrap();
    let kept = connector(&db.conn, &made.id).await.unwrap().unwrap();
    assert_eq!(kept.definition_hash, made.definition_hash);
    assert_ne!(kept.definition_hash, def.hash());
}

fn tool(name: &str, description: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        title: None,
        description: Some(description.into()),
        input_schema: serde_json::json!({ "type": "object" }),
        output_schema: Some(serde_json::json!({ "type": "object" })),
        annotations: Some(serde_json::json!({ "readOnlyHint": true })),
    }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

async fn connected(db: &crate::db::AppDatabase) -> String {
    let def = stored(db, "Linear").await;
    create_connector(&db.conn, &def, "Work", CredentialStore::Keychain)
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn a_snapshot_reports_what_each_listing_changed() {
    let db = fresh_in_memory_db().await;
    let id = connected(&db).await;

    let first = save_tool_snapshot(&db.conn, &id, &[tool("a", "A"), tool("b", "B")])
        .await
        .unwrap();
    assert_eq!(first.added, names(&["a", "b"]));
    assert!(first.changed.is_empty() && first.removed.is_empty());
    assert_eq!(
        tools(&db.conn, &id).await.unwrap(),
        [tool("a", "A"), tool("b", "B")]
    );

    let same = save_tool_snapshot(&db.conn, &id, &[tool("b", "B"), tool("a", "A")])
        .await
        .unwrap();
    assert!(same.is_empty(), "{same:?}");

    let next = save_tool_snapshot(&db.conn, &id, &[tool("a", "Deletes A"), tool("c", "C")])
        .await
        .unwrap();
    assert_eq!(
        next,
        SnapshotChanges {
            added: names(&["c"]),
            changed: names(&["a"]),
            removed: names(&["b"]),
        }
    );
    assert_eq!(
        tools(&db.conn, &id).await.unwrap(),
        [tool("a", "Deletes A"), tool("c", "C")]
    );

    // A removed tool that comes back is new again, whatever its hash.
    let back = save_tool_snapshot(&db.conn, &id, &[tool("a", "Deletes A"), tool("b", "B2")])
        .await
        .unwrap();
    assert_eq!(back.added, names(&["b"]));
    assert_eq!(back.removed, names(&["c"]));
    assert!(back.changed.is_empty());
}

#[tokio::test]
async fn snapshots_are_per_connector_and_go_with_it() {
    let db = fresh_in_memory_db().await;
    let one = connected(&db).await;
    let def = stored(&db, "Other").await;
    let two = create_connector(&db.conn, &def, "Work", CredentialStore::Keychain)
        .await
        .unwrap()
        .id;
    save_tool_snapshot(&db.conn, &one, &[tool("a", "A")])
        .await
        .unwrap();
    save_tool_snapshot(&db.conn, &two, &[tool("a", "Other A")])
        .await
        .unwrap();
    assert_eq!(tools(&db.conn, &one).await.unwrap(), [tool("a", "A")]);

    crate::db::entities::connector::Entity::delete_by_id(one.clone())
        .exec(&db.conn)
        .await
        .unwrap();
    assert!(tools(&db.conn, &one).await.unwrap().is_empty());
    assert_eq!(tools(&db.conn, &two).await.unwrap(), [tool("a", "Other A")]);
}
