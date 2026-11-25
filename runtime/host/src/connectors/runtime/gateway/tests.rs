//! The gateway against the fake MCP server over real HTTP.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Map, Value};

use super::super::tests::{service, setup, Setup};
use super::super::{
    AnswerError, ApprovalAnswer, ApprovalCard, ApprovalCards, CallSession, NoCards,
};
use crate::connectors::approvals::{self, ApprovalStatus};
use crate::connectors::executions::{executions, Execution, ExecutionState};
use crate::connectors::fake_mcp::{tool, CallBehaviour, Era, FakeMcp};
use crate::connectors::grants::{self, Fallback, Scope, ToolPolicy};
use crate::connectors::upstream::CallOutput;

/// A session in no workspace, run by Codex, that no one can answer for.
fn anyone() -> CallSession {
    session(Arc::new(NoCards))
}

fn session(cards: Arc<dyn ApprovalCards>) -> CallSession {
    CallSession {
        session_id: "s1".into(),
        workspace_id: None,
        agent: "codex".into(),
        engine: false,
        cards,
    }
}

/// Every session may use the connection; tools with no row ask.
async fn granted(setup: &Setup) -> i32 {
    grants::set_grant(
        &setup.db.conn,
        &setup.connection,
        Scope::default(),
        Fallback::Ask,
        Fallback::Ask,
    )
    .await
    .unwrap()
    .id
}

/// Cards as a client would see them. `on_clear` runs as a card comes down,
/// after its answer and before the call goes on.
#[derive(Default)]
struct Cards {
    shown: Mutex<Vec<ApprovalCard>>,
    cleared: Mutex<Vec<String>>,
    on_clear: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

#[async_trait]
impl ApprovalCards for Cards {
    async fn show(&self, card: ApprovalCard) -> bool {
        self.shown.lock().unwrap().push(card);
        true
    }

