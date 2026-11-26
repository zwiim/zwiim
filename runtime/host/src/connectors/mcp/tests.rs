//! [`McpUpstream`] over Streamable HTTP against the hand-written fake server,
//! in every protocol era, through a real [`EgressHttpClient`].

use std::sync::Arc;
use std::time::Duration;

use http::header::AUTHORIZATION;
use serde_json::{json, Map};
use url::Url;

use super::McpUpstream;
use crate::connectors::egress::{
    Credential, EgressGuard, EgressHttpClient, EndpointRule, NetworkClass, SystemResolver,
};
use crate::connectors::fake_mcp::{
    echo_tool, tool, CallBehaviour, Era, FakeMcp, HttpServer, LEGACY, MODERN,
};
use crate::connectors::upstream::{
    CallFailure, ConnectorError, DispatchMark, ShutdownReason, UnknownCause, Upstream, UpstreamCall,
};

const WAIT: Duration = Duration::from_secs(5);

fn egress_for(origin: &str) -> Arc<EgressHttpClient> {
    let rule = EndpointRule::new(origin, true, NetworkClass::Local).unwrap();
    let guard = EgressGuard::new(vec![rule], Arc::new(SystemResolver));
    Arc::new(EgressHttpClient::new(Arc::new(guard)).unwrap())
}

async fn connect(fake: &FakeMcp) -> (HttpServer, McpUpstream) {
    let server = fake.serve_http().await;
    let url = Url::parse(&server.url()).unwrap();
    let egress = egress_for(&format!("http://{}", server.addr()));
    let credential = Credential::new(AUTHORIZATION, "Bearer t0ken").unwrap();
    let upstream = McpUpstream::connect_http(&url, egress, Some(credential))
        .await
        .unwrap_or_else(|e| panic!("{:?}: {e}", fake.era()));
    (server, upstream)
}

fn echo(text: &str) -> UpstreamCall {
    let mut arguments = Map::new();
    arguments.insert("text".into(), json!(text));
    UpstreamCall {
        tool: "echo".into(),
        arguments,
    }
}

async fn eventually(what: &str, check: impl Fn() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

#[tokio::test]
async fn every_era_negotiates_lists_and_calls() {
    let mut eras = vec![(Era::Modern, MODERN), (Era::DualEra, MODERN)];
    for version in LEGACY {
        eras.push((Era::Legacy(version), version));
        eras.push((Era::LegacyAware(version), version));
    }
    for (era, expected) in eras {
        let fake = FakeMcp::new(era);
        let (_server, upstream) = connect(&fake).await;
        assert_eq!(
            upstream.protocol_version().as_deref(),
            Some(expected),
            "{era:?}"
        );

        let listing = upstream.list_tools().await.unwrap();
        assert_eq!(listing.tools.len(), 1, "{era:?}");
        assert_eq!(listing.tools[0].name, "echo");

        let dispatch = DispatchMark::default();
        let output = upstream.call(echo("hi"), &dispatch).await.unwrap();
        assert!(dispatch.dispatched());
        assert_eq!(
            output.content,
            vec![json!({ "type": "text", "text": "hi" })]
        );
        assert!(!output.is_error);
        upstream
            .probe()
            .await
            .unwrap_or_else(|e| panic!("{era:?} probe: {e}"));
    }
}

#[tokio::test]
async fn an_unknown_version_is_reported_with_what_the_server_supports() {
    let fake = FakeMcp::new(Era::LegacyAware("2023-01-01"));
    let server = fake.serve_http().await;
    let url = Url::parse(&server.url()).unwrap();
    let egress = egress_for(&format!("http://{}", server.addr()));
    let error = McpUpstream::connect_http(&url, egress, None)
        .await
        .err()
        .unwrap();
    assert_eq!(
        error,
        ConnectorError::UnsupportedVersion {
            supported: vec!["2023-01-01".into()],
        }
    );
}

#[tokio::test]
async fn tool_definitions_keep_every_field() {
    let fake = FakeMcp::new(Era::Modern);
    let mut rich = tool("create_issue", "Creates an issue");
    rich["title"] = json!("Create issue");
    rich["outputSchema"] = json!({ "type": "object" });
    rich["annotations"] = json!({ "destructiveHint": false });
    fake.set_tools(vec![rich.clone()]);
    let (_server, upstream) = connect(&fake).await;
    let tool = upstream.list_tools().await.unwrap().tools.remove(0);
    assert_eq!(tool.name, "create_issue");
    assert_eq!(tool.title.as_deref(), Some("Create issue"));
    assert_eq!(tool.description.as_deref(), Some("Creates an issue"));
    assert_eq!(tool.input_schema, rich["inputSchema"]);
    assert_eq!(tool.output_schema, Some(json!({ "type": "object" })));
    assert_eq!(tool.annotations, Some(json!({ "destructiveHint": false })));
}

#[tokio::test]
async fn tool_failures_and_refusals_are_told_apart() {
    let fake = FakeMcp::new(Era::DualEra);
    let (_server, upstream) = connect(&fake).await;

    fake.on_call("echo", CallBehaviour::ToolError("no such repo".into()));
    let output = upstream
        .call(echo("x"), &DispatchMark::default())
        .await
        .unwrap();
    assert!(output.is_error);

    fake.on_call(
        "echo",
        CallBehaviour::RpcError {
            code: -32602,
            message: "bad arguments".into(),
        },
    );
    let failure = upstream
        .call(echo("x"), &DispatchMark::default())
        .await
        .unwrap_err();
    assert_eq!(
        failure,
        CallFailure::Rejected {
            code: -32602,
            message: "bad arguments".into(),
        }
    );
}

#[tokio::test]
async fn a_modern_server_reports_tool_changes() {
    let fake = FakeMcp::new(Era::Modern);
    let (_server, upstream) = connect(&fake).await;
    let mut changes = upstream.tool_changes();
    // The listen stream is opened in the background; wait until it is.
    eventually("subscriptions/listen", || {
        fake.received_methods()
            .iter()
            .any(|m| m == "subscriptions/listen")
    })
    .await;

    fake.set_tools(vec![echo_tool(), tool("search", "Search")]);
    tokio::time::timeout(WAIT, changes.changed())
        .await
        .expect("a tool change")
        .unwrap();
    let names: Vec<_> = upstream
        .list_tools()
        .await
        .unwrap()
        .tools
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["echo", "search"]);
}

