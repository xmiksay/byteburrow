use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "shared")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i32,
    pub path_id: i32,
    pub token: Option<String>,
    pub can_write: bool,
    pub user_ids: Vec<i32>,
    pub group_ids: Vec<i32>,
    pub expires_at: Option<DateTime>,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::entry::Entity",
        from = "Column::PathId",
        to = "super::entry::Column::Id"
    )]
    Path,
}

impl Related<super::entry::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Path.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
