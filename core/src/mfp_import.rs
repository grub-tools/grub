use std::collections::HashMap;
use std::io::Read;

use anyhow::{Context, Result, bail};

use crate::db::Database;
use crate::models::NewFood;

/// A single row parsed from an MFP CSV export.
#[derive(Debug, Clone)]
pub struct MfpRow {
    pub date: String,
    pub meal: String,
    pub food_name: String,
    pub calories: f64,
    pub fat: f64,
    pub protein: f64,
    pub carbs: f64,
    pub fiber: Option<f64>,
    pub sugar: Option<f64>,
}

/// Summary of what an MFP import would do / did.
#[derive(Debug, Clone)]
pub struct MfpImportSummary {
    pub rows_parsed: usize,
    pub foods_created: usize,
    pub foods_reused: usize,
    pub meals_logged: usize,
    pub dates_spanned: usize,
}

/// Parse an MFP CSV export from any reader.
///
/// Expected header:
/// `Date,Meal,Food Name,Calories,Fat (g),Protein (g),Carbohydrates (g),Fiber (g),Sugar (g)`
///
/// Columns after the first 7 (Carbohydrates) are optional.
pub fn parse_mfp_csv<R: Read>(reader: R) -> Result<Vec<MfpRow>> {
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(reader);

    let headers = rdr.headers().context("Failed to read CSV headers")?.clone();

    // Validate required columns
    let required = ["Date", "Meal", "Food Name", "Calories"];
    for name in &required {
        if !headers.iter().any(|h| h.eq_ignore_ascii_case(name)) {
            bail!("Missing required column: {name}");
        }
    }

    // Build column index map (case-insensitive)
    let col =
        |name: &str| -> Option<usize> { headers.iter().position(|h| h.eq_ignore_ascii_case(name)) };

    let idx_date = col("Date").context("Missing 'Date' column")?;
    let idx_meal = col("Meal").context("Missing 'Meal' column")?;
    let idx_food = col("Food Name").context("Missing 'Food Name' column")?;
    let idx_cal = col("Calories").context("Missing 'Calories' column")?;
    let idx_fat = col("Fat (g)");
    let idx_protein = col("Protein (g)");
    let idx_carbs = col("Carbohydrates (g)");
    let idx_fiber = col("Fiber (g)");
    let idx_sugar = col("Sugar (g)");

    let mut rows = Vec::new();

    for (line_num, result) in rdr.records().enumerate() {
        let record = result.with_context(|| format!("Failed to parse CSV row {}", line_num + 2))?;

        let date = record.get(idx_date).unwrap_or("").trim().to_string();
        let meal = record.get(idx_meal).unwrap_or("").trim().to_string();
        let food_name = record.get(idx_food).unwrap_or("").trim().to_string();

        if date.is_empty() || food_name.is_empty() {
            continue; // skip blank rows
        }

        let parse_f64 = |idx: Option<usize>| -> f64 {
            idx.and_then(|i| record.get(i))
                .and_then(|v| v.trim().parse::<f64>().ok())
                .unwrap_or(0.0)
        };

        let parse_opt_f64 = |idx: Option<usize>| -> Option<f64> {
            idx.and_then(|i| record.get(i))
                .and_then(|v| v.trim().parse::<f64>().ok())
        };

        let calories = parse_f64(Some(idx_cal));

        rows.push(MfpRow {
            date,
            meal,
            food_name,
            calories,
            fat: parse_f64(idx_fat),
            protein: parse_f64(idx_protein),
            carbs: parse_f64(idx_carbs),
            fiber: parse_opt_f64(idx_fiber),
            sugar: parse_opt_f64(idx_sugar),
        });
    }

    Ok(rows)
}

/// Normalize an MFP meal name to one of grub's valid meal types.
#[must_use]
pub fn normalize_meal_type(mfp_meal: &str) -> &'static str {
    match mfp_meal.to_lowercase().as_str() {
        "breakfast" => "breakfast",
        "lunch" => "lunch",
        "dinner" => "dinner",
        _ => "snack",
    }
}

/// Normalize an MFP date to YYYY-MM-DD format.
///
/// MFP exports dates as `YYYY-MM-DD` (or sometimes `M/D/YYYY`).
fn normalize_date(mfp_date: &str) -> Result<String> {
    // Try YYYY-MM-DD first
    if chrono::NaiveDate::parse_from_str(mfp_date, "%Y-%m-%d").is_ok() {
        return Ok(mfp_date.to_string());
    }
    // Try M/D/YYYY
    if let Ok(d) = chrono::NaiveDate::parse_from_str(mfp_date, "%m/%d/%Y") {
        return Ok(d.format("%Y-%m-%d").to_string());
    }
    // Try D/M/YYYY
    if let Ok(d) = chrono::NaiveDate::parse_from_str(mfp_date, "%d/%m/%Y") {
        return Ok(d.format("%Y-%m-%d").to_string());
    }
    bail!("Cannot parse date: '{mfp_date}'")
}

