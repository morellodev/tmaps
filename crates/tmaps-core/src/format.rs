//! Distances and durations as text for people.

use crate::config::Units;

const METERS_PER_MILE: f64 = 1609.344;
const FEET_PER_METER: f64 = 3.280_84;

/// A distance in `units`, with less precision for longer distances.
#[must_use]
pub fn distance(meters: f64, units: Units) -> String {
    match units {
        Units::Metric if meters < 950.0 => format!("{:.0} m", round_short(meters)),
        Units::Metric if meters < 100_000.0 => format!("{:.1} km", meters / 1000.0),
        Units::Metric => format!("{:.0} km", meters / 1000.0),
        Units::Imperial => {
            let miles = meters / METERS_PER_MILE;
            if miles < 0.1 {
                format!("{:.0} ft", round_short(meters * FEET_PER_METER))
            } else if miles < 100.0 {
                format!("{miles:.1} mi")
            } else {
                format!("{miles:.0} mi")
            }
        }
    }
}

/// Short distances to the nearest 10, because more precision is noise in a street view.
fn round_short(value: f64) -> f64 {
    if value < 100.0 {
        value.round()
    } else {
        (value / 10.0).round() * 10.0
    }
}

#[must_use]
pub fn duration(seconds: f64) -> String {
    let minutes = (seconds / 60.0).round().max(1.0);
    if minutes < 60.0 {
        return format!("{minutes:.0} min");
    }
    let hours = (minutes / 60.0).floor();
    let rest = minutes - hours * 60.0;
    if rest == 0.0 {
        format!("{hours:.0} h")
    } else {
        format!("{hours:.0} h {rest:.0} min")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod distance {
        use super::*;

        #[test]
        fn rounds_metric_meters_to_tens() {
            assert_eq!(distance(347.0, Units::Metric), "350 m");
        }

        #[test]
        fn shows_kilometers_with_one_decimal() {
            assert_eq!(distance(4232.0, Units::Metric), "4.2 km");
        }

        #[test]
        fn shows_short_imperial_distance_in_feet() {
            assert_eq!(distance(100.0, Units::Imperial), "330 ft");
        }

        #[test]
        fn shows_imperial_distance_in_miles() {
            assert_eq!(distance(4232.0, Units::Imperial), "2.6 mi");
        }
    }

    mod duration {
        use super::*;

        #[test]
        fn shows_at_least_one_minute() {
            assert_eq!(duration(12.0), "1 min");
        }

        #[test]
        fn shows_hours_and_minutes() {
            assert_eq!(duration(3900.0), "1 h 5 min");
        }

        #[test]
        fn omits_zero_minutes() {
            assert_eq!(duration(7200.0), "2 h");
        }
    }
}
