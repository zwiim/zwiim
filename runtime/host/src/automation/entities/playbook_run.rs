use sea_orm::entity::prelude::*;

/// Zwiim-owned: one firing of a pinned playbook version.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "playbook_run")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// What clients address it by; the row id stays inside the Host.
    pub public_id: String,
    pub playbook_id: i32,
    pub playbook_version: i32,
    pub trigger: String,
    pub status: String,
    pub pause_reason: Option<String>,
    pub outcome: Option<String>,
    pub item_limit: Option<i32>,
    pub items_total: i32,
    pub items_done: i32,
    pub error: Option<String>,
    pub created_at: DateTimeUtc,
    pub started_at: Option<DateTimeUtc>,
    pub ended_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
