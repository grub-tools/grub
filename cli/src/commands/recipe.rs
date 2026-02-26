use anyhow::{Context, Result, bail};
use std::process;
use tabled::{
    Table, Tabled,
    settings::{Alignment, Modify, Style, object::Columns},
};

use crate::openfoodfacts::OpenFoodFactsClient;
use grub_core::db::Database;
use grub_core::models::{CooklangIngredient, convert_to_grams};

use super::helpers::{json_error, parse_ingredient_quantity, truncate};
use super::resolve_food;

pub(crate) fn cmd_recipe_create(
    db: &Database,
    name: &str,
    portions: f64,
    json: bool,
) -> Result<()> {
    if portions <= 0.0 {
        bail!("Portions must be greater than 0");
    }
    let recipe = db.create_recipe(name, portions)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&recipe)?);
    } else {
        let id = recipe.id;
        println!("Created recipe: {name} (id: {id}, portions: {portions})");
        println!("Add ingredients with: grub recipe add-ingredient \"{name}\" <food> <quantity>");
    }
    Ok(())
}

pub(crate) async fn cmd_recipe_add_ingredient(
    db: &Database,
    off: &OpenFoodFactsClient,
    recipe_name: &str,
    ingredient_name: &str,
    quantity_str: &str,
    json: bool,
) -> Result<()> {
    let recipe = db.get_recipe_by_food_name(recipe_name)?;
    let quantity_g = parse_ingredient_quantity(quantity_str)?;

    // Resolve ingredient to a food record
    let food = resolve_food(db, off, ingredient_name).await?;

    let ingredient = db.add_recipe_ingredient(recipe.id, food.id, quantity_g)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&ingredient)?);
    } else {
        let food_name = &food.name;
        println!("Added {quantity_g}g of {food_name} to {recipe_name}");
    }

    Ok(())
}

pub(crate) fn cmd_recipe_remove_ingredient(
    db: &Database,
    recipe_name: &str,
    ingredient_name: &str,
    json: bool,
) -> Result<()> {
    let recipe = db.get_recipe_by_food_name(recipe_name)?;
    if db.remove_recipe_ingredient(recipe.id, ingredient_name)? {
        if json {
            println!("{}", serde_json::json!({ "removed": ingredient_name }));
        } else {
            println!("Removed {ingredient_name} from {recipe_name}");
        }
    } else {
        if json {
            println!(
                "{}",
                json_error(&format!(
                    "Ingredient '{ingredient_name}' not found in recipe"
                ))
            );
        } else {
            eprintln!("Ingredient '{ingredient_name}' not found in recipe");
        }
        process::exit(2);
    }
    Ok(())
}

pub(crate) fn cmd_recipe_set_portions(
    db: &Database,
    recipe_name: &str,
    portions: f64,
    json: bool,
) -> Result<()> {
    if portions <= 0.0 {
        bail!("Portions must be greater than 0");
    }
    let recipe = db.get_recipe_by_food_name(recipe_name)?;
    db.set_recipe_portions(recipe.id, portions)?;
    if json {
        let detail = db.get_recipe_detail(recipe.id)?;
        println!("{}", serde_json::to_string_pretty(&detail)?);
    } else {
        println!("Updated {recipe_name} to {portions} portions");
    }
    Ok(())
}

pub(crate) fn cmd_recipe_show(db: &Database, recipe_name: &str, json: bool) -> Result<()> {
    let recipe = db.get_recipe_by_food_name(recipe_name)?;
    let detail = db.get_recipe_detail(recipe.id)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&detail)?);
        return Ok(());
    }

    let name = &detail.name;
    let portions = detail.portions;
    let total_w = detail.total_weight_g;
    let portion_w = detail.per_portion_g;
    println!("=== {name} ===");
    println!("  Portions: {portions}  |  Total: {total_w:.0}g  |  Per portion: {portion_w:.0}g\n");

    println!("  INGREDIENTS:");
    for ing in &detail.ingredients {
        let fname = ing.food_name.as_deref().unwrap_or("?");
        let qty = ing.quantity_g;
        let cal = ing.calories.unwrap_or(0.0);
        println!("    {fname} — {qty:.0}g — {cal:.0} kcal");
    }

    let pp_cal = detail.per_portion_calories;
    let pp_pro = detail.per_portion_protein;
    let pp_carb = detail.per_portion_carbs;
    let pp_fat = detail.per_portion_fat;
    println!("\n  PER PORTION:");
    println!("    {pp_cal:.0} kcal | P:{pp_pro:.0}g C:{pp_carb:.0}g F:{pp_fat:.0}g");

    Ok(())
}

