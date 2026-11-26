//! Asking the user about one call (connector design r3 §5.1 answers, §5.2
//! lifecycle). The call shows a card in its session and waits; whoever
//! answers moves the approval row, and the call reads its fate from the row.
//!
//! ```text
//! call ── create pending ── show card ── wait ──┬─ approved ── re-check ── consume ── send
//!                                               ├─ denied / expired / cancelled / void
//!                                               └─ proxy hung up ── cancelled
//! ```
//!
//! After an approval the call checks again that the connector is active,
//! a grant still applies and the tool is the one approved; anything else
//! voids the approval and nothing is sent.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::Notify;

use super::Connectors;
use crate::connectors::approvals::{
    self, Approval, ApprovalError, ApprovalStatus, NewApproval, Risk, VoidReason,
};
use crate::connectors::executions::{self, Outcome, RejectReason};
use crate::connectors::grants::{self, GrantError, ToolPolicy};
use crate::connectors::store::{self, ConnectorStatus};
use crate::connectors::upstream::ToolDefinition;

/// Who is calling. Built by the Host from the proxy's token, never from
/// anything the proxy or the agent sends.
#[derive(Clone)]
pub struct CallSession {
    /// The session the proxy serves.
    pub session_id: String,
    /// Its root workspace; `None` outside every workspace.
    pub workspace_id: Option<String>,
    /// The protocol's agent name, e.g. `claude_code`.
    pub agent: String,
    /// An automation engine session: connectors are refused.
    pub engine: bool,
    /// Where its approval cards go.
    pub cards: Arc<dyn ApprovalCards>,
}

/// What the user is asked about.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalCard {
    pub approval_id: String,
    /// An answer must carry it back.
    pub binding_hash: String,
    /// The connector's label, e.g. `Work`.
    pub label: String,
    /// The server name agents see, e.g. `linear_work`.
    pub slug: String,
    pub tool: String,
    pub tool_title: Option<String>,
    /// What the tool says of itself; never decides anything.
    pub risk: Risk,
    /// The arguments, redacted and short.
    pub preview: String,
    pub expires_at: DateTime<Utc>,
}

/// A session's approval cards. The production impl puts them on the session
/// every client sees.
#[async_trait]
pub trait ApprovalCards: Send + Sync {
    /// Shows `card`. False if the session is gone: no one can answer.
    async fn show(&self, card: ApprovalCard) -> bool;
    /// Takes card `approval_id` down on every client.
    async fn clear(&self, approval_id: &str);
}

/// No one to ask: every call that asks is refused.
pub struct NoCards;

#[async_trait]
impl ApprovalCards for NoCards {
    async fn show(&self, _card: ApprovalCard) -> bool {
        false
    }

    async fn clear(&self, _approval_id: &str) {}
}

/// The user's answer to a card (r3 §5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalAnswer {
    /// This call only.
    Once,
    /// This tool, as it is now, for the rest of the session.
    Session,
    /// This tool, as it is now, from now on: an `allow` row in the grant.
    Always,
    /// This call only.
    Deny,
    /// This tool from now on: a `deny` row in the grant.
    AlwaysDeny,
}

impl ApprovalAnswer {
    pub fn approves(self) -> bool {
        matches!(self, Self::Once | Self::Session | Self::Always)
    }

