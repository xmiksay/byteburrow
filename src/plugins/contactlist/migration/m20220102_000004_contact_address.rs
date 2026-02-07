use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ContactAddress::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ContactAddress::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ContactAddress::ContactId).integer().not_null())
                    .col(ColumnDef::new(ContactAddress::AddressType).string().not_null())
                    .col(ColumnDef::new(ContactAddress::PoBox).string())
                    .col(ColumnDef::new(ContactAddress::ExtendedAddress).string())
                    .col(ColumnDef::new(ContactAddress::StreetAddress).string())
                    .col(ColumnDef::new(ContactAddress::Locality).string())
                    .col(ColumnDef::new(ContactAddress::Region).string())
                    .col(ColumnDef::new(ContactAddress::PostalCode).string())
                    .col(ColumnDef::new(ContactAddress::Country).string())
                    .col(ColumnDef::new(ContactAddress::Label).text())
                    .col(
                        ColumnDef::new(ContactAddress::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_contact_address_contact")
                            .from(ContactAddress::Table, ContactAddress::ContactId)
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
                    .name("idx_contact_address_contact_id")
                    .table(ContactAddress::Table)
                    .col(ContactAddress::ContactId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ContactAddress::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ContactAddress {
    Table,
    Id,
    ContactId,
    AddressType,
    PoBox,
    ExtendedAddress,
    StreetAddress,
    Locality,
    Region,
    PostalCode,
    Country,
    Label,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Contact {
    Table,
    Id,
}
