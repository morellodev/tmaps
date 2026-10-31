//! The map engine of tmaps: geo types, vector tiles, projection, map classes, and labels. It does
//! no I/O and no rendering, so that every renderer can use it, also in WebAssembly. A renderer
//! builds a [`Scene`] and draws its shapes and labels on its own medium.

pub mod error;
pub mod geo;
pub mod label;
pub mod mvt;
pub mod scene;
pub mod style;
pub mod tiles;
pub mod view;

pub use error::{Error, Result};
pub use scene::{PreparedTile, Scene};

/// Credit line that every view of provider data must show.
pub const ATTRIBUTION: &str = "© OpenStreetMap contributors";
pub use geo::LatLon;
