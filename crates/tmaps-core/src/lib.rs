//! Geocoding, routing, and configuration for tmaps, shared by the TUI and the CLI.

pub mod config;
pub mod error;
pub mod file;
pub mod format;
pub mod geo;
pub mod geocode;
pub mod http;
pub mod mvt;
pub mod places;
pub mod providers;
pub mod render;
pub mod routing;
pub mod tiles;

pub use error::{Error, Result};

/// Credit line that every view of provider data must show.
pub const ATTRIBUTION: &str = "© OpenStreetMap contributors";
pub use geo::LatLon;
