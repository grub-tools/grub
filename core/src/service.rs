use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;
use chrono::NaiveDate;

use crate::db::Database;
use crate::mfp_import::{self, MfpImportSummary};
use crate::models::{
    DailySummary, DailyTarget, ExportData, Food, ImportSummary, MealEntry, NewFood, NewMealEntry,
    NewWeightEntry, RecentFood, Recipe, RecipeDetail, RecipeIngredient, SyncPayload,
    SyncPushRequest, UpdateMealEntry, WatchGlance, WatchRecentFood, WeightEntry,
};

/// Platform-native food lookup provider.
///
/// iOS implements this with `URLSession`, Android with Ktor, CLI with reqwest.
/// Called synchronously from Rust — mobile callers should invoke `GrubService`
/// methods from a background thread.
pub trait FoodLookupProvider: Send + Sync {
    fn search(&self, query: &str) -> Result<Vec<NewFood>>;
    fn lookup_barcode(&self, barcode: &str) -> Result<Option<NewFood>>;
}

pub struct GrubService {
    db: Database,
}

impl GrubService {
    pub fn new(db_path: &str) -> Result<Self> {
        let db = Database::open(Path::new(db_path))?;
        Ok(Self { db })
    }

    pub fn new_in_memory() -> Result<Self> {
        let db = Database::open_in_memory()?;
        Ok(Self { db })
    }

    // --- Direct DB operations ---

    pub fn get_daily_summary(&self, date: &str) -> Result<DailySummary> {
        let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")?;
        self.db.build_daily_summary(date)
    }

    pub fn log_meal(
        &self,
        date: &str,
        meal_type: &str,
        food_id: i64,
        serving_g: f64,
    ) -> Result<MealEntry> {
        self.log_meal_with_display(date, meal_type, food_id, serving_g, None, None)
    }

    pub fn log_meal_with_display(
        &self,
        date: &str,
        meal_type: &str,
        food_id: i64,
        serving_g: f64,
        display_unit: Option<String>,
        display_quantity: Option<f64>,
    ) -> Result<MealEntry> {
        let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")?;
        let meal_type = crate::models::validate_meal_type(meal_type)?;
        self.db.insert_meal_entry(&NewMealEntry {
            date,
            meal_type,
            food_id,
            serving_g,
            display_unit,
            display_quantity,
        })
    }

    pub fn delete_meal(&self, id: i64) -> Result<bool> {
        // Record tombstone before deleting
        if let Ok(Some(uuid)) = self.db.get_meal_entry_uuid(id) {
            self.db.record_tombstone(&uuid, "meal_entries")?;
        }
        self.db.delete_meal_entry(id)
    }

    pub fn update_meal(&self, id: i64, update: &UpdateMealEntry) -> Result<MealEntry> {
        self.db.update_meal_entry(id, update)
    }

    pub fn get_meal_entry(&self, id: i64) -> Result<MealEntry> {
        self.db.get_meal_entry(id)
    }

    pub fn get_food_by_id(&self, id: i64) -> Result<Food> {
        self.db.get_food_by_id(id)
    }

    pub fn get_food_by_barcode(&self, barcode: &str) -> Result<Option<Food>> {
        self.db.get_food_by_barcode(barcode)
    }

    pub fn search_foods_local(&self, query: &str) -> Result<Vec<Food>> {
        self.db.search_foods_local(query)
    }

    pub fn list_foods(&self, search: Option<&str>) -> Result<Vec<Food>> {
        self.db.list_foods(search)
    }

    pub fn insert_food(&self, food: &NewFood) -> Result<Food> {
        self.db.insert_food(food)
    }

    pub fn upsert_food_by_barcode(&self, food: &NewFood) -> Result<Food> {
        self.db.upsert_food_by_barcode(food)
    }

    // --- Targets ---

    pub fn set_target(
        &self,
        day_of_week: i64,
        calories: i64,
        protein_pct: Option<i64>,
        carbs_pct: Option<i64>,
        fat_pct: Option<i64>,
    ) -> Result<DailyTarget> {
        self.db
            .set_target(day_of_week, calories, protein_pct, carbs_pct, fat_pct)
    }

    pub fn get_target(&self, day_of_week: i64) -> Result<Option<DailyTarget>> {
        self.db.get_target(day_of_week)
    }

    pub fn get_all_targets(&self) -> Result<Vec<DailyTarget>> {
        self.db.get_all_targets()
    }

    pub fn clear_target(&self, day_of_week: i64) -> Result<bool> {
        self.db.clear_target(day_of_week)
    }

    pub fn clear_all_targets(&self) -> Result<bool> {
        self.db.clear_all_targets()
    }

    // --- Recipes ---

