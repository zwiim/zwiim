use sea_orm::entity::prelude::*;

/// Zwiim-owned: the per-playbook skip list (spec 6.7.2).
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "playbook_skip")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub playbook_id: i32,
    #[sea_orm(primary_key, auto_increment = false)]
    pub site: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub account: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_key: String,
    pub reason: String,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
