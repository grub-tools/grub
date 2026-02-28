use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{Datelike, Local, NaiveDate};
use rusqlite::{Connection, params};
use uuid::Uuid;

use crate::models::{
    DailySummary, DailyTarget, ExportData, ExportMealEntry, ExportRecipe, ExportRecipeIngredient,
    ExportTarget, ExportWeightEntry, Food, ImportSummary, MEAL_TYPES, MealEntry, MealGroup,
    NewFood, NewMealEntry, NewWeightEntry, RecentFood, Recipe, RecipeDetail, RecipeIngredient,
    SyncPayload, SyncTombstone, UpdateMealEntry, WeightEntry,
};

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open database: {}", path.display()))?;
        let db = Database { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Database { conn };
        db.migrate()?;
        Ok(db)
    }

    #[allow(clippy::too_many_lines)]
    fn migrate(&self) -> Result<()> {
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?;

        if version < 1 {
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS foods (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    brand TEXT,
                    barcode TEXT UNIQUE,
                    calories_per_100g REAL NOT NULL,
                    protein_per_100g REAL,
                    carbs_per_100g REAL,
                    fat_per_100g REAL,
                    default_serving_g REAL,
                    source TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS meal_entries (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    date TEXT NOT NULL,
                    meal_type TEXT NOT NULL,
                    food_id INTEGER NOT NULL REFERENCES foods(id),
                    serving_g REAL NOT NULL,
                    created_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS recipes (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    food_id INTEGER NOT NULL UNIQUE REFERENCES foods(id),
                    portions REAL NOT NULL DEFAULT 1.0,
                    created_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS recipe_ingredients (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    recipe_id INTEGER NOT NULL REFERENCES recipes(id) ON DELETE CASCADE,
                    food_id INTEGER NOT NULL REFERENCES foods(id),
                    quantity_g REAL NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_meal_entries_date ON meal_entries(date);
                CREATE INDEX IF NOT EXISTS idx_foods_barcode ON foods(barcode);
                CREATE INDEX IF NOT EXISTS idx_foods_name ON foods(name);
                CREATE INDEX IF NOT EXISTS idx_recipe_ingredients_recipe ON recipe_ingredients(recipe_id);

                CREATE TABLE IF NOT EXISTS targets (
                    day_of_week INTEGER PRIMARY KEY CHECK (day_of_week BETWEEN 0 AND 6),
                    calories INTEGER NOT NULL,
                    protein_pct INTEGER,
                    carbs_pct INTEGER,
                    fat_pct INTEGER,
                    updated_at TEXT NOT NULL
                );

                PRAGMA user_version = 1;",
            )?;
        }

        if version < 2 {
            // Add uuid and updated_at columns to existing tables
            self.conn.execute_batch(
                "ALTER TABLE foods ADD COLUMN uuid TEXT;
                 ALTER TABLE foods ADD COLUMN updated_at TEXT;
                 ALTER TABLE meal_entries ADD COLUMN uuid TEXT;
                 ALTER TABLE meal_entries ADD COLUMN updated_at TEXT;
                 ALTER TABLE recipes ADD COLUMN uuid TEXT;
                 ALTER TABLE recipes ADD COLUMN updated_at TEXT;
                 ALTER TABLE recipe_ingredients ADD COLUMN uuid TEXT;
                 ALTER TABLE recipe_ingredients ADD COLUMN updated_at TEXT;",
            )?;

            // Generate UUIDs for existing rows
            let now = Local::now().to_rfc3339();
            for table in &["foods", "meal_entries", "recipes"] {
                let ids: Vec<i64> = {
                    let mut stmt = self.conn.prepare(&format!("SELECT id FROM {table}"))?;
                    stmt.query_map([], |row| row.get(0))?
                        .collect::<Result<Vec<_>, _>>()?
                };
                for id in ids {
                    let uuid = Uuid::new_v4().to_string();
                    // Use created_at as updated_at for existing rows
                    let created_at: Option<String> = self
                        .conn
                        .query_row(
                            &format!("SELECT created_at FROM {table} WHERE id = ?1"),
                            params![id],
                            |row| row.get(0),
                        )
                        .ok();
                    let updated_at = created_at.unwrap_or_else(|| now.clone());
                    self.conn.execute(
                        &format!("UPDATE {table} SET uuid = ?1, updated_at = ?2 WHERE id = ?3"),
                        params![uuid, updated_at, id],
                    )?;
                }
            }
            // recipe_ingredients don't have created_at, use now()
            {
                let ids: Vec<i64> = {
                    let mut stmt = self.conn.prepare("SELECT id FROM recipe_ingredients")?;
                    stmt.query_map([], |row| row.get(0))?
                        .collect::<Result<Vec<_>, _>>()?
                };
                for id in ids {
                    let uuid = Uuid::new_v4().to_string();
                    self.conn.execute(
                        "UPDATE recipe_ingredients SET uuid = ?1, updated_at = ?2 WHERE id = ?3",
                        params![uuid, now, id],
                    )?;
                }
            }

            // Create unique indexes and new tables
            self.conn.execute_batch(
                "CREATE UNIQUE INDEX idx_foods_uuid ON foods(uuid);
                 CREATE UNIQUE INDEX idx_meal_entries_uuid ON meal_entries(uuid);
                 CREATE UNIQUE INDEX idx_recipes_uuid ON recipes(uuid);
                 CREATE UNIQUE INDEX idx_recipe_ingredients_uuid ON recipe_ingredients(uuid);

                 CREATE TABLE sync_tombstones (
                     uuid TEXT NOT NULL,
                     table_name TEXT NOT NULL,
                     deleted_at TEXT NOT NULL
                 );
                 CREATE INDEX idx_tombstones_uuid ON sync_tombstones(uuid);

                 CREATE TABLE config (
                     key TEXT PRIMARY KEY,
                     value TEXT NOT NULL
                 );

                 PRAGMA user_version = 2;",
            )?;
        }

        if version < 3 {
            self.conn.execute_batch(
                "ALTER TABLE meal_entries ADD COLUMN display_unit TEXT;
                 ALTER TABLE meal_entries ADD COLUMN display_quantity REAL;
                 PRAGMA user_version = 3;",
            )?;
        }

        if version < 4 {
            // Migrate targets table from single-row (id=1) to per-day-of-week schema.
            // Check if old schema has 'id' column (old single-row layout).
            let has_old_schema: bool = self.conn.prepare("SELECT id FROM targets LIMIT 0").is_ok();

            if has_old_schema {
                // Preserve existing target by applying it to all 7 days.
                self.conn.execute_batch(
                    "CREATE TABLE targets_new (
                        day_of_week INTEGER PRIMARY KEY CHECK (day_of_week BETWEEN 0 AND 6),
                        calories INTEGER NOT NULL,
                        protein_pct INTEGER,
                        carbs_pct INTEGER,
                        fat_pct INTEGER,
                        updated_at TEXT NOT NULL
                     );

                     INSERT OR IGNORE INTO targets_new (day_of_week, calories, protein_pct, carbs_pct, fat_pct, updated_at)
                     SELECT d.day, t.calories, t.protein_pct, t.carbs_pct, t.fat_pct, t.updated_at
                     FROM targets t, (SELECT 0 AS day UNION SELECT 1 UNION SELECT 2 UNION SELECT 3 UNION SELECT 4 UNION SELECT 5 UNION SELECT 6) d
                     WHERE t.id = 1;

                     DROP TABLE targets;
                     ALTER TABLE targets_new RENAME TO targets;",
                )?;
            }

            self.conn.execute_batch("PRAGMA user_version = 4;")?;
        }

        if version < 5 {
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS weight_entries (
                    id INTEGER PRIMARY KEY,
                    uuid TEXT NOT NULL DEFAULT (lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-4' || substr(hex(randomblob(2)),2) || '-' || substr('89ab', abs(random()) % 4 + 1, 1) || substr(hex(randomblob(2)),2) || '-' || hex(randomblob(6)))),
                    date TEXT NOT NULL UNIQUE,
                    weight_kg REAL NOT NULL,
                    source TEXT NOT NULL DEFAULT 'manual',
                    notes TEXT,
                    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                );

                PRAGMA user_version = 5;",
            )?;
        }

        if version < 6 {
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS user_settings (
                    key TEXT PRIMARY KEY NOT NULL,
                    value TEXT NOT NULL,
                    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                );

                PRAGMA user_version = 6;",
            )?;
        }

        Ok(())
    }

    // --- Row mapping helpers ---

    fn food_from_row(row: &rusqlite::Row) -> rusqlite::Result<Food> {
        Ok(Food {
            id: row.get(0)?,
            name: row.get(1)?,
            brand: row.get(2)?,
            barcode: row.get(3)?,
            calories_per_100g: row.get(4)?,
            protein_per_100g: row.get(5)?,
            carbs_per_100g: row.get(6)?,
            fat_per_100g: row.get(7)?,
            default_serving_g: row.get(8)?,
            source: row.get(9)?,
            created_at: row.get(10)?,
            uuid: row.get::<_, Option<String>>(11)?.unwrap_or_default(),
            updated_at: row.get::<_, Option<String>>(12)?.unwrap_or_default(),
        })
    }

    // Expects columns:
    // 0: me.id, 1: me.uuid, 2: me.date, 3: me.meal_type, 4: me.food_id,
    // 5: me.serving_g, 6: me.display_unit, 7: me.display_quantity,
    // 8: me.created_at, 9: me.updated_at,
    // 10: f.name, 11: f.brand, 12: f.calories_per_100g, 13: f.protein_per_100g,
    // 14: f.carbs_per_100g, 15: f.fat_per_100g
    fn meal_entry_from_row(row: &rusqlite::Row) -> rusqlite::Result<MealEntry> {
        let serving_g: f64 = row.get(5)?;
        let cal_100: f64 = row.get(12)?;
        let pro_100: Option<f64> = row.get(13)?;
        let carb_100: Option<f64> = row.get(14)?;
        let fat_100: Option<f64> = row.get(15)?;
        Ok(MealEntry {
            id: row.get(0)?,
            uuid: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            date: row.get(2)?,
            meal_type: row.get(3)?,
            food_id: row.get(4)?,
            serving_g,
            display_unit: row.get(6)?,
            display_quantity: row.get(7)?,
            created_at: row.get(8)?,
            updated_at: row.get::<_, Option<String>>(9)?.unwrap_or_default(),
            food_name: Some(row.get(10)?),
            food_brand: row.get(11)?,
            calories: Some(cal_100 * serving_g / 100.0),
            protein: pro_100.map(|v| v * serving_g / 100.0),
            carbs: carb_100.map(|v| v * serving_g / 100.0),
            fat: fat_100.map(|v| v * serving_g / 100.0),
        })
    }

    // --- Foods ---

    pub fn insert_food(&self, food: &NewFood) -> Result<Food> {
        let now = Local::now().to_rfc3339();
        let uuid = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO foods (name, brand, barcode, calories_per_100g, protein_per_100g, carbs_per_100g, fat_per_100g, default_serving_g, source, created_at, uuid, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                food.name,
                food.brand,
                food.barcode,
                food.calories_per_100g,
                food.protein_per_100g,
                food.carbs_per_100g,
                food.fat_per_100g,
                food.default_serving_g,
                food.source,
                now,
                uuid,
                now,
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.get_food_by_id(id)
    }

    pub fn upsert_food_by_barcode(&self, food: &NewFood) -> Result<Food> {
        if let Some(barcode) = &food.barcode {
            if let Some(existing) = self.get_food_by_barcode(barcode)? {
                return Ok(existing);
            }
        }
        self.insert_food(food)
    }

    pub fn get_food_by_id(&self, id: i64) -> Result<Food> {
        self.conn
            .query_row(
                "SELECT * FROM foods WHERE id = ?1",
                params![id],
                Self::food_from_row,
            )
            .context("Food not found")
    }

    pub fn get_food_by_barcode(&self, barcode: &str) -> Result<Option<Food>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM foods WHERE barcode = ?1")?;
        let mut rows = stmt.query(params![barcode])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Self::food_from_row(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn search_foods_local(&self, query: &str) -> Result<Vec<Food>> {
        let escaped = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        let mut stmt = self.conn.prepare(
            "SELECT * FROM foods WHERE name LIKE ?1 ESCAPE '\\' OR brand LIKE ?1 ESCAPE '\\' ORDER BY name LIMIT 20",
        )?;
        let foods = stmt
            .query_map(params![pattern], Self::food_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(foods)
    }

    pub fn list_foods(&self, search: Option<&str>) -> Result<Vec<Food>> {
        if let Some(query) = search {
            return self.search_foods_local(query);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM foods ORDER BY name LIMIT 100")?;
        let foods = stmt
            .query_map([], Self::food_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(foods)
    }

    // --- Meal Entries ---

    pub fn insert_meal_entry(&self, entry: &NewMealEntry) -> Result<MealEntry> {
        let now = Local::now().to_rfc3339();
        let uuid = Uuid::new_v4().to_string();
        let date_str = entry.date.format("%Y-%m-%d").to_string();
        self.conn.execute(
            "INSERT INTO meal_entries (date, meal_type, food_id, serving_g, display_unit, display_quantity, created_at, uuid, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                date_str,
                entry.meal_type,
                entry.food_id,
                entry.serving_g,
                entry.display_unit,
                entry.display_quantity,
                now,
                uuid,
                now,
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.get_meal_entry(id)
    }

    pub fn get_meal_entry(&self, id: i64) -> Result<MealEntry> {
        self.conn
            .query_row(
                "SELECT me.id, me.uuid, me.date, me.meal_type, me.food_id, me.serving_g,
                        me.display_unit, me.display_quantity, me.created_at, me.updated_at,
                        f.name, f.brand, f.calories_per_100g, f.protein_per_100g, f.carbs_per_100g, f.fat_per_100g
                 FROM meal_entries me
                 JOIN foods f ON me.food_id = f.id
                 WHERE me.id = ?1",
                params![id],
                Self::meal_entry_from_row,
            )
            .context("Meal entry not found")
    }

    pub fn delete_meal_entry(&self, id: i64) -> Result<bool> {
        let rows = self
            .conn
            .execute("DELETE FROM meal_entries WHERE id = ?1", params![id])?;
        Ok(rows > 0)
    }

    pub fn update_meal_entry(&self, id: i64, update: &UpdateMealEntry) -> Result<MealEntry> {
        // Verify existence
        self.get_meal_entry(id)?;

        let now = Local::now().to_rfc3339();
        if let Some(serving_g) = update.serving_g {
            self.conn.execute(
                "UPDATE meal_entries SET serving_g = ?1, updated_at = ?2 WHERE id = ?3",
                params![serving_g, now, id],
            )?;
        }
        if let Some(ref meal_type) = update.meal_type {
            self.conn.execute(
                "UPDATE meal_entries SET meal_type = ?1, updated_at = ?2 WHERE id = ?3",
                params![meal_type, now, id],
            )?;
        }
        if let Some(date) = update.date {
            let date_str = date.format("%Y-%m-%d").to_string();
            self.conn.execute(
                "UPDATE meal_entries SET date = ?1, updated_at = ?2 WHERE id = ?3",
                params![date_str, now, id],
            )?;
        }
        if let Some(ref display_unit) = update.display_unit {
            self.conn.execute(
                "UPDATE meal_entries SET display_unit = ?1, updated_at = ?2 WHERE id = ?3",
                params![display_unit, now, id],
            )?;
        }
        if let Some(ref display_quantity) = update.display_quantity {
            self.conn.execute(
                "UPDATE meal_entries SET display_quantity = ?1, updated_at = ?2 WHERE id = ?3",
                params![display_quantity, now, id],
            )?;
        }

        self.get_meal_entry(id)
    }

    pub fn get_entries_for_date(&self, date: NaiveDate) -> Result<Vec<MealEntry>> {
        let date_str = date.format("%Y-%m-%d").to_string();
        let mut stmt = self.conn.prepare(
            "SELECT me.id, me.uuid, me.date, me.meal_type, me.food_id, me.serving_g,
                    me.display_unit, me.display_quantity, me.created_at, me.updated_at,
                    f.name, f.brand, f.calories_per_100g, f.protein_per_100g, f.carbs_per_100g, f.fat_per_100g
             FROM meal_entries me
             JOIN foods f ON me.food_id = f.id
             WHERE me.date = ?1
             ORDER BY me.id",
        )?;
        let entries = stmt
            .query_map(params![date_str], Self::meal_entry_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    pub fn get_entries_for_date_and_meal(
        &self,
        date: NaiveDate,
        meal_type: &str,
    ) -> Result<Vec<MealEntry>> {
        let date_str = date.format("%Y-%m-%d").to_string();
        let mut stmt = self.conn.prepare(
            "SELECT me.id, me.uuid, me.date, me.meal_type, me.food_id, me.serving_g,
                    me.display_unit, me.display_quantity, me.created_at, me.updated_at,
                    f.name, f.brand, f.calories_per_100g, f.protein_per_100g, f.carbs_per_100g, f.fat_per_100g
             FROM meal_entries me
             JOIN foods f ON me.food_id = f.id
             WHERE me.date = ?1 AND me.meal_type = ?2
             ORDER BY me.id",
        )?;
        let entries = stmt
            .query_map(params![date_str, meal_type], Self::meal_entry_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(entries)
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
        let now = Local::now().to_rfc3339();
        self.conn.execute(
            "INSERT OR REPLACE INTO targets (day_of_week, calories, protein_pct, carbs_pct, fat_pct, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![day_of_week, calories, protein_pct, carbs_pct, fat_pct, now],
        )?;
        Ok(DailyTarget::from_db(
            day_of_week,
            calories,
            protein_pct,
            carbs_pct,
            fat_pct,
        ))
    }

    pub fn get_target(&self, day_of_week: i64) -> Result<Option<DailyTarget>> {
        let mut stmt = self.conn.prepare(
            "SELECT day_of_week, calories, protein_pct, carbs_pct, fat_pct FROM targets WHERE day_of_week = ?1",
        )?;
        let mut rows = stmt.query(params![day_of_week])?;
        if let Some(row) = rows.next()? {
            let day: i64 = row.get(0)?;
            let calories: i64 = row.get(1)?;
            let protein_pct: Option<i64> = row.get(2)?;
            let carbs_pct: Option<i64> = row.get(3)?;
            let fat_pct: Option<i64> = row.get(4)?;
            Ok(Some(DailyTarget::from_db(
                day,
                calories,
                protein_pct,
                carbs_pct,
                fat_pct,
            )))
        } else {
            Ok(None)
        }
    }

    pub fn get_all_targets(&self) -> Result<Vec<DailyTarget>> {
        let mut stmt = self.conn.prepare(
            "SELECT day_of_week, calories, protein_pct, carbs_pct, fat_pct FROM targets ORDER BY day_of_week",
        )?;
        let targets = stmt
            .query_map([], |row| {
                let day: i64 = row.get(0)?;
                let calories: i64 = row.get(1)?;
                let protein_pct: Option<i64> = row.get(2)?;
                let carbs_pct: Option<i64> = row.get(3)?;
                let fat_pct: Option<i64> = row.get(4)?;
                Ok(DailyTarget::from_db(
                    day,
                    calories,
                    protein_pct,
                    carbs_pct,
                    fat_pct,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(targets)
    }

    pub fn clear_target(&self, day_of_week: i64) -> Result<bool> {
        let rows = self.conn.execute(
            "DELETE FROM targets WHERE day_of_week = ?1",
            params![day_of_week],
        )?;
        Ok(rows > 0)
    }

    pub fn clear_all_targets(&self) -> Result<bool> {
        let rows = self.conn.execute("DELETE FROM targets", [])?;
        Ok(rows > 0)
    }

    // --- Recipes ---

    pub fn create_recipe(&self, name: &str, portions: f64) -> Result<Recipe> {
        let now = Local::now().to_rfc3339();
        let uuid = Uuid::new_v4().to_string();
        // Create a placeholder virtual food with zero macros — will be recomputed on add-ingredient
        let food = self.insert_food(&NewFood {
            name: name.to_string(),
            brand: None,
            barcode: None,
            calories_per_100g: 0.0,
            protein_per_100g: Some(0.0),
            carbs_per_100g: Some(0.0),
            fat_per_100g: Some(0.0),
            default_serving_g: Some(0.0),
            source: "recipe".to_string(),
        })?;

        self.conn.execute(
            "INSERT INTO recipes (food_id, portions, created_at, uuid, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![food.id, portions, now, uuid, now],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(Recipe {
            id,
            uuid,
            food_id: food.id,
            portions,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub fn get_recipe_by_id(&self, id: i64) -> Result<Recipe> {
        self.conn
            .query_row(
                "SELECT id, uuid, food_id, portions, created_at, updated_at FROM recipes WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Recipe {
                        id: row.get(0)?,
                        uuid: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                        food_id: row.get(2)?,
                        portions: row.get(3)?,
                        created_at: row.get(4)?,
                        updated_at: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                    })
                },
            )
            .context("Recipe not found")
    }

    pub fn get_recipe_by_food_name(&self, name: &str) -> Result<Recipe> {
        self.conn
            .query_row(
                "SELECT r.id, r.uuid, r.food_id, r.portions, r.created_at, r.updated_at
                 FROM recipes r JOIN foods f ON r.food_id = f.id
                 WHERE LOWER(f.name) = LOWER(?1)",
                params![name],
                |row| {
                    Ok(Recipe {
                        id: row.get(0)?,
                        uuid: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                        food_id: row.get(2)?,
                        portions: row.get(3)?,
                        created_at: row.get(4)?,
                        updated_at: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                    })
                },
            )
            .context(format!("Recipe '{name}' not found"))
    }

    pub fn add_recipe_ingredient(
        &self,
        recipe_id: i64,
        food_id: i64,
        quantity_g: f64,
    ) -> Result<RecipeIngredient> {
        let now = Local::now().to_rfc3339();
        let uuid = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO recipe_ingredients (recipe_id, food_id, quantity_g, uuid, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![recipe_id, food_id, quantity_g, uuid, now],
        )?;
        let id = self.conn.last_insert_rowid();

        // Recompute virtual food
        self.recompute_recipe_food(recipe_id)?;

        Ok(RecipeIngredient {
            id,
            uuid,
            recipe_id,
            food_id,
            quantity_g,
            food_name: None,
            food_brand: None,
            calories: None,
            protein: None,
            carbs: None,
            fat: None,
        })
    }

    pub fn remove_recipe_ingredient(&self, recipe_id: i64, food_name: &str) -> Result<bool> {
        let rows = self.conn.execute(
            "DELETE FROM recipe_ingredients WHERE recipe_id = ?1 AND food_id IN (
                SELECT id FROM foods WHERE LOWER(name) = LOWER(?2)
            )",
            params![recipe_id, food_name],
        )?;
        if rows > 0 {
            self.recompute_recipe_food(recipe_id)?;
        }
        Ok(rows > 0)
    }

    pub fn set_recipe_portions(&self, recipe_id: i64, portions: f64) -> Result<()> {
        let now = Local::now().to_rfc3339();
        self.conn.execute(
            "UPDATE recipes SET portions = ?1, updated_at = ?2 WHERE id = ?3",
            params![portions, now, recipe_id],
        )?;
        self.recompute_recipe_food(recipe_id)?;
        Ok(())
    }

    pub fn get_recipe_ingredients(&self, recipe_id: i64) -> Result<Vec<RecipeIngredient>> {
        let mut stmt = self.conn.prepare(
            "SELECT ri.id, ri.uuid, ri.recipe_id, ri.food_id, ri.quantity_g,
                    f.name, f.brand, f.calories_per_100g, f.protein_per_100g, f.carbs_per_100g, f.fat_per_100g
             FROM recipe_ingredients ri
             JOIN foods f ON ri.food_id = f.id
             WHERE ri.recipe_id = ?1
             ORDER BY ri.id",
        )?;
        let ingredients = stmt
            .query_map(params![recipe_id], |row| {
                let qty: f64 = row.get(4)?;
                let cal_100: f64 = row.get(7)?;
                let pro_100: Option<f64> = row.get(8)?;
                let carb_100: Option<f64> = row.get(9)?;
                let fat_100: Option<f64> = row.get(10)?;
                Ok(RecipeIngredient {
                    id: row.get(0)?,
                    uuid: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    recipe_id: row.get(2)?,
                    food_id: row.get(3)?,
                    quantity_g: qty,
                    food_name: Some(row.get(5)?),
                    food_brand: row.get(6)?,
                    calories: Some(cal_100 * qty / 100.0),
                    protein: pro_100.map(|v| v * qty / 100.0),
                    carbs: carb_100.map(|v| v * qty / 100.0),
                    fat: fat_100.map(|v| v * qty / 100.0),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ingredients)
    }

    pub fn get_recipe_detail(&self, recipe_id: i64) -> Result<RecipeDetail> {
        let recipe = self.get_recipe_by_id(recipe_id)?;
        let food = self.get_food_by_id(recipe.food_id)?;
        let ingredients = self.get_recipe_ingredients(recipe_id)?;

        let total_weight: f64 = ingredients.iter().map(|i| i.quantity_g).sum();
        let total_cal: f64 = ingredients.iter().filter_map(|i| i.calories).sum();
        let total_pro: f64 = ingredients.iter().filter_map(|i| i.protein).sum();
        let total_carbs: f64 = ingredients.iter().filter_map(|i| i.carbs).sum();
        let total_fat: f64 = ingredients.iter().filter_map(|i| i.fat).sum();

        Ok(RecipeDetail {
            id: recipe.id,
            uuid: recipe.uuid,
            food_id: recipe.food_id,
            name: food.name,
            portions: recipe.portions,
            total_weight_g: total_weight,
            per_portion_g: if recipe.portions > 0.0 {
                total_weight / recipe.portions
            } else {
                0.0
            },
            ingredients,
            per_portion_calories: if recipe.portions > 0.0 {
                total_cal / recipe.portions
            } else {
                0.0
            },
            per_portion_protein: if recipe.portions > 0.0 {
                total_pro / recipe.portions
            } else {
                0.0
            },
            per_portion_carbs: if recipe.portions > 0.0 {
                total_carbs / recipe.portions
            } else {
                0.0
            },
            per_portion_fat: if recipe.portions > 0.0 {
                total_fat / recipe.portions
            } else {
                0.0
            },
            calories_per_100g: food.calories_per_100g,
            protein_per_100g: food.protein_per_100g.unwrap_or(0.0),
            carbs_per_100g: food.carbs_per_100g.unwrap_or(0.0),
            fat_per_100g: food.fat_per_100g.unwrap_or(0.0),
        })
    }

    pub fn list_recipes(&self) -> Result<Vec<RecipeDetail>> {
        let mut stmt = self.conn.prepare("SELECT id FROM recipes ORDER BY id")?;
        let ids: Vec<i64> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut details = Vec::new();
        for id in ids {
            details.push(self.get_recipe_detail(id)?);
        }
        Ok(details)
    }

    pub fn delete_recipe(&self, recipe_id: i64) -> Result<()> {
        let recipe = self.get_recipe_by_id(recipe_id)?;
        // Delete ingredients first (CASCADE should handle this, but be explicit)
        self.conn.execute(
            "DELETE FROM recipe_ingredients WHERE recipe_id = ?1",
            params![recipe_id],
        )?;
        self.conn
            .execute("DELETE FROM recipes WHERE id = ?1", params![recipe_id])?;
        // Delete the virtual food
        self.conn
            .execute("DELETE FROM foods WHERE id = ?1", params![recipe.food_id])?;
        Ok(())
    }

    fn recompute_recipe_food(&self, recipe_id: i64) -> Result<()> {
        let recipe = self.get_recipe_by_id(recipe_id)?;
        let ingredients = self.get_recipe_ingredients(recipe_id)?;

        let total_weight: f64 = ingredients.iter().map(|i| i.quantity_g).sum();
        let total_cal: f64 = ingredients.iter().filter_map(|i| i.calories).sum();
        let total_pro: f64 = ingredients.iter().filter_map(|i| i.protein).sum();
        let total_carbs: f64 = ingredients.iter().filter_map(|i| i.carbs).sum();
        let total_fat: f64 = ingredients.iter().filter_map(|i| i.fat).sum();

        let (cal_100, pro_100, carb_100, fat_100, serving_g) = if total_weight > 0.0 {
            (
                total_cal * 100.0 / total_weight,
                total_pro * 100.0 / total_weight,
                total_carbs * 100.0 / total_weight,
                total_fat * 100.0 / total_weight,
                total_weight / recipe.portions,
            )
        } else {
            (0.0, 0.0, 0.0, 0.0, 0.0)
        };

        let now = Local::now().to_rfc3339();
        self.conn.execute(
            "UPDATE foods SET calories_per_100g = ?1, protein_per_100g = ?2, carbs_per_100g = ?3,
             fat_per_100g = ?4, default_serving_g = ?5, updated_at = ?6 WHERE id = ?7",
            params![
                cal_100,
                pro_100,
                carb_100,
                fat_100,
                serving_g,
                now,
                recipe.food_id
            ],
        )?;
        Ok(())
    }

    // --- Sync support ---

    pub fn record_tombstone(&self, uuid: &str, table_name: &str) -> Result<()> {
        let now = Local::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO sync_tombstones (uuid, table_name, deleted_at) VALUES (?1, ?2, ?3)",
            params![uuid, table_name, now],
        )?;
        Ok(())
    }

    pub fn get_tombstones(&self) -> Result<Vec<SyncTombstone>> {
        let mut stmt = self
            .conn
            .prepare("SELECT uuid, table_name, deleted_at FROM sync_tombstones")?;
        let tombstones = stmt
            .query_map([], |row| {
                Ok(SyncTombstone {
                    uuid: row.get(0)?,
                    table_name: row.get(1)?,
                    deleted_at: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tombstones)
    }

    pub fn get_tombstones_since(&self, since: &str) -> Result<Vec<SyncTombstone>> {
        let mut stmt = self.conn.prepare(
            "SELECT uuid, table_name, deleted_at FROM sync_tombstones WHERE deleted_at > ?1",
        )?;
        let tombstones = stmt
            .query_map(params![since], |row| {
                Ok(SyncTombstone {
                    uuid: row.get(0)?,
                    table_name: row.get(1)?,
                    deleted_at: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tombstones)
    }

    pub fn clear_tombstones(&self) -> Result<()> {
        self.conn.execute("DELETE FROM sync_tombstones", [])?;
        Ok(())
    }

    // --- Delta sync ---

    pub fn get_foods_since(&self, since: &str) -> Result<Vec<Food>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM foods WHERE updated_at > ?1 ORDER BY id")?;
        let foods = stmt
            .query_map(params![since], Self::food_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(foods)
    }

    pub fn get_all_foods(&self) -> Result<Vec<Food>> {
        let mut stmt = self.conn.prepare("SELECT * FROM foods ORDER BY id")?;
        let foods = stmt
            .query_map([], Self::food_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(foods)
    }

    pub fn get_meal_entries_since(&self, since: &str) -> Result<Vec<ExportMealEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT me.id, me.uuid, me.date, me.meal_type, me.food_id, me.serving_g,
                    me.display_unit, me.display_quantity, me.created_at,
                    me.updated_at, f.uuid as food_uuid
             FROM meal_entries me JOIN foods f ON me.food_id = f.id
             WHERE me.updated_at > ?1
             ORDER BY me.id",
        )?;
        let entries = stmt
            .query_map(params![since], Self::export_meal_entry_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    pub fn get_all_meal_entries_export(&self) -> Result<Vec<ExportMealEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT me.id, me.uuid, me.date, me.meal_type, me.food_id, me.serving_g,
                    me.display_unit, me.display_quantity, me.created_at,
                    me.updated_at, f.uuid as food_uuid
             FROM meal_entries me JOIN foods f ON me.food_id = f.id
             ORDER BY me.id",
        )?;
        let entries = stmt
            .query_map([], Self::export_meal_entry_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    fn export_meal_entry_from_row(row: &rusqlite::Row) -> rusqlite::Result<ExportMealEntry> {
        Ok(ExportMealEntry {
            id: row.get(0)?,
            uuid: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            date: row.get(2)?,
            meal_type: row.get(3)?,
            food_id: row.get(4)?,
            serving_g: row.get(5)?,
            display_unit: row.get(6)?,
            display_quantity: row.get(7)?,
            created_at: row.get(8)?,
            updated_at: row.get::<_, Option<String>>(9)?.unwrap_or_default(),
            food_uuid: row.get::<_, Option<String>>(10)?.unwrap_or_default(),
        })
    }

    fn export_recipe_from_row(row: &rusqlite::Row) -> rusqlite::Result<ExportRecipe> {
        Ok(ExportRecipe {
            id: row.get(0)?,
            uuid: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            food_id: row.get(2)?,
            portions: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            food_uuid: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
        })
    }

    fn export_recipe_ingredient_from_row(
        row: &rusqlite::Row,
    ) -> rusqlite::Result<ExportRecipeIngredient> {
        Ok(ExportRecipeIngredient {
            id: row.get(0)?,
            uuid: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            recipe_id: row.get(2)?,
            food_id: row.get(3)?,
            quantity_g: row.get(4)?,
            recipe_uuid: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            food_uuid: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
        })
    }

    fn export_target_from_row(row: &rusqlite::Row) -> rusqlite::Result<ExportTarget> {
        Ok(ExportTarget {
            day_of_week: row.get(0)?,
            calories: row.get(1)?,
            protein_pct: row.get(2)?,
            carbs_pct: row.get(3)?,
            fat_pct: row.get(4)?,
            updated_at: row.get(5)?,
        })
    }

    fn export_weight_entry_from_row(row: &rusqlite::Row) -> rusqlite::Result<ExportWeightEntry> {
        Ok(ExportWeightEntry {
            uuid: row.get(0)?,
            date: row.get(1)?,
            weight_kg: row.get(2)?,
            source: row.get(3)?,
            notes: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
        })
    }

    pub fn get_recipes_since(&self, since: &str) -> Result<Vec<ExportRecipe>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.uuid, r.food_id, r.portions, r.created_at, r.updated_at, f.uuid as food_uuid
             FROM recipes r JOIN foods f ON r.food_id = f.id
             WHERE r.updated_at > ?1
             ORDER BY r.id",
        )?;
        let recipes = stmt
            .query_map(params![since], Self::export_recipe_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(recipes)
    }

    pub fn get_all_recipes_export(&self) -> Result<Vec<ExportRecipe>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.uuid, r.food_id, r.portions, r.created_at, r.updated_at, f.uuid as food_uuid
             FROM recipes r JOIN foods f ON r.food_id = f.id
             ORDER BY r.id",
        )?;
        let recipes = stmt
            .query_map([], Self::export_recipe_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(recipes)
    }

    pub fn get_recipe_ingredients_since(&self, since: &str) -> Result<Vec<ExportRecipeIngredient>> {
        let mut stmt = self.conn.prepare(
            "SELECT ri.id, ri.uuid, ri.recipe_id, ri.food_id, ri.quantity_g,
                    r.uuid as recipe_uuid, f.uuid as food_uuid
             FROM recipe_ingredients ri
             JOIN recipes r ON ri.recipe_id = r.id
             JOIN foods f ON ri.food_id = f.id
             WHERE ri.updated_at > ?1
             ORDER BY ri.id",
        )?;
        let ingredients = stmt
            .query_map(params![since], Self::export_recipe_ingredient_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ingredients)
    }

    pub fn get_all_recipe_ingredients_export(&self) -> Result<Vec<ExportRecipeIngredient>> {
        let mut stmt = self.conn.prepare(
            "SELECT ri.id, ri.uuid, ri.recipe_id, ri.food_id, ri.quantity_g,
                    r.uuid as recipe_uuid, f.uuid as food_uuid
             FROM recipe_ingredients ri
             JOIN recipes r ON ri.recipe_id = r.id
             JOIN foods f ON ri.food_id = f.id
             ORDER BY ri.id",
        )?;
        let ingredients = stmt
            .query_map([], Self::export_recipe_ingredient_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ingredients)
    }

    pub fn get_targets_since(&self, since: &str) -> Result<Vec<ExportTarget>> {
        let mut stmt = self.conn.prepare(
            "SELECT day_of_week, calories, protein_pct, carbs_pct, fat_pct, updated_at
             FROM targets WHERE updated_at > ?1
             ORDER BY day_of_week",
        )?;
        let targets = stmt
            .query_map(params![since], Self::export_target_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(targets)
    }

    pub fn get_all_targets_export(&self) -> Result<Vec<ExportTarget>> {
        let mut stmt = self.conn.prepare(
            "SELECT day_of_week, calories, protein_pct, carbs_pct, fat_pct, updated_at
             FROM targets ORDER BY day_of_week",
        )?;
        let targets = stmt
            .query_map([], Self::export_target_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(targets)
    }

    pub fn get_weight_entries_since(&self, since: &str) -> Result<Vec<ExportWeightEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT uuid, date, weight_kg, source, notes, created_at, updated_at
             FROM weight_entries WHERE updated_at > ?1
             ORDER BY date",
        )?;
        let entries = stmt
            .query_map(params![since], Self::export_weight_entry_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    pub fn get_all_weight_entries_export(&self) -> Result<Vec<ExportWeightEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT uuid, date, weight_kg, source, notes, created_at, updated_at
             FROM weight_entries ORDER BY date",
        )?;
        let entries = stmt
            .query_map([], Self::export_weight_entry_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    pub fn changes_since(
        &self,
        since: Option<&str>,
        server_timestamp: &str,
    ) -> Result<SyncPayload> {
        let (foods, meal_entries, recipes, recipe_ingredients, targets, weight_entries, tombstones) =
            match since {
                Some(ts) => (
                    self.get_foods_since(ts)?,
                    self.get_meal_entries_since(ts)?,
                    self.get_recipes_since(ts)?,
                    self.get_recipe_ingredients_since(ts)?,
                    self.get_targets_since(ts)?,
                    self.get_weight_entries_since(ts)?,
                    self.get_tombstones_since(ts)?,
                ),
                None => (
                    self.get_all_foods()?,
                    self.get_all_meal_entries_export()?,
                    self.get_all_recipes_export()?,
                    self.get_all_recipe_ingredients_export()?,
                    self.get_all_targets_export()?,
                    self.get_all_weight_entries_export()?,
                    self.get_tombstones()?,
                ),
            };
        Ok(SyncPayload {
            foods,
            meal_entries,
            recipes,
            recipe_ingredients,
            targets,
            weight_entries,
            tombstones,
            server_timestamp: server_timestamp.to_string(),
        })
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn apply_remote_changes(
        &self,
        foods: &[Food],
        meal_entries: &[ExportMealEntry],
        recipes: &[ExportRecipe],
        recipe_ingredients: &[ExportRecipeIngredient],
        targets: &[ExportTarget],
        weight_entries: &[ExportWeightEntry],
        tombstones: &[SyncTombstone],
    ) -> Result<()> {
        // Step 1: Merge foods — build uuid→local_id mapping
        let mut food_uuid_to_local_id: HashMap<String, i64> = HashMap::new();
        for food in foods {
            if food.uuid.is_empty() {
                continue;
            }
            if let Some(existing) = self.get_food_by_uuid(&food.uuid)? {
                food_uuid_to_local_id.insert(food.uuid.clone(), existing.id);
                if food.updated_at > existing.updated_at {
                    self.conn.execute(
                        "UPDATE foods SET name=?1, brand=?2, barcode=?3, calories_per_100g=?4,
                         protein_per_100g=?5, carbs_per_100g=?6, fat_per_100g=?7,
                         default_serving_g=?8, source=?9, updated_at=?10 WHERE uuid=?11",
                        params![
                            food.name,
                            food.brand,
                            food.barcode,
                            food.calories_per_100g,
                            food.protein_per_100g,
                            food.carbs_per_100g,
                            food.fat_per_100g,
                            food.default_serving_g,
                            food.source,
                            food.updated_at,
                            food.uuid,
                        ],
                    )?;
                }
            } else {
                self.conn.execute(
                    "INSERT INTO foods (name, brand, barcode, calories_per_100g,
                     protein_per_100g, carbs_per_100g, fat_per_100g,
                     default_serving_g, source, created_at, uuid, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        food.name,
                        food.brand,
                        food.barcode,
                        food.calories_per_100g,
                        food.protein_per_100g,
                        food.carbs_per_100g,
                        food.fat_per_100g,
                        food.default_serving_g,
                        food.source,
                        food.created_at,
                        food.uuid,
                        food.updated_at,
                    ],
                )?;
                let new_id = self.conn.last_insert_rowid();
                food_uuid_to_local_id.insert(food.uuid.clone(), new_id);
            }
        }

        // Step 2: Merge meal entries
        for entry in meal_entries {
            if entry.uuid.is_empty() {
                continue;
            }
            let local_food_id = if entry.food_uuid.is_empty() {
                None
            } else {
                food_uuid_to_local_id
                    .get(&entry.food_uuid)
                    .copied()
                    .or_else(|| {
                        self.get_food_by_uuid(&entry.food_uuid)
                            .ok()
                            .flatten()
                            .map(|f| f.id)
                    })
            };
            let Some(food_id) = local_food_id else {
                continue;
            };

            if let Some(existing_id) = self.get_meal_entry_by_uuid(&entry.uuid)? {
                let existing_updated: String = self.conn.query_row(
                    "SELECT COALESCE(updated_at, '') FROM meal_entries WHERE id = ?1",
                    params![existing_id],
                    |row| row.get(0),
                )?;
                if entry.updated_at > existing_updated {
                    self.conn.execute(
                        "UPDATE meal_entries SET date=?1, meal_type=?2, food_id=?3, serving_g=?4, display_unit=?5, display_quantity=?6, updated_at=?7 WHERE id=?8",
                        params![entry.date, entry.meal_type, food_id, entry.serving_g, entry.display_unit, entry.display_quantity, entry.updated_at, existing_id],
                    )?;
                }
            } else {
                self.conn.execute(
                    "INSERT INTO meal_entries (date, meal_type, food_id, serving_g, display_unit, display_quantity, created_at, uuid, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![entry.date, entry.meal_type, food_id, entry.serving_g, entry.display_unit, entry.display_quantity, entry.created_at, entry.uuid, entry.updated_at],
                )?;
            }
        }

        // Step 3: Merge recipes — build recipe_uuid→local_id mapping
        let mut recipe_uuid_to_local_id: HashMap<String, i64> = HashMap::new();
        for recipe in recipes {
            if recipe.uuid.is_empty() {
                continue;
            }
            let local_food_id = if recipe.food_uuid.is_empty() {
                None
            } else {
                food_uuid_to_local_id
                    .get(&recipe.food_uuid)
                    .copied()
                    .or_else(|| {
                        self.get_food_by_uuid(&recipe.food_uuid)
                            .ok()
                            .flatten()
                            .map(|f| f.id)
                    })
            };
            let Some(food_id) = local_food_id else {
                continue;
            };

            if let Some(existing) = self.get_recipe_by_uuid(&recipe.uuid)? {
                recipe_uuid_to_local_id.insert(recipe.uuid.clone(), existing.id);
                if recipe.updated_at > existing.updated_at {
                    self.conn.execute(
                        "UPDATE recipes SET food_id=?1, portions=?2, updated_at=?3 WHERE id=?4",
                        params![food_id, recipe.portions, recipe.updated_at, existing.id],
                    )?;
                }
            } else {
                self.conn.execute(
                    "INSERT INTO recipes (food_id, portions, created_at, uuid, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![food_id, recipe.portions, recipe.created_at, recipe.uuid, recipe.updated_at],
                )?;
                let new_id = self.conn.last_insert_rowid();
                recipe_uuid_to_local_id.insert(recipe.uuid.clone(), new_id);
            }
        }

        // Step 4: Merge recipe ingredients
        let mut recipes_to_recompute: std::collections::HashSet<i64> =
            std::collections::HashSet::new();
        for ing in recipe_ingredients {
            if ing.uuid.is_empty() {
                continue;
            }
            let local_recipe_id = if ing.recipe_uuid.is_empty() {
                None
            } else {
                recipe_uuid_to_local_id
                    .get(&ing.recipe_uuid)
                    .copied()
                    .or_else(|| {
                        self.get_recipe_by_uuid(&ing.recipe_uuid)
                            .ok()
                            .flatten()
                            .map(|r| r.id)
                    })
            };
            let local_food_id = if ing.food_uuid.is_empty() {
                None
            } else {
                food_uuid_to_local_id
                    .get(&ing.food_uuid)
                    .copied()
                    .or_else(|| {
                        self.get_food_by_uuid(&ing.food_uuid)
                            .ok()
                            .flatten()
                            .map(|f| f.id)
                    })
            };
            let (Some(recipe_id), Some(food_id)) = (local_recipe_id, local_food_id) else {
                continue;
            };

            if let Some(existing_id) = self.get_recipe_ingredient_by_uuid(&ing.uuid)? {
                self.conn.execute(
                    "UPDATE recipe_ingredients SET recipe_id=?1, food_id=?2, quantity_g=?3 WHERE id=?4",
                    params![recipe_id, food_id, ing.quantity_g, existing_id],
                )?;
            } else {
                let now = Local::now().to_rfc3339();
                self.conn.execute(
                    "INSERT INTO recipe_ingredients (recipe_id, food_id, quantity_g, uuid, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![recipe_id, food_id, ing.quantity_g, ing.uuid, now],
                )?;
            }
            recipes_to_recompute.insert(recipe_id);
        }

        // Recompute virtual foods for affected recipes
        for recipe_id in &recipes_to_recompute {
            self.recompute_recipe_food(*recipe_id)?;
        }

        // Step 5: Merge targets
        for incoming_target in targets {
            let local_updated: Option<String> = self
                .conn
                .query_row(
                    "SELECT updated_at FROM targets WHERE day_of_week = ?1",
                    params![incoming_target.day_of_week],
                    |row| row.get(0),
                )
                .ok();
            let should_update = match (&incoming_target.updated_at, &local_updated) {
                (Some(incoming), Some(local)) => incoming > local,
                (Some(_), None) | (None, _) => true,
            };
            if should_update {
                let updated_at = incoming_target
                    .updated_at
                    .clone()
                    .unwrap_or_else(|| Local::now().to_rfc3339());
                self.conn.execute(
                    "INSERT OR REPLACE INTO targets (day_of_week, calories, protein_pct, carbs_pct, fat_pct, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        incoming_target.day_of_week,
                        incoming_target.calories,
                        incoming_target.protein_pct,
                        incoming_target.carbs_pct,
                        incoming_target.fat_pct,
                        updated_at,
                    ],
                )?;
            }
        }

        // Step 6: Process tombstones
        let mut dummy_recompute = std::collections::HashSet::new();
        for tombstone in tombstones {
            self.apply_tombstone(tombstone, &mut dummy_recompute)?;
            // Store tombstone for propagation
            let exists: i64 = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM sync_tombstones WHERE uuid = ?1 AND table_name = ?2",
                    params![tombstone.uuid, tombstone.table_name],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            if exists == 0 {
                self.conn.execute(
                    "INSERT INTO sync_tombstones (uuid, table_name, deleted_at) VALUES (?1, ?2, ?3)",
                    params![tombstone.uuid, tombstone.table_name, tombstone.deleted_at],
                )?;
            }
        }

        // Step 7: Merge weight entries (LWW by date — newer updated_at wins)
        for entry in weight_entries {
            if entry.uuid.is_empty() {
                continue;
            }
            let existing: Option<(String, String)> = self
                .conn
                .query_row(
                    "SELECT uuid, updated_at FROM weight_entries WHERE date = ?1",
                    params![entry.date],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .ok();
            if let Some((_existing_uuid, existing_updated)) = existing {
                if entry.updated_at > existing_updated {
                    self.conn.execute(
                        "UPDATE weight_entries SET uuid=?1, weight_kg=?2, source=?3, notes=?4, updated_at=?5 WHERE date=?6",
                        params![entry.uuid, entry.weight_kg, entry.source, entry.notes, entry.updated_at, entry.date],
                    )?;
                }
            } else {
                self.conn.execute(
                    "INSERT INTO weight_entries (uuid, date, weight_kg, source, notes, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![entry.uuid, entry.date, entry.weight_kg, entry.source, entry.notes, entry.created_at, entry.updated_at],
                )?;
            }
        }

        Ok(())
    }

    pub fn get_or_create_device_id(&self) -> Result<String> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM config WHERE key = 'device_id'")?;
        let mut rows = stmt.query([])?;
        if let Some(row) = rows.next()? {
            return Ok(row.get(0)?);
        }
        drop(rows);
        drop(stmt);

        let device_id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO config (key, value) VALUES ('device_id', ?1)",
            params![device_id],
        )?;
        Ok(device_id)
    }

    pub fn get_food_by_uuid(&self, uuid: &str) -> Result<Option<Food>> {
        let mut stmt = self.conn.prepare("SELECT * FROM foods WHERE uuid = ?1")?;
        let mut rows = stmt.query(params![uuid])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Self::food_from_row(row)?))
        } else {
            Ok(None)
        }
    }

    fn get_meal_entry_by_uuid(&self, uuid: &str) -> Result<Option<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM meal_entries WHERE uuid = ?1")?;
        let mut rows = stmt.query(params![uuid])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    fn get_recipe_by_uuid(&self, uuid: &str) -> Result<Option<Recipe>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, uuid, food_id, portions, created_at, updated_at FROM recipes WHERE uuid = ?1",
        )?;
        let mut rows = stmt.query(params![uuid])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Recipe {
                id: row.get(0)?,
                uuid: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                food_id: row.get(2)?,
                portions: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            }))
        } else {
            Ok(None)
        }
    }

    fn get_recipe_ingredient_by_uuid(&self, uuid: &str) -> Result<Option<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM recipe_ingredients WHERE uuid = ?1")?;
        let mut rows = stmt.query(params![uuid])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_meal_entry_uuid(&self, id: i64) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT uuid FROM meal_entries WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .context("Meal entry not found")
            .map(Some)
    }

    pub fn get_recipe_uuid(&self, id: i64) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT uuid FROM recipes WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .context("Recipe not found")
            .map(Some)
    }

    pub fn get_recipe_ingredient_uuids(&self, recipe_id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT uuid FROM recipe_ingredients WHERE recipe_id = ?1")?;
        let uuids = stmt
            .query_map(params![recipe_id], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(uuids)
    }

    // --- Export / Import ---

    #[allow(clippy::too_many_lines)]
    pub fn export_all(&self) -> Result<ExportData> {
        let device_id = self.get_or_create_device_id()?;
        let foods = self.get_all_foods()?;
        let meal_entries = self.get_all_meal_entries_export()?;
        let recipes = self.get_all_recipes_export()?;
        let recipe_ingredients = self.get_all_recipe_ingredients_export()?;
        let targets = self.get_all_targets_export()?;
        let weight_entries = self.get_all_weight_entries_export()?;
        let tombstones = self.get_tombstones()?;

        let exported_at = Local::now().to_rfc3339();
        Ok(ExportData {
            version: 3,
            exported_at,
            device_id: Some(device_id),
            foods,
            meal_entries,
            recipes,
            recipe_ingredients,
            target: None,
            targets,
            weight_entries,
            tombstones: Some(tombstones),
        })
    }

    pub fn import_all(&self, data: &ExportData) -> Result<ImportSummary> {
        if data.version >= 2 {
            self.merge_import(data)
        } else {
            self.import_v1(data)
        }
    }

    fn import_v1(&self, data: &ExportData) -> Result<ImportSummary> {
        let foods_imported = self.import_foods(&data.foods)?;
        let meal_entries_imported = self.import_meal_entries(&data.meal_entries)?;
        let (recipes_imported, recipe_ingredients_imported) =
            self.import_recipes(&data.recipes, &data.recipe_ingredients)?;
        let targets_imported = self.import_targets(data)?;
        let weight_entries_imported = self.import_weight_entries(&data.weight_entries)?;

        Ok(ImportSummary {
            foods_imported,
            meal_entries_imported,
            recipes_imported,
            recipe_ingredients_imported,
            targets_imported,
            weight_entries_imported,
            tombstones_processed: 0,
        })
    }

    #[allow(clippy::cast_possible_wrap)]
    fn import_foods(&self, foods: &[Food]) -> Result<i64> {
        let mut count: i64 = 0;
        for food in foods {
            let exists = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM foods WHERE id = ?1",
                    params![food.id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap_or(0);
            if exists > 0 {
                self.conn.execute(
                    "UPDATE foods SET name=?1, brand=?2, barcode=?3, calories_per_100g=?4,
                     protein_per_100g=?5, carbs_per_100g=?6, fat_per_100g=?7,
                     default_serving_g=?8, source=?9 WHERE id=?10",
                    params![
                        food.name,
                        food.brand,
                        food.barcode,
                        food.calories_per_100g,
                        food.protein_per_100g,
                        food.carbs_per_100g,
                        food.fat_per_100g,
                        food.default_serving_g,
                        food.source,
                        food.id,
                    ],
                )?;
            } else {
                self.insert_food_for_import(food)?;
            }
            count += 1;
        }
        Ok(count)
    }

    fn insert_food_for_import(&self, food: &Food) -> Result<()> {
        if let Some(barcode) = &food.barcode {
            let barcode_exists = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM foods WHERE barcode = ?1",
                    params![barcode],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap_or(0);
            if barcode_exists > 0 {
                return Ok(());
            }
        }
        self.conn.execute(
            "INSERT INTO foods (id, name, brand, barcode, calories_per_100g,
             protein_per_100g, carbs_per_100g, fat_per_100g,
             default_serving_g, source, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                food.id,
                food.name,
                food.brand,
                food.barcode,
                food.calories_per_100g,
                food.protein_per_100g,
                food.carbs_per_100g,
                food.fat_per_100g,
                food.default_serving_g,
                food.source,
                food.created_at,
            ],
        )?;
        Ok(())
    }

    #[allow(clippy::cast_possible_wrap)]
    fn import_meal_entries(&self, entries: &[ExportMealEntry]) -> Result<i64> {
        let mut count: i64 = 0;
        for entry in entries {
            self.conn.execute(
                "INSERT OR REPLACE INTO meal_entries (id, date, meal_type, food_id, serving_g, display_unit, display_quantity, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    entry.id,
                    entry.date,
                    entry.meal_type,
                    entry.food_id,
                    entry.serving_g,
                    entry.display_unit,
                    entry.display_quantity,
                    entry.created_at,
                ],
            )?;
            count += 1;
        }
        Ok(count)
    }

    #[allow(clippy::cast_possible_wrap)]
    fn import_recipes(
        &self,
        recipes: &[ExportRecipe],
        ingredients: &[ExportRecipeIngredient],
    ) -> Result<(i64, i64)> {
        let mut recipe_count: i64 = 0;
        let mut ingredient_count: i64 = 0;

        for recipe in recipes {
            self.conn.execute(
                "INSERT OR REPLACE INTO recipes (id, food_id, portions, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    recipe.id,
                    recipe.food_id,
                    recipe.portions,
                    recipe.created_at
                ],
            )?;
            self.conn.execute(
                "DELETE FROM recipe_ingredients WHERE recipe_id = ?1",
                params![recipe.id],
            )?;
            recipe_count += 1;
        }

        for ing in ingredients {
            self.conn.execute(
                "INSERT INTO recipe_ingredients (id, recipe_id, food_id, quantity_g)
                 VALUES (?1, ?2, ?3, ?4)",
                params![ing.id, ing.recipe_id, ing.food_id, ing.quantity_g],
            )?;
            ingredient_count += 1;
        }

        Ok((recipe_count, ingredient_count))
    }

    #[allow(clippy::cast_possible_wrap)]
    fn import_targets(&self, data: &ExportData) -> Result<i64> {
        let now = Local::now().to_rfc3339();

        if !data.targets.is_empty() {
            let mut count: i64 = 0;
            for target in &data.targets {
                self.conn.execute(
                    "INSERT OR REPLACE INTO targets (day_of_week, calories, protein_pct, carbs_pct, fat_pct, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        target.day_of_week,
                        target.calories,
                        target.protein_pct,
                        target.carbs_pct,
                        target.fat_pct,
                        now,
                    ],
                )?;
                count += 1;
            }
            Ok(count)
        } else if let Some(legacy) = &data.target {
            // Legacy single target — apply to all 7 days
            for day in 0..7_i64 {
                self.conn.execute(
                    "INSERT OR REPLACE INTO targets (day_of_week, calories, protein_pct, carbs_pct, fat_pct, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        day,
                        legacy.calories,
                        legacy.protein_pct,
                        legacy.carbs_pct,
                        legacy.fat_pct,
                        now,
                    ],
                )?;
            }
            Ok(7)
        } else {
            Ok(0)
        }
    }

    #[allow(clippy::cast_possible_wrap)]
    fn import_weight_entries(&self, entries: &[ExportWeightEntry]) -> Result<i64> {
        let mut count: i64 = 0;
        for entry in entries {
            self.conn.execute(
                "INSERT OR REPLACE INTO weight_entries (uuid, date, weight_kg, source, notes, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    entry.uuid,
                    entry.date,
                    entry.weight_kg,
                    entry.source,
                    entry.notes,
                    entry.created_at,
                    entry.updated_at,
                ],
            )?;
            count += 1;
        }
        Ok(count)
    }

    #[allow(clippy::cast_possible_wrap, clippy::too_many_lines)]
    fn merge_import(&self, data: &ExportData) -> Result<ImportSummary> {
        let mut foods_imported: i64 = 0;
        let mut meal_entries_imported: i64 = 0;
        let mut recipes_imported: i64 = 0;
        let mut recipe_ingredients_imported: i64 = 0;
        let mut tombstones_processed: i64 = 0;

        // Step 1: Merge foods — build uuid→local_id mapping
        let mut food_uuid_to_local_id: HashMap<String, i64> = HashMap::new();
        for food in &data.foods {
            if food.uuid.is_empty() {
                continue;
            }
            if let Some(existing) = self.get_food_by_uuid(&food.uuid)? {
                food_uuid_to_local_id.insert(food.uuid.clone(), existing.id);
                if food.updated_at > existing.updated_at {
                    self.conn.execute(
                        "UPDATE foods SET name=?1, brand=?2, barcode=?3, calories_per_100g=?4,
                         protein_per_100g=?5, carbs_per_100g=?6, fat_per_100g=?7,
                         default_serving_g=?8, source=?9, updated_at=?10 WHERE uuid=?11",
                        params![
                            food.name,
                            food.brand,
                            food.barcode,
                            food.calories_per_100g,
                            food.protein_per_100g,
                            food.carbs_per_100g,
                            food.fat_per_100g,
                            food.default_serving_g,
                            food.source,
                            food.updated_at,
                            food.uuid,
                        ],
                    )?;
                    foods_imported += 1;
                }
            } else {
                self.conn.execute(
                    "INSERT INTO foods (name, brand, barcode, calories_per_100g,
                     protein_per_100g, carbs_per_100g, fat_per_100g,
                     default_serving_g, source, created_at, uuid, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        food.name,
                        food.brand,
                        food.barcode,
                        food.calories_per_100g,
                        food.protein_per_100g,
                        food.carbs_per_100g,
                        food.fat_per_100g,
                        food.default_serving_g,
                        food.source,
                        food.created_at,
                        food.uuid,
                        food.updated_at,
                    ],
                )?;
                let new_id = self.conn.last_insert_rowid();
                food_uuid_to_local_id.insert(food.uuid.clone(), new_id);
                foods_imported += 1;
            }
        }

        // Step 2: Merge meal entries
        for entry in &data.meal_entries {
            if entry.uuid.is_empty() {
                continue;
            }
            let local_food_id = if entry.food_uuid.is_empty() {
                None
            } else {
                food_uuid_to_local_id.get(&entry.food_uuid).copied()
            };
            let Some(food_id) = local_food_id else {
                continue;
            };

            if let Some(existing_id) = self.get_meal_entry_by_uuid(&entry.uuid)? {
                let existing_updated: String = self.conn.query_row(
                    "SELECT COALESCE(updated_at, '') FROM meal_entries WHERE id = ?1",
                    params![existing_id],
                    |row| row.get(0),
                )?;
                if entry.updated_at > existing_updated {
                    self.conn.execute(
                        "UPDATE meal_entries SET date=?1, meal_type=?2, food_id=?3, serving_g=?4, display_unit=?5, display_quantity=?6, updated_at=?7 WHERE id=?8",
                        params![entry.date, entry.meal_type, food_id, entry.serving_g, entry.display_unit, entry.display_quantity, entry.updated_at, existing_id],
                    )?;
                    meal_entries_imported += 1;
                }
            } else {
                self.conn.execute(
                    "INSERT INTO meal_entries (date, meal_type, food_id, serving_g, display_unit, display_quantity, created_at, uuid, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![entry.date, entry.meal_type, food_id, entry.serving_g, entry.display_unit, entry.display_quantity, entry.created_at, entry.uuid, entry.updated_at],
                )?;
                meal_entries_imported += 1;
            }
        }

        // Step 3: Merge recipes — build recipe_uuid→local_id mapping
        let mut recipe_uuid_to_local_id: HashMap<String, i64> = HashMap::new();
        for recipe in &data.recipes {
            if recipe.uuid.is_empty() {
                continue;
            }
            let local_food_id = if recipe.food_uuid.is_empty() {
                None
            } else {
                food_uuid_to_local_id.get(&recipe.food_uuid).copied()
            };
            let Some(food_id) = local_food_id else {
                continue;
            };

            if let Some(existing) = self.get_recipe_by_uuid(&recipe.uuid)? {
                recipe_uuid_to_local_id.insert(recipe.uuid.clone(), existing.id);
                if recipe.updated_at > existing.updated_at {
                    self.conn.execute(
                        "UPDATE recipes SET food_id=?1, portions=?2, updated_at=?3 WHERE id=?4",
                        params![food_id, recipe.portions, recipe.updated_at, existing.id],
                    )?;
                    recipes_imported += 1;
                }
            } else {
                self.conn.execute(
                    "INSERT INTO recipes (food_id, portions, created_at, uuid, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![food_id, recipe.portions, recipe.created_at, recipe.uuid, recipe.updated_at],
                )?;
                let new_id = self.conn.last_insert_rowid();
                recipe_uuid_to_local_id.insert(recipe.uuid.clone(), new_id);
                recipes_imported += 1;
            }
        }

        // Step 4: Merge recipe ingredients
        let mut recipes_to_recompute: std::collections::HashSet<i64> =
            std::collections::HashSet::new();
        for ing in &data.recipe_ingredients {
            if ing.uuid.is_empty() {
                continue;
            }
            let local_recipe_id = if ing.recipe_uuid.is_empty() {
                None
            } else {
                recipe_uuid_to_local_id.get(&ing.recipe_uuid).copied()
            };
            let local_food_id = if ing.food_uuid.is_empty() {
                None
            } else {
                food_uuid_to_local_id.get(&ing.food_uuid).copied()
            };
            let (Some(recipe_id), Some(food_id)) = (local_recipe_id, local_food_id) else {
                continue;
            };

            if let Some(existing_id) = self.get_recipe_ingredient_by_uuid(&ing.uuid)? {
                self.conn.execute(
                    "UPDATE recipe_ingredients SET recipe_id=?1, food_id=?2, quantity_g=?3 WHERE id=?4",
                    params![recipe_id, food_id, ing.quantity_g, existing_id],
                )?;
                recipe_ingredients_imported += 1;
            } else {
                let now = Local::now().to_rfc3339();
                self.conn.execute(
                    "INSERT INTO recipe_ingredients (recipe_id, food_id, quantity_g, uuid, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![recipe_id, food_id, ing.quantity_g, ing.uuid, now],
                )?;
                recipe_ingredients_imported += 1;
            }
            recipes_to_recompute.insert(recipe_id);
        }

        // Recompute virtual foods for affected recipes
        for recipe_id in &recipes_to_recompute {
            self.recompute_recipe_food(*recipe_id)?;
        }

        // Step 5: Merge targets
        let mut targets_imported: i64 = 0;
        // Determine the list of targets to merge
        let targets_to_merge: Vec<ExportTarget> = if !data.targets.is_empty() {
            data.targets.clone()
        } else if let Some(legacy) = &data.target {
            // Legacy single target — expand to all 7 days
            (0..7_i64)
                .map(|day| ExportTarget {
                    day_of_week: day,
                    calories: legacy.calories,
                    protein_pct: legacy.protein_pct,
                    carbs_pct: legacy.carbs_pct,
                    fat_pct: legacy.fat_pct,
                    updated_at: legacy.updated_at.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };
        for incoming_target in &targets_to_merge {
            let local_updated: Option<String> = self
                .conn
                .query_row(
                    "SELECT updated_at FROM targets WHERE day_of_week = ?1",
                    params![incoming_target.day_of_week],
                    |row| row.get(0),
                )
                .ok();
            let should_update = match (&incoming_target.updated_at, &local_updated) {
                (Some(incoming), Some(local)) => incoming > local,
                (Some(_), None) | (None, _) => true,
            };
            if should_update {
                let updated_at = incoming_target
                    .updated_at
                    .clone()
                    .unwrap_or_else(|| Local::now().to_rfc3339());
                self.conn.execute(
                    "INSERT OR REPLACE INTO targets (day_of_week, calories, protein_pct, carbs_pct, fat_pct, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        incoming_target.day_of_week,
                        incoming_target.calories,
                        incoming_target.protein_pct,
                        incoming_target.carbs_pct,
                        incoming_target.fat_pct,
                        updated_at,
                    ],
                )?;
                targets_imported += 1;
            }
        }

        // Step 6: Process tombstones — delete local records if older than tombstone
        if let Some(tombstones) = &data.tombstones {
            for tombstone in tombstones {
                let deleted = self.apply_tombstone(tombstone, &mut recipes_to_recompute)?;
                if deleted {
                    tombstones_processed += 1;
                }
            }
        }

        // Step 7: Store incoming tombstones locally for propagation
        if let Some(tombstones) = &data.tombstones {
            for tombstone in tombstones {
                let exists: i64 = self
                    .conn
                    .query_row(
                        "SELECT COUNT(*) FROM sync_tombstones WHERE uuid = ?1 AND table_name = ?2",
                        params![tombstone.uuid, tombstone.table_name],
                        |row| row.get(0),
                    )
                    .unwrap_or(0);
                if exists == 0 {
                    self.conn.execute(
                        "INSERT INTO sync_tombstones (uuid, table_name, deleted_at) VALUES (?1, ?2, ?3)",
                        params![tombstone.uuid, tombstone.table_name, tombstone.deleted_at],
                    )?;
                }
            }
        }

        // Recompute any recipes affected by tombstone ingredient deletions
        for recipe_id in recipes_to_recompute {
            if self.get_recipe_by_id(recipe_id).is_ok() {
                self.recompute_recipe_food(recipe_id)?;
            }
        }

        // Step 8: Merge weight entries (LWW by date — newer updated_at wins)
        let mut weight_entries_imported: i64 = 0;
        for entry in &data.weight_entries {
            if entry.uuid.is_empty() {
                continue;
            }
            let existing: Option<(String, String)> = self
                .conn
                .query_row(
                    "SELECT uuid, updated_at FROM weight_entries WHERE date = ?1",
                    params![entry.date],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .ok();
            if let Some((_existing_uuid, existing_updated)) = existing {
                if entry.updated_at > existing_updated {
                    self.conn.execute(
                        "UPDATE weight_entries SET uuid=?1, weight_kg=?2, source=?3, notes=?4, updated_at=?5 WHERE date=?6",
                        params![entry.uuid, entry.weight_kg, entry.source, entry.notes, entry.updated_at, entry.date],
                    )?;
                    weight_entries_imported += 1;
                }
            } else {
                self.conn.execute(
                    "INSERT INTO weight_entries (uuid, date, weight_kg, source, notes, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![entry.uuid, entry.date, entry.weight_kg, entry.source, entry.notes, entry.created_at, entry.updated_at],
                )?;
                weight_entries_imported += 1;
            }
        }

        Ok(ImportSummary {
            foods_imported,
            meal_entries_imported,
            recipes_imported,
            recipe_ingredients_imported,
            targets_imported,
            weight_entries_imported,
            tombstones_processed,
        })
    }

    fn apply_tombstone(
        &self,
        tombstone: &SyncTombstone,
        recipes_to_recompute: &mut std::collections::HashSet<i64>,
    ) -> Result<bool> {
        match tombstone.table_name.as_str() {
            "foods" => {
                if let Some(food) = self.get_food_by_uuid(&tombstone.uuid)? {
                    if food.updated_at < tombstone.deleted_at {
                        self.conn.execute(
                            "DELETE FROM foods WHERE uuid = ?1",
                            params![tombstone.uuid],
                        )?;
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            "meal_entries" => {
                let local: Option<(i64, String)> = self
                    .conn
                    .query_row(
                        "SELECT id, COALESCE(updated_at, '') FROM meal_entries WHERE uuid = ?1",
                        params![tombstone.uuid],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .ok();
                if let Some((id, updated_at)) = local {
                    if updated_at < tombstone.deleted_at {
                        self.conn
                            .execute("DELETE FROM meal_entries WHERE id = ?1", params![id])?;
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            "recipes" => {
                if let Some(recipe) = self.get_recipe_by_uuid(&tombstone.uuid)? {
                    if recipe.updated_at < tombstone.deleted_at {
                        self.conn.execute(
                            "DELETE FROM recipe_ingredients WHERE recipe_id = ?1",
                            params![recipe.id],
                        )?;
                        self.conn
                            .execute("DELETE FROM recipes WHERE id = ?1", params![recipe.id])?;
                        self.conn
                            .execute("DELETE FROM foods WHERE id = ?1", params![recipe.food_id])?;
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            "recipe_ingredients" => {
                let local: Option<(i64, String, i64)> = self
                    .conn
                    .query_row(
                        "SELECT id, COALESCE(updated_at, ''), recipe_id FROM recipe_ingredients WHERE uuid = ?1",
                        params![tombstone.uuid],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .ok();
                if let Some((id, updated_at, recipe_id)) = local {
                    if updated_at < tombstone.deleted_at {
                        self.conn
                            .execute("DELETE FROM recipe_ingredients WHERE id = ?1", params![id])?;
                        recipes_to_recompute.insert(recipe_id);
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    // --- Weight Entries ---

    pub fn upsert_weight(&self, entry: &NewWeightEntry) -> Result<WeightEntry> {
        let now = Local::now().to_rfc3339();
        let uuid = Uuid::new_v4().to_string();
        let date_str = entry.date.format("%Y-%m-%d").to_string();
        self.conn.execute(
            "INSERT INTO weight_entries (uuid, date, weight_kg, source, notes, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(date) DO UPDATE SET
                weight_kg = excluded.weight_kg,
                source = excluded.source,
                notes = excluded.notes,
                updated_at = excluded.updated_at",
            params![uuid, date_str, entry.weight_kg, entry.source, entry.notes, now, now],
        )?;
        self.get_weight(entry.date)?
            .context("Weight entry not found after upsert")
    }

    pub fn get_weight(&self, date: NaiveDate) -> Result<Option<WeightEntry>> {
        let date_str = date.format("%Y-%m-%d").to_string();
        let mut stmt = self.conn.prepare(
            "SELECT id, uuid, date, weight_kg, source, notes, created_at, updated_at
             FROM weight_entries WHERE date = ?1",
        )?;
        let mut rows = stmt.query(params![date_str])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Self::weight_entry_from_row(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_weight_history(&self, days: Option<i64>) -> Result<Vec<WeightEntry>> {
        let query = match days {
            Some(n) => format!(
                "SELECT id, uuid, date, weight_kg, source, notes, created_at, updated_at
                 FROM weight_entries ORDER BY date DESC LIMIT {n}"
            ),
            None => "SELECT id, uuid, date, weight_kg, source, notes, created_at, updated_at
                     FROM weight_entries ORDER BY date DESC"
                .to_string(),
        };
        let mut stmt = self.conn.prepare(&query)?;
        let entries = stmt
            .query_map([], Self::weight_entry_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    pub fn delete_weight(&self, id: i64) -> Result<()> {
        let rows = self
            .conn
            .execute("DELETE FROM weight_entries WHERE id = ?1", params![id])?;
        if rows == 0 {
            anyhow::bail!("Weight entry not found");
        }
        Ok(())
    }

    fn weight_entry_from_row(row: &rusqlite::Row) -> rusqlite::Result<WeightEntry> {
        let date_str: String = row.get(2)?;
        let date = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
            .unwrap_or_else(|_| NaiveDate::from_ymd_opt(2000, 1, 1).expect("valid date"));
        Ok(WeightEntry {
            id: row.get(0)?,
            uuid: row.get(1)?,
            date,
            weight_kg: row.get(3)?,
            source: row.get(4)?,
            notes: row.get(5)?,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
        })
    }

    // --- UX Queries ---

    pub fn get_recently_logged_foods(&self, limit: i64) -> Result<Vec<RecentFood>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, f.name, f.brand, f.barcode, f.calories_per_100g,
                    f.protein_per_100g, f.carbs_per_100g, f.fat_per_100g,
                    f.default_serving_g, f.source, f.created_at, f.uuid, f.updated_at,
                    latest.last_serving_g, latest.last_meal_type,
                    counts.log_count, counts.last_date
             FROM foods f
             JOIN (
                 SELECT food_id, COUNT(*) as log_count, MAX(date) as last_date
                 FROM meal_entries
                 GROUP BY food_id
             ) counts ON f.id = counts.food_id
             JOIN (
                 SELECT me.food_id, me.serving_g as last_serving_g, me.meal_type as last_meal_type
                 FROM meal_entries me
                 INNER JOIN (
                     SELECT food_id, MAX(id) as max_id
                     FROM meal_entries
                     WHERE (food_id, date) IN (
                         SELECT food_id, MAX(date) FROM meal_entries GROUP BY food_id
                     )
                     GROUP BY food_id
                 ) latest_ids ON me.id = latest_ids.max_id
             ) latest ON f.id = latest.food_id
             ORDER BY counts.last_date DESC, counts.log_count DESC
             LIMIT ?1",
        )?;
        let foods = stmt
            .query_map(params![limit], |row| {
                let food = Self::food_from_row(row)?;
                Ok(RecentFood {
                    food,
                    last_serving_g: row.get(13)?,
                    last_meal_type: row.get(14)?,
                    log_count: row.get(15)?,
                    last_logged: row.get(16)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(foods)
    }

    pub fn get_logging_streak(&self, today: NaiveDate) -> Result<i64> {
        // Get distinct dates with meal entries, ordered DESC
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT date FROM meal_entries ORDER BY date DESC")?;
        let dates: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        if dates.is_empty() {
            return Ok(0);
        }

        let today_str = today.format("%Y-%m-%d").to_string();
        let yesterday = today - chrono::Duration::days(1);
        let yesterday_str = yesterday.format("%Y-%m-%d").to_string();

        // Determine starting point: today or yesterday
        let start_date = if dates.first().is_some_and(|d| d == &today_str) {
            today
        } else if dates.first().is_some_and(|d| d == &yesterday_str) {
            yesterday
        } else {
            return Ok(0);
        };

        let mut streak: i64 = 0;
        for date_str in &dates {
            let expected = (start_date - chrono::Duration::days(streak))
                .format("%Y-%m-%d")
                .to_string();
            if date_str == &expected {
                streak += 1;
            } else {
                break;
            }
        }

        Ok(streak)
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn get_calorie_average(&self, days: i64) -> Result<f64> {
        let today = Local::now().date_naive();
        let start_date = today - chrono::Duration::days(days - 1);
        let start_str = start_date.format("%Y-%m-%d").to_string();
        let end_str = today.format("%Y-%m-%d").to_string();

        let result: Option<f64> = self.conn.query_row(
            "SELECT AVG(daily_total) FROM (
                SELECT SUM(f.calories_per_100g * me.serving_g / 100.0) as daily_total
                FROM meal_entries me
                JOIN foods f ON me.food_id = f.id
                WHERE me.date >= ?1 AND me.date <= ?2
                GROUP BY me.date
            )",
            params![start_str, end_str],
            |row| row.get(0),
        )?;

        Ok(result.unwrap_or(0.0))
    }

    // --- Watch queries (Apple Watch / Wear OS) ---

    /// Build a compact glance for watch complications and tiles.
    #[allow(clippy::cast_precision_loss)]
    pub fn build_watch_glance(&self, date: NaiveDate) -> Result<crate::models::WatchGlance> {
        let date_str = date.format("%Y-%m-%d").to_string();

        // Totals for the day
        let (calories, protein, carbs, fat, meal_count): (f64, f64, f64, f64, i64) =
            self.conn.query_row(
                "SELECT COALESCE(SUM(f.calories_per_100g * me.serving_g / 100.0), 0),
                        COALESCE(SUM(COALESCE(f.protein_per_100g, 0) * me.serving_g / 100.0), 0),
                        COALESCE(SUM(COALESCE(f.carbs_per_100g, 0) * me.serving_g / 100.0), 0),
                        COALESCE(SUM(COALESCE(f.fat_per_100g, 0) * me.serving_g / 100.0), 0),
                        COUNT(*)
                 FROM meal_entries me
                 JOIN foods f ON me.food_id = f.id
                 WHERE me.date = ?1",
                params![date_str],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?;

        let day_of_week = i64::from(date.weekday().num_days_from_monday());
        let target = self.get_target(day_of_week)?;

        let calories_target = target.as_ref().map(|t| t.calories);
        let calories_remaining = calories_target.map(|t| t as f64 - calories);
        let protein_target_g = target.as_ref().and_then(|t| t.protein_g);
        let carbs_target_g = target.as_ref().and_then(|t| t.carbs_g);
        let fat_target_g = target.as_ref().and_then(|t| t.fat_g);

        let streak = self.get_logging_streak(date)?;

        Ok(crate::models::WatchGlance {
            date: date_str,
            calories_eaten: calories,
            calories_target,
            calories_remaining,
            protein_g: protein,
            carbs_g: carbs,
            fat_g: fat,
            protein_target_g,
            carbs_target_g,
            fat_target_g,
            meal_count,
            logging_streak: streak,
        })
    }

    /// Get recent foods in a compact format for quick re-logging on watch.
    pub fn get_watch_recent_foods(
        &self,
        limit: i64,
    ) -> Result<Vec<crate::models::WatchRecentFood>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, f.name, f.brand, f.calories_per_100g,
                    latest.last_serving_g, latest.last_meal_type
             FROM foods f
             JOIN (
                 SELECT food_id,
                        serving_g AS last_serving_g,
                        meal_type AS last_meal_type,
                        ROW_NUMBER() OVER (PARTITION BY food_id ORDER BY created_at DESC) AS rn
                 FROM meal_entries
             ) latest ON latest.food_id = f.id AND latest.rn = 1
             JOIN (
                 SELECT food_id, MAX(created_at) AS max_created
                 FROM meal_entries
                 GROUP BY food_id
             ) freq ON freq.food_id = f.id
             ORDER BY freq.max_created DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit], |row| {
            let food_id: i64 = row.get(0)?;
            let name: String = row.get(1)?;
            let brand: Option<String> = row.get(2)?;
            let calories_per_100g: f64 = row.get(3)?;
            let last_serving_g: f64 = row.get(4)?;
            let last_meal_type: String = row.get(5)?;
            let last_calories = calories_per_100g * last_serving_g / 100.0;
            Ok(crate::models::WatchRecentFood {
                food_id,
                name,
                brand,
                calories_per_100g,
                last_serving_g,
                last_meal_type,
                last_calories,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    // --- User Settings ---

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let now = Local::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO user_settings (key, value, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, now],
        )?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM user_settings WHERE key = ?1")?;
        let mut rows = stmt.query(params![key])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn delete_setting(&self, key: &str) -> Result<bool> {
        let rows = self
            .conn
            .execute("DELETE FROM user_settings WHERE key = ?1", params![key])?;
        Ok(rows > 0)
    }

    pub fn build_daily_summary(&self, date: NaiveDate) -> Result<DailySummary> {
        let entries = self.get_entries_for_date(date)?;
        let mut meals: Vec<MealGroup> = Vec::new();

        for meal_type in MEAL_TYPES {
            let meal_entries: Vec<MealEntry> = entries
                .iter()
                .filter(|e| e.meal_type == *meal_type)
                .cloned()
                .collect();

            if meal_entries.is_empty() {
                continue;
            }

            let subtotal_calories: f64 = meal_entries.iter().filter_map(|e| e.calories).sum();
            let subtotal_protein: f64 = meal_entries.iter().filter_map(|e| e.protein).sum();
            let subtotal_carbs: f64 = meal_entries.iter().filter_map(|e| e.carbs).sum();
            let subtotal_fat: f64 = meal_entries.iter().filter_map(|e| e.fat).sum();

            meals.push(MealGroup {
                meal_type: meal_type.to_string(),
                entries: meal_entries,
                subtotal_calories,
                subtotal_protein,
                subtotal_carbs,
                subtotal_fat,
            });
        }

        let total_calories: f64 = meals.iter().map(|m| m.subtotal_calories).sum();
        let total_protein: f64 = meals.iter().map(|m| m.subtotal_protein).sum();
        let total_carbs: f64 = meals.iter().map(|m| m.subtotal_carbs).sum();
        let total_fat: f64 = meals.iter().map(|m| m.subtotal_fat).sum();

        let day_of_week = i64::from(date.weekday().num_days_from_monday());
        let target = self.get_target(day_of_week)?;

        Ok(DailySummary {
            date: date.format("%Y-%m-%d").to_string(),
            meals,
            total_calories,
            total_protein,
            total_carbs,
            total_fat,
            target,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{NewFood, NewMealEntry, UpdateMealEntry};

    fn sample_food() -> NewFood {
        NewFood {
            name: "Chicken Breast".to_string(),
            brand: Some("Acme".to_string()),
            barcode: Some("1234567890".to_string()),
            calories_per_100g: 165.0,
            protein_per_100g: Some(31.0),
            carbs_per_100g: Some(0.0),
            fat_per_100g: Some(3.6),
            default_serving_g: Some(150.0),
            source: "manual".to_string(),
        }
    }

    #[test]
    fn test_insert_and_get_food() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();

        assert_eq!(food.name, "Chicken Breast");
        assert_eq!(food.brand.as_deref(), Some("Acme"));
        assert_eq!(food.barcode.as_deref(), Some("1234567890"));
        assert_eq!(food.calories_per_100g, 165.0);
        assert_eq!(food.protein_per_100g, Some(31.0));
        assert_eq!(food.source, "manual");

        let fetched = db.get_food_by_id(food.id).unwrap();
        assert_eq!(fetched.id, food.id);
        assert_eq!(fetched.name, "Chicken Breast");
    }

    #[test]
    fn test_upsert_food_by_barcode() {
        let db = Database::open_in_memory().unwrap();
        let food1 = db.upsert_food_by_barcode(&sample_food()).unwrap();
        let food2 = db.upsert_food_by_barcode(&sample_food()).unwrap();

        // Should return the same food (dedup by barcode)
        assert_eq!(food1.id, food2.id);
    }

    #[test]
    fn test_search_foods_local() {
        let db = Database::open_in_memory().unwrap();
        db.insert_food(&sample_food()).unwrap();
        db.insert_food(&NewFood {
            name: "Brown Rice".to_string(),
            brand: None,
            barcode: None,
            calories_per_100g: 112.0,
            protein_per_100g: Some(2.6),
            carbs_per_100g: Some(23.5),
            fat_per_100g: Some(0.9),
            default_serving_g: None,
            source: "manual".to_string(),
        })
        .unwrap();

        let results = db.search_foods_local("chicken").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Chicken Breast");

        let results = db.search_foods_local("rice").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Brown Rice");

        let results = db.search_foods_local("pizza").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_list_foods() {
        let db = Database::open_in_memory().unwrap();
        db.insert_food(&sample_food()).unwrap();
        db.insert_food(&NewFood {
            name: "Brown Rice".to_string(),
            brand: None,
            barcode: None,
            calories_per_100g: 112.0,
            protein_per_100g: None,
            carbs_per_100g: None,
            fat_per_100g: None,
            default_serving_g: None,
            source: "manual".to_string(),
        })
        .unwrap();

        // List all
        let all = db.list_foods(None).unwrap();
        assert_eq!(all.len(), 2);

        // List with filter
        let filtered = db.list_foods(Some("rice")).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "Brown Rice");
    }

    #[test]
    fn test_insert_and_get_meal_entry() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();

        let entry = db
            .insert_meal_entry(&NewMealEntry {
                date: NaiveDate::from_ymd_opt(2024, 6, 15).unwrap(),
                meal_type: "lunch".to_string(),
                food_id: food.id,
                serving_g: 200.0,
                display_unit: None,
                display_quantity: None,
            })
            .unwrap();

        assert_eq!(entry.meal_type, "lunch");
        assert_eq!(entry.serving_g, 200.0);
        assert_eq!(entry.food_name.as_deref(), Some("Chicken Breast"));
        // 165 cal/100g * 200g / 100 = 330 kcal
        let cal = entry.calories.unwrap();
        assert!((cal - 330.0).abs() < 0.01);
        // 31 protein/100g * 200/100 = 62
        let pro = entry.protein.unwrap();
        assert!((pro - 62.0).abs() < 0.01);
    }

    #[test]
    fn test_delete_meal_entry() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let entry = db
            .insert_meal_entry(&NewMealEntry {
                date: NaiveDate::from_ymd_opt(2024, 6, 15).unwrap(),
                meal_type: "lunch".to_string(),
                food_id: food.id,
                serving_g: 100.0,
                display_unit: None,
                display_quantity: None,
            })
            .unwrap();

        assert!(db.delete_meal_entry(entry.id).unwrap());
        // Deleting again should return false
        assert!(!db.delete_meal_entry(entry.id).unwrap());
    }

    #[test]
    fn test_get_entries_for_date() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let date1 = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
        let date2 = NaiveDate::from_ymd_opt(2024, 6, 16).unwrap();

        db.insert_meal_entry(&NewMealEntry {
            date: date1,
            meal_type: "breakfast".to_string(),
            food_id: food.id,
            serving_g: 100.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();
        db.insert_meal_entry(&NewMealEntry {
            date: date2,
            meal_type: "lunch".to_string(),
            food_id: food.id,
            serving_g: 150.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();

        let entries = db.get_entries_for_date(date1).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].meal_type, "breakfast");

        let entries = db.get_entries_for_date(date2).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].meal_type, "lunch");
    }

    #[test]
    fn test_get_entries_for_date_and_meal() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let date = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();

        db.insert_meal_entry(&NewMealEntry {
            date,
            meal_type: "breakfast".to_string(),
            food_id: food.id,
            serving_g: 100.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();
        db.insert_meal_entry(&NewMealEntry {
            date,
            meal_type: "lunch".to_string(),
            food_id: food.id,
            serving_g: 200.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();

        let breakfast = db.get_entries_for_date_and_meal(date, "breakfast").unwrap();
        assert_eq!(breakfast.len(), 1);
        assert_eq!(breakfast[0].serving_g, 100.0);

        let lunch = db.get_entries_for_date_and_meal(date, "lunch").unwrap();
        assert_eq!(lunch.len(), 1);
        assert_eq!(lunch[0].serving_g, 200.0);

        let dinner = db.get_entries_for_date_and_meal(date, "dinner").unwrap();
        assert!(dinner.is_empty());
    }

    #[test]
    fn test_build_daily_summary() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let date = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();

        // Breakfast: 100g -> 165 kcal
        db.insert_meal_entry(&NewMealEntry {
            date,
            meal_type: "breakfast".to_string(),
            food_id: food.id,
            serving_g: 100.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();
        // Lunch: 200g -> 330 kcal
        db.insert_meal_entry(&NewMealEntry {
            date,
            meal_type: "lunch".to_string(),
            food_id: food.id,
            serving_g: 200.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();

        let summary = db.build_daily_summary(date).unwrap();
        assert_eq!(summary.meals.len(), 2);
        assert_eq!(summary.meals[0].meal_type, "breakfast");
        assert_eq!(summary.meals[1].meal_type, "lunch");
        assert!((summary.meals[0].subtotal_calories - 165.0).abs() < 0.01);
        assert!((summary.meals[1].subtotal_calories - 330.0).abs() < 0.01);
        assert!((summary.total_calories - 495.0).abs() < 0.01);
        assert!((summary.total_protein - 93.0).abs() < 0.01); // 31*1 + 31*2
    }

    #[test]
    fn test_build_daily_summary_empty() {
        let db = Database::open_in_memory().unwrap();
        let date = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();

        let summary = db.build_daily_summary(date).unwrap();
        assert!(summary.meals.is_empty());
        assert_eq!(summary.total_calories, 0.0);
        assert_eq!(summary.total_protein, 0.0);
        assert_eq!(summary.total_carbs, 0.0);
        assert_eq!(summary.total_fat, 0.0);
        assert!(summary.target.is_none());
    }

    #[test]
    fn test_set_and_get_target() {
        let db = Database::open_in_memory().unwrap();

        // No target initially
        assert!(db.get_target(0).unwrap().is_none());

        // Set target with macros for Monday (0)
        let target = db
            .set_target(0, 1800, Some(40), Some(30), Some(30))
            .unwrap();
        assert_eq!(target.day_of_week, 0);
        assert_eq!(target.calories, 1800);
        assert_eq!(target.protein_pct, Some(40));
        assert!((target.protein_g.unwrap() - 180.0).abs() < 0.01);
        assert!((target.carbs_g.unwrap() - 135.0).abs() < 0.01);
        assert!((target.fat_g.unwrap() - 60.0).abs() < 0.01);

        // Read it back
        let fetched = db.get_target(0).unwrap().unwrap();
        assert_eq!(fetched.calories, 1800);
        assert_eq!(fetched.protein_pct, Some(40));

        // Different day should have no target
        assert!(db.get_target(1).unwrap().is_none());

        // Set a different day
        let sat = db.set_target(5, 2200, None, None, None).unwrap();
        assert_eq!(sat.day_of_week, 5);
        assert_eq!(sat.calories, 2200);

        // Update Monday (replace)
        let updated = db.set_target(0, 2000, None, None, None).unwrap();
        assert_eq!(updated.calories, 2000);
        assert!(updated.protein_pct.is_none());

        // Monday should be updated
        let fetched = db.get_target(0).unwrap().unwrap();
        assert_eq!(fetched.calories, 2000);

        // get_all_targets should return both
        let all = db.get_all_targets().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].day_of_week, 0);
        assert_eq!(all[1].day_of_week, 5);
    }

    #[test]
    fn test_clear_target() {
        let db = Database::open_in_memory().unwrap();

        // Clear when nothing set
        assert!(!db.clear_target(0).unwrap());

        // Set targets for Mon and Tue
        db.set_target(0, 1800, None, None, None).unwrap();
        db.set_target(1, 1900, None, None, None).unwrap();
        assert!(db.get_target(0).unwrap().is_some());
        assert!(db.get_target(1).unwrap().is_some());

        // Clear Monday only
        assert!(db.clear_target(0).unwrap());
        assert!(db.get_target(0).unwrap().is_none());
        assert!(db.get_target(1).unwrap().is_some());

        // Clear all
        db.set_target(0, 1800, None, None, None).unwrap();
        assert!(db.clear_all_targets().unwrap());
        assert!(db.get_all_targets().unwrap().is_empty());

        // Clear all when empty
        assert!(!db.clear_all_targets().unwrap());
    }

    #[test]
    fn test_summary_includes_target() {
        let db = Database::open_in_memory().unwrap();
        // 2024-06-15 is a Saturday = day_of_week 5
        let date = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();

        // No target
        let summary = db.build_daily_summary(date).unwrap();
        assert!(summary.target.is_none());

        // Set target for Saturday (5)
        db.set_target(5, 1800, Some(40), Some(30), Some(30))
            .unwrap();
        let summary = db.build_daily_summary(date).unwrap();
        let target = summary.target.unwrap();
        assert_eq!(target.calories, 1800);
        assert_eq!(target.day_of_week, 5);
        assert!((target.protein_g.unwrap() - 180.0).abs() < 0.01);

        // Monday target should NOT appear for Saturday
        db.set_target(0, 2500, None, None, None).unwrap();
        let summary = db.build_daily_summary(date).unwrap();
        let target = summary.target.unwrap();
        assert_eq!(target.calories, 1800); // still Saturday's target
    }

    #[test]
    fn test_update_meal_entry_serving() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let entry = db
            .insert_meal_entry(&NewMealEntry {
                date: NaiveDate::from_ymd_opt(2024, 6, 15).unwrap(),
                meal_type: "lunch".to_string(),
                food_id: food.id,
                serving_g: 100.0,
                display_unit: None,
                display_quantity: None,
            })
            .unwrap();

        let updated = db
            .update_meal_entry(
                entry.id,
                &UpdateMealEntry {
                    serving_g: Some(250.0),
                    meal_type: None,
                    date: None,
                    display_unit: None,
                    display_quantity: None,
                },
            )
            .unwrap();

        assert_eq!(updated.serving_g, 250.0);
        assert_eq!(updated.meal_type, "lunch");
        // 165 * 250 / 100 = 412.5
        assert!((updated.calories.unwrap() - 412.5).abs() < 0.01);
    }

    #[test]
    fn test_update_meal_entry_meal_type() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let entry = db
            .insert_meal_entry(&NewMealEntry {
                date: NaiveDate::from_ymd_opt(2024, 6, 15).unwrap(),
                meal_type: "lunch".to_string(),
                food_id: food.id,
                serving_g: 100.0,
                display_unit: None,
                display_quantity: None,
            })
            .unwrap();

        let updated = db
            .update_meal_entry(
                entry.id,
                &UpdateMealEntry {
                    serving_g: None,
                    meal_type: Some("dinner".to_string()),
                    date: None,
                    display_unit: None,
                    display_quantity: None,
                },
            )
            .unwrap();

        assert_eq!(updated.meal_type, "dinner");
        assert_eq!(updated.serving_g, 100.0);
    }

    #[test]
    fn test_update_meal_entry_not_found() {
        let db = Database::open_in_memory().unwrap();
        let result = db.update_meal_entry(
            999,
            &UpdateMealEntry {
                serving_g: Some(100.0),
                meal_type: None,
                date: None,
                display_unit: None,
                display_quantity: None,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_update_meal_entry_noop() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let entry = db
            .insert_meal_entry(&NewMealEntry {
                date: NaiveDate::from_ymd_opt(2024, 6, 15).unwrap(),
                meal_type: "lunch".to_string(),
                food_id: food.id,
                serving_g: 100.0,
                display_unit: None,
                display_quantity: None,
            })
            .unwrap();

        let updated = db
            .update_meal_entry(
                entry.id,
                &UpdateMealEntry {
                    serving_g: None,
                    meal_type: None,
                    date: None,
                    display_unit: None,
                    display_quantity: None,
                },
            )
            .unwrap();

        assert_eq!(updated.serving_g, 100.0);
        assert_eq!(updated.meal_type, "lunch");
    }

    // --- Recipe tests ---

    fn sample_ingredient_rice() -> NewFood {
        NewFood {
            name: "Brown Rice".to_string(),
            brand: None,
            barcode: None,
            calories_per_100g: 112.0,
            protein_per_100g: Some(2.6),
            carbs_per_100g: Some(23.5),
            fat_per_100g: Some(0.9),
            default_serving_g: None,
            source: "manual".to_string(),
        }
    }

    #[test]
    fn test_create_recipe() {
        let db = Database::open_in_memory().unwrap();
        let recipe = db.create_recipe("Chicken and Rice", 4.0).unwrap();
        assert_eq!(recipe.portions, 4.0);

        // Virtual food should exist
        let food = db.get_food_by_id(recipe.food_id).unwrap();
        assert_eq!(food.name, "Chicken and Rice");
        assert_eq!(food.source, "recipe");
    }

    #[test]
    fn test_recipe_add_ingredient_recomputes() {
        let db = Database::open_in_memory().unwrap();
        let chicken = db.insert_food(&sample_food()).unwrap();
        let rice = db.insert_food(&sample_ingredient_rice()).unwrap();
        let recipe = db.create_recipe("Chicken and Rice", 2.0).unwrap();

        // Add 200g chicken: 165 cal/100g -> 330 cal total
        db.add_recipe_ingredient(recipe.id, chicken.id, 200.0)
            .unwrap();
        // Add 300g rice: 112 cal/100g -> 336 cal total
        db.add_recipe_ingredient(recipe.id, rice.id, 300.0).unwrap();

        let detail = db.get_recipe_detail(recipe.id).unwrap();
        assert_eq!(detail.ingredients.len(), 2);
        assert!((detail.total_weight_g - 500.0).abs() < 0.01);
        assert!((detail.per_portion_g - 250.0).abs() < 0.01);

        // Total cal = 330 + 336 = 666
        let expected_total_cal = 330.0 + 336.0;
        let expected_per_portion_cal = expected_total_cal / 2.0;
        assert!((detail.per_portion_calories - expected_per_portion_cal).abs() < 0.01);

        // Virtual food per-100g should be recomputed
        let food = db.get_food_by_id(recipe.food_id).unwrap();
        let expected_cal_100 = expected_total_cal * 100.0 / 500.0;
        assert!((food.calories_per_100g - expected_cal_100).abs() < 0.01);
        // default_serving_g = total_weight / portions = 500/2 = 250
        assert!((food.default_serving_g.unwrap() - 250.0).abs() < 0.01);
    }

    #[test]
    fn test_recipe_set_portions() {
        let db = Database::open_in_memory().unwrap();
        let chicken = db.insert_food(&sample_food()).unwrap();
        let recipe = db.create_recipe("Just Chicken", 2.0).unwrap();
        db.add_recipe_ingredient(recipe.id, chicken.id, 400.0)
            .unwrap();

        // Change to 4 portions
        db.set_recipe_portions(recipe.id, 4.0).unwrap();

        let food = db.get_food_by_id(recipe.food_id).unwrap();
        // default_serving_g = 400 / 4 = 100
        assert!((food.default_serving_g.unwrap() - 100.0).abs() < 0.01);
        // cal per 100g stays the same
        assert!((food.calories_per_100g - 165.0).abs() < 0.01);
    }

    #[test]
    fn test_recipe_remove_ingredient() {
        let db = Database::open_in_memory().unwrap();
        let chicken = db.insert_food(&sample_food()).unwrap();
        let rice = db.insert_food(&sample_ingredient_rice()).unwrap();
        let recipe = db.create_recipe("Mixed", 1.0).unwrap();
        db.add_recipe_ingredient(recipe.id, chicken.id, 100.0)
            .unwrap();
        db.add_recipe_ingredient(recipe.id, rice.id, 100.0).unwrap();

        assert!(
            db.remove_recipe_ingredient(recipe.id, "Brown Rice")
                .unwrap()
        );
        let detail = db.get_recipe_detail(recipe.id).unwrap();
        assert_eq!(detail.ingredients.len(), 1);
        assert!((detail.total_weight_g - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_recipe_log_as_food() {
        let db = Database::open_in_memory().unwrap();
        let chicken = db.insert_food(&sample_food()).unwrap();
        let recipe = db.create_recipe("Meal Prep Chicken", 4.0).unwrap();
        db.add_recipe_ingredient(recipe.id, chicken.id, 800.0)
            .unwrap();

        // Log one portion as a meal
        let date = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
        let food = db.get_food_by_id(recipe.food_id).unwrap();
        let serving = food.default_serving_g.unwrap(); // 800/4 = 200g
        assert!((serving - 200.0).abs() < 0.01);

        let entry = db
            .insert_meal_entry(&NewMealEntry {
                date,
                meal_type: "dinner".to_string(),
                food_id: recipe.food_id,
                serving_g: serving,
                display_unit: None,
                display_quantity: None,
            })
            .unwrap();

        // 165 cal/100g * 200g / 100 = 330 kcal
        assert!((entry.calories.unwrap() - 330.0).abs() < 0.01);

        // Verify daily summary includes it
        let summary = db.build_daily_summary(date).unwrap();
        assert!((summary.total_calories - 330.0).abs() < 0.01);
    }

    #[test]
    fn test_delete_recipe() {
        let db = Database::open_in_memory().unwrap();
        let chicken = db.insert_food(&sample_food()).unwrap();
        let recipe = db.create_recipe("To Delete", 1.0).unwrap();
        db.add_recipe_ingredient(recipe.id, chicken.id, 100.0)
            .unwrap();
        let food_id = recipe.food_id;

        db.delete_recipe(recipe.id).unwrap();
        // Virtual food should be gone
        assert!(db.get_food_by_id(food_id).is_err());
        // Recipe should be gone
        assert!(db.get_recipe_by_id(recipe.id).is_err());
    }

    #[test]
    fn test_list_recipes() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.list_recipes().unwrap().is_empty());

        db.create_recipe("Recipe A", 2.0).unwrap();
        db.create_recipe("Recipe B", 4.0).unwrap();
        let recipes = db.list_recipes().unwrap();
        assert_eq!(recipes.len(), 2);
    }

    // --- Export / Import tests ---

    #[test]
    fn test_export_all_empty() {
        let db = Database::open_in_memory().unwrap();
        let export = db.export_all().unwrap();
        assert_eq!(export.version, 3);
        assert!(export.device_id.is_some());
        assert!(export.foods.is_empty());
        assert!(export.meal_entries.is_empty());
        assert!(export.recipes.is_empty());
        assert!(export.recipe_ingredients.is_empty());
        assert!(export.targets.is_empty());
    }

    #[test]
    fn test_export_all_with_data() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let date = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
        db.insert_meal_entry(&NewMealEntry {
            date,
            meal_type: "lunch".to_string(),
            food_id: food.id,
            serving_g: 200.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();
        db.set_target(0, 2000, Some(30), Some(40), Some(30))
            .unwrap();

        let export = db.export_all().unwrap();
        assert_eq!(export.foods.len(), 1);
        assert_eq!(export.meal_entries.len(), 1);
        assert_eq!(export.targets.len(), 1);
        assert_eq!(export.targets[0].calories, 2000);
        assert_eq!(export.targets[0].day_of_week, 0);
    }

    #[test]
    fn test_import_into_empty_db() {
        let db = Database::open_in_memory().unwrap();

        // Create export data from another db
        let source_db = Database::open_in_memory().unwrap();
        let food = source_db.insert_food(&sample_food()).unwrap();
        let date = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
        source_db
            .insert_meal_entry(&NewMealEntry {
                date,
                meal_type: "lunch".to_string(),
                food_id: food.id,
                serving_g: 200.0,
                display_unit: None,
                display_quantity: None,
            })
            .unwrap();
        source_db
            .set_target(0, 2000, Some(30), Some(40), Some(30))
            .unwrap();

        let export = source_db.export_all().unwrap();
        let summary = db.import_all(&export).unwrap();

        assert_eq!(summary.foods_imported, 1);
        assert_eq!(summary.meal_entries_imported, 1);
        assert_eq!(summary.targets_imported, 1);

        // Verify data was imported
        let imported_food = db.get_food_by_id(food.id).unwrap();
        assert_eq!(imported_food.name, "Chicken Breast");
        let target = db.get_target(0).unwrap().unwrap();
        assert_eq!(target.calories, 2000);
    }

    #[test]
    fn test_import_upsert_existing() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();

        // Create export with updated food name and bumped updated_at
        let export = db.export_all().unwrap();
        let mut modified = export;
        modified.foods[0].name = "Updated Chicken".to_string();
        modified.foods[0].updated_at = "2099-01-01T00:00:00+00:00".to_string();

        let summary = db.import_all(&modified).unwrap();
        assert_eq!(summary.foods_imported, 1);

        let updated_food = db.get_food_by_id(food.id).unwrap();
        assert_eq!(updated_food.name, "Updated Chicken");
    }

    #[test]
    fn test_get_recipe_by_food_name() {
        let db = Database::open_in_memory().unwrap();
        let recipe = db.create_recipe("My Stew", 3.0).unwrap();

        // Case-insensitive lookup
        let found = db.get_recipe_by_food_name("my stew").unwrap();
        assert_eq!(found.id, recipe.id);
        let found = db.get_recipe_by_food_name("MY STEW").unwrap();
        assert_eq!(found.id, recipe.id);

        // Not found
        assert!(db.get_recipe_by_food_name("nonexistent").is_err());
    }

    // --- v2 schema / sync tests ---

    #[test]
    fn test_insert_food_generates_uuid() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        assert!(!food.uuid.is_empty());
        assert!(!food.updated_at.is_empty());
        // UUID should be valid v4 format
        assert!(uuid::Uuid::parse_str(&food.uuid).is_ok());
    }

    #[test]
    fn test_insert_meal_generates_uuid() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let entry = db
            .insert_meal_entry(&NewMealEntry {
                date: NaiveDate::from_ymd_opt(2024, 6, 15).unwrap(),
                meal_type: "lunch".to_string(),
                food_id: food.id,
                serving_g: 200.0,
                display_unit: None,
                display_quantity: None,
            })
            .unwrap();
        assert!(!entry.uuid.is_empty());
        assert!(!entry.updated_at.is_empty());
        assert!(uuid::Uuid::parse_str(&entry.uuid).is_ok());
    }

    #[test]
    fn test_update_meal_updates_timestamp() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let entry = db
            .insert_meal_entry(&NewMealEntry {
                date: NaiveDate::from_ymd_opt(2024, 6, 15).unwrap(),
                meal_type: "lunch".to_string(),
                food_id: food.id,
                serving_g: 100.0,
                display_unit: None,
                display_quantity: None,
            })
            .unwrap();
        let original_updated = entry.updated_at.clone();

        // Small delay to ensure different timestamp
        std::thread::sleep(std::time::Duration::from_millis(10));

        let updated = db
            .update_meal_entry(
                entry.id,
                &UpdateMealEntry {
                    serving_g: Some(250.0),
                    meal_type: None,
                    date: None,
                    display_unit: None,
                    display_quantity: None,
                },
            )
            .unwrap();
        assert!(updated.updated_at >= original_updated);
        assert_eq!(updated.uuid, entry.uuid); // UUID should not change
    }

    #[test]
    fn test_merge_foods_new() {
        let db = Database::open_in_memory().unwrap();
        let incoming_uuid = Uuid::new_v4().to_string();
        let now = Local::now().to_rfc3339();

        let import_data = ExportData {
            version: 2,
            exported_at: now.clone(),
            device_id: Some("other-device".to_string()),
            foods: vec![Food {
                id: 999,
                uuid: incoming_uuid.clone(),
                name: "Remote Food".to_string(),
                brand: None,
                barcode: None,
                calories_per_100g: 100.0,
                protein_per_100g: Some(10.0),
                carbs_per_100g: Some(20.0),
                fat_per_100g: Some(5.0),
                default_serving_g: None,
                source: "manual".to_string(),
                created_at: now.clone(),
                updated_at: now,
            }],
            meal_entries: vec![],
            recipes: vec![],
            recipe_ingredients: vec![],
            target: None,
            targets: vec![],
            weight_entries: vec![],
            tombstones: None,
        };

        let summary = db.import_all(&import_data).unwrap();
        assert_eq!(summary.foods_imported, 1);

        // Should be findable by UUID
        let found = db.get_food_by_uuid(&incoming_uuid).unwrap().unwrap();
        assert_eq!(found.name, "Remote Food");
    }

    #[test]
    fn test_merge_foods_newer_wins() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();

        let import_data = ExportData {
            version: 2,
            exported_at: Local::now().to_rfc3339(),
            device_id: Some("other-device".to_string()),
            foods: vec![Food {
                id: 999,
                uuid: food.uuid.clone(),
                name: "Updated Name".to_string(),
                brand: None,
                barcode: None,
                calories_per_100g: 200.0,
                protein_per_100g: Some(20.0),
                carbs_per_100g: Some(10.0),
                fat_per_100g: Some(5.0),
                default_serving_g: None,
                source: "manual".to_string(),
                created_at: food.created_at.clone(),
                updated_at: "2099-01-01T00:00:00+00:00".to_string(),
            }],
            meal_entries: vec![],
            recipes: vec![],
            recipe_ingredients: vec![],
            target: None,
            targets: vec![],
            weight_entries: vec![],
            tombstones: None,
        };

        let summary = db.import_all(&import_data).unwrap();
        assert_eq!(summary.foods_imported, 1);

        let updated = db.get_food_by_id(food.id).unwrap();
        assert_eq!(updated.name, "Updated Name");
        assert_eq!(updated.calories_per_100g, 200.0);
    }

    #[test]
    fn test_merge_foods_older_skipped() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();

        let import_data = ExportData {
            version: 2,
            exported_at: Local::now().to_rfc3339(),
            device_id: Some("other-device".to_string()),
            foods: vec![Food {
                id: 999,
                uuid: food.uuid.clone(),
                name: "Should Not Update".to_string(),
                brand: None,
                barcode: None,
                calories_per_100g: 200.0,
                protein_per_100g: None,
                carbs_per_100g: None,
                fat_per_100g: None,
                default_serving_g: None,
                source: "manual".to_string(),
                created_at: food.created_at.clone(),
                updated_at: "2000-01-01T00:00:00+00:00".to_string(),
            }],
            meal_entries: vec![],
            recipes: vec![],
            recipe_ingredients: vec![],
            target: None,
            targets: vec![],
            weight_entries: vec![],
            tombstones: None,
        };

        let summary = db.import_all(&import_data).unwrap();
        assert_eq!(summary.foods_imported, 0);

        let unchanged = db.get_food_by_id(food.id).unwrap();
        assert_eq!(unchanged.name, "Chicken Breast");
    }

    #[test]
    fn test_merge_meal_entries() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let entry_uuid = Uuid::new_v4().to_string();

        let import_data = ExportData {
            version: 2,
            exported_at: Local::now().to_rfc3339(),
            device_id: Some("other-device".to_string()),
            foods: vec![food.clone()],
            meal_entries: vec![ExportMealEntry {
                id: 999,
                uuid: entry_uuid.clone(),
                date: "2024-06-15".to_string(),
                meal_type: "lunch".to_string(),
                food_id: 999,
                food_uuid: food.uuid.clone(),
                serving_g: 200.0,
                display_unit: None,
                display_quantity: None,
                created_at: Local::now().to_rfc3339(),
                updated_at: Local::now().to_rfc3339(),
            }],
            recipes: vec![],
            recipe_ingredients: vec![],
            target: None,
            targets: vec![],
            weight_entries: vec![],
            tombstones: None,
        };

        let summary = db.import_all(&import_data).unwrap();
        assert_eq!(summary.meal_entries_imported, 1);

        // Verify the entry exists by checking entries for the date
        let entries = db
            .get_entries_for_date(NaiveDate::from_ymd_opt(2024, 6, 15).unwrap())
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].uuid, entry_uuid);
        assert_eq!(entries[0].serving_g, 200.0);
    }

    #[test]
    fn test_merge_recipes() {
        let db = Database::open_in_memory().unwrap();
        let chicken = db.insert_food(&sample_food()).unwrap();

        // Create a virtual food for the recipe
        let recipe_food = db
            .insert_food(&NewFood {
                name: "Remote Recipe".to_string(),
                brand: None,
                barcode: None,
                calories_per_100g: 150.0,
                protein_per_100g: Some(20.0),
                carbs_per_100g: Some(10.0),
                fat_per_100g: Some(5.0),
                default_serving_g: Some(200.0),
                source: "recipe".to_string(),
            })
            .unwrap();

        let recipe_uuid = Uuid::new_v4().to_string();
        let ing_uuid = Uuid::new_v4().to_string();
        let now = Local::now().to_rfc3339();

        let import_data = ExportData {
            version: 2,
            exported_at: now.clone(),
            device_id: Some("other-device".to_string()),
            foods: vec![chicken.clone(), recipe_food.clone()],
            meal_entries: vec![],
            recipes: vec![ExportRecipe {
                id: 999,
                uuid: recipe_uuid.clone(),
                food_id: 999,
                food_uuid: recipe_food.uuid.clone(),
                portions: 4.0,
                created_at: now.clone(),
                updated_at: now.clone(),
            }],
            recipe_ingredients: vec![ExportRecipeIngredient {
                id: 999,
                uuid: ing_uuid,
                recipe_id: 999,
                recipe_uuid: recipe_uuid.clone(),
                food_id: 999,
                food_uuid: chicken.uuid.clone(),
                quantity_g: 400.0,
            }],
            target: None,
            targets: vec![],
            weight_entries: vec![],
            tombstones: None,
        };

        let summary = db.import_all(&import_data).unwrap();
        assert_eq!(summary.recipes_imported, 1);
        assert_eq!(summary.recipe_ingredients_imported, 1);
    }

    #[test]
    fn test_merge_tombstones() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let entry = db
            .insert_meal_entry(&NewMealEntry {
                date: NaiveDate::from_ymd_opt(2024, 6, 15).unwrap(),
                meal_type: "lunch".to_string(),
                food_id: food.id,
                serving_g: 200.0,
                display_unit: None,
                display_quantity: None,
            })
            .unwrap();

        let import_data = ExportData {
            version: 2,
            exported_at: Local::now().to_rfc3339(),
            device_id: Some("other-device".to_string()),
            foods: vec![],
            meal_entries: vec![],
            recipes: vec![],
            recipe_ingredients: vec![],
            target: None,
            targets: vec![],
            weight_entries: vec![],
            tombstones: Some(vec![SyncTombstone {
                uuid: entry.uuid.clone(),
                table_name: "meal_entries".to_string(),
                deleted_at: "2099-01-01T00:00:00+00:00".to_string(),
            }]),
        };

        let summary = db.import_all(&import_data).unwrap();
        assert_eq!(summary.tombstones_processed, 1);

        // Entry should be deleted
        let entries = db
            .get_entries_for_date(NaiveDate::from_ymd_opt(2024, 6, 15).unwrap())
            .unwrap();
        assert!(entries.is_empty());

        // Tombstone should be stored locally
        let tombstones = db.get_tombstones().unwrap();
        assert_eq!(tombstones.len(), 1);
        assert_eq!(tombstones[0].uuid, entry.uuid);
    }

    #[test]
    fn test_merge_tombstone_older_than_record() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let entry = db
            .insert_meal_entry(&NewMealEntry {
                date: NaiveDate::from_ymd_opt(2024, 6, 15).unwrap(),
                meal_type: "lunch".to_string(),
                food_id: food.id,
                serving_g: 200.0,
                display_unit: None,
                display_quantity: None,
            })
            .unwrap();

        // Tombstone has an old deleted_at — record should survive
        let import_data = ExportData {
            version: 2,
            exported_at: Local::now().to_rfc3339(),
            device_id: Some("other-device".to_string()),
            foods: vec![],
            meal_entries: vec![],
            recipes: vec![],
            recipe_ingredients: vec![],
            target: None,
            targets: vec![],
            weight_entries: vec![],
            tombstones: Some(vec![SyncTombstone {
                uuid: entry.uuid.clone(),
                table_name: "meal_entries".to_string(),
                deleted_at: "2000-01-01T00:00:00+00:00".to_string(),
            }]),
        };

        let summary = db.import_all(&import_data).unwrap();
        assert_eq!(summary.tombstones_processed, 0);

        // Entry should still exist
        let entries = db
            .get_entries_for_date(NaiveDate::from_ymd_opt(2024, 6, 15).unwrap())
            .unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_v1_import_still_works() {
        let db = Database::open_in_memory().unwrap();

        // Create a v1 export data (no UUIDs)
        let v1_data = ExportData {
            version: 1,
            exported_at: Local::now().to_rfc3339(),
            device_id: None,
            foods: vec![Food {
                id: 1,
                uuid: String::new(),
                name: "V1 Food".to_string(),
                brand: None,
                barcode: None,
                calories_per_100g: 100.0,
                protein_per_100g: None,
                carbs_per_100g: None,
                fat_per_100g: None,
                default_serving_g: None,
                source: "manual".to_string(),
                created_at: Local::now().to_rfc3339(),
                updated_at: String::new(),
            }],
            meal_entries: vec![],
            recipes: vec![],
            recipe_ingredients: vec![],
            target: None,
            targets: vec![],
            weight_entries: vec![],
            tombstones: None,
        };

        let summary = db.import_all(&v1_data).unwrap();
        assert_eq!(summary.foods_imported, 1);
        assert_eq!(summary.tombstones_processed, 0);

        let food = db.get_food_by_id(1).unwrap();
        assert_eq!(food.name, "V1 Food");
    }

    #[test]
    fn test_device_id_persistence() {
        let db = Database::open_in_memory().unwrap();
        let id1 = db.get_or_create_device_id().unwrap();
        let id2 = db.get_or_create_device_id().unwrap();
        assert_eq!(id1, id2);
        assert!(uuid::Uuid::parse_str(&id1).is_ok());
    }

    #[test]
    fn test_export_v2_format() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let date = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
        db.insert_meal_entry(&NewMealEntry {
            date,
            meal_type: "lunch".to_string(),
            food_id: food.id,
            serving_g: 200.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();

        let export = db.export_all().unwrap();
        assert_eq!(export.version, 3);
        assert!(export.device_id.is_some());
        assert!(!export.foods[0].uuid.is_empty());
        assert!(!export.foods[0].updated_at.is_empty());
        assert!(!export.meal_entries[0].uuid.is_empty());
        assert!(!export.meal_entries[0].food_uuid.is_empty());
        assert_eq!(export.meal_entries[0].food_uuid, food.uuid);
        assert!(export.tombstones.is_some());
    }

    #[test]
    fn test_migration_v2_generates_uuids() {
        // Simulate a v1 database by creating one, then inserting data at v1 level
        // Since open_in_memory runs migrate() which goes all the way to v2,
        // we verify that data inserted after migration has UUIDs
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        assert!(!food.uuid.is_empty());

        let date = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
        let entry = db
            .insert_meal_entry(&NewMealEntry {
                date,
                meal_type: "lunch".to_string(),
                food_id: food.id,
                serving_g: 100.0,
                display_unit: None,
                display_quantity: None,
            })
            .unwrap();
        assert!(!entry.uuid.is_empty());

        let recipe = db.create_recipe("Test Recipe", 2.0).unwrap();
        assert!(!recipe.uuid.is_empty());
    }

    #[test]
    fn test_tombstone_crud() {
        let db = Database::open_in_memory().unwrap();

        // Initially empty
        assert!(db.get_tombstones().unwrap().is_empty());

        // Record tombstones
        db.record_tombstone("uuid-1", "foods").unwrap();
        db.record_tombstone("uuid-2", "meal_entries").unwrap();

        let tombstones = db.get_tombstones().unwrap();
        assert_eq!(tombstones.len(), 2);

        // Clear
        db.clear_tombstones().unwrap();
        assert!(db.get_tombstones().unwrap().is_empty());
    }

    // --- Delta sync tests ---

    #[test]
    fn test_get_foods_since() {
        let db = Database::open_in_memory().unwrap();

        // Insert two foods
        let food1 = db.insert_food(&sample_food()).unwrap();
        let food2 = db
            .insert_food(&NewFood {
                name: "Brown Rice".to_string(),
                brand: None,
                barcode: None,
                calories_per_100g: 112.0,
                protein_per_100g: Some(2.6),
                carbs_per_100g: Some(23.5),
                fat_per_100g: Some(0.9),
                default_serving_g: None,
                source: "manual".to_string(),
            })
            .unwrap();

        // All foods since epoch should return both
        let all = db.get_foods_since("1970-01-01T00:00:00+00:00").unwrap();
        assert_eq!(all.len(), 2);

        // Foods since a future time should return none
        let none = db.get_foods_since("2099-01-01T00:00:00+00:00").unwrap();
        assert!(none.is_empty());

        // get_all_foods should return both
        let all_foods = db.get_all_foods().unwrap();
        assert_eq!(all_foods.len(), 2);
        assert_eq!(all_foods[0].id, food1.id);
        assert_eq!(all_foods[1].id, food2.id);
    }

    #[test]
    fn test_get_meal_entries_since() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();

        db.insert_meal_entry(&NewMealEntry {
            date: NaiveDate::from_ymd_opt(2024, 6, 15).unwrap(),
            meal_type: "lunch".to_string(),
            food_id: food.id,
            serving_g: 200.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();

        // All entries since epoch
        let all = db
            .get_meal_entries_since("1970-01-01T00:00:00+00:00")
            .unwrap();
        assert_eq!(all.len(), 1);
        assert!(!all[0].food_uuid.is_empty());

        // None since future
        let none = db
            .get_meal_entries_since("2099-01-01T00:00:00+00:00")
            .unwrap();
        assert!(none.is_empty());

        // get_all_meal_entries_export
        let all_export = db.get_all_meal_entries_export().unwrap();
        assert_eq!(all_export.len(), 1);
    }

    #[test]
    fn test_get_tombstones_since() {
        let db = Database::open_in_memory().unwrap();

        db.record_tombstone("uuid-1", "foods").unwrap();
        db.record_tombstone("uuid-2", "meal_entries").unwrap();

        let all = db
            .get_tombstones_since("1970-01-01T00:00:00+00:00")
            .unwrap();
        assert_eq!(all.len(), 2);

        let none = db
            .get_tombstones_since("2099-01-01T00:00:00+00:00")
            .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn test_changes_since_full() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        db.insert_meal_entry(&NewMealEntry {
            date: NaiveDate::from_ymd_opt(2024, 6, 15).unwrap(),
            meal_type: "lunch".to_string(),
            food_id: food.id,
            serving_g: 200.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();
        db.record_tombstone("dead-uuid", "foods").unwrap();

        // Full sync (no since param)
        let payload = db.changes_since(None, "2024-06-15T12:00:00Z").unwrap();
        assert_eq!(payload.foods.len(), 1);
        assert_eq!(payload.meal_entries.len(), 1);
        assert_eq!(payload.tombstones.len(), 1);
        assert_eq!(payload.server_timestamp, "2024-06-15T12:00:00Z");
    }

    #[test]
    fn test_changes_since_incremental() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();

        // Record a timestamp after food creation
        let mid_timestamp = "2099-01-01T00:00:00+00:00";

        // Delta since future returns nothing
        let payload = db.changes_since(Some(mid_timestamp), "now").unwrap();
        assert!(payload.foods.is_empty());
        assert!(payload.meal_entries.is_empty());
        assert!(payload.tombstones.is_empty());

        // Delta since epoch returns everything
        let payload = db
            .changes_since(Some("1970-01-01T00:00:00+00:00"), "now")
            .unwrap();
        assert_eq!(payload.foods.len(), 1);
        assert_eq!(payload.foods[0].id, food.id);
    }

    #[test]
    fn test_changes_since_includes_all_entity_types() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();

        // Create recipe
        let recipe = db.create_recipe("Test Recipe", 4.0).unwrap();
        db.add_recipe_ingredient(recipe.id, food.id, 200.0).unwrap();

        // Set target
        db.set_target(0, 2000, Some(40), Some(30), Some(30))
            .unwrap();

        // Log weight
        db.upsert_weight(&NewWeightEntry {
            date: NaiveDate::from_ymd_opt(2025, 1, 15).unwrap(),
            weight_kg: 80.0,
            source: "manual".to_string(),
            notes: None,
        })
        .unwrap();

        let payload = db.changes_since(None, "now").unwrap();
        // foods: sample_food + recipe virtual food = 2
        assert_eq!(payload.foods.len(), 2);
        assert_eq!(payload.recipes.len(), 1);
        assert_eq!(payload.recipe_ingredients.len(), 1);
        assert_eq!(payload.targets.len(), 1);
        assert_eq!(payload.weight_entries.len(), 1);
    }

    #[test]
    fn test_changes_since_incremental_new_entity_types() {
        let db = Database::open_in_memory().unwrap();
        db.insert_food(&sample_food()).unwrap();

        // Create recipe (gets a timestamp)
        db.create_recipe("Test Recipe", 4.0).unwrap();

        // Set target
        db.set_target(0, 2000, Some(40), Some(30), Some(30))
            .unwrap();

        // Far future — nothing returned
        let payload = db
            .changes_since(Some("2099-01-01T00:00:00+00:00"), "now")
            .unwrap();
        assert!(payload.recipes.is_empty());
        assert!(payload.targets.is_empty());
        assert!(payload.weight_entries.is_empty());
        assert!(payload.recipe_ingredients.is_empty());

        // Epoch — everything returned
        let payload = db
            .changes_since(Some("1970-01-01T00:00:00+00:00"), "now")
            .unwrap();
        assert_eq!(payload.foods.len(), 2); // sample_food + recipe virtual food
        assert_eq!(payload.recipes.len(), 1);
        assert_eq!(payload.targets.len(), 1);
    }

    #[test]
    fn test_apply_remote_changes_new_food() {
        let db = Database::open_in_memory().unwrap();

        let incoming_food = Food {
            id: 0,
            uuid: "remote-uuid-1".to_string(),
            name: "Remote Food".to_string(),
            brand: Some("Remote Brand".to_string()),
            barcode: None,
            calories_per_100g: 200.0,
            protein_per_100g: Some(20.0),
            carbs_per_100g: Some(10.0),
            fat_per_100g: Some(5.0),
            default_serving_g: Some(100.0),
            source: "openfoodfacts".to_string(),
            created_at: "2024-01-01T00:00:00+00:00".to_string(),
            updated_at: "2024-06-01T00:00:00+00:00".to_string(),
        };

        db.apply_remote_changes(&[incoming_food], &[], &[], &[], &[], &[], &[])
            .unwrap();

        let food = db.get_food_by_uuid("remote-uuid-1").unwrap().unwrap();
        assert_eq!(food.name, "Remote Food");
    }

    #[test]
    fn test_apply_remote_changes_lww_food() {
        let db = Database::open_in_memory().unwrap();
        let local = db.insert_food(&sample_food()).unwrap();

        let incoming = Food {
            id: 0,
            uuid: local.uuid.clone(),
            name: "Updated Name".to_string(),
            brand: Some("New Brand".to_string()),
            barcode: local.barcode.clone(),
            calories_per_100g: 999.0,
            protein_per_100g: Some(99.0),
            carbs_per_100g: Some(0.0),
            fat_per_100g: Some(0.0),
            default_serving_g: Some(100.0),
            source: "manual".to_string(),
            created_at: local.created_at.clone(),
            updated_at: "2099-01-01T00:00:00+00:00".to_string(),
        };

        db.apply_remote_changes(&[incoming], &[], &[], &[], &[], &[], &[])
            .unwrap();

        let updated = db.get_food_by_uuid(&local.uuid).unwrap().unwrap();
        assert_eq!(updated.name, "Updated Name");
        assert_eq!(updated.calories_per_100g, 999.0);
    }

    #[test]
    fn test_apply_remote_changes_lww_food_older_ignored() {
        let db = Database::open_in_memory().unwrap();
        let local = db.insert_food(&sample_food()).unwrap();

        let incoming = Food {
            id: 0,
            uuid: local.uuid.clone(),
            name: "Old Name".to_string(),
            brand: None,
            barcode: None,
            calories_per_100g: 1.0,
            protein_per_100g: None,
            carbs_per_100g: None,
            fat_per_100g: None,
            default_serving_g: None,
            source: "manual".to_string(),
            created_at: "2000-01-01T00:00:00+00:00".to_string(),
            updated_at: "2000-01-01T00:00:00+00:00".to_string(),
        };

        db.apply_remote_changes(&[incoming], &[], &[], &[], &[], &[], &[])
            .unwrap();

        let unchanged = db.get_food_by_uuid(&local.uuid).unwrap().unwrap();
        assert_eq!(unchanged.name, "Chicken Breast");
    }

    #[test]
    fn test_apply_remote_changes_meal_entry() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();

        let incoming_entry = crate::models::ExportMealEntry {
            id: 0,
            uuid: "remote-meal-uuid-1".to_string(),
            date: "2024-06-15".to_string(),
            meal_type: "lunch".to_string(),
            food_id: 0,
            food_uuid: food.uuid.clone(),
            serving_g: 250.0,
            display_unit: None,
            display_quantity: None,
            created_at: "2024-06-15T12:00:00+00:00".to_string(),
            updated_at: "2024-06-15T12:00:00+00:00".to_string(),
        };

        db.apply_remote_changes(&[], &[incoming_entry], &[], &[], &[], &[], &[])
            .unwrap();

        let entries = db.get_all_meal_entries_export().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].uuid, "remote-meal-uuid-1");
        assert_eq!(entries[0].serving_g, 250.0);
    }

    #[test]
    fn test_apply_remote_changes_tombstone() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();

        let tombstone = SyncTombstone {
            uuid: food.uuid.clone(),
            table_name: "foods".to_string(),
            deleted_at: "2099-01-01T00:00:00+00:00".to_string(),
        };

        db.apply_remote_changes(&[], &[], &[], &[], &[], &[], &[tombstone])
            .unwrap();

        assert!(db.get_food_by_uuid(&food.uuid).unwrap().is_none());

        let stored = db.get_tombstones().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].uuid, food.uuid);
    }

    #[test]
    fn test_apply_remote_changes_recipes() {
        let db = Database::open_in_memory().unwrap();

        // Create a virtual food for the recipe
        let recipe_food = db
            .insert_food(&NewFood {
                name: "Remote Recipe".to_string(),
                brand: None,
                barcode: None,
                calories_per_100g: 150.0,
                protein_per_100g: Some(20.0),
                carbs_per_100g: Some(10.0),
                fat_per_100g: Some(5.0),
                default_serving_g: Some(200.0),
                source: "recipe".to_string(),
            })
            .unwrap();
        let ingredient_food = db.insert_food(&sample_food()).unwrap();

        let recipe_uuid = Uuid::new_v4().to_string();
        let ing_uuid = Uuid::new_v4().to_string();
        let now = Local::now().to_rfc3339();

        let recipes = vec![ExportRecipe {
            id: 0,
            uuid: recipe_uuid.clone(),
            food_id: 0,
            food_uuid: recipe_food.uuid.clone(),
            portions: 4.0,
            created_at: now.clone(),
            updated_at: now.clone(),
        }];

        let recipe_ingredients = vec![ExportRecipeIngredient {
            id: 0,
            uuid: ing_uuid,
            recipe_id: 0,
            recipe_uuid: recipe_uuid.clone(),
            food_id: 0,
            food_uuid: ingredient_food.uuid.clone(),
            quantity_g: 400.0,
        }];

        db.apply_remote_changes(&[], &[], &recipes, &recipe_ingredients, &[], &[], &[])
            .unwrap();

        // Recipe should exist
        let imported_recipe = db.get_recipe_by_uuid(&recipe_uuid).unwrap().unwrap();
        assert!((imported_recipe.portions - 4.0).abs() < f64::EPSILON);

        // Ingredient should exist
        let ingredients = db.get_recipe_ingredients(imported_recipe.id).unwrap();
        assert_eq!(ingredients.len(), 1);
        assert!((ingredients[0].quantity_g - 400.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_apply_remote_changes_targets_lww() {
        let db = Database::open_in_memory().unwrap();

        // Set a local target
        db.set_target(0, 1800, Some(40), Some(30), Some(30))
            .unwrap();

        // Apply a newer remote target
        let targets = vec![ExportTarget {
            day_of_week: 0,
            calories: 2200,
            protein_pct: Some(35),
            carbs_pct: Some(40),
            fat_pct: Some(25),
            updated_at: Some("2099-01-01T00:00:00+00:00".to_string()),
        }];

        db.apply_remote_changes(&[], &[], &[], &[], &targets, &[], &[])
            .unwrap();

        let target = db.get_target(0).unwrap().unwrap();
        assert_eq!(target.calories, 2200);
        assert_eq!(target.protein_pct, Some(35));
    }

    #[test]
    fn test_apply_remote_changes_targets_older_ignored() {
        let db = Database::open_in_memory().unwrap();

        // Set a local target (gets current timestamp)
        db.set_target(0, 1800, Some(40), Some(30), Some(30))
            .unwrap();

        // Apply an older remote target
        let targets = vec![ExportTarget {
            day_of_week: 0,
            calories: 1200,
            protein_pct: None,
            carbs_pct: None,
            fat_pct: None,
            updated_at: Some("2000-01-01T00:00:00+00:00".to_string()),
        }];

        db.apply_remote_changes(&[], &[], &[], &[], &targets, &[], &[])
            .unwrap();

        let target = db.get_target(0).unwrap().unwrap();
        assert_eq!(target.calories, 1800); // unchanged
    }

    #[test]
    fn test_apply_remote_changes_weight_entries_lww() {
        let db = Database::open_in_memory().unwrap();

        // Insert a local weight
        db.upsert_weight(&NewWeightEntry {
            date: NaiveDate::from_ymd_opt(2025, 1, 15).unwrap(),
            weight_kg: 80.0,
            source: "manual".to_string(),
            notes: None,
        })
        .unwrap();

        // Apply a newer remote weight for the same date
        let weights = vec![ExportWeightEntry {
            uuid: Uuid::new_v4().to_string(),
            date: "2025-01-15".to_string(),
            weight_kg: 79.5,
            source: "scale".to_string(),
            notes: Some("Smart scale reading".to_string()),
            created_at: "2025-01-15T08:00:00+00:00".to_string(),
            updated_at: "2099-01-01T00:00:00+00:00".to_string(),
        }];

        db.apply_remote_changes(&[], &[], &[], &[], &[], &weights, &[])
            .unwrap();

        let entry = db
            .get_weight(NaiveDate::from_ymd_opt(2025, 1, 15).unwrap())
            .unwrap()
            .unwrap();
        assert!((entry.weight_kg - 79.5).abs() < f64::EPSILON);
        assert_eq!(entry.source, "scale");
    }

    #[test]
    fn test_apply_remote_changes_weight_entries_older_ignored() {
        let db = Database::open_in_memory().unwrap();

        // Insert a local weight (gets current timestamp)
        db.upsert_weight(&NewWeightEntry {
            date: NaiveDate::from_ymd_opt(2025, 1, 15).unwrap(),
            weight_kg: 80.0,
            source: "manual".to_string(),
            notes: None,
        })
        .unwrap();

        // Apply an older remote weight for the same date
        let weights = vec![ExportWeightEntry {
            uuid: Uuid::new_v4().to_string(),
            date: "2025-01-15".to_string(),
            weight_kg: 75.0,
            source: "old_scale".to_string(),
            notes: None,
            created_at: "2020-01-01T00:00:00+00:00".to_string(),
            updated_at: "2020-01-01T00:00:00+00:00".to_string(),
        }];

        db.apply_remote_changes(&[], &[], &[], &[], &[], &weights, &[])
            .unwrap();

        let entry = db
            .get_weight(NaiveDate::from_ymd_opt(2025, 1, 15).unwrap())
            .unwrap()
            .unwrap();
        assert!((entry.weight_kg - 80.0).abs() < f64::EPSILON); // unchanged
    }

    #[test]
    fn test_apply_remote_changes_recipe_tombstone() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let recipe = db.create_recipe("To Delete", 2.0).unwrap();
        db.add_recipe_ingredient(recipe.id, food.id, 100.0).unwrap();

        let tombstone = SyncTombstone {
            uuid: recipe.uuid.clone(),
            table_name: "recipes".to_string(),
            deleted_at: "2099-01-01T00:00:00+00:00".to_string(),
        };

        db.apply_remote_changes(&[], &[], &[], &[], &[], &[], &[tombstone])
            .unwrap();

        assert!(db.get_recipe_by_uuid(&recipe.uuid).unwrap().is_none());
    }

    // --- Weight entry tests ---

    fn sample_weight_entry(date: NaiveDate) -> NewWeightEntry {
        NewWeightEntry {
            date,
            weight_kg: 80.5,
            source: "manual".to_string(),
            notes: Some("Morning weigh-in".to_string()),
        }
    }

    #[test]
    fn test_upsert_weight_creates_new_entry() {
        let db = Database::open_in_memory().unwrap();
        let date = NaiveDate::from_ymd_opt(2025, 1, 15).unwrap();
        let entry = db.upsert_weight(&sample_weight_entry(date)).unwrap();

        assert_eq!(entry.date, date);
        assert!((entry.weight_kg - 80.5).abs() < f64::EPSILON);
        assert_eq!(entry.source, "manual");
        assert_eq!(entry.notes.as_deref(), Some("Morning weigh-in"));
        assert!(!entry.uuid.is_empty());
    }

    #[test]
    fn test_upsert_weight_replaces_existing_for_same_date() {
        let db = Database::open_in_memory().unwrap();
        let date = NaiveDate::from_ymd_opt(2025, 1, 15).unwrap();

        let first = db.upsert_weight(&sample_weight_entry(date)).unwrap();
        assert!((first.weight_kg - 80.5).abs() < f64::EPSILON);

        let updated = db
            .upsert_weight(&NewWeightEntry {
                date,
                weight_kg: 79.8,
                source: "manual".to_string(),
                notes: Some("Evening weigh-in".to_string()),
            })
            .unwrap();

        assert!((updated.weight_kg - 79.8).abs() < f64::EPSILON);
        assert_eq!(updated.notes.as_deref(), Some("Evening weigh-in"));

        // Should only be one entry for this date
        let history = db.get_weight_history(None).unwrap();
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn test_get_weight_returns_none_for_missing_date() {
        let db = Database::open_in_memory().unwrap();
        let date = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        let result = db.get_weight(date).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_get_weight_returns_entry_for_existing_date() {
        let db = Database::open_in_memory().unwrap();
        let date = NaiveDate::from_ymd_opt(2025, 1, 15).unwrap();
        db.upsert_weight(&sample_weight_entry(date)).unwrap();

        let result = db.get_weight(date).unwrap();
        assert!(result.is_some());
        let entry = result.unwrap();
        assert_eq!(entry.date, date);
        assert!((entry.weight_kg - 80.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_get_weight_history_ordered_by_date_desc() {
        let db = Database::open_in_memory().unwrap();
        let dates = [
            NaiveDate::from_ymd_opt(2025, 1, 10).unwrap(),
            NaiveDate::from_ymd_opt(2025, 1, 12).unwrap(),
            NaiveDate::from_ymd_opt(2025, 1, 11).unwrap(),
        ];
        for date in &dates {
            db.upsert_weight(&sample_weight_entry(*date)).unwrap();
        }

        let history = db.get_weight_history(None).unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].date, dates[1]); // 2025-01-12 (most recent)
        assert_eq!(history[1].date, dates[2]); // 2025-01-11
        assert_eq!(history[2].date, dates[0]); // 2025-01-10
    }

    #[test]
    fn test_get_weight_history_with_days_limit() {
        let db = Database::open_in_memory().unwrap();
        for day in 1..=5 {
            let date = NaiveDate::from_ymd_opt(2025, 1, day).unwrap();
            db.upsert_weight(&sample_weight_entry(date)).unwrap();
        }

        let history = db.get_weight_history(Some(3)).unwrap();
        assert_eq!(history.len(), 3);
        // Most recent first
        assert_eq!(
            history[0].date,
            NaiveDate::from_ymd_opt(2025, 1, 5).unwrap()
        );
    }

    #[test]
    fn test_delete_weight() {
        let db = Database::open_in_memory().unwrap();
        let date = NaiveDate::from_ymd_opt(2025, 1, 15).unwrap();
        let entry = db.upsert_weight(&sample_weight_entry(date)).unwrap();

        db.delete_weight(entry.id).unwrap();
        let result = db.get_weight(date).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_delete_weight_not_found() {
        let db = Database::open_in_memory().unwrap();
        let result = db.delete_weight(9999);
        assert!(result.is_err());
    }

    #[test]
    fn test_export_import_roundtrip_includes_weight_entries() {
        let db = Database::open_in_memory().unwrap();

        // Add some weight entries
        for day in 1..=3 {
            let date = NaiveDate::from_ymd_opt(2025, 1, day).unwrap();
            db.upsert_weight(&NewWeightEntry {
                date,
                weight_kg: 80.0 + f64::from(day),
                source: "manual".to_string(),
                notes: None,
            })
            .unwrap();
        }

        let exported = db.export_all().unwrap();
        assert_eq!(exported.weight_entries.len(), 3);

        // Import into a fresh DB
        let db2 = Database::open_in_memory().unwrap();
        let summary = db2.import_all(&exported).unwrap();
        assert_eq!(summary.weight_entries_imported, 3);

        let history = db2.get_weight_history(None).unwrap();
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn test_merge_import_weight_lww() {
        let db = Database::open_in_memory().unwrap();

        // Create an initial weight entry
        let date = NaiveDate::from_ymd_opt(2025, 1, 15).unwrap();
        let entry = db.upsert_weight(&sample_weight_entry(date)).unwrap();

        // Import data with a newer updated_at for the same date
        let import_data = ExportData {
            version: 2,
            exported_at: "2025-01-16T00:00:00Z".to_string(),
            device_id: None,
            foods: vec![],
            meal_entries: vec![],
            recipes: vec![],
            recipe_ingredients: vec![],
            target: None,
            targets: vec![],
            weight_entries: vec![crate::models::ExportWeightEntry {
                uuid: "new-uuid".to_string(),
                date: "2025-01-15".to_string(),
                weight_kg: 79.0,
                source: "apple_health".to_string(),
                notes: Some("From Apple Health".to_string()),
                created_at: entry.created_at.clone(),
                updated_at: "2099-01-01T00:00:00Z".to_string(),
            }],
            tombstones: None,
        };

        let summary = db.import_all(&import_data).unwrap();
        assert_eq!(summary.weight_entries_imported, 1);

        let updated = db.get_weight(date).unwrap().unwrap();
        assert!((updated.weight_kg - 79.0).abs() < f64::EPSILON);
        assert_eq!(updated.source, "apple_health");
    }

    #[test]
    fn test_migration_creates_weight_entries_table() {
        let db = Database::open_in_memory().unwrap();
        // If migration ran successfully, we should be able to query the table
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM weight_entries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    // --- Recently logged foods tests ---

    #[test]
    fn test_recently_logged_foods_empty() {
        let db = Database::open_in_memory().unwrap();
        let result = db.get_recently_logged_foods(10).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_recently_logged_foods_single_entry() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        db.insert_meal_entry(&NewMealEntry {
            date: NaiveDate::from_ymd_opt(2024, 6, 15).unwrap(),
            meal_type: "lunch".to_string(),
            food_id: food.id,
            serving_g: 200.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();

        let result = db.get_recently_logged_foods(10).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].food.id, food.id);
        assert!((result[0].last_serving_g - 200.0).abs() < f64::EPSILON);
        assert_eq!(result[0].last_meal_type, "lunch");
        assert_eq!(result[0].log_count, 1);
        assert_eq!(result[0].last_logged, "2024-06-15");
    }

    #[test]
    fn test_recently_logged_foods_ordering_and_dedup() {
        let db = Database::open_in_memory().unwrap();
        let chicken = db.insert_food(&sample_food()).unwrap();
        let rice = db
            .insert_food(&NewFood {
                name: "Brown Rice".to_string(),
                brand: None,
                barcode: None,
                calories_per_100g: 112.0,
                protein_per_100g: Some(2.6),
                carbs_per_100g: Some(23.5),
                fat_per_100g: Some(0.9),
                default_serving_g: None,
                source: "manual".to_string(),
            })
            .unwrap();

        // Log chicken 3 times on different dates
        for day in [10, 12, 14] {
            db.insert_meal_entry(&NewMealEntry {
                date: NaiveDate::from_ymd_opt(2024, 6, day).unwrap(),
                meal_type: "lunch".to_string(),
                food_id: chicken.id,
                serving_g: 150.0,
                display_unit: None,
                display_quantity: None,
            })
            .unwrap();
        }

        // Log rice once on a more recent date
        db.insert_meal_entry(&NewMealEntry {
            date: NaiveDate::from_ymd_opt(2024, 6, 16).unwrap(),
            meal_type: "dinner".to_string(),
            food_id: rice.id,
            serving_g: 250.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();

        let result = db.get_recently_logged_foods(10).unwrap();
        assert_eq!(result.len(), 2);
        // Rice is more recent (June 16 vs June 14)
        assert_eq!(result[0].food.name, "Brown Rice");
        assert_eq!(result[0].log_count, 1);
        // Chicken is second
        assert_eq!(result[1].food.name, "Chicken Breast");
        assert_eq!(result[1].log_count, 3);
    }

    #[test]
    fn test_recently_logged_foods_limit() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let rice = db
            .insert_food(&NewFood {
                name: "Brown Rice".to_string(),
                brand: None,
                barcode: None,
                calories_per_100g: 112.0,
                protein_per_100g: Some(2.6),
                carbs_per_100g: Some(23.5),
                fat_per_100g: Some(0.9),
                default_serving_g: None,
                source: "manual".to_string(),
            })
            .unwrap();

        db.insert_meal_entry(&NewMealEntry {
            date: NaiveDate::from_ymd_opt(2024, 6, 15).unwrap(),
            meal_type: "lunch".to_string(),
            food_id: food.id,
            serving_g: 100.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();
        db.insert_meal_entry(&NewMealEntry {
            date: NaiveDate::from_ymd_opt(2024, 6, 15).unwrap(),
            meal_type: "dinner".to_string(),
            food_id: rice.id,
            serving_g: 200.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();

        let result = db.get_recently_logged_foods(1).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_recently_logged_foods_uses_most_recent_entry() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();

        // First entry: 100g breakfast
        db.insert_meal_entry(&NewMealEntry {
            date: NaiveDate::from_ymd_opt(2024, 6, 10).unwrap(),
            meal_type: "breakfast".to_string(),
            food_id: food.id,
            serving_g: 100.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();

        // Second (more recent) entry: 250g dinner
        db.insert_meal_entry(&NewMealEntry {
            date: NaiveDate::from_ymd_opt(2024, 6, 15).unwrap(),
            meal_type: "dinner".to_string(),
            food_id: food.id,
            serving_g: 250.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();

        let result = db.get_recently_logged_foods(10).unwrap();
        assert_eq!(result.len(), 1);
        // Should use the most recent entry's serving/meal
        assert!((result[0].last_serving_g - 250.0).abs() < f64::EPSILON);
        assert_eq!(result[0].last_meal_type, "dinner");
        assert_eq!(result[0].last_logged, "2024-06-15");
        assert_eq!(result[0].log_count, 2);
    }

    // --- Logging streak tests ---

    #[test]
    fn test_logging_streak_zero_days() {
        let db = Database::open_in_memory().unwrap();
        let today = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
        assert_eq!(db.get_logging_streak(today).unwrap(), 0);
    }

    #[test]
    fn test_logging_streak_one_day_today() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let today = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();

        db.insert_meal_entry(&NewMealEntry {
            date: today,
            meal_type: "lunch".to_string(),
            food_id: food.id,
            serving_g: 100.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();

        assert_eq!(db.get_logging_streak(today).unwrap(), 1);
    }

    #[test]
    fn test_logging_streak_starts_from_yesterday() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let today = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
        let yesterday = NaiveDate::from_ymd_opt(2024, 6, 14).unwrap();

        // No entry today, but yesterday has one
        db.insert_meal_entry(&NewMealEntry {
            date: yesterday,
            meal_type: "lunch".to_string(),
            food_id: food.id,
            serving_g: 100.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();

        assert_eq!(db.get_logging_streak(today).unwrap(), 1);
    }

    #[test]
    fn test_logging_streak_multiple_consecutive_days() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let today = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();

        // Log meals for 5 consecutive days ending today
        for day in 11..=15 {
            db.insert_meal_entry(&NewMealEntry {
                date: NaiveDate::from_ymd_opt(2024, 6, day).unwrap(),
                meal_type: "lunch".to_string(),
                food_id: food.id,
                serving_g: 100.0,
                display_unit: None,
                display_quantity: None,
            })
            .unwrap();
        }

        assert_eq!(db.get_logging_streak(today).unwrap(), 5);
    }

    #[test]
    fn test_logging_streak_gap_in_middle() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let today = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();

        // Log today, yesterday, skip a day, then another
        for day in [15, 14, 12] {
            db.insert_meal_entry(&NewMealEntry {
                date: NaiveDate::from_ymd_opt(2024, 6, day).unwrap(),
                meal_type: "lunch".to_string(),
                food_id: food.id,
                serving_g: 100.0,
                display_unit: None,
                display_quantity: None,
            })
            .unwrap();
        }

        // Streak should be 2 (today + yesterday), gap on June 13
        assert_eq!(db.get_logging_streak(today).unwrap(), 2);
    }

    #[test]
    fn test_logging_streak_no_recent_entries() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let today = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();

        // Old entry, not today or yesterday
        db.insert_meal_entry(&NewMealEntry {
            date: NaiveDate::from_ymd_opt(2024, 6, 10).unwrap(),
            meal_type: "lunch".to_string(),
            food_id: food.id,
            serving_g: 100.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();

        assert_eq!(db.get_logging_streak(today).unwrap(), 0);
    }

    // --- Calorie average tests ---

    #[test]
    fn test_calorie_average_no_entries() {
        let db = Database::open_in_memory().unwrap();
        let avg = db.get_calorie_average(7).unwrap();
        assert!((avg - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_calorie_average_single_day() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let today = Local::now().date_naive();

        // 200g of chicken: 165 * 200 / 100 = 330 kcal
        db.insert_meal_entry(&NewMealEntry {
            date: today,
            meal_type: "lunch".to_string(),
            food_id: food.id,
            serving_g: 200.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();

        let avg = db.get_calorie_average(7).unwrap();
        assert!((avg - 330.0).abs() < 0.01);
    }

    #[test]
    fn test_calorie_average_multiple_days() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let today = Local::now().date_naive();

        // Day 1 (today): 200g = 330 kcal
        db.insert_meal_entry(&NewMealEntry {
            date: today,
            meal_type: "lunch".to_string(),
            food_id: food.id,
            serving_g: 200.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();

        // Day 2 (yesterday): 100g = 165 kcal
        let yesterday = today - chrono::Duration::days(1);
        db.insert_meal_entry(&NewMealEntry {
            date: yesterday,
            meal_type: "lunch".to_string(),
            food_id: food.id,
            serving_g: 100.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();

        // Average of 330 and 165 = 247.5
        let avg = db.get_calorie_average(7).unwrap();
        assert!((avg - 247.5).abs() < 0.01);
    }

    #[test]
    fn test_calorie_average_skips_zero_days() {
        let db = Database::open_in_memory().unwrap();
        let food = db.insert_food(&sample_food()).unwrap();
        let today = Local::now().date_naive();

        // Only log on today: 200g = 330 kcal
        db.insert_meal_entry(&NewMealEntry {
            date: today,
            meal_type: "lunch".to_string(),
            food_id: food.id,
            serving_g: 200.0,
            display_unit: None,
            display_quantity: None,
        })
        .unwrap();

        // Averaging over 7 days but only 1 day has entries
        // Should return 330 (not 330/7)
        let avg = db.get_calorie_average(7).unwrap();
        assert!((avg - 330.0).abs() < 0.01);
    }

    // --- User settings / goal weight tests ---

    #[test]
    fn test_user_settings_set_get() {
        let db = Database::open_in_memory().unwrap();
        db.set_setting("test_key", "test_value").unwrap();
        let val = db.get_setting("test_key").unwrap();
        assert_eq!(val.as_deref(), Some("test_value"));
    }

    #[test]
    fn test_user_settings_get_nonexistent() {
        let db = Database::open_in_memory().unwrap();
        let val = db.get_setting("nonexistent").unwrap();
        assert!(val.is_none());
    }

    #[test]
    fn test_user_settings_upsert() {
        let db = Database::open_in_memory().unwrap();
        db.set_setting("key", "value1").unwrap();
        db.set_setting("key", "value2").unwrap();
        let val = db.get_setting("key").unwrap();
        assert_eq!(val.as_deref(), Some("value2"));
    }

    #[test]
    fn test_user_settings_delete() {
        let db = Database::open_in_memory().unwrap();
        db.set_setting("key", "value").unwrap();
        assert!(db.delete_setting("key").unwrap());
        assert!(db.get_setting("key").unwrap().is_none());
        // Deleting again returns false
        assert!(!db.delete_setting("key").unwrap());
    }

    #[test]
    fn test_migration_creates_user_settings_table() {
        let db = Database::open_in_memory().unwrap();
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM user_settings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
