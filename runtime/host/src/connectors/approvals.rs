//! Approvals (connector design r3 §5.2): the user's answer to one connector
//! call that policy said to ask about. The row decides; a card only shows it.
//!
//! ```text
//! pending ──┬─> approved ──┬─> consumed   the call is sent, once
//!           │              └─> void       fail-closed re-check, revocation, restart
//!           ├─> denied
//!           ├─> expired                   expires_at passed, or restart
//!           └─> cancelled                 the agent stopped waiting, or revocation
//! ```
//!
//! Every move is a compare-and-set on `status`, so the first answer wins and
//! an approval is consumed at most once. An answer must carry the approval's
//! `binding_hash`, and only lands before `expires_at`, which is fixed when
//! the approval is made.

use chrono::{DateTime, Duration, Utc};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait, QueryFilter,
    Set,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::upstream::sha256_of;
use crate::db::entities::connector_approval;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Consumed,
    Denied,
    Expired,
    Cancelled,
    Void,
}

impl ApprovalStatus {
    /// The states an approval may move to this one from.
    fn follows(self) -> &'static [ApprovalStatus] {
        match self {
            Self::Approved | Self::Denied | Self::Expired | Self::Cancelled => &[Self::Pending],
            Self::Consumed | Self::Void => &[Self::Approved],
            Self::Pending => &[],
        }
    }
}

/// What a tool says of itself, as it was when the approval was made. Only
/// the expiry reads it; it never allows anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    Read,
    Write,
    Destructive,
    Unknown,
}

impl Risk {
    /// From a tool's MCP annotations: only an explicit hint counts.
    pub fn of(annotations: Option<&Value>) -> Self {
        let hint = |name: &str| {
            annotations
                .and_then(|a| a.get(name))
                .and_then(Value::as_bool)
        };
        match (hint("readOnlyHint"), hint("destructiveHint")) {
            (Some(true), _) => Self::Read,
            (_, Some(true)) => Self::Destructive,
            (_, Some(false)) => Self::Write,
            _ => Self::Unknown,
        }
    }

    /// How long an approval waits for an answer (r3 §5.2).
    pub fn expiry(self) -> Duration {
        match self {
            Self::Destructive => Duration::minutes(5),
            _ => Duration::minutes(10),
        }
    }
}

/// Why an approved call can't be sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoidReason {
    /// The connector was revoked.
    Revoked,
    /// The connector isn't active any more.
    Unavailable,
    /// The grant that applied is gone.
    GrantRemoved,
    /// The tool changed after the user approved it.
    ToolChanged,
    /// The Host restarted before the call was sent.
    HostRestart,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    pub id: String,
    pub execution_id: String,
    pub session_id: String,
    pub connector_id: String,
    pub tool_name: String,
    pub schema_hash: String,
    pub args_hash: String,
    pub binding_hash: String,
    pub risk: Risk,
    pub preview: Option<String>,
    pub status: ApprovalStatus,
    pub void_reason: Option<VoidReason>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub decided_by: Option<String>,
    pub decided_at: Option<DateTime<Utc>>,
    pub consumed_at: Option<DateTime<Utc>>,
}

/// A call to ask about.
#[derive(Clone, Copy, Debug)]
pub struct NewApproval<'a> {
    pub execution_id: &'a str,
    pub session_id: &'a str,
    pub connector_id: &'a str,
    pub tool_name: &'a str,
    pub schema_hash: &'a str,
    pub arguments: &'a Map<String, Value>,
    pub risk: Risk,
    /// The arguments as the card shows them, already redacted.
    pub preview: &'a str,
}

#[derive(Debug, thiserror::Error)]
pub enum ApprovalError {
    #[error("no approval {0}")]
    NotFound(String),
    #[error("the answer is for another call")]
    Mismatch,
    #[error("already {status:?}")]
    AlreadyResolved {
        status: ApprovalStatus,
        decided_by: Option<String>,
    },
    #[error("the approval expired")]
    Expired,
    #[error("stored approval data is unreadable: {0}")]
    Corrupt(String),
    #[error("database: {0}")]
    Db(#[from] DbErr),
}

/// The longest preview kept.
const MAX_PREVIEW_CHARS: usize = 2_000;

fn name_of<T: Serialize>(value: T) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(name)) => name,
        _ => unreachable!("unit variants serialize as strings"),
    }
}

fn parse<T: for<'de> Deserialize<'de>>(name: &str) -> Result<T, ApprovalError> {
    serde_json::from_value(Value::String(name.into()))
        .map_err(|_| ApprovalError::Corrupt(format!("value {name:?}")))
}

fn capped(text: &str) -> String {
    match text.char_indices().nth(MAX_PREVIEW_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.into(),
    }
}

/// `sha256:<hex>` over the arguments, key order aside.
pub fn args_hash(arguments: &Map<String, Value>) -> String {
    sha256_of(&Value::Object(arguments.clone()))
}

