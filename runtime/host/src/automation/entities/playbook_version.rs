use sea_orm::entity::prelude::*;

/// Zwiim-owned: one saved version of a playbook's text; runs pin one.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "playbook_version")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub playbook_id: i32,
    pub version: i32,
    pub source: String,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
