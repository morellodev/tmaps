//! The `tmaps` command line: geocoding, routes, ETAs, and saved places, for people and AI agents.

mod args;
mod error;
mod location;
mod output;
mod text;
mod time;

use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use jiff::Zoned;
use serde::Serialize;
use tmaps_core::config::{Config, Paths, Units};
use tmaps_core::geo::encode_polyline6;
use tmaps_core::geocode::SearchOptions;
use tmaps_core::providers::photon::Photon;
use tmaps_core::providers::valhalla::Valhalla;
use tmaps_core::routing::{Route, RouteRequest};
use tmaps_core::{ATTRIBUTION, Error, LatLon, http, places};

use crate::args::{Cli, Command, Format, GeometryFormat, PlacesAction, RouteArgs, TripArgs};
use crate::error::CliError;
use crate::location::{Resolved, Resolver};
pub use crate::output::schemas;
use crate::output::{
    ETA_BASIS, ETA_SCHEMA, ErrorDoc, EtaDoc, GEOCODE_SCHEMA, GeocodeDoc, Geometry, LineString,
    PLACES_SCHEMA, PlacesDoc, REVERSE_SCHEMA, ROUTE_SCHEMA, ReverseDoc, RouteDoc, RouteItem,
    SavedPlace, TRAFFIC,
};
use crate::text::Render;
use crate::time::DateTime;

/// Parses `args` (with the program name first), runs the command, and writes the result.
pub async fn run<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => return usage_error(&error),
    };
    let format = cli.format.unwrap_or_else(|| {
        if io::stdout().is_terminal() {
            Format::Text
        } else {
            Format::Json
        }
    });
    match execute(cli, format).await {
        Ok(()) => ExitCode::SUCCESS,
        // A reader such as `head` closed the pipe because it has what it needs.
        Err(CliError::Output(error)) if error.kind() == io::ErrorKind::BrokenPipe => {
            ExitCode::SUCCESS
        }
        Err(error) => report(&error, format),
    }
}

/// Help and version text go to stdout. Usage errors are JSON when stdout is not a terminal,
/// because an agent reads them.
fn usage_error(error: &clap::Error) -> ExitCode {
    if !error.use_stderr() || io::stdout().is_terminal() {
        let _ = error.print();
        return ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(2));
    }
    let rendered = error.to_string();
    let message = rendered.lines().next().unwrap_or_default();
    let message = message.strip_prefix("error: ").unwrap_or(message);
    report(&CliError::Usage(message.to_owned()), Format::Json)
}

fn report(error: &CliError, format: Format) -> ExitCode {
    let mut stderr = io::stderr().lock();
    // Nothing is left to report a failure to write stderr to.
    let _ = match format {
        Format::Json => serde_json::to_writer(&mut stderr, &ErrorDoc::new(error))
            .map_err(io::Error::from)
            .and_then(|()| writeln!(stderr)),
        Format::Text => write_text_error(&mut stderr, error),
    };
    ExitCode::from(error.exit_code())
}

fn write_text_error(out: &mut impl Write, error: &CliError) -> io::Result<()> {
    writeln!(out, "error: {error}")?;
    for place in error.candidates().unwrap_or_default() {
        writeln!(out, "  {} ({})", place.label, place.location)?;
    }
    Ok(())
}

struct Context {
    config: Config,
    config_path: PathBuf,
    format: Format,
    units: Units,
    lang: String,
    strict: bool,
    photon: Photon,
    valhalla: Valhalla,
}

struct Trip {
    from: Resolved,
    to: Resolved,
    depart_at: Zoned,
    routes: Vec<Route>,
}

async fn execute(cli: Cli, format: Format) -> Result<(), CliError> {
    if cli.command.is_none() && !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        return Err(CliError::Usage(
            "the map needs a terminal; to use tmaps from a script, run a subcommand (see --help)"
                .to_owned(),
        ));
    }
    let config_path = match cli.config {
        Some(path) => path,
        None => Paths::detect()?.config_file(),
    };
    // Before the load, so that a place with an invalid value can be removed.
    if let Some(Command::Places {
        action: PlacesAction::Rm { name },
    }) = &cli.command
    {
        places::remove(&config_path, name)?;
    }
    let config = Config::load(&config_path)?;
    let Some(command) = cli.command else {
        return Ok(tmaps_tui::run(&config, &config_path, &Paths::detect()?).await?);
    };
    let client = http::client()?;
    let context = Context {
        format,
        units: cli.units.unwrap_or(config.units),
        lang: cli.lang.unwrap_or_else(|| config.lang.clone()),
        strict: cli.strict,
        photon: Photon::new(client.clone(), &config.endpoints.photon),
        valhalla: Valhalla::new(client, &config.endpoints.valhalla),
        config,
        config_path,
    };
    match command {
        Command::Geocode { query, near, limit } => context.geocode(query, near, limit).await,
        Command::Reverse { location } => context.reverse(&location).await,
        Command::Route(args) => context.route(&args).await,
        Command::Eta(args) => context.eta(&args).await,
        Command::Places { action } => context.places(action).await,
    }
}

