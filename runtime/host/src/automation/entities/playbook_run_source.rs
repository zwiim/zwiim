use sea_orm::entity::prelude::*;

/// Zwiim-owned: one source of a run and how far discovery got with it
/// (automation-engine spec 6.2.8).
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "playbook_run_source")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub run_id: i32,
    pub source_index: i32,
    /// `pending` or `done`.
    pub status: String,
    pub note: Option<String>,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
