use byteburrow::migration::Migrator;
use sea_orm_migration::prelude::*;

#[tokio::main]
async fn main() {
    let config = byteburrow::config::Config::from_env();
    // Safe because we are at the very beginning of main and no other threads are running yet
    unsafe { std::env::set_var("DATABASE_URL", config.database_url) };
    cli::run_cli(Migrator).await;
}
