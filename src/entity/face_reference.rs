use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "face_reference")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub contact_id: Option<i32>,
    pub hash: Vec<u8>,
    pub face_index: i16,
    pub bbox_x: i32,
    pub bbox_y: i32,
    pub bbox_w: i32,
    pub bbox_h: i32,
    #[serde(skip)]
    pub embedding: Vec<u8>,
    pub confirmed: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
