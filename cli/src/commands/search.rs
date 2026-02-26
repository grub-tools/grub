use anyhow::Result;
use std::process;

use crate::openfoodfacts::OpenFoodFactsClient;
use grub_core::db::Database;
use grub_core::models::{Food, NewFood};

use super::helpers::print_food_table;
use super::search_and_cache;

pub(crate) async fn cmd_search(
    db: &Database,
    off: &OpenFoodFactsClient,
    query: &str,
    json: bool,
) -> Result<()> {
    let all = search_and_cache(db, off, query).await?;

    if all.is_empty() {
        if json {
            println!("[]");
        } else {
            eprintln!("No results found for '{query}'");
        }
        process::exit(2);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&all)?);
    } else {
        let refs: Vec<&Food> = all.iter().collect();
        print_food_table(&refs);
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_food_add(
    db: &Database,
    name: &str,
    calories: f64,
    protein: Option<f64>,
    carbs: Option<f64>,
    fat: Option<f64>,
    serving: Option<f64>,
    brand: Option<String>,
    json: bool,
) -> Result<()> {
    let food = db.insert_food(&NewFood {
        name: name.to_string(),
        brand,
        barcode: None,
        calories_per_100g: calories,
        protein_per_100g: protein,
        carbs_per_100g: carbs,
        fat_per_100g: fat,
        default_serving_g: serving,
        source: "manual".to_string(),
    })?;

    if json {
        println!("{}", serde_json::to_string_pretty(&food)?);
    } else {
        let name = &food.name;
        let id = food.id;
        println!("Added food: {name} (id: {id})");
    }

    Ok(())
}

pub(crate) fn cmd_food_list(db: &Database, search: Option<&str>, json: bool) -> Result<()> {
    let foods = db.list_foods(search)?;

    if foods.is_empty() {
        if json {
            println!("[]");
        } else {
            eprintln!("No foods found");
        }
        process::exit(2);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&foods)?);
    } else {
        let refs: Vec<&Food> = foods.iter().collect();
        print_food_table(&refs);
    }

    Ok(())
}
