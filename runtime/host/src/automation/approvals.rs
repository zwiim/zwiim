// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: engine-owned approvals (automation-engine spec 8.5).
//!
//! An approval binds to a hash of the declaration (kind, submit target, the
//! sorted declared fields, body text whitespace-normalized). The person
//! answers once: approve, edit-then-approve, or skip. The first answer wins by
//! a conditional UPDATE, so a Mac and a phone racing produce one outcome and
//! the loser gets `approval_not_pending` with the current state.

use chrono::Utc;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Set,
    TransactionTrait,
};

use crate::automation::declaration::{Declaration, DeclaredValue};
use crate::automation::entities::playbook_approval;
use crate::db::error::DbError;

/// An approval as the Host stores and hands back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub id: i32,
    pub run_id: i32,
    pub item_id: i32,
    pub kind: String,
    pub payload: String,
    pub hash: String,
    pub state: ApprovalState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalState {
    Pending,
    Approved,
    Rejected,
    Stopped,
}

impl ApprovalState {
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalState::Pending => "pending",
            ApprovalState::Approved => "approved",
            ApprovalState::Rejected => "rejected",
            ApprovalState::Stopped => "stopped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => ApprovalState::Pending,
            "approved" => ApprovalState::Approved,
            "rejected" => ApprovalState::Rejected,
            "stopped" => ApprovalState::Stopped,
            _ => return None,
        })
    }
}

/// The answer the person gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalAnswer {
    /// Send (optionally edited): the Host performs the approved click.
    Approve,
    /// Skip item: `skipped` with `approval_rejected`.
    Skip,
}

/// Whether the answer found the approval pending (first-wins) or already
/// answered (the caller raises `approval_not_pending` with `current`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerOutcome {
    Applied,
    NotPending(ApprovalState),
}

/// The whitespace-insensitive binding hash (spec 8.5.3). Whitespace-only
/// edits keep it; any field change breaks it.
pub fn binding_hash(declaration: &Declaration) -> String {
    let mut parts: Vec<String> = vec![
        declaration.kind.as_str().to_string(),
        collapse(&declaration.submit_target.name),
        collapse(&declaration.summary),
    ];
    let mut fields: Vec<String> = declaration
        .fields
        .iter()
        .map(|f| {
            let value = match &f.value {
                DeclaredValue::Text { text } => collapse(text),
                DeclaredValue::Secret { handle } => handle.clone(),
                DeclaredValue::InputFile { name } => name.clone(),
            };
            format!("{}={}", f.target.name, value)
        })
        .collect();
    fields.sort();
    parts.extend(fields);
    fnv_hex(&parts.join("\u{1f}"))
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn fnv_hex(s: &str) -> String {
    // A stable, dependency-free 64-bit FNV-1a; enough for change detection.
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Store a pending approval for an item (spec 8.5.1).
pub async fn create(
    conn: &DatabaseConnection,
    run_id: i32,
    item_id: i32,
    declaration: &Declaration,
) -> Result<Approval, DbError> {
    // One pending approval per item: a repeat replaces the old, unanswered
    // one, in one transaction so two creates cannot leave two pending.
    let tx = conn.begin().await?;
    playbook_approval::Entity::delete_many()
        .filter(playbook_approval::Column::ItemId.eq(item_id))
        .filter(playbook_approval::Column::State.eq("pending"))
        .exec(&tx)
        .await?;
    let payload = serde_json::to_string(declaration).unwrap_or_default();
    let row = playbook_approval::Entity::insert(playbook_approval::ActiveModel {
        run_id: Set(run_id),
        item_id: Set(item_id),
        kind: Set(declaration.kind.as_str().to_string()),
        hash: Set(binding_hash(declaration)),
        payload: Set(payload),
        state: Set("pending".to_string()),
        consumed_at: Set(None),
        created_at: Set(Utc::now()),
        ..Default::default()
    })
    .exec_with_returning(&tx)
    .await?;
    tx.commit().await?;
    Ok(to_approval(row))
}

/// The person's answer to an approval, with its effect on the item, in one
/// transaction (spec 8.5.4, 8.8.3). The approval and the item never
/// disagree: an item that has left `waiting_approval` turns its approval
/// `stopped` and the answer is `NotPending`. Skip ends the item `skipped`.
/// Send moves the item to `working`, except while the run is paused or the
/// lane is busy: then the approval is recorded and the item stays parked
/// until [`release_approved`] moves it.
pub async fn respond(
    conn: &DatabaseConnection,
    approval_id: i32,
    reply: ApprovalAnswer,
) -> Result<AnswerOutcome, DbError> {
    use crate::automation::entities::{playbook_item, playbook_run};
    let tx = conn.begin().await?;
    let row = playbook_approval::Entity::find_by_id(approval_id)
        .one(&tx)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("approval {approval_id}")))?;
    let item = playbook_item::Entity::find_by_id(row.item_id)
        .one(&tx)
        .await?;
    let parked = item
        .as_ref()
        .is_some_and(|i| i.status == "waiting_approval");
    if row.state == "pending" && !parked {
        set_state(&tx, approval_id, "pending", "stopped").await?;
        tx.commit().await?;
        return Ok(AnswerOutcome::NotPending(ApprovalState::Stopped));
    }
    let outcome = answer(&tx, approval_id, reply.clone()).await?;
    if outcome != AnswerOutcome::Applied {
        tx.rollback().await?;
        return Ok(outcome);
    }
    match reply {
        ApprovalAnswer::Skip => {
            move_parked(&tx, row.item_id, "skipped", Some("approval_rejected")).await?;
        }
        ApprovalAnswer::Approve => {
            let paused = playbook_run::Entity::find_by_id(row.run_id)
                .one(&tx)
                .await?
                .is_some_and(|r| r.status == "paused");
            if !paused {
                start_approved(&tx, row.item_id).await?;
            }
        }
    }
    tx.commit().await?;
    Ok(AnswerOutcome::Applied)
}

