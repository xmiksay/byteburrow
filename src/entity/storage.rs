use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "storage")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
    pub path: String,
    pub default_user: i32,
    pub default_group: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::DefaultUser",
        to = "super::user::Column::Id"
    )]
    DefaultUser,
    #[sea_orm(
        belongs_to = "super::group::Entity",
        from = "Column::DefaultGroup",
        to = "super::group::Column::Id"
    )]
    DefaultGroup,
    #[sea_orm(has_many = "super::entry::Entity")]
    Path,
}

impl Related<super::entry::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Path.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
