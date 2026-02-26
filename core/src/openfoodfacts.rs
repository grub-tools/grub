use serde::Deserialize;

use crate::models::NewFood;

#[derive(Debug, Deserialize)]
pub struct SearchResponse {
    pub products: Vec<ProductData>,
}

#[derive(Debug, Deserialize)]
pub struct ProductResponse {
    pub status: i32,
    pub product: Option<ProductData>,
}

#[derive(Debug, Deserialize)]
pub struct ProductData {
    pub product_name: Option<String>,
    pub brands: Option<String>,
    pub code: Option<String>,
    pub nutriments: Option<Nutriments>,
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_field_names)]
pub struct Nutriments {
    #[serde(rename = "energy-kcal_100g")]
    pub energy_kcal_100g: Option<f64>,
    pub proteins_100g: Option<f64>,
    pub carbohydrates_100g: Option<f64>,
    pub fat_100g: Option<f64>,
}

#[must_use]
pub fn product_to_food(p: ProductData) -> Option<NewFood> {
    let name = p.product_name.filter(|n| !n.is_empty())?;
    let nutriments = p.nutriments?;
    let calories = nutriments.energy_kcal_100g?;

    Some(NewFood {
        name,
        brand: p.brands.filter(|b| !b.is_empty()),
        barcode: p.code.filter(|c| !c.is_empty()),
        calories_per_100g: calories,
        protein_per_100g: nutriments.proteins_100g,
        carbs_per_100g: nutriments.carbohydrates_100g,
        fat_per_100g: nutriments.fat_100g,
        default_serving_g: None,
        source: "openfoodfacts".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
