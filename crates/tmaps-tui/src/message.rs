use tmaps_core::geocode::Place;
use tmaps_core::render::PreparedTile;
use tmaps_core::routing::{Mode, Route};
use tmaps_core::tiles::TileId;
use tmaps_core::{Error, LatLon};

/// A result of background work. The number in a result is the number of its request.
#[derive(Debug)]
pub enum Message {
    Source(Result<SourceInfo, String>),
    Tile(TileId, Result<Box<PreparedTile>, String>),
    /// The tile left the screen before its request started.
    Skipped(TileId),
    Routes(u64, Result<Vec<Route>, String>),
    /// The name of the place at a waypoint. `None` when no place is known or the request
    /// fails: the waypoint then keeps its coordinates.
    PlaceName(u64, Option<String>),
    SearchResults(u64, Result<Vec<Place>, String>),
    /// A place is in the config file.
    PlaceSaved(String, LatLon),
    /// A place is no longer in the config file.
    PlaceRemoved(String),
    /// A change to the saved places did not go into the config file.
    PlaceFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInfo {
    pub max_zoom: u8,
    pub attribution: String,
}

#[derive(Debug, Clone, Copy)]
pub enum Server {
    Map,
    Search,
    Routing,
}

/// A short error text for the status line. A network error only adds a long URL and the text
/// of the HTTP client, which do not help the user.
pub fn describe(error: &Error, server: Server) -> String {
    let server = match server {
        Server::Map => "map",
        Server::Search => "search",
        Server::Routing => "routing",
    };
    match error {
        Error::Network(_) => format!("Cannot reach the {server} server."),
        Error::Provider(detail) => format!("The {server} server sent an error: {detail}"),
        _ => error.to_string(),
    }
}

/// Another mode can have a road where this one has none.
pub fn no_route(mode: Mode) -> String {
    let others = match mode {
        Mode::Car => "2 or 3",
        Mode::Bike => "1 or 3",
        Mode::Foot => "1 or 2",
    };
    format!("No route by {mode}. Press {others} for another mode, or move a point.")
}
