use sea_orm::entity::prelude::*;

/// Zwiim-owned: a playbook the person owns, outliving every session.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "playbook")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// What clients address it by; the row id stays inside the Host.
    pub public_id: String,
    pub name: String,
    pub source: String,
    pub format: i32,
    pub version: i32,
    pub show_me: bool,
    pub freeze_learning: bool,
    pub deleted_at: Option<DateTimeUtc>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
