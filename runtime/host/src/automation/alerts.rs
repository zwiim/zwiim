//! Zwiim-owned: alerts for the moments that need the person (automation-
//! engine spec 11.4.2-3): a run done, an item waiting for the person or out
//! of retries, an approval waiting. Each is recorded once per (kind,
//! subject); only a newly recorded alert is sent.

use sea_orm::sea_query::OnConflict;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};
use serde::Serialize;

use crate::automation::entities::{playbook_alert, playbook_item, playbook_run};
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
    pub alert_id: String,
    pub kind: String,
    pub run_id: String,
    pub item_id: Option<String>,
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
    // Clients see public ids only.
    let run = playbook_run::Entity::find_by_id(row.run_id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("run {}", row.run_id)))?;
    let item = match row.item_id {
        Some(id) => Some(
            playbook_item::Entity::find_by_id(id)
                .one(conn)
                .await?
                .ok_or_else(|| DbError::NotFound(format!("item {id}")))?
                .public_id,
        ),
        None => None,
    };
    Ok(Some(Alert {
        alert_id: row.public_id,
        kind: row.kind,
        run_id: run.public_id,
        item_id: item,
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
        let pb = crate::automation::PlaybookStore::new(db.clone())
            .create("Apply", "goal")
            .await
            .unwrap();
        let run = crate::automation::AutomationEngine::new(db.clone())
            .start_run(pb.id, pb.version, crate::automation::RunTrigger::Mac, None)
            .await
            .unwrap();
        let now = chrono::Utc::now();
        let item = playbook_item::Entity::insert(playbook_item::ActiveModel {
            public_id: Set(uuid::Uuid::new_v4().to_string()),
            run_id: Set(run),
            playbook_id: Set(pb.id),
            site: Set("indeed.com".into()),
            account: Set("main".into()),
            item_key: Set("job-1".into()),
            status: Set("waiting_person".into()),
            epoch: Set(1),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        })
        .exec_with_returning(&db)
        .await
        .unwrap();
        let first = record(
            &db,
            AlertKind::ItemWaitingPerson,
            item.id,
            run,
            Some(item.id),
            "Acme: needs you",
        )
        .await
        .unwrap()
        .expect("new");
        // Clients see public ids only.
        assert_eq!(first.item_id, Some(item.public_id.clone()));
        assert!(uuid::Uuid::parse_str(&first.run_id).is_ok());
        let again = record(
            &db,
            AlertKind::ItemWaitingPerson,
            item.id,
            run,
            Some(item.id),
            "Acme: needs you",
        )
        .await
        .unwrap();
        assert_eq!(again, None);
        // Another kind about the same item is its own alert.
        let other = record(
            &db,
            AlertKind::ItemRetriesExhausted,
            item.id,
            run,
            Some(item.id),
            "Acme",
        )
        .await
        .unwrap();
        assert!(other.is_some());
    }
}
