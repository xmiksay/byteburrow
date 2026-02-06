use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "shared")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: u32,
    pub path_id: u32,
    pub token: Option<String>,
    pub can_write: bool,
    pub expires_at: Option<DateTime>,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::path::Entity",
        from = "Column::PathId",
        to = "super::path::Column::Id"
    )]
    Path,
}

impl Related<super::path::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Path.def()
    }
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        super::shared_user::Relation::User.def()
    }

    fn via() -> Option<RelationDef> {
        Some(super::shared_user::Relation::Shared.def().rev())
    }
}

impl Related<super::group::Entity> for Entity {
    fn to() -> RelationDef {
        super::shared_group::Relation::Group.def()
    }

    fn via() -> Option<RelationDef> {
        Some(super::shared_group::Relation::Shared.def().rev())
    }
}

impl ActiveModelBehavior for ActiveModel {}
