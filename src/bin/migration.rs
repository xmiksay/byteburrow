use sea_orm_migration::prelude::*;
use shared::migrations::Migrator;

#[tokio::main]
async fn main() {
    cli::run_cli(Migrator).await;
}
