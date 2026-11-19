use sea_orm::entity::prelude::*;

/// Zwiim-owned: one alert the Host sent for a run (automation-engine spec
/// 11.4.2-3), unique per (kind, subject).
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "playbook_alert")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub kind: String,
    pub subject: i32,
    pub run_id: i32,
    pub item_id: Option<i32>,
    pub text: String,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
