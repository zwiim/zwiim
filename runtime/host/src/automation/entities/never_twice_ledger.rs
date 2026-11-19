use sea_orm::entity::prelude::*;

/// Zwiim-owned: the shared never-twice ledger (spec 6.7.1); `reserved` or
/// `done`, keyed by (site, account, action kind, item key).
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "never_twice_ledger")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub site: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub account: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub action_kind: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_key: String,
    pub state: String,
    pub round_id: Option<i32>,
    pub item_id: Option<i32>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
