use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Create enum type for entry_type
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TYPE entry_type_enum AS ENUM ('file', 'directory', 'symlink')",
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Entry::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Entry::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Entry::StorageId).integer().not_null())
                    .col(ColumnDef::new(Entry::UserId).integer().not_null())
                    .col(ColumnDef::new(Entry::GroupId).integer().not_null())
                    .col(ColumnDef::new(Entry::ParentId).integer())
                    .col(ColumnDef::new(Entry::Path).string().not_null())
                    .col(ColumnDef::new(Entry::Hash).binary())
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
                    .col(
                        ColumnDef::new(Entry::Notify)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(Entry::Kind)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(Entry::Size).big_integer().not_null())
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
            .await?;

        manager
            .get_connection()
            .execute_unprepared("DROP TYPE IF EXISTS entry_type_enum")
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
#[allow(clippy::enum_variant_names)]
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
    Notify,
    Kind,
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
