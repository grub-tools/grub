mod commands;
mod config;
mod openfoodfacts;
mod server;
mod tls;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::process;

use crate::commands::{
    cmd_barcode, cmd_copy, cmd_delete, cmd_food_add, cmd_food_list, cmd_history, cmd_import_mfp,
    cmd_log, cmd_recipe_add_ingredient, cmd_recipe_create, cmd_recipe_import, cmd_recipe_list,
    cmd_recipe_remove_ingredient, cmd_recipe_set_portions, cmd_recipe_show, cmd_search,
    cmd_summary, cmd_target_clear, cmd_target_set, cmd_target_show, cmd_update, cmd_weight_delete,
    cmd_weight_history, cmd_weight_log, cmd_weight_show,
};
use crate::config::Config;
use crate::openfoodfacts::OpenFoodFactsClient;
use grub_core::db::Database;

#[derive(Parser)]
#[command(
    name = "grub",
    version,
    about = "A simple calorie tracker CLI",
    long_about = "\n\n   ██████╗ ██████╗ ██╗   ██╗██████╗
  ██╔════╝ ██╔══██╗██║   ██║██╔══██╗
  ██║  ███╗██████╔╝██║   ██║██████╔╝
  ██║   ██║██╔══██╗██║   ██║██╔══██╗
  ╚██████╔╝██║  ██║╚██████╔╝██████╔╝
   ╚═════╝ ╚═╝  ╚═╝ ╚═════╝ ╚═════╝
        know what you're eating.
