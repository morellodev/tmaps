//! Draws tmaps maps on an HTML canvas, from WebAssembly. The page fetches the tiles and drives
//! [`WebMap`]; this crate does no I/O.

mod cache;
mod fade;
mod labels;
mod paint;
mod raster;
mod tile;
mod web;
mod wire;

pub use web::{WebMap, prepare_tile};
