//! What a connector proxy asks of a connection (design r3 §3.3): the tools
//! an agent sees, when they change, and calls. Every refusal is a line for
//! the agent, delivered as a tool error.
//!
//! Every call is recorded (r3 §4.3). Policy and approvals (r3 §5.1, §5.2)
//! come before anything reaches this: no proxy token names a connection
//! until then.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sea_orm::DatabaseConnection;
use serde_json::{json, Map, Value};

use super::{Connectors, ValidateError};
use crate::connectors::executions::{self, NewCall, Outcome, RejectReason, Timings};
use crate::connectors::store::{self, ConnectionStatus};
use crate::connectors::upstream::{
    CallFailure, CallOutput, DispatchMark, ToolDefinition, UnknownCause, Upstream, UpstreamCall,
};

/// How long one call may take before its outcome is unknown.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

impl Connectors {
    /// The tools agents see on connection `id`, and the revision they are
    /// at. None while the connection isn't active.
    pub async fn exposed_tools(&self, id: &str) -> Result<(u64, Vec<ToolDefinition>), String> {
        // Read first: a change after this is a later revision.
        let revision = self.runtime(id).revision();
        let connection = store::connection(&self.db, id)
            .await
            .map_err(internal)?
            .ok_or_else(gone)?;
        if connection.status != ConnectionStatus::Active || self.runtime(id).is_blocked() {
            return Ok((revision, Vec::new()));
        }
        let tools = store::tools(&self.db, id).await.map_err(internal)?;
        Ok((revision, tools))
    }

    /// Runs `tool` on connection `id`, with a record of the call (r3 §4.3).
    /// Anything that keeps it from running, or leaves its outcome unknown,
    /// comes back as a tool error that says so. A call whose request left
    /// the Host is never retried.
    pub async fn call(&self, id: &str, tool: &str, arguments: Map<String, Value>) -> CallOutput {
        let started = Instant::now();
        let connection = match store::connection(&self.db, id).await {
            Ok(Some(connection)) => connection,
            Ok(None) => return refusal(&gone()),
            Err(e) => return refusal(&internal(e)),
        };
        let tools = match store::tools(&self.db, id).await {
            Ok(tools) => tools,
            Err(e) => return refusal(&internal(e)),
        };
        let listed = tools.iter().find(|t| t.name == tool);
        let hash = listed.map(ToolDefinition::hash);
        let record = executions::receive(
            &self.db,
            NewCall {
                connection_id: id,
                session_id: None,
                tool_name: tool,
                schema_hash: hash.as_deref(),
                arguments: &arguments,
            },
        )
        .await;
        let record = match record {
            Ok(record) => Record::new(self.db.clone(), record.id, started),
            Err(e) => return refusal(&unrecorded(e)),
        };
        let (outcome, upstream_ms, output) = self
            .run(&connection, listed.is_some(), tool, arguments, &record)
            .await;
        let detail = output.is_error.then(|| first_text(&output));
        record.end(outcome, detail.as_deref(), upstream_ms).await;
        output
    }

    /// Everything between receiving a call and its outcome: the outcome,
    /// the service's round trip, and what the agent is given.
    async fn run(
        &self,
        connection: &store::Connection,
        listed: bool,
        tool: &str,
        arguments: Map<String, Value>,
        record: &Record,
    ) -> (Outcome, Option<Duration>, CallOutput) {
        let rejected = |reason, line: String| (Outcome::Rejected(reason), None, refusal(&line));
        if connection.status != ConnectionStatus::Active
            || self.runtime(&connection.id).is_blocked()
        {
            return rejected(
                RejectReason::Unavailable,
                format!(
                    "The {} connection is {}; nothing was sent.",
                    connection.slug, connection.status
                ),
            );
        }
        if !listed {
            return rejected(
                RejectReason::NotExposed,
                format!(
                    "The {} connection has no tool named {tool:?}; nothing was sent.",
                    connection.slug
                ),
            );
        }
        let upstream = match self.shared_upstream(connection).await {
            Ok(upstream) => upstream,
            Err((reason, line)) => return rejected(reason, line),
        };
        match executions::dispatch(&self.db, &record.id).await {
            Ok(true) => {}
            Ok(false) => return rejected(RejectReason::NotSent, unrecorded("already ended")),
            Err(e) => return rejected(RejectReason::NotSent, unrecorded(e)),
        }
        let call = UpstreamCall {
            tool: tool.into(),
            arguments,
        };
        let unknown = |cause| {
            (
                Outcome::Unknown(cause),
                refusal(&format!(
                    "This call may have run on {}. Check before trying again.",
                    connection.slug
                )),
            )
        };
        let sent = Instant::now();
        let result = tokio::time::timeout(CALL_TIMEOUT, upstream.call(call, &record.mark)).await;
        let upstream_ms = Some(sent.elapsed());
        let (outcome, output) = match result {
            Ok(Ok(output)) if output.is_error => (Outcome::ToolError, output),
            Ok(Ok(output)) => (Outcome::Succeeded, output),
            Ok(Err(CallFailure::NotSent(e))) => (
                Outcome::Rejected(RejectReason::NotSent),
                refusal(&format!(
                    "The call wasn't sent: {}",
                    self.redacted(connection, &e.to_string()).await
                )),
            ),
            Ok(Err(CallFailure::Rejected { message, .. })) => (
                Outcome::Failed,
                refusal(&format!(
                    "{} refused the call: {}",
                    connection.slug,
                    self.redacted(connection, &message).await
                )),
            ),
            Ok(Err(CallFailure::OutcomeUnknown(cause))) => unknown(cause),
            Err(_) if record.mark.dispatched() => unknown(UnknownCause::Timeout),
            Err(_) => (
                Outcome::CancelledBeforeDispatch,
                refusal("The service didn't answer in time; nothing was sent."),
            ),
        };
        (outcome, upstream_ms, output)
    }

