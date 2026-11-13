use sea_orm::entity::prelude::*;

/// Zwiim-owned: a site the Host browser's site policy keeps. See
/// `crate::browser::store`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "browser_sites")]
pub struct Model {
    /// `strict` or `approved`.
    #[sea_orm(primary_key, auto_increment = false)]
    pub kind: String,
    /// The public session id of a strict site list; empty for `approved`.
    #[sea_orm(primary_key, auto_increment = false)]
    pub session_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub pattern: String,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
