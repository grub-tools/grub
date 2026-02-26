use anyhow::Result;
use chrono::Local;
use std::process;
use tabled::{
    Table, Tabled,
    settings::{Alignment, Modify, Style, object::Columns},
};

use grub_core::db::Database;

use super::helpers::{no_neg_zero, parse_date};

pub(crate) fn cmd_summary(db: &Database, date: Option<String>, json: bool) -> Result<()> {
    let date = parse_date(date)?;
    let summary = db.build_daily_summary(date)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(());
    }

    if summary.meals.is_empty() {
        let date = &summary.date;
        eprintln!("No entries for {date}");
        process::exit(2);
    }

    let date = &summary.date;
    println!("=== {date} ===\n");

    for meal in &summary.meals {
        let meal_label = meal.meal_type.to_uppercase();
        let sub_cal = meal.subtotal_calories;
        println!("  {meal_label} ({sub_cal:.0} kcal)");
        for e in &meal.entries {
            let name = e.food_name.as_deref().unwrap_or("?");
            let brand = e
                .food_brand
                .as_ref()
                .map(|b| format!(" ({b})"))
                .unwrap_or_default();
            let id = e.id;
            let serving_display = match (&e.display_unit, e.display_quantity) {
                (Some(unit), Some(qty)) if qty.fract() == 0.0 => format!("{qty:.0}{unit}"),
                (Some(unit), Some(qty)) => format!("{qty}{unit}"),
                _ => format!("{:.0}g", e.serving_g),
            };
            let cal = e.calories.unwrap_or(0.0);
            let protein = e.protein.unwrap_or(0.0);
            let carbs = e.carbs.unwrap_or(0.0);
            let fat = e.fat.unwrap_or(0.0);
            println!(
                "    [{id}] {name}{brand} — {serving_display} — {cal:.0} kcal | P:{protein:.0}g C:{carbs:.0}g F:{fat:.0}g"
            );
        }
        println!();
    }

    let total_cal = summary.total_calories;
    let total_p = summary.total_protein;
    let total_c = summary.total_carbs;
    let total_f = summary.total_fat;
    println!("  TOTAL: {total_cal:.0} kcal | P:{total_p:.0}g C:{total_c:.0}g F:{total_f:.0}g");

    if let Some(target) = &summary.target {
        let tcal = target.calories;
        #[allow(clippy::cast_precision_loss)]
        let tcal_f = tcal as f64;
        if let (Some(pg), Some(cg), Some(fg)) = (target.protein_g, target.carbs_g, target.fat_g) {
            println!("  TARGET: {tcal} kcal | P:{pg:.0}g C:{cg:.0}g F:{fg:.0}g");
            let rcal = tcal_f - total_cal;
            let rp = pg - total_p;
            let rc = cg - total_c;
            let rf = fg - total_f;
            println!("  REMAINING: {rcal:.0} kcal | P:{rp:.0}g C:{rc:.0}g F:{rf:.0}g");
        } else {
            println!("  TARGET: {tcal} kcal");
            let rcal = tcal_f - total_cal;
            println!("  REMAINING: {rcal:.0} kcal");
        }
    }

    Ok(())
}

pub(crate) fn cmd_history(db: &Database, days: u32, json: bool) -> Result<()> {
    #[derive(Tabled)]
    struct HistoryRow {
        #[tabled(rename = "Date")]
        date: String,
        #[tabled(rename = "Calories")]
        calories: String,
        #[tabled(rename = "Protein")]
        protein: String,
        #[tabled(rename = "Carbs")]
        carbs: String,
        #[tabled(rename = "Fat")]
        fat: String,
    }

    let today = Local::now().date_naive();
    let mut summaries = Vec::new();

    for i in 0..days {
        let date = today - chrono::Duration::days(i64::from(i));
        let summary = db.build_daily_summary(date)?;
        summaries.push(summary);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&summaries)?);
        return Ok(());
    }

    let rows: Vec<HistoryRow> = summaries
        .iter()
        .map(|s| {
            let cal = no_neg_zero(s.total_calories);
            let p = no_neg_zero(s.total_protein);
            let c = no_neg_zero(s.total_carbs);
            let f = no_neg_zero(s.total_fat);
            HistoryRow {
                date: s.date.clone(),
                calories: format!("{cal:.0}"),
                protein: format!("{p:.0}g"),
                carbs: format!("{c:.0}g"),
                fat: format!("{f:.0}g"),
            }
        })
        .collect();

    if rows.iter().all(|r| r.calories == "0") {
        eprintln!("No entries in the last {days} days");
        process::exit(2);
    }

    let table = Table::new(&rows)
        .with(Style::rounded())
        .with(Modify::new(Columns::new(1..)).with(Alignment::right()))
        .to_string();
    println!("{table}");

    Ok(())
}
