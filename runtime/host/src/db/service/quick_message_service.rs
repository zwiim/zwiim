use chrono::Utc;
use sea_orm::DatabaseConnection;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ConnectionTrait, DbBackend, EntityTrait,
    IntoActiveModel, QueryOrder, Set, Statement,
};

use crate::db::entities::quick_message;
use crate::db::error::DbError;
use crate::models::QuickMessageInfo;

fn to_info(m: quick_message::Model) -> QuickMessageInfo {
    QuickMessageInfo {
        id: m.id,
        title: m.title,
        content: m.content,
        sort_order: m.sort_order,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

pub async fn list(conn: &DatabaseConnection) -> Result<Vec<QuickMessageInfo>, DbError> {
    let rows = quick_message::Entity::find()
        .order_by_asc(quick_message::Column::SortOrder)
        .all(conn)
        .await?;

    Ok(rows.into_iter().map(to_info).collect())
}

pub async fn create(
    conn: &DatabaseConnection,
    title: &str,
    content: &str,
) -> Result<QuickMessageInfo, DbError> {
    let now = Utc::now();

    let max_order = quick_message::Entity::find()
        .order_by_desc(quick_message::Column::SortOrder)
        .one(conn)
        .await?
        .map(|m| m.sort_order)
        .unwrap_or(-1);

    let active = quick_message::ActiveModel {
        id: NotSet,
        title: Set(title.to_string()),
        content: Set(content.to_string()),
        sort_order: Set(max_order + 1),
        created_at: Set(now),
        updated_at: Set(now),
    };

    let model = active.insert(conn).await?;
    Ok(to_info(model))
}

pub async fn update(
    conn: &DatabaseConnection,
    id: i32,
    title: Option<String>,
    content: Option<String>,
) -> Result<QuickMessageInfo, DbError> {
    let row = quick_message::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("QuickMessage {} not found", id)))?;

    let mut active = row.into_active_model();
    if let Some(t) = title {
        active.title = Set(t);
    }
    if let Some(c) = content {
        active.content = Set(c);
    }
    active.updated_at = Set(Utc::now());

    let model = active.update(conn).await?;
    Ok(to_info(model))
}

pub async fn delete(conn: &DatabaseConnection, id: i32) -> Result<(), DbError> {
    quick_message::Entity::delete_by_id(id).exec(conn).await?;
    Ok(())
}

pub async fn reorder(conn: &DatabaseConnection, ids: Vec<i32>) -> Result<(), DbError> {
    if ids.is_empty() {
        return Ok(());
    }

    let now = Utc::now();
    let now_str = now.format("%Y-%m-%d %H:%M:%S %:z").to_string();
    let case_expr = ids
        .iter()
        .enumerate()
        .map(|(idx, id)| format!("WHEN {} THEN {}", id, idx))
        .collect::<Vec<_>>()
        .join(" ");
    let id_list = ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(", ");

    let sql = format!(
        "UPDATE quick_message SET sort_order = CASE id {case_expr} END, updated_at = '{now_str}' WHERE id IN ({id_list})"
    );
    conn.execute(Statement::from_string(DbBackend::Sqlite, sql))
        .await?;

    Ok(())
}
