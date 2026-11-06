use sea_orm::entity::prelude::*;

/// Zwiim-owned: the session and turn a forked session came from. See
/// `crate::session_fork`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "session_forks")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub session_id: String,
    pub source_session_id: String,
    pub source_turn_id: String,
    pub created_at: DateTimeUtc,
    /// The transcript the session's first finished turn still has to carry.
    pub pending_transcript: Option<String>,
    /// The turn under way carries `pending_transcript`.
    pub transcript_sent: bool,
    /// The agent still has to be told the files stayed at the source's latest
    /// turn.
    pub notice_owed: bool,
    /// The options asked for, as JSON, when the agent's choices weren't known
    /// yet: applied when it first starts.
    pub pending_options: Option<String>,
    /// How many files the new worktree started with; `None` for a fork in place.
    pub worktree_files: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
