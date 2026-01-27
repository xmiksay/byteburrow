use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Dummy::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Dummy::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Dummy::Name).string().not_null())
                    .col(ColumnDef::new(Dummy::Description).string())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Dummy::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Dummy {
    Table,
    Id,
    Name,
    Description,
}
