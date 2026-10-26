use std::fmt;
use std::str::FromStr;

use serde::Serialize;

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LatLon {
    lat: f64,
    lon: f64,
}

impl LatLon {
    /// # Errors
    ///
    /// [`Error::InvalidInput`] if `lat` is outside ±90 or `lon` is outside ±180.
    pub fn new(lat: f64, lon: f64) -> Result<Self> {
        if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) {
            Ok(Self { lat, lon })
        } else {
            Err(Error::InvalidInput(format!(
                "coordinates out of range: {lat},{lon}"
            )))
        }
    }

    #[must_use]
    pub fn lat(self) -> f64 {
        self.lat
    }

    #[must_use]
    pub fn lon(self) -> f64 {
        self.lon
    }
}

impl FromStr for LatLon {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let invalid = || Error::InvalidInput(format!("expected \"lat,lon\", got {s:?}"));
        let (lat, lon) = s.split_once(',').ok_or_else(invalid)?;
        let lat = lat.trim().parse().map_err(|_| invalid())?;
        let lon = lon.trim().parse().map_err(|_| invalid())?;
        Self::new(lat, lon)
    }
}

impl fmt::Display for LatLon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{},{}", self.lat, self.lon)
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;

    mod from_str {
        use super::*;

        #[test]
        fn accepts_spaces_around_values() {
            let point: LatLon = " 45.4642, 9.19 ".parse().unwrap();
            assert_eq!(point, LatLon::new(45.4642, 9.19).unwrap());
        }

        #[test]
        fn rejects_missing_longitude() {
            assert_matches!("45.4642".parse::<LatLon>(), Err(Error::InvalidInput(_)));
        }

        #[test]
        fn rejects_non_numeric_values() {
            assert_matches!("a,b".parse::<LatLon>(), Err(Error::InvalidInput(_)));
        }

        #[test]
        fn rejects_three_values() {
            assert_matches!("45,9,1".parse::<LatLon>(), Err(Error::InvalidInput(_)));
        }

        #[test]
        fn rejects_latitude_out_of_range() {
            assert_matches!("91,0".parse::<LatLon>(), Err(Error::InvalidInput(_)));
        }

        #[test]
        fn rejects_longitude_out_of_range() {
            assert_matches!("0,181".parse::<LatLon>(), Err(Error::InvalidInput(_)));
        }

        #[test]
        fn rejects_nan() {
            assert_matches!("NaN,0".parse::<LatLon>(), Err(Error::InvalidInput(_)));
        }
    }

    #[test]
    fn serializes_as_lat_lon_object() {
        let json = serde_json::to_string(&LatLon::new(45.5, 9.25).unwrap()).unwrap();
        assert_eq!(json, r#"{"lat":45.5,"lon":9.25}"#);
    }
}
