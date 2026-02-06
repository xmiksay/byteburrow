use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "token")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: u32,
    pub user_id: u32,
    pub nonce: String,
    pub expires_at: DateTime,
    pub user_agent: Option<String>,
    pub ip_address: Option<String>,
    pub last_activity: Option<DateTime>,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id"
    )]
    User,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

impl Model {
    /// Check if the token is expired
    pub fn is_expired(&self, now: DateTime) -> bool {
        self.expires_at < now
    }

    /// Check if the token is valid (not expired)
    pub fn is_valid(&self, now: DateTime) -> bool {
        !self.is_expired(now)
    }
}