    pub fn create_recipe(&self, name: &str, portions: f64) -> Result<Recipe> {
        self.db.create_recipe(name, portions)
    }

    pub fn get_recipe_detail(&self, recipe_id: i64) -> Result<RecipeDetail> {
        self.db.get_recipe_detail(recipe_id)
    }

    pub fn get_recipe_by_food_name(&self, name: &str) -> Result<Recipe> {
        self.db.get_recipe_by_food_name(name)
    }

    pub fn add_recipe_ingredient(
        &self,
        recipe_id: i64,
        food_id: i64,
        quantity_g: f64,
    ) -> Result<RecipeIngredient> {
        self.db
            .add_recipe_ingredient(recipe_id, food_id, quantity_g)
    }

    pub fn remove_recipe_ingredient(&self, recipe_id: i64, food_name: &str) -> Result<bool> {
        self.db.remove_recipe_ingredient(recipe_id, food_name)
    }

    pub fn set_recipe_portions(&self, recipe_id: i64, portions: f64) -> Result<()> {
        self.db.set_recipe_portions(recipe_id, portions)
    }

    pub fn list_recipes(&self) -> Result<Vec<RecipeDetail>> {
        self.db.list_recipes()
    }

    pub fn delete_recipe(&self, recipe_id: i64) -> Result<()> {
        // Record tombstones for recipe, its ingredients, and its virtual food
        if let Ok(Some(recipe_uuid)) = self.db.get_recipe_uuid(recipe_id) {
            self.db.record_tombstone(&recipe_uuid, "recipes")?;
        }
        if let Ok(ingredient_uuids) = self.db.get_recipe_ingredient_uuids(recipe_id) {
            for uuid in ingredient_uuids {
                self.db.record_tombstone(&uuid, "recipe_ingredients")?;
            }
        }
        if let Ok(recipe) = self.db.get_recipe_by_id(recipe_id) {
            let food = self.db.get_food_by_id(recipe.food_id)?;
            self.db.record_tombstone(&food.uuid, "foods")?;
        }
        self.db.delete_recipe(recipe_id)
    }

    // --- Weight ---

    pub fn log_weight(&self, entry: &NewWeightEntry) -> Result<WeightEntry> {
        self.db.upsert_weight(entry)
    }

    pub fn get_weight(&self, date: NaiveDate) -> Result<Option<WeightEntry>> {
        self.db.get_weight(date)
    }

    pub fn get_weight_history(&self, days: Option<i64>) -> Result<Vec<WeightEntry>> {
        self.db.get_weight_history(days)
    }

    pub fn delete_weight(&self, id: i64) -> Result<()> {
        self.db.delete_weight(id)
    }

    // --- UX queries ---

    pub fn get_recently_logged_foods(&self, limit: i64) -> Result<Vec<RecentFood>> {
        self.db.get_recently_logged_foods(limit)
    }

    pub fn get_logging_streak(&self) -> Result<i64> {
        let today = chrono::Local::now().date_naive();
        self.db.get_logging_streak(today)
    }

    pub fn get_calorie_average(&self, days: i64) -> Result<f64> {
        self.db.get_calorie_average(days)
    }

    // --- Watch (Apple Watch / Wear OS) ---

    pub fn get_watch_glance(&self, date: &str) -> Result<WatchGlance> {
        let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")?;
        self.db.build_watch_glance(date)
    }

    pub fn get_watch_recent_foods(&self, limit: i64) -> Result<Vec<WatchRecentFood>> {
        self.db.get_watch_recent_foods(limit)
    }

    // --- Goal weight ---

    pub fn set_goal_weight(&self, kg: f64) -> Result<()> {
        self.db.set_setting("goal_weight_kg", &kg.to_string())
    }

    pub fn get_goal_weight(&self) -> Result<Option<f64>> {
        match self.db.get_setting("goal_weight_kg")? {
            Some(v) => Ok(Some(v.parse::<f64>()?)),
            None => Ok(None),
        }
    }

    pub fn clear_goal_weight(&self) -> Result<bool> {
        self.db.delete_setting("goal_weight_kg")
    }

    // --- Orchestrated lookups (search local, call provider if needed, cache results) ---

    /// Search local DB first, then call the provider for remote results, cache them, and
    /// return a deduplicated list.
    pub fn search_and_cache(
        &self,
        provider: &dyn FoodLookupProvider,
        query: &str,
    ) -> Result<Vec<Food>> {
        let local = self.db.search_foods_local(query)?;
        let remote = provider.search(query)?;

        let mut cached_remote: Vec<Food> = Vec::new();
        for food in &remote {
            if let Ok(f) = self.db.upsert_food_by_barcode(food) {
                cached_remote.push(f);
            } else {
                let mut no_barcode = food.clone();
                no_barcode.barcode = None;
                if let Ok(f) = self.db.insert_food(&no_barcode) {
                    cached_remote.push(f);
                }
            }
        }

        let mut all = local;
        let seen: HashSet<i64> = all.iter().map(|f| f.id).collect();
        for f in cached_remote {
            if !seen.contains(&f.id) {
                all.push(f);
            }
        }

        Ok(all)
    }

