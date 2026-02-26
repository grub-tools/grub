use anyhow::{Context, Result, bail};
use chrono::{Local, NaiveDate};
use serde::Serialize;
use std::io::{self, BufRead, Write};
use tabled::{
    Table, Tabled,
    settings::{Alignment, Modify, Style, object::Columns},
};

use grub_core::models::Food;

/// Parse a serving string with optional unit, returning `(grams, display_unit, display_quantity)`.
/// Accepts: "200", "200g", "500ml", "500 ml", "2 tbsp", "1.5 oz", etc.
/// Returns the converted grams value plus the original unit/quantity for display.
pub(crate) fn parse_serving_with_unit(s: &str) -> Result<(f64, Option<String>, Option<f64>)> {
    use grub_core::models::convert_to_grams;

    let s = s.trim();

    // Try plain grams first: "500" or "500g"
    if let Ok(g) = parse_serving(s) {
        return Ok((g, None, None));
    }

    // Try "N<unit>" with no space (e.g. "500ml", "2tbsp")
    if let Some((qty, unit)) = split_number_unit(s) {
        if let Some((grams, is_approx)) = convert_to_grams(qty, unit) {
            if is_approx {
                eprintln!("Note: {qty} {unit} ≈ {grams:.0}g (approximate, assumes water density)");
            }
            return Ok((grams, Some(unit.to_lowercase()), Some(qty)));
        }
        bail!("Unknown unit '{unit}' in '{s}'. Supported: g, kg, lb, oz, tbsp, tsp, ml, l");
    }

    // Try "<number> <unit>" format
    let parts: Vec<&str> = s.splitn(2, char::is_whitespace).collect();
    if parts.len() == 2 {
        let qty: f64 = parts[0]
            .parse()
            .with_context(|| format!("Invalid quantity: '{s}'"))?;
        let unit = parts[1].trim();
        if let Some((grams, is_approx)) = convert_to_grams(qty, unit) {
            if is_approx {
                eprintln!("Note: {qty} {unit} ≈ {grams:.0}g (approximate, assumes water density)");
            }
            return Ok((grams, Some(unit.to_lowercase()), Some(qty)));
        }
        bail!("Unknown unit '{unit}' in '{s}'. Supported: g, kg, lb, oz, tbsp, tsp, ml, l");
    }

    bail!("Invalid serving format: '{s}'. Use '200g', '500ml', '2 tbsp', etc.")
}

/// Split "500ml" or "2.5tbsp" into (500.0, "ml") or (2.5, "tbsp").
fn split_number_unit(s: &str) -> Option<(f64, &str)> {
    let idx = s.find(|c: char| c.is_alphabetic())?;
    if idx == 0 {
        return None;
    }
    let (num_part, unit_part) = s.split_at(idx);
    let qty: f64 = num_part.parse().ok()?;
    if unit_part.is_empty() {
        return None;
    }
    Some((qty, unit_part))
}

/// Parse a quantity string like "500g", "1.5 lb" into grams.
pub(crate) fn parse_ingredient_quantity(s: &str) -> Result<f64> {
    use grub_core::models::convert_to_grams;

    let s = s.trim();

    // Try plain grams first: "500" or "500g"
    if let Ok(g) = parse_serving(s) {
        return Ok(g);
    }

    // Try "<number> <unit>" format
    let parts: Vec<&str> = s.splitn(2, char::is_whitespace).collect();
    if parts.len() == 2 {
        let qty: f64 = parts[0]
            .parse()
            .with_context(|| format!("Invalid quantity: '{s}'"))?;
        let unit = parts[1].trim();
        if let Some((grams, is_approx)) = convert_to_grams(qty, unit) {
            if is_approx {
                eprintln!("Note: {qty} {unit} → {grams:.0}g (approximate, assumes water density)");
            }
            return Ok(grams);
        }
        bail!("Unknown unit '{unit}' in '{s}'. Supported: g, kg, lb, oz, tbsp, tsp, ml, l");
    }

    bail!("Invalid quantity format: '{s}'. Use '<number>g' or '<number> <unit>'")
}

pub(crate) fn parse_serving(s: &str) -> Result<f64> {
    let trimmed = s.trim_end_matches('g').trim();
    let value: f64 = trimmed.parse().with_context(|| {
        format!("Invalid serving size: '{s}'. Use a number like '200' or '200g'")
    })?;
    if value <= 0.0 {
        bail!("Serving size must be greater than 0");
    }
    Ok(value)
}

pub(crate) fn parse_date(date_str: Option<String>) -> Result<NaiveDate> {
    match date_str {
        None => Ok(Local::now().date_naive()),
        Some(s) => match s.as_str() {
            "today" => Ok(Local::now().date_naive()),
            "yesterday" => Ok(Local::now().date_naive() - chrono::Duration::days(1)),
            "tomorrow" => Ok(Local::now().date_naive() + chrono::Duration::days(1)),
            _ => NaiveDate::parse_from_str(&s, "%Y-%m-%d").with_context(|| {
                format!("Invalid date '{s}'. Use YYYY-MM-DD or today/yesterday/tomorrow")
            }),
        },
    }
}

pub(crate) fn parse_meal_ref(s: &str) -> Result<(NaiveDate, String)> {
    use grub_core::models::validate_meal_type;

    let parts: Vec<&str> = s.splitn(2, ':').collect();
    if parts.len() != 2 {
        bail!("Invalid meal reference '{s}'. Use format 'date:meal' (e.g. 'today:lunch')");
    }
    let date = parse_date(Some(parts[0].to_string()))?;
    let meal = validate_meal_type(parts[1])?;
    Ok((date, meal))
}