    /// At boot, before any call: ends the calls the last Host left open.
    pub async fn end_unfinished_calls(&self) {
        match executions::sweep(&self.db).await {
            Ok(0) => {}
            Ok(n) => tracing::info!("[connectors] ended {n} calls the last Host left open"),
            Err(e) => tracing::warn!("[connectors] couldn't end unfinished calls: {e}"),
        }
    }

    /// The connection's shared upstream, opened on first use (after a Host
    /// restart the connection is active but nothing is open yet).
    async fn shared_upstream(
        &self,
        connection: &store::Connection,
    ) -> Result<std::sync::Arc<dyn Upstream>, (RejectReason, String)> {
        let runtime = self.runtime(&connection.id);
        let mut shared = runtime.shared.lock().await;
        if let Some(upstream) = shared.as_ref() {
            return Ok(upstream.clone());
        }
        let upstream = self.open(connection).await.map_err(|e| match e {
            ValidateError::Failed(message) => (
                RejectReason::NotSent,
                format!("Couldn't reach the service: {message}"),
            ),
            other => (
                RejectReason::Unavailable,
                format!("The connection can't be used: {other}"),
            ),
        })?;
        if runtime.is_blocked() {
            let _ = upstream
                .shutdown(crate::connectors::upstream::ShutdownReason::Revoked)
                .await;
            return Err((
                RejectReason::Unavailable,
                format!("The {} connection was revoked.", connection.slug),
            ));
        }
        *shared = Some(upstream.clone());
        drop(shared);
        self.follow(&connection.id, &upstream);
        Ok(upstream)
    }

    async fn redacted(&self, connection: &store::Connection, text: &str) -> String {
        // Without the auth there is no secret to look for; say nothing.
        self.redact(connection, text)
            .await
            .unwrap_or_else(|_| "the service's answer can't be shown".into())
    }
}

/// A refusal the agent reads as a failed tool call.
fn refusal(line: &str) -> CallOutput {
    CallOutput {
        content: vec![json!({ "type": "text", "text": line })],
        structured_content: None,
        is_error: true,
    }
}

/// The first text block of `output`, for the record.
fn first_text(output: &CallOutput) -> String {
    output
        .content
        .iter()
        .find_map(|block| block.get("text").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

/// A call's record while it runs. Dropped before it ends (the proxy hung
/// up, or the Host is stopping the task), it ends the record itself: a call
/// on its way may have run.
struct Record {
    db: DatabaseConnection,
    id: String,
    started: Instant,
    mark: DispatchMark,
    ended: AtomicBool,
}

impl Record {
    fn new(db: DatabaseConnection, id: String, started: Instant) -> Self {
        Self {
            db,
            id,
            started,
            mark: DispatchMark::default(),
            ended: AtomicBool::new(false),
        }
    }

    async fn end(&self, outcome: Outcome, detail: Option<&str>, upstream: Option<Duration>) {
        self.ended.store(true, Ordering::SeqCst);
        let total = self.started.elapsed();
        let timings = Timings {
            upstream_ms: upstream.map(millis),
            overhead_ms: Some(millis(total.saturating_sub(upstream.unwrap_or_default()))),
        };
        if let Err(e) = executions::finish(&self.db, &self.id, outcome, detail, timings).await {
            tracing::warn!(
                "[connectors] couldn't record the end of call {}: {e}",
                self.id
            );
        }
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        if self.ended.load(Ordering::SeqCst) {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (outcome, detail) = if self.mark.dispatched() {
            (
                Outcome::Unknown(UnknownCause::CancelledAfterDispatch),
                "The agent stopped waiting; the call may have run.",
            )
        } else {
            (
                Outcome::CancelledBeforeDispatch,
                "The agent stopped waiting before the call was sent.",
            )
        };
        let (db, id) = (self.db.clone(), std::mem::take(&mut self.id));
        handle.spawn(async move {
            if let Err(e) =
                executions::finish(&db, &id, outcome, Some(detail), Timings::default()).await
            {
                tracing::warn!("[connectors] couldn't record the end of call {id}: {e}");
            }
        });
    }
}

fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

fn unrecorded(e: impl std::fmt::Display) -> String {
    tracing::warn!("[connectors] gateway: couldn't record a call: {e}");
    "Zwiim couldn't record this call; nothing was sent.".into()
}

fn gone() -> String {
    "This connection no longer exists.".into()
}

fn internal(e: impl std::fmt::Display) -> String {
    tracing::warn!("[connectors] gateway: {e}");
    "Zwiim couldn't read this connection; nothing was sent.".into()
}

#[cfg(test)]
mod tests;
