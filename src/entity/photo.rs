use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "photo")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub hash: Vec<u8>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub date: Option<DateTime>,
    pub keywords: Vec<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
