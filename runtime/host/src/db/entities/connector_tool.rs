use sea_orm::entity::prelude::*;

/// Zwiim-owned: one tool in a connector's snapshot. See
/// `crate::connectors::store`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "connector_tools")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub connector_id: String,
    pub name: String,
    pub title: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub description: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub input_schema_json: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub output_schema_json: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub annotations_json: Option<String>,
    pub schema_hash: String,
    pub status: String,
    pub first_seen_at: DateTimeUtc,
    pub last_seen_at: DateTimeUtc,
    pub changed_at: Option<DateTimeUtc>,
    /// The user's switch: agents are shown the tool only when on.
    pub enabled: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
