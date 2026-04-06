use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const DEFAULT_PATTERNS: &str = ".git,.cache,node_modules,.DS_Store,__pycache__,.Trash";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Storage::Table)
                    .add_column(
                        ColumnDef::new(Storage::IgnorePatterns)
                            .text()
                            .not_null()
                            .default(DEFAULT_PATTERNS),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Storage::Table)
                    .drop_column(Storage::IgnorePatterns)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Storage {
    Table,
    IgnorePatterns,
}
