//! Zwiim-owned: alerts for the moments that need the person (automation-
//! engine spec 11.4.2-3): a run done, an item waiting for the person or out
//! of retries, an approval waiting. Each is recorded once per (kind,
//! subject); only a newly recorded alert is sent.

use sea_orm::sea_query::OnConflict;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};
use serde::Serialize;

use crate::automation::entities::playbook_alert;
use crate::db::error::DbError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    RunDone,
    ItemWaitingPerson,
    ItemRetriesExhausted,
    ApprovalWaiting,
}

impl AlertKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AlertKind::RunDone => "run_done",
            AlertKind::ItemWaitingPerson => "item_waiting_person",
            AlertKind::ItemRetriesExhausted => "item_retries_exhausted",
            AlertKind::ApprovalWaiting => "approval_waiting",
        }
    }
}

/// One alert as `run.alert` carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Alert {
    pub alert_id: i32,
    pub kind: String,
    pub run_id: i32,
    pub item_id: Option<i32>,
    pub text: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Record an alert about `subject` (the run for `run_done`, the item for
/// item alerts, the approval for `approval_waiting`). `None` when it was
/// already recorded, so the caller sends only new alerts.
pub async fn record<C: ConnectionTrait>(
    conn: &C,
    kind: AlertKind,
    subject: i32,
    run_id: i32,
    item_id: Option<i32>,
    text: &str,
) -> Result<Option<Alert>, DbError> {
    let now = chrono::Utc::now();
    let inserted = playbook_alert::Entity::insert(playbook_alert::ActiveModel {
        public_id: Set(uuid::Uuid::new_v4().to_string()),
        kind: Set(kind.as_str().to_string()),
        subject: Set(subject),
        run_id: Set(run_id),
        item_id: Set(item_id),
        text: Set(text.to_string()),
        created_at: Set(now),
        ..Default::default()
    })
    .on_conflict(
        OnConflict::columns([
            playbook_alert::Column::Kind,
            playbook_alert::Column::Subject,
        ])
        .do_nothing()
        .to_owned(),
    )
    .exec_without_returning(conn)
    .await?;
    if inserted == 0 {
        return Ok(None);
    }
    let row = playbook_alert::Entity::find()
        .filter(playbook_alert::Column::Kind.eq(kind.as_str()))
        .filter(playbook_alert::Column::Subject.eq(subject))
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("alert {} {subject}", kind.as_str())))?;
    Ok(Some(Alert {
        alert_id: row.id,
        kind: row.kind,
        run_id: row.run_id,
        item_id: row.item_id,
        text: row.text,
        created_at: row.created_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migration::Migrator;
    use sea_orm_migration::MigratorTrait;

    #[tokio::test]
    async fn an_alert_is_recorded_once_per_kind_and_subject() {
        let db = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
        Migrator::up(&db, None).await.unwrap();
        let first = record(
            &db,
            AlertKind::ItemWaitingPerson,
            7,
            1,
            Some(7),
            "Acme: needs you",
        )
        .await
        .unwrap();
        assert!(first.is_some());
        let again = record(
            &db,
            AlertKind::ItemWaitingPerson,
            7,
            1,
            Some(7),
            "Acme: needs you",
        )
        .await
        .unwrap();
        assert_eq!(again, None);
        // Another kind about the same item is its own alert.
        let other = record(&db, AlertKind::ItemRetriesExhausted, 7, 1, Some(7), "Acme")
            .await
            .unwrap();
        assert!(other.is_some());
    }
}
