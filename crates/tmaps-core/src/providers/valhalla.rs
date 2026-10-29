use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::http::{Throttle, body_snippet};
use crate::routing::{Maneuver, Mode, Route, RouteRequest, Step};
use crate::{Error, LatLon, Result};

const POLYLINE_PRECISION: u32 = 6;
/// The public FOSSGIS server asks for at most about one request per second.
const MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(1);
/// Valhalla error codes that mean "these inputs have no route", as opposed to a server fault.
/// See <https://valhalla.github.io/valhalla/api/route/api-reference/>.
const NO_ROUTE_ERROR_CODES: [u32; 5] = [154, 170, 171, 441, 442];
const MAX_ALTERNATIVES: u8 = 2;

/// Router backed by a [Valhalla](https://github.com/valhalla/valhalla) server.
pub struct Valhalla {
    client: reqwest::Client,
    base_url: String,
    throttle: Throttle,
}

impl Valhalla {
    #[must_use]
    pub fn new(client: reqwest::Client, base_url: &str) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_owned(),
            throttle: Throttle::new(MIN_REQUEST_INTERVAL),
        }
    }

    /// Returns the primary route first, then at most two alternatives. Valhalla can return fewer
    /// alternatives than requested, or none. Calls on one instance are spaced at least
    /// one second apart.
    ///
    /// # Errors
    ///
    /// - [`Error::NoRoute`] if no route connects the points, a point is too far from a road,
    ///   or the distance is over the server limit.
    /// - [`Error::Network`] if the request fails.
    /// - [`Error::Provider`] for other Valhalla errors or an unexpected body.
    pub async fn route(&self, request: &RouteRequest) -> Result<Vec<Route>> {
        // No departure time: Valhalla gives no alternates for a time-dependent route.
        let body = RouteBody {
            locations: [request.from.into(), request.to.into()],
            costing: costing(request.mode),
            alternates: Some(request.alternatives.min(MAX_ALTERNATIVES)).filter(|&count| count > 0),
            units: "kilometers",
            language: request.lang.as_deref(),
        };

        self.throttle.wait().await;
        let response = self
            .client
            .post(format!("{}/route", self.base_url))
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        if !status.is_success() {
            return Err(route_error(status, &bytes));
        }
        let response: RouteResponse = serde_json::from_slice(&bytes)
            .map_err(|error| Error::Provider(format!("unexpected Valhalla response: {error}")))?;

        std::iter::once(response.trip)
            .chain(
                response
                    .alternates
                    .into_iter()
                    .map(|alternate| alternate.trip),
            )
            .map(Trip::into_route)
            .collect()
    }
}

fn costing(mode: Mode) -> &'static str {
    match mode {
        Mode::Car => "auto",
        Mode::Bike => "bicycle",
        Mode::Foot => "pedestrian",
    }
}

fn route_error(status: reqwest::StatusCode, body: &[u8]) -> Error {
    match serde_json::from_slice::<ErrorBody>(body) {
        Ok(error) if NO_ROUTE_ERROR_CODES.contains(&error.error_code) => {
            Error::NoRoute(error.error)
        }
        Ok(error) => Error::Provider(format!(
            "Valhalla error {}: {}",
            error.error_code, error.error
        )),
        Err(_) => Error::Provider(format!(
            "Valhalla returned HTTP {status}: {}",
            body_snippet(body)
        )),
    }
}

fn maneuver(valhalla_type: u8) -> Maneuver {
    match valhalla_type {
        1..=3 => Maneuver::Depart,
        4..=6 => Maneuver::Arrive,
        7 | 8 => Maneuver::Continue,
        9 => Maneuver::TurnSlightRight,
        10 => Maneuver::TurnRight,
        11 => Maneuver::TurnSharpRight,
        12 => Maneuver::UturnRight,
        13 => Maneuver::UturnLeft,
        14 => Maneuver::TurnSharpLeft,
        15 => Maneuver::TurnLeft,
        16 => Maneuver::TurnSlightLeft,
        17 => Maneuver::RampStraight,
        18 => Maneuver::RampRight,
        19 => Maneuver::RampLeft,
        20 => Maneuver::ExitRight,
        21 => Maneuver::ExitLeft,
        22 => Maneuver::KeepStraight,
        23 => Maneuver::KeepRight,
        24 => Maneuver::KeepLeft,
        25 => Maneuver::Merge,
        26 => Maneuver::RoundaboutEnter,
        27 => Maneuver::RoundaboutExit,
        28 => Maneuver::FerryEnter,
        29 => Maneuver::FerryExit,
        37 => Maneuver::MergeRight,
        38 => Maneuver::MergeLeft,
        _ => Maneuver::Other,
    }
}

#[derive(Serialize)]
struct RouteBody<'a> {
    locations: [BodyLocation; 2],
    costing: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    alternates: Option<u8>,
    units: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'a str>,
}

#[derive(Serialize)]
struct BodyLocation {
    lat: f64,
    lon: f64,
}

