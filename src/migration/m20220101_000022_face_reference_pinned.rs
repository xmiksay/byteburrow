use sea_orm_migration::prelude::*;

/// E2/E3 (#26/#27): `face_reference.pinned` distinguishes a **human** label
/// from a **machine** suggestion.
///
/// - `confirmed = true` — exemplar: part of the matching pool (implicitly
///   human-pinned; `pinned` is irrelevant for these rows).
/// - `confirmed = false, pinned = true` — human assignment ("this is Alice,
///   but don't learn from it"): never touched by the backfill re-match.
/// - `confirmed = false, pinned = false` — machine suggestion: recomputed
///   (set *or cleared*) whenever the exemplar pool changes.
///
/// Without the flag the backfill cannot tell an API assignment from a
/// match suggestion and would wipe the former on every re-run.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(FaceReference::Table)
                    .add_column(
                        ColumnDef::new(FaceReference::Pinned)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(FaceReference::Table)
                    .drop_column(FaceReference::Pinned)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum FaceReference {
    Table,
    Pinned,
}
