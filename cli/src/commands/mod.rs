mod helpers;
mod import;
mod log;
mod meal;
mod recipe;
mod search;
mod summary;
mod target;
mod weight;

use anyhow::{Result, bail};

use crate::openfoodfacts::OpenFoodFactsClient;
use grub_core::db::Database;
use grub_core::models::Food;

use helpers::{print_food_table, prompt_choice};

pub(crate) use import::cmd_import_mfp;
pub(crate) use log::{cmd_barcode, cmd_log};
pub(crate) use meal::{cmd_copy, cmd_delete, cmd_update};
pub(crate) use recipe::{
    cmd_recipe_add_ingredient, cmd_recipe_create, cmd_recipe_import, cmd_recipe_list,
    cmd_recipe_remove_ingredient, cmd_recipe_set_portions, cmd_recipe_show,
};
pub(crate) use search::{cmd_food_add, cmd_food_list, cmd_search};
pub(crate) use summary::{cmd_history, cmd_summary};
pub(crate) use target::{cmd_target_clear, cmd_target_set, cmd_target_show};
pub(crate) use weight::{cmd_weight_delete, cmd_weight_history, cmd_weight_log, cmd_weight_show};

/// Search local DB and `OpenFoodFacts`, cache remote results, dedup by ID.
pub(super) async fn search_and_cache(
    db: &Database,
    off: &OpenFoodFactsClient,
    query: &str,
) -> Result<Vec<Food>> {
    let local = db.search_foods_local(query)?;
    let remote = off.search_async(query).await?;

    let mut cached_remote: Vec<Food> = Vec::new();
    for food in &remote {
        if let Ok(f) = db.upsert_food_by_barcode(food) {
            cached_remote.push(f);
        } else {
            let mut no_barcode = food.clone();
            no_barcode.barcode = None;
            if let Ok(f) = db.insert_food(&no_barcode) {
                cached_remote.push(f);
            }
        }
    }

    let mut all = local;
    let seen: std::collections::HashSet<i64> = all.iter().map(|f| f.id).collect();
    for f in cached_remote {
        if !seen.contains(&f.id) {
            all.push(f);
        }
    }

    Ok(all)
}

/// Resolve a food name to a Food record, searching local DB first then `OpenFoodFacts`.
pub(super) async fn resolve_food(
    db: &Database,
    off: &OpenFoodFactsClient,
    food_query: &str,
) -> Result<Food> {
    let all = search_and_cache(db, off, food_query).await?;

    if all.is_empty() {
        bail!("No food found for '{food_query}'");
    }

    if all.len() == 1 {
        return Ok(all.into_iter().next().unwrap());
    }

    let refs: Vec<&Food> = all.iter().collect();
    print_food_table(&refs);
    let idx = prompt_choice(all.len())?;
    Ok(all.into_iter().nth(idx).unwrap())
}
