use sea_orm_migration::prelude::*;

/// Add an explicit `owner_id` (creator) column to `shared` (issue #32 / G1).
///
/// Previously "who created this share" was only derivable from the backing
/// entry's ownership. This records it authoritatively so it can drive the
/// "my shares" listing and share-management authorization. Existing rows are
/// backfilled from the entry's `user_id`.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1. Add the column nullable so existing rows can be backfilled.
        manager
            .alter_table(
                Table::alter()
                    .table(Shared::Table)
                    .add_column(ColumnDef::new(Shared::OwnerId).integer())
                    .to_owned(),
            )
            .await?;

        // 2. Backfill from the backing entry's owner. `path_id` always points
        //    at a live entry (FK, cascade), so every row gets a value.
        manager
            .get_connection()
            .execute_unprepared(
                "UPDATE shared SET owner_id = entry.user_id \
                 FROM entry WHERE shared.path_id = entry.id",
            )
            .await?;

        // 3. Now that every row has a value, enforce NOT NULL.
        manager
            .alter_table(
                Table::alter()
                    .table(Shared::Table)
                    .modify_column(ColumnDef::new(Shared::OwnerId).integer().not_null())
                    .to_owned(),
            )
            .await?;

        // 4. Reference the user table. `user` is a reserved word, hence the
        //    quoting. Restrict on delete to match `fk_entry_user`.
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE shared ADD CONSTRAINT fk_shared_owner \
                 FOREIGN KEY (owner_id) REFERENCES \"user\" (id) \
                 ON DELETE RESTRICT ON UPDATE CASCADE",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE shared DROP CONSTRAINT IF EXISTS fk_shared_owner")
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Shared::Table)
                    .drop_column(Shared::OwnerId)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Shared {
    Table,
    OwnerId,
}
