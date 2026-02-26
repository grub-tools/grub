use anyhow::{Result, bail};

use grub_core::db::Database;
use grub_core::models::validate_macro_split;

const DAY_NAMES: &[&str] = &[
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

#[allow(clippy::cast_sign_loss)]
fn day_name(day_of_week: i64) -> &'static str {
    DAY_NAMES[day_of_week as usize]
}

fn parse_days(day: &str) -> Result<Vec<i64>> {
    match day.to_lowercase().as_str() {
        "monday" | "mon" => Ok(vec![0]),
        "tuesday" | "tue" => Ok(vec![1]),
        "wednesday" | "wed" => Ok(vec![2]),
        "thursday" | "thu" => Ok(vec![3]),
        "friday" | "fri" => Ok(vec![4]),
        "saturday" | "sat" => Ok(vec![5]),
        "sunday" | "sun" => Ok(vec![6]),
        "weekdays" => Ok(vec![0, 1, 2, 3, 4]),
        "weekends" => Ok(vec![5, 6]),
        "all" => Ok(vec![0, 1, 2, 3, 4, 5, 6]),
        _ => bail!("Invalid day: {day}. Use monday-sunday, mon-sun, weekdays, weekends, or all"),
    }
}

pub(crate) fn cmd_target_set(
    db: &Database,
    calories: i64,
    protein: Option<i64>,
    carbs: Option<i64>,
    fat: Option<i64>,
    day: &str,
    json: bool,
) -> Result<()> {
    if calories <= 0 {
        bail!("Calorie target must be greater than 0");
    }

    // If any macro % is provided, all three must be provided
    let (protein_pct, carbs_pct, fat_pct) = match (protein, carbs, fat) {
        (None, None, None) => (None, None, None),
        (Some(p), Some(c), Some(f)) => {
            validate_macro_split(p, c, f)?;
            (Some(p), Some(c), Some(f))
        }
        _ => bail!(
            "If setting macro percentages, all three (--protein, --carbs, --fat) must be provided"
        ),
    };

    let days = parse_days(day)?;
    let mut targets = Vec::new();

    for &d in &days {
        let target = db.set_target(d, calories, protein_pct, carbs_pct, fat_pct)?;
        targets.push(target);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&targets)?);
    } else {
        for target in &targets {
            let day_name = day_name(target.day_of_week);
            print!("{day_name}: {calories} kcal/day");
            if let (Some(p), Some(c), Some(f)) =
                (target.protein_pct, target.carbs_pct, target.fat_pct)
            {
                let pg = target.protein_g.unwrap_or(0.0);
                let cg = target.carbs_g.unwrap_or(0.0);
                let fg = target.fat_g.unwrap_or(0.0);
                print!("  Protein: {p}% ({pg:.0}g)  Carbs: {c}% ({cg:.0}g)  Fat: {f}% ({fg:.0}g)");
            }
            println!();
        }
    }

    Ok(())
}

pub(crate) fn cmd_target_show(db: &Database, json: bool) -> Result<()> {
    let targets = db.get_all_targets()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&targets)?);
    } else if targets.is_empty() {
        eprintln!("No targets set. Use `grub target set <calories>` to set one.");
    } else {
        for target in &targets {
            let day_name = day_name(target.day_of_week);
            let cal = target.calories;
            print!("{day_name}: {cal} kcal/day");
            if let (Some(p), Some(c), Some(f)) =
                (target.protein_pct, target.carbs_pct, target.fat_pct)
            {
                let pg = target.protein_g.unwrap_or(0.0);
                let cg = target.carbs_g.unwrap_or(0.0);
                let fg = target.fat_g.unwrap_or(0.0);
                print!("  Protein: {p}% ({pg:.0}g)  Carbs: {c}% ({cg:.0}g)  Fat: {f}% ({fg:.0}g)");
            }
            println!();
        }
    }

    Ok(())
}

pub(crate) fn cmd_target_clear(db: &Database, day: Option<&str>, json: bool) -> Result<()> {
    let cleared = if let Some(day_str) = day {
        let days = parse_days(day_str)?;
        let mut any_cleared = false;
        for &d in &days {
            if db.clear_target(d)? {
                any_cleared = true;
            }
        }
        any_cleared
    } else {
        db.clear_all_targets()?
    };

    if json {
        println!("{}", serde_json::json!({ "cleared": cleared }));
    } else if cleared {
        println!("Target(s) cleared");
    } else {
        eprintln!("No target was set");
    }
    Ok(())
}
