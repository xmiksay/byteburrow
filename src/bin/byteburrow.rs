use byteburrow::{config::Config, db_connect, inotify::InotifyHandler, job::JobRunner};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "byteburrow=debug,tower_http=debug,sea_orm=info,sqlx=warn".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env();
    Config::set(std::sync::Arc::new(config.clone()));
    let db = db_connect(&config)
        .await
        .expect("Failed to connect to database");

    let (job_runner, job_sender) = JobRunner::new(db.clone());
    let inotify_handler = InotifyHandler::new(db.clone(), job_sender.clone());

    tokio::select! {
        _ = job_runner.run() => tracing::warn!("Job runner exited"),
        _ = inotify_handler.run() => tracing::warn!("Inotify handler exited"),
        _ = byteburrow::web::run(config, db, job_sender) => tracing::warn!("Web server exited"),
    }
}