/// Calculate per-100g values from per-serving nutrition.
///
/// MFP exports total calories/macros per serving. We assume a default serving
/// of 100g when no serving weight is available (since MFP doesn't export weight).
fn to_per_100g(value: f64) -> f64 {
    // MFP exports per-serving values. Without serving weight info, we store
    // the values as-is (treating 1 serving = 100g equivalent).
    value
}

/// Import parsed MFP rows into the database.
///
/// Returns an `MfpImportSummary`. When `dry_run` is true, no data is written.
pub fn import_mfp_meals(db: &Database, rows: &[MfpRow], dry_run: bool) -> Result<MfpImportSummary> {
    let mut foods_created: usize = 0;
    let mut foods_reused: usize = 0;
    let mut meals_logged: usize = 0;
    let mut dates: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Cache: food_name → food_id (to avoid repeated DB lookups)
    let mut food_cache: HashMap<String, i64> = HashMap::new();

    for row in rows {
        let date = normalize_date(&row.date)?;
        dates.insert(date.clone());

        let meal_type = normalize_meal_type(&row.meal);

        // Resolve or create food
        let food_key = row.food_name.to_lowercase();
        let food_id = if let Some(&id) = food_cache.get(&food_key) {
            foods_reused += 1;
            id
        } else if dry_run {
            // In dry-run, check if food exists but don't create
            let existing = deduplicate_food(db, &row.food_name)?;
            if let Some(f) = existing {
                food_cache.insert(food_key, f);
                foods_reused += 1;
                f
            } else {
                foods_created += 1;
                0 // placeholder
            }
        } else {
            let existing = deduplicate_food(db, &row.food_name)?;
            if let Some(id) = existing {
                food_cache.insert(food_key, id);
                foods_reused += 1;
                id
            } else {
                let new_food = NewFood {
                    name: row.food_name.clone(),
                    brand: None,
                    barcode: None,
                    calories_per_100g: to_per_100g(row.calories),
                    protein_per_100g: Some(to_per_100g(row.protein)),
                    carbs_per_100g: Some(to_per_100g(row.carbs)),
                    fat_per_100g: Some(to_per_100g(row.fat)),
                    default_serving_g: Some(100.0),
                    source: "myfitnesspal".to_string(),
                };
                let food = db.insert_food(&new_food)?;
                food_cache.insert(food_key, food.id);
                foods_created += 1;
                food.id
            }
        };

        if !dry_run {
            let parsed_date = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d")?;
            db.insert_meal_entry(&crate::models::NewMealEntry {
                date: parsed_date,
                meal_type: meal_type.to_string(),
                food_id,
                serving_g: 100.0, // 1 serving = 100g equivalent
                display_unit: Some("serving".to_string()),
                display_quantity: Some(1.0),
            })?;
        }
        meals_logged += 1;
    }

    Ok(MfpImportSummary {
        rows_parsed: rows.len(),
        foods_created,
        foods_reused,
        meals_logged,
        dates_spanned: dates.len(),
    })
}

