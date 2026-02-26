use std::path::Path;

use anyhow::{Context, Result};

use grub_core::db::Database;
use grub_core::mfp_import::{import_mfp_meals, parse_mfp_csv};

pub fn cmd_import_mfp(db: &Database, path: &Path, dry_run: bool, json: bool) -> Result<()> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open file: {}", path.display()))?;

    let rows = parse_mfp_csv(file)?;

    if rows.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::json!({ "error": "No rows found in CSV file" })
            );
        } else {
            eprintln!("No rows found in CSV file.");
        }
        return Ok(());
    }

    let summary = import_mfp_meals(db, &rows, dry_run)?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "dry_run": dry_run,
                "rows_parsed": summary.rows_parsed,
                "foods_created": summary.foods_created,
                "foods_reused": summary.foods_reused,
                "meals_logged": summary.meals_logged,
                "dates_spanned": summary.dates_spanned,
            })
        );
    } else if dry_run {
        println!("Dry run — no changes made.\n");
        println!("  Rows parsed:   {}", summary.rows_parsed);
        println!("  Foods to create: {}", summary.foods_created);
        println!("  Foods reused:  {}", summary.foods_reused);
        println!("  Meals to log:  {}", summary.meals_logged);
        println!("  Dates spanned: {}", summary.dates_spanned);
    } else {
        println!("Import complete.\n");
        println!("  Rows parsed:   {}", summary.rows_parsed);
        println!("  Foods created: {}", summary.foods_created);
        println!("  Foods reused:  {}", summary.foods_reused);
        println!("  Meals logged:  {}", summary.meals_logged);
        println!("  Dates spanned: {}", summary.dates_spanned);
    }

    Ok(())
}
