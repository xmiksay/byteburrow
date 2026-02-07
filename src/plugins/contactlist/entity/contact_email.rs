use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// vCard EMAIL entity for CardDAV compatibility
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "contact_email")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i32,
    pub contact_id: i32,

    // vCard EMAIL value
    pub email: String,

    // vCard EMAIL TYPE parameter (home, work, internet, etc.)
    pub email_type: String,

    // vCard PREF parameter (priority/preference)
    pub preference: Option<i32>,

    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::contact::Entity",
        from = "Column::ContactId",
        to = "super::contact::Column::Id"
    )]
    Contact,
}

impl Related<super::contact::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Contact.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
