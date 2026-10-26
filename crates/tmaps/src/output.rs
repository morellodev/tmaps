//! JSON documents that the CLI writes, and their JSON Schemas.

use schemars::{JsonSchema, Schema, schema_for};
use serde::Serialize;
use tmaps_core::LatLon;
use tmaps_core::geocode::Place;
use tmaps_core::routing::{Mode, Route};

use crate::error::CliError;
use crate::location::Resolved;
use crate::time::DateTime;

pub const GEOCODE_SCHEMA: &str = "tmaps/geocode@1";
pub const REVERSE_SCHEMA: &str = "tmaps/reverse@1";
pub const ROUTE_SCHEMA: &str = "tmaps/route@1";
pub const ETA_SCHEMA: &str = "tmaps/eta@1";
pub const PLACES_SCHEMA: &str = "tmaps/places@1";
pub const ERROR_SCHEMA: &str = "tmaps/error@1";
pub const TRAFFIC: &str = "none";
pub const ETA_BASIS: &str = "free-flow";
pub const ERROR_KINDS: [&str; 8] = [
    "invalid_input",
    "ambiguous",
    "usage",
    "no_route",
    "network",
    "provider",
    "config",
    "output",
];

/// Returns each schema with its file name, for example `route@1`.
#[must_use]
pub fn schemas() -> Vec<(&'static str, Schema)> {
    [
        (GEOCODE_SCHEMA, schema_for!(GeocodeDoc)),
        (REVERSE_SCHEMA, schema_for!(ReverseDoc)),
        (ROUTE_SCHEMA, schema_for!(RouteDoc)),
        (ETA_SCHEMA, schema_for!(EtaDoc)),
        (PLACES_SCHEMA, schema_for!(PlacesDoc)),
        (ERROR_SCHEMA, schema_for!(ErrorDoc)),
    ]
    .into_iter()
    .map(|(id, schema)| (id.trim_start_matches("tmaps/"), schema))
    .collect()
}

#[derive(Serialize, JsonSchema)]
pub struct GeocodeDoc {
    #[schemars(extend("const" = GEOCODE_SCHEMA))]
    pub schema: &'static str,
    pub attribution: &'static str,
    pub query: String,
    pub places: Vec<Place>,
}

#[derive(Serialize, JsonSchema)]
pub struct ReverseDoc {
    #[schemars(extend("const" = REVERSE_SCHEMA))]
    pub schema: &'static str,
    pub attribution: &'static str,
    pub location: LatLon,
    /// Null when the geocoder knows no place near `location`.
    pub place: Option<Place>,
}

#[derive(Serialize, JsonSchema)]
pub struct RouteDoc {
    #[schemars(extend("const" = ROUTE_SCHEMA))]
    pub schema: &'static str,
    pub attribution: &'static str,
    pub mode: Mode,
    pub from: Resolved,
    pub to: Resolved,
    #[schemars(extend("const" = TRAFFIC))]
    pub traffic: &'static str,
    #[schemars(extend("const" = ETA_BASIS))]
    pub eta_basis: &'static str,
    pub depart_at: DateTime,
    /// The best route first, then the alternatives.
    pub routes: Vec<RouteItem>,
}

#[derive(Serialize, JsonSchema)]
pub struct RouteItem {
    #[serde(flatten)]
    pub route: Route,
    pub arrive_at: DateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geometry: Option<Geometry>,
}

#[derive(Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Geometry {
    /// Encoded polyline with precision 6.
    Polyline6(String),
    GeoJson(LineString),
}

/// `GeoJSON` `LineString`, with `[lon, lat]` positions.
#[derive(Serialize, JsonSchema)]
pub struct LineString {
    #[serde(rename = "type")]
    #[schemars(extend("const" = "LineString"))]
    pub kind: &'static str,
    pub coordinates: Vec<[f64; 2]>,
}

#[derive(Serialize, JsonSchema)]
pub struct EtaDoc {
    #[schemars(extend("const" = ETA_SCHEMA))]
    pub schema: &'static str,
    pub attribution: &'static str,
    pub mode: Mode,
    pub from: Resolved,
    pub to: Resolved,
    #[schemars(extend("const" = TRAFFIC))]
    pub traffic: &'static str,
    #[schemars(extend("const" = ETA_BASIS))]
    pub eta_basis: &'static str,
    pub depart_at: DateTime,
    pub arrive_at: DateTime,
    pub distance_m: f64,
    pub duration_s: f64,
}

#[derive(Serialize, JsonSchema)]
pub struct PlacesDoc {
    #[schemars(extend("const" = PLACES_SCHEMA))]
    pub schema: &'static str,
    pub attribution: &'static str,
    pub places: Vec<SavedPlace>,
}

#[derive(Serialize, JsonSchema)]
pub struct SavedPlace {
    /// Use as `@name` in a LOCATION argument.
    pub name: String,
    pub location: LatLon,
}

#[derive(Serialize, JsonSchema)]
pub struct ErrorDoc<'a> {
    #[schemars(extend("const" = ERROR_SCHEMA))]
    pub schema: &'static str,
    pub error: ErrorBody<'a>,
}

#[derive(Serialize, JsonSchema)]
pub struct ErrorBody<'a> {
    #[schemars(extend("enum" = ERROR_KINDS))]
    pub kind: &'static str,
    pub message: String,
    /// The matches of an `ambiguous` LOCATION.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidates: Option<&'a [Place]>,
}

impl<'a> ErrorDoc<'a> {
    pub fn new(error: &'a CliError) -> Self {
        Self {
            schema: ERROR_SCHEMA,
            error: ErrorBody {
                kind: error.kind(),
                message: error.to_string(),
                candidates: error.candidates(),
            },
        }
    }
}
