//! [`McpUpstream`] over stdio framing against the hand-written fake, in every
//! protocol era. Each connection attempt gets a fresh in-memory stream, as a
//! fresh process would.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Map};
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};

use super::McpUpstream;
use crate::connectors::fake_mcp::{
    echo_tool, tool, CallBehaviour, Era, FakeMcp, StreamServer, LEGACY, MODERN,
};
use crate::connectors::upstream::{
    CallFailure, ConnectorError, DispatchMark, ShutdownReason, UnknownCause, Upstream, UpstreamCall,
};

const WAIT: Duration = Duration::from_secs(5);

/// Every stream the fake served, oldest first. Kept so the fakes stay up.
type Servers = Arc<Mutex<Vec<StreamServer>>>;

fn opener(
    fake: &FakeMcp,
) -> (
    Servers,
    impl Fn() -> (ReadHalf<DuplexStream>, WriteHalf<DuplexStream>),
) {
    let servers = Servers::default();
    let (fake, kept) = (fake.clone(), servers.clone());
    let open = move || {
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        kept.lock().unwrap().push(fake.serve_stream(theirs));
        tokio::io::split(ours)
    };
    (servers, open)
}

async fn connect(fake: &FakeMcp) -> (Servers, McpUpstream) {
    let (servers, open) = opener(fake);
    let upstream = McpUpstream::connect_stdio(open)
        .await
        .unwrap_or_else(|e| panic!("{:?}: {e}", fake.era()));
    (servers, upstream)
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
    let mut eras = vec![(Era::Modern, MODERN, 1), (Era::DualEra, MODERN, 1)];
    for version in LEGACY {
        eras.push((Era::Legacy(version), version, 1));
        // Refuses `server/discover` with "unsupported version": one more stream.
        eras.push((Era::LegacyAware(version), version, 2));
    }
    for (era, expected, streams) in eras {
        let fake = FakeMcp::new(era);
        let (servers, upstream) = connect(&fake).await;
        assert_eq!(
            upstream.protocol_version().as_deref(),
            Some(expected),
            "{era:?}"
        );
        assert_eq!(servers.lock().unwrap().len(), streams, "{era:?}");

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
        upstream
            .probe()
            .await
            .unwrap_or_else(|e| panic!("{era:?} probe: {e}"));
    }
}

#[tokio::test]
async fn an_unknown_version_is_reported_with_what_the_server_supports() {
    let fake = FakeMcp::new(Era::LegacyAware("2023-01-01"));
    let (_servers, open) = opener(&fake);
    let error = McpUpstream::connect_stdio(open).await.err().unwrap();
    assert_eq!(
        error,
        ConnectorError::UnsupportedVersion {
            supported: vec!["2023-01-01".into()],
        }
    );
}

#[tokio::test]
async fn tool_changes_are_reported_in_both_eras() {
    for era in [Era::Modern, Era::Legacy("2025-06-18")] {
        let fake = FakeMcp::new(era);
        let (_servers, upstream) = connect(&fake).await;
        let mut changes = upstream.tool_changes();
        // Modern: the listen stream opens in the background. Legacy: the
        // fake only notifies after `notifications/initialized`.
        let ready = match era {
            Era::Modern => "subscriptions/listen",
            _ => "notifications/initialized",
        };
        eventually(ready, || fake.received_methods().iter().any(|m| m == ready)).await;

        fake.set_tools(vec![echo_tool(), tool("search", "Search")]);
        tokio::time::timeout(WAIT, changes.changed())
            .await
            .unwrap_or_else(|_| panic!("{era:?}: a tool change"))
            .unwrap();
        let listing = upstream.list_tools().await.unwrap();
        assert_eq!(listing.tools.len(), 2, "{era:?}");
    }
}

#[tokio::test]
async fn a_dropped_call_is_cancelled_upstream() {
    for era in [Era::Modern, Era::Legacy("2025-06-18")] {
        let fake = FakeMcp::new(era);
        fake.on_call("echo", CallBehaviour::Hang);
        let (_servers, upstream) = connect(&fake).await;
        let dispatch = DispatchMark::default();
        let timed_out = tokio::time::timeout(
            Duration::from_millis(200),
            upstream.call(echo("x"), &dispatch),
        )
        .await;
        assert!(timed_out.is_err(), "{era:?}");
        assert!(dispatch.dispatched());
        eventually(&format!("{era:?}: notifications/cancelled"), || {
            fake.received_methods()
                .iter()
                .any(|m| m == "notifications/cancelled")
        })
        .await;
    }
}

#[tokio::test]
async fn a_server_that_exits_mid_call_is_never_retried() {
    let fake = FakeMcp::new(Era::DualEra);
    fake.on_call("echo", CallBehaviour::Drop);
    let (servers, upstream) = connect(&fake).await;
    let dispatch = DispatchMark::default();
    let failure = upstream.call(echo("x"), &dispatch).await.unwrap_err();
    assert_eq!(
        failure,
        CallFailure::OutcomeUnknown(UnknownCause::ConnectionLost)
    );
    assert!(dispatch.dispatched());
    tokio::time::sleep(Duration::from_millis(200)).await;
    let calls = fake
        .received_methods()
        .into_iter()
        .filter(|m| m == "tools/call")
        .count();
    assert_eq!(calls, 1);
    // No new stream either: nothing reconnected behind the caller's back.
    assert_eq!(servers.lock().unwrap().len(), 1);
    assert!(upstream.probe().await.is_err());
}

#[tokio::test]
async fn shutdown_closes_the_stream_once() {
    let fake = FakeMcp::new(Era::Legacy("2025-11-25"));
    let (servers, upstream) = connect(&fake).await;
    upstream.shutdown(ShutdownReason::Revoked).await.unwrap();
    eventually("the server's stdin to close", || {
        servers.lock().unwrap()[0].is_closed()
    })
    .await;
    upstream.shutdown(ShutdownReason::Revoked).await.unwrap();
    assert!(upstream.probe().await.is_err());
    assert!(matches!(
        upstream.call(echo("x"), &DispatchMark::default()).await,
        Err(CallFailure::OutcomeUnknown(_) | CallFailure::NotSent(_))
    ));
}
