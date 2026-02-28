use std::collections::HashSet;
use std::net::UdpSocket;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use chrono::NaiveDate;
use serde::{Deserialize, Deserializer, Serialize};
use tower_http::limit::RequestBodyLimitLayer;

use crate::openfoodfacts::OpenFoodFactsClient;
use grub_core::db::Database;
use grub_core::models::{
    ExportData, Food, NewFood, NewMealEntry, NewWeightEntry, RecipeDetail, SyncPayload,
    SyncPushRequest, UpdateMealEntry, WeightEntry, validate_export_meal_entry,
    validate_export_recipe, validate_export_recipe_ingredient, validate_export_target,
    validate_export_weight_entry, validate_food_data, validate_macro_split, validate_meal_type,
    validate_tombstone,
};

const BODY_LIMIT: usize = 50 * 1024 * 1024; // 50 MB

#[derive(Clone)]
struct AppState {
    db: Arc<Mutex<Database>>,
    off: Arc<OpenFoodFactsClient>,
    api_key: Option<String>,
}

// --- Request / Response types ---

#[derive(Deserialize)]
struct CreateMealRequest {
    food_id: i64,
    date: String,
    meal_type: String,
    serving_g: f64,
    display_unit: Option<String>,
    display_quantity: Option<f64>,
}

fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[allow(clippy::option_option)]
struct UpdateMealRequest {
    serving_g: Option<f64>,
    meal_type: Option<String>,
    date: Option<String>,
    #[serde(default, deserialize_with = "deserialize_some")]
    display_unit: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    display_quantity: Option<Option<f64>>,
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

#[derive(Deserialize)]
struct SetTargetRequest {
    calories: i64,
    protein_pct: Option<i64>,
    carbs_pct: Option<i64>,
    fat_pct: Option<i64>,
}

#[derive(Deserialize)]
struct CreateRecipeIngredient {
    food_id: i64,
    quantity_g: f64,
}

#[derive(Deserialize)]
struct CreateRecipeRequest {
    name: String,
    portions: f64,
    ingredients: Vec<CreateRecipeIngredient>,
}

#[derive(Deserialize)]
struct UpdateRecipeRequest {
    portions: Option<f64>,
    ingredients: Option<Vec<CreateRecipeIngredient>>,
}

#[derive(Deserialize)]
struct CreateFoodRequest {
    name: String,
    brand: Option<String>,
    barcode: Option<String>,
    calories_per_100g: f64,
    protein_per_100g: Option<f64>,
    carbs_per_100g: Option<f64>,
    fat_per_100g: Option<f64>,
    default_serving_g: Option<f64>,
    #[serde(default = "default_source")]
    source: String,
}

fn default_source() -> String {
    "manual".to_string()
}

#[derive(Deserialize)]
struct CreateWeightRequest {
    date: String,
    weight_kg: f64,
    #[serde(default = "default_source")]
    source: String,
    notes: Option<String>,
}

#[derive(Deserialize)]
struct WeightHistoryQuery {
    start: Option<String>,
    end: Option<String>,
}

// --- Watch request types (Apple Watch / Wear OS) ---

#[derive(Deserialize)]
struct WatchQuickLogRequest {
    food_id: i64,
    serving_g: f64,
    meal_type: String,
    date: Option<String>,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

// --- Error handling ---

enum ApiError {
    NotFound(String),
    BadRequest(String),
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            Self::Internal(err) => {
                eprintln!("Internal server error: {err:#}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
        };
        (status, Json(ErrorResponse { error: message })).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        Self::Internal(err)
    }
}

// --- Middleware ---

async fn require_auth(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if let Some(ref expected_key) = state.api_key {
        let authorized = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|token| token == expected_key);

        if !authorized {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: "Invalid or missing API key".to_string(),
                }),
            )
                .into_response();
        }
    }
    next.run(request).await
}

async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static("default-src 'none'"),
    );
    response
}

// --- Handlers ---

