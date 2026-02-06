use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Shared::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Shared::Id)
                            .unsigned()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Shared::PathId).unsigned().not_null())
                    .col(ColumnDef::new(Shared::Token).string())
                    .col(ColumnDef::new(Shared::CanWrite).boolean().not_null().default(false))
                    .col(ColumnDef::new(Shared::ExpiresAt).timestamp())
                    .col(
                        ColumnDef::new(Shared::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_shared_path")
                            .from(Shared::Table, Shared::PathId)
                            .to(Path::Table, Path::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Shared::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Shared {
    Table,
    Id,
    PathId,
    Token,
    CanWrite,
    ExpiresAt,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Path {
    Table,
    Id,
}
