use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Entry::Table)
                    .add_column(
                        ColumnDef::new(Entry::SkipPlugins)
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
                    .table(Entry::Table)
                    .drop_column(Entry::SkipPlugins)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Entry {
    Table,
    SkipPlugins,
}
