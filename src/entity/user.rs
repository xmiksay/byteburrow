use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "user")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
    pub username: String,
    pub password: String,
    pub enabled: bool,
    pub admin: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::tag::Entity")]
    Tag,
}

impl Related<super::tag::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Tag.def()
    }
}

impl Related<super::group::Entity> for Entity {
    fn to() -> RelationDef {
        super::group_user::Relation::Group.def()
    }

    fn via() -> Option<RelationDef> {
        Some(super::group_user::Relation::User.def().rev())
    }
}

impl ActiveModelBehavior for ActiveModel {}
