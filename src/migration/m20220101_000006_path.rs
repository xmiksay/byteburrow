use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Path::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Path::Id)
                            .unsigned()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Path::StorageId).unsigned().not_null())
                    .col(ColumnDef::new(Path::UserId).unsigned().not_null())
                    .col(ColumnDef::new(Path::GroupId).unsigned().not_null())
                    .col(ColumnDef::new(Path::ParentId).unsigned())
                    .col(ColumnDef::new(Path::Path).string().not_null())
                    .col(ColumnDef::new(Path::Hash).string())
                    .col(ColumnDef::new(Path::IsDirectory).boolean().not_null())
                    .col(ColumnDef::new(Path::Size).big_integer())
                    .col(ColumnDef::new(Path::ModifiedAt).timestamp())
                    .col(
                        ColumnDef::new(Path::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_path_storage")
                            .from(Path::Table, Path::StorageId)
                            .to(Storage::Table, Storage::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_path_user")
                            .from(Path::Table, Path::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Restrict)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_path_group")
                            .from(Path::Table, Path::GroupId)
                            .to(Group::Table, Group::Id)
                            .on_delete(ForeignKeyAction::Restrict)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_path_parent")
                            .from(Path::Table, Path::ParentId)
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
            .drop_table(Table::drop().table(Path::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Path {
    Table,
    Id,
    StorageId,
    UserId,
    GroupId,
    ParentId,
    Path,
    Hash,
    IsDirectory,
    Size,
    ModifiedAt,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Storage {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Group {
    Table,
    Id,
}