async fn get_food_by_barcode(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> Result<Json<Food>, ApiError> {
    // Check local cache first
    let cached = {
        let db = state
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        db.get_food_by_barcode(&code).context("database error")?
    };

    if let Some(food) = cached {
        return Ok(Json(food));
    }

    // Miss — hit OpenFoodFacts API
    let remote = state
        .off
        .lookup_barcode_async(&code)
        .await
        .context("OpenFoodFacts API error")?;

    let remote = remote
        .ok_or_else(|| ApiError::NotFound(format!("No product found for barcode '{code}'")))?;

    let food = {
        let db = state
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        db.upsert_food_by_barcode(&remote)
            .context("database error")?
    };

    Ok(Json(food))
}

async fn create_meal(
    State(state): State<AppState>,
    Json(req): Json<CreateMealRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let meal_type =
        validate_meal_type(&req.meal_type).map_err(|e| ApiError::BadRequest(format!("{e}")))?;

    let date = NaiveDate::parse_from_str(&req.date, "%Y-%m-%d").map_err(|_| {
        ApiError::BadRequest(format!("Invalid date '{}'. Use YYYY-MM-DD", req.date))
    })?;

    if req.serving_g <= 0.0 {
        return Err(ApiError::BadRequest(
            "serving_g must be greater than 0".to_string(),
        ));
    }

    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // Verify food exists
    db.get_food_by_id(req.food_id)
        .map_err(|_| ApiError::BadRequest(format!("Food with id {} not found", req.food_id)))?;

    let entry = db
        .insert_meal_entry(&NewMealEntry {
            date,
            meal_type,
            food_id: req.food_id,
            serving_g: req.serving_g,
            display_unit: req.display_unit,
            display_quantity: req.display_quantity,
        })
        .context("failed to insert meal entry")?;

    let value = serde_json::to_value(entry).context("failed to serialize meal entry")?;
    Ok((StatusCode::CREATED, Json(value)))
}

async fn update_meal(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateMealRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if req.serving_g.is_none()
        && req.meal_type.is_none()
        && req.date.is_none()
        && req.display_unit.is_none()
        && req.display_quantity.is_none()
    {
        return Err(ApiError::BadRequest(
            "At least one field must be provided".to_string(),
        ));
    }

    let meal_type = req
        .meal_type
        .as_deref()
        .map(validate_meal_type)
        .transpose()
        .map_err(|e| ApiError::BadRequest(format!("{e}")))?;

    let date = req
        .date
        .as_deref()
        .map(|d| {
            NaiveDate::parse_from_str(d, "%Y-%m-%d")
                .map_err(|_| ApiError::BadRequest(format!("Invalid date '{d}'. Use YYYY-MM-DD")))
        })
        .transpose()?;

    if let Some(serving_g) = req.serving_g {
        if serving_g <= 0.0 {
            return Err(ApiError::BadRequest(
                "serving_g must be greater than 0".to_string(),
            ));
        }
    }

    let update = UpdateMealEntry {
        serving_g: req.serving_g,
        meal_type,
        date,
        display_unit: req.display_unit,
        display_quantity: req.display_quantity,
    };

    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let entry = db
        .update_meal_entry(id, &update)
        .map_err(|_| ApiError::NotFound(format!("Meal entry {id} not found")))?;

    let value = serde_json::to_value(entry).context("failed to serialize meal entry")?;
    Ok(Json(value))
}

async fn delete_meal(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if db.delete_meal_entry(id).context("database error")? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("Meal entry {id} not found")))
    }
}

async fn get_daily_summary(
    State(state): State<AppState>,
    Path(date_str): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let date = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
        .map_err(|_| ApiError::BadRequest(format!("Invalid date '{date_str}'. Use YYYY-MM-DD")))?;

    let summary = {
        let db = state
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        db.build_daily_summary(date).context("database error")?
    };

    let value = serde_json::to_value(summary).context("failed to serialize summary")?;
    Ok(Json(value))
}

async fn search_foods(
    State(state): State<AppState>,
    Query(params): Query<SearchQuery>,
) -> Result<Json<Vec<Food>>, ApiError> {
    let query = &params.q;

    // Search local DB
    let local = {
        let db = state
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        db.search_foods_local(query).context("database error")?
    };

    // Search OpenFoodFacts
    let remote = state
        .off
        .search_async(query)
        .await
        .context("OpenFoodFacts API error")?;

    // Cache remote results
    let cached_remote = {
        let db = state
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut cached = Vec::new();
        for food in &remote {
            if let Ok(f) = db.upsert_food_by_barcode(food) {
                cached.push(f);
            }
        }
        cached
    };

    // Deduplicate by id: local first, then remote
    let mut all: Vec<Food> = Vec::new();
    let mut seen_ids = HashSet::new();
    for f in local {
        if seen_ids.insert(f.id) {
            all.push(f);
        }
    }
    for f in cached_remote {
        if seen_ids.insert(f.id) {
            all.push(f);
        }
    }

    Ok(Json(all))
}

async fn create_food(
    State(state): State<AppState>,
    Json(req): Json<CreateFoodRequest>,
) -> Result<(StatusCode, Json<Food>), ApiError> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name must not be empty".to_string()));
    }
    if req.calories_per_100g < 0.0 {
        return Err(ApiError::BadRequest(
            "calories_per_100g must not be negative".to_string(),
        ));
    }
    if req.protein_per_100g.is_some_and(|v| v < 0.0) {
        return Err(ApiError::BadRequest(
            "protein_per_100g must not be negative".to_string(),
        ));
    }
    if req.carbs_per_100g.is_some_and(|v| v < 0.0) {
        return Err(ApiError::BadRequest(
            "carbs_per_100g must not be negative".to_string(),
        ));
    }
    if req.fat_per_100g.is_some_and(|v| v < 0.0) {
        return Err(ApiError::BadRequest(
            "fat_per_100g must not be negative".to_string(),
        ));
    }

    let new_food = NewFood {
        name,
        brand: req.brand,
        barcode: req.barcode,
        calories_per_100g: req.calories_per_100g,
        protein_per_100g: req.protein_per_100g,
        carbs_per_100g: req.carbs_per_100g,
        fat_per_100g: req.fat_per_100g,
        default_serving_g: req.default_serving_g,
        source: req.source,
    };

    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let food = db.insert_food(&new_food).context("failed to insert food")?;
    Ok((StatusCode::CREATED, Json(food)))
}

