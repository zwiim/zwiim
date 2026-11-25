//! Execution records (connector design r3 §4.3): one per tool call made
//! through a connection, written as the call moves, so the audit trail
//! survives a crash.
//!
//! ```text
//! received ──┬─> rejected                   nothing sent
//!            ├─> cancelled_before_dispatch  nothing sent
//!            └─> dispatched ──┬─> succeeded
//!                             ├─> tool_error
//!                             ├─> failed
//!                             ├─> outcome_unknown   may have run
//!                             └─> rejected          the adapter proved it wasn't sent
//! ```
//!
//! A record only moves forward: [`finish`] from a state it can't follow is a
//! no-op, so a late write never overwrites the real outcome. Arguments are
//! never stored, only a digest and their top-level keys.

use chrono::{DateTime, Utc};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, Set,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::upstream::{sha256_of, UnknownCause};
use crate::db::entities::connector_execution;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    Received,
    Rejected,
    AwaitingApproval,
    ApprovalDenied,
    ApprovalExpired,
    ApprovalVoid,
    CancelledBeforeDispatch,
    Dispatched,
    Succeeded,
    ToolError,
    Failed,
    OutcomeUnknown,
}

impl ExecutionState {
    /// Whether the call reached the service.
    pub fn was_sent(self) -> bool {
        matches!(
            self,
            Self::Dispatched
                | Self::Succeeded
                | Self::ToolError
                | Self::Failed
                | Self::OutcomeUnknown
        )
    }

    /// The states a record may move to this one from.
    fn follows(self) -> &'static [ExecutionState] {
        match self {
            Self::Rejected => &[Self::Received, Self::Dispatched],
            Self::CancelledBeforeDispatch => &[Self::Received],
            Self::Dispatched => &[Self::Received],
            Self::Succeeded | Self::ToolError | Self::Failed | Self::OutcomeUnknown => {
                &[Self::Dispatched]
            }
            // Written once approvals land.
            Self::Received
            | Self::AwaitingApproval
            | Self::ApprovalDenied
            | Self::ApprovalExpired
            | Self::ApprovalVoid => &[],
        }
    }
}

/// Why a call was refused before anything ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// The connection isn't active.
    Unavailable,
    /// The connection doesn't list the tool.
    NotExposed,
    /// The service couldn't be reached; nothing left the Host.
    NotSent,
}

/// How a call ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Rejected(RejectReason),
    CancelledBeforeDispatch,
    Succeeded,
    /// The service ran the tool and said it failed (`isError`).
    ToolError,
    /// The service answered with an error.
    Failed,
    Unknown(UnknownCause),
}

impl Outcome {
    fn state(&self) -> ExecutionState {
        match self {
            Self::Rejected(_) => ExecutionState::Rejected,
            Self::CancelledBeforeDispatch => ExecutionState::CancelledBeforeDispatch,
            Self::Succeeded => ExecutionState::Succeeded,
            Self::ToolError => ExecutionState::ToolError,
            Self::Failed => ExecutionState::Failed,
            Self::Unknown(_) => ExecutionState::OutcomeUnknown,
        }
    }

    fn reason(&self) -> Option<String> {
        match self {
            Self::Rejected(reason) => Some(name_of(reason)),
            Self::Unknown(cause) => Some(cause_name(*cause).into()),
            _ => None,
        }
    }
}

fn cause_name(cause: UnknownCause) -> &'static str {
    match cause {
        UnknownCause::Timeout => "timeout",
        UnknownCause::CancelledAfterDispatch => "cancelled_after_dispatch",
        UnknownCause::ConnectionLost => "connection_lost",
        UnknownCause::HostShutdown => "host_shutdown",
    }
}

/// Where the time went, in milliseconds (r3 §8.1). `upstream` is the service's
/// round trip; `overhead` is the rest of the call's time in the Host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timings {
    pub upstream_ms: Option<i64>,
    pub overhead_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Execution {
    pub id: String,
    pub connection_id: String,
    pub session_id: Option<String>,
    pub tool_name: String,
    pub schema_hash: Option<String>,
    pub state: ExecutionState,
    pub reason: Option<String>,
    pub detail: Option<String>,
    pub decided_by: Option<String>,
    pub args_digest: String,
    pub arg_keys: Vec<String>,
    pub received_at: DateTime<Utc>,
    pub dispatched_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub upstream_ms: Option<i64>,
    pub overhead_ms: Option<i64>,
}

