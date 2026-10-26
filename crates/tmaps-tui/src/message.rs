use tmaps_core::LatLon;
use tmaps_core::geocode::Place;
use tmaps_core::render::PreparedTile;
use tmaps_core::routing::Route;
use tmaps_core::tiles::TileId;

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
    /// A place did not go into the config file.
    SaveFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInfo {
    pub max_zoom: u8,
    pub attribution: String,
}
