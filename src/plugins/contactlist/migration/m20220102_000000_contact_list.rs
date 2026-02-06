use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ContactList::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ContactList::Id)
                            .unsigned()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ContactList::UserId).unsigned().not_null())
                    .col(ColumnDef::new(ContactList::GroupId).unsigned().not_null())
                    .col(ColumnDef::new(ContactList::Name).string().not_null())
                    .col(ColumnDef::new(ContactList::Description).text())
                    .col(
                        ColumnDef::new(ContactList::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(ContactList::UpdatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_contact_list_user")
                            .from(ContactList::Table, ContactList::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_contact_list_group")
                            .from(ContactList::Table, ContactList::GroupId)
                            .to(Group::Table, Group::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Create indexes for common queries
        manager
            .create_index(
                Index::create()
                    .name("idx_contact_list_user_id")
                    .table(ContactList::Table)
                    .col(ContactList::UserId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_contact_list_group_id")
                    .table(ContactList::Table)
                    .col(ContactList::GroupId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ContactList::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ContactList {
    Table,
    Id,
    UserId,
    GroupId,
    Name,
    Description,
    CreatedAt,
    UpdatedAt,
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
