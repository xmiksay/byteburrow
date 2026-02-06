use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// vCard ADR (Address) entity for CardDAV compatibility
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "contact_address")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: u32,
    pub contact_id: u32,

    // vCard ADR TYPE parameter (home, work, postal, etc.)
    pub address_type: String,

    // vCard ADR components (in order)
    pub po_box: Option<String>,          // post office box
    pub extended_address: Option<String>, // apartment, suite number
    pub street_address: Option<String>,   // street address
    pub locality: Option<String>,         // city
    pub region: Option<String>,           // state/province
    pub postal_code: Option<String>,      // postal/zip code
    pub country: Option<String>,          // country name

    // vCard LABEL (formatted address)
    pub label: Option<String>,

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