    /// Look up a barcode: check local cache first, then call the provider, cache and return.
    pub fn barcode_lookup(
        &self,
        provider: &dyn FoodLookupProvider,
        code: &str,
    ) -> Result<Option<Food>> {
        if let Some(cached) = self.db.get_food_by_barcode(code)? {
            return Ok(Some(cached));
        }

        let remote = provider.lookup_barcode(code)?;
        match remote {
            Some(new_food) => {
                let food = self.db.upsert_food_by_barcode(&new_food)?;
                Ok(Some(food))
            }
            None => Ok(None),
        }
    }

    // --- Sync ---

    pub fn get_device_id(&self) -> Result<String> {
        self.db.get_or_create_device_id()
    }

    pub fn clear_tombstones(&self) -> Result<()> {
        self.db.clear_tombstones()
    }

    // --- Delta sync ---

    pub fn changes_since(&self, since: Option<&str>) -> Result<SyncPayload> {
        let server_timestamp = chrono::Utc::now().to_rfc3339();
        self.db.changes_since(since, &server_timestamp)
    }

    pub fn apply_remote_changes(&self, request: &SyncPushRequest) -> Result<SyncPayload> {
        let server_timestamp = chrono::Utc::now().to_rfc3339();
        // Get server's changes BEFORE applying client changes (avoids echoing)
        let delta = self
            .db
            .changes_since(request.since.as_deref(), &server_timestamp)?;
        // Apply client changes with LWW
        self.db.apply_remote_changes(
            &request.foods,
            &request.meal_entries,
            &request.recipes,
            &request.recipe_ingredients,
            &request.targets,
            &request.weight_entries,
            &request.tombstones,
        )?;
        Ok(delta)
    }

    // --- MFP import ---

    pub fn import_mfp_csv(&self, csv_data: &str, dry_run: bool) -> Result<MfpImportSummary> {
        let rows = mfp_import::parse_mfp_csv(csv_data.as_bytes())?;
        mfp_import::import_mfp_meals(&self.db, &rows, dry_run)
    }

    // --- Export / Import ---

    pub fn export_all(&self) -> Result<ExportData> {
        self.db.export_all()
    }

