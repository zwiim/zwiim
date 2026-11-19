use sea_orm::entity::prelude::*;

/// Zwiim-owned: one unit of work inside a run (one job, one post).
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "playbook_item")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub run_id: i32,
    pub playbook_id: i32,
    pub site: String,
    pub account: String,
    pub item_key: String,
    pub title: Option<String>,
    pub url: Option<String>,
    pub outward_site: Option<String>,
    pub outward_url: Option<String>,
    pub status: String,
    pub reason_code: Option<String>,
    pub epoch: i32,
    pub outward_started_at: Option<DateTimeUtc>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
