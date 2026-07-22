use byteburrow::migration::Migrator;
use byteburrow::{
    auth::Auth,
    config::Config,
    db_connect,
    entity::{contact, face_reference, group, group_user, user},
    face_match::{bytes_to_floats, match_embedding, Exemplar, MatchParams},
};
use clap::{Args, Parser, Subcommand};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, NotSet, QueryFilter, Set};
use sea_orm_migration::prelude::*;

#[derive(Parser)]
#[command(name = "byteburrow-cli")]
#[command(about = "ByteBurrow CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Test database connection
    TestDb,
    /// User management commands
    User {
        #[command(subcommand)]
        command: UserCommands,
    },
    /// Load development fixtures (admin user + admin group, no conflict checks)
    Fixtures,
    /// List all face references in the database
    FaceList,
    /// Test face matching: find unconfirmed faces whose best confirmed match is
    /// this contact, using the shared host-side matcher (`face_match`).
    FaceMatch {
        /// Contact ID to test candidate faces against
        contact_id: i32,
        /// Similarity threshold (0.0-1.0). Defaults to the configured
        /// `face_match_threshold`.
        #[arg(short, long)]
        threshold: Option<f32>,
        /// Ambiguity margin: minimum gap to the best *other* contact. Defaults
        /// to the configured `face_match_margin`.
        #[arg(short, long)]
        margin: Option<f32>,
    },
}

#[derive(Subcommand)]
enum UserCommands {
    /// Add a new user
    Add(AddUserArgs),
    /// List all users
    List,
    /// Delete a user
    Delete {
        /// Username to delete
        username: String,
    },
    /// Enable/disable a user
    Toggle {
        /// Username to enable/disable
        username: String,
    },
}

#[derive(Args)]
struct AddUserArgs {
    /// Username for the new user
    #[arg(short, long)]
    username: String,

    /// Full name of the user
    #[arg(short, long)]
    name: String,

    /// Password for the user
    #[arg(short, long)]
    password: String,

    /// User description (optional)
    #[arg(short, long)]
    description: Option<String>,

    /// Make the user an admin
    #[arg(short, long, default_value_t = false)]
    admin: bool,

    /// Enable the user (default: true)
    #[arg(short, long, default_value_t = true)]
    enabled: bool,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let config = Config::from_env();
    Config::set(std::sync::Arc::new(config.clone()));

