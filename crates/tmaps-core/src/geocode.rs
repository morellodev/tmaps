use schemars::JsonSchema;
use serde::Serialize;

use crate::LatLon;

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Place {
    pub label: String,
    pub name: Option<String>,
    pub location: LatLon,
    /// OSM `key:value` tag, for example `railway:station`.
    pub category: Option<String>,
    pub country_code: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Ranks results near this point higher.
    pub near: Option<LatLon>,
    pub limit: usize,
    /// BCP 47 language tag for result names.
    pub lang: Option<String>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            near: None,
            limit: 5,
            lang: None,
        }
    }
}