/// Legacy servers get `notifications/cancelled`; over 2026-07-28 HTTP the
/// client cancels by closing the request.
#[tokio::test]
async fn a_dropped_call_is_cancelled_upstream() {
    for (era, signal) in [
        (Era::Modern, "HTTP abandoned"),
        (Era::Legacy("2025-06-18"), "notifications/cancelled"),
    ] {
        let fake = FakeMcp::new(era);
        fake.on_call("echo", CallBehaviour::Hang);
        let (_server, upstream) = connect(&fake).await;
        let dispatch = DispatchMark::default();
        let timed_out = tokio::time::timeout(
            Duration::from_millis(200),
            upstream.call(echo("x"), &dispatch),
        )
        .await;
        assert!(timed_out.is_err(), "{era:?}");
        assert!(dispatch.dispatched());
        eventually(&format!("{era:?}: {signal}"), || {
            fake.received_methods().iter().any(|m| m == signal)
        })
        .await;
    }
}

#[tokio::test]
async fn a_lost_connection_mid_call_is_never_retried() {
    let fake = FakeMcp::new(Era::DualEra);
    fake.on_call("echo", CallBehaviour::Drop);
    let (_server, upstream) = connect(&fake).await;
    let dispatch = DispatchMark::default();
    let failure = upstream.call(echo("x"), &dispatch).await.unwrap_err();
    assert_eq!(
        failure,
        CallFailure::OutcomeUnknown(UnknownCause::ConnectionLost)
    );
    assert!(dispatch.dispatched());
    // Give a retry the chance to happen, then check it didn't.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let calls = fake
        .received_methods()
        .into_iter()
        .filter(|m| m == "tools/call")
        .count();
    assert_eq!(calls, 1);
}

#[tokio::test]
async fn the_credential_goes_on_every_request() {
    let fake = FakeMcp::new(Era::Legacy("2025-11-25"));
    let (_server, upstream) = connect(&fake).await;
    upstream
        .call(echo("x"), &DispatchMark::default())
        .await
        .unwrap();
    let received = fake.received();
    assert!(!received.is_empty());
    for request in received {
        assert_eq!(
            request.header("authorization"),
            Some("Bearer t0ken"),
            "{}",
            request.method
        );
    }
}

#[tokio::test]
async fn a_refused_origin_is_never_contacted() {
    let fake = FakeMcp::new(Era::Modern);
    let server = fake.serve_http().await;
    let url = Url::parse(&server.url()).unwrap();
    // The connector's only endpoint is another port.
    let egress = egress_for("http://127.0.0.1:1");
    let error = McpUpstream::connect_http(&url, egress, None)
        .await
        .err()
        .unwrap();
    assert!(
        matches!(&error, ConnectorError::Transport(message) if message.contains("isn't an endpoint")),
        "{error:?}"
    );
    assert!(fake.received().is_empty());
}

#[tokio::test]
async fn shutdown_ends_the_legacy_session_once() {
    let fake = FakeMcp::new(Era::Legacy("2025-03-26"));
    let (_server, upstream) = connect(&fake).await;
    upstream.shutdown(ShutdownReason::Revoked).await.unwrap();
    eventually("the session DELETE", || {
        fake.received_methods().iter().any(|m| m == "HTTP DELETE")
    })
    .await;
    upstream.shutdown(ShutdownReason::Revoked).await.unwrap();
    assert!(upstream.probe().await.is_err());
    assert!(matches!(
        upstream.call(echo("x"), &DispatchMark::default()).await,
        Err(CallFailure::OutcomeUnknown(_) | CallFailure::NotSent(_))
    ));
}

#[tokio::test]
async fn eight_connections_work_at_once() {
    let fakes: Vec<_> = (0..8)
        .map(|i| {
            FakeMcp::new(if i % 2 == 0 {
                Era::Modern
            } else {
                Era::Legacy("2025-06-18")
            })
        })
        .collect();
    let runs = fakes.iter().map(|fake| async move {
        let (server, upstream) = connect(fake).await;
        let output = upstream
            .call(echo("together"), &DispatchMark::default())
            .await
            .unwrap();
        upstream
            .shutdown(ShutdownReason::SessionEnded)
            .await
            .unwrap();
        drop(server);
        output.content
    });
    for content in futures::future::join_all(runs).await {
        assert_eq!(content, vec![json!({ "type": "text", "text": "together" })]);
    }
}