pub(crate) fn prompt_choice(count: usize) -> Result<usize> {
    eprint!("\nSelect a food (1-{count}): ");
    io::stderr().flush()?;
    let stdin = io::stdin();
    let line = stdin.lock().lines().next().context("No input")??;
    let n: usize = line.trim().parse().context("Invalid number")?;
    if n < 1 || n > count {
        bail!("Selection out of range");
    }
    Ok(n - 1)
}

pub(crate) fn print_food_table(foods: &[&Food]) {
    #[derive(Tabled)]
    struct FoodRow {
        #[tabled(rename = "#")]
        idx: usize,
        #[tabled(rename = "ID")]
        id: i64,
        #[tabled(rename = "Name")]
        name: String,
        #[tabled(rename = "Brand")]
        brand: String,
        #[tabled(rename = "Cal/100g")]
        calories: String,
        #[tabled(rename = "P/100g")]
        protein: String,
        #[tabled(rename = "C/100g")]
        carbs: String,
        #[tabled(rename = "F/100g")]
        fat: String,
        #[tabled(rename = "Source")]
        source: String,
    }

    let rows: Vec<FoodRow> = foods
        .iter()
        .enumerate()
        .map(|(i, f)| FoodRow {
            idx: i + 1,
            id: f.id,
            name: truncate(&f.name, 35),
            brand: f
                .brand
                .as_deref()
                .map(|b| truncate(b, 20))
                .unwrap_or_default(),
            calories: {
                let cal = f.calories_per_100g;
                format!("{cal:.0}")
            },
            protein: f.protein_per_100g.map_or("-".into(), |v| format!("{v:.1}")),
            carbs: f.carbs_per_100g.map_or("-".into(), |v| format!("{v:.1}")),
            fat: f.fat_per_100g.map_or("-".into(), |v| format!("{v:.1}")),
            source: f.source.clone(),
        })
        .collect();

    let table = Table::new(&rows)
        .with(Style::rounded())
        .with(Modify::new(Columns::new(4..8)).with(Alignment::right()))
        .to_string();
    println!("{table}");
}

pub(crate) fn json_error(message: &str) -> String {
    #[derive(Serialize)]
    struct CliError<'a> {
        error: &'a str,
    }
    serde_json::to_string(&CliError { error: message })
        .unwrap_or_else(|_| format!("{{\"error\":\"{message}\"}}"))
}

pub(crate) fn no_neg_zero(v: f64) -> f64 {
    if v == 0.0 { 0.0 } else { v }
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let end = s.char_indices().nth(max - 3).map_or(s.len(), |(i, _)| i);
        format!("{}...", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_serving() {
        assert!((parse_serving("200").unwrap() - 200.0).abs() < f64::EPSILON);
        assert!((parse_serving("200g").unwrap() - 200.0).abs() < f64::EPSILON);
        assert!((parse_serving("200.5g").unwrap() - 200.5).abs() < f64::EPSILON);
        assert!((parse_serving("200 ").unwrap() - 200.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_serving_invalid() {
        assert!(parse_serving("abc").is_err());
    }

    #[test]
    fn test_parse_serving_zero() {
        assert!(parse_serving("0").is_err());
        assert!(parse_serving("0g").is_err());
    }

    #[test]
    fn test_parse_serving_negative() {
        assert!(parse_serving("-50").is_err());
        assert!(parse_serving("-50g").is_err());
    }

    #[test]
    fn test_parse_date_none() {
        let today = Local::now().date_naive();
        assert_eq!(parse_date(None).unwrap(), today);
    }

    #[test]
    fn test_parse_date_keywords() {
        let today = Local::now().date_naive();
        assert_eq!(parse_date(Some("today".to_string())).unwrap(), today);
        assert_eq!(
            parse_date(Some("yesterday".to_string())).unwrap(),
            today - chrono::Duration::days(1)
        );
        assert_eq!(
            parse_date(Some("tomorrow".to_string())).unwrap(),
            today + chrono::Duration::days(1)
        );
    }

    #[test]
    fn test_parse_date_iso() {
        let date = parse_date(Some("2024-01-15".to_string())).unwrap();
        assert_eq!(date, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
    }

    #[test]
    fn test_parse_date_invalid() {
        assert!(parse_date(Some("nope".to_string())).is_err());
    }

    #[test]
    fn test_parse_meal_ref() {
        let (date, meal) = parse_meal_ref("today:lunch").unwrap();
        assert_eq!(date, Local::now().date_naive());
        assert_eq!(meal, "lunch");
    }

    #[test]
    fn test_parse_meal_ref_invalid() {
        assert!(parse_meal_ref("nocolon").is_err());
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world this is long", 10), "hello w...");
    }

    #[test]
    fn test_truncate_utf8() {
        // Should not panic on multi-byte characters
        assert_eq!(truncate("Crème fraîche", 10), "Crème f...");
        assert_eq!(truncate("Müsli", 10), "Müsli");
        assert_eq!(truncate("日清カップヌードル", 8), "日清カップ...");
    }

    #[test]
    fn test_no_neg_zero() {
        assert_eq!(no_neg_zero(-0.0).to_bits(), 0.0_f64.to_bits());
        assert_eq!(no_neg_zero(5.0), 5.0);
        assert_eq!(no_neg_zero(-3.0), -3.0);
    }
}
