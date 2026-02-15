use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Entry type enum - File, Directory, or Symlink
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    EnumIter,
    DeriveActiveEnum,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "entry_type_enum")]
pub enum EntryType {
    #[sea_orm(string_value = "file")]
    File,
    #[sea_orm(string_value = "directory")]
    Directory,
    #[sea_orm(string_value = "symlink")]
    Symlink,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "entry")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i32,
    pub storage_id: i32,
    pub user_id: i32,
    pub group_id: i32,
    pub parent_id: Option<i32>,
    pub path: String,
    pub hash: Option<Vec<u8>>,
    pub entry_type: EntryType,
    pub size: i64,
    pub tags: Vec<i32>,
    pub modified_at: DateTime,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::storage::Entity",
        from = "Column::StorageId",
        to = "super::storage::Column::Id"
    )]
    Storage,
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id"
    )]
    User,
    #[sea_orm(
        belongs_to = "super::group::Entity",
        from = "Column::GroupId",
        to = "super::group::Column::Id"
    )]
    Group,
    #[sea_orm(belongs_to = "Entity", from = "Column::ParentId", to = "Column::Id")]
    Parent,
}

impl Related<super::storage::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Storage.def()
    }
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl Related<super::group::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Group.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

impl Model {
    /// Check if entry is a directory
    pub fn is_directory(&self) -> bool {
        self.entry_type == EntryType::Directory
    }

    /// Check if entry is a file
    pub fn is_file(&self) -> bool {
        self.entry_type == EntryType::File
    }

    /// Check if entry is a symlink
    pub fn is_symlink(&self) -> bool {
        self.entry_type == EntryType::Symlink
    }

    /// Check if entry has a specific tag
    pub fn has_tag(&self, tag_id: i32) -> bool {
        self.tags.contains(&tag_id)
    }
}