/// Try to find an existing food by name (case-insensitive).
fn deduplicate_food(db: &Database, name: &str) -> Result<Option<i64>> {
    let results = db.search_foods_local(name)?;
    for food in &results {
        if food.name.eq_ignore_ascii_case(name) {
            return Ok(Some(food.id));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CSV: &str = "\
Date,Meal,Food Name,Calories,Fat (g),Protein (g),Carbohydrates (g),Fiber (g),Sugar (g)
2024-01-15,Breakfast,Oatmeal - Plain,150,3,5,27,4,1
2024-01-15,Lunch,Chicken Breast - Grilled,165,3.6,31,0,0,0
2024-01-15,Dinner,Salmon Fillet,208,13,20,0,0,0
2024-01-16,Breakfast,Greek Yogurt,100,0.7,17,6,0,4
2024-01-16,Snacks,Almonds - Raw,164,14.2,6,6.1,3.5,1.2
";

    #[test]
    fn test_parse_mfp_csv_basic() {
        let rows = parse_mfp_csv(SAMPLE_CSV.as_bytes()).unwrap();
        assert_eq!(rows.len(), 5);

        assert_eq!(rows[0].date, "2024-01-15");
        assert_eq!(rows[0].meal, "Breakfast");
        assert_eq!(rows[0].food_name, "Oatmeal - Plain");
        assert!((rows[0].calories - 150.0).abs() < f64::EPSILON);
        assert!((rows[0].protein - 5.0).abs() < f64::EPSILON);
        assert!((rows[0].carbs - 27.0).abs() < f64::EPSILON);
        assert!((rows[0].fat - 3.0).abs() < f64::EPSILON);
        assert!((rows[0].fiber.unwrap() - 4.0).abs() < f64::EPSILON);
        assert!((rows[0].sugar.unwrap() - 1.0).abs() < f64::EPSILON);

        assert_eq!(rows[4].food_name, "Almonds - Raw");
    }

    #[test]
    fn test_parse_mfp_csv_missing_required_column() {
        let bad_csv = "Date,Meal,Calories\n2024-01-15,Lunch,100\n";
        let result = parse_mfp_csv(bad_csv.as_bytes());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Food Name"));
    }

    #[test]
    fn test_parse_mfp_csv_minimal_columns() {
        // Only required + macro columns, no Fiber/Sugar
        let csv = "\
Date,Meal,Food Name,Calories,Fat (g),Protein (g),Carbohydrates (g)
2024-01-15,Lunch,Chicken,165,3.6,31,0
";
        let rows = parse_mfp_csv(csv.as_bytes()).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].fiber.is_none());
        assert!(rows[0].sugar.is_none());
    }

    #[test]
    fn test_parse_mfp_csv_skips_blank_rows() {
        let csv = "\
Date,Meal,Food Name,Calories,Fat (g),Protein (g),Carbohydrates (g)
2024-01-15,Lunch,Chicken,165,3.6,31,0
,,,,,,
2024-01-15,Dinner,Rice,130,0.3,2.7,28
";
        let rows = parse_mfp_csv(csv.as_bytes()).unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn test_normalize_meal_type() {
        assert_eq!(normalize_meal_type("Breakfast"), "breakfast");
        assert_eq!(normalize_meal_type("LUNCH"), "lunch");
        assert_eq!(normalize_meal_type("dinner"), "dinner");
        assert_eq!(normalize_meal_type("Snacks"), "snack");
        assert_eq!(normalize_meal_type("Morning Snack"), "snack");
    }

    #[test]
    fn test_normalize_date_iso() {
        assert_eq!(normalize_date("2024-01-15").unwrap(), "2024-01-15");
    }

    #[test]
    fn test_normalize_date_us_format() {
        assert_eq!(normalize_date("1/15/2024").unwrap(), "2024-01-15");
    }

    #[test]
    fn test_normalize_date_invalid() {
        assert!(normalize_date("not-a-date").is_err());
    }

    #[test]
    fn test_import_mfp_dry_run() {
        let db = Database::open_in_memory().unwrap();
        let rows = parse_mfp_csv(SAMPLE_CSV.as_bytes()).unwrap();

        let summary = import_mfp_meals(&db, &rows, true).unwrap();
        assert_eq!(summary.rows_parsed, 5);
        assert_eq!(summary.foods_created, 5);
        assert_eq!(summary.foods_reused, 0);
        assert_eq!(summary.meals_logged, 5);
        assert_eq!(summary.dates_spanned, 2);

        // Dry run should not have created any foods
        let all_foods = db.list_foods(None).unwrap();
        assert!(all_foods.is_empty());
    }

    #[test]
    fn test_import_mfp_actual() {
        let db = Database::open_in_memory().unwrap();
        let rows = parse_mfp_csv(SAMPLE_CSV.as_bytes()).unwrap();

        let summary = import_mfp_meals(&db, &rows, false).unwrap();
        assert_eq!(summary.rows_parsed, 5);
        assert_eq!(summary.foods_created, 5);
        assert_eq!(summary.foods_reused, 0);
        assert_eq!(summary.meals_logged, 5);
        assert_eq!(summary.dates_spanned, 2);

        // Foods should be in the DB
        let all_foods = db.list_foods(None).unwrap();
        assert_eq!(all_foods.len(), 5);

        // Check source is myfitnesspal
        assert!(all_foods.iter().all(|f| f.source == "myfitnesspal"));
    }

    #[test]
    fn test_import_mfp_deduplication() {
        let db = Database::open_in_memory().unwrap();

        // First import
        let csv1 = "\
Date,Meal,Food Name,Calories,Fat (g),Protein (g),Carbohydrates (g)
2024-01-15,Lunch,Chicken Breast,165,3.6,31,0
";
        let rows1 = parse_mfp_csv(csv1.as_bytes()).unwrap();
        let s1 = import_mfp_meals(&db, &rows1, false).unwrap();
        assert_eq!(s1.foods_created, 1);
        assert_eq!(s1.foods_reused, 0);

        // Second import with same food name
        let csv2 = "\
Date,Meal,Food Name,Calories,Fat (g),Protein (g),Carbohydrates (g)
2024-01-16,Dinner,Chicken Breast,165,3.6,31,0
";
        let rows2 = parse_mfp_csv(csv2.as_bytes()).unwrap();
        let s2 = import_mfp_meals(&db, &rows2, false).unwrap();
        assert_eq!(s2.foods_created, 0);
        assert_eq!(s2.foods_reused, 1);

        // Only one food in DB
        let all_foods = db.list_foods(None).unwrap();
        assert_eq!(all_foods.len(), 1);
    }
}
