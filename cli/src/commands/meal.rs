use anyhow::{Result, bail};
use std::process;

use grub_core::db::Database;
use grub_core::models::{NewMealEntry, UpdateMealEntry, validate_meal_type};

use super::helpers::{json_error, parse_date, parse_meal_ref, parse_serving_with_unit};
use super::log::format_serving_display;

pub(crate) fn cmd_delete(db: &Database, entry_id: i64, json: bool) -> Result<()> {
    if db.delete_meal_entry(entry_id)? {
        if json {
            println!("{}", serde_json::json!({ "deleted": entry_id }));
        } else {
            println!("Deleted entry {entry_id}");
        }
        Ok(())
    } else {
        if json {
            println!("{}", json_error(&format!("Entry {entry_id} not found")));
        } else {
            eprintln!("Entry {entry_id} not found");
        }
        process::exit(2);
    }
}

pub(crate) fn cmd_update(
    db: &Database,
    entry_id: i64,
    serving: Option<&String>,
    meal: Option<&String>,
    date: Option<String>,
    json: bool,
) -> Result<()> {
    if serving.is_none() && meal.is_none() && date.is_none() {
        bail!("Nothing to update. Provide at least one of --serving, --meal, or --date");
    }

    let (serving_g, display_unit, display_quantity) = match serving {
        Some(s) => {
            let (g, du, dq) = parse_serving_with_unit(s)?;
            (Some(g), Some(du), Some(dq))
        }
        None => (None, None, None),
    };
    let meal_type = meal.map(|m| validate_meal_type(m)).transpose()?;
    let parsed_date = date.map(Some).map(parse_date).transpose()?;

    let update = UpdateMealEntry {
        serving_g,
        meal_type,
        date: parsed_date,
        display_unit,
        display_quantity,
    };

    if let Ok(entry) = db.update_meal_entry(entry_id, &update) {
        if json {
            println!("{}", serde_json::to_string_pretty(&entry)?);
        } else {
            let name = entry.food_name.as_deref().unwrap_or("?");
            let serving_display = format_serving_display(&entry);
            let meal = &entry.meal_type;
            let cal = entry.calories.unwrap_or(0.0);
            println!(
                "Updated entry {entry_id}: {name} {serving_display} for {meal} — {cal:.0} kcal"
            );
        }
        Ok(())
    } else {
        if json {
            println!("{}", json_error(&format!("Entry {entry_id} not found")));
        } else {
            eprintln!("Entry {entry_id} not found");
        }
        process::exit(2);
    }
}

pub(crate) fn cmd_copy(db: &Database, from: &str, to: &str, json: bool) -> Result<()> {
    let (from_date, from_meal) = parse_meal_ref(from)?;
    let (to_date, to_meal) = parse_meal_ref(to)?;

    let entries = db.get_entries_for_date_and_meal(from_date, &from_meal)?;

    if entries.is_empty() {
        if json {
            println!(
                "{}",
                json_error(&format!("No entries found for {from_date}:{from_meal}"))
            );
        } else {
            eprintln!("No entries found for {from_date}:{from_meal}");
        }
        process::exit(2);
    }

    let mut copied = Vec::new();
    for e in &entries {
        let new_entry = db.insert_meal_entry(&NewMealEntry {
            date: to_date,
            meal_type: to_meal.clone(),
            food_id: e.food_id,
            serving_g: e.serving_g,
            display_unit: e.display_unit.clone(),
            display_quantity: e.display_quantity,
        })?;
        copied.push(new_entry);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&copied)?);
    } else {
        let count = copied.len();
        println!("Copied {count} entries from {from_date}:{from_meal} to {to_date}:{to_meal}");
    }

    Ok(())
}
