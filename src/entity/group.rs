use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "group")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        super::group_user::Relation::User.def()
    }

    fn via() -> Option<RelationDef> {
        Some(super::group_user::Relation::Group.def().rev())
    }
}

impl ActiveModelBehavior for ActiveModel {}
