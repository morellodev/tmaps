//! Draws tmaps maps on an HTML canvas, from WebAssembly. The page fetches the tiles and drives
//! [`WebMap`]; this crate does no I/O.

mod cache;
mod labels;
mod paint;
mod tile;
mod web;

pub use web::WebMap;