// --- Target handlers ---

async fn get_all_targets(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let targets = db.get_all_targets().context("database error")?;
    let value = serde_json::to_value(targets).context("failed to serialize targets")?;
    Ok(Json(value))
}

async fn get_target(
    State(state): State<AppState>,
    Path(day): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !(0..=6).contains(&day) {
        return Err(ApiError::BadRequest(
            "day must be between 0 (Monday) and 6 (Sunday)".to_string(),
        ));
    }
    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let target = db.get_target(day).context("database error")?;
    match target {
        Some(t) => {
            let value = serde_json::to_value(t).context("failed to serialize target")?;
            Ok(Json(value))
        }
        None => Err(ApiError::NotFound(format!("No target set for day {day}"))),
    }
}

async fn set_target(
    State(state): State<AppState>,
    Path(day): Path<i64>,
    Json(req): Json<SetTargetRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !(0..=6).contains(&day) {
        return Err(ApiError::BadRequest(
            "day must be between 0 (Monday) and 6 (Sunday)".to_string(),
        ));
    }
    if req.calories <= 0 {
        return Err(ApiError::BadRequest(
            "calories must be greater than 0".to_string(),
        ));
    }

    match (req.protein_pct, req.carbs_pct, req.fat_pct) {
        (None, None, None) => {}
        (Some(p), Some(c), Some(f)) => {
            validate_macro_split(p, c, f).map_err(|e| ApiError::BadRequest(format!("{e}")))?;
        }
        _ => {
            return Err(ApiError::BadRequest(
                "If setting macro percentages, all three (protein_pct, carbs_pct, fat_pct) must be provided".to_string(),
            ));
        }
    }

    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let target = db
        .set_target(
            day,
            req.calories,
            req.protein_pct,
            req.carbs_pct,
            req.fat_pct,
        )
        .context("database error")?;
    let value = serde_json::to_value(target).context("failed to serialize target")?;
    Ok(Json(value))
}

async fn delete_target(
    State(state): State<AppState>,
    Path(day): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !(0..=6).contains(&day) {
        return Err(ApiError::BadRequest(
            "day must be between 0 (Monday) and 6 (Sunday)".to_string(),
        ));
    }
    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cleared = db.clear_target(day).context("database error")?;
    Ok(Json(serde_json::json!({ "cleared": cleared })))
}

async fn delete_all_targets(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cleared = db.clear_all_targets().context("database error")?;
    Ok(Json(serde_json::json!({ "cleared": cleared })))
}

// --- Recipe Handlers ---

async fn create_recipe(
    State(state): State<AppState>,
    Json(req): Json<CreateRecipeRequest>,
) -> Result<(StatusCode, Json<RecipeDetail>), ApiError> {
    if req.portions <= 0.0 {
        return Err(ApiError::BadRequest(
            "portions must be greater than 0".to_string(),
        ));
    }

    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let recipe = db
        .create_recipe(&req.name, req.portions)
        .context("failed to create recipe")?;

    for ing in &req.ingredients {
        if ing.quantity_g <= 0.0 {
            return Err(ApiError::BadRequest(
                "ingredient quantity_g must be greater than 0".to_string(),
            ));
        }
        // Verify food exists
        db.get_food_by_id(ing.food_id)
            .map_err(|_| ApiError::BadRequest(format!("Food with id {} not found", ing.food_id)))?;
        db.add_recipe_ingredient(recipe.id, ing.food_id, ing.quantity_g)
            .context("failed to add ingredient")?;
    }

    let detail = db
        .get_recipe_detail(recipe.id)
        .context("failed to get recipe detail")?;
    Ok((StatusCode::CREATED, Json(detail)))
}

async fn list_recipes(State(state): State<AppState>) -> Result<Json<Vec<RecipeDetail>>, ApiError> {
    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let recipes = db.list_recipes().context("database error")?;
    Ok(Json(recipes))
}

