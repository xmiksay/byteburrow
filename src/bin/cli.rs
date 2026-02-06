use clap::{Parser, Subcommand};
use cloud::{db_connect, config::Config};

#[derive(Parser)]
#[command(name = "cloud-agent")]
#[command(about = "Cloud system agent CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the agent
    Start,
    /// Stop the agent
    Stop,
    /// Show agent status
    Status,
    /// Test database connection
    TestDb,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let config = Config::from_env();

    match &cli.command {
        Commands::Start => {
            println!("Starting agent...");
            // Agent logic here
        }
        Commands::Stop => {
            println!("Stopping agent...");
        }
        Commands::Status => {
            println!("Agent is running (mock)");
        }
        Commands::TestDb => match db_connect(&config).await {
            Ok(_) => println!(
                "Successfully connected to database at {}",
                config.database_url
            ),
            Err(e) => eprintln!("Failed to connect to database: {}", e),
        },
    }
}
