use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(SharedGroup::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SharedGroup::Id)
                            .unsigned()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(SharedGroup::SharedId).unsigned().not_null())
                    .col(ColumnDef::new(SharedGroup::GroupId).unsigned().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_shared_group_shared")
                            .from(SharedGroup::Table, SharedGroup::SharedId)
                            .to(Shared::Table, Shared::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_shared_group_group")
                            .from(SharedGroup::Table, SharedGroup::GroupId)
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
            .drop_table(Table::drop().table(SharedGroup::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum SharedGroup {
    Table,
    Id,
    SharedId,
    GroupId,
}

#[derive(DeriveIden)]
enum Shared {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Group {
    Table,
    Id,
}
