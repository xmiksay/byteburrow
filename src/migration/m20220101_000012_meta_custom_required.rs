use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Fill existing NULLs with empty JSON object
        manager
            .get_connection()
            .execute_unprepared("UPDATE meta SET custom = '{}' WHERE custom IS NULL")
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Meta::Table)
                    .modify_column(
                        ColumnDef::new(Meta::Custom)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'{}'::jsonb")),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Meta::Table)
                    .modify_column(ColumnDef::new(Meta::Custom).json_binary().null())
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Meta {
    Table,
    Custom,
}
