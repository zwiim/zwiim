use sea_orm::entity::prelude::*;

/// Zwiim-owned: the user's approval of one connector call. See
/// `crate::connectors::approvals`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "connector_approvals")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub execution_id: String,
    pub session_id: String,
    pub connection_id: String,
    pub tool_name: String,
    pub schema_hash: String,
    pub args_hash: String,
    pub binding_hash: String,
    pub risk: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub preview: Option<String>,
    pub status: String,
    pub void_reason: Option<String>,
    pub created_at: DateTimeUtc,
    pub expires_at: DateTimeUtc,
    pub decided_by: Option<String>,
    pub decided_at: Option<DateTimeUtc>,
    pub consumed_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
