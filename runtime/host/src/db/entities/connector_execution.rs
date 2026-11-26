use sea_orm::entity::prelude::*;

/// Zwiim-owned: one tool call made through a connector. See
/// `crate::connectors::executions`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "connector_executions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub connector_id: String,
    pub session_id: Option<String>,
    pub tool_name: String,
    pub schema_hash: Option<String>,
    pub state: String,
    pub reason: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub detail: Option<String>,
    pub decided_by: Option<String>,
    pub args_digest: String,
    #[sea_orm(column_type = "Text")]
    pub arg_keys_json: String,
    pub received_at: DateTimeUtc,
    pub dispatched_at: Option<DateTimeUtc>,
    pub finished_at: Option<DateTimeUtc>,
    pub upstream_ms: Option<i64>,
    pub overhead_ms: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
