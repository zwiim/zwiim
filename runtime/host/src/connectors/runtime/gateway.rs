//! What a connector proxy asks of a connection (design r3 §3.3): the tools
//! an agent sees, when they change, and calls. Every refusal is a line for
//! the agent, delivered as a tool error.
//!
//! Policy, approvals and execution records (r3 §5.1, §5.2, §4.3) come
//! before anything reaches this: no proxy token names a connection until
//! then.

use std::time::Duration;

use serde_json::{json, Map, Value};

use super::{Connectors, ValidateError};
use crate::connectors::store::{self, ConnectionStatus};
use crate::connectors::upstream::{
    CallFailure, CallOutput, DispatchMark, ToolDefinition, Upstream, UpstreamCall,
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

    /// Runs `tool` on connection `id`. Anything that keeps it from running,
    /// or leaves its outcome unknown, comes back as a tool error that says
    /// so. A call whose request left the Host is never retried.
    pub async fn call(&self, id: &str, tool: &str, arguments: Map<String, Value>) -> CallOutput {
        match self.try_call(id, tool, arguments).await {
            Ok(output) => output,
            Err(line) => refusal(&line),
        }
    }

    async fn try_call(
        &self,
        id: &str,
        tool: &str,
        arguments: Map<String, Value>,
    ) -> Result<CallOutput, String> {
        let connection = store::connection(&self.db, id)
            .await
            .map_err(internal)?
            .ok_or_else(gone)?;
        let runtime = self.runtime(id);
        if connection.status != ConnectionStatus::Active || runtime.is_blocked() {
            return Err(format!(
                "The {} connection is {}; nothing was sent.",
                connection.slug, connection.status
            ));
        }
        let tools = store::tools(&self.db, id).await.map_err(internal)?;
        if !tools.iter().any(|t| t.name == tool) {
            return Err(format!(
                "The {} connection has no tool named {tool:?}; nothing was sent.",
                connection.slug
            ));
        }
        let upstream = self.shared_upstream(&connection).await?;
        let dispatch = DispatchMark::default();
        let call = UpstreamCall {
            tool: tool.into(),
            arguments,
        };
        let unknown = || {
            format!(
                "This call may have run on {}. Check before trying again.",
                connection.slug
            )
        };
        match tokio::time::timeout(CALL_TIMEOUT, upstream.call(call, &dispatch)).await {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(CallFailure::NotSent(e))) => Err(format!(
                "The call wasn't sent: {}",
                self.redacted(&connection, &e.to_string()).await
            )),
            Ok(Err(CallFailure::Rejected { message, .. })) => Err(format!(
                "{} refused the call: {}",
                connection.slug,
                self.redacted(&connection, &message).await
            )),
            Ok(Err(CallFailure::OutcomeUnknown(_))) => Err(unknown()),
            Err(_) if dispatch.dispatched() => Err(unknown()),
            Err(_) => Err("The service didn't answer in time; nothing was sent.".into()),
        }
    }

    /// The connection's shared upstream, opened on first use (after a Host
    /// restart the connection is active but nothing is open yet).
    async fn shared_upstream(
        &self,
        connection: &store::Connection,
    ) -> Result<std::sync::Arc<dyn Upstream>, String> {
        let runtime = self.runtime(&connection.id);
        let mut shared = runtime.shared.lock().await;
        if let Some(upstream) = shared.as_ref() {
            return Ok(upstream.clone());
        }
        let upstream = self.open(connection).await.map_err(|e| match e {
            ValidateError::Failed(message) => format!("Couldn't reach the service: {message}"),
            other => format!("The connection can't be used: {other}"),
        })?;
        if runtime.is_blocked() {
            let _ = upstream
                .shutdown(crate::connectors::upstream::ShutdownReason::Revoked)
                .await;
            return Err(format!("The {} connection was revoked.", connection.slug));
        }
        *shared = Some(upstream.clone());
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

fn gone() -> String {
    "This connection no longer exists.".into()
}

fn internal(e: impl std::fmt::Display) -> String {
    tracing::warn!("[connectors] gateway: {e}");
    "Zwiim couldn't read this connection; nothing was sent.".into()
}

#[cfg(test)]
mod tests;
