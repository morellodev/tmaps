use std::borrow::Cow;

use jiff::civil::{self, Time};
use jiff::{SignedDuration, Timestamp, Zoned};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Serialize, Serializer};
use tmaps_core::Error;

/// Accepts RFC 3339, or a civil date-time or time of day in the time zone of `now`.
pub fn parse_depart_at(text: &str, now: &Zoned) -> Result<Zoned, String> {
    let zone = now.time_zone().clone();
    if let Ok(timestamp) = text.parse::<Timestamp>() {
        return Ok(timestamp.to_zoned(zone));
    }
    // `strptime` accepts single digits, so the length check stops "18:4" from meaning 18:04.
    let datetime = if text.len() == "YYYY-MM-DDTHH:MM".len()
        && let Ok(datetime) = civil::DateTime::strptime("%Y-%m-%dT%H:%M", text)
    {
        datetime
    } else if text.len() == "HH:MM".len()
        && let Ok(time) = Time::strptime("%H:%M", text)
    {
        now.date().to_datetime(time)
    } else {
        return Err(format!(
            "expected RFC 3339, \"YYYY-MM-DDTHH:MM\", or \"HH:MM\", got {text:?}"
        ));
    };
    datetime.to_zoned(zone).map_err(|error| error.to_string())
}

/// # Errors
///
/// [`Error::InvalidInput`] if the arrival time is outside the supported range.
pub fn arrive_at(depart_at: &Zoned, duration_s: f64) -> Result<Zoned, Error> {
    SignedDuration::try_from_secs_f64(duration_s.round())
        .and_then(|duration| depart_at.checked_add(duration))
        .map_err(|error| Error::InvalidInput(format!("arrival time out of range: {error}")))
}

/// A zoned time that serializes as RFC 3339 with a numeric offset.
#[derive(Debug, Clone, PartialEq)]
pub struct DateTime(pub Zoned);

impl Serialize for DateTime {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0.strftime("%Y-%m-%dT%H:%M:%S%:z"))
    }
}

impl JsonSchema for DateTime {
    fn schema_name() -> Cow<'static, str> {
        "DateTime".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string", "format": "date-time" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Zoned {
        "2026-10-26T09:30:00+01:00[Europe/Rome]".parse().unwrap()
    }

    mod parse_depart_at {
        use super::*;

        #[test]
        fn converts_rfc3339_to_the_local_zone() {
            let zoned = parse_depart_at("2026-10-26T13:00:00Z", &now()).unwrap();
            assert_eq!(zoned.strftime("%H:%M").to_string(), "14:00");
        }

        #[test]
        fn reads_civil_datetime_in_the_local_zone() {
            let zoned = parse_depart_at("2026-10-27T08:15", &now()).unwrap();
            assert_eq!(zoned.to_string(), "2026-10-27T08:15:00+01:00[Europe/Rome]");
        }

        #[test]
        fn reads_time_of_day_as_today() {
            let zoned = parse_depart_at("18:45", &now()).unwrap();
            assert_eq!(zoned.to_string(), "2026-10-26T18:45:00+01:00[Europe/Rome]");
        }

        #[test]
        fn rejects_date_without_time() {
            assert!(parse_depart_at("2026-10-27", &now()).is_err());
        }

        #[test]
        fn rejects_single_digit_minutes() {
            assert!(parse_depart_at("18:4", &now()).is_err());
        }

        #[test]
        fn rejects_single_digit_month() {
            assert!(parse_depart_at("2026-1-27T08:15", &now()).is_err());
        }

        #[test]
        fn rejects_time_without_colon() {
            assert!(parse_depart_at("1845", &now()).is_err());
        }

        #[test]
        fn rejects_other_text() {
            assert!(parse_depart_at("tomorrow", &now()).is_err());
        }
    }

    #[test]
    fn arrive_at_adds_rounded_duration() {
        let arrival = arrive_at(&now(), 89.6).unwrap();
        assert_eq!(arrival.strftime("%H:%M:%S").to_string(), "09:31:30");
    }

    #[test]
    fn date_time_serializes_as_rfc3339() {
        let json = serde_json::to_string(&DateTime(now())).unwrap();
        assert_eq!(json, r#""2026-10-26T09:30:00+01:00""#);
    }
}
