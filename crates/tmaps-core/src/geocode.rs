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
    #[serde(skip)]
    pub kind: Option<PlaceKind>,
    #[serde(skip)]
    pub extent: Option<Extent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceKind {
    House,
    Street,
    Locality,
    District,
    City,
    County,
    State,
    Country,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Extent {
    pub north_west: LatLon,
    pub south_east: LatLon,
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
