use sea_orm::entity::prelude::*;

/// Zwiim-owned: one action on a page, part of the run record.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "playbook_step")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub item_id: i32,
    pub seq: i32,
    pub kind: String,
    pub target: Option<String>,
    pub outcome: Option<String>,
    pub route_match: Option<String>,
    pub screenshot_path: Option<String>,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
