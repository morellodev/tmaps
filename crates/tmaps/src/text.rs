//! Text output for people. JSON output has the same data in SI units.

use std::fmt::Write;

use jiff::Zoned;
use tmaps_core::config::Units;
use tmaps_core::routing::Route;

use crate::output::{EtaDoc, GeocodeDoc, PlacesDoc, ReverseDoc, RouteDoc};

const METERS_PER_MILE: f64 = 1609.344;
const FEET_PER_METER: f64 = 3.280_84;

pub trait Render {
    fn render(&self, units: Units) -> String;
}

impl Render for GeocodeDoc {
    fn render(&self, _: Units) -> String {
        if self.places.is_empty() {
            return format!("No place matches {:?}.\n", self.query);
        }
        let mut out = String::new();
        for (index, place) in self.places.iter().enumerate() {
            let _ = writeln!(out, "{}. {}", index + 1, place.label);
            let _ = write!(out, "   {}", place.location);
            if let Some(category) = &place.category {
                let _ = write!(out, " · {category}");
            }
            out.push('\n');
        }
        let _ = writeln!(out, "\n{}", self.attribution);
        out
    }
}

impl Render for ReverseDoc {
    fn render(&self, _: Units) -> String {
        match &self.place {
            Some(place) => format!(
                "{}\n{}\n\n{}\n",
                place.label, place.location, self.attribution
            ),
            None => format!("No place found near {}.\n", self.location),
        }
    }
}

impl Render for RouteDoc {
    fn render(&self, units: Units) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{} → {}",
            self.from.display_name(),
            self.to.display_name()
        );
        let _ = writeln!(
            out,
            "{} · depart {} · free-flow estimate, no traffic",
            self.mode,
            clock(&self.depart_at.0, &Zoned::now())
        );
        for (index, item) in self.routes.iter().enumerate() {
            let route = &item.route;
            let _ = write!(
                out,
                "\nRoute {}: {}, {}, arrive {}",
                index + 1,
                distance(route.distance_m, units),
                duration(route.duration_s),
                clock(&item.arrive_at.0, &self.depart_at.0)
            );
            let flags = route_flags(route);
            if !flags.is_empty() {
                let _ = write!(out, " · {}", flags.join(", "));
            }
            out.push('\n');
            for (number, step) in route.steps.iter().enumerate() {
                let _ = write!(out, "{:>4}. {}", number + 1, step.instruction);
                if step.distance_m > 0.0 {
                    let _ = write!(out, " ({})", distance(step.distance_m, units));
                }
                out.push('\n');
            }
        }
        let _ = writeln!(out, "\n{}", self.attribution);
        out
    }
}

impl Render for EtaDoc {
    fn render(&self, units: Units) -> String {
        format!(
            "{}, {}, arrive {}\n{} → {}\n{} · depart {} · free-flow estimate, no traffic\n\n{}\n",
            duration(self.duration_s),
            distance(self.distance_m, units),
            clock(&self.arrive_at.0, &self.depart_at.0),
            self.from.display_name(),
            self.to.display_name(),
            self.mode,
            clock(&self.depart_at.0, &Zoned::now()),
            self.attribution
        )
    }
}

impl Render for PlacesDoc {
    fn render(&self, _: Units) -> String {
        if self.places.is_empty() {
            return "No saved places. Add one with `tmaps places add <name> <location>`.\n"
                .to_owned();
        }
        let width = self
            .places
            .iter()
            .map(|place| place.name.len())
            .max()
            .unwrap_or_default();
        let mut out = String::new();
        for place in &self.places {
            let _ = writeln!(out, "@{:<width$}  {}", place.name, place.location);
        }
        out
    }
}

fn route_flags(route: &Route) -> Vec<&'static str> {
    [
        (route.has_toll, "tolls"),
        (route.has_highway, "highways"),
        (route.has_ferry, "ferries"),
    ]
    .into_iter()
    .filter_map(|(present, flag)| present.then_some(flag))
    .collect()
}

fn distance(meters: f64, units: Units) -> String {
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

fn duration(seconds: f64) -> String {
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

/// Time of day. Adds the weekday for a later day in the same week as `reference`, and the date
/// for other days, so that a time next week does not look like this week.
fn clock(time: &Zoned, reference: &Zoned) -> String {
    let days_ahead = reference
        .date()
        .until(time.date())
        .map_or(i32::MAX, |span| span.get_days());
    let format = match days_ahead {
        0 => "%H:%M",
        1..=6 => "%a %H:%M",
        _ => "%a %Y-%m-%d %H:%M",
    };
    time.strftime(format).to_string()
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

    mod clock {
        use super::*;

        #[test]
        fn shows_weekday_for_another_day() {
            let depart: Zoned = "2026-10-26T23:30:00+00:00[UTC]".parse().unwrap();
            let arrive: Zoned = "2026-10-27T00:10:00+00:00[UTC]".parse().unwrap();
            assert_eq!(clock(&arrive, &depart), "Tue 00:10");
        }

        #[test]
        fn shows_date_from_seven_days_ahead() {
            let now: Zoned = "2026-10-26T09:00:00+00:00[UTC]".parse().unwrap();
            let depart: Zoned = "2026-11-03T08:00:00+00:00[UTC]".parse().unwrap();
            assert_eq!(clock(&depart, &now), "Tue 2026-11-03 08:00");
        }

        #[test]
        fn shows_date_for_a_past_day() {
            let now: Zoned = "2026-10-26T09:00:00+00:00[UTC]".parse().unwrap();
            let depart: Zoned = "2026-10-25T08:00:00+00:00[UTC]".parse().unwrap();
            assert_eq!(clock(&depart, &now), "Sun 2026-10-25 08:00");
        }
    }
}
