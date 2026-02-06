use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(SharedUser::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SharedUser::Id)
                            .unsigned()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(SharedUser::SharedId).unsigned().not_null())
                    .col(ColumnDef::new(SharedUser::UserId).unsigned().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_shared_user_shared")
                            .from(SharedUser::Table, SharedUser::SharedId)
                            .to(Shared::Table, Shared::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_shared_user_user")
                            .from(SharedUser::Table, SharedUser::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(SharedUser::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum SharedUser {
    Table,
    Id,
    SharedId,
    UserId,
}

#[derive(DeriveIden)]
enum Shared {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
}
