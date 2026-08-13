use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// A persisted WebDAV lock token (C4 Part C). Mirrors the in-memory `Lock`
/// in [`crate::web::dav::util`] so locks survive a process restart.
///
/// This table is a durability shadow of the in-process lock map: writes go to
/// both, reads at request time hit the in-memory map, and the table is
/// rehydrated into the map at startup by
/// [`crate::web::dav::util::load_active_locks`].
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "dav_lock")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i32,
    /// RFC 4918 lock token, e.g. `opaquelocktoken:<uuid>`. Unique.
    pub token: String,
    pub storage_id: i32,
    /// Locked path within the storage (root-relative, no leading slash).
    pub path: String,
    /// `0` or `255` (infinity) — matches the in-memory `Lock::depth` (u8).
    pub depth: i16,
    /// Serialized `<D:owner>` value.
    pub owner: String,
    /// The user that holds the lock — enforces token ownership on UNLOCK and
    /// write enforcement (C4 Part A).
    pub user_id: i32,
    pub expires_at: DateTimeUtc,
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

impl ActiveModelBehavior for ActiveModel {}