impl From<LatLon> for BodyLocation {
    fn from(point: LatLon) -> Self {
        Self {
            lat: point.lat(),
            lon: point.lon(),
        }
    }
}

#[derive(Deserialize)]
struct ErrorBody {
    error_code: u32,
    error: String,
}

#[derive(Deserialize)]
struct RouteResponse {
    trip: Trip,
    #[serde(default)]
    alternates: Vec<Alternate>,
}

#[derive(Deserialize)]
struct Alternate {
    trip: Trip,
}

#[derive(Deserialize)]
struct Trip {
    legs: Vec<Leg>,
    summary: Summary,
}

#[derive(Deserialize)]
struct Summary {
    /// Kilometers, because the request sets `units`.
    length: f64,
    time: f64,
    #[serde(default)]
    has_toll: bool,
    #[serde(default)]
    has_highway: bool,
    #[serde(default)]
    has_ferry: bool,
}

#[derive(Deserialize)]
struct Leg {
    maneuvers: Vec<ValhallaManeuver>,
    shape: String,
}

#[derive(Deserialize)]
struct ValhallaManeuver {
    #[serde(rename = "type")]
    kind: u8,
    #[serde(default)]
    instruction: String,
    #[serde(default)]
    street_names: Vec<String>,
    length: f64,
    time: f64,
    begin_shape_index: usize,
}

impl Trip {
    fn into_route(self) -> Result<Route> {
        let mut geometry = Vec::new();
        let mut steps = Vec::new();
        let mut elapsed_s = 0.0;
        for leg in self.legs {
            let shape = decode_shape(&leg.shape)?;
            for maneuver in leg.maneuvers {
                let location = *shape.get(maneuver.begin_shape_index).ok_or_else(|| {
                    Error::Provider("Valhalla maneuver points outside the route shape".to_owned())
                })?;
                steps.push(Step {
                    instruction: maneuver.instruction,
                    maneuver: self::maneuver(maneuver.kind),
                    street_names: maneuver.street_names,
                    distance_m: kilometers_to_meters(maneuver.length),
                    duration_s: maneuver.time,
                    elapsed_s,
                    location,
                });
                elapsed_s += maneuver.time;
            }
            geometry.extend(shape);
        }
        Ok(Route {
            distance_m: kilometers_to_meters(self.summary.length),
            duration_s: self.summary.time,
            has_toll: self.summary.has_toll,
            has_highway: self.summary.has_highway,
            has_ferry: self.summary.has_ferry,
            steps,
            geometry,
        })
    }
}

fn decode_shape(encoded: &str) -> Result<Vec<LatLon>> {
    let line = polyline::decode_polyline(encoded, POLYLINE_PRECISION)
        .map_err(|error| Error::Provider(format!("invalid Valhalla route shape: {error}")))?;
    line.0
        .into_iter()
        .map(|coord| LatLon::new(coord.y, coord.x))
        .collect()
}

/// Valhalla returns kilometers with three decimals, so whole meters keep all the precision.
fn kilometers_to_meters(kilometers: f64) -> f64 {
    (kilometers * 1000.0).round()
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;

    mod maneuver {
        use super::*;

        #[test]
        fn maps_destination_left_to_arrive() {
            assert_eq!(maneuver(6), Maneuver::Arrive);
        }

        #[test]
        fn maps_transit_types_to_other() {
            assert_eq!(maneuver(30), Maneuver::Other);
        }
    }

    mod route_error {
        use super::*;

        #[test]
        fn maps_no_path_code_to_no_route() {
            let body = br#"{"error_code":442,"error":"No path could be found for input"}"#;
            assert_matches!(
                route_error(reqwest::StatusCode::BAD_REQUEST, body),
                Error::NoRoute(_)
            );
        }

        #[test]
        fn maps_unconnected_regions_code_to_no_route() {
            let body = br#"{"error_code":170,"error":"Locations are in unconnected regions"}"#;
            assert_matches!(
                route_error(reqwest::StatusCode::BAD_REQUEST, body),
                Error::NoRoute(_)
            );
        }

        #[test]
        fn maps_unreachable_location_code_to_no_route() {
            let body = br#"{"error_code":441,"error":"Location is unreachable"}"#;
            assert_matches!(
                route_error(reqwest::StatusCode::BAD_REQUEST, body),
                Error::NoRoute(_)
            );
        }

        #[test]
        fn maps_other_codes_to_provider_error() {
            let body = br#"{"error_code":100,"error":"Failed to parse json request"}"#;
            assert_matches!(
                route_error(reqwest::StatusCode::BAD_REQUEST, body),
                Error::Provider(_)
            );
        }

        #[test]
        fn maps_non_json_body_to_provider_error() {
            assert_matches!(
                route_error(reqwest::StatusCode::BAD_GATEWAY, b"<html>"),
                Error::Provider(_)
            );
        }
    }

    #[test]
    fn kilometers_to_meters_drops_float_noise() {
        assert_eq!(kilometers_to_meters(0.035), 35.0);
    }
}