/// A call as it arrives.
#[derive(Clone, Copy, Debug)]
pub struct NewCall<'a> {
    pub connection_id: &'a str,
    pub session_id: Option<&'a str>,
    pub tool_name: &'a str,
    /// The tool's hash, if the connection lists it.
    pub schema_hash: Option<&'a str>,
    pub arguments: &'a Map<String, Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("stored execution data is unreadable: {0}")]
    Corrupt(String),
    #[error("database: {0}")]
    Db(#[from] DbErr),
}

/// The most argument keys kept, and the longest; an agent picks them.
const MAX_KEYS: usize = 32;
const MAX_KEY_CHARS: usize = 64;
/// The longest detail kept.
const MAX_DETAIL_CHARS: usize = 500;

fn name_of<T: Serialize>(value: T) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(name)) => name,
        _ => unreachable!("unit variants serialize as strings"),
    }
}

fn parse<T: for<'de> Deserialize<'de>>(name: &str) -> Result<T, ExecutionError> {
    serde_json::from_value(Value::String(name.into()))
        .map_err(|_| ExecutionError::Corrupt(format!("state {name:?}")))
}

fn capped(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.into(),
    }
}

fn arg_keys(arguments: &Map<String, Value>) -> Vec<String> {
    let mut keys: Vec<String> = arguments
        .keys()
        .map(|key| capped(key, MAX_KEY_CHARS))
        .collect();
    keys.sort();
    keys.truncate(MAX_KEYS);
    keys
}

fn to_execution(row: connector_execution::Model) -> Result<Execution, ExecutionError> {
    Ok(Execution {
        state: parse(&row.state)?,
        arg_keys: serde_json::from_str(&row.arg_keys_json)
            .map_err(|e| ExecutionError::Corrupt(format!("argument keys: {e}")))?,
        id: row.id,
        connection_id: row.connection_id,
        session_id: row.session_id,
        tool_name: row.tool_name,
        schema_hash: row.schema_hash,
        reason: row.reason,
        detail: row.detail,
        decided_by: row.decided_by,
        args_digest: row.args_digest,
        received_at: row.received_at,
        dispatched_at: row.dispatched_at,
        finished_at: row.finished_at,
        upstream_ms: row.upstream_ms,
        overhead_ms: row.overhead_ms,
    })
}

/// Records a call as received.
pub async fn receive(
    conn: &DatabaseConnection,
    call: NewCall<'_>,
) -> Result<Execution, ExecutionError> {
    let keys = serde_json::to_string(&arg_keys(call.arguments)).expect("strings serialize");
    let row = connector_execution::ActiveModel {
        id: Set(uuid::Uuid::new_v4().to_string()),
        connection_id: Set(call.connection_id.into()),
        session_id: Set(call.session_id.map(Into::into)),
        tool_name: Set(call.tool_name.into()),
        schema_hash: Set(call.schema_hash.map(Into::into)),
        state: Set(name_of(ExecutionState::Received)),
        reason: Set(None),
        detail: Set(None),
        decided_by: Set(None),
        args_digest: Set(sha256_of(&Value::Object(call.arguments.clone()))),
        arg_keys_json: Set(keys),
        received_at: Set(Utc::now()),
        dispatched_at: Set(None),
        finished_at: Set(None),
        upstream_ms: Set(None),
        overhead_ms: Set(None),
    }
    .insert(conn)
    .await?;
    to_execution(row)
}

fn from_any(states: &[ExecutionState]) -> Condition {
    states.iter().fold(Condition::any(), |any, state| {
        any.add(connector_execution::Column::State.eq(name_of(*state)))
    })
}

/// Marks call `id` as reaching the service, just before it is sent. False if
/// it had already moved on.
pub async fn dispatch(conn: &DatabaseConnection, id: &str) -> Result<bool, ExecutionError> {
    let moved = connector_execution::Entity::update_many()
        .col_expr(
            connector_execution::Column::State,
            Expr::value(name_of(ExecutionState::Dispatched)),
        )
        .col_expr(
            connector_execution::Column::DispatchedAt,
            Expr::value(Some(Utc::now())),
        )
        .filter(connector_execution::Column::Id.eq(id))
        .filter(from_any(ExecutionState::Dispatched.follows()))
        .exec(conn)
        .await?;
    Ok(moved.rows_affected == 1)
}

