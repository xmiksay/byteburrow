use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Contact entity following vCard 4.0 specification (RFC 6350) for CardDAV compatibility
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "contact")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i32,
    pub contact_list_id: i32,

    // vCard UID - unique identifier for CardDAV sync
    #[sea_orm(unique)]
    pub uid: String,

    // vCard REV - revision timestamp for CardDAV sync
    pub rev: DateTime,

    // vCard FN (Formatted Name) - required field
    #[sea_orm(column_name = "fn")]
    pub formatted_name: String,

    // vCard N (Name) components
    pub family_name: Option<String>,      // surname/last name
    pub given_name: Option<String>,       // first name
    pub additional_name: Option<String>,  // middle name
    pub honorific_prefix: Option<String>, // Mr., Mrs., Dr., etc.
    pub honorific_suffix: Option<String>, // Jr., Sr., PhD, etc.

    // vCard NICKNAME
    pub nickname: Option<String>,

    // vCard BDAY (Birthday) - ISO 8601 format
    pub birthday: Option<String>,

    // vCard ORG (Organization)
    pub organization: Option<String>,
    pub organization_unit: Option<String>,

    // vCard TITLE (Job Title)
    pub title: Option<String>,

    // vCard NOTE
    pub note: Option<String>,

    // vCard PHOTO - URL or data URI
    pub photo: Option<String>,

    // vCard CATEGORIES - comma-separated tags
    pub categories: Option<String>,

    // Full vCard data (for storing complete vCard including extensions)
    pub vcard_data: Option<String>,

    // Metadata
    pub etag: String,              // for CardDAV ETag
    pub is_favorite: bool,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::contact_list::Entity",
        from = "Column::ContactListId",
        to = "super::contact_list::Column::Id"
    )]
    ContactList,
}

impl Related<super::contact_list::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ContactList.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

impl Model {
    /// Get formatted name (FN from vCard)
    pub fn get_formatted_name(&self) -> &str {
        &self.formatted_name
    }

    /// Build vCard N property from name components
    pub fn structured_name(&self) -> String {
        format!(
            "{};{};{};{};{}",
            self.family_name.as_deref().unwrap_or(""),
            self.given_name.as_deref().unwrap_or(""),
            self.additional_name.as_deref().unwrap_or(""),
            self.honorific_prefix.as_deref().unwrap_or(""),
            self.honorific_suffix.as_deref().unwrap_or("")
        )
    }
}
