use sea_orm::entity::prelude::*;

/// Zwiim-owned: site-scoped knowledge (item-ID rule, pause, login, risk
/// acceptance), read only through `PlaybookStore` (spec 5.2.1, 5.4).
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "site_knowledge")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub site: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub kind: String,
    /// Empty for site-wide kinds (the item-ID rule); the account otherwise.
    #[sea_orm(primary_key, auto_increment = false)]
    pub account: String,
    pub value: String,
    pub version: i32,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
