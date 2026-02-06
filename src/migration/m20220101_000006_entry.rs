use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Entry::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Entry::Id)
                            .unsigned()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Entry::StorageId).unsigned().not_null())
                    .col(ColumnDef::new(Entry::UserId).unsigned().not_null())
                    .col(ColumnDef::new(Entry::GroupId).unsigned().not_null())
                    .col(ColumnDef::new(Entry::ParentId).unsigned())
                    .col(ColumnDef::new(Entry::Path).string().not_null())
                    .col(ColumnDef::new(Entry::Hash).string())
                    .col(
                        ColumnDef::new(Entry::EntryType)
                            .enumeration(
                                Alias::new("entry_type_enum"),
                                [
                                    Alias::new("file"),
                                    Alias::new("directory"),
                                    Alias::new("symlink"),
                                ],
                            )
                            .not_null(),
                    )
                    .col(ColumnDef::new(Entry::Size).big_integer())
                    .col(
                        ColumnDef::new(Entry::Tags)
                            .array(ColumnType::Integer)
                            .not_null()
                            .default(Expr::cust("ARRAY[]::INTEGER[]")),
                    )
                    .col(ColumnDef::new(Entry::ModifiedAt).timestamp())
                    .col(
                        ColumnDef::new(Entry::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_entry_storage")
                            .from(Entry::Table, Entry::StorageId)
                            .to(Storage::Table, Storage::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_entry_user")
                            .from(Entry::Table, Entry::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Restrict)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_entry_group")
                            .from(Entry::Table, Entry::GroupId)
                            .to(Group::Table, Group::Id)
                            .on_delete(ForeignKeyAction::Restrict)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_entry_parent")
                            .from(Entry::Table, Entry::ParentId)
                            .to(Entry::Table, Entry::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Entry::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Entry {
    Table,
    Id,
    StorageId,
    UserId,
    GroupId,
    ParentId,
    Path,
    Hash,
    EntryType,
    Size,
    Tags,
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
