use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ContactEmail::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ContactEmail::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ContactEmail::ContactId).integer().not_null())
                    .col(ColumnDef::new(ContactEmail::Email).string().not_null())
                    .col(ColumnDef::new(ContactEmail::EmailType).string().not_null())
                    .col(ColumnDef::new(ContactEmail::Preference).integer())
                    .col(
                        ColumnDef::new(ContactEmail::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_contact_email_contact")
                            .from(ContactEmail::Table, ContactEmail::ContactId)
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
                    .name("idx_contact_email_contact_id")
                    .table(ContactEmail::Table)
                    .col(ContactEmail::ContactId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ContactEmail::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ContactEmail {
    Table,
    Id,
    ContactId,
    Email,
    EmailType,
    Preference,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Contact {
    Table,
    Id,
}
