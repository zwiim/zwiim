use std::path::Path;

use chrono::Utc;

use super::{ConnectorSessions, SessionCards};
use crate::acp::delegation::listener::ConnectorSessionLookup;
use crate::acp::manager::ConnectionManager;
use crate::connectors::approvals::Risk;
use crate::connectors::runtime::{ApprovalCard, ApprovalCards};
use crate::models::agent::AgentType;
use crate::web::event_bridge::EventEmitter;

fn card(id: &str) -> ApprovalCard {
    ApprovalCard {
        approval_id: id.into(),
        binding_hash: format!("sha256:{id}"),
        label: "Work".into(),
        slug: "linear_work".into(),
        tool: "create_issue".into(),
        tool_title: Some("Create issue".into()),
        risk: Risk::Write,
        preview: r#"{"title":"Ship it"}"#.into(),
        expires_at: Utc::now(),
    }
}

async fn manager() -> ConnectionManager {
    let manager = ConnectionManager::new();
    manager
        .insert_test_connection("c1", AgentType::Codex, None, EventEmitter::Noop)
        .await;
    manager
}

/// What a client attaching now is shown.
async fn shown(manager: &ConnectionManager) -> serde_json::Value {
    let snapshot = manager
        .get_state("c1")
        .await
        .unwrap()
        .read()
        .await
        .to_snapshot();
    crate::protocol::events::project_snapshot(&snapshot, None)["pending_request"].clone()
}

#[tokio::test]
async fn a_session_shows_one_card_at_a_time_oldest_first() {
    let manager = manager().await;
    let cards = SessionCards {
        manager: manager.clone_ref(),
        connection_id: "c1".into(),
    };
    assert!(cards.show(card("a1")).await);
    assert!(cards.show(card("a2")).await);
    let head = shown(&manager).await;
    assert_eq!(head["kind"], "connector_approval");
    assert_eq!(head["request_id"], "a1");
    assert_eq!(head["binding_hash"], "sha256:a1");
    assert_eq!(head["tool"]["title"], "Create issue");
    assert_eq!(head["connection"]["slug"], "linear_work");
    assert_eq!(head["risk"], "write");

    cards.clear("a1").await;
    assert_eq!(
        shown(&manager).await["request_id"],
        "a2",
        "the next one shows"
    );
    cards.clear("a1").await;
    cards.clear("a2").await;
    assert!(shown(&manager).await.is_null());
    let state = manager.get_state("c1").await.unwrap();
    assert!(!crate::session::is_busy(&*state.read().await));
}

#[tokio::test]
async fn a_queued_card_can_come_down_before_its_turn() {
    let manager = manager().await;
    let cards = SessionCards {
        manager: manager.clone_ref(),
        connection_id: "c1".into(),
    };
    cards.show(card("a1")).await;
    cards.show(card("a2")).await;
    cards.clear("a2").await;
    assert_eq!(shown(&manager).await["request_id"], "a1");
    let state = manager.get_state("c1").await.unwrap();
    assert!(
        crate::session::is_busy(&*state.read().await),
        "waits on the user"
    );
    cards.clear("a1").await;
    assert!(shown(&manager).await.is_null());
}

#[tokio::test]
async fn a_gone_session_shows_nothing() {
    let manager = ConnectionManager::new();
    let cards = SessionCards {
        manager: manager.clone_ref(),
        connection_id: "gone".into(),
    };
    assert!(!cards.show(card("a1")).await);
}

#[tokio::test]
async fn a_calls_session_comes_from_the_hosts_own_state() {
    let db = crate::db::test_helpers::fresh_in_memory_db().await;
    let manager = manager().await;
    let sessions = ConnectorSessions::new(db.conn.clone(), manager.clone_ref());
    let session = sessions
        .session("c1", Path::new("/nowhere"), true)
        .await
        .expect("a live session");
    assert_eq!(session.session_id, "c1");
    assert_eq!(session.agent, "codex");
    assert_eq!(session.workspace_id, None, "outside every workspace");
    assert!(session.engine);
    assert!(sessions
        .session("gone", Path::new("/nowhere"), false)
        .await
        .is_none());
}
