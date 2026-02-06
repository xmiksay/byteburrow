use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Contact::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Contact::Id)
                            .unsigned()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Contact::ContactListId).unsigned().not_null())
                    .col(ColumnDef::new(Contact::Uid).string().not_null().unique_key())
                    .col(ColumnDef::new(Contact::Rev).timestamp().not_null())
                    .col(ColumnDef::new(Contact::Fn).string().not_null())
                    .col(ColumnDef::new(Contact::FamilyName).string())
                    .col(ColumnDef::new(Contact::GivenName).string())
                    .col(ColumnDef::new(Contact::AdditionalName).string())
                    .col(ColumnDef::new(Contact::HonorificPrefix).string())
                    .col(ColumnDef::new(Contact::HonorificSuffix).string())
                    .col(ColumnDef::new(Contact::Nickname).string())
                    .col(ColumnDef::new(Contact::Birthday).string())
                    .col(ColumnDef::new(Contact::Organization).string())
                    .col(ColumnDef::new(Contact::OrganizationUnit).string())
                    .col(ColumnDef::new(Contact::Title).string())
                    .col(ColumnDef::new(Contact::Note).text())
                    .col(ColumnDef::new(Contact::Photo).text())
                    .col(ColumnDef::new(Contact::Categories).text())
                    .col(ColumnDef::new(Contact::VcardData).text())
                    .col(ColumnDef::new(Contact::Etag).string().not_null())
                    .col(ColumnDef::new(Contact::IsFavorite).boolean().not_null().default(false))
                    .col(
                        ColumnDef::new(Contact::CreatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Contact::UpdatedAt)
                            .timestamp()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_contact_contact_list")
                            .from(Contact::Table, Contact::ContactListId)
                            .to(ContactList::Table, ContactList::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Create indexes for CardDAV queries
        manager
            .create_index(
                Index::create()
                    .name("idx_contact_uid")
                    .table(Contact::Table)
                    .col(Contact::Uid)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_contact_contact_list_id")
                    .table(Contact::Table)
                    .col(Contact::ContactListId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_contact_etag")
                    .table(Contact::Table)
                    .col(Contact::Etag)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Contact::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Contact {
    Table,
    Id,
    ContactListId,
    Uid,
    Rev,
    Fn,
    FamilyName,
    GivenName,
    AdditionalName,
    HonorificPrefix,
    HonorificSuffix,
    Nickname,
    Birthday,
    Organization,
    OrganizationUnit,
    Title,
    Note,
    Photo,
    Categories,
    VcardData,
    Etag,
    IsFavorite,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ContactList {
    Table,
    Id,
}
