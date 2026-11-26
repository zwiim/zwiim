//! What a connector proxy asks of a connector (design r3 §3.3): the tools
//! an agent sees, when they change, and calls. Every refusal is a line for
//! the agent, delivered as a tool error.
//!
//! Every call is recorded (r3 §4.3), and decided by policy, asking the
//! user where it says to (r3 §5.1, §5.2). A session gets a proxy only for
//! the connectors it is offered.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sea_orm::DatabaseConnection;
use serde_json::{json, Map, Value};
use tokio::sync::broadcast;

use super::ask::{store_failed, CallSession, Refusal};
use super::{Connectors, ValidateError};
use crate::connectors::executions::{
    self, Execution, ExecutionError, NewCall, Outcome, RejectReason, Timings,
};
use crate::connectors::grants;
use crate::connectors::policy::{self, AllowedBy, Decision, DenyReason, Facts, ToolState};
use crate::connectors::store::{self, ConnectorStatus};
use crate::connectors::upstream::{
    CallFailure, CallOutput, DispatchMark, ToolDefinition, UnknownCause, Upstream, UpstreamCall,
};

/// How long one call may take before its outcome is unknown.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

impl Connectors {
    /// Runs `tool` on connector `id`, with a record of the call (r3 §4.3).
    /// Anything that keeps it from running, or leaves its outcome unknown,
    /// comes back as a tool error that says so. A call whose request left
    /// the Host is never retried.
    pub async fn call(
        &self,
        session: &CallSession,
        id: &str,
        tool: &str,
        arguments: Map<String, Value>,
    ) -> CallOutput {
        let started = Instant::now();
        let connector = match store::connector(&self.db, id).await {
            Ok(Some(connector)) => connector,
            Ok(None) => return refusal(&gone()),
            Err(e) => return refusal(&internal(e)),
        };
        let tools = match store::listed(&self.db, id).await {
            Ok(tools) => tools,
            Err(e) => return refusal(&internal(e)),
        };
        let listed = tools.iter().find(|t| t.tool.name == tool);
        let hash = listed.map(|t| t.tool.hash());
        let record = executions::receive(
            &self.db,
            NewCall {
                connector_id: id,
                session_id: Some(&session.session_id),
                tool_name: tool,
                schema_hash: hash.as_deref(),
                arguments: &arguments,
            },
        )
        .await;
        let record = match record {
            Ok(record) => {
                let id = record.id.clone();
                let _ = self.executions.send(record);
                Record::new(self.db.clone(), self.executions.clone(), id, started)
            }
            Err(e) => return refusal(&unrecorded(e)),
        };
        let (outcome, upstream_ms, output) = self
            .run(session, &connector, listed, tool, arguments, &record)
            .await;
        let detail = output.is_error.then(|| first_text(&output));
        record.end(outcome, detail.as_deref(), upstream_ms).await;
        output
    }

