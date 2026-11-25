use chrono::{Duration, Utc};
use serde_json::{json, Map, Value};

use super::{
    answer, approval, args_hash, binding_hash, consume, create, end, end_for_connection,
    expire_due, sweep, Approval, ApprovalError, ApprovalStatus, NewApproval, Risk, VoidReason,
};
use crate::connectors::definition::{from_user, UserAuth, UserConnector};
use crate::connectors::egress::NetworkClass;
use crate::connectors::executions::{receive, NewCall};
use crate::connectors::store::{create_connection, save_definition, CredentialStore};
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
    let def = from_user(input, Utc::now()).unwrap();
    save_definition(&db.conn, &def).await.unwrap();
    let id = create_connection(&db.conn, &def.id, "Work", CredentialStore::Keychain)
        .await
        .unwrap()
        .id;
    (db, id)
}

fn args() -> Map<String, Value> {
    json!({ "title": "Ship it" }).as_object().unwrap().clone()
}

/// A pending approval for a fresh call, made at `now`.
async fn asked(
    db: &AppDatabase,
    connection: &str,
    risk: Risk,
    now: chrono::DateTime<Utc>,
) -> Approval {
    let arguments = args();
    let execution = receive(
        &db.conn,
        NewCall {
            connection_id: connection,
            session_id: Some("s1"),
            tool_name: "create_issue",
            schema_hash: Some("sha256:tool"),
            arguments: &arguments,
        },
    )
    .await
    .unwrap();
    create(
        &db.conn,
        NewApproval {
            execution_id: &execution.id,
            session_id: "s1",
            connection_id: connection,
            tool_name: "create_issue",
            schema_hash: "sha256:tool",
            arguments: &arguments,
            risk,
            preview: r#"{"title":"Ship it"}"#,
        },
        now,
    )
    .await
    .unwrap()
}

#[test]
fn only_an_explicit_hint_sets_the_risk_and_destructive_expires_sooner() {
    assert_eq!(Risk::of(None), Risk::Unknown);
    assert_eq!(Risk::of(Some(&json!({}))), Risk::Unknown);
    assert_eq!(Risk::of(Some(&json!({ "readOnlyHint": true }))), Risk::Read);
    assert_eq!(
        Risk::of(Some(&json!({ "destructiveHint": true }))),
        Risk::Destructive
    );
    assert_eq!(
        Risk::of(Some(
            &json!({ "readOnlyHint": false, "destructiveHint": false })
        )),
        Risk::Write
    );
    assert_eq!(Risk::Destructive.expiry(), Duration::minutes(5));
    assert_eq!(Risk::Unknown.expiry(), Duration::minutes(10));
}

#[tokio::test]
async fn an_approval_is_bound_to_its_call_and_expires_when_made() {
    let (db, id) = connected().await;
    let now = Utc::now();
    let a = asked(&db, &id, Risk::Destructive, now).await;
    assert_eq!(a.status, ApprovalStatus::Pending);
    assert_eq!(a.expires_at, now + Duration::minutes(5));
    assert_eq!(a.args_hash, args_hash(&args()));
    assert_eq!(
        a.binding_hash,
        binding_hash(
            &a.execution_id,
            &id,
            "create_issue",
            "sha256:tool",
            &a.args_hash
        )
    );
    let other = binding_hash("another", &id, "create_issue", "sha256:tool", &a.args_hash);
    assert_ne!(a.binding_hash, other);
    assert!(matches!(
        answer(&db.conn, &a.id, &other, true, "host", now).await,
        Err(ApprovalError::Mismatch)
    ));
    assert!(matches!(
        answer(&db.conn, "nope", &a.binding_hash, true, "host", now).await,
        Err(ApprovalError::NotFound(_))
    ));
}

#[tokio::test]
async fn the_first_answer_wins_and_an_approval_is_used_once() {
    let (db, id) = connected().await;
    let now = Utc::now();
    let a = asked(&db, &id, Risk::Write, now).await;
    let approved = answer(&db.conn, &a.id, &a.binding_hash, true, "device:phone", now)
        .await
        .unwrap();
    assert_eq!(approved.status, ApprovalStatus::Approved);
    assert_eq!(approved.decided_by.as_deref(), Some("device:phone"));
    assert!(approved.preview.is_some(), "kept until it's used");

    match answer(&db.conn, &a.id, &a.binding_hash, false, "host", now).await {
        Err(ApprovalError::AlreadyResolved { status, decided_by }) => {
            assert_eq!(status, ApprovalStatus::Approved);
            assert_eq!(decided_by.as_deref(), Some("device:phone"));
        }
        other => panic!("{other:?}"),
    }

    assert!(consume(&db.conn, &a.id, now).await.unwrap());
    assert!(!consume(&db.conn, &a.id, now).await.unwrap(), "used once");
    let used = approval(&db.conn, &a.id).await.unwrap().unwrap();
    assert_eq!(used.status, ApprovalStatus::Consumed);
    assert!(used.consumed_at.is_some() && used.preview.is_none());

    let denied = asked(&db, &id, Risk::Write, now).await;
    let denied = answer(
        &db.conn,
        &denied.id,
        &denied.binding_hash,
        false,
        "host",
        now,
    )
    .await
    .unwrap();
    assert_eq!(denied.status, ApprovalStatus::Denied);
    assert!(!consume(&db.conn, &denied.id, now).await.unwrap());
}

