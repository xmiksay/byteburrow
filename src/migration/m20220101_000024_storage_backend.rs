use sea_orm_migration::prelude::*;

/// Storage backend discriminator + remote (Nextcloud/WebDAV) credentials.
///
/// `backend` defaults to `'local'` so pre-existing rows keep their
/// filesystem-backed semantics with zero data backfill. The `remote_*`
/// columns are only meaningful for non-local backends:
///
/// - `remote_url`      — Nextcloud server base URL (e.g. `https://cloud.example.org`)
/// - `remote_username` — Nextcloud login name (also the DAV files-namespace segment)
/// - `remote_password` — Nextcloud **app password** (device password), stored as-is
///
/// `storage.path` stays NOT NULL: for remote storages it holds the canonical
/// DAV base URL, which keeps the existing path-uniqueness check meaningful.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Storage::Table)
                    .add_column(
                        ColumnDef::new(Storage::Backend)
                            .text()
                            .not_null()
                            .default("local"),
                    )
                    .to_owned(),
            )
            .await?;

        for col in [
            Storage::RemoteUrl,
            Storage::RemoteUsername,
            Storage::RemotePassword,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(Storage::Table)
                        .add_column(ColumnDef::new(col).text())
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for col in [
            Storage::RemotePassword,
            Storage::RemoteUsername,
            Storage::RemoteUrl,
            Storage::Backend,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(Storage::Table)
                        .drop_column(col)
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Storage {
    Table,
    Backend,
    RemoteUrl,
    RemoteUsername,
    RemotePassword,
}