async fn get_recipe(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<RecipeDetail>, ApiError> {
    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let detail = db
        .get_recipe_detail(id)
        .map_err(|_| ApiError::NotFound(format!("Recipe {id} not found")))?;
    Ok(Json(detail))
}

async fn update_recipe(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateRecipeRequest>,
) -> Result<Json<RecipeDetail>, ApiError> {
    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // Verify recipe exists
    db.get_recipe_by_id(id)
        .map_err(|_| ApiError::NotFound(format!("Recipe {id} not found")))?;

    if let Some(portions) = req.portions {
        if portions <= 0.0 {
            return Err(ApiError::BadRequest(
                "portions must be greater than 0".to_string(),
            ));
        }
        db.set_recipe_portions(id, portions)
            .context("failed to update portions")?;
    }

    if let Some(ingredients) = &req.ingredients {
        // Replace all ingredients: remove existing, add new
        let existing = db
            .get_recipe_ingredients(id)
            .context("failed to get ingredients")?;
        for ing in &existing {
            let food = db.get_food_by_id(ing.food_id).context("database error")?;
            db.remove_recipe_ingredient(id, &food.name)
                .context("failed to remove ingredient")?;
        }
        for ing in ingredients {
            if ing.quantity_g <= 0.0 {
                return Err(ApiError::BadRequest(
                    "ingredient quantity_g must be greater than 0".to_string(),
                ));
            }
            db.get_food_by_id(ing.food_id).map_err(|_| {
                ApiError::BadRequest(format!("Food with id {} not found", ing.food_id))
            })?;
            db.add_recipe_ingredient(id, ing.food_id, ing.quantity_g)
                .context("failed to add ingredient")?;
        }
    }

    let detail = db
        .get_recipe_detail(id)
        .context("failed to get recipe detail")?;
    Ok(Json(detail))
}

async fn delete_recipe(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    db.get_recipe_by_id(id)
        .map_err(|_| ApiError::NotFound(format!("Recipe {id} not found")))?;
    db.delete_recipe(id).context("failed to delete recipe")?;
    Ok(StatusCode::NO_CONTENT)
}

// --- Sync handlers ---

#[derive(Deserialize)]
struct SyncQuery {
    since: Option<String>,
}

async fn get_sync_delta(
    State(state): State<AppState>,
    Query(params): Query<SyncQuery>,
) -> Result<Json<SyncPayload>, ApiError> {
    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let server_timestamp = chrono::Utc::now().to_rfc3339();
    let payload = db
        .changes_since(params.since.as_deref(), &server_timestamp)
        .context("failed to get sync delta")?;
    Ok(Json(payload))
}

async fn push_sync(
    State(state): State<AppState>,
    Json(mut req): Json<SyncPushRequest>,
) -> Result<Json<SyncPayload>, ApiError> {
    // Validate incoming foods
    for food in &req.foods {
        validate_food_data(food).map_err(|e| ApiError::BadRequest(format!("{e}")))?;
    }
    // Validate incoming meal entries
    for entry in &req.meal_entries {
        validate_export_meal_entry(entry).map_err(|e| ApiError::BadRequest(format!("{e}")))?;
    }
    // Validate incoming recipes
    for recipe in &req.recipes {
        validate_export_recipe(recipe).map_err(|e| ApiError::BadRequest(format!("{e}")))?;
    }
    // Validate incoming recipe ingredients
    for ingredient in &req.recipe_ingredients {
        validate_export_recipe_ingredient(ingredient)
            .map_err(|e| ApiError::BadRequest(format!("{e}")))?;
    }
    // Validate incoming targets
    for target in &req.targets {
        validate_export_target(target).map_err(|e| ApiError::BadRequest(format!("{e}")))?;
    }
    // Validate incoming weight entries
    for entry in &req.weight_entries {
        validate_export_weight_entry(entry).map_err(|e| ApiError::BadRequest(format!("{e}")))?;
    }
    // Validate and sanitize tombstones
    for tombstone in &mut req.tombstones {
        validate_tombstone(tombstone).map_err(|e| ApiError::BadRequest(format!("{e}")))?;
    }

    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let server_timestamp = chrono::Utc::now().to_rfc3339();
    // Get server's changes BEFORE applying client changes (avoids echoing)
    let delta = db
        .changes_since(req.since.as_deref(), &server_timestamp)
        .context("failed to get sync delta")?;
    db.apply_remote_changes(
        &req.foods,
        &req.meal_entries,
        &req.recipes,
        &req.recipe_ingredients,
        &req.targets,
        &req.weight_entries,
        &req.tombstones,
    )
    .context("failed to merge sync data")?;
    Ok(Json(delta))
}

// --- Weight handlers ---

async fn create_weight(
    State(state): State<AppState>,
    Json(req): Json<CreateWeightRequest>,
) -> Result<(StatusCode, Json<WeightEntry>), ApiError> {
    let date = NaiveDate::parse_from_str(&req.date, "%Y-%m-%d").map_err(|_| {
        ApiError::BadRequest(format!("Invalid date '{}'. Use YYYY-MM-DD", req.date))
    })?;

    if req.weight_kg <= 0.0 {
        return Err(ApiError::BadRequest(
            "weight_kg must be greater than 0".to_string(),
        ));
    }

    let entry = NewWeightEntry {
        date,
        weight_kg: req.weight_kg,
        source: req.source,
        notes: req.notes,
    };

    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let result = db
        .upsert_weight(&entry)
        .context("failed to upsert weight")?;
    Ok((StatusCode::CREATED, Json(result)))
}

async fn get_weight(
    State(state): State<AppState>,
    Path(date_str): Path<String>,
) -> Result<Json<WeightEntry>, ApiError> {
    let date = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
        .map_err(|_| ApiError::BadRequest(format!("Invalid date '{date_str}'. Use YYYY-MM-DD")))?;

    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let entry = db
        .get_weight(date)
        .context("database error")?
        .ok_or_else(|| ApiError::NotFound(format!("No weight entry for {date_str}")))?;
    Ok(Json(entry))
}

async fn get_weight_history(
    State(state): State<AppState>,
    Query(params): Query<WeightHistoryQuery>,
) -> Result<Json<Vec<WeightEntry>>, ApiError> {
    // Validate date params if provided
    if let Some(ref s) = params.start {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| {
            ApiError::BadRequest(format!("Invalid start date '{s}'. Use YYYY-MM-DD"))
        })?;
    }
    if let Some(ref e) = params.end {
        NaiveDate::parse_from_str(e, "%Y-%m-%d")
            .map_err(|_| ApiError::BadRequest(format!("Invalid end date '{e}'. Use YYYY-MM-DD")))?;
    }

    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let mut entries = db.get_weight_history(None).context("database error")?;

    // Filter by date range if provided
    if let Some(ref start) = params.start {
        if let Ok(start_date) = NaiveDate::parse_from_str(start, "%Y-%m-%d") {
            entries.retain(|e| e.date >= start_date);
        }
    }
    if let Some(ref end) = params.end {
        if let Ok(end_date) = NaiveDate::parse_from_str(end, "%Y-%m-%d") {
            entries.retain(|e| e.date <= end_date);
        }
    }

    Ok(Json(entries))
}

