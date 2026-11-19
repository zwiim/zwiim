use sea_orm::entity::prelude::*;

/// Zwiim-owned: failure count per (playbook, item), surviving runs (6.7.4).
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "item_retry_counter")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub playbook_id: i32,
    #[sea_orm(primary_key, auto_increment = false)]
    pub site: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub account: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_key: String,
    pub failures: i32,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
