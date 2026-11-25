//! Connector approval cards on an agent session (connector design r3 §5.2).
//!
//! Each session shows one card at a time: the oldest waiting call's. Others
//! queue behind it (`ConnectorApprovalQueued`, not sent to clients) and the
//! next is shown when the head comes down. The approval rows decide; these
//! events only show them, and carry the cards on the session snapshot for a
//! client that attaches mid-wait.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::DatabaseConnection;

use crate::acp::manager::ConnectionManager;
use crate::acp::types::AcpEvent;
use crate::connectors::runtime::{ApprovalCard, ApprovalCards, CallSession};
use crate::web::event_bridge::emit_with_state;

/// Show and clear one at a time, so two calls never both take the head.
static CARDS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The cards of the session agent connection `connection_id` serves.
pub struct SessionCards {
    manager: ConnectionManager,
    connection_id: String,
}

#[async_trait]
impl ApprovalCards for SessionCards {
    async fn show(&self, card: ApprovalCard) -> bool {
        let _one = CARDS.lock().await;
        let Some((state, emitter)) = self
            .manager
            .get_state_and_emitter(&self.connection_id)
            .await
        else {
            return false;
        };
        let first = state.read().await.pending_connector_approvals.is_empty();
        let event = if first {
            AcpEvent::ConnectorApprovalRequest { card }
        } else {
            AcpEvent::ConnectorApprovalQueued { card }
        };
        emit_with_state(&state, &emitter, event).await;
        true
    }

    async fn clear(&self, approval_id: &str) {
        let _one = CARDS.lock().await;
        let Some((state, emitter)) = self
            .manager
            .get_state_and_emitter(&self.connection_id)
            .await
        else {
            return;
        };
        let (was_shown, next) = {
            let s = state.read().await;
            let cards = &s.pending_connector_approvals;
            if !cards.iter().any(|c| c.approval_id == approval_id) {
                return;
            }
            let shown = cards.first().is_some_and(|c| c.approval_id == approval_id);
            (shown, cards.get(1).cloned())
        };
        emit_with_state(
            &state,
            &emitter,
            AcpEvent::ConnectorApprovalResolved {
                approval_id: approval_id.into(),
            },
        )
        .await;
        if let (true, Some(card)) = (was_shown, next) {
            emit_with_state(
                &state,
                &emitter,
                AcpEvent::ConnectorApprovalRequest { card },
            )
            .await;
        }
    }
}

/// Who a connector proxy's session is, from the Host's own state. Set on
/// the delegation listener at startup.
pub struct ConnectorSessions {
    db: DatabaseConnection,
    manager: ConnectionManager,
}

impl ConnectorSessions {
    pub fn new(db: DatabaseConnection, manager: ConnectionManager) -> Self {
        Self { db, manager }
    }
}

#[async_trait]
impl crate::acp::delegation::listener::ConnectorSessionLookup for ConnectorSessions {
    async fn session(
        &self,
        parent_connection_id: &str,
        working_dir: &Path,
        engine: bool,
    ) -> Option<CallSession> {
        let state = self.manager.get_state(parent_connection_id).await?;
        let agent = match serde_json::to_value(state.read().await.agent_type) {
            Ok(serde_json::Value::String(name)) => name,
            _ => return None,
        };
        let workspace_id =
            crate::workspace::root_workspace_id_for_dir(&self.db, &working_dir.to_string_lossy())
                .await
                .ok()
                .flatten();
        Some(CallSession {
            session_id: parent_connection_id.into(),
            workspace_id,
            agent,
            engine,
            cards: Arc::new(SessionCards {
                manager: self.manager.clone_ref(),
                connection_id: parent_connection_id.into(),
            }),
        })
    }
}

#[cfg(test)]
mod tests;