/// Move a run's approved, parked items to `working` (on resume, spec 8.8.3).
pub async fn release_approved(conn: &DatabaseConnection, run_id: i32) -> Result<(), DbError> {
    let rows = playbook_approval::Entity::find()
        .filter(playbook_approval::Column::RunId.eq(run_id))
        .filter(playbook_approval::Column::State.eq("approved"))
        .filter(playbook_approval::Column::ConsumedAt.is_null())
        .all(conn)
        .await?;
    for row in rows {
        start_approved(conn, row.item_id).await?;
    }
    Ok(())
}

/// `waiting_approval -> working` with a new epoch. A busy lane (the
/// one-working-per-lane index) leaves the item parked with its approval
/// recorded rather than failing the answer.
async fn start_approved<C: ConnectionTrait>(conn: &C, item_id: i32) -> Result<(), DbError> {
    use crate::automation::entities::playbook_item;
    use sea_orm::sea_query::Expr;
    let res = playbook_item::Entity::update_many()
        .col_expr(playbook_item::Column::Status, Expr::value("working"))
        .col_expr(
            playbook_item::Column::Epoch,
            Expr::col(playbook_item::Column::Epoch).add(1),
        )
        .col_expr(playbook_item::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(playbook_item::Column::Id.eq(item_id))
        .filter(playbook_item::Column::Status.eq("waiting_approval"))
        .exec(conn)
        .await;
    match res {
        Ok(_) => Ok(()),
        Err(e) if e.to_string().to_ascii_lowercase().contains("unique") => Ok(()),
        Err(e) => Err(e.into()),
    }
}

async fn move_parked<C: ConnectionTrait>(
    conn: &C,
    item_id: i32,
    to: &str,
    reason: Option<&str>,
) -> Result<(), DbError> {
    use crate::automation::entities::playbook_item;
    use sea_orm::sea_query::Expr;
    playbook_item::Entity::update_many()
        .col_expr(playbook_item::Column::Status, Expr::value(to))
        .col_expr(playbook_item::Column::ReasonCode, Expr::value(reason))
        .col_expr(playbook_item::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(playbook_item::Column::Id.eq(item_id))
        .filter(playbook_item::Column::Status.eq("waiting_approval"))
        .exec(conn)
        .await?;
    Ok(())
}

async fn set_state<C: ConnectionTrait>(
    conn: &C,
    approval_id: i32,
    from: &str,
    to: &str,
) -> Result<u64, DbError> {
    let res = playbook_approval::Entity::update_many()
        .col_expr(
            playbook_approval::Column::State,
            sea_orm::sea_query::Expr::value(to),
        )
        .filter(playbook_approval::Column::Id.eq(approval_id))
        .filter(playbook_approval::Column::State.eq(from))
        .exec(conn)
        .await?;
    Ok(res.rows_affected)
}

/// Answer a pending approval; the first answer wins (spec 8.5.4).
pub async fn answer<C: ConnectionTrait>(
    conn: &C,
    approval_id: i32,
    answer: ApprovalAnswer,
) -> Result<AnswerOutcome, DbError> {
    let to = match answer {
        ApprovalAnswer::Approve => "approved",
        ApprovalAnswer::Skip => "rejected",
    };
    // CAS: only a still-pending row moves.
    if set_state(conn, approval_id, "pending", to).await? == 1 {
        return Ok(AnswerOutcome::Applied);
    }
    // Already answered (or gone): report the current state.
    let current = playbook_approval::Entity::find_by_id(approval_id)
        .one(conn)
        .await?
        .and_then(|m| ApprovalState::parse(&m.state))
        .ok_or_else(|| DbError::NotFound(format!("approval {approval_id}")))?;
    Ok(AnswerOutcome::NotPending(current))
}

/// Use an item's approval for the Host's approved click (spec 8.5.3, 8.5.5):
/// `true` exactly once, and only when the declaration about to be sent
/// hashes to what the person approved. The conditional UPDATE makes a
/// second click, or one with changed content, refused.
pub async fn consume<C: ConnectionTrait>(
    conn: &C,
    item_id: i32,
    declaration: &Declaration,
) -> Result<bool, DbError> {
    let res = playbook_approval::Entity::update_many()
        .col_expr(
            playbook_approval::Column::ConsumedAt,
            sea_orm::sea_query::Expr::value(Some(Utc::now())),
        )
        .filter(playbook_approval::Column::ItemId.eq(item_id))
        .filter(playbook_approval::Column::State.eq("approved"))
        .filter(playbook_approval::Column::ConsumedAt.is_null())
        .filter(playbook_approval::Column::Hash.eq(binding_hash(declaration)))
        .exec(conn)
        .await?;
    Ok(res.rows_affected == 1)
}

/// The content the person approved for an item and the Host has not yet
/// sent, if any: the declaration with any edits applied (spec 8.5.3).
pub async fn approved_declaration<C: ConnectionTrait>(
    conn: &C,
    item_id: i32,
) -> Result<Option<Declaration>, DbError> {
    let row = playbook_approval::Entity::find()
        .filter(playbook_approval::Column::ItemId.eq(item_id))
        .filter(playbook_approval::Column::State.eq("approved"))
        .filter(playbook_approval::Column::ConsumedAt.is_null())
        .one(conn)
        .await?;
    Ok(row.and_then(|m| serde_json::from_str(&m.payload).ok()))
}

/// Cancel every pending approval of a run (Stop, spec 8.8.2).
pub async fn stop_pending(conn: &DatabaseConnection, run_id: i32) -> Result<u64, DbError> {
    let res = playbook_approval::Entity::update_many()
        .col_expr(
            playbook_approval::Column::State,
            sea_orm::sea_query::Expr::value("stopped"),
        )
        .filter(playbook_approval::Column::RunId.eq(run_id))
        .filter(playbook_approval::Column::State.eq("pending"))
        .exec(conn)
        .await?;
    Ok(res.rows_affected)
}

fn to_approval(m: playbook_approval::Model) -> Approval {
    Approval {
        id: m.id,
        run_id: m.run_id,
        item_id: m.item_id,
        kind: m.kind,
        payload: m.payload,
        hash: m.hash,
        state: ApprovalState::parse(&m.state).unwrap_or(ApprovalState::Pending),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::declaration::{
        ControlKind, Declaration, DeclaredField, DeclaredValue, Target,
    };
    use crate::automation::reasons::ActionKind;

    fn decl() -> Declaration {
        Declaration {
            kind: ActionKind::SubmitApplication,
            summary: "Apply to Acme".into(),
            fields: vec![DeclaredField {
                target: Target::new("textbox", "Name"),
                control: ControlKind::Text,
                value: DeclaredValue::Text {
                    text: "Asha".into(),
                },
            }],
            submit_target: Target::new("button", "Submit"),
        }
    }

    #[test]
    fn whitespace_changes_keep_the_hash_others_break_it() {
        let base = binding_hash(&decl());
        let mut spaced = decl();
        spaced.summary = "Apply  to\tAcme".into();
        assert_eq!(
            binding_hash(&spaced),
            base,
            "whitespace-only keeps the hash"
        );
        let mut changed = decl();
        changed.fields[0].value = DeclaredValue::Text {
            text: "Someone".into(),
        };
        assert_ne!(binding_hash(&changed), base);
    }

    #[tokio::test]
    async fn the_first_answer_wins_and_the_loser_sees_the_state() {
        let db = test_db().await;
        let a = create(&db, 1, 1, &decl()).await.expect("create");
        assert_eq!(
            answer(&db, a.id, ApprovalAnswer::Approve)
                .await
                .expect("ok"),
            AnswerOutcome::Applied
        );
        assert_eq!(
            answer(&db, a.id, ApprovalAnswer::Skip).await.expect("ok"),
            AnswerOutcome::NotPending(ApprovalState::Approved)
        );
    }

    async fn test_db() -> DatabaseConnection {
        use crate::db::migration::Migrator;
        use sea_orm::Database;
        use sea_orm_migration::MigratorTrait;
        let db = Database::connect("sqlite::memory:").await.unwrap();
        Migrator::up(&db, None).await.unwrap();
        db
    }

    #[tokio::test]
    async fn an_approval_is_consumed_once_and_only_for_its_own_content() {
        use crate::db::migration::Migrator;
        use sea_orm_migration::MigratorTrait;
        let db = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
        Migrator::up(&db, None).await.unwrap();
        let a = create(&db, 1, 7, &decl()).await.unwrap();
        // Not yet approved: nothing to consume.
        assert!(!consume(&db, 7, &decl()).await.unwrap());
        answer(&db, a.id, ApprovalAnswer::Approve).await.unwrap();
        // Changed content breaks the hash: the click is refused.
        let mut changed = decl();
        changed.fields[0].value = DeclaredValue::Text {
            text: "Someone else".into(),
        };
        assert!(!consume(&db, 7, &changed).await.unwrap());
        // Whitespace-only difference keeps the hash; it is single-use.
        let mut spaced = decl();
        spaced.summary = "Apply  to   Acme".into();
        assert!(consume(&db, 7, &spaced).await.unwrap());
        assert!(!consume(&db, 7, &decl()).await.unwrap());
    }
}
