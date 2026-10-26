use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use jiff::Zoned;
use tmaps_core::config::{Units, normalize_locale};
use tmaps_core::routing::Mode;

use crate::time;

/// Terminal maps with a route planner. Without a subcommand, tmaps shows the map. The
/// subcommands are a CLI for scripts and AI agents.
///
/// A LOCATION is "lat,lon", @place (see `tmaps places`), or free text. Free text resolves
/// to the best geocoder match; add --strict to fail when the text matches more than one place.
#[derive(Debug, Parser)]
#[command(name = "tmaps", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Output format. Default: text on a terminal, JSON otherwise.
    #[arg(long, global = true, value_enum)]
    pub format: Option<Format>,

    /// Config file. Default: ~/.config/tmaps/config.toml.
    #[arg(long, global = true, env = "TMAPS_CONFIG", value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Units for text output: metric or imperial. JSON always uses meters and seconds.
    #[arg(long, global = true)]
    pub units: Option<Units>,

    /// BCP 47 language tag for place names and instructions, for example "it-IT".
    #[arg(long, global = true, value_name = "TAG", value_parser = parse_lang)]
    pub lang: Option<String>,

    /// Fail with exit code 2 when free text matches more than one place.
    #[arg(long, global = true)]
    pub strict: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Json,
    Text,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Find places that match a text query.
    Geocode {
        query: String,
        /// Rank results near this LOCATION higher.
        #[arg(long, value_name = "LOCATION")]
        near: Option<String>,
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u8).range(1..=20))]
        limit: u8,
    },
    /// Find the place at a LOCATION.
    Reverse { location: String },
    /// Plan a route with turn-by-turn steps.
    Route(RouteArgs),
    /// Get the travel time, arrival time, and distance of the best route.
    Eta(TripArgs),
    /// Manage saved places, which you can use as @name.
    Places {
        #[command(subcommand)]
        action: PlacesAction,
    },
}

#[derive(Debug, Args)]
pub struct TripArgs {
    #[arg(long, value_name = "LOCATION")]
    pub from: String,
    #[arg(long, value_name = "LOCATION")]
    pub to: String,
    /// Travel mode: car, bike, or foot.
    #[arg(long, default_value = "car")]
    pub mode: Mode,
    /// Departure time: RFC 3339, "YYYY-MM-DDTHH:MM", or "HH:MM" (today) in the local time zone.
    /// Only the arrival time changes: estimates never include traffic. Default: now.
    #[arg(long, value_name = "TIME", value_parser = parse_depart_at)]
    pub depart_at: Option<Zoned>,
}

#[derive(Debug, Args)]
pub struct RouteArgs {
    #[command(flatten)]
    pub trip: TripArgs,
    /// Maximum number of alternative routes. The router can return fewer.
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=2))]
    pub alternatives: u8,
    /// Add the route geometry to the output.
    #[arg(long, value_enum)]
    pub geometry: Option<GeometryFormat>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum GeometryFormat {
    Polyline6,
    #[value(name = "geojson")]
    GeoJson,
}

#[derive(Debug, Subcommand)]
pub enum PlacesAction {
    /// Save a LOCATION as @NAME. An existing place with the same name is replaced.
    Add { name: String, location: String },
    /// List the saved places.
    List,
    /// Remove a saved place.
    Rm { name: String },
}

fn parse_lang(text: &str) -> Result<String, String> {
    let mut subtags = text.split(['-', '_']);
    let language = subtags.next().unwrap_or_default();
    let well_formed = (2..=3).contains(&language.len())
        && language.chars().all(|c| c.is_ascii_alphabetic())
        && subtags.all(|subtag| {
            (1..=8).contains(&subtag.len()) && subtag.chars().all(|c| c.is_ascii_alphanumeric())
        });
    well_formed
        .then(|| normalize_locale(text))
        .flatten()
        .ok_or_else(|| format!("expected a language tag such as \"en-US\", got {text:?}"))
}

fn parse_depart_at(text: &str) -> Result<Zoned, String> {
    time::parse_depart_at(text, &Zoned::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    mod parse_lang {
        use super::*;

        #[test]
        fn normalizes_case_and_separator() {
            assert_eq!(parse_lang("zh_hans_cn").unwrap(), "zh-Hans-CN");
        }

        #[test]
        fn rejects_text_with_spaces() {
            assert!(parse_lang("not a tag").is_err());
        }

        #[test]
        fn rejects_posix_locale_with_encoding() {
            assert!(parse_lang("en_US.UTF-8").is_err());
        }
    }
}
