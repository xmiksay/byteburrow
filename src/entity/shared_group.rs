use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "shared_group")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i32,
    pub shared_id: i32,
    pub group_id: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::shared::Entity",
        from = "Column::SharedId",
        to = "super::shared::Column::Id"
    )]
    Shared,
    #[sea_orm(
        belongs_to = "super::group::Entity",
        from = "Column::GroupId",
        to = "super::group::Column::Id"
    )]
    Group,
}

impl Related<super::shared::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Shared.def()
    }
}

impl Related<super::group::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Group.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
