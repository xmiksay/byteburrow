use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Storage::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Storage::Id)
                            .unsigned()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Storage::Name).string().not_null())
                    .col(ColumnDef::new(Storage::Description).string())
                    .col(ColumnDef::new(Storage::Path).string().not_null())
                    .col(ColumnDef::new(Storage::DefaultUser).unsigned().not_null())
                    .col(ColumnDef::new(Storage::DefaultGroup).unsigned().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_storage_default_user")
                            .from(Storage::Table, Storage::DefaultUser)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Restrict)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_storage_default_group")
                            .from(Storage::Table, Storage::DefaultGroup)
                            .to(Group::Table, Group::Id)
                            .on_delete(ForeignKeyAction::Restrict)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Storage::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Storage {
    Table,
    Id,
    Name,
    Description,
    Path,
    DefaultUser,
    DefaultGroup,
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