async fn delete_weight(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    db.delete_weight(id)
        .map_err(|_| ApiError::NotFound(format!("Weight entry {id} not found")))?;
    Ok(StatusCode::NO_CONTENT)
}

// --- Export / Import handlers ---

async fn export_data(State(state): State<AppState>) -> Result<Json<ExportData>, ApiError> {
    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let data = db.export_all().context("failed to export data")?;
    Ok(Json(data))
}

async fn import_data(
    State(state): State<AppState>,
    Json(mut data): Json<ExportData>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate imported foods
    for food in &data.foods {
        validate_food_data(food).map_err(|e| ApiError::BadRequest(format!("{e}")))?;
    }
    // Validate imported meal entries
    for entry in &data.meal_entries {
        validate_export_meal_entry(entry).map_err(|e| ApiError::BadRequest(format!("{e}")))?;
    }
    // Validate imported recipes
    for recipe in &data.recipes {
        validate_export_recipe(recipe).map_err(|e| ApiError::BadRequest(format!("{e}")))?;
    }
    // Validate imported recipe ingredients
    for ingredient in &data.recipe_ingredients {
        validate_export_recipe_ingredient(ingredient)
            .map_err(|e| ApiError::BadRequest(format!("{e}")))?;
    }
    // Validate imported targets
    for target in &data.targets {
        validate_export_target(target).map_err(|e| ApiError::BadRequest(format!("{e}")))?;
    }
    // Validate imported weight entries
    for entry in &data.weight_entries {
        validate_export_weight_entry(entry).map_err(|e| ApiError::BadRequest(format!("{e}")))?;
    }
    // Validate and sanitize tombstones if present
    if let Some(ref mut tombstones) = data.tombstones {
        for tombstone in tombstones.iter_mut() {
            validate_tombstone(tombstone).map_err(|e| ApiError::BadRequest(format!("{e}")))?;
        }
    }

    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let summary = db.import_all(&data).context("failed to import data")?;
    let value = serde_json::to_value(summary).context("failed to serialize import summary")?;
    Ok(Json(value))
}

// --- QR code helpers ---

/// Detect the machine's local network IP address.
///
/// Uses the UDP socket trick: create a UDP socket and "connect" to a public IP
/// (no actual traffic is sent), then read back the local address the OS chose.
fn detect_local_ip() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let addr = socket.local_addr().ok()?;
    let ip = addr.ip();
    if ip.is_loopback() {
        None
    } else {
        Some(ip.to_string())
    }
}

/// Build a `grub://connect` deep link URL for mobile app auto-configuration.
///
/// The URL format is: `grub://connect?url=<percent-encoded>&key=<key>`
/// Phone cameras recognize this as a URL and offer to open the Grub app.
fn build_connect_deep_link(server_url: &str, api_key: &str) -> String {
    // Percent-encode the server URL (it contains :// and : which need escaping)
    let encoded_url = percent_encode_component(server_url);
    format!("grub://connect?url={encoded_url}&key={api_key}")
}

/// Minimal percent-encoding for a URL query parameter value.
///
/// Encodes characters that are not unreserved per RFC 3986 and would break
/// query-parameter parsing (`:`, `/`, `?`, `#`, `&`, `=`, `+`, `%`, space).
fn percent_encode_component(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 3);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push('%');
                encoded.push(char::from(HEX_CHARS[(byte >> 4) as usize]));
                encoded.push(char::from(HEX_CHARS[(byte & 0x0F) as usize]));
            }
        }
    }
    encoded
}

const HEX_CHARS: [u8; 16] = *b"0123456789ABCDEF";

