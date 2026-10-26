//! Draws vector tiles as braille dots and colored cells, without a terminal dependency.

mod canvas;
mod scene;
mod style;
mod view;

pub use self::canvas::{Canvas, Cell};
pub use self::scene::{PreparedTile, render};
pub use self::style::Class;
pub use self::view::{MAX_ZOOM, View, VisibleTile};

/// Braille dots in one terminal cell: 2 columns and 4 rows.
pub const DOTS_PER_CELL_X: u16 = 2;
pub const DOTS_PER_CELL_Y: u16 = 4;
