use sea_orm::entity::prelude::*;

/// Zwiim-owned: one unit of work inside a run (one job, one post).
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "playbook_item")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// What clients address it by; the row id stays inside the Host.
    pub public_id: String,
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
    /// The cap slot the dispatch took: site, kind and local day (6.6.5).
    pub cap_site: Option<String>,
    pub cap_kind: Option<String>,
    pub cap_day: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
