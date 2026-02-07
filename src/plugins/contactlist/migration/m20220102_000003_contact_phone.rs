use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ContactPhone::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ContactPhone::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ContactPhone::ContactId).integer().not_null())
                    .col(ColumnDef::new(ContactPhone::PhoneNumber).string().not_null())
                    .col(ColumnDef::new(ContactPhone::PhoneType).string().not_null())
                    .col(ColumnDef::new(ContactPhone::Preference).integer())
                    .col(
                        ColumnDef::new(ContactPhone::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_contact_phone_contact")
                            .from(ContactPhone::Table, ContactPhone::ContactId)
                            .to(Contact::Table, Contact::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_contact_phone_contact_id")
                    .table(ContactPhone::Table)
                    .col(ContactPhone::ContactId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ContactPhone::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ContactPhone {
    Table,
    Id,
    ContactId,
    PhoneNumber,
    PhoneType,
    Preference,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Contact {
    Table,
    Id,
}
