use sea_orm_migration::prelude::*;

/// Model identity that produced every embedding written before this migration.
/// Backfill existing rows with it so legacy vectors stay comparable among
/// themselves (they were all produced by the same model) but are refused
/// against any future model whose id/version differs.
const LEGACY_MODEL_ID: &str = "faceonnx-recognition-resnet27";
const LEGACY_MODEL_VERSION: &str = "1";

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
                        ColumnDef::new(FaceReference::ModelId)
                            .string()
                            .not_null()
                            .default(LEGACY_MODEL_ID),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(FaceReference::Table)
                    .add_column(
                        ColumnDef::new(FaceReference::ModelVersion)
                            .string()
                            .not_null()
                            .default(LEGACY_MODEL_VERSION),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(FaceReference::Table)
                    .add_column(
                        ColumnDef::new(FaceReference::Dim)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;

        // Backfill dim from the stored little-endian f32 byte length.
        manager
            .get_connection()
            .execute_unprepared("UPDATE face_reference SET dim = octet_length(embedding) / 4")
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(FaceReference::Table)
                    .drop_column(FaceReference::Dim)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(FaceReference::Table)
                    .drop_column(FaceReference::ModelVersion)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(FaceReference::Table)
                    .drop_column(FaceReference::ModelId)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum FaceReference {
    Table,
    ModelId,
    ModelVersion,
    Dim,
}
