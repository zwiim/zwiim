// Modified from upstream Apache-2.0 source; see NOTICE.
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "remote_workspace_connection")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: String,
    pub base_url: String,
    #[sea_orm(column_type = "Text")]
    pub token: String,
    /// JSON `Vec<RemoteWorkspaceHeader>` — extra headers the desktop client
    /// sends on every request to this connection.
    #[sea_orm(column_type = "Text")]
    pub headers: String,
    pub sort_order: i32,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

/// Credentials never land in this table in plaintext (`db::plaintext_secrets`).
#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, _insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        if let sea_orm::ActiveValue::Set(token) = &self.token {
            crate::db::plaintext_secrets::refuse_credential("remote_workspace_connection.token", token)?;
        }
        Ok(self)
    }
}
