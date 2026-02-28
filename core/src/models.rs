use anyhow::{Result, bail};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Food {
    pub id: i64,
    #[serde(default)]
    pub uuid: String,
    pub name: String,
    pub brand: Option<String>,
    pub barcode: Option<String>,
    pub calories_per_100g: f64,
    pub protein_per_100g: Option<f64>,
    pub carbs_per_100g: Option<f64>,
    pub fat_per_100g: Option<f64>,
    pub default_serving_g: Option<f64>,
    pub source: String,
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MealEntry {
    pub id: i64,
    #[serde(default)]
    pub uuid: String,
    pub date: String,
    pub meal_type: String,
    pub food_id: i64,
    pub serving_g: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_quantity: Option<f64>,
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    // Joined fields for display
    #[serde(skip_serializing_if = "Option::is_none")]
    pub food_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub food_brand: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calories: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protein: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub carbs: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fat: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DailySummary {
    pub date: String,
    pub meals: Vec<MealGroup>,
    pub total_calories: f64,
    pub total_protein: f64,
    pub total_carbs: f64,
    pub total_fat: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<DailyTarget>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MealGroup {
    pub meal_type: String,
    pub entries: Vec<MealEntry>,
    pub subtotal_calories: f64,
    pub subtotal_protein: f64,
    pub subtotal_carbs: f64,
    pub subtotal_fat: f64,
}

#[derive(Debug, Clone)]
pub struct NewFood {
    pub name: String,
    pub brand: Option<String>,
    pub barcode: Option<String>,
    pub calories_per_100g: f64,
    pub protein_per_100g: Option<f64>,
    pub carbs_per_100g: Option<f64>,
    pub fat_per_100g: Option<f64>,
    pub default_serving_g: Option<f64>,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct NewMealEntry {
    pub date: NaiveDate,
    pub meal_type: String,
    pub food_id: i64,
    pub serving_g: f64,
    pub display_unit: Option<String>,
    pub display_quantity: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct UpdateMealEntry {
    pub serving_g: Option<f64>,
    pub meal_type: Option<String>,
    pub date: Option<NaiveDate>,
    pub display_unit: Option<Option<String>>,
    pub display_quantity: Option<Option<f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyTarget {
    pub day_of_week: i64,
    pub calories: i64,
    pub protein_pct: Option<i64>,
    pub carbs_pct: Option<i64>,
    pub fat_pct: Option<i64>,
    #[serde(skip_deserializing)]
    pub protein_g: Option<f64>,
    #[serde(skip_deserializing)]
    pub carbs_g: Option<f64>,
    #[serde(skip_deserializing)]
    pub fat_g: Option<f64>,
}

impl DailyTarget {
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn from_db(
        day_of_week: i64,
        calories: i64,
        protein_pct: Option<i64>,
        carbs_pct: Option<i64>,
        fat_pct: Option<i64>,
    ) -> Self {
        let cal = calories as f64;
        let protein_g = protein_pct.map(|p| cal * p as f64 / 100.0 / 4.0);
        let carbs_g = carbs_pct.map(|c| cal * c as f64 / 100.0 / 4.0);
        let fat_g = fat_pct.map(|f| cal * f as f64 / 100.0 / 9.0);
        Self {
            day_of_week,
            calories,
            protein_pct,
            carbs_pct,
            fat_pct,
            protein_g,
            carbs_g,
            fat_g,
        }
    }
}

pub fn validate_macro_split(protein: i64, carbs: i64, fat: i64) -> Result<()> {
    if protein < 0 || carbs < 0 || fat < 0 {
        bail!("Macro percentages must be non-negative");
    }
    if protein > 100 || carbs > 100 || fat > 100 {
        bail!("Each macro percentage must be between 0 and 100");
    }
    let sum = protein + carbs + fat;
    if sum != 100 {
        bail!("Macro percentages must sum to 100 (got {sum})");
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct Recipe {
    pub id: i64,
    #[serde(default)]
    pub uuid: String,
    pub food_id: i64,
    pub portions: f64,
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecipeIngredient {
    pub id: i64,
    #[serde(default)]
    pub uuid: String,
    pub recipe_id: i64,
    pub food_id: i64,
    pub quantity_g: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub food_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub food_brand: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calories: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protein: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub carbs: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fat: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecipeDetail {
    pub id: i64,
    #[serde(default)]
    pub uuid: String,
    pub food_id: i64,
    pub name: String,
    pub portions: f64,
    pub total_weight_g: f64,
    pub per_portion_g: f64,
    pub ingredients: Vec<RecipeIngredient>,
    pub per_portion_calories: f64,
    pub per_portion_protein: f64,
    pub per_portion_carbs: f64,
    pub per_portion_fat: f64,
    pub calories_per_100g: f64,
    pub protein_per_100g: f64,
    pub carbs_per_100g: f64,
    pub fat_per_100g: f64,
}

// --- UX query types ---

#[derive(Debug, Clone, Serialize)]
pub struct RecentFood {
    pub food: Food,
    pub last_serving_g: f64,
    pub last_meal_type: String,
    pub log_count: i64,
    pub last_logged: String,
}

// --- Watch types (Apple Watch / Wear OS) ---

/// Compact glance data for watch face complications and tiles.
#[derive(Debug, Clone, Serialize)]
pub struct WatchGlance {
    pub date: String,
    pub calories_eaten: f64,
    pub calories_target: Option<i64>,
    pub calories_remaining: Option<f64>,
    pub protein_g: f64,
    pub carbs_g: f64,
    pub fat_g: f64,
    pub protein_target_g: Option<f64>,
    pub carbs_target_g: Option<f64>,
    pub fat_target_g: Option<f64>,
    pub meal_count: i64,
    pub logging_streak: i64,
}

/// Compact recent food entry for quick re-logging on watch.
#[derive(Debug, Clone, Serialize)]
pub struct WatchRecentFood {
    pub food_id: i64,
    pub name: String,
    pub brand: Option<String>,
    pub calories_per_100g: f64,
    pub last_serving_g: f64,
    pub last_meal_type: String,
    pub last_calories: f64,
}

// --- Weight tracking types ---

#[derive(Debug, Clone, Serialize)]
pub struct WeightEntry {
    pub id: i64,
    pub uuid: String,
    pub date: NaiveDate,
    pub weight_kg: f64,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct NewWeightEntry {
    pub date: NaiveDate,
    pub weight_kg: f64,
    pub source: String,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportWeightEntry {
    pub uuid: String,
    pub date: String,
    pub weight_kg: f64,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub notes: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

// --- Export / Import types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportMealEntry {
    pub id: i64,
    #[serde(default)]
    pub uuid: String,
    pub date: String,
    pub meal_type: String,
    pub food_id: i64,
    #[serde(default)]
    pub food_uuid: String,
    pub serving_g: f64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub display_unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub display_quantity: Option<f64>,
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportRecipe {
    pub id: i64,
    #[serde(default)]
    pub uuid: String,
    pub food_id: i64,
    #[serde(default)]
    pub food_uuid: String,
    pub portions: f64,
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportRecipeIngredient {
    pub id: i64,
    #[serde(default)]
    pub uuid: String,
    pub recipe_id: i64,
    #[serde(default)]
    pub recipe_uuid: String,
    pub food_id: i64,
    #[serde(default)]
    pub food_uuid: String,
    pub quantity_g: f64,
}

/// Legacy export target without `day_of_week` (for backward compatibility with old exports).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyExportTarget {
    pub calories: i64,
    pub protein_pct: Option<i64>,
    pub carbs_pct: Option<i64>,
    pub fat_pct: Option<i64>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportTarget {
    pub day_of_week: i64,
    pub calories: i64,
    pub protein_pct: Option<i64>,
    pub carbs_pct: Option<i64>,
    pub fat_pct: Option<i64>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportData {
    pub version: i64,
    pub exported_at: String,
    #[serde(default)]
    pub device_id: Option<String>,
    pub foods: Vec<Food>,
    pub meal_entries: Vec<ExportMealEntry>,
    pub recipes: Vec<ExportRecipe>,
    pub recipe_ingredients: Vec<ExportRecipeIngredient>,
    #[serde(default, skip_serializing)]
    pub target: Option<LegacyExportTarget>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<ExportTarget>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weight_entries: Vec<ExportWeightEntry>,
    #[serde(default)]
    pub tombstones: Option<Vec<SyncTombstone>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_field_names)]
pub struct ImportSummary {
    pub foods_imported: i64,
    pub meal_entries_imported: i64,
    pub recipes_imported: i64,
    pub recipe_ingredients_imported: i64,
    pub targets_imported: i64,
    pub weight_entries_imported: i64,
    pub tombstones_processed: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTombstone {
    pub uuid: String,
    pub table_name: String,
    pub deleted_at: String,
}

// --- Delta sync types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPayload {
    pub foods: Vec<Food>,
    pub meal_entries: Vec<ExportMealEntry>,
    pub recipes: Vec<ExportRecipe>,
    pub recipe_ingredients: Vec<ExportRecipeIngredient>,
    pub targets: Vec<ExportTarget>,
    pub weight_entries: Vec<ExportWeightEntry>,
    pub tombstones: Vec<SyncTombstone>,
    pub server_timestamp: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SyncPushRequest {
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub foods: Vec<Food>,
    #[serde(default)]
    pub meal_entries: Vec<ExportMealEntry>,
    #[serde(default)]
    pub recipes: Vec<ExportRecipe>,
    #[serde(default)]
    pub recipe_ingredients: Vec<ExportRecipeIngredient>,
    #[serde(default)]
    pub targets: Vec<ExportTarget>,
    #[serde(default)]
    pub weight_entries: Vec<ExportWeightEntry>,
    #[serde(default)]
    pub tombstones: Vec<SyncTombstone>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CooklangIngredient {
    pub name: String,
    pub quantity: Option<serde_json::Value>,
    pub units: Option<String>,
}

/// Convert a quantity with a unit to grams.
/// Volume-based conversions assume water density (1 ml = 1 g).
/// Returns `(grams, is_approximate)` where `is_approximate` is true for volume conversions.
#[must_use]
pub fn convert_to_grams(quantity: f64, unit: &str) -> Option<(f64, bool)> {
    let lower = unit.to_lowercase();
    match lower.as_str() {
        "g" | "gram" | "grams" => Some((quantity, false)),
        "kg" | "kilogram" | "kilograms" => Some((quantity * 1000.0, false)),
        "lb" | "lbs" | "pound" | "pounds" => Some((quantity * 454.0, false)),
        "oz" | "ounce" | "ounces" => Some((quantity * 28.35, false)),
        "tbsp" | "tablespoon" | "tablespoons" => Some((quantity * 15.0, true)),
        "tsp" | "teaspoon" | "teaspoons" => Some((quantity * 5.0, true)),
        "ml" | "milliliter" | "milliliters" | "millilitre" | "millilitres" => {
            Some((quantity, true))
        }
        "l" | "liter" | "liters" | "litre" | "litres" => Some((quantity * 1000.0, true)),
        _ => None,
    }
}

pub const MEAL_TYPES: &[&str] = &["breakfast", "lunch", "dinner", "snack"];

/// Valid table names for sync tombstones.
pub const VALID_TOMBSTONE_TABLES: &[&str] =
    &["foods", "meal_entries", "recipes", "recipe_ingredients"];

pub fn validate_meal_type(meal: &str) -> anyhow::Result<String> {
    let lower = meal.to_lowercase();
    if MEAL_TYPES.contains(&lower.as_str()) {
        Ok(lower)
    } else {
        anyhow::bail!(
            "Invalid meal type '{meal}'. Must be one of: {}",
            MEAL_TYPES.join(", ")
        )
    }
}

/// Validate a sync tombstone: `table_name` must be in the allowed list,
/// `deleted_at` must be valid RFC 3339, and future timestamps are capped to now.
pub fn validate_tombstone(tombstone: &mut SyncTombstone) -> anyhow::Result<()> {
    if !VALID_TOMBSTONE_TABLES.contains(&tombstone.table_name.as_str()) {
        anyhow::bail!(
            "Invalid tombstone table_name '{}'. Must be one of: {}",
            tombstone.table_name,
            VALID_TOMBSTONE_TABLES.join(", ")
        );
    }
    // Parse and validate timestamp, cap future-dated to now
    let ts = chrono::DateTime::parse_from_rfc3339(&tombstone.deleted_at).map_err(|_| {
        anyhow::anyhow!(
            "Invalid tombstone deleted_at '{}'. Must be RFC 3339 format",
            tombstone.deleted_at
        )
    })?;
    let now = chrono::Utc::now();
    if ts > now {
        tombstone.deleted_at = now.to_rfc3339();
    }
    Ok(())
}

/// Validate imported food data: name must not be empty, calories must not be negative.
pub fn validate_food_data(food: &Food) -> anyhow::Result<()> {
    if food.name.trim().is_empty() {
        anyhow::bail!("Food name must not be empty");
    }
    if food.calories_per_100g < 0.0 {
        anyhow::bail!("calories_per_100g must not be negative");
    }
    if food.protein_per_100g.is_some_and(|v| v < 0.0) {
        anyhow::bail!("protein_per_100g must not be negative");
    }
    if food.carbs_per_100g.is_some_and(|v| v < 0.0) {
        anyhow::bail!("carbs_per_100g must not be negative");
    }
    if food.fat_per_100g.is_some_and(|v| v < 0.0) {
        anyhow::bail!("fat_per_100g must not be negative");
    }
    Ok(())
}

/// Validate an imported meal entry: `meal_type` and `serving_g`.
pub fn validate_meal_entry_data(meal_type: &str, serving_g: f64) -> anyhow::Result<()> {
    validate_meal_type(meal_type)?;
    if serving_g <= 0.0 {
        anyhow::bail!("serving_g must be greater than 0");
    }
    Ok(())
}

/// Validate an exported/synced meal entry: `meal_type`, `serving_g`, and date format.
pub fn validate_export_meal_entry(entry: &ExportMealEntry) -> anyhow::Result<()> {
    validate_meal_entry_data(&entry.meal_type, entry.serving_g)?;
    NaiveDate::parse_from_str(&entry.date, "%Y-%m-%d").map_err(|_| {
        anyhow::anyhow!(
            "Invalid meal entry date '{}'. Must be YYYY-MM-DD",
            entry.date
        )
    })?;
    Ok(())
}

/// Validate an exported/synced recipe: portions must be positive.
pub fn validate_export_recipe(recipe: &ExportRecipe) -> anyhow::Result<()> {
    if recipe.portions <= 0.0 {
        anyhow::bail!("Recipe portions must be greater than 0");
    }
    Ok(())
}

/// Validate an exported/synced recipe ingredient: quantity must be positive.
pub fn validate_export_recipe_ingredient(
    ingredient: &ExportRecipeIngredient,
) -> anyhow::Result<()> {
    if ingredient.quantity_g <= 0.0 {
        anyhow::bail!("Recipe ingredient quantity_g must be greater than 0");
    }
    Ok(())
}

/// Validate an exported/synced target: day 0-6, calories > 0, macro split if present.
pub fn validate_export_target(target: &ExportTarget) -> anyhow::Result<()> {
    if !(0..=6).contains(&target.day_of_week) {
        anyhow::bail!("Target day_of_week must be between 0 (Monday) and 6 (Sunday)");
    }
    if target.calories <= 0 {
        anyhow::bail!("Target calories must be greater than 0");
    }
    match (target.protein_pct, target.carbs_pct, target.fat_pct) {
        (None, None, None) => {}
        (Some(p), Some(c), Some(f)) => {
            validate_macro_split(p, c, f)?;
        }
        _ => {
            anyhow::bail!(
                "If setting macro percentages, all three (protein_pct, carbs_pct, fat_pct) must be provided"
            );
        }
    }
    Ok(())
}

/// Validate an exported/synced weight entry: weight > 0, valid date.
pub fn validate_export_weight_entry(entry: &ExportWeightEntry) -> anyhow::Result<()> {
    if entry.weight_kg <= 0.0 {
        anyhow::bail!("weight_kg must be greater than 0");
    }
    NaiveDate::parse_from_str(&entry.date, "%Y-%m-%d").map_err(|_| {
        anyhow::anyhow!(
            "Invalid weight entry date '{}'. Must be YYYY-MM-DD",
            entry.date
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_meal_types() {
        assert_eq!(validate_meal_type("breakfast").unwrap(), "breakfast");
        assert_eq!(validate_meal_type("lunch").unwrap(), "lunch");
        assert_eq!(validate_meal_type("dinner").unwrap(), "dinner");
        assert_eq!(validate_meal_type("snack").unwrap(), "snack");
    }

    #[test]
    fn test_invalid_meal_type() {
        assert!(validate_meal_type("brunch").is_err());
        assert!(validate_meal_type("").is_err());
    }

    #[test]
    fn test_meal_type_case_insensitive() {
        assert_eq!(validate_meal_type("Lunch").unwrap(), "lunch");
        assert_eq!(validate_meal_type("BREAKFAST").unwrap(), "breakfast");
        assert_eq!(validate_meal_type("Dinner").unwrap(), "dinner");
    }

    #[test]
    fn test_daily_target_from_db_with_macros() {
        let target = DailyTarget::from_db(0, 1800, Some(40), Some(30), Some(30));
        assert_eq!(target.day_of_week, 0);
        assert_eq!(target.calories, 1800);
        assert_eq!(target.protein_pct, Some(40));
        assert_eq!(target.carbs_pct, Some(30));
        assert_eq!(target.fat_pct, Some(30));
        // 1800 * 40% / 4 = 180g protein
        assert!((target.protein_g.unwrap() - 180.0).abs() < 0.01);
        // 1800 * 30% / 4 = 135g carbs
        assert!((target.carbs_g.unwrap() - 135.0).abs() < 0.01);
        // 1800 * 30% / 9 = 60g fat
        assert!((target.fat_g.unwrap() - 60.0).abs() < 0.01);
    }

    #[test]
    fn test_daily_target_from_db_calories_only() {
        let target = DailyTarget::from_db(3, 2000, None, None, None);
        assert_eq!(target.day_of_week, 3);
        assert_eq!(target.calories, 2000);
        assert!(target.protein_g.is_none());
        assert!(target.carbs_g.is_none());
        assert!(target.fat_g.is_none());
    }

    #[test]
    fn test_validate_macro_split_valid() {
        assert!(validate_macro_split(40, 30, 30).is_ok());
        assert!(validate_macro_split(33, 34, 33).is_ok());
        assert!(validate_macro_split(100, 0, 0).is_ok());
    }

    #[test]
    fn test_validate_macro_split_invalid_sum() {
        assert!(validate_macro_split(40, 30, 20).is_err());
        assert!(validate_macro_split(50, 50, 50).is_err());
    }

    #[test]
    fn test_validate_macro_split_negative() {
        assert!(validate_macro_split(-10, 60, 50).is_err());
    }

    #[test]
    fn test_convert_to_grams_weight_units() {
        let (g, approx) = convert_to_grams(1.0, "g").unwrap();
        assert!((g - 1.0).abs() < f64::EPSILON);
        assert!(!approx);

        let (g, approx) = convert_to_grams(2.0, "kg").unwrap();
        assert!((g - 2000.0).abs() < f64::EPSILON);
        assert!(!approx);

        let (g, _) = convert_to_grams(1.0, "lb").unwrap();
        assert!((g - 454.0).abs() < f64::EPSILON);

        let (g, _) = convert_to_grams(1.0, "oz").unwrap();
        assert!((g - 28.35).abs() < f64::EPSILON);
    }

    #[test]
    fn test_convert_to_grams_volume_units() {
        let (g, approx) = convert_to_grams(1.0, "tbsp").unwrap();
        assert!((g - 15.0).abs() < f64::EPSILON);
        assert!(approx);

        let (g, approx) = convert_to_grams(1.0, "tsp").unwrap();
        assert!((g - 5.0).abs() < f64::EPSILON);
        assert!(approx);

        let (g, approx) = convert_to_grams(500.0, "ml").unwrap();
        assert!((g - 500.0).abs() < f64::EPSILON);
        assert!(approx);

        let (g, _) = convert_to_grams(1.0, "l").unwrap();
        assert!((g - 1000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_convert_to_grams_cups_not_supported() {
        assert!(convert_to_grams(1.0, "cup").is_none());
        assert!(convert_to_grams(1.0, "cups").is_none());
    }

    #[test]
    fn test_convert_to_grams_case_insensitive() {
        assert!(convert_to_grams(1.0, "G").is_some());
        assert!(convert_to_grams(1.0, "Kg").is_some());
        assert!(convert_to_grams(1.0, "TBSP").is_some());
    }

    #[test]
    fn test_convert_to_grams_unknown_unit() {
        assert!(convert_to_grams(1.0, "piece").is_none());
        assert!(convert_to_grams(1.0, "").is_none());
    }

    #[test]
    fn test_validate_tombstone_valid_tables() {
        for table in VALID_TOMBSTONE_TABLES {
            let mut t = SyncTombstone {
                uuid: "test-uuid".to_string(),
                table_name: table.to_string(),
                deleted_at: "2024-01-01T00:00:00Z".to_string(),
            };
            assert!(validate_tombstone(&mut t).is_ok());
        }
    }

    #[test]
    fn test_validate_tombstone_invalid_table() {
        let mut t = SyncTombstone {
            uuid: "test-uuid".to_string(),
            table_name: "users".to_string(),
            deleted_at: "2024-01-01T00:00:00Z".to_string(),
        };
        assert!(validate_tombstone(&mut t).is_err());
    }

    #[test]
    fn test_validate_tombstone_caps_future_timestamp() {
        let mut t = SyncTombstone {
            uuid: "test-uuid".to_string(),
            table_name: "foods".to_string(),
            deleted_at: "2099-01-01T00:00:00Z".to_string(),
        };
        validate_tombstone(&mut t).unwrap();
        // Should be capped to approximately now, not 2099
        assert!(t.deleted_at < "2099-01-01T00:00:00Z".to_string());
    }

    #[test]
    fn test_validate_food_data_valid() {
        let food = Food {
            id: 1,
            uuid: "test".to_string(),
            name: "Chicken".to_string(),
            brand: None,
            barcode: None,
            calories_per_100g: 165.0,
            protein_per_100g: Some(31.0),
            carbs_per_100g: Some(0.0),
            fat_per_100g: Some(3.6),
            default_serving_g: Some(100.0),
            source: "manual".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(validate_food_data(&food).is_ok());
    }

    #[test]
    fn test_validate_food_data_empty_name() {
        let food = Food {
            id: 1,
            uuid: "test".to_string(),
            name: "  ".to_string(),
            brand: None,
            barcode: None,
            calories_per_100g: 100.0,
            protein_per_100g: None,
            carbs_per_100g: None,
            fat_per_100g: None,
            default_serving_g: None,
            source: "manual".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(validate_food_data(&food).is_err());
    }

    #[test]
    fn test_validate_food_data_negative_calories() {
        let food = Food {
            id: 1,
            uuid: "test".to_string(),
            name: "Bad Food".to_string(),
            brand: None,
            barcode: None,
            calories_per_100g: -50.0,
            protein_per_100g: None,
            carbs_per_100g: None,
            fat_per_100g: None,
            default_serving_g: None,
            source: "manual".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(validate_food_data(&food).is_err());
    }

    #[test]
    fn test_validate_meal_entry_data_valid() {
        assert!(validate_meal_entry_data("lunch", 200.0).is_ok());
    }

    #[test]
    fn test_validate_meal_entry_data_invalid_type() {
        assert!(validate_meal_entry_data("brunch", 200.0).is_err());
    }

    #[test]
    fn test_validate_meal_entry_data_zero_serving() {
        assert!(validate_meal_entry_data("lunch", 0.0).is_err());
    }

    #[test]
    fn test_validate_meal_entry_data_negative_serving() {
        assert!(validate_meal_entry_data("lunch", -100.0).is_err());
    }

    #[test]
    fn test_validate_tombstone_rejects_malformed_timestamp() {
        let mut t = SyncTombstone {
            uuid: "test-uuid".to_string(),
            table_name: "foods".to_string(),
            deleted_at: "not-a-date".to_string(),
        };
        assert!(validate_tombstone(&mut t).is_err());
    }

    #[test]
    fn test_validate_export_meal_entry_valid() {
        let entry = ExportMealEntry {
            id: 1,
            uuid: "test".to_string(),
            date: "2024-06-15".to_string(),
            meal_type: "lunch".to_string(),
            food_id: 1,
            food_uuid: String::new(),
            serving_g: 200.0,
            display_unit: None,
            display_quantity: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(validate_export_meal_entry(&entry).is_ok());
    }

    #[test]
    fn test_validate_export_meal_entry_invalid_date() {
        let entry = ExportMealEntry {
            id: 1,
            uuid: "test".to_string(),
            date: "not-a-date".to_string(),
            meal_type: "lunch".to_string(),
            food_id: 1,
            food_uuid: String::new(),
            serving_g: 200.0,
            display_unit: None,
            display_quantity: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(validate_export_meal_entry(&entry).is_err());
    }

    #[test]
    fn test_validate_export_recipe_valid() {
        let recipe = ExportRecipe {
            id: 1,
            uuid: "test".to_string(),
            food_id: 1,
            food_uuid: String::new(),
            portions: 4.0,
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(validate_export_recipe(&recipe).is_ok());
    }

    #[test]
    fn test_validate_export_recipe_zero_portions() {
        let recipe = ExportRecipe {
            id: 1,
            uuid: "test".to_string(),
            food_id: 1,
            food_uuid: String::new(),
            portions: 0.0,
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(validate_export_recipe(&recipe).is_err());
    }

    #[test]
    fn test_validate_export_recipe_negative_portions() {
        let recipe = ExportRecipe {
            id: 1,
            uuid: "test".to_string(),
            food_id: 1,
            food_uuid: String::new(),
            portions: -1.0,
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(validate_export_recipe(&recipe).is_err());
    }

    #[test]
    fn test_validate_export_recipe_ingredient_valid() {
        let ing = ExportRecipeIngredient {
            id: 1,
            uuid: "test".to_string(),
            recipe_id: 1,
            recipe_uuid: String::new(),
            food_id: 1,
            food_uuid: String::new(),
            quantity_g: 100.0,
        };
        assert!(validate_export_recipe_ingredient(&ing).is_ok());
    }

    #[test]
    fn test_validate_export_recipe_ingredient_zero() {
        let ing = ExportRecipeIngredient {
            id: 1,
            uuid: "test".to_string(),
            recipe_id: 1,
            recipe_uuid: String::new(),
            food_id: 1,
            food_uuid: String::new(),
            quantity_g: 0.0,
        };
        assert!(validate_export_recipe_ingredient(&ing).is_err());
    }

    #[test]
    fn test_validate_export_target_valid() {
        let target = ExportTarget {
            day_of_week: 0,
            calories: 2000,
            protein_pct: Some(30),
            carbs_pct: Some(40),
            fat_pct: Some(30),
            updated_at: None,
        };
        assert!(validate_export_target(&target).is_ok());
    }

    #[test]
    fn test_validate_export_target_calories_only() {
        let target = ExportTarget {
            day_of_week: 3,
            calories: 1800,
            protein_pct: None,
            carbs_pct: None,
            fat_pct: None,
            updated_at: None,
        };
        assert!(validate_export_target(&target).is_ok());
    }

    #[test]
    fn test_validate_export_target_invalid_day() {
        let target = ExportTarget {
            day_of_week: 7,
            calories: 2000,
            protein_pct: None,
            carbs_pct: None,
            fat_pct: None,
            updated_at: None,
        };
        assert!(validate_export_target(&target).is_err());
    }

    #[test]
    fn test_validate_export_target_zero_calories() {
        let target = ExportTarget {
            day_of_week: 0,
            calories: 0,
            protein_pct: None,
            carbs_pct: None,
            fat_pct: None,
            updated_at: None,
        };
        assert!(validate_export_target(&target).is_err());
    }

    #[test]
    fn test_validate_export_target_partial_macros() {
        let target = ExportTarget {
            day_of_week: 0,
            calories: 2000,
            protein_pct: Some(30),
            carbs_pct: None,
            fat_pct: Some(30),
            updated_at: None,
        };
        assert!(validate_export_target(&target).is_err());
    }

    #[test]
    fn test_validate_export_target_macros_not_100() {
        let target = ExportTarget {
            day_of_week: 0,
            calories: 2000,
            protein_pct: Some(30),
            carbs_pct: Some(30),
            fat_pct: Some(30),
            updated_at: None,
        };
        assert!(validate_export_target(&target).is_err());
    }

    #[test]
    fn test_validate_export_weight_entry_valid() {
        let entry = ExportWeightEntry {
            uuid: "test".to_string(),
            date: "2024-06-15".to_string(),
            weight_kg: 75.0,
            source: "manual".to_string(),
            notes: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(validate_export_weight_entry(&entry).is_ok());
    }

    #[test]
    fn test_validate_export_weight_entry_zero_weight() {
        let entry = ExportWeightEntry {
            uuid: "test".to_string(),
            date: "2024-06-15".to_string(),
            weight_kg: 0.0,
            source: "manual".to_string(),
            notes: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(validate_export_weight_entry(&entry).is_err());
    }

    #[test]
    fn test_validate_export_weight_entry_invalid_date() {
        let entry = ExportWeightEntry {
            uuid: "test".to_string(),
            date: "bad-date".to_string(),
            weight_kg: 75.0,
            source: "manual".to_string(),
            notes: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(validate_export_weight_entry(&entry).is_err());
    }
}
