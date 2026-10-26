//! Text output for people. JSON output has the same data in SI units.

use std::fmt::Write;

use jiff::Zoned;
use tmaps_core::config::Units;
use tmaps_core::format::{distance, duration};
use tmaps_core::routing::Route;

use crate::output::{EtaDoc, GeocodeDoc, PlacesDoc, ReverseDoc, RouteDoc};

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
