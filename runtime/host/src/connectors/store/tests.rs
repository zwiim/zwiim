use sea_orm::{ActiveModelTrait, EntityTrait, Set};

use super::{
    connection, connections, create_connection, definition, endpoint_rules, save_definition,
    set_status, ConnectionStatus, CredentialStore, StoreError,
};
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

async fn stored(db: &crate::db::AppDatabase, name: &str) -> String {
    let mut def = user_definition("https://mcp.linear.app/mcp");
    def.display.name = name.into();
    save_definition(&db.conn, &def).await.unwrap();
    def.id
}

#[tokio::test]
async fn a_connection_starts_validating_with_its_definitions_endpoints() {
    let db = fresh_in_memory_db().await;
    let def_id = stored(&db, "Linear").await;
    let def = definition(&db.conn, &def_id).await.unwrap().unwrap();

    let made = create_connection(&db.conn, &def_id, " Work ", CredentialStore::Keychain)
        .await
        .unwrap();
    assert_eq!(made.label, "Work");
    assert_eq!(made.slug, "linear_work");
    assert_eq!(made.status, ConnectionStatus::Validating);
    assert_eq!(made.store, CredentialStore::Keychain);
    assert_eq!(made.definition_hash, def.hash());
    assert_eq!(
        connection(&db.conn, &made.id).await.unwrap(),
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

    let work = create_connection(&db.conn, &linear, "Work", keychain)
        .await
        .unwrap();
    let err = create_connection(&db.conn, &linear, "Work", keychain)
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::LabelTaken(_)), "{err:?}");
    let home = create_connection(&db.conn, &linear, "Home", keychain)
        .await
        .unwrap();
    assert_eq!(home.slug, "linear_home");
    // Another definition with the same name: same label, its own slug.
    let again = create_connection(&db.conn, &other, "Work", keychain)
        .await
        .unwrap();
    assert_eq!(again.slug, "linear_work_2");

    let all: Vec<String> = connections(&db.conn)
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
    let made = create_connection(&db.conn, &def, "My work account", CredentialStore::File)
        .await
        .unwrap();
    assert_eq!(made.slug, "n_code_tools_github_enterprise");
    assert!(made.slug.len() <= 32);
    assert_eq!(made.store, CredentialStore::File);

    let symbols = stored(&db, "✨").await;
    let made = create_connection(&db.conn, &symbols, "✨", CredentialStore::Keychain)
        .await
        .unwrap();
    assert_eq!(made.slug, "connector");
}

#[tokio::test]
async fn a_connection_needs_a_stored_definition_and_a_label() {
    let db = fresh_in_memory_db().await;
    let err = create_connection(&db.conn, "custom_none", "Work", CredentialStore::Keychain)
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::DefinitionNotFound(_)), "{err:?}");
    let def = stored(&db, "Linear").await;
    for label in ["", "   ", &"x".repeat(41)] {
        let err = create_connection(&db.conn, &def, label, CredentialStore::Keychain)
            .await
            .unwrap_err();
        assert!(
            matches!(err, StoreError::InvalidLabel),
            "{label:?}: {err:?}"
        );
    }
    assert!(connections(&db.conn).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_revoked_connection_never_becomes_usable_again() {
    let db = fresh_in_memory_db().await;
    let def = stored(&db, "Linear").await;
    let made = create_connection(&db.conn, &def, "Work", CredentialStore::Keychain)
        .await
        .unwrap();

    let failed = set_status(&db.conn, &made.id, ConnectionStatus::Error, Some("401"))
        .await
        .unwrap();
    assert_eq!(failed.last_error.as_deref(), Some("401"));
    assert_eq!(failed.last_validated_at, None);
    let active = set_status(&db.conn, &made.id, ConnectionStatus::Active, None)
        .await
        .unwrap();
    assert_eq!(active.last_error, None);
    assert!(active.last_validated_at.is_some());

    set_status(&db.conn, &made.id, ConnectionStatus::Revoking, None)
        .await
        .unwrap();
    for status in [ConnectionStatus::Active, ConnectionStatus::Validating] {
        let err = set_status(&db.conn, &made.id, status, None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, StoreError::InvalidTransition { .. }),
            "{err:?}"
        );
    }
    set_status(&db.conn, &made.id, ConnectionStatus::Revoked, None)
        .await
        .unwrap();
    let err = set_status(&db.conn, &made.id, ConnectionStatus::Active, None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, StoreError::InvalidTransition { .. }),
        "{err:?}"
    );
    let err = set_status(&db.conn, "missing", ConnectionStatus::Active, None)
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::ConnectionNotFound(_)), "{err:?}");
}

/// The connection keeps the hash the user confirmed, even after its
/// definition changes, so the change can be noticed.
#[tokio::test]
async fn a_connection_keeps_the_hash_it_was_confirmed_with() {
    let db = fresh_in_memory_db().await;
    let def_id = stored(&db, "Linear").await;
    let made = create_connection(&db.conn, &def_id, "Work", CredentialStore::Keychain)
        .await
        .unwrap();
    let mut def = definition(&db.conn, &def_id).await.unwrap().unwrap();
    def.runtime = Runtime::McpHttp {
        url: "https://mcp.linear.app/v2/mcp".into(),
        protocol: None,
    };
    save_definition(&db.conn, &def).await.unwrap();
    let kept = connection(&db.conn, &made.id).await.unwrap().unwrap();
    assert_eq!(kept.definition_hash, made.definition_hash);
    assert_ne!(kept.definition_hash, def.hash());
}
