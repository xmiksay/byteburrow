use sea_orm_migration::prelude::*;

/// Persist WebDAV lock tokens so they survive a process restart (C4 Part C).
///
/// The in-process lock map remains the request-time source of truth (fast
/// path); this table is a durability shadow written on `LOCK`, deleted on
/// `UNLOCK`, and reloaded at startup. Binding `user_id` here is what lets
/// `UNLOCK` and write-path enforcement reject tokens presented by users other
/// than the lock owner (C4 Part A).
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(DavLock::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(DavLock::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(DavLock::Token).string().not_null())
                    .col(ColumnDef::new(DavLock::StorageId).integer().not_null())
                    .col(ColumnDef::new(DavLock::Path).string().not_null())
                    .col(ColumnDef::new(DavLock::Depth).small_integer().not_null())
                    .col(ColumnDef::new(DavLock::Owner).string().not_null())
                    .col(ColumnDef::new(DavLock::UserId).integer().not_null())
                    .col(
                        ColumnDef::new(DavLock::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_dav_lock_storage")
                            .from(DavLock::Table, DavLock::StorageId)
                            .to(Storage::Table, Storage::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_dav_lock_user")
                            .from(DavLock::Table, DavLock::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Token lookups (UNLOCK) and the (storage, path) covering scan both
        // need indexes to stay cheap.
        manager
            .create_index(
                Index::create()
                    .name("idx_dav_lock_token")
                    .table(DavLock::Table)
                    .col(DavLock::Token)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_dav_lock_storage_path")
                    .table(DavLock::Table)
                    .col(DavLock::StorageId)
                    .col(DavLock::Path)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DavLock::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum DavLock {
    Table,
    Id,
    Token,
    StorageId,
    Path,
    Depth,
    Owner,
    UserId,
    ExpiresAt,
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
