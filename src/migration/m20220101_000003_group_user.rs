use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(GroupUser::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(GroupUser::Id)
                            .unsigned()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(GroupUser::UserId).unsigned().not_null())
                    .col(ColumnDef::new(GroupUser::GroupId).unsigned().not_null())
                    .col(ColumnDef::new(GroupUser::Admin).boolean().not_null().default(false))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_group_user_user")
                            .from(GroupUser::Table, GroupUser::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_group_user_group")
                            .from(GroupUser::Table, GroupUser::GroupId)
                            .to(Group::Table, Group::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(GroupUser::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum GroupUser {
    Table,
    Id,
    UserId,
    GroupId,
    Admin,
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
