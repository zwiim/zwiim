use serde_json::{json, Map, Value};

use super::{
    dispatch, execution, executions, finish, receive, sweep, ExecutionState, NewCall, Outcome,
    RejectReason, Timings,
};
use crate::connectors::definition::{from_user, UserAuth, UserConnector};
use crate::connectors::egress::NetworkClass;
use crate::connectors::store::{create_connection, save_definition, CredentialStore};
use crate::connectors::upstream::UnknownCause;
use crate::db::test_helpers::fresh_in_memory_db;
use crate::db::AppDatabase;

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
    (db, id)
}

fn args(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => unreachable!(),
    }
}

fn call<'a>(connection_id: &'a str, arguments: &'a Map<String, Value>) -> NewCall<'a> {
    NewCall {
        connection_id,
        session_id: None,
        tool_name: "create_issue",
        schema_hash: Some("sha256:0"),
        arguments,
    }
}

const TIMED: Timings = Timings {
    upstream_ms: Some(40),
    overhead_ms: Some(3),
};

#[tokio::test]
async fn a_call_keeps_a_digest_of_its_arguments_never_the_arguments() {
    let (db, id) = connected().await;
    let secret = args(json!({ "title": "Rotate the key sk-live-123", "team": "ENG" }));
    let received = receive(&db.conn, call(&id, &secret)).await.unwrap();
    assert_eq!(received.state, ExecutionState::Received);
    assert_eq!(received.arg_keys, ["team", "title"]);
    assert!(received.args_digest.starts_with("sha256:"));
    assert!(received.dispatched_at.is_none());

    let reordered = args(json!({ "team": "ENG", "title": "Rotate the key sk-live-123" }));
    let again = receive(&db.conn, call(&id, &reordered)).await.unwrap();
    assert_eq!(
        again.args_digest, received.args_digest,
        "key order doesn't matter"
    );

    let stored = format!("{:?}", executions(&db.conn, &id, 10, None).await.unwrap());
    assert!(
        !stored.contains("sk-live-123") && !stored.contains("ENG"),
        "{stored}"
    );
}

#[tokio::test]
async fn a_sent_call_ends_with_its_outcome_and_timings() {
    let (db, id) = connected().await;
    let none = Map::new();
    let e = receive(&db.conn, call(&id, &none)).await.unwrap();
    assert!(dispatch(&db.conn, &e.id).await.unwrap());
    assert!(!dispatch(&db.conn, &e.id).await.unwrap(), "dispatched once");
    let done = finish(&db.conn, &e.id, Outcome::Succeeded, None, TIMED)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.state, ExecutionState::Succeeded);
    assert!(done.dispatched_at.is_some() && done.finished_at.is_some());
    assert_eq!((done.upstream_ms, done.overhead_ms), (Some(40), Some(3)));

    let late = finish(
        &db.conn,
        &e.id,
        Outcome::Unknown(UnknownCause::CancelledAfterDispatch),
        None,
        Timings::default(),
    )
    .await
    .unwrap();
    assert!(late.is_none(), "an ended call keeps its outcome");
    let kept = execution(&db.conn, &e.id).await.unwrap().unwrap();
    assert_eq!(kept.state, ExecutionState::Succeeded);
}

#[tokio::test]
async fn only_a_dispatched_call_can_have_run() {
    let (db, id) = connected().await;
    let none = Map::new();
    let unsent = receive(&db.conn, call(&id, &none)).await.unwrap();
    assert!(
        finish(&db.conn, &unsent.id, Outcome::Succeeded, None, TIMED)
            .await
            .unwrap()
            .is_none(),
        "a call never sent has no answer"
    );
    let rejected = finish(
        &db.conn,
        &unsent.id,
        Outcome::Rejected(RejectReason::Unavailable),
        Some("The linear connection is revoking; nothing was sent."),
        TIMED,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(rejected.state, ExecutionState::Rejected);
    assert_eq!(rejected.reason.as_deref(), Some("unavailable"));
    assert!(rejected.detail.unwrap().contains("revoking"));

    // The adapter marks a call before it can prove nothing left the Host.
    let refused = receive(&db.conn, call(&id, &none)).await.unwrap();
    dispatch(&db.conn, &refused.id).await.unwrap();
    let not_sent = finish(
        &db.conn,
        &refused.id,
        Outcome::Rejected(RejectReason::NotSent),
        None,
        TIMED,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(not_sent.dispatched_at.is_none(), "proven not sent");

    let lost = receive(&db.conn, call(&id, &none)).await.unwrap();
    dispatch(&db.conn, &lost.id).await.unwrap();
    let unknown = finish(
        &db.conn,
        &lost.id,
        Outcome::Unknown(UnknownCause::Timeout),
        None,
        TIMED,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(unknown.state, ExecutionState::OutcomeUnknown);
    assert_eq!(unknown.reason.as_deref(), Some("timeout"));
    assert!(unknown.dispatched_at.is_some());
}

#[tokio::test]
async fn boot_ends_what_the_last_host_left_open() {
    let (db, id) = connected().await;
    let none = Map::new();
    let waiting = receive(&db.conn, call(&id, &none)).await.unwrap();
    let running = receive(&db.conn, call(&id, &none)).await.unwrap();
    dispatch(&db.conn, &running.id).await.unwrap();
    let done = receive(&db.conn, call(&id, &none)).await.unwrap();
    dispatch(&db.conn, &done.id).await.unwrap();
    finish(&db.conn, &done.id, Outcome::ToolError, None, TIMED)
        .await
        .unwrap();

    assert_eq!(sweep(&db.conn).await.unwrap(), 2);
    let state = |e: &str| {
        let conn = db.conn.clone();
        let e = e.to_string();
        async move { execution(&conn, &e).await.unwrap().unwrap() }
    };
    let waiting = state(&waiting.id).await;
    assert_eq!(waiting.state, ExecutionState::CancelledBeforeDispatch);
    let running = state(&running.id).await;
    assert_eq!(running.state, ExecutionState::OutcomeUnknown);
    assert_eq!(running.reason.as_deref(), Some("host_shutdown"));
    assert_eq!(state(&done.id).await.state, ExecutionState::ToolError);
    assert_eq!(sweep(&db.conn).await.unwrap(), 0);
}

#[tokio::test]
async fn calls_are_listed_newest_first_a_page_at_a_time() {
    let (db, id) = connected().await;
    let none = Map::new();
    let mut made = Vec::new();
    for _ in 0..5 {
        made.push(receive(&db.conn, call(&id, &none)).await.unwrap().id);
    }
    let first = executions(&db.conn, &id, 2, None).await.unwrap();
    let ids = |page: &[super::Execution]| page.iter().map(|e| e.id.clone()).collect::<Vec<_>>();
    let mut seen = ids(&first);
    let second = executions(&db.conn, &id, 2, Some(&first[1].id))
        .await
        .unwrap();
    seen.extend(ids(&second));
    let third = executions(&db.conn, &id, 2, Some(&second[1].id))
        .await
        .unwrap();
    seen.extend(ids(&third));
    assert_eq!(third.len(), 1);
    // Calls in the same instant fall back to id order; compare as sets then.
    let mut sorted_seen = seen.clone();
    sorted_seen.sort();
    let mut sorted_made = made.clone();
    sorted_made.sort();
    assert_eq!(sorted_seen, sorted_made, "every call once");
    assert!(first[0].received_at >= third[0].received_at);
    assert!(executions(&db.conn, "nope", 10, None)
        .await
        .unwrap()
        .is_empty());
}
