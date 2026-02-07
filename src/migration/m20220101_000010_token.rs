use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Token::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Token::Nonce)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Token::UserId).integer().not_null())
                    .col(ColumnDef::new(Token::ExpiresAt).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(Token::UserAgent).string())
                    .col(ColumnDef::new(Token::IpAddress).string())
                    .col(ColumnDef::new(Token::LastActivity).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(Token::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_token_user")
                            .from(Token::Table, Token::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Create index on nonce for faster lookups
        manager
            .create_index(
                Index::create()
                    .name("idx_token_nonce")
                    .table(Token::Table)
                    .col(Token::Nonce)
                    .to_owned(),
            )
            .await?;

        // Create index on user_id for faster lookups
        manager
            .create_index(
                Index::create()
                    .name("idx_token_user_id")
                    .table(Token::Table)
                    .col(Token::UserId)
                    .to_owned(),
            )
            .await?;

        // Create index on expires_at for cleanup queries
        manager
            .create_index(
                Index::create()
                    .name("idx_token_expires_at")
                    .table(Token::Table)
                    .col(Token::ExpiresAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Token::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Token {
    Table,
    Nonce,
    UserId,
    ExpiresAt,
    UserAgent,
    IpAddress,
    LastActivity,
    CreatedAt,
}

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
}