/// Ties an answer to one call (r3 §5.2).
pub fn binding_hash(
    execution_id: &str,
    connector_id: &str,
    tool_name: &str,
    schema_hash: &str,
    args_hash: &str,
) -> String {
    sha256_of(&json!([
        execution_id,
        connector_id,
        tool_name,
        schema_hash,
        args_hash
    ]))
}

fn to_approval(row: connector_approval::Model) -> Result<Approval, ApprovalError> {
    Ok(Approval {
        risk: parse(&row.risk)?,
        status: parse(&row.status)?,
        void_reason: row.void_reason.as_deref().map(parse).transpose()?,
        id: row.id,
        execution_id: row.execution_id,
        session_id: row.session_id,
        connector_id: row.connector_id,
        tool_name: row.tool_name,
        schema_hash: row.schema_hash,
        args_hash: row.args_hash,
        binding_hash: row.binding_hash,
        preview: row.preview,
        created_at: row.created_at,
        expires_at: row.expires_at,
        decided_by: row.decided_by,
        decided_at: row.decided_at,
        consumed_at: row.consumed_at,
    })
}

/// Asks about a call: a `pending` approval that expires by its risk.
pub async fn create(
    conn: &DatabaseConnection,
    new: NewApproval<'_>,
    now: DateTime<Utc>,
) -> Result<Approval, ApprovalError> {
    let args = args_hash(new.arguments);
    let binding = binding_hash(
        new.execution_id,
        new.connector_id,
        new.tool_name,
        new.schema_hash,
        &args,
    );
    let row = connector_approval::ActiveModel {
        id: Set(uuid::Uuid::new_v4().to_string()),
        execution_id: Set(new.execution_id.into()),
        session_id: Set(new.session_id.into()),
        connector_id: Set(new.connector_id.into()),
        tool_name: Set(new.tool_name.into()),
        schema_hash: Set(new.schema_hash.into()),
        args_hash: Set(args),
        binding_hash: Set(binding),
        risk: Set(name_of(new.risk)),
        preview: Set(Some(capped(new.preview))),
        status: Set(name_of(ApprovalStatus::Pending)),
        void_reason: Set(None),
        created_at: Set(now),
        expires_at: Set(now + new.risk.expiry()),
        decided_by: Set(None),
        decided_at: Set(None),
        consumed_at: Set(None),
    }
    .insert(conn)
    .await?;
    to_approval(row)
}

pub async fn approval(
    conn: &DatabaseConnection,
    id: &str,
) -> Result<Option<Approval>, ApprovalError> {
    connector_approval::Entity::find_by_id(id.to_string())
        .one(conn)
        .await?
        .map(to_approval)
        .transpose()
}

fn from_any(states: &[ApprovalStatus]) -> Condition {
    states.iter().fold(Condition::any(), |any, state| {
        any.add(connector_approval::Column::Status.eq(name_of(*state)))
    })
}

/// Moves approval `id` to `to`, if it may. Leaving `pending` or `approved`
/// for good clears the preview.
async fn move_to(
    conn: &DatabaseConnection,
    id: &str,
    to: ApprovalStatus,
    extra: impl FnOnce(
        sea_orm::UpdateMany<connector_approval::Entity>,
    ) -> sea_orm::UpdateMany<connector_approval::Entity>,
) -> Result<bool, ApprovalError> {
    let mut update = connector_approval::Entity::update_many()
        .col_expr(connector_approval::Column::Status, Expr::value(name_of(to)));
    if to != ApprovalStatus::Approved {
        update = update.col_expr(
            connector_approval::Column::Preview,
            Expr::value(Option::<String>::None),
        );
    }
    let moved = extra(update)
        .filter(connector_approval::Column::Id.eq(id))
        .filter(from_any(to.follows()))
        .exec(conn)
        .await?;
    Ok(moved.rows_affected == 1)
}

/// The user's answer to approval `id`. The first answer wins; it must name
/// the call (`binding_hash`) and come before the approval expires.
pub async fn answer(
    conn: &DatabaseConnection,
    id: &str,
    binding: &str,
    approve: bool,
    decided_by: &str,
    now: DateTime<Utc>,
) -> Result<Approval, ApprovalError> {
    let to = if approve {
        ApprovalStatus::Approved
    } else {
        ApprovalStatus::Denied
    };
    let landed = move_to(conn, id, to, |update| {
        update
            .col_expr(
                connector_approval::Column::DecidedBy,
                Expr::value(Some(decided_by.to_string())),
            )
            .col_expr(
                connector_approval::Column::DecidedAt,
                Expr::value(Some(now)),
            )
            .filter(connector_approval::Column::BindingHash.eq(binding))
            .filter(connector_approval::Column::ExpiresAt.gt(now))
    })
    .await?;
    let current = approval(conn, id)
        .await?
        .ok_or_else(|| ApprovalError::NotFound(id.into()))?;
    if landed {
        return Ok(current);
    }
    if current.binding_hash != binding {
        return Err(ApprovalError::Mismatch);
    }
    match current.status {
        // Overdue, and the sweeper hasn't got to it yet.
        ApprovalStatus::Pending => {
            end(conn, id, ApprovalStatus::Expired, None, now).await?;
            Err(ApprovalError::Expired)
        }
        ApprovalStatus::Expired => Err(ApprovalError::Expired),
        status => Err(ApprovalError::AlreadyResolved {
            status,
            decided_by: current.decided_by,
        }),
    }
}