/// Print a compact QR code to stderr using Unicode half-block characters.
///
/// Each character encodes two vertical modules, halving the output height.
fn print_qr_code(data: &str) {
    use qrcode::QrCode;

    let code = match QrCode::new(data.as_bytes()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to generate QR code: {e}");
            return;
        }
    };

    let width = code.width();
    let colors: Vec<bool> = code
        .into_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();

    // 1-module quiet zone on each side
    let quiet = 1;
    let total_w = width + 2 * quiet;
    let total_h = width + 2 * quiet;

    // Helper to query whether a module is dark (quiet zone = light)
    let is_dark = |row: usize, col: usize| -> bool {
        if row < quiet || row >= quiet + width || col < quiet || col >= quiet + width {
            return false;
        }
        colors[(row - quiet) * width + (col - quiet)]
    };

    eprintln!();
    eprintln!("Scan to connect:");

    // Process two rows at a time using half-block characters
    let mut row = 0;
    while row < total_h {
        let mut line = String::with_capacity(total_w);
        for col in 0..total_w {
            let top = is_dark(row, col);
            let bot = if row + 1 < total_h {
                is_dark(row + 1, col)
            } else {
                false
            };
            line.push(match (top, bot) {
                (true, true) => '\u{2588}',  // █
                (true, false) => '\u{2580}', // ▀
                (false, true) => '\u{2584}', // ▄
                (false, false) => ' ',
            });
        }
        eprintln!("{line}");
        row += 2;
    }
    eprintln!();
}

// --- Watch handlers (Apple Watch / Wear OS) ---

async fn watch_glance(
    State(state): State<AppState>,
) -> Result<Json<grub_core::models::WatchGlance>, ApiError> {
    let today = chrono::Local::now().date_naive();
    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let glance = db.build_watch_glance(today).context("database error")?;
    Ok(Json(glance))
}

async fn watch_glance_date(
    State(state): State<AppState>,
    Path(date_str): Path<String>,
) -> Result<Json<grub_core::models::WatchGlance>, ApiError> {
    let date = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
        .map_err(|_| ApiError::BadRequest(format!("Invalid date '{date_str}'. Use YYYY-MM-DD")))?;
    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let glance = db.build_watch_glance(date).context("database error")?;
    Ok(Json(glance))
}

async fn watch_recent(
    State(state): State<AppState>,
) -> Result<Json<Vec<grub_core::models::WatchRecentFood>>, ApiError> {
    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let recent = db.get_watch_recent_foods(10).context("database error")?;
    Ok(Json(recent))
}

async fn watch_quick_log(
    State(state): State<AppState>,
    Json(req): Json<WatchQuickLogRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let meal_type =
        validate_meal_type(&req.meal_type).map_err(|e| ApiError::BadRequest(format!("{e}")))?;

    let date_str = req.date.unwrap_or_else(|| {
        chrono::Local::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string()
    });

    let date = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
        .map_err(|_| ApiError::BadRequest(format!("Invalid date '{date_str}'. Use YYYY-MM-DD")))?;

    if req.serving_g <= 0.0 {
        return Err(ApiError::BadRequest(
            "serving_g must be greater than 0".to_string(),
        ));
    }

    let db = state
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // Verify food exists
    db.get_food_by_id(req.food_id)
        .map_err(|_| ApiError::BadRequest(format!("Food with id {} not found", req.food_id)))?;

    let entry = db
        .insert_meal_entry(&NewMealEntry {
            date,
            meal_type,
            food_id: req.food_id,
            serving_g: req.serving_g,
            display_unit: None,
            display_quantity: None,
        })
        .context("failed to insert meal entry")?;

    let value = serde_json::to_value(entry).context("failed to serialize meal entry")?;
    Ok((StatusCode::CREATED, Json(value)))
}

// --- Router builder ---

