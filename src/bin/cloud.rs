use cloud::{db_connect, config::Config};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cloud=debug,tower_http=debug,sea_orm=info,sqlx=warn".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env();
    Config::set(std::sync::Arc::new(config.clone()));
    let db = db_connect(&config)
        .await
        .expect("Failed to connect to database");

    cloud::web::run(config, db).await;
}
