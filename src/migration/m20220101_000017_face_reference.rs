use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(FaceReference::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(FaceReference::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(FaceReference::ContactId).integer())
                    .col(
                        ColumnDef::new(FaceReference::Hash)
                            .binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FaceReference::FaceIndex)
                            .small_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FaceReference::BboxX)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FaceReference::BboxY)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FaceReference::BboxW)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FaceReference::BboxH)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FaceReference::Embedding)
                            .binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FaceReference::Confirmed)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(FaceReference::Table, FaceReference::ContactId)
                            .to(Contact::Table, Contact::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(FaceReference::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum FaceReference {
    Table,
    Id,
    ContactId,
    Hash,
    FaceIndex,
    BboxX,
    BboxY,
    BboxW,
    BboxH,
    Embedding,
    Confirmed,
}

#[derive(DeriveIden)]
enum Contact {
    Table,
    Id,
}
