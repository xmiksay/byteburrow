use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// ContactList - Main container for organizing contacts
/// Each contact list belongs to a user and a group
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "contact_list")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: u32,
    pub user_id: u32,
    pub group_id: u32,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "crate::entity::user::Entity",
        from = "Column::UserId",
        to = "crate::entity::user::Column::Id"
    )]
    User,
    #[sea_orm(
        belongs_to = "crate::entity::group::Entity",
        from = "Column::GroupId",
        to = "crate::entity::group::Column::Id"
    )]
    Group,
    #[sea_orm(has_many = "super::contact::Entity")]
    Contacts,
}

impl Related<crate::entity::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl Related<crate::entity::group::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Group.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