    async fn clear(&self, approval_id: &str) {
        self.cleared.lock().unwrap().push(approval_id.into());
        let hook = self.on_clear.lock().unwrap().take();
        if let Some(hook) = hook {
            let _ = hook.send(());
            // Let the hook's side finish before the call goes on.
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

impl Cards {
    /// The `n`th card shown, once it is.
    async fn nth(&self, n: usize) -> ApprovalCard {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(card) = self.shown.lock().unwrap().get(n).cloned() {
                    return card;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a card was shown")
    }

    fn count(&self) -> usize {
        self.shown.lock().unwrap().len()
    }
}

fn args(text: &str) -> Map<String, Value> {
    json!({ "text": text }).as_object().unwrap().clone()
}

fn text(output: &CallOutput) -> &str {
    output.content[0]["text"].as_str().unwrap()
}

fn calls(fake: &FakeMcp) -> usize {
    fake.received_methods()
        .iter()
        .filter(|m| *m == "tools/call")
        .count()
}

/// The connection's call records, oldest first.
async fn records(setup: &Setup) -> Vec<Execution> {
    let mut all = executions(&setup.db.conn, &setup.connection, 100, None)
        .await
        .unwrap();
    all.reverse();
    all
}

/// The state and reason of each record, oldest first.
async fn outcomes(setup: &Setup) -> Vec<(ExecutionState, Option<String>)> {
    records(setup)
        .await
        .into_iter()
        .map(|e| (e.state, e.reason))
        .collect()
}

/// A validated connection whose every tool every session may run.
async fn active(fake: &FakeMcp) -> Setup {
    let setup = asking(fake).await;
    let grant = granted(&setup).await;
    grants::allow_all(&setup.db.conn, grant).await.unwrap();
    setup
}

/// A validated connection, with no grant yet.
async fn asking(fake: &FakeMcp) -> Setup {
    let setup = setup(fake).await;
    setup.connectors.validate(&setup.connection).await.unwrap();
    setup
}

#[tokio::test]
async fn an_active_connection_shows_its_tools_and_runs_calls() {
    let fake = FakeMcp::new(Era::Modern);
    let setup = active(&fake).await;
    let id = &setup.connection;
    let (_, tools) = setup.connectors.exposed_tools(id).await.unwrap();
    assert_eq!(tools[0].name, "echo");

    let output = setup
        .connectors
        .call(&anyone(), id, "echo", args("hi"))
        .await;
    assert!(!output.is_error);
    assert_eq!(text(&output), "hi");
    let sent = fake.received().pop().unwrap();
    assert_eq!(sent.header("authorization"), Some("Bearer k3y"));

    let [record] = &records(&setup).await[..] else {
        panic!("one record per call");
    };
    assert_eq!(record.state, ExecutionState::Succeeded);
    assert_eq!(record.tool_name, "echo");
    assert_eq!(record.schema_hash.as_ref(), Some(&tools[0].hash()));
    assert_eq!(record.arg_keys, ["text"]);
    assert!(record.dispatched_at.is_some() && record.finished_at.is_some());
    assert!(record.upstream_ms.is_some() && record.overhead_ms.is_some());
    assert!(record.detail.is_none());
}

#[tokio::test]
async fn a_tool_the_snapshot_lacks_is_never_sent() {
    let fake = FakeMcp::new(Era::Modern);
    let setup = active(&fake).await;
    let output = setup
        .connectors
        .call(&anyone(), &setup.connection, "delete_everything", args(""))
        .await;
    assert!(output.is_error);
    assert!(
        text(&output).contains("nothing was sent"),
        "{}",
        text(&output)
    );
    assert_eq!(calls(&fake), 0);
    let [record] = &records(&setup).await[..] else {
        panic!("one record per call");
    };
    assert_eq!(record.state, ExecutionState::Rejected);
    assert_eq!(record.reason.as_deref(), Some("not_exposed"));
    assert!(record.schema_hash.is_none() && record.dispatched_at.is_none());
}

#[tokio::test]
async fn a_revoked_connection_shows_nothing_and_sends_nothing() {
    let fake = FakeMcp::new(Era::Modern);
    let setup = active(&fake).await;
    let id = &setup.connection;
    let (before, _) = setup.connectors.exposed_tools(id).await.unwrap();
    setup.connectors.revoke(id).await.unwrap();

    let (after, tools) = setup.connectors.exposed_tools(id).await.unwrap();
    assert!(after > before, "proxies are told");
    assert!(tools.is_empty());
    let output = setup
        .connectors
        .call(&anyone(), id, "echo", args("hi"))
        .await;
    assert!(output.is_error);
    assert!(text(&output).contains("revoked"), "{}", text(&output));
    assert_eq!(calls(&fake), 0);
    assert_eq!(
        outcomes(&setup).await,
        [(ExecutionState::Rejected, Some("unavailable".into()))]
    );
}

#[tokio::test]
async fn after_a_host_restart_the_first_call_connects_again() {
    let fake = FakeMcp::new(Era::DualEra);
    let setup = active(&fake).await;
    let restarted = service(&setup.db, &setup.secrets);
    let output = restarted
        .call(&anyone(), &setup.connection, "echo", args("again"))
        .await;
    assert!(!output.is_error, "{output:?}");
    assert_eq!(text(&output), "again");
}

#[tokio::test]
async fn failures_say_whether_the_call_may_have_run() {
    let fake = FakeMcp::new(Era::Modern);
    fake.set_tools(vec![
        tool("broken", "Fails"),
        tool("refusing", "Refuses"),
        tool("dropping", "Drops"),
    ]);
    fake.on_call("broken", CallBehaviour::ToolError("no such issue".into()));
    fake.on_call(
        "refusing",
        CallBehaviour::RpcError {
            code: -32000,
            message: "bad key k3y".into(),
        },
    );
    fake.on_call("dropping", CallBehaviour::Drop);
    let setup = active(&fake).await;
    let id = &setup.connection;

    // The service ran it and says it failed: passed on as it is.
    let output = setup
        .connectors
        .call(&anyone(), id, "broken", args(""))
        .await;
    assert!(output.is_error);
    assert_eq!(text(&output), "no such issue");

    let output = setup
        .connectors
        .call(&anyone(), id, "refusing", args(""))
        .await;
    assert!(output.is_error);
    assert!(
        text(&output).contains("refused the call"),
        "{}",
        text(&output)
    );
    assert!(!text(&output).contains("k3y"), "the key is never shown");

    let output = setup
        .connectors
        .call(&anyone(), id, "dropping", args(""))
        .await;
    assert!(output.is_error);
    assert!(text(&output).contains("may have run"), "{}", text(&output));
    let dropping = fake
        .received()
        .iter()
        .filter(|r| r.method == "tools/call")
        .count();
    assert_eq!(dropping, 3, "never retried");

    assert_eq!(
        outcomes(&setup).await,
        [
            (ExecutionState::ToolError, None),
            (ExecutionState::Failed, None),
            (
                ExecutionState::OutcomeUnknown,
                Some("connection_lost".into())
            ),
        ]
    );
    let details: Vec<String> = records(&setup)
        .await
        .into_iter()
        .map(|e| e.detail.unwrap())
        .collect();
    assert_eq!(details[0], "no such issue");
    assert!(!details[1].contains("k3y"), "the key is never recorded");
    assert!(details[2].contains("may have run"));
}

#[tokio::test]
async fn a_call_dropped_midway_is_recorded_as_maybe_run() {
    let fake = FakeMcp::new(Era::Modern);
    fake.on_call("echo", CallBehaviour::Hang);
    let setup = std::sync::Arc::new(active(&fake).await);
    let call = {
        let setup = setup.clone();
        tokio::spawn(async move {
            setup
                .connectors
                .call(&anyone(), &setup.connection, "echo", args("hi"))
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while calls(&fake) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the call reached the service");
    // The proxy hanging up drops the call like this.
    call.abort();
    let _ = call.await;

    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let outcomes = outcomes(&setup).await;
            if outcomes[0].0 != ExecutionState::Dispatched {
                return outcomes;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the record was ended");
    assert_eq!(
        ended,
        [(
            ExecutionState::OutcomeUnknown,
            Some("cancelled_after_dispatch".into())
        )]
    );
}

#[tokio::test]
async fn proxies_are_told_only_when_what_agents_see_changes() {
    let fake = FakeMcp::new(Era::Modern);
    let setup = active(&fake).await;
    let id = &setup.connection;
    let runtime = setup.connectors.runtime(id);
    let seen = runtime.revision();

    // Listing the same tools again changes nothing.
    setup.connectors.validate(id).await.unwrap();
    assert_eq!(runtime.revision(), seen);

    let waiter = {
        let runtime = runtime.clone();
        tokio::spawn(async move { runtime.changed_since(seen).await })
    };
    fake.set_tools(vec![tool("search", "Searches")]);
    setup.connectors.validate(id).await.unwrap();
    let next = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .unwrap()
        .unwrap();
    assert!(next > seen);
    let (revision, tools) = setup.connectors.exposed_tools(id).await.unwrap();
    assert_eq!((revision, tools[0].name.as_str()), (next, "search"));
}

#[tokio::test]
async fn a_service_reporting_new_tools_updates_what_agents_see() {
    // Legacy HTTP servers report changes only on a GET stream, which the
    // fake doesn't offer; the legacy notification itself is covered over
    // stdio.
    for era in [Era::Modern, Era::DualEra] {
        let fake = FakeMcp::new(era);
        let setup = active(&fake).await;
        let id = &setup.connection;
        let runtime = setup.connectors.runtime(id);
        let seen = runtime.revision();
        fake.set_tools(vec![tool("search", "Searches")]);
        let next = tokio::time::timeout(Duration::from_secs(5), runtime.changed_since(seen))
            .await
            .unwrap_or_else(|_| panic!("{era:?}: the change was never followed"));
        assert!(next > seen);
        let (_, tools) = setup.connectors.exposed_tools(id).await.unwrap();
        assert_eq!(tools[0].name, "search", "{era:?}");
    }
}

#[tokio::test]
async fn a_revoked_connection_stops_following_its_service() {
    let fake = FakeMcp::new(Era::Modern);
    let setup = active(&fake).await;
    let id = &setup.connection;
    setup.connectors.revoke(id).await.unwrap();
    let lists = |fake: &FakeMcp| {
        fake.received_methods()
            .iter()
            .filter(|m| *m == "tools/list")
            .count()
    };
    let before = lists(&fake);
    fake.set_tools(vec![tool("search", "Searches")]);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(lists(&fake), before, "nothing listed after revocation");
}

// ── policy and approvals ──────────────────────────────────────────────────

/// Runs `echo` on a task, as a proxy would, for `session`.
fn spawn_echo(
    setup: &Arc<Setup>,
    session: CallSession,
    text: &str,
) -> tokio::task::JoinHandle<CallOutput> {
    let (setup, text) = (setup.clone(), text.to_string());
    tokio::spawn(async move {
        setup
            .connectors
            .call(&session, &setup.connection, "echo", args(&text))
            .await
    })
}

async fn finished(call: tokio::task::JoinHandle<CallOutput>) -> CallOutput {
    tokio::time::timeout(Duration::from_secs(5), call)
        .await
        .expect("the call ended")
        .unwrap()
}

async fn answer(
    setup: &Setup,
    card: &ApprovalCard,
    answer: ApprovalAnswer,
) -> Result<(), AnswerError> {
    setup
        .connectors
        .answer_approval("s1", &card.approval_id, &card.binding_hash, answer, "host")
        .await
}

#[tokio::test]
async fn without_a_grant_nothing_is_sent() {
    let fake = FakeMcp::new(Era::Modern);
    let setup = asking(&fake).await;
    let output = setup
        .connectors
        .call(&anyone(), &setup.connection, "echo", args("hi"))
        .await;
    assert!(output.is_error);
    assert!(text(&output).contains("isn't allowed"), "{}", text(&output));

    let mut engine = anyone();
    engine.engine = true;
    granted(&setup).await;
    let output = setup
        .connectors
        .call(&engine, &setup.connection, "echo", args("hi"))
        .await;
    assert!(
        text(&output).contains("Automation sessions"),
        "{}",
        text(&output)
    );
    assert_eq!(calls(&fake), 0);
    let records = records(&setup).await;
    assert!(records
        .iter()
        .all(|r| r.state == ExecutionState::Rejected && r.reason.as_deref() == Some("denied")));
    assert!(records
        .iter()
        .all(|r| r.decided_by.as_deref() == Some("policy")));
}

#[tokio::test]
async fn a_tool_that_asks_runs_once_the_user_says_yes() {
    let fake = FakeMcp::new(Era::Modern);
    let setup = Arc::new(asking(&fake).await);
    granted(&setup).await;
    let cards = Arc::new(Cards::default());

    let call = spawn_echo(&setup, session(cards.clone()), "secret k3y here");
    let card = cards.nth(0).await;
    assert_eq!((card.tool.as_str(), card.label.as_str()), ("echo", "Work"));
    assert!(card.preview.contains("secret") && !card.preview.contains("k3y"));
    assert_eq!(calls(&fake), 0, "nothing is sent while it asks");
    let waiting = records(&setup).await;
    assert_eq!(waiting[0].state, ExecutionState::AwaitingApproval);

    answer(&setup, &card, ApprovalAnswer::Once).await.unwrap();
    let output = finished(call).await;
    assert!(!output.is_error, "{output:?}");
    assert_eq!(calls(&fake), 1);
    assert_eq!(
        cards.cleared.lock().unwrap()[..],
        [card.approval_id.as_str()]
    );
    let record = &records(&setup).await[0];
    assert_eq!(record.state, ExecutionState::Succeeded);
    assert_eq!(record.decided_by.as_deref(), Some("host"));
    let used = approvals::approval(&setup.db.conn, &card.approval_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(used.status, ApprovalStatus::Consumed);
    assert!(matches!(
        answer(&setup, &card, ApprovalAnswer::Once).await,
        Err(AnswerError::AlreadyResolved { .. })
    ));

    // Once is once: the next call asks again.
    let again = spawn_echo(&setup, session(cards.clone()), "again");
    let second = cards.nth(1).await;
    assert_ne!(second.approval_id, card.approval_id);
    answer(&setup, &second, ApprovalAnswer::Deny).await.unwrap();
    let output = finished(again).await;
    assert!(text(&output).contains("denied"), "{}", text(&output));
    assert_eq!(calls(&fake), 1);
    let record = &records(&setup).await[1];
    assert_eq!(record.state, ExecutionState::ApprovalDenied);
}

#[tokio::test]
async fn an_answer_must_name_its_call_and_session() {
    let fake = FakeMcp::new(Era::Modern);
    let setup = Arc::new(asking(&fake).await);
    granted(&setup).await;
    let cards = Arc::new(Cards::default());
    let call = spawn_echo(&setup, session(cards.clone()), "hi");
    let card = cards.nth(0).await;

    let wrong = setup
        .connectors
        .answer_approval(
            "s1",
            &card.approval_id,
            "sha256:other",
            ApprovalAnswer::Once,
            "host",
        )
        .await;
    assert!(matches!(wrong, Err(AnswerError::Mismatch)));
    let elsewhere = setup
        .connectors
        .answer_approval(
            "s2",
            &card.approval_id,
            &card.binding_hash,
            ApprovalAnswer::Once,
            "host",
        )
        .await;
    assert!(matches!(elsewhere, Err(AnswerError::NotFound)));
    assert_eq!(calls(&fake), 0);

    answer(&setup, &card, ApprovalAnswer::Deny).await.unwrap();
    finished(call).await;
}

#[tokio::test]
async fn a_session_allow_lasts_until_the_session_ends() {
    let fake = FakeMcp::new(Era::Modern);
    let setup = Arc::new(asking(&fake).await);
    granted(&setup).await;
    let cards = Arc::new(Cards::default());

    let call = spawn_echo(&setup, session(cards.clone()), "one");
    answer(&setup, &cards.nth(0).await, ApprovalAnswer::Session)
        .await
        .unwrap();
    assert!(!finished(call).await.is_error);
    let output = finished(spawn_echo(&setup, session(cards.clone()), "two")).await;
    assert!(!output.is_error);
    assert_eq!(cards.count(), 1, "allowed for the session: no card");
    assert_eq!(
        records(&setup).await[1].decided_by.as_deref(),
        Some("session_grant")
    );

    setup.connectors.end_session("s1");
    let call = spawn_echo(&setup, session(cards.clone()), "three");
    answer(&setup, &cards.nth(1).await, ApprovalAnswer::Deny)
        .await
        .unwrap();
    assert!(finished(call).await.is_error);
}

#[tokio::test]
async fn always_and_always_deny_become_rules() {
    let fake = FakeMcp::new(Era::Modern);
    let setup = Arc::new(asking(&fake).await);
    let grant = granted(&setup).await;
    let cards = Arc::new(Cards::default());

    let call = spawn_echo(&setup, session(cards.clone()), "one");
    answer(&setup, &cards.nth(0).await, ApprovalAnswer::Always)
        .await
        .unwrap();
    assert!(!finished(call).await.is_error);
    let rules = grants::grant(&setup.db.conn, grant).await.unwrap().unwrap();
    assert_eq!(rules.row("echo").unwrap().policy, ToolPolicy::Allow);
    assert!(
        !finished(spawn_echo(&setup, session(cards.clone()), "two"))
            .await
            .is_error
    );
    assert_eq!(cards.count(), 1, "allowed from now on");

    grants::set_tool_policy(&setup.db.conn, grant, "echo", None)
        .await
        .unwrap();
    let call = spawn_echo(&setup, session(cards.clone()), "three");
    answer(&setup, &cards.nth(1).await, ApprovalAnswer::AlwaysDeny)
        .await
        .unwrap();
    assert!(finished(call).await.is_error);
    let output = finished(spawn_echo(&setup, session(cards.clone()), "four")).await;
    assert!(output.is_error);
    assert_eq!(cards.count(), 2, "denied from now on");
    assert_eq!(calls(&fake), 2);
}

#[tokio::test]
async fn a_tool_that_changed_after_approval_is_not_sent() {
    let fake = FakeMcp::new(Era::Modern);
    let setup = Arc::new(asking(&fake).await);
    granted(&setup).await;
    let cards = Arc::new(Cards::default());
    let (hook, changed) = tokio::sync::oneshot::channel();
    *cards.on_clear.lock().unwrap() = Some(hook);
    let call = spawn_echo(&setup, session(cards.clone()), "hi");
    let card = cards.nth(0).await;

    let changer = {
        let setup = setup.clone();
        tokio::spawn(async move {
            changed.await.unwrap();
            fake_change(&setup).await;
        })
    };
    answer(&setup, &card, ApprovalAnswer::Once).await.unwrap();
    let output = finished(call).await;
    changer.await.unwrap();
    assert!(text(&output).contains("changed"), "{}", text(&output));
    let voided = approvals::approval(&setup.db.conn, &card.approval_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(voided.status, ApprovalStatus::Void);
    let record = &records(&setup).await[0];
    assert_eq!(record.state, ExecutionState::ApprovalVoid);
    assert_eq!(record.reason.as_deref(), Some("tool_changed"));
}

/// The service now lists `echo` differently.
async fn fake_change(setup: &Setup) {
    let changed = crate::connectors::upstream::ToolDefinition {
        name: "echo".into(),
        title: None,
        description: Some("Echoes, now differently".into()),
        input_schema: json!({ "type": "object" }),
        output_schema: None,
        annotations: None,
    };
    crate::connectors::store::save_tool_snapshot(&setup.db.conn, &setup.connection, &[changed])
        .await
        .unwrap();
}

#[tokio::test]
async fn revoking_ends_what_waits_for_an_answer() {
    let fake = FakeMcp::new(Era::Modern);
    let setup = Arc::new(asking(&fake).await);
    granted(&setup).await;
    let cards = Arc::new(Cards::default());
    let call = spawn_echo(&setup, session(cards.clone()), "hi");
    let card = cards.nth(0).await;

    setup.connectors.revoke(&setup.connection).await.unwrap();
    let output = finished(call).await;
    assert!(text(&output).contains("revoked"), "{}", text(&output));
    assert_eq!(
        cards.cleared.lock().unwrap()[..],
        [card.approval_id.as_str()]
    );
    let ended = approvals::approval(&setup.db.conn, &card.approval_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ended.status, ApprovalStatus::Cancelled);
    assert_eq!(calls(&fake), 0);
}

#[tokio::test]
async fn an_unanswered_card_expires() {
    let fake = FakeMcp::new(Era::Modern);
    let setup = Arc::new(asking(&fake).await);
    granted(&setup).await;
    let cards = Arc::new(Cards::default());
    let call = spawn_echo(&setup, session(cards.clone()), "hi");
    let card = cards.nth(0).await;

    // Its time runs out.
    use sea_orm::{ActiveModelTrait, Set};
    crate::db::entities::connector_approval::ActiveModel {
        id: Set(card.approval_id.clone()),
        expires_at: Set(chrono::Utc::now() - chrono::Duration::seconds(1)),
        ..Default::default()
    }
    .update(&setup.db.conn)
    .await
    .unwrap();
    setup.connectors.expire_approvals().await;

    let output = finished(call).await;
    assert!(text(&output).contains("in time"), "{}", text(&output));
    assert_eq!(
        records(&setup).await[0].state,
        ExecutionState::ApprovalExpired
    );
    assert!(matches!(
        answer(&setup, &card, ApprovalAnswer::Once).await,
        Err(AnswerError::Expired)
    ));
}

#[tokio::test]
async fn a_proxy_hanging_up_takes_its_card_down() {
    let fake = FakeMcp::new(Era::Modern);
    let setup = Arc::new(asking(&fake).await);
    granted(&setup).await;
    let cards = Arc::new(Cards::default());
    let call = spawn_echo(&setup, session(cards.clone()), "hi");
    let card = cards.nth(0).await;
    call.abort();
    let _ = call.await;

    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let approval = approvals::approval(&setup.db.conn, &card.approval_id)
                .await
                .unwrap()
                .unwrap();
            if approval.status != ApprovalStatus::Pending
                && !cards.cleared.lock().unwrap().is_empty()
            {
                return approval;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the card came down");
    assert_eq!(ended.status, ApprovalStatus::Cancelled);
    assert!(matches!(
        answer(&setup, &card, ApprovalAnswer::Once).await,
        Err(AnswerError::AlreadyResolved { .. })
    ));
    assert_eq!(calls(&fake), 0);
}

#[tokio::test]
async fn with_no_one_to_ask_nothing_is_sent() {
    let fake = FakeMcp::new(Era::Modern);
    let setup = asking(&fake).await;
    granted(&setup).await;
    let output = setup
        .connectors
        .call(&anyone(), &setup.connection, "echo", args("hi"))
        .await;
    assert!(
        text(&output).contains("No one can answer"),
        "{}",
        text(&output)
    );
    assert_eq!(calls(&fake), 0);
}
