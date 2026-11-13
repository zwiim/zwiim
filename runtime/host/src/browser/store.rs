//! The Host browser's persisted tabs (design 6.4): enough to bring a tab
//! back after an engine restart, a Host restart or a discard.

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use serde::{Deserialize, Serialize};

use crate::db::entities::browser_tab;

/// Who a tab belongs to (design 6.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TabOwner {
    /// The session's own tab, in its own context; its agent may use it.
    Session {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    /// The person's tab, in the personal context.
    Personal,
}

impl TabOwner {
    fn kind(&self) -> &'static str {
        match self {
            TabOwner::Session { .. } => "session",
            TabOwner::Personal => "personal",
        }
    }

    fn session_id(&self) -> Option<String> {
        match self {
            TabOwner::Session { session_id } => Some(session_id.clone()),
            TabOwner::Personal => None,
        }
    }

    fn from_row(kind: &str, session_id: Option<String>) -> Option<TabOwner> {
        match (kind, session_id) {
            ("session", Some(session_id)) => Some(TabOwner::Session { session_id }),
            ("personal", None) => Some(TabOwner::Personal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabRecord {
    pub id: String,
    pub owner: TabOwner,
    pub url: String,
    pub title: String,
    pub position: i32,
    pub discarded: bool,
}

/// Every tab, in each owner's order. Rows with an unknown owner are skipped.
pub async fn load(conn: &DatabaseConnection) -> Result<Vec<TabRecord>, DbErr> {
    let rows = browser_tab::Entity::find()
        .order_by_asc(browser_tab::Column::Position)
        .all(conn)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            Some(TabRecord {
                owner: TabOwner::from_row(&row.owner_kind, row.session_id)?,
                id: row.id,
                url: row.url,
                title: row.title,
                position: row.position,
                discarded: row.discarded,
            })
        })
        .collect())
}

/// The position after the owner's last tab.
pub async fn next_position(conn: &DatabaseConnection, owner: &TabOwner) -> Result<i32, DbErr> {
    let mut query = browser_tab::Entity::find()
        .select_only()
        .column_as(browser_tab::Column::Position.max(), "max")
        .filter(browser_tab::Column::OwnerKind.eq(owner.kind()));
    query = match owner.session_id() {
        Some(id) => query.filter(browser_tab::Column::SessionId.eq(id)),
        None => query.filter(browser_tab::Column::SessionId.is_null()),
    };
    let max: Option<Option<i32>> = query.into_tuple().one(conn).await?;
    Ok(max.flatten().map_or(0, |m| m + 1))
}

pub async fn insert(conn: &DatabaseConnection, tab: &TabRecord) -> Result<(), DbErr> {
    let now = Utc::now();
    browser_tab::ActiveModel {
        id: Set(tab.id.clone()),
        owner_kind: Set(tab.owner.kind().to_string()),
        session_id: Set(tab.owner.session_id()),
        url: Set(tab.url.clone()),
        title: Set(tab.title.clone()),
        position: Set(tab.position),
        created_at: Set(now),
        last_active_at: Set(now),
        discarded: Set(tab.discarded),
    }
    .insert(conn)
    .await?;
    Ok(())
}

/// The tab's page changed.
pub async fn update_page(
    conn: &DatabaseConnection,
    id: &str,
    url: &str,
    title: &str,
) -> Result<(), DbErr> {
    browser_tab::ActiveModel {
        id: Set(id.to_string()),
        url: Set(url.to_string()),
        title: Set(title.to_string()),
        last_active_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(conn)
    .await
    .map(|_| ())
    .or_else(ignore_missing)
}

pub async fn set_discarded(
    conn: &DatabaseConnection,
    id: &str,
    discarded: bool,
) -> Result<(), DbErr> {
    browser_tab::ActiveModel {
        id: Set(id.to_string()),
        discarded: Set(discarded),
        ..Default::default()
    }
    .update(conn)
    .await
    .map(|_| ())
    .or_else(ignore_missing)
}

pub async fn remove(conn: &DatabaseConnection, id: &str) -> Result<(), DbErr> {
    browser_tab::Entity::delete_by_id(id.to_string())
        .exec(conn)
        .await?;
    Ok(())
}

/// A tab closed while its update was in flight is not an error.
fn ignore_missing(e: DbErr) -> Result<(), DbErr> {
    match e {
        DbErr::RecordNotUpdated => Ok(()),
        e => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::fresh_in_memory_db;

    fn tab(id: &str, owner: TabOwner, position: i32) -> TabRecord {
        TabRecord {
            id: id.into(),
            owner,
            url: "about:blank".into(),
            title: String::new(),
            position,
            discarded: false,
        }
    }

    fn session(id: &str) -> TabOwner {
        TabOwner::Session {
            session_id: id.into(),
        }
    }

    #[tokio::test]
    async fn tabs_round_trip_in_order_with_positions_per_owner() {
        let db = fresh_in_memory_db().await;
        let conn = &db.conn;
        assert_eq!(next_position(conn, &session("s1")).await.unwrap(), 0);

        insert(conn, &tab("b", session("s1"), 1)).await.unwrap();
        insert(conn, &tab("a", session("s1"), 0)).await.unwrap();
        insert(conn, &tab("p", TabOwner::Personal, 0))
            .await
            .unwrap();
        assert_eq!(next_position(conn, &session("s1")).await.unwrap(), 2);
        assert_eq!(next_position(conn, &session("s2")).await.unwrap(), 0);
        assert_eq!(next_position(conn, &TabOwner::Personal).await.unwrap(), 1);

        update_page(conn, "a", "http://localhost:5173/", "App")
            .await
            .unwrap();
        set_discarded(conn, "b", true).await.unwrap();
        remove(conn, "p").await.unwrap();

        let tabs = load(conn).await.unwrap();
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0].id, "a");
        assert_eq!(tabs[0].url, "http://localhost:5173/");
        assert_eq!(tabs[0].title, "App");
        assert_eq!(tabs[0].owner, session("s1"));
        assert_eq!(tabs[1].id, "b");
        assert!(tabs[1].discarded);
    }

    #[tokio::test]
    async fn updating_a_closed_tab_is_not_an_error() {
        let db = fresh_in_memory_db().await;
        update_page(&db.conn, "gone", "x", "y").await.unwrap();
        set_discarded(&db.conn, "gone", true).await.unwrap();
    }

    #[test]
    fn owners_serialize_for_the_protocol() {
        assert_eq!(
            serde_json::to_value(session("s1")).unwrap(),
            serde_json::json!({"kind": "session", "sessionId": "s1"})
        );
        assert_eq!(
            serde_json::to_value(TabOwner::Personal).unwrap(),
            serde_json::json!({"kind": "personal"})
        );
    }
}