impl Context {
    fn resolver(&self) -> Resolver<'_> {
        Resolver {
            photon: &self.photon,
            places: &self.config.places,
            lang: Some(&self.lang),
            strict: self.strict,
        }
    }

    fn emit<D: Serialize + Render>(&self, document: &D) -> Result<(), CliError> {
        let mut stdout = io::stdout().lock();
        match self.format {
            Format::Json => {
                serde_json::to_writer(&mut stdout, document)?;
                writeln!(stdout)?;
            }
            Format::Text => stdout.write_all(document.render(self.units).as_bytes())?,
        }
        Ok(())
    }

    async fn geocode(
        &self,
        query: String,
        near: Option<String>,
        limit: u8,
    ) -> Result<(), CliError> {
        let near = match near {
            Some(near) => Some(self.resolver().resolve(&near, None).await?.location),
            None => None,
        };
        let options = SearchOptions {
            near,
            limit: usize::from(limit),
            lang: Some(self.lang.clone()),
        };
        let places = self.photon.search(&query, &options).await?;
        self.emit(&GeocodeDoc {
            schema: GEOCODE_SCHEMA,
            attribution: ATTRIBUTION,
            query,
            places,
        })
    }

    async fn reverse(&self, location: &str) -> Result<(), CliError> {
        let location = self.resolver().resolve(location, None).await?.location;
        let place = self.photon.reverse(location, Some(&self.lang)).await?;
        self.emit(&ReverseDoc {
            schema: REVERSE_SCHEMA,
            attribution: ATTRIBUTION,
            location,
            place,
        })
    }

    async fn route(&self, args: &RouteArgs) -> Result<(), CliError> {
        let trip = self.plan(&args.trip, args.alternatives).await?;
        let routes = trip
            .routes
            .into_iter()
            .map(|route| {
                Ok(RouteItem {
                    arrive_at: DateTime(time::arrive_at(&trip.depart_at, route.duration_s)?),
                    geometry: args
                        .geometry
                        .map(|format| geometry(&route.geometry, format))
                        .transpose()?,
                    route,
                })
            })
            .collect::<Result<_, Error>>()?;
        self.emit(&RouteDoc {
            schema: ROUTE_SCHEMA,
            attribution: ATTRIBUTION,
            mode: args.trip.mode,
            from: trip.from,
            to: trip.to,
            traffic: TRAFFIC,
            eta_basis: ETA_BASIS,
            depart_at: DateTime(trip.depart_at),
            routes,
        })
    }

    async fn eta(&self, args: &TripArgs) -> Result<(), CliError> {
        let trip = self.plan(args, 0).await?;
        let route = trip
            .routes
            .into_iter()
            .next()
            .ok_or_else(|| Error::Provider("the router returned no route".to_owned()))?;
        self.emit(&EtaDoc {
            schema: ETA_SCHEMA,
            attribution: ATTRIBUTION,
            mode: args.mode,
            from: trip.from,
            to: trip.to,
            traffic: TRAFFIC,
            eta_basis: ETA_BASIS,
            arrive_at: DateTime(time::arrive_at(&trip.depart_at, route.duration_s)?),
            depart_at: DateTime(trip.depart_at),
            distance_m: route.distance_m,
            duration_s: route.duration_s,
        })
    }

    /// Resolves `to` near `from`, so that "Central Station" means the one in the same city.
    async fn plan(&self, args: &TripArgs, alternatives: u8) -> Result<Trip, CliError> {
        let resolver = self.resolver();
        let from = resolver.resolve(&args.from, None).await?;
        let to = resolver.resolve(&args.to, Some(from.location)).await?;
        let routes = self
            .valhalla
            .route(&RouteRequest {
                from: from.location,
                to: to.location,
                mode: args.mode,
                alternatives,
                lang: Some(self.lang.clone()),
            })
            .await?;
        Ok(Trip {
            from,
            to,
            depart_at: args.depart_at.clone().unwrap_or_else(Zoned::now),
            routes,
        })
    }

    async fn places(&self, action: PlacesAction) -> Result<(), CliError> {
        match action {
            PlacesAction::Add { name, location } => {
                let location = self.resolver().resolve(&location, None).await?.location;
                places::add(&self.config_path, &name, location)?;
                let config = Config::load(&self.config_path)?;
                self.emit(&places_doc(&config))
            }
            // `execute` removed the place before it loaded the config.
            PlacesAction::Rm { .. } | PlacesAction::List => self.emit(&places_doc(&self.config)),
        }
    }
}

fn places_doc(config: &Config) -> PlacesDoc {
    PlacesDoc {
        schema: PLACES_SCHEMA,
        attribution: ATTRIBUTION,
        places: config
            .places
            .iter()
            .map(|(name, location)| SavedPlace {
                name: name.clone(),
                location: *location,
            })
            .collect(),
    }
}

fn geometry(points: &[LatLon], format: GeometryFormat) -> Result<Geometry, Error> {
    Ok(match format {
        GeometryFormat::Polyline6 => Geometry::Polyline6(encode_polyline6(points)?),
        GeometryFormat::GeoJson => Geometry::GeoJson(LineString {
            kind: "LineString",
            coordinates: points
                .iter()
                .map(|point| [point.lon(), point.lat()])
                .collect(),
        }),
    })
}
