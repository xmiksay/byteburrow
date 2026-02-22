use sea_orm::entity::prelude::*;
use sea_orm::{ColIdx, TryGetError, TryGetable};
use serde::{Deserialize, Serialize};

/// Individual kind flag constants for file classification.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    // Plain text
    Text = 1,
    // Programming language
    Language = 2,
    // Markdown
    Markdown = 4,
    // Document like world
    RightText = 8,

    // Any image
    Image = 32,
    // Photography
    Photo = 64,

    // Any Video
    Video = 256,
    // Film
    Film = 512,

    // Any Audio
    Audio = 2048,
    // Interpreted music
    Song = 4096,
}

/// Bitfield of Kind flags stored as an i32 in the database.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Kinds(pub i32);

impl Kinds {
    pub fn has(self, kind: Kind) -> bool {
        self.0 & (kind as i32) != 0
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

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "meta")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub hash: Vec<u8>,
    pub tags: Vec<i32>,
    pub keywords: Vec<String>,
    pub kind: Kinds,
    pub custom: Option<Json>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