#[tokio::test]
async fn an_overdue_approval_takes_no_answer() {
    let (db, id) = connected().await;
    let then = Utc::now() - Duration::minutes(11);
    let a = asked(&db, &id, Risk::Unknown, then).await;
    assert!(matches!(
        answer(&db.conn, &a.id, &a.binding_hash, true, "host", Utc::now()).await,
        Err(ApprovalError::Expired)
    ));
    let expired = approval(&db.conn, &a.id).await.unwrap().unwrap();
    assert_eq!(expired.status, ApprovalStatus::Expired);
    assert!(expired.preview.is_none());

    let late = asked(&db, &id, Risk::Unknown, then).await;
    let fresh = asked(&db, &id, Risk::Unknown, Utc::now()).await;
    let ended = expire_due(&db.conn, Utc::now()).await.unwrap();
    assert_eq!(
        ended.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        [late.id.as_str()]
    );
    let fresh = approval(&db.conn, &fresh.id).await.unwrap().unwrap();
    assert_eq!(fresh.status, ApprovalStatus::Pending);
}

#[tokio::test]
async fn revocation_cancels_what_waits_and_voids_what_was_approved() {
    let (db, id) = connected().await;
    let now = Utc::now();
    let waiting = asked(&db, &id, Risk::Write, now).await;
    let approved = asked(&db, &id, Risk::Write, now).await;
    answer(
        &db.conn,
        &approved.id,
        &approved.binding_hash,
        true,
        "host",
        now,
    )
    .await
    .unwrap();

    let mut ended = end_for_connection(&db.conn, &id, now).await.unwrap();
    ended.sort_by_key(|a| a.status as u8);
    let states: Vec<_> = ended.iter().map(|a| (a.status, a.void_reason)).collect();
    assert!(states.contains(&(ApprovalStatus::Cancelled, None)));
    assert!(states.contains(&(ApprovalStatus::Void, Some(VoidReason::Revoked))));
    assert!(!consume(&db.conn, &approved.id, now).await.unwrap());
    assert!(matches!(
        answer(
            &db.conn,
            &waiting.id,
            &waiting.binding_hash,
            true,
            "host",
            now
        )
        .await,
        Err(ApprovalError::AlreadyResolved {
            status: ApprovalStatus::Cancelled,
            ..
        })
    ));
}

#[tokio::test]
async fn boot_leaves_nothing_usable_and_an_ended_approval_stays_ended() {
    let (db, id) = connected().await;
    let now = Utc::now();
    let waiting = asked(&db, &id, Risk::Write, now).await;
    let approved = asked(&db, &id, Risk::Write, now).await;
    answer(
        &db.conn,
        &approved.id,
        &approved.binding_hash,
        true,
        "host",
        now,
    )
    .await
    .unwrap();
    let used = asked(&db, &id, Risk::Write, now).await;
    answer(&db.conn, &used.id, &used.binding_hash, true, "host", now)
        .await
        .unwrap();
    consume(&db.conn, &used.id, now).await.unwrap();

    assert_eq!(sweep(&db.conn, now).await.unwrap(), 2);
    let status = |a: &Approval| {
        let (conn, id) = (db.conn.clone(), a.id.clone());
        async move { approval(&conn, &id).await.unwrap().unwrap() }
    };
    assert_eq!(status(&waiting).await.status, ApprovalStatus::Expired);
    let voided = status(&approved).await;
    assert_eq!(
        (voided.status, voided.void_reason),
        (ApprovalStatus::Void, Some(VoidReason::HostRestart))
    );
    assert_eq!(status(&used).await.status, ApprovalStatus::Consumed);
    assert!(end(
        &db.conn,
        &used.id,
        ApprovalStatus::Void,
        Some(VoidReason::ToolChanged),
        now
    )
    .await
    .unwrap()
    .is_none());
}
