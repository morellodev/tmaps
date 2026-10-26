use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Error, LatLon, Result};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Car,
    Bike,
    Foot,
}

impl FromStr for Mode {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "car" => Ok(Self::Car),
            "bike" => Ok(Self::Bike),
            "foot" => Ok(Self::Foot),
            _ => Err(Error::InvalidInput(format!(
                "unknown mode {s:?}, expected car, bike, or foot"
            ))),
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Car => "car",
            Self::Bike => "bike",
            Self::Foot => "foot",
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RouteRequest {
    pub from: LatLon,
    pub to: LatLon,
    pub mode: Mode,
    /// Upper bound, capped at 2. The provider can return fewer alternatives, or none.
    pub alternatives: u8,
    /// BCP 47 language tag for the instructions.
    pub lang: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Route {
    pub distance_m: f64,
    pub duration_s: f64,
    pub has_toll: bool,
    pub has_highway: bool,
    pub has_ferry: bool,
    pub steps: Vec<Step>,
    #[serde(skip)]
    pub geometry: Vec<LatLon>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Step {
    pub instruction: String,
    pub maneuver: Maneuver,
    pub street_names: Vec<String>,
    pub distance_m: f64,
    pub duration_s: f64,
    /// Time from departure to the start of this step.
    pub elapsed_s: f64,
    pub location: LatLon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Maneuver {
    Depart,
    Arrive,
    Continue,
    TurnSlightRight,
    TurnRight,
    TurnSharpRight,
    UturnRight,
    UturnLeft,
    TurnSharpLeft,
    TurnLeft,
    TurnSlightLeft,
    RampStraight,
    RampRight,
    RampLeft,
    ExitRight,
    ExitLeft,
    KeepStraight,
    KeepRight,
    KeepLeft,
    Merge,
    MergeRight,
    MergeLeft,
    RoundaboutEnter,
    RoundaboutExit,
    FerryEnter,
    FerryExit,
    Other,
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;

    mod mode_from_str {
        use super::*;

        #[test]
        fn parses_every_display_name() {
            let parsed: Vec<Mode> = ["car", "bike", "foot"]
                .iter()
                .map(|name| name.parse().unwrap())
                .collect();
            assert_eq!(parsed, [Mode::Car, Mode::Bike, Mode::Foot]);
        }

        #[test]
        fn returns_invalid_input_for_unknown_mode() {
            assert_matches!("train".parse::<Mode>(), Err(Error::InvalidInput(_)));
        }
    }
}