    pub fn import_all(&self, data: &ExportData) -> Result<ImportSummary> {
        self.db.import_all(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockProvider {
        foods: Vec<NewFood>,
    }

    impl FoodLookupProvider for MockProvider {
        fn search(&self, _query: &str) -> Result<Vec<NewFood>> {
            Ok(self.foods.clone())
        }

        fn lookup_barcode(&self, barcode: &str) -> Result<Option<NewFood>> {
            Ok(self
                .foods
                .iter()
                .find(|f| f.barcode.as_deref() == Some(barcode))
                .cloned())
        }
    }

    fn sample_food() -> NewFood {
        NewFood {
            name: "Test Food".to_string(),
            brand: Some("Brand".to_string()),
            barcode: Some("1234567890".to_string()),
            calories_per_100g: 100.0,
            protein_per_100g: Some(10.0),
            carbs_per_100g: Some(20.0),
            fat_per_100g: Some(5.0),
            default_serving_g: Some(100.0),
            source: "openfoodfacts".to_string(),
        }
    }

    #[test]
    fn test_search_and_cache() {
        let svc = GrubService::new_in_memory().unwrap();
        let provider = MockProvider {
            foods: vec![sample_food()],
        };

        let results = svc.search_and_cache(&provider, "test").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Test Food");

        // Second search should return cached result without hitting provider
        let empty_provider = MockProvider { foods: vec![] };
        let results = svc.search_and_cache(&empty_provider, "test").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Test Food");
    }

    #[test]
    fn test_barcode_lookup_cache() {
        let svc = GrubService::new_in_memory().unwrap();
        let provider = MockProvider {
            foods: vec![sample_food()],
        };

        let food = svc
            .barcode_lookup(&provider, "1234567890")
            .unwrap()
            .unwrap();
        assert_eq!(food.name, "Test Food");

        // Should be cached now
        let empty_provider = MockProvider { foods: vec![] };
        let cached = svc
            .barcode_lookup(&empty_provider, "1234567890")
            .unwrap()
            .unwrap();
        assert_eq!(cached.id, food.id);
    }

    #[test]
    fn test_barcode_lookup_not_found() {
        let svc = GrubService::new_in_memory().unwrap();
        let provider = MockProvider { foods: vec![] };

        let result = svc.barcode_lookup(&provider, "0000000000").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_log_meal_and_summary() {
        let svc = GrubService::new_in_memory().unwrap();
        let food = svc.insert_food(&sample_food()).unwrap();

        let entry = svc.log_meal("2024-06-15", "lunch", food.id, 200.0).unwrap();
        assert_eq!(entry.meal_type, "lunch");
        assert_eq!(entry.serving_g, 200.0);

        let summary = svc.get_daily_summary("2024-06-15").unwrap();
        assert_eq!(summary.meals.len(), 1);
        assert!((summary.total_calories - 200.0).abs() < 0.01);
    }

    #[test]
    fn test_goal_weight_set_get_clear() {
        let svc = GrubService::new_in_memory().unwrap();

        // Initially none
        assert!(svc.get_goal_weight().unwrap().is_none());

        // Set
        svc.set_goal_weight(75.0).unwrap();
        let gw = svc.get_goal_weight().unwrap().unwrap();
        assert!((gw - 75.0).abs() < f64::EPSILON);

        // Update
        svc.set_goal_weight(70.0).unwrap();
        let gw = svc.get_goal_weight().unwrap().unwrap();
        assert!((gw - 70.0).abs() < f64::EPSILON);

        // Clear
        assert!(svc.clear_goal_weight().unwrap());
        assert!(svc.get_goal_weight().unwrap().is_none());

        // Clear again returns false
        assert!(!svc.clear_goal_weight().unwrap());
    }

    #[test]
    fn test_watch_glance_empty_day() {
        let svc = GrubService::new_in_memory().unwrap();
        let glance = svc.get_watch_glance("2024-06-15").unwrap();
        assert_eq!(glance.date, "2024-06-15");
        assert!((glance.calories_eaten - 0.0).abs() < f64::EPSILON);
        assert_eq!(glance.meal_count, 0);
        assert!(glance.calories_target.is_none());
        assert!(glance.calories_remaining.is_none());
    }

    #[test]
    fn test_watch_glance_with_meals() {
        let svc = GrubService::new_in_memory().unwrap();
        let food = svc.insert_food(&sample_food()).unwrap();

        svc.log_meal("2024-06-15", "breakfast", food.id, 150.0)
            .unwrap();
        svc.log_meal("2024-06-15", "lunch", food.id, 200.0).unwrap();

        let glance = svc.get_watch_glance("2024-06-15").unwrap();
        assert_eq!(glance.meal_count, 2);
        // 100 cal/100g * 150g + 100 cal/100g * 200g = 150 + 200 = 350
        assert!((glance.calories_eaten - 350.0).abs() < 0.01);
        assert!((glance.protein_g - 35.0).abs() < 0.01);
        assert!((glance.carbs_g - 70.0).abs() < 0.01);
        assert!((glance.fat_g - 17.5).abs() < 0.01);
    }

    #[test]
    fn test_watch_glance_with_target() {
        let svc = GrubService::new_in_memory().unwrap();
        let food = svc.insert_food(&sample_food()).unwrap();

        // Saturday = day_of_week 5
        svc.set_target(5, 2000, Some(30), Some(40), Some(30))
            .unwrap();

        svc.log_meal("2024-06-15", "lunch", food.id, 200.0).unwrap();

        let glance = svc.get_watch_glance("2024-06-15").unwrap();
        assert_eq!(glance.calories_target, Some(2000));
        // 2000 - 200 = 1800 remaining
        assert!((glance.calories_remaining.unwrap() - 1800.0).abs() < 0.01);
        assert!(glance.protein_target_g.is_some());
        assert!(glance.carbs_target_g.is_some());
        assert!(glance.fat_target_g.is_some());
    }

    #[test]
    fn test_watch_recent_foods_empty() {
        let svc = GrubService::new_in_memory().unwrap();
        let recent = svc.get_watch_recent_foods(10).unwrap();
        assert!(recent.is_empty());
    }

    #[test]
    fn test_watch_recent_foods_with_meals() {
        let svc = GrubService::new_in_memory().unwrap();
        let food = svc.insert_food(&sample_food()).unwrap();

        svc.log_meal("2024-06-15", "lunch", food.id, 200.0).unwrap();

        let recent = svc.get_watch_recent_foods(10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].food_id, food.id);
        assert_eq!(recent[0].name, "Test Food");
        assert!((recent[0].last_serving_g - 200.0).abs() < f64::EPSILON);
        assert_eq!(recent[0].last_meal_type, "lunch");
        // 100 cal/100g * 200g = 200 cal
        assert!((recent[0].last_calories - 200.0).abs() < 0.01);
    }
}
