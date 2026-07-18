use byteburrow::migration::Migrator;
use byteburrow::{
    auth::Auth,
    config::Config,
    db_connect,
    entity::{contact, face_reference, group, group_user, user},
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
    /// Test face matching: compare a contact's confirmed faces against all others
    FaceMatch {
        /// Contact ID whose confirmed faces to use as source
        contact_id: i32,
        /// Similarity threshold (0.0-1.0, default 0.5)
        #[arg(short, long, default_value_t = 0.5)]
        threshold: f32,
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
        } => {
            if let Err(e) = face_match(&config, *contact_id, *threshold).await {
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
    let hashed = Auth::hash_string("admin");
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
            let hashed_password = Auth::hash_string(&args.password);

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
        "\n{:<5} {:<66} {:<6} {:<10} {:<20} {:<9} Bbox",
        "ID", "Hash", "Face#", "Confirmed", "Contact", "Embed"
    );
    println!("{}", "-".repeat(130));

    for r in &refs {
        let hash_hex = hex::encode(&r.hash);
        let contact_name = r
            .contact_id
            .and_then(|id| contact_map.get(&id))
            .map(|n| n.as_str())
            .unwrap_or("-");
        let embed_dim = r.embedding.len() / 4;

        println!(
            "{:<5} {:<66} {:<6} {:<10} {:<20} {:<9} {}x{}+{}+{}",
            r.id,
            hash_hex,
            r.face_index,
            if r.confirmed { "YES" } else { "no" },
            contact_name,
            format!("{}d", embed_dim),
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
    threshold: f32,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = db_connect(config).await?;

    // Look up contact name
    let contact_record = contact::Entity::find_by_id(contact_id)
        .one(&db)
        .await?
        .ok_or_else(|| format!("Contact with id={contact_id} not found"))?;

    // Get confirmed face references for this contact
    let source_refs = face_reference::Entity::find()
        .filter(face_reference::Column::ContactId.eq(contact_id))
        .filter(face_reference::Column::Confirmed.eq(true))
        .all(&db)
        .await?;

    if source_refs.is_empty() {
        return Err(format!(
            "No confirmed face references for contact '{}' (id={})",
            contact_record.name, contact_id
        )
        .into());
    }

    let source_embeddings: Vec<Vec<f32>> = source_refs
        .iter()
        .map(|r| bytes_to_floats(&r.embedding))
        .collect();

    println!(
        "Contact: '{}' (id={}) — {} confirmed reference(s)",
        contact_record.name,
        contact_id,
        source_refs.len()
    );
    println!("Threshold: {:.2}\n", threshold);

    // Load all other face references
    let all_refs = face_reference::Entity::find().all(&db).await?;
    let source_ids: std::collections::HashSet<i32> = source_refs.iter().map(|r| r.id).collect();

    let contacts = contact::Entity::find().all(&db).await?;
    let contact_map: std::collections::HashMap<i32, String> =
        contacts.into_iter().map(|c| (c.id, c.name)).collect();

    let mut matches: Vec<(f32, &face_reference::Model)> = Vec::new();

    for r in &all_refs {
        // Skip source faces
        if source_ids.contains(&r.id) {
            continue;
        }
        // Skip faces already assigned to the same contact
        if r.contact_id == Some(contact_id) {
            continue;
        }
        // Skip faces already confirmed for another contact
        if r.confirmed && r.contact_id.is_some() {
            continue;
        }

        let emb = bytes_to_floats(&r.embedding);

        // Best similarity across all source embeddings for this contact
        let best_sim = source_embeddings
            .iter()
            .map(|src| cosine_similarity(src, &emb))
            .fold(0.0f32, f32::max);

        if best_sim >= threshold {
            matches.push((best_sim, r));
        }
    }

    matches.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    if matches.is_empty() {
        println!("No matches found above threshold {:.2}", threshold);
    } else {
        println!(
            "{:<10} {:<5} {:<66} {:<6} {:<10} {:<20}",
            "Similarity", "ID", "Hash", "Face#", "Confirmed", "Contact"
        );
        println!("{}", "-".repeat(120));

        let mut saved = 0;
        for (sim, r) in &matches {
            let hash_hex = hex::encode(&r.hash);
            let contact_name = r
                .contact_id
                .and_then(|id| contact_map.get(&id))
                .map(|n| n.as_str())
                .unwrap_or("-");

            println!(
                "{:<10.4} {:<5} {:<66} {:<6} {:<10} {:<20}",
                sim,
                r.id,
                hash_hex,
                r.face_index,
                if r.confirmed { "YES" } else { "no" },
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

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a > 0.0 && norm_b > 0.0 {
        dot / (norm_a * norm_b)
    } else {
        0.0
    }
}

fn bytes_to_floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}
