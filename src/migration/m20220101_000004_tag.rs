use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Tag::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Tag::Id)
                            .unsigned()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Tag::UserId).unsigned().not_null())
                    .col(ColumnDef::new(Tag::Name).string().not_null())
                    .col(ColumnDef::new(Tag::Description).string())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_tag_user")
                            .from(Tag::Table, Tag::UserId)
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
            .drop_table(Table::drop().table(Tag::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Tag {
    Table,
    Id,
    UserId,
    Name,
    Description,
}

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
}
