use anyhow::{Result, bail};
use tabled::{
    Table, Tabled,
    settings::{Alignment, Modify, Style, object::Columns},
};

use grub_core::db::Database;
use grub_core::models::NewWeightEntry;

use super::helpers::{no_neg_zero, parse_date};

const LBS_PER_KG: f64 = 2.20462;
const KG_PER_LB: f64 = 0.453_592;

pub(crate) fn cmd_weight_log(
    db: &Database,
    value: f64,
    unit: &str,
    date: Option<String>,
    notes: Option<String>,
    json: bool,
) -> Result<()> {
    if value <= 0.0 {
        bail!("Weight must be greater than 0");
    }

    let weight_kg = match unit.to_lowercase().as_str() {
        "kg" => value,
        "lbs" | "lb" => {
            let kg = no_neg_zero(value * KG_PER_LB);
            eprintln!("Converting {value:.1} lbs → {kg:.2} kg");
            kg
        }
        _ => bail!("Invalid unit '{unit}'. Use 'kg' or 'lbs'"),
    };

    let date = parse_date(date)?;
    let entry = NewWeightEntry {
        date,
        weight_kg,
        source: "manual".to_string(),
        notes,
    };

    let result = db.upsert_weight(&entry)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        let lbs = result.weight_kg * LBS_PER_KG;
        println!(
            "Logged {:.1} kg ({:.1} lbs) for {}",
            result.weight_kg,
            lbs,
            result.date.format("%Y-%m-%d")
        );
        if let Some(ref n) = result.notes {
            println!("  Notes: {n}");
        }
    }

    Ok(())
}

pub(crate) fn cmd_weight_show(db: &Database, date: Option<String>, json: bool) -> Result<()> {
    let date = parse_date(date)?;
    let entry = db.get_weight(date)?;

    if let Some(e) = entry {
        if json {
            println!("{}", serde_json::to_string_pretty(&e)?);
        } else {
            let lbs = e.weight_kg * LBS_PER_KG;
            println!(
                "{}: {:.1} kg ({:.1} lbs)",
                e.date.format("%Y-%m-%d"),
                e.weight_kg,
                lbs
            );
            if let Some(ref n) = e.notes {
                println!("  Notes: {n}");
            }
        }
    } else {
        let date_str = date.format("%Y-%m-%d");
        if json {
            println!(
                "{}",
                serde_json::json!({ "error": format!("No weight entry for {date_str}") })
            );
        } else {
            eprintln!("No weight entry for {date_str}");
        }
    }

    Ok(())
}

pub(crate) fn cmd_weight_history(db: &Database, days: Option<u32>, json: bool) -> Result<()> {
    let entries = db.get_weight_history(days.map(i64::from))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else if entries.is_empty() {
        eprintln!("No weight entries found. Use `grub weight log` to record your weight.");
    } else {
        #[derive(Tabled)]
        struct WeightRow {
            #[tabled(rename = "ID")]
            id: i64,
            #[tabled(rename = "Date")]
            date: String,
            #[tabled(rename = "Weight (kg)")]
            kg: String,
            #[tabled(rename = "Weight (lbs)")]
            lbs: String,
            #[tabled(rename = "Notes")]
            notes: String,
        }

        let rows: Vec<WeightRow> = entries
            .iter()
            .map(|e| WeightRow {
                id: e.id,
                date: e.date.format("%Y-%m-%d").to_string(),
                kg: format!("{:.1}", e.weight_kg),
                lbs: format!("{:.1}", e.weight_kg * LBS_PER_KG),
                notes: e.notes.clone().unwrap_or_default(),
            })
            .collect();

        let table = Table::new(&rows)
            .with(Style::rounded())
            .with(Modify::new(Columns::new(2..4)).with(Alignment::right()))
            .to_string();
        println!("{table}");
    }

    Ok(())
}

pub(crate) fn cmd_weight_delete(db: &Database, id: i64, json: bool) -> Result<()> {
    db.delete_weight(id)?;

    if json {
        println!("{}", serde_json::json!({ "deleted": id }));
    } else {
        println!("Deleted weight entry {id}");
    }

    Ok(())
}