/// TLS configuration for the server.
pub struct TlsConfig {
    pub cert_path: std::path::PathBuf,
    pub key_path: std::path::PathBuf,
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/foods/barcode/{code}", get(get_food_by_barcode))
        .route("/api/meals", post(create_meal))
        .route("/api/meals/{id}", put(update_meal).delete(delete_meal))
        .route("/api/summary/{date}", get(get_daily_summary))
        .route("/api/foods", post(create_food))
        .route("/api/foods/search", get(search_foods))
        .route(
            "/api/targets",
            get(get_all_targets).delete(delete_all_targets),
        )
        .route(
            "/api/targets/{day}",
            get(get_target).put(set_target).delete(delete_target),
        )
        .route("/api/recipes", post(create_recipe).get(list_recipes))
        .route(
            "/api/recipes/{id}",
            get(get_recipe).put(update_recipe).delete(delete_recipe),
        )
        .route("/api/weight", post(create_weight).get(get_weight_history))
        .route("/api/weight/{date}", get(get_weight))
        .route("/api/weight/entry/{id}", delete(delete_weight))
        .route("/api/export", get(export_data))
        .route("/api/import", post(import_data))
        .route("/api/sync", get(get_sync_delta).post(push_sync))
        // Watch endpoints (Apple Watch / Wear OS)
        .route("/api/watch/glance", get(watch_glance))
        .route("/api/watch/glance/{date}", get(watch_glance_date))
        .route("/api/watch/recent", get(watch_recent))
        .route("/api/watch/quick-log", post(watch_quick_log))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .layer(RequestBodyLimitLayer::new(BODY_LIMIT))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

// --- Server startup ---

pub async fn start_server(
    db: Database,
    port: u16,
    bind: &str,
    api_key: Option<String>,
    tls: Option<TlsConfig>,
    new_api_key: bool,
) -> anyhow::Result<()> {
    let state = AppState {
        db: Arc::new(Mutex::new(db)),
        off: Arc::new(OpenFoodFactsClient::new()),
        api_key: api_key.clone(),
    };

    let app = build_router(state);

    if let Some(ref key) = api_key {
        eprintln!(
            "API key: {}...{} (see api_key file in data directory)",
            &key[..4],
            &key[key.len() - 4..],
        );
    } else {
        eprintln!("Warning: Authentication disabled (--no-auth). API is open to anyone.");
    }

    if bind != "127.0.0.1" && bind != "localhost" && api_key.is_none() {
        eprintln!(
            "Warning: Listening on {bind} with no authentication. Any device on your network can access this API."
        );
    }

    if new_api_key {
        if let Some(ref key) = api_key {
            let scheme = if tls.is_some() { "https" } else { "http" };
            let host = if bind == "0.0.0.0" {
                detect_local_ip().unwrap_or_else(|| bind.to_string())
            } else {
                bind.to_string()
            };
            let server_url = format!("{scheme}://{host}:{port}");
            let deep_link = build_connect_deep_link(&server_url, key);
            print_qr_code(&deep_link);
        }
    }

    if let Some(tls_config) = tls {
        let fingerprint = crate::tls::ensure_cert(&tls_config.cert_path, &tls_config.key_path)?;

        let rustls_config = axum_server::tls_rustls::RustlsConfig::from_pem_file(
            &tls_config.cert_path,
            &tls_config.key_path,
        )
        .await
        .context("failed to load TLS certificate")?;

        let addr = format!("{bind}:{port}")
            .parse::<std::net::SocketAddr>()
            .context("invalid bind address")?;

        eprintln!("Listening on https://{bind}:{port}");
        eprintln!("Certificate fingerprint (SHA-256):");
        eprintln!("  {fingerprint}");

        axum_server::bind_rustls(addr, rustls_config)
            .serve(app.into_make_service())
            .await?;
    } else {
        let listener = tokio::net::TcpListener::bind(format!("{bind}:{port}")).await?;
        eprintln!("Listening on http://{bind}:{port}");
        axum::serve(listener, app).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_state(api_key: Option<String>) -> AppState {
        AppState {
            db: Arc::new(Mutex::new(Database::open_in_memory().unwrap())),
            off: Arc::new(OpenFoodFactsClient::new()),
            api_key,
        }
    }

    fn test_app(api_key: Option<String>) -> Router {
        build_router(test_state(api_key))
    }

    #[tokio::test]
    async fn auth_missing_key_returns_401() {
        let app = test_app(Some("test-key-abc123".to_string()));

        let response = app
            .oneshot(
                axum::http::Request::get("/api/targets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "Invalid or missing API key");
    }

    #[tokio::test]
    async fn auth_wrong_key_returns_401() {
        let app = test_app(Some("test-key-abc123".to_string()));

        let response = app
            .oneshot(
                axum::http::Request::get("/api/targets")
                    .header("Authorization", "Bearer wrong-key")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_correct_key_succeeds() {
        let app = test_app(Some("test-key-abc123".to_string()));

        let response = app
            .oneshot(
                axum::http::Request::get("/api/targets")
                    .header("Authorization", "Bearer test-key-abc123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn no_auth_mode_allows_requests() {
        let app = test_app(None);

        let response = app
            .oneshot(
                axum::http::Request::get("/api/targets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn security_headers_present() {
        let app = test_app(None);

        let response = app
            .oneshot(
                axum::http::Request::get("/api/targets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
        assert_eq!(response.headers().get("x-frame-options").unwrap(), "DENY");
        assert_eq!(
            response.headers().get("content-security-policy").unwrap(),
            "default-src 'none'"
        );
    }

    #[tokio::test]
    async fn security_headers_on_auth_failure() {
        let app = test_app(Some("secret".to_string()));

        let response = app
            .oneshot(
                axum::http::Request::get("/api/targets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
    }

    #[tokio::test]
    async fn body_size_limit_rejects_oversized() {
        let app = test_app(None);

        let big_body = vec![0u8; BODY_LIMIT + 1];
        let response = app
            .oneshot(
                axum::http::Request::post("/api/meals")
                    .header("content-type", "application/json")
                    .body(Body::from(big_body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn internal_error_does_not_leak_details() {
        // The Internal variant should produce a generic message
        let error = ApiError::Internal(anyhow::anyhow!("secret database path /home/user/.grub/db"));
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "Internal server error");
        assert!(!json["error"].as_str().unwrap().contains("secret"));
    }

    #[test]
    fn detect_local_ip_returns_non_loopback() {
        // This test may return None in environments without network access
        // (e.g. sandboxed CI), so we only assert the format when it succeeds.
        if let Some(ip) = detect_local_ip() {
            assert!(!ip.starts_with("127."), "IP should not be loopback: {ip}");
            // Should parse as a valid IPv4 address
            assert!(
                ip.parse::<std::net::Ipv4Addr>().is_ok(),
                "Not a valid IPv4: {ip}"
            );
        }
    }

    #[test]
    fn print_qr_code_does_not_panic() {
        let deep_link = build_connect_deep_link("http://192.168.1.10:8080", "abc123");
        print_qr_code(&deep_link);
    }

    #[test]
    fn deep_link_format() {
        let link = build_connect_deep_link("http://192.168.1.42:8080", "abc123def456");
        assert!(link.starts_with("grub://connect?"));
        assert!(link.contains("url=http%3A%2F%2F192.168.1.42%3A8080"));
        assert!(link.contains("key=abc123def456"));
    }

    #[test]
    fn deep_link_https() {
        let link = build_connect_deep_link("https://192.168.1.42:8080", "key123");
        assert!(link.contains("url=https%3A%2F%2F192.168.1.42%3A8080"));
    }

    #[test]
    fn percent_encode_roundtrip() {
        let input = "http://192.168.1.10:8080";
        let encoded = percent_encode_component(input);
        assert_eq!(encoded, "http%3A%2F%2F192.168.1.10%3A8080");
    }

    // --- Watch endpoint tests ---

    #[tokio::test]
    async fn watch_glance_returns_200() {
        let app = test_app(None);

        let response = app
            .oneshot(
                axum::http::Request::get("/api/watch/glance")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["calories_eaten"].is_number());
        assert!(json["meal_count"].is_number());
        assert!(json["logging_streak"].is_number());
    }

    #[tokio::test]
    async fn watch_glance_date_returns_200() {
        let app = test_app(None);

        let response = app
            .oneshot(
                axum::http::Request::get("/api/watch/glance/2024-06-15")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["date"], "2024-06-15");
    }

    #[tokio::test]
    async fn watch_glance_invalid_date_returns_400() {
        let app = test_app(None);

        let response = app
            .oneshot(
                axum::http::Request::get("/api/watch/glance/not-a-date")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn watch_recent_returns_200() {
        let app = test_app(None);

        let response = app
            .oneshot(
                axum::http::Request::get("/api/watch/recent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.is_array());
    }

    #[tokio::test]
    async fn watch_quick_log_creates_meal() {
        let state = test_state(None);
        let app = build_router(state.clone());

        // Insert a food first
        let food = {
            let db = state.db.lock().unwrap();
            db.insert_food(&grub_core::models::NewFood {
                name: "Watch Food".to_string(),
                brand: None,
                barcode: None,
                calories_per_100g: 200.0,
                protein_per_100g: Some(20.0),
                carbs_per_100g: Some(30.0),
                fat_per_100g: Some(10.0),
                default_serving_g: Some(100.0),
                source: "manual".to_string(),
            })
            .unwrap()
        };

        let body = serde_json::json!({
            "food_id": food.id,
            "serving_g": 150.0,
            "meal_type": "lunch",
            "date": "2024-06-15"
        });

        let response = app
            .oneshot(
                axum::http::Request::post("/api/watch/quick-log")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["meal_type"], "lunch");
        assert_eq!(json["food_id"], food.id);
    }

    #[tokio::test]
    async fn watch_quick_log_defaults_to_today() {
        let state = test_state(None);
        let app = build_router(state.clone());

        let food = {
            let db = state.db.lock().unwrap();
            db.insert_food(&grub_core::models::NewFood {
                name: "Quick Food".to_string(),
                brand: None,
                barcode: None,
                calories_per_100g: 100.0,
                protein_per_100g: None,
                carbs_per_100g: None,
                fat_per_100g: None,
                default_serving_g: None,
                source: "manual".to_string(),
            })
            .unwrap()
        };

        let body = serde_json::json!({
            "food_id": food.id,
            "serving_g": 100.0,
            "meal_type": "snack"
        });

        let response = app
            .oneshot(
                axum::http::Request::post("/api/watch/quick-log")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let today = chrono::Local::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string();
        assert_eq!(json["date"], today);
    }

    #[tokio::test]
    async fn watch_quick_log_invalid_serving_returns_400() {
        let app = test_app(None);

        let body = serde_json::json!({
            "food_id": 1,
            "serving_g": -10.0,
            "meal_type": "lunch"
        });

        let response = app
            .oneshot(
                axum::http::Request::post("/api/watch/quick-log")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn watch_endpoints_require_auth() {
        let app = test_app(Some("secret-key-12345678".to_string()));

        let response = app
            .oneshot(
                axum::http::Request::get("/api/watch/glance")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