    match &cli.command {
        Commands::TestDb => match db_connect(&config).await {
            Ok(_) => println!(
                "Successfully connected to database at {}",
                config.database_url
            ),
            Err(e) => eprintln!("Failed to connect to database: {}", e),
        },
        Commands::User { command } => {
            if let Err(e) = handle_user_command(command, &config).await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Fixtures => {
            if let Err(e) = load_fixtures(&config).await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::FaceList => {
            if let Err(e) = face_list(&config).await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::FaceMatch {
            contact_id,
            threshold,
            margin,
        } => {
            let params = MatchParams {
                threshold: threshold.unwrap_or(config.face_match_threshold),
                margin: margin.unwrap_or(config.face_match_margin),
            };
            if let Err(e) = face_match(&config, *contact_id, params).await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    }
}

async fn load_fixtures(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let db = db_connect(config).await?;
    Migrator::fresh(&db).await?;

    // Insert admin group (ignore conflicts)
    let admin_group = group::ActiveModel {
        id: NotSet,
        name: Set("admin".to_string()),
        description: Set(Some("Administrators".to_string())),
    };
    let group_result = admin_group.insert(&db).await?;
    println!(
        "✓ Group created: {} (id={})",
        group_result.name, group_result.id
    );

    // Insert admin user (ignore conflicts)
    let hashed = Auth::hash_password("admin").map_err(|_| "Failed to hash password".to_string())?;
    let admin_user = user::ActiveModel {
        id: NotSet,
        username: Set("admin".to_string()),
        name: Set("Administrator".to_string()),
        password: Set(hashed),
        description: Set(None),
        admin: Set(true),
        enabled: Set(true),
    };
    let user_result = admin_user.insert(&db).await?;
    println!(
        "✓ User created:  {} (id={})",
        user_result.username, user_result.id
    );

    // Add admin user to admin group
    let membership = group_user::ActiveModel {
        id: NotSet,
        group_id: Set(group_result.id),
        user_id: Set(user_result.id),
        admin: Set(true),
    };
    membership.insert(&db).await?;
    println!(
        "✓ User '{}' added to group '{}'",
        user_result.username, group_result.name
    );

    Ok(())
}

async fn handle_user_command(
    command: &UserCommands,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = db_connect(config).await?;

    match command {
        UserCommands::Add(args) => {
            // Check if user already exists
            let existing = user::Entity::find()
                .filter(user::Column::Username.eq(&args.username))
                .one(&db)
                .await?;

            if existing.is_some() {
                return Err(format!("User '{}' already exists", args.username).into());
            }

            // Hash the password
            let hashed_password = Auth::hash_password(&args.password)
                .map_err(|_| "Failed to hash password".to_string())?;

            // Create new user
            let new_user = user::ActiveModel {
                id: NotSet, // Will be auto-generated by database
                username: Set(args.username.clone()),
                name: Set(args.name.clone()),
                password: Set(hashed_password),
                description: Set(args.description.clone()),
                admin: Set(args.admin),
                enabled: Set(args.enabled),
            };

            let result = new_user.insert(&db).await?;

            println!("✓ User created successfully!");
            println!("  ID:       {}", result.id);
            println!("  Username: {}", result.username);
            println!("  Name:     {}", result.name);
            println!("  Admin:    {}", result.admin);
            println!("  Enabled:  {}", result.enabled);
        }

        UserCommands::List => {
            let users = user::Entity::find().all(&db).await?;

            if users.is_empty() {
                println!("No users found.");
                return Ok(());
            }

            println!(
                "\n{:<6} {:<20} {:<30} {:<8} {:<8}",
                "ID", "Username", "Name", "Admin", "Enabled"
            );
            println!("{}", "-".repeat(80));

            for user in users {
                println!(
                    "{:<6} {:<20} {:<30} {:<8} {:<8}",
                    user.id,
                    user.username,
                    user.name,
                    if user.admin { "Yes" } else { "No" },
                    if user.enabled { "Yes" } else { "No" }
                );
            }
            println!();
        }

        UserCommands::Delete { username } => {
            let user_record = user::Entity::find()
                .filter(user::Column::Username.eq(username))
                .one(&db)
                .await?;

            match user_record {
                Some(user) => {
                    user::Entity::delete_by_id(user.id).exec(&db).await?;
                    println!("✓ User '{}' deleted successfully!", username);
                }
                None => {
                    return Err(format!("User '{}' not found", username).into());
                }
            }
        }

        UserCommands::Toggle { username } => {
            let user_record = user::Entity::find()
                .filter(user::Column::Username.eq(username))
                .one(&db)
                .await?;

            match user_record {
                Some(user) => {
                    let new_enabled = !user.enabled;
                    let mut active_user: user::ActiveModel = user.into();
                    active_user.enabled = Set(new_enabled);
                    active_user.update(&db).await?;

                    println!(
                        "✓ User '{}' {} successfully!",
                        username,
                        if new_enabled { "enabled" } else { "disabled" }
                    );
                }
                None => {
                    return Err(format!("User '{}' not found", username).into());
                }
            }
        }
    }

    Ok(())
}

async fn face_list(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let db = db_connect(config).await?;

    let refs = face_reference::Entity::find().all(&db).await?;

    if refs.is_empty() {
        println!("No face references found.");
        return Ok(());
    }

    let contacts = contact::Entity::find().all(&db).await?;
    let contact_map: std::collections::HashMap<i32, String> =
        contacts.into_iter().map(|c| (c.id, c.name)).collect();

    println!(
        "\n{:<5} {:<66} {:<6} {:<10} {:<20} {:<30} {:<9} Bbox",
        "ID", "Hash", "Face#", "Confirmed", "Contact", "Model", "Embed"
    );
    println!("{}", "-".repeat(160));

    for r in &refs {
        let hash_hex = hex::encode(&r.hash);
        let contact_name = r
            .contact_id
            .and_then(|id| contact_map.get(&id))
            .map(|n| n.as_str())
            .unwrap_or("-");

        println!(
            "{:<5} {:<66} {:<6} {:<10} {:<20} {:<30} {:<9} {}x{}+{}+{}",
            r.id,
            hash_hex,
            r.face_index,
            if r.confirmed { "YES" } else { "no" },
            contact_name,
            format!("{}@{}", r.model_id, r.model_version),
            format!("{}d", r.dim),
            r.bbox_w,
            r.bbox_h,
            r.bbox_x,
            r.bbox_y,
        );
    }
    println!("\nTotal: {} face references", refs.len());

    Ok(())
}

async fn face_match(
    config: &Config,
    contact_id: i32,
    params: MatchParams,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = db_connect(config).await?;

    // Look up contact name
    let contact_record = contact::Entity::find_by_id(contact_id)
        .one(&db)
        .await?
        .ok_or_else(|| format!("Contact with id={contact_id} not found"))?;

    // All confirmed references (any contact) are the exemplar set the shared
    // matcher ranks each candidate against — this is what makes the CLI agree
    // with the job pipeline. Decode embeddings once for reuse.
    let all_refs = face_reference::Entity::find().all(&db).await?;
    let confirmed_decoded: Vec<(i32, String, String, Vec<f32>)> = all_refs
        .iter()
        .filter(|r| r.confirmed)
        .filter_map(|r| {
            r.contact_id.map(|cid| {
                (
                    cid,
                    r.model_id.clone(),
                    r.model_version.clone(),
                    bytes_to_floats(&r.embedding),
                )
            })
        })
        .collect();

    if !confirmed_decoded.iter().any(|(cid, ..)| *cid == contact_id) {
        return Err(format!(
            "No confirmed face references for contact '{}' (id={})",
            contact_record.name, contact_id
        )
        .into());
    }

    let exemplars: Vec<Exemplar> = confirmed_decoded
        .iter()
        .map(|(cid, model_id, model_version, embedding)| Exemplar {
            contact_id: *cid,
            model_id,
            model_version,
            embedding,
        })
        .collect();

    println!(
        "Contact: '{}' (id={}) — {} confirmed exemplar(s) across all contacts",
        contact_record.name,
        contact_id,
        exemplars.len()
    );
    println!(
        "Threshold: {:.2}   Margin: {:.2}\n",
        params.threshold, params.margin
    );

    let contacts = contact::Entity::find().all(&db).await?;
    let contact_map: std::collections::HashMap<i32, String> =
        contacts.into_iter().map(|c| (c.id, c.name)).collect();

    // Candidates: unconfirmed faces whose best confirmed match is this contact.
    let mut matches: Vec<(f32, Option<f32>, &face_reference::Model)> = Vec::new();

    for r in &all_refs {
        if r.confirmed {
            continue;
        }
        let emb = bytes_to_floats(&r.embedding);
        let outcome = match_embedding(&emb, &r.model_id, &r.model_version, &exemplars, params);
        if let Some(m) = outcome.best {
            if m.contact_id == contact_id {
                matches.push((m.similarity, m.runner_up, r));
            }
        }
    }

    matches.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    if matches.is_empty() {
        println!(
            "No unconfirmed faces matched contact '{}' at threshold {:.2} / margin {:.2}",
            contact_record.name, params.threshold, params.margin
        );
    } else {
        println!(
            "{:<10} {:<10} {:<5} {:<66} {:<6} {:<20}",
            "Similarity", "RunnerUp", "ID", "Hash", "Face#", "Contact"
        );
        println!("{}", "-".repeat(120));

        let mut saved = 0;
        for (sim, runner_up, r) in &matches {
            let hash_hex = hex::encode(&r.hash);
            let contact_name = r
                .contact_id
                .and_then(|id| contact_map.get(&id))
                .map(|n| n.as_str())
                .unwrap_or("-");

            println!(
                "{:<10.4} {:<10} {:<5} {:<66} {:<6} {:<20}",
                sim,
                runner_up
                    .map(|s| format!("{s:.4}"))
                    .unwrap_or_else(|| "-".to_string()),
                r.id,
                hash_hex,
                r.face_index,
                contact_name,
            );

            // Assign contact (unconfirmed) to matched face references
            let model: face_reference::Model = (*r).clone();
            let mut active: face_reference::ActiveModel = model.into();
            active.contact_id = Set(Some(contact_id));
            active.update(&db).await?;
            saved += 1;
        }
        println!(
            "\n{} matches found, {} saved (unconfirmed)",
            matches.len(),
            saved
        );
    }

    Ok(())
}
