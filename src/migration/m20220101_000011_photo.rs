use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Photo::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Photo::Hash)
                            .binary()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Photo::Latitude).double())
                    .col(ColumnDef::new(Photo::Longitude).double())
                    .col(ColumnDef::new(Photo::Date).timestamp())
                    .col(
                        ColumnDef::new(Photo::Keywords)
                            .array(ColumnType::String(StringLen::None))
                            .not_null()
                            .default(Expr::cust("ARRAY[]::TEXT[]")),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Photo::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Photo {
    Table,
    Hash,
    Latitude,
    Longitude,
    Date,
    Keywords,
}
