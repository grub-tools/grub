use anyhow::{Context, Result};

use grub_core::models::NewFood;
use grub_core::openfoodfacts::{ProductResponse, SearchResponse, product_to_food};
use grub_core::service::FoodLookupProvider;

const SEARCH_URL: &str = "https://world.openfoodfacts.org/cgi/search.pl";
const PRODUCT_URL: &str = "https://world.openfoodfacts.org/api/v0/product";

pub struct OpenFoodFactsClient {
    client: reqwest::Client,
    rt: tokio::runtime::Handle,
}

impl OpenFoodFactsClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent(format!(
                "grub-cli/{} (calorie tracker)",
                env!("CARGO_PKG_VERSION")
            ))
            .timeout(std::time::Duration::from_secs(10))
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("Failed to build HTTP client");
        Self {
            client,
            rt: tokio::runtime::Handle::current(),
        }
    }

    pub async fn search_async(&self, query: &str) -> Result<Vec<NewFood>> {
        let resp = self
            .client
            .get(SEARCH_URL)
            .query(&[("search_terms", query), ("json", "1"), ("page_size", "10")])
            .send()
            .await
            .context("Failed to reach OpenFoodFacts API")?;

        let data: SearchResponse = resp
            .json()
            .await
            .context("Failed to parse OpenFoodFacts search response")?;

        let foods: Vec<NewFood> = data
            .products
            .into_iter()
            .filter_map(product_to_food)
            .collect();

        Ok(foods)
    }

    pub async fn lookup_barcode_async(&self, barcode: &str) -> Result<Option<NewFood>> {
        let url = format!("{PRODUCT_URL}/{barcode}.json");
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to reach OpenFoodFacts API")?;

        let data: ProductResponse = resp
            .json()
            .await
            .context("Failed to parse OpenFoodFacts barcode response")?;

        if data.status != 1 {
            return Ok(None);
        }

        Ok(data.product.and_then(product_to_food))
    }
}

impl FoodLookupProvider for OpenFoodFactsClient {
    fn search(&self, query: &str) -> Result<Vec<NewFood>> {
        self.rt.block_on(self.search_async(query))
    }

    fn lookup_barcode(&self, barcode: &str) -> Result<Option<NewFood>> {
        self.rt.block_on(self.lookup_barcode_async(barcode))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grub_core::openfoodfacts::{Nutriments, ProductData};

    fn full_product() -> ProductData {
        ProductData {
            product_name: Some("Nutella".to_string()),
            brands: Some("Ferrero".to_string()),
            code: Some("3017620422003".to_string()),
            nutriments: Some(Nutriments {
                energy_kcal_100g: Some(539.0),
                proteins_100g: Some(6.3),
                carbohydrates_100g: Some(57.5),
                fat_100g: Some(30.9),
            }),
        }
    }

    #[test]
    fn test_product_to_food_complete() {
        let food = product_to_food(full_product()).unwrap();
        assert_eq!(food.name, "Nutella");
        assert_eq!(food.brand.as_deref(), Some("Ferrero"));
        assert_eq!(food.barcode.as_deref(), Some("3017620422003"));
        assert_eq!(food.calories_per_100g, 539.0);
        assert_eq!(food.protein_per_100g, Some(6.3));
        assert_eq!(food.carbs_per_100g, Some(57.5));
        assert_eq!(food.fat_per_100g, Some(30.9));
        assert_eq!(food.source, "openfoodfacts");
    }

    #[test]
    fn test_product_to_food_missing_name() {
        let mut p = full_product();
        p.product_name = None;
        assert!(product_to_food(p).is_none());

        // Empty name should also return None
        let mut p2 = full_product();
        p2.product_name = Some("".to_string());
        assert!(product_to_food(p2).is_none());
    }

    #[test]
    fn test_product_to_food_missing_calories() {
        let mut p = full_product();
        p.nutriments.as_mut().unwrap().energy_kcal_100g = None;
        assert!(product_to_food(p).is_none());

        // Missing nutriments entirely
        let mut p2 = full_product();
        p2.nutriments = None;
        assert!(product_to_food(p2).is_none());
    }

    #[test]
    fn test_product_to_food_minimal() {
        let p = ProductData {
            product_name: Some("Plain Oats".to_string()),
            brands: None,
            code: None,
            nutriments: Some(Nutriments {
                energy_kcal_100g: Some(389.0),
                proteins_100g: None,
                carbohydrates_100g: None,
                fat_100g: None,
            }),
        };
        let food = product_to_food(p).unwrap();
        assert_eq!(food.name, "Plain Oats");
        assert!(food.brand.is_none());
        assert!(food.barcode.is_none());
        assert_eq!(food.calories_per_100g, 389.0);
        assert!(food.protein_per_100g.is_none());
        assert!(food.carbs_per_100g.is_none());
        assert!(food.fat_per_100g.is_none());
    }

    // --- Integration tests (hit real OpenFoodFacts API) ---

    #[tokio::test]
    #[ignore = "hits OpenFoodFacts API"]
    async fn test_lookup_barcode_known_product() {
        let client = OpenFoodFactsClient::new();
        let result = client.lookup_barcode_async("3017620422003").await.unwrap();
        let food = result.expect("Nutella should exist in OpenFoodFacts");
        assert!(food.name.to_lowercase().contains("nutella"));
        assert!(food.calories_per_100g > 0.0);
        assert_eq!(food.barcode.as_deref(), Some("3017620422003"));
    }

    #[tokio::test]
    #[ignore = "hits OpenFoodFacts API"]
    async fn test_lookup_barcode_not_found() {
        let client = OpenFoodFactsClient::new();
        let result = client.lookup_barcode_async("0000000000000").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore = "hits OpenFoodFacts API"]
    async fn test_search_returns_results() {
        let client = OpenFoodFactsClient::new();
        let results = client.search_async("nutella").await.unwrap();
        assert!(!results.is_empty());
        // Every result should have a name and calories
        for food in &results {
            assert!(!food.name.is_empty());
            assert!(food.calories_per_100g > 0.0);
        }
    }
}
