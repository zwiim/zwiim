use sea_orm::entity::prelude::*;

/// Zwiim-owned: one account on one connector definition. See
/// `crate::connectors::store`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "connections")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub definition_id: String,
    pub definition_hash: String,
    pub label: String,
    #[sea_orm(unique)]
    pub slug: String,
    pub status: String,
    pub store: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub revocation_json: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub last_error: Option<String>,
    pub last_validated_at: Option<DateTimeUtc>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
