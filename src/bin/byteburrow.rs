use std::collections::HashMap;
use std::path::Path;

use byteburrow::{
    config::Config, db_connect, inotify::InotifyHandler, job::JobRunner, migration::Migrator,
    plugin::PluginRegistry,
};
use sea_orm_migration::MigratorTrait;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "byteburrow=debug,tower_http=debug,sea_orm=info,sqlx=warn".into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env();
    Config::set(std::sync::Arc::new(config.clone()));
    let db = db_connect(&config)
        .await
        .expect("Failed to connect to database");

    // Run pending migrations
    Migrator::up(&db, None)
        .await
        .expect("Failed to run migrations");

    // Load classifier plugins
    let plugin_config = HashMap::new();
    let plugins =
        PluginRegistry::load_from_directory(Path::new(&config.plugin_dir), &plugin_config);

    plugins.log_summary();

    let (job_runner, job_sender) = JobRunner::new(db.clone(), plugins);
    let inotify_handler = InotifyHandler::new(db.clone(), job_sender.clone());

    // Run the job runner on a dedicated OS thread — it owns its own
    // low-priority tokio runtime and blocks until the channel closes.
    std::thread::Builder::new()
        .name("job-runner".into())
        .spawn(move || job_runner.run())
        .expect("Failed to spawn job runner thread");

    // The main tokio runtime handles only the web server and inotify
    // watcher, keeping request handling at normal OS priority.
    tokio::select! {
        _ = inotify_handler.run() => tracing::warn!("Inotify handler exited"),
        _ = byteburrow::web::run(config, db, job_sender) => tracing::warn!("Web server exited"),
    }
}
