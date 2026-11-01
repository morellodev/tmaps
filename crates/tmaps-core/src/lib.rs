//! The map engine of tmaps: geo types, vector tiles, projection, map classes, and labels. It does
//! no I/O, so that every renderer can use it, also in WebAssembly.

pub mod error;
pub mod geo;
pub mod mvt;
pub mod render;
pub mod tiles;

pub use error::{Error, Result};

/// Credit line that every view of provider data must show.
pub const ATTRIBUTION: &str = "© OpenStreetMap contributors";
pub use geo::LatLon;
