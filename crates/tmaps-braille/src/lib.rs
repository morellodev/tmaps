//! Draws vector tiles as braille dots and colored cells, without a terminal dependency.

mod grid;
mod label;
mod render;
mod tile;

pub use self::grid::{Cell, Grid};
pub use self::label::Label;
pub use self::render::{RouteLine, render};
pub use self::tile::BrailleTile;

/// Braille dots in one terminal cell: 2 columns and 4 rows. One dot is one screen unit, because
/// dots are about square in common terminal fonts.
pub const DOTS_PER_CELL_X: u16 = 2;
pub const DOTS_PER_CELL_Y: u16 = 4;