/// Uses approved approval `id` for its call, just before sending it. False
/// if it isn't approved (any more).
pub async fn consume(
    conn: &DatabaseConnection,
    id: &str,
    now: DateTime<Utc>,
) -> Result<bool, ApprovalError> {
    move_to(conn, id, ApprovalStatus::Consumed, |update| {
        update.col_expr(
            connector_approval::Column::ConsumedAt,
            Expr::value(Some(now)),
        )
    })
    .await
}

/// Ends approval `id` without it being used: `expired` or `cancelled` from
/// `pending`, `void` (with a reason) from `approved`. `None` if it had
/// already moved on.
pub async fn end(
    conn: &DatabaseConnection,
    id: &str,
    to: ApprovalStatus,
    reason: Option<VoidReason>,
    now: DateTime<Utc>,
) -> Result<Option<Approval>, ApprovalError> {
    debug_assert!(matches!(
        to,
        ApprovalStatus::Expired | ApprovalStatus::Cancelled | ApprovalStatus::Void
    ));
    debug_assert_eq!(to == ApprovalStatus::Void, reason.is_some());
    let landed = move_to(conn, id, to, |update| {
        let update = update.col_expr(
            connector_approval::Column::VoidReason,
            Expr::value(reason.map(name_of)),
        );
        if to == ApprovalStatus::Void {
            update
        } else {
            update.col_expr(
                connector_approval::Column::DecidedAt,
                Expr::value(Some(now)),
            )
        }
    })
    .await?;
    if !landed {
        return Ok(None);
    }
    approval(conn, id).await
}

async fn with_status(
    conn: &DatabaseConnection,
    condition: Condition,
) -> Result<Vec<Approval>, ApprovalError> {
    connector_approval::Entity::find()
        .filter(condition)
        .all(conn)
        .await?
        .into_iter()
        .map(to_approval)
        .collect()
}

/// Ends every approval of `rows` that moves `from` → `to`; returns those.
async fn end_all(
    conn: &DatabaseConnection,
    rows: Vec<Approval>,
    to: ApprovalStatus,
    reason: Option<VoidReason>,
    now: DateTime<Utc>,
) -> Result<Vec<Approval>, ApprovalError> {
    let mut ended = Vec::new();
    for row in rows {
        if let Some(approval) = end(conn, &row.id, to, reason, now).await? {
            ended.push(approval);
        }
    }
    Ok(ended)
}

fn status_is(status: ApprovalStatus) -> Condition {
    Condition::all().add(connector_approval::Column::Status.eq(name_of(status)))
}

/// Expires every pending approval past its time; returns those.
pub async fn expire_due(
    conn: &DatabaseConnection,
    now: DateTime<Utc>,
) -> Result<Vec<Approval>, ApprovalError> {
    let due = with_status(
        conn,
        status_is(ApprovalStatus::Pending).add(connector_approval::Column::ExpiresAt.lte(now)),
    )
    .await?;
    end_all(conn, due, ApprovalStatus::Expired, None, now).await
}

/// A connector is being revoked: what waits is cancelled and what was
/// approved can no longer be sent. Returns what ended.
pub async fn end_for_connector(
    conn: &DatabaseConnection,
    connector_id: &str,
    now: DateTime<Utc>,
) -> Result<Vec<Approval>, ApprovalError> {
    let of =
        |status| status_is(status).add(connector_approval::Column::ConnectorId.eq(connector_id));
    let pending = with_status(conn, of(ApprovalStatus::Pending)).await?;
    let mut ended = end_all(conn, pending, ApprovalStatus::Cancelled, None, now).await?;
    let approved = with_status(conn, of(ApprovalStatus::Approved)).await?;
    ended.extend(
        end_all(
            conn,
            approved,
            ApprovalStatus::Void,
            Some(VoidReason::Revoked),
            now,
        )
        .await?,
    );
    Ok(ended)
}

/// At boot: nothing the last Host left waiting or approved can be used.
/// Returns how many ended.
pub async fn sweep(conn: &DatabaseConnection, now: DateTime<Utc>) -> Result<usize, ApprovalError> {
    let pending = with_status(conn, status_is(ApprovalStatus::Pending)).await?;
    let mut ended = end_all(conn, pending, ApprovalStatus::Expired, None, now)
        .await?
        .len();
    let approved = with_status(conn, status_is(ApprovalStatus::Approved)).await?;
    ended += end_all(
        conn,
        approved,
        ApprovalStatus::Void,
        Some(VoidReason::HostRestart),
        now,
    )
    .await?
    .len();
    Ok(ended)
}

#[cfg(test)]
mod tests;