    /// Everything between receiving a call and its outcome: the outcome,
    /// the service's round trip, and what the agent is given.
    async fn run(
        &self,
        session: &CallSession,
        connector: &store::Connector,
        listed: Option<&store::ListedTool>,
        tool: &str,
        arguments: Map<String, Value>,
        record: &Record,
    ) -> (Outcome, Option<Duration>, CallOutput) {
        let rejected = |reason, line: String| (Outcome::Rejected(reason), None, refusal(&line));
        if connector.status != ConnectorStatus::Active || self.runtime(&connector.id).is_blocked() {
            return rejected(
                RejectReason::Unavailable,
                format!(
                    "The {} connector is {}; nothing was sent.",
                    connector.slug, connector.status
                ),
            );
        }
        let Some(listed) = listed else {
            return rejected(
                RejectReason::NotExposed,
                format!(
                    "The {} connector has no tool named {tool:?}; nothing was sent.",
                    connector.slug
                ),
            );
        };
        if !listed.enabled {
            return rejected(
                RejectReason::NotExposed,
                format!(
                    "The {} connector's {tool:?} tool is turned off; nothing was sent.",
                    connector.slug
                ),
            );
        }
        let listed = &listed.tool;
        let decided_by = match self
            .permit(session, connector, listed, &arguments, &record.id)
            .await
        {
            Ok(by) => by,
            Err((outcome, line)) => return (outcome, None, refusal(&line)),
        };
        if let Err(e) = executions::decided(&self.db, &record.id, &decided_by).await {
            return rejected(RejectReason::NotSent, unrecorded(e));
        }
        let upstream = match self.shared_upstream(connector).await {
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
                    connector.slug
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
                    self.redacted(connector, &e.to_string()).await
                )),
            ),
            Ok(Err(CallFailure::Rejected { message, .. })) => (
                Outcome::Failed,
                refusal(&format!(
                    "{} refused the call: {}",
                    connector.slug,
                    self.redacted(connector, &message).await
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

    /// Whether the call may run (r3 §5.1), asking the user if policy says
    /// to. Returns who let it run.
    async fn permit(
        &self,
        session: &CallSession,
        connector: &store::Connector,
        tool: &ToolDefinition,
        arguments: &Map<String, Value>,
        execution_id: &str,
    ) -> Result<String, Refusal> {
        let hash = tool.hash();
        let first_seen = store::first_seen(&self.db, &connector.id, &tool.name)
            .await
            .map_err(store_failed)?;
        let grant = grants::grant_for(
            &self.db,
            &connector.id,
            session.workspace_id.as_deref(),
            &session.agent,
        )
        .await
        .map_err(store_failed)?;
        let allowed = self
            .asking
            .session_allowed(&session.session_id, &connector.id, &tool.name);
        let decision = policy::decide(Facts {
            connector_active: true,
            engine_session: session.engine,
            tool: first_seen.map(|first_seen_at| ToolState {
                name: &tool.name,
                hash: &hash,
                first_seen_at,
            }),
            grant: grant.as_ref(),
            session_allowed: allowed.as_deref(),
        });
        match decision {
            Decision::Allow(AllowedBy::ToolRow) => Ok("policy".into()),
            Decision::Allow(AllowedBy::Session) => Ok("session_grant".into()),
            Decision::Deny(reason) => {
                let _ = executions::decided(&self.db, execution_id, "policy").await;
                let rejected = match reason {
                    DenyReason::Unavailable => RejectReason::Unavailable,
                    DenyReason::NotExposed => RejectReason::NotExposed,
                    _ => RejectReason::Denied,
                };
                Err((Outcome::Rejected(rejected), reason.line().into()))
            }
            Decision::Ask(_) => {
                let grant = grant.expect("asking needs a grant");
                self.ask(
                    session,
                    connector,
                    tool,
                    &hash,
                    grant.id,
                    arguments,
                    execution_id,
                )
                .await
            }
        }
    }

    /// At boot, before any call: ends the calls the last Host left open,
    /// and leaves none of its approvals usable.
    pub async fn end_unfinished_calls(&self) {
        if let Err(e) = crate::connectors::approvals::sweep(&self.db, chrono::Utc::now()).await {
            tracing::warn!("[connectors] couldn't end unfinished approvals: {e}");
        }
        match executions::sweep(&self.db).await {
            Ok(0) => {}
            Ok(n) => tracing::info!("[connectors] ended {n} calls the last Host left open"),
            Err(e) => tracing::warn!("[connectors] couldn't end unfinished calls: {e}"),
        }
    }

    /// The connector's shared upstream, opened on first use (after a Host
    /// restart the connector is active but nothing is open yet).
    async fn shared_upstream(
        &self,
        connector: &store::Connector,
    ) -> Result<std::sync::Arc<dyn Upstream>, (RejectReason, String)> {
        let runtime = self.runtime(&connector.id);
        let mut shared = runtime.shared.lock().await;
        if let Some(upstream) = shared.as_ref() {
            return Ok(upstream.clone());
        }
        let upstream = self.open(connector).await.map_err(|e| match e {
            ValidateError::Failed(message) => (
                RejectReason::NotSent,
                format!("Couldn't reach the service: {message}"),
            ),
            other => (
                RejectReason::Unavailable,
                format!("The connector can't be used: {other}"),
            ),
        })?;
        if runtime.is_blocked() {
            let _ = upstream
                .shutdown(crate::connectors::upstream::ShutdownReason::Revoked)
                .await;
            return Err((
                RejectReason::Unavailable,
                format!("The {} connector was revoked.", connector.slug),
            ));
        }
        *shared = Some(upstream.clone());
        drop(shared);
        self.follow(&connector.id, &upstream);
        Ok(upstream)
    }

    async fn redacted(&self, connector: &store::Connector, text: &str) -> String {
        // Without the auth there is no secret to look for; say nothing.
        self.redact(connector, text)
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
    events: broadcast::Sender<Execution>,
    id: String,
    started: Instant,
    mark: DispatchMark,
    ended: AtomicBool,
}

impl Record {
    fn new(
        db: DatabaseConnection,
        events: broadcast::Sender<Execution>,
        id: String,
        started: Instant,
    ) -> Self {
        Self {
            db,
            events,
            id,
            started,
            mark: DispatchMark::default(),
            ended: AtomicBool::new(false),
        }
    }

    async fn end(&self, outcome: Outcome, detail: Option<&str>, upstream: Option<Duration>) {
        let total = self.started.elapsed();
        let timings = Timings {
            upstream_ms: upstream.map(millis),
            overhead_ms: Some(millis(total.saturating_sub(upstream.unwrap_or_default()))),
        };
        let result = executions::finish(&self.db, &self.id, outcome, detail, timings).await;
        // Only now: dropped during the write, the record still ends itself,
        // and a record that already ended keeps its outcome.
        self.ended.store(true, Ordering::SeqCst);
        ended(&self.events, &self.id, result);
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
        let (db, events, id) = (
            self.db.clone(),
            self.events.clone(),
            std::mem::take(&mut self.id),
        );
        handle.spawn(async move {
            let result =
                executions::finish(&db, &id, outcome, Some(detail), Timings::default()).await;
            ended(&events, &id, result);
        });
    }
}

/// Tells listeners how call `id` ended.
fn ended(
    events: &broadcast::Sender<Execution>,
    id: &str,
    result: Result<Option<Execution>, ExecutionError>,
) {
    match result {
        Ok(Some(execution)) => {
            let _ = events.send(execution);
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("[connectors] couldn't record the end of call {id}: {e}"),
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
    "This connector no longer exists.".into()
}

fn internal(e: impl std::fmt::Display) -> String {
    tracing::warn!("[connectors] gateway: {e}");
    "Zwiim couldn't read this connector; nothing was sent.".into()
}

#[cfg(test)]
mod tests;