"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Log a food entry by searching for it
    Log {
        /// Food name to search for
        food: String,
        /// Serving size (e.g. "200g", "500ml", "2 tbsp", "1.5 oz")
        serving: String,
        /// Meal type: breakfast, lunch, dinner, snack
        #[arg(short, long, default_value = "snack")]
        meal: String,
        /// Log directly by food ID (skip search)
        #[arg(long)]
        food_id: Option<i64>,
        /// Date to log for (YYYY-MM-DD, default: today)
        #[arg(long)]
        date: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Look up a food by barcode and log it
    Barcode {
        /// Barcode number
        code: String,
        /// Serving size (e.g. "200g", "500ml", "2 tbsp"; optional, uses default if available)
        serving: Option<String>,
        /// Meal type: breakfast, lunch, dinner, snack
        #[arg(short, long, default_value = "snack")]
        meal: String,
        /// Date to log for (YYYY-MM-DD, default: today)
        #[arg(long)]
        date: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Search `OpenFoodFacts` for a food
    Search {
        /// Search query
        query: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show daily summary (defaults to today)
    Summary {
        /// Date to show (YYYY-MM-DD, default: today)
        date: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show summary for the last N days
    History {
        /// Number of days to show
        #[arg(short, long, default_value = "7")]
        days: u32,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Delete a meal entry by ID
    Delete {
        /// Entry ID to delete
        entry_id: i64,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Update a meal entry (serving size, meal type, or date)
    Update {
        /// Entry ID to update
        entry_id: i64,
        /// New serving size (e.g. "200g", "500ml", "2 tbsp")
        #[arg(short, long)]
        serving: Option<String>,
        /// New meal type: breakfast, lunch, dinner, snack
        #[arg(long)]
        meal: Option<String>,
        /// New date (YYYY-MM-DD or today/yesterday/tomorrow)
        #[arg(long)]
        date: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Copy a meal from one date/meal to another
    Copy {
        /// Source in format "date:meal" (e.g. "today:lunch" or "2024-01-15:breakfast")
        from: String,
        /// Destination in format "date:meal"
        to: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Start the REST API server
    Serve {
        /// Port to listen on
        #[arg(short, long, default_value = "8080")]
        port: u16,
        /// Address to bind to (default: 127.0.0.1, use 0.0.0.0 to expose to network)
        #[arg(short, long, default_value = "127.0.0.1")]
        bind: String,
        /// Disable API key authentication (for development/testing)
        #[arg(long)]
        no_auth: bool,
        /// Enable TLS (HTTPS). Generates a self-signed certificate on first use.
        #[arg(long)]
        tls: bool,
        /// Path to TLS certificate file (PEM). Implies --tls.
        #[arg(long, value_name = "PATH")]
        tls_cert: Option<std::path::PathBuf>,
        /// Path to TLS private key file (PEM). Implies --tls.
        #[arg(long, value_name = "PATH")]
        tls_key: Option<std::path::PathBuf>,
    },
    /// Manage daily calorie/macro targets
    Target {
        #[command(subcommand)]
        command: TargetCommands,
    },
    /// Manage custom foods
    Food {
        #[command(subcommand)]
        command: FoodCommands,
    },
    /// Manage recipes (composite foods with portions)
    Recipe {
        #[command(subcommand)]
        command: RecipeCommands,
    },
    /// Import data from external sources
    Import {
        #[command(subcommand)]
        command: ImportCommands,
    },
    /// Track body weight
    Weight {
        #[command(subcommand)]
        command: WeightCommands,
    },
}

#[derive(Subcommand)]
enum ImportCommands {
    /// Import meals from a `MyFitnessPal` CSV export
    Mfp {
        /// Path to the MFP CSV file
        file: std::path::PathBuf,
        /// Preview import without making changes
        #[arg(long)]
        dry_run: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum TargetCommands {
    /// Set daily calorie/macro target
    Set {
        /// Daily calorie target
        calories: i64,
        /// Protein percentage (requires --carbs and --fat, must sum to 100)
        #[arg(long)]
        protein: Option<i64>,
        /// Carbs percentage (requires --protein and --fat, must sum to 100)
        #[arg(long)]
        carbs: Option<i64>,
        /// Fat percentage (requires --protein and --carbs, must sum to 100)
        #[arg(long)]
        fat: Option<i64>,
        /// Day(s) to apply to: monday-sunday, mon-sun, weekdays, weekends, all (default: all)
        #[arg(long, default_value = "all")]
        day: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show all targets
    Show {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Clear target(s)
    Clear {
        /// Day(s) to clear: monday-sunday, mon-sun, weekdays, weekends, all (default: clear all)
        #[arg(long)]
        day: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum FoodCommands {
    /// Add a custom food
    Add {
        /// Food name
        name: String,
        /// Calories per 100g
        #[arg(long)]
        calories: f64,
        /// Protein per 100g
        #[arg(long)]
        protein: Option<f64>,
        /// Carbs per 100g
        #[arg(long)]
        carbs: Option<f64>,
        /// Fat per 100g
        #[arg(long)]
        fat: Option<f64>,
        /// Default serving size in grams
        #[arg(long)]
        serving: Option<f64>,
        /// Brand name
        #[arg(long)]
        brand: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// List/search local food database
    List {
        /// Search query to filter foods
        #[arg(short, long)]
        search: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum RecipeCommands {
    /// Create a new recipe
    Create {
        /// Recipe name
        name: String,
        /// Number of portions this recipe makes
        #[arg(short, long, default_value = "1")]
        portions: f64,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Add an ingredient to a recipe
    AddIngredient {
        /// Recipe name
        recipe: String,
        /// Ingredient food name (will search local DB + `OpenFoodFacts`)
        ingredient: String,
        /// Quantity (e.g. "500g", "2 cups", "1 lb")
        quantity: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove an ingredient from a recipe
    RemoveIngredient {
        /// Recipe name
        recipe: String,
        /// Ingredient food name to remove
        ingredient: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Change the number of portions for a recipe
    SetPortions {
        /// Recipe name
        recipe: String,
        /// New number of portions
        portions: f64,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show recipe details (ingredients + per-portion nutrition)
    Show {
        /// Recipe name
        recipe: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// List all recipes
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Import a recipe from a Cooklang (.cook) file
    Import {
        /// Path to the .cook file
        file: std::path::PathBuf,
        /// Recipe name override (defaults to metadata title or filename)
        #[arg(long)]
        name: Option<String>,
        /// Portions override (defaults to metadata servings)
        #[arg(long)]
        portions: Option<f64>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum WeightCommands {
    /// Log a weight entry
    Log {
        /// Weight value (number)
        value: f64,
        /// Unit: kg or lbs (default: kg)
        #[arg(short, long, default_value = "kg")]
        unit: String,
        /// Date (YYYY-MM-DD or today/yesterday/tomorrow, default: today)
        #[arg(long)]
        date: Option<String>,
        /// Optional notes
        #[arg(long)]
        notes: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show weight for a specific date (default: today)
    Show {
        /// Date (YYYY-MM-DD or today/yesterday/tomorrow, default: today)
        date: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show weight history
    History {
        /// Number of days to show (default: all)
        #[arg(short, long)]
        days: Option<u32>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Delete a weight entry by ID
    Delete {
        /// Weight entry ID
        id: i64,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli).await {
        eprintln!("Error: {e:#}");
        process::exit(1);
    }
}

#[allow(clippy::too_many_lines)]
async fn run(cli: Cli) -> Result<()> {
    let config = Config::load()?;
    let db = Database::open(&config.db_path)?;
    let off = OpenFoodFactsClient::new();

    match cli.command {
        Commands::Search { query, json } => cmd_search(&db, &off, &query, json).await,
        Commands::Log {
            food,
            serving,
            meal,
            food_id,
            date,
            json,
        } => cmd_log(&db, &off, &food, &serving, &meal, food_id, date, json).await,
        Commands::Barcode {
            code,
            serving,
            meal,
            date,
            json,
        } => cmd_barcode(&db, &off, &code, serving, &meal, date, json).await,
        Commands::Summary { date, json } => cmd_summary(&db, date, json),
        Commands::History { days, json } => cmd_history(&db, days, json),
        Commands::Delete { entry_id, json } => cmd_delete(&db, entry_id, json),
        Commands::Update {
            entry_id,
            serving,
            meal,
            date,
            json,
        } => cmd_update(&db, entry_id, serving.as_ref(), meal.as_ref(), date, json),
        Commands::Copy { from, to, json } => cmd_copy(&db, &from, &to, json),
        Commands::Serve {
            port,
            bind,
            no_auth,
            tls,
            tls_cert,
            tls_key,
        } => {
            let (api_key, new_api_key) = if no_auth {
                (None, false)
            } else {
                let (key, new) = config.load_or_create_api_key()?;
                (Some(key), new)
            };
            let tls_config = if tls || tls_cert.is_some() || tls_key.is_some() {
                let cert_path = tls_cert.map_or_else(tls::default_cert_path, Ok)?;
                let key_path = tls_key.map_or_else(tls::default_key_path, Ok)?;
                Some(server::TlsConfig {
                    cert_path,
                    key_path,
                })
            } else {
                None
            };
            server::start_server(db, port, &bind, api_key, tls_config, new_api_key).await
        }
        Commands::Target { command } => match command {
            TargetCommands::Set {
                calories,
                protein,
                carbs,
                fat,
                day,
                json,
            } => cmd_target_set(&db, calories, protein, carbs, fat, &day, json),
            TargetCommands::Show { json } => cmd_target_show(&db, json),
            TargetCommands::Clear { day, json } => cmd_target_clear(&db, day.as_deref(), json),
        },
        Commands::Food { command } => match command {
            FoodCommands::Add {
                name,
                calories,
                protein,
                carbs,
                fat,
                serving,
                brand,
                json,
            } => cmd_food_add(
                &db, &name, calories, protein, carbs, fat, serving, brand, json,
            ),
            FoodCommands::List { search, json } => cmd_food_list(&db, search.as_deref(), json),
        },
        Commands::Recipe { command } => match command {
            RecipeCommands::Create {
                name,
                portions,
                json,
            } => cmd_recipe_create(&db, &name, portions, json),
            RecipeCommands::AddIngredient {
                recipe,
                ingredient,
                quantity,
                json,
            } => cmd_recipe_add_ingredient(&db, &off, &recipe, &ingredient, &quantity, json).await,
            RecipeCommands::RemoveIngredient {
                recipe,
                ingredient,
                json,
            } => cmd_recipe_remove_ingredient(&db, &recipe, &ingredient, json),
            RecipeCommands::SetPortions {
                recipe,
                portions,
                json,
            } => cmd_recipe_set_portions(&db, &recipe, portions, json),
            RecipeCommands::Show { recipe, json } => cmd_recipe_show(&db, &recipe, json),
            RecipeCommands::List { json } => cmd_recipe_list(&db, json),
            RecipeCommands::Import {
                file,
                name,
                portions,
                json,
            } => cmd_recipe_import(&db, &off, &file, name, portions, json).await,
        },
        Commands::Import { command } => match command {
            ImportCommands::Mfp {
                file,
                dry_run,
                json,
            } => cmd_import_mfp(&db, &file, dry_run, json),
        },
        Commands::Weight { command } => match command {
            WeightCommands::Log {
                value,
                unit,
                date,
                notes,
                json,
            } => cmd_weight_log(&db, value, &unit, date, notes, json),
            WeightCommands::Show { date, json } => cmd_weight_show(&db, date, json),
            WeightCommands::History { days, json } => cmd_weight_history(&db, days, json),
            WeightCommands::Delete { id, json } => cmd_weight_delete(&db, id, json),
        },
    }
}