/// Ends call `id` with `outcome`. `detail` is the line the agent was given,
/// already redacted. `None` if the record had already ended.
pub async fn finish(
    conn: &DatabaseConnection,
    id: &str,
    outcome: Outcome,
    detail: Option<&str>,
    timings: Timings,
) -> Result<Option<Execution>, ExecutionError> {
    let state = outcome.state();
    let mut update = connector_execution::Entity::update_many()
        .col_expr(
            connector_execution::Column::State,
            Expr::value(name_of(state)),
        )
        .col_expr(
            connector_execution::Column::Reason,
            Expr::value(outcome.reason()),
        )
        .col_expr(
            connector_execution::Column::Detail,
            Expr::value(detail.map(|d| capped(d, MAX_DETAIL_CHARS))),
        )
        .col_expr(
            connector_execution::Column::FinishedAt,
            Expr::value(Some(Utc::now())),
        )
        .col_expr(
            connector_execution::Column::UpstreamMs,
            Expr::value(timings.upstream_ms),
        )
        .col_expr(
            connector_execution::Column::OverheadMs,
            Expr::value(timings.overhead_ms),
        );
    if !state.was_sent() {
        // Proven not sent, even if it was marked as on its way.
        update = update.col_expr(
            connector_execution::Column::DispatchedAt,
            Expr::value(Option::<DateTime<Utc>>::None),
        );
    }
    let moved = update
        .filter(connector_execution::Column::Id.eq(id))
        .filter(from_any(state.follows()))
        .exec(conn)
        .await?;
    if moved.rows_affected == 0 {
        return Ok(None);
    }
    execution(conn, id).await
}

pub async fn execution(
    conn: &DatabaseConnection,
    id: &str,
) -> Result<Option<Execution>, ExecutionError> {
    connector_execution::Entity::find_by_id(id.to_string())
        .one(conn)
        .await?
        .map(to_execution)
        .transpose()
}

/// Connection `connection_id`'s calls, newest first: `limit` of them, older
/// than call `before` if given.
pub async fn executions(
    conn: &DatabaseConnection,
    connection_id: &str,
    limit: u64,
    before: Option<&str>,
) -> Result<Vec<Execution>, ExecutionError> {
    let mut query = connector_execution::Entity::find()
        .filter(connector_execution::Column::ConnectionId.eq(connection_id));
    if let Some(before) = before {
        let Some(cursor) = execution(conn, before).await? else {
            return Ok(Vec::new());
        };
        query = query.filter(
            Condition::any()
                .add(connector_execution::Column::ReceivedAt.lt(cursor.received_at))
                .add(
                    Condition::all()
                        .add(connector_execution::Column::ReceivedAt.eq(cursor.received_at))
                        .add(connector_execution::Column::Id.lt(cursor.id)),
                ),
        );
    }
    query
        .order_by_desc(connector_execution::Column::ReceivedAt)
        .order_by_desc(connector_execution::Column::Id)
        .limit(limit)
        .all(conn)
        .await?
        .into_iter()
        .map(to_execution)
        .collect()
}

/// At boot, ends every call the last Host left open: one never sent was
/// cancelled, one sent may have run. Returns how many it ended.
pub async fn sweep(conn: &DatabaseConnection) -> Result<u64, ExecutionError> {
    let unfinished = connector_execution::Entity::find()
        .filter(from_any(&[
            ExecutionState::Received,
            ExecutionState::Dispatched,
        ]))
        .all(conn)
        .await?;
    let mut ended = 0;
    for row in unfinished {
        let outcome = if row.dispatched_at.is_some() {
            Outcome::Unknown(UnknownCause::HostShutdown)
        } else {
            Outcome::CancelledBeforeDispatch
        };
        let detail = if row.dispatched_at.is_some() {
            "Zwiim stopped while the call was running; it may have run."
        } else {
            "Zwiim stopped before the call was sent."
        };
        if finish(conn, &row.id, outcome, Some(detail), Timings::default())
            .await?
            .is_some()
        {
            ended += 1;
        }
    }
    Ok(ended)
}

#[cfg(test)]
mod tests;
