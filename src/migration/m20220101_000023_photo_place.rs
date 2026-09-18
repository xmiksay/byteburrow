use sea_orm_migration::prelude::*;

/// #2: `photo.place` — the human-readable location ("Pl. de la Comédie,
/// Geneva, Switzerland") resolved from the photo's EXIF coordinates by the
/// host-side reverse-geocoding provider seam (`src/geo.rs`). Nullable: rows
/// without GPS or whose provider lookup failed/skipped stay `NULL`, and the
/// CLI `photo-geocode` command backfills them on demand.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Photo::Table)
                    .add_column(ColumnDef::new(Photo::Place).text())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Photo::Table)
                    .drop_column(Photo::Place)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Photo {
    Table,
    Place,
}
