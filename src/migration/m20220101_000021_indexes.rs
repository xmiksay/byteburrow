use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // M10: composite index on face_reference(hash, face_index) — the face
        // pipeline and the CLI face_match tool both query by (hash, face_index).
        manager
            .create_index(
                Index::create()
                    .name("idx_face_reference_hash_face_index")
                    .table(FaceReference::Table)
                    .col(FaceReference::Hash)
                    .col(FaceReference::FaceIndex)
                    .to_owned(),
            )
            .await?;

        // M10: index on entry.hash — the job runner, thumbnail regeneration,
        // and the inotify deletion cascade all look up entries by hash.
        manager
            .create_index(
                Index::create()
                    .name("idx_entry_hash")
                    .table(Entry::Table)
                    .col(Entry::Hash)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_entry_hash")
                    .table(Entry::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx_face_reference_hash_face_index")
                    .table(FaceReference::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Entry {
    Table,
    Hash,
}

#[derive(DeriveIden)]
enum FaceReference {
    Table,
    Hash,
    FaceIndex,
}