pub(crate) fn cmd_recipe_list(db: &Database, json: bool) -> Result<()> {
    #[derive(Tabled)]
    struct RecipeRow {
        #[tabled(rename = "ID")]
        id: i64,
        #[tabled(rename = "Name")]
        name: String,
        #[tabled(rename = "Portions")]
        portions: String,
        #[tabled(rename = "Per portion")]
        per_portion: String,
        #[tabled(rename = "Cal/portion")]
        cal_per_portion: String,
    }

    let recipes = db.list_recipes()?;
    if recipes.is_empty() {
        if json {
            println!("[]");
        } else {
            eprintln!("No recipes found");
        }
        process::exit(2);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&recipes)?);
        return Ok(());
    }

    let rows: Vec<RecipeRow> = recipes
        .iter()
        .map(|r| RecipeRow {
            id: r.id,
            name: truncate(&r.name, 30),
            portions: format!("{:.0}", r.portions),
            per_portion: format!("{:.0}g", r.per_portion_g),
            cal_per_portion: format!("{:.0}", r.per_portion_calories),
        })
        .collect();

    let table = Table::new(&rows)
        .with(Style::rounded())
        .with(Modify::new(Columns::new(2..)).with(Alignment::right()))
        .to_string();
    println!("{table}");

    Ok(())
}

pub(crate) async fn cmd_recipe_import(
    db: &Database,
    off: &OpenFoodFactsClient,
    file: &std::path::Path,
    name_override: Option<String>,
    portions_override: Option<f64>,
    json: bool,
) -> Result<()> {
    let input = std::fs::read_to_string(file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let (recipe_data, _report) = cooklang::parse(&input)
        .into_result()
        .map_err(|e| anyhow::anyhow!("Failed to parse Cooklang file: {e}"))?;

    let name = name_override
        .or_else(|| recipe_data.metadata.title().map(String::from))
        .or_else(|| file.file_stem().and_then(|s| s.to_str()).map(String::from))
        .context("Could not determine recipe name. Use --name to specify one")?;

    let portions = portions_override
        .or_else(|| {
            recipe_data
                .metadata
                .servings()
                .and_then(|s| s.as_number().map(f64::from))
        })
        .unwrap_or(1.0);

    let converter = cooklang::Converter::default();
    let grouped = recipe_data.group_ingredients(&converter);

    let ingredients: Vec<CooklangIngredient> = grouped
        .iter()
        .map(|gi| cooklang_ingredient_to_grub(gi))
        .collect();

    if ingredients.is_empty() {
        bail!("No ingredients found in recipe");
    }

    let recipe = db.create_recipe(&name, portions)?;
    let warnings = import_ingredients(db, off, recipe.id, &ingredients).await?;

    if !warnings.is_empty() {
        eprintln!("Volume-based conversions (approximate):");
        for w in &warnings {
            eprintln!("{w}");
        }
    }

    let detail = db.get_recipe_detail(recipe.id)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&detail)?);
    } else {
        let rname = &detail.name;
        let ing_count = detail.ingredients.len();
        let rportions = detail.portions;
        let pp_cal = detail.per_portion_calories;
        println!(
            "Imported recipe: {rname} ({ing_count} ingredients, {rportions} portions, {pp_cal:.0} kcal/portion)"
        );
    }

    Ok(())
}

fn cooklang_ingredient_to_grub(
    gi: &cooklang::ingredient_list::GroupedIngredient<'_>,
) -> CooklangIngredient {
    // Take the first quantity from the grouped quantities (if any)
    let (quantity, units) =
        gi.quantity
            .iter()
            .next()
            .map_or((None, None), |qty: &cooklang::Quantity| {
                let value = match qty.value() {
                    cooklang::Value::Number(n) => Some(serde_json::Value::Number(
                        serde_json::Number::from_f64(n.value())
                            .unwrap_or_else(|| serde_json::Number::from(1)),
                    )),
                    cooklang::Value::Range { start, .. } => Some(serde_json::Value::Number(
                        serde_json::Number::from_f64(start.value())
                            .unwrap_or_else(|| serde_json::Number::from(1)),
                    )),
                    cooklang::Value::Text(t) => Some(serde_json::Value::String(t.clone())),
                };
                let unit = qty.unit().map(String::from);
                (value, unit)
            });

    CooklangIngredient {
        name: gi.ingredient.display_name().to_string(),
        quantity,
        units,
    }
}

async fn import_ingredients(
    db: &Database,
    off: &OpenFoodFactsClient,
    recipe_id: i64,
    ingredients: &[CooklangIngredient],
) -> Result<Vec<String>> {
    let mut warnings = Vec::new();

    for ing in ingredients {
        let raw_qty = match &ing.quantity {
            Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(1.0),
            Some(serde_json::Value::String(s)) => s.parse::<f64>().unwrap_or(1.0),
            _ => 1.0,
        };

        let quantity_g = if let Some(unit) = &ing.units {
            match convert_to_grams(raw_qty, unit) {
                Some((g, true)) => {
                    let ing_name = &ing.name;
                    warnings.push(format!(
                        "  {ing_name}: {raw_qty} {unit} → {g:.0}g (approximate, assumes water density)"
                    ));
                    g
                }
                Some((g, false)) => g,
                None => {
                    let ing_name = &ing.name;
                    eprintln!(
                        "Warning: Unknown unit '{unit}' for {ing_name}, treating {raw_qty} as grams"
                    );
                    raw_qty
                }
            }
        } else {
            raw_qty
        };

        match resolve_food(db, off, &ing.name).await {
            Ok(f) => {
                db.add_recipe_ingredient(recipe_id, f.id, quantity_g)?;
            }
            Err(e) => {
                let ing_name = &ing.name;
                eprintln!("Warning: Could not resolve '{ing_name}': {e}");
            }
        }
    }

    Ok(warnings)
}
