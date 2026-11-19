use sea_orm::entity::prelude::*;

/// Zwiim-owned: one engine approval request (automation-engine spec 8.5).
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "playbook_approval")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// What clients address it by; the row id stays inside the Host.
    pub public_id: String,
    pub run_id: i32,
    pub item_id: i32,
    pub kind: String,
    pub payload: String,
    pub hash: String,
    pub state: String,
    pub consumed_at: Option<DateTimeUtc>,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
