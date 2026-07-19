use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Meta::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Meta::Hash).binary().not_null().primary_key())
                    .col(
                        ColumnDef::new(Meta::Tags)
                            .array(ColumnType::Integer)
                            .not_null()
                            .default(Expr::cust("ARRAY[]::INTEGER[]")),
                    )
                    .col(
                        ColumnDef::new(Meta::Keywords)
                            .array(ColumnType::Text)
                            .not_null()
                            .default(Expr::cust("ARRAY[]::TEXT[]")),
                    )
                    .col(ColumnDef::new(Meta::Custom).json_binary())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Meta::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Meta {
    Table,
    Hash,
    Tags,
    Keywords,
    Custom,
}
