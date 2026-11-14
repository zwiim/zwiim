//! The Host browser's persisted tabs (design 6.4): enough to bring a tab
//! back after an engine restart, a Host restart or a discard.

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use serde::{Deserialize, Serialize};

use crate::db::entities::{browser_site, browser_tab};

/// Who a tab belongs to (design 6.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TabOwner {
    /// The session's own tab, in its own context; its agent may use it.
    Session { session_id: String },
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

/// Forget every tab and the strict site list of a deleted session.
pub async fn remove_session(conn: &DatabaseConnection, session_id: &str) -> Result<(), DbErr> {
    browser_tab::Entity::delete_many()
        .filter(browser_tab::Column::SessionId.eq(session_id))
        .exec(conn)
        .await?;
    browser_site::Entity::delete_many()
        .filter(browser_site::Column::Kind.eq(STRICT))
        .filter(browser_site::Column::SessionId.eq(session_id))
        .exec(conn)
        .await?;
    Ok(())
}

const STRICT: &str = "strict";
const APPROVED: &str = "approved";

async fn sites(
    conn: &DatabaseConnection,
    kind: &str,
    session_id: &str,
) -> Result<Vec<String>, DbErr> {
    let rows = browser_site::Entity::find()
        .filter(browser_site::Column::Kind.eq(kind))
        .filter(browser_site::Column::SessionId.eq(session_id))
        .order_by_asc(browser_site::Column::CreatedAt)
        .order_by_asc(browser_site::Column::Pattern)
        .all(conn)
        .await?;
    Ok(rows.into_iter().map(|r| r.pattern).collect())
}

async fn add_site(
    conn: &impl sea_orm::ConnectionTrait,
    kind: &str,
    session_id: &str,
    pattern: &str,
) -> Result<(), DbErr> {
    let row = browser_site::ActiveModel {
        kind: Set(kind.to_string()),
        session_id: Set(session_id.to_string()),
        pattern: Set(pattern.to_string()),
        created_at: Set(Utc::now()),
    };
    browser_site::Entity::insert(row)
        .on_conflict(
            sea_orm::sea_query::OnConflict::columns([
                browser_site::Column::Kind,
                browser_site::Column::SessionId,
                browser_site::Column::Pattern,
            ])
            .do_nothing()
            .to_owned(),
        )
        .do_nothing()
        .exec(conn)
        .await?;
    Ok(())
}

/// Session `session_id`'s strict site list (design 13.2); empty for none.
pub async fn strict_sites(
    conn: &DatabaseConnection,
    session_id: &str,
) -> Result<Vec<String>, DbErr> {
    sites(conn, STRICT, session_id).await
}

/// Replace session `session_id`'s strict site list; empty removes it.
pub async fn set_strict_sites(
    conn: &DatabaseConnection,
    session_id: &str,
    patterns: &[String],
) -> Result<(), DbErr> {
    use sea_orm::TransactionTrait;
    let txn = conn.begin().await?;
    browser_site::Entity::delete_many()
        .filter(browser_site::Column::Kind.eq(STRICT))
        .filter(browser_site::Column::SessionId.eq(session_id))
        .exec(&txn)
        .await?;
    for pattern in patterns {
        add_site(&txn, STRICT, session_id, pattern).await?;
    }
    txn.commit().await
}

/// The sites a person always lets agents open, in every session.
pub async fn approved_sites(conn: &DatabaseConnection) -> Result<Vec<String>, DbErr> {
    sites(conn, APPROVED, "").await
}

/// Always let agents open `pattern`'s site.
pub async fn approve_site(conn: &DatabaseConnection, pattern: &str) -> Result<(), DbErr> {
    add_site(conn, APPROVED, "", pattern).await
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
    async fn a_deleted_sessions_tabs_are_forgotten() {
        let db = fresh_in_memory_db().await;
        insert(&db.conn, &tab("a", session("s1"), 0)).await.unwrap();
        insert(&db.conn, &tab("b", session("s2"), 0)).await.unwrap();
        insert(&db.conn, &tab("p", TabOwner::Personal, 0))
            .await
            .unwrap();
        set_strict_sites(&db.conn, "s1", &["github.com".into()])
            .await
            .unwrap();
        remove_session(&db.conn, "s1").await.unwrap();
        assert!(strict_sites(&db.conn, "s1").await.unwrap().is_empty());
        let mut left: Vec<String> = load(&db.conn)
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        left.sort();
        assert_eq!(left, vec!["b".to_string(), "p".to_string()]);
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
            serde_json::json!({"kind": "session", "session_id": "s1"})
        );
        assert_eq!(
            serde_json::to_value(TabOwner::Personal).unwrap(),
            serde_json::json!({"kind": "personal"})
        );
    }

    #[tokio::test]
    async fn a_strict_list_is_replaced_whole_and_approvals_are_kept_once() {
        let db = fresh_in_memory_db().await;
        let conn = &db.conn;
        let list = vec![
            "github.com".to_string(),
            "*.example.com".into(),
            "github.com".into(),
        ];
        set_strict_sites(conn, "s1", &list).await.unwrap();
        let mut got = strict_sites(conn, "s1").await.unwrap();
        got.sort();
        assert_eq!(got, vec!["*.example.com".to_string(), "github.com".into()]);
        assert!(strict_sites(conn, "s2").await.unwrap().is_empty());
        set_strict_sites(conn, "s1", &["docs.rs".into()])
            .await
            .unwrap();
        assert_eq!(
            strict_sites(conn, "s1").await.unwrap(),
            vec!["docs.rs".to_string()]
        );
        set_strict_sites(conn, "s1", &[]).await.unwrap();
        assert!(strict_sites(conn, "s1").await.unwrap().is_empty());

        approve_site(conn, "example.com:443").await.unwrap();
        approve_site(conn, "example.com:443").await.unwrap();
        assert_eq!(
            approved_sites(conn).await.unwrap(),
            vec!["example.com:443".to_string()]
        );
        // Removing a session leaves what is approved for every session.
        remove_session(conn, "s1").await.unwrap();
        assert_eq!(approved_sites(conn).await.unwrap().len(), 1);
    }
}
