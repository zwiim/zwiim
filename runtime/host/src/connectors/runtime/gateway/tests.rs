//! The gateway against the fake MCP server over real HTTP.

use std::time::Duration;

use serde_json::{json, Map, Value};

use super::super::tests::{service, setup, Setup};
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
