//! The gateway against the fake MCP server over real HTTP.

use std::time::Duration;

use serde_json::{json, Map, Value};

use super::super::tests::{service, setup, Setup};
use crate::connectors::executions::{executions, Execution, ExecutionState};
use crate::connectors::fake_mcp::{tool, CallBehaviour, Era, FakeMcp};
use crate::connectors::upstream::CallOutput;

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

async fn active(fake: &FakeMcp) -> Setup {
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

    let output = setup.connectors.call(id, "echo", args("hi")).await;
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
        .call(&setup.connection, "delete_everything", args(""))
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
    let output = setup.connectors.call(id, "echo", args("hi")).await;
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
        .call(&setup.connection, "echo", args("again"))
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
    let output = setup.connectors.call(id, "broken", args("")).await;
    assert!(output.is_error);
    assert_eq!(text(&output), "no such issue");

    let output = setup.connectors.call(id, "refusing", args("")).await;
    assert!(output.is_error);
    assert!(
        text(&output).contains("refused the call"),
        "{}",
        text(&output)
    );
    assert!(!text(&output).contains("k3y"), "the key is never shown");

    let output = setup.connectors.call(id, "dropping", args("")).await;
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
                .call(&setup.connection, "echo", args("hi"))
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
