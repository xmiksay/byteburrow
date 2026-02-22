use sea_orm::entity::prelude::*;
use sea_orm::{ColIdx, TryGetError, TryGetable};
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

/// Individual kind flag constants describing what a file is and how it can be handled.
///
/// Values are powers of 2 so they can be combined as bitflags inside [`Kinds`].
/// Groups share a bit range to allow category-level masking:
///
/// | Range     | Category |
/// |-----------|----------|
/// | 0x001–0x00F | Text / document |
/// | 0x020–0x07F | Image |
/// | 0x100–0x7FF | Video |
/// | 0x800–0x7FFF | Audio |
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    // --- Text group (showable + editable in browser) ---
    /// Plain text file
    Text = 0x001,
    /// Source code / programming language
    Language = 0x002,
    /// Markdown document (rendered + editable)
    Markdown = 0x004,
    /// Rich document (Word, ODT, PDF-like)
    Document = 0x008,

    // --- Image group (showable, not editable) ---
    /// Generic raster or vector image
    Image = 0x020,
    /// Photograph (carries EXIF/GPS metadata)
    Photo = 0x040,

    // --- Video group (streamable + showable) ---
    /// Generic video clip
    Video = 0x100,
    /// Feature film / movie
    Film = 0x200,

    // --- Audio group (streamable) ---
    /// Generic audio recording
    Audio = 0x800,
    /// Music track (carries ID3/artist metadata)
    Song = 0x1000,
}

const TEXT_MASK: i32 = 0x00F;
const IMAGE_MASK: i32 = 0x0E0;
const VIDEO_MASK: i32 = 0x700;
const AUDIO_MASK: i32 = 0x7800;
const EDIT_MASK: i32 = Kind::Text as i32 | Kind::Language as i32 | Kind::Markdown as i32;
const STREAM_MASK: i32 = VIDEO_MASK | AUDIO_MASK;

/// Bitfield of [`Kind`] flags stored as an `i32` in the database.
///
/// A single file may carry multiple flags (e.g. `Photo | Image`).
/// Use the capability helpers to decide how to handle the file in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Kinds(pub i32);

impl Kinds {
    pub fn has(self, kind: Kind) -> bool {
        self.0 & (kind as i32) != 0
    }

    // --- Category checks ---

    pub fn is_text(self) -> bool {
        self.0 & TEXT_MASK != 0
    }

    pub fn is_image(self) -> bool {
        self.0 & IMAGE_MASK != 0
    }

    pub fn is_video(self) -> bool {
        self.0 & VIDEO_MASK != 0
    }

    pub fn is_audio(self) -> bool {
        self.0 & AUDIO_MASK != 0
    }

    // --- Web capability checks ---

    /// Can be rendered / previewed directly in the browser.
    pub fn can_show(self) -> bool {
        self.is_text() || self.is_image() || self.is_video() || self.is_audio()
    }

    /// Can be opened in an in-browser text / code editor.
    pub fn can_edit(self) -> bool {
        self.0 & EDIT_MASK != 0
    }

    /// Has a media player (video or audio stream).
    pub fn can_stream(self) -> bool {
        self.0 & STREAM_MASK != 0
    }
}

impl From<i32> for Kinds {
    fn from(v: i32) -> Self {
        Self(v)
    }
}

impl From<Kind> for Kinds {
    fn from(k: Kind) -> Self {
        Self(k as i32)
    }
}

/// Combine two individual flags: `Kind::Photo | Kind::Image`
impl std::ops::BitOr for Kind {
    type Output = Kinds;
    fn bitor(self, rhs: Kind) -> Kinds {
        Kinds(self as i32 | rhs as i32)
    }
}

/// Extend a `Kinds` bitfield with one more flag: `kinds | Kind::Song`
impl std::ops::BitOr<Kind> for Kinds {
    type Output = Self;
    fn bitor(self, rhs: Kind) -> Self {
        Self(self.0 | rhs as i32)
    }
}

impl From<Kinds> for Value {
    fn from(k: Kinds) -> Self {
        Value::Int(Some(k.0))
    }
}

impl TryGetable for Kinds {
    fn try_get_by<I: ColIdx>(res: &QueryResult, idx: I) -> Result<Self, TryGetError> {
        <i32 as TryGetable>::try_get_by(res, idx).map(Kinds)
    }
}

impl sea_orm::sea_query::ValueType for Kinds {
    fn try_from(v: Value) -> Result<Self, sea_orm::sea_query::ValueTypeErr> {
        match v {
            Value::Int(Some(v)) => Ok(Kinds(v)),
            _ => Err(sea_orm::sea_query::ValueTypeErr),
        }
    }

    fn type_name() -> String {
        "Kinds".to_owned()
    }

    fn array_type() -> sea_orm::sea_query::ArrayType {
        sea_orm::sea_query::ArrayType::Int
    }

    fn column_type() -> sea_orm::sea_query::ColumnType {
        sea_orm::sea_query::ColumnType::Integer
    }
}

impl sea_orm::sea_query::Nullable for Kinds {
    fn null() -> Value {
        Value::Int(None)
    }
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
    pub notify: bool,
    pub kind: Kinds,
    pub size: i64,
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
    pub fn is_directory(&self) -> bool {
        self.entry_type == EntryType::Directory
    }

    pub fn is_file(&self) -> bool {
        self.entry_type == EntryType::File
    }

    pub fn is_symlink(&self) -> bool {
        self.entry_type == EntryType::Symlink
    }
}