    /// Changes a grant, which only the Host itself may do.
    pub fn changes_grant(self) -> bool {
        matches!(self, Self::Always | Self::AlwaysDeny)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AnswerError {
    #[error("no such approval")]
    NotFound,
    #[error("the answer is for another call")]
    Mismatch,
    #[error("already answered")]
    AlreadyResolved {
        status: ApprovalStatus,
        decided_by: Option<String>,
    },
    #[error("the approval expired")]
    Expired,
    #[error("store: {0}")]
    Store(String),
}

impl From<ApprovalError> for AnswerError {
    fn from(e: ApprovalError) -> Self {
        match e {
            ApprovalError::NotFound(_) => Self::NotFound,
            ApprovalError::Mismatch => Self::Mismatch,
            ApprovalError::AlreadyResolved { status, decided_by } => {
                Self::AlreadyResolved { status, decided_by }
            }
            ApprovalError::Expired => Self::Expired,
            other => Self::Store(other.to_string()),
        }
    }
}

/// A call waiting for its answer.
pub(super) struct Waiter {
    grant_id: i32,
    wake: Arc<Notify>,
}

/// What is waiting, and what sessions allowed.
#[derive(Default)]
pub(super) struct Asking {
    waiters: std::sync::Mutex<HashMap<String, Waiter>>,
    /// (session, connector, tool) → the tool hash it was allowed at.
    session_allows: std::sync::Mutex<HashMap<(String, String, String), String>>,
}

impl Asking {
    pub(super) fn session_allowed(
        &self,
        session: &str,
        connector: &str,
        tool: &str,
    ) -> Option<String> {
        self.session_allows
            .lock()
            .unwrap()
            .get(&(session.into(), connector.into(), tool.into()))
            .cloned()
    }

    fn wake(&self, approval_id: &str) {
        if let Some(waiter) = self.waiters.lock().unwrap().get(approval_id) {
            waiter.wake.notify_one();
        }
    }
}

/// A refused or ended call: its outcome and the agent's line.
pub(super) type Refusal = (Outcome, String);

/// Takes an unanswered approval down if its call is dropped (the proxy hung
/// up): the approval is cancelled and the card cleared.
struct Pending {
    db: sea_orm::DatabaseConnection,
    asking: Arc<Asking>,
    approval_id: String,
    cards: Arc<dyn ApprovalCards>,
    armed: bool,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.asking
            .waiters
            .lock()
            .unwrap()
            .remove(&self.approval_id);
        if !self.armed {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (db, id, cards) = (
            self.db.clone(),
            std::mem::take(&mut self.approval_id),
            self.cards.clone(),
        );
        handle.spawn(async move {
            let _ = approvals::end(&db, &id, ApprovalStatus::Cancelled, None, Utc::now()).await;
            cards.clear(&id).await;
        });
    }
}

impl Connectors {
    /// Asks the user about the call recorded as `execution_id` and waits.
    /// Returns who approved it, with the approval consumed; or why it may
    /// not run.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn ask(
        &self,
        session: &CallSession,
        connector: &store::Connector,
        tool: &ToolDefinition,
        hash: &str,
        grant_id: i32,
        arguments: &Map<String, Value>,
        execution_id: &str,
    ) -> Result<String, Refusal> {
        let nothing_sent = |line: &str| line.to_string();
        let stopped = || {
            (
                Outcome::CancelledBeforeDispatch,
                nothing_sent("The call was stopped; nothing was sent."),
            )
        };
        match executions::await_approval(&self.db, execution_id).await {
            Ok(true) => {}
            Ok(false) => return Err(stopped()),
            Err(e) => return Err(store_failed(e)),
        }
        let shown =
            serde_json::to_string_pretty(&Value::Object(arguments.clone())).unwrap_or_default();
        let preview = match self.redact(connector, &shown).await {
            Ok(text) => crate::redact::scrub_secrets(&text).into_owned(),
            Err(e) => return Err(store_failed(e)),
        };
        let risk = Risk::of(tool.annotations.as_ref());
        let created = approvals::create(
            &self.db,
            NewApproval {
                execution_id,
                session_id: &session.session_id,
                connector_id: &connector.id,
                tool_name: &tool.name,
                schema_hash: hash,
                arguments,
                risk,
                preview: &preview,
            },
            Utc::now(),
        )
        .await;
        let approval = match created {
            Ok(approval) => approval,
            Err(e) => return Err(store_failed(e)),
        };
        let wake = Arc::new(Notify::new());
        self.asking.waiters.lock().unwrap().insert(
            approval.id.clone(),
            Waiter {
                grant_id,
                wake: wake.clone(),
            },
        );
        let mut pending = Pending {
            db: self.db.clone(),
            asking: self.asking.clone(),
            approval_id: approval.id.clone(),
            cards: session.cards.clone(),
            armed: true,
        };
        let card = ApprovalCard {
            approval_id: approval.id.clone(),
            binding_hash: approval.binding_hash.clone(),
            label: connector.label.clone(),
            slug: connector.slug.clone(),
            tool: tool.name.clone(),
            tool_title: tool.title.clone(),
            risk,
            preview: approval.preview.clone().unwrap_or_default(),
            expires_at: approval.expires_at,
        };
        if !session.cards.show(card).await {
            return Err((
                Outcome::CancelledBeforeDispatch,
                nothing_sent("No one can answer for this session; nothing was sent."),
            ));
        }
        let answered = self.wait(&approval, &wake).await;
        pending.armed = false;
        drop(pending);
        session.cards.clear(&approval.id).await;
        let answered = answered?;
        if let Some(reason) = self
            .changed_since_approval(session, connector, &answered)
            .await
        {
            let _ = approvals::end(
                &self.db,
                &answered.id,
                ApprovalStatus::Void,
                Some(reason),
                Utc::now(),
            )
            .await;
            return Err(void(reason));
        }
        match approvals::consume(&self.db, &answered.id, Utc::now()).await {
            Ok(true) => Ok(answered.decided_by.unwrap_or_else(|| "host".into())),
            Ok(false) => Err(void(VoidReason::Revoked)),
            Err(e) => Err(store_failed(e)),
        }
    }

    /// Waits until approval `approval` is answered or ends.
    async fn wait(&self, approval: &Approval, wake: &Notify) -> Result<Approval, Refusal> {
        loop {
            let woken = wake.notified();
            tokio::pin!(woken);
            woken.as_mut().enable();
            let current = match approvals::approval(&self.db, &approval.id).await {
                Ok(Some(current)) => current,
                Ok(None) => return Err(void(VoidReason::Revoked)),
                Err(e) => return Err(store_failed(e)),
            };
            match current.status {
                ApprovalStatus::Pending if Utc::now() >= current.expires_at => {
                    let _ = approvals::end(
                        &self.db,
                        &current.id,
                        ApprovalStatus::Expired,
                        None,
                        Utc::now(),
                    )
                    .await;
                    continue;
                }
                ApprovalStatus::Pending => {}
                ApprovalStatus::Approved => return Ok(current),
                ApprovalStatus::Denied => {
                    return Err((
                        Outcome::ApprovalDenied,
                        "The user denied this call; nothing was sent.".into(),
                    ))
                }
                ApprovalStatus::Expired => {
                    return Err((
                        Outcome::ApprovalExpired,
                        "No one answered in time; nothing was sent.".into(),
                    ))
                }
                ApprovalStatus::Cancelled => {
                    return Err((
                        Outcome::Rejected(RejectReason::Unavailable),
                        "The connector was revoked; nothing was sent.".into(),
                    ))
                }
                ApprovalStatus::Void | ApprovalStatus::Consumed => {
                    return Err(void(current.void_reason.unwrap_or(VoidReason::Revoked)))
                }
            }
            let left = (current.expires_at - Utc::now())
                .to_std()
                .unwrap_or_default();
            tokio::select! {
                _ = woken => {}
                _ = tokio::time::sleep(left) => {}
            }
        }
    }

    /// Why an approved call may no longer run, if it may not (r3 §5.2 fail
    /// closed).
    async fn changed_since_approval(
        &self,
        session: &CallSession,
        connector: &store::Connector,
        approval: &Approval,
    ) -> Option<VoidReason> {
        let Ok(Some(now)) = store::connector(&self.db, &connector.id).await else {
            return Some(VoidReason::Unavailable);
        };
        if self.runtime(&connector.id).is_blocked() {
            return Some(VoidReason::Revoked);
        }
        if now.status != ConnectorStatus::Active {
            return Some(VoidReason::Unavailable);
        }
        let grant = grants::grant_for(
            &self.db,
            &connector.id,
            session.workspace_id.as_deref(),
            &session.agent,
        )
        .await;
        if !matches!(grant, Ok(Some(_))) {
            return Some(VoidReason::GrantRemoved);
        }
        let current = match store::tools(&self.db, &connector.id).await {
            Ok(tools) => tools
                .into_iter()
                .find(|t| t.name == approval.tool_name)
                .map(|t| t.hash()),
            Err(_) => None,
        };
        (current.as_deref() != Some(approval.schema_hash.as_str()))
            .then_some(VoidReason::ToolChanged)
    }

    /// The user's answer to approval `approval_id` on session `session_id`.
    /// `decided_by` is `host` or `device:<id>`. A grant-changing answer must
    /// be checked as allowed by the caller.
    pub async fn answer_approval(
        &self,
        session_id: &str,
        approval_id: &str,
        binding: &str,
        answer: ApprovalAnswer,
        decided_by: &str,
    ) -> Result<(), AnswerError> {
        let found = approvals::approval(&self.db, approval_id).await?;
        let approval = match found {
            Some(approval) if approval.session_id == session_id => approval,
            _ => return Err(AnswerError::NotFound),
        };
        approvals::answer(
            &self.db,
            approval_id,
            binding,
            answer.approves(),
            decided_by,
            Utc::now(),
        )
        .await?;
        let grant_id = self
            .asking
            .waiters
            .lock()
            .unwrap()
            .get(approval_id)
            .map(|w| w.grant_id);
        match (answer, grant_id) {
            (ApprovalAnswer::Session, _) => {
                self.asking.session_allows.lock().unwrap().insert(
                    (
                        approval.session_id.clone(),
                        approval.connector_id.clone(),
                        approval.tool_name.clone(),
                    ),
                    approval.schema_hash.clone(),
                );
            }
            (ApprovalAnswer::Always | ApprovalAnswer::AlwaysDeny, Some(grant_id)) => {
                let policy = if answer == ApprovalAnswer::Always {
                    ToolPolicy::Allow
                } else {
                    ToolPolicy::Deny
                };
                match grants::set_tool_policy(&self.db, grant_id, &approval.tool_name, Some(policy))
                    .await
                {
                    Ok(_) => self.exposure_changed(),
                    // The answer itself stands; the row can be set again.
                    Err(e) => log_grant(&approval, e),
                }
            }
            _ => {}
        }
        self.asking.wake(approval_id);
        Ok(())
    }

    /// Session `session_id` ended: what it allowed for itself goes with it.
    pub fn end_session(&self, session_id: &str) {
        self.asking
            .session_allows
            .lock()
            .unwrap()
            .retain(|(session, _, _), _| session != session_id);
    }

    /// Connector `id` is being revoked: what waits is cancelled, what was
    /// approved is void, and the calls waiting on them are told.
    pub(super) async fn end_approvals_for(&self, id: &str) {
        match approvals::end_for_connector(&self.db, id, Utc::now()).await {
            Ok(ended) => ended.iter().for_each(|a| self.asking.wake(&a.id)),
            Err(e) => tracing::warn!("[connectors] couldn't end approvals of {id}: {e}"),
        }
        self.asking
            .session_allows
            .lock()
            .unwrap()
            .retain(|(_, connector, _), _| connector != id);
    }

    /// Expires every approval past its time and tells its call.
    pub async fn expire_approvals(&self) {
        match approvals::expire_due(&self.db, Utc::now()).await {
            Ok(ended) => ended.iter().for_each(|a| self.asking.wake(&a.id)),
            Err(e) => tracing::warn!("[connectors] couldn't expire approvals: {e}"),
        }
    }

    /// Expires overdue approvals every 15 s (r3 §5.2). Each waiting call
    /// also checks its own expiry, so nothing depends on this timing.
    pub fn spawn_approval_sweeper(self: &Arc<Self>) {
        let connectors = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(15));
            loop {
                tick.tick().await;
                let Some(connectors) = connectors.upgrade() else {
                    return;
                };
                connectors.expire_approvals().await;
            }
        });
    }
}

fn void(reason: VoidReason) -> Refusal {
    let line = match reason {
        VoidReason::ToolChanged => "The tool changed after it was approved; nothing was sent.",
        VoidReason::GrantRemoved => {
            "This session may no longer use this connector; nothing was sent."
        }
        VoidReason::Revoked => "The connector was revoked; nothing was sent.",
        VoidReason::Unavailable => "The connector is unavailable; nothing was sent.",
        VoidReason::HostRestart => "Zwiim restarted before the call was sent; nothing was sent.",
    };
    (Outcome::ApprovalVoid(reason), line.into())
}

pub(super) fn store_failed(e: impl std::fmt::Display) -> Refusal {
    tracing::warn!("[connectors] couldn't ask about a call: {e}");
    (
        Outcome::CancelledBeforeDispatch,
        "Zwiim couldn't ask about this call; nothing was sent.".into(),
    )
}

fn log_grant(approval: &Approval, e: GrantError) {
    tracing::warn!(
        "[connectors] couldn't save the answer for {} as a rule: {e}",
        approval.tool_name
    );
}
