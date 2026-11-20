use sea_orm::entity::prelude::*;

/// Zwiim-owned: daily cap usage per (site, account, action kind, local day).
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "cap_usage")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub site: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub account: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub action_kind: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub local_day: String,
    pub used: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
