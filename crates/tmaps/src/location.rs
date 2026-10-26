use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Serialize;
use tmaps_core::geocode::{Place, SearchOptions};
use tmaps_core::providers::photon::Photon;
use tmaps_core::{Error, LatLon};

use crate::error::CliError;

const STRICT_CANDIDATES: usize = 5;
/// Matches closer than this to the top match are the same place for `--strict`, for example
/// a station and its metro stop.
const SAME_PLACE_RADIUS_M: f64 = 1_000.0;

/// A LOCATION argument and the point it resolved to.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Resolved {
    pub input: String,
    /// Geocoder label for free text, `@name` for a saved place, null for coordinates.
    pub label: Option<String>,
    pub location: LatLon,
}

impl Resolved {
    pub fn display_name(&self) -> String {
        self.label
            .clone()
            .unwrap_or_else(|| self.location.to_string())
    }
}

pub struct Resolver<'a> {
    pub photon: &'a Photon,
    pub places: &'a BTreeMap<String, LatLon>,
    pub lang: Option<&'a str>,
    pub strict: bool,
}

impl Resolver<'_> {
    /// Resolves "lat,lon", @place, or free text. `near` ranks free-text matches near a point higher.
    pub async fn resolve(&self, input: &str, near: Option<LatLon>) -> Result<Resolved, CliError> {
        let input = input.trim();
        if let Some(name) = input.strip_prefix('@') {
            let location = *self.places.get(name).ok_or_else(|| {
                Error::InvalidInput(format!(
                    "no saved place named @{name}; run `tmaps places list`"
                ))
            })?;
            return Ok(Resolved {
                input: input.to_owned(),
                label: Some(format!("@{name}")),
                location,
            });
        }
        if looks_like_coordinates(input) {
            return Ok(Resolved {
                input: input.to_owned(),
                label: None,
                location: input.parse()?,
            });
        }

        let options = SearchOptions {
            near,
            limit: if self.strict { STRICT_CANDIDATES } else { 1 },
            lang: self.lang.map(str::to_owned),
        };
        let places = self.photon.search(input, &options).await?;
        if self.strict && has_distinct_matches(&places) {
            return Err(CliError::Ambiguous {
                input: input.to_owned(),
                candidates: places,
            });
        }
        let place = places
            .into_iter()
            .next()
            .ok_or_else(|| Error::InvalidInput(format!("no place matches {input:?}")))?;
        Ok(Resolved {
            input: input.to_owned(),
            label: Some(place.label),
            location: place.location,
        })
    }
}

fn has_distinct_matches(places: &[Place]) -> bool {
    let Some((top, rest)) = places.split_first() else {
        return false;
    };
    rest.iter()
        .any(|place| top.location.distance_m(place.location) > SAME_PLACE_RADIUS_M)
}

/// True for two comma-separated numbers, so that "91,0" fails as a coordinate
/// instead of going to the geocoder as text.
fn looks_like_coordinates(input: &str) -> bool {
    input.split_once(',').is_some_and(|(lat, lon)| {
        lat.trim().parse::<f64>().is_ok() && lon.trim().parse::<f64>().is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    mod looks_like_coordinates {
        use super::*;

        #[test]
        fn accepts_out_of_range_numbers() {
            assert!(looks_like_coordinates("91, 0"));
        }

        #[test]
        fn rejects_address_with_comma() {
            assert!(!looks_like_coordinates("Via Roma 3, Milano"));
        }
    }
}
