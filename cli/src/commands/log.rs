use anyhow::{Context, Result};
use std::process;

use crate::openfoodfacts::OpenFoodFactsClient;
use grub_core::db::Database;
use grub_core::models::{MealEntry, NewMealEntry, validate_meal_type};

pub(crate) fn format_serving_display(entry: &MealEntry) -> String {
    match (&entry.display_unit, entry.display_quantity) {
        (Some(unit), Some(qty)) => {
            if qty.fract() == 0.0 {
                format!("{qty:.0}{unit}")
            } else {
                format!("{qty}{unit}")
            }
        }
        _ => format!("{:.0}g", entry.serving_g),
    }
}

use super::helpers::{
    json_error, parse_date, parse_serving_with_unit, print_food_table, prompt_choice,
};
use super::search_and_cache;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn cmd_log(
    db: &Database,
    off: &OpenFoodFactsClient,
    food_query: &str,
    serving_str: &str,
    meal: &str,
    food_id: Option<i64>,
    date: Option<String>,
    json: bool,
) -> Result<()> {
    let meal_type = validate_meal_type(meal)?;
    let (serving_g, display_unit, display_quantity) = parse_serving_with_unit(serving_str)?;
    let date = parse_date(date)?;

    let food = if let Some(id) = food_id {
        db.get_food_by_id(id)?
    } else {
        let all = search_and_cache(db, off, food_query).await?;

        if all.is_empty() {
            if json {
                println!(
                    "{}",
                    json_error(&format!("No food found for '{food_query}'"))
                );
            } else {
                eprintln!("No food found for '{food_query}'");
            }
            process::exit(2);
        }

        if all.len() == 1 {
            all.into_iter().next().unwrap()
        } else {
            let refs: Vec<&_> = all.iter().collect();
            print_food_table(&refs);
            let idx = prompt_choice(all.len())?;
            all.into_iter().nth(idx).unwrap()
        }
    };

    let entry = db.insert_meal_entry(&NewMealEntry {
        date,
        meal_type,
        food_id: food.id,
        serving_g,
        display_unit,
        display_quantity,
    })?;

    if json {
        println!("{}", serde_json::to_string_pretty(&entry)?);
    } else {
        let name = &food.name;
        let meal_type = &entry.meal_type;
        let cal = entry.calories.unwrap_or(0.0);
        let serving_display = format_serving_display(&entry);
        println!("Logged: {name} {serving_display} for {meal_type} — {cal:.0} kcal");
    }

    Ok(())
}

pub(crate) async fn cmd_barcode(
    db: &Database,
    off: &OpenFoodFactsClient,
    code: &str,
    serving: Option<String>,
    meal: &str,
    date: Option<String>,
    json: bool,
) -> Result<()> {
    let meal_type = validate_meal_type(meal)?;
    let date = parse_date(date)?;

    // Check local cache first
    let food = if let Some(cached) = db.get_food_by_barcode(code)? {
        cached
    } else {
        // Look up remotely
        let remote = off
            .lookup_barcode_async(code)
            .await?
            .with_context(|| format!("No product found for barcode '{code}'"))?;
        db.upsert_food_by_barcode(&remote)?
    };

    let (serving_g, display_unit, display_quantity) = match serving {
        Some(s) => parse_serving_with_unit(&s)?,
        None => (food.default_serving_g.unwrap_or(100.0), None, None),
    };

    let entry = db.insert_meal_entry(&NewMealEntry {
        date,
        meal_type,
        food_id: food.id,
        serving_g,
        display_unit,
        display_quantity,
    })?;

    if json {
        println!("{}", serde_json::to_string_pretty(&entry)?);
    } else {
        let name = &food.name;
        let display_name = match &food.brand {
            Some(b) => format!("{name} ({b})"),
            None => food.name.clone(),
        };
        let meal_type = &entry.meal_type;
        let cal = entry.calories.unwrap_or(0.0);
        let serving_display = format_serving_display(&entry);
        println!("Logged: {display_name} {serving_display} for {meal_type} — {cal:.0} kcal");
    }

    Ok(())
}
