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
    /// Identity of the embedding model that produced `embedding`. Embeddings
    /// from different (`model_id`, `model_version`) pairs live in incomparable
    /// vector spaces and must never be compared — see `crate::job::face`.
    pub model_id: String,
    pub model_version: String,
    /// Number of f32 components in `embedding` (i.e. `embedding.len() / 4`).
    pub dim: i32,
    pub confirmed: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
