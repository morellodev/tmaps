# tmaps-core

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/morellodev/tmaps/blob/main/LICENSE)

The map engine of [tmaps](https://github.com/morellodev/tmaps). It decodes vector tiles in the OpenMapTiles schema, sorts their features into map classes, projects coordinates to Web Mercator, and places labels.

tmaps-core does no I/O and no drawing. Your code fetches the tiles, and a renderer draws them. Thus the crate also builds for WebAssembly.

These crates are renderers for tmaps-core:

- [`tmaps-braille`](../tmaps-braille) draws the map as braille characters in a terminal.
- [`tmaps-canvas`](../tmaps-canvas) draws the map on an HTML canvas from WebAssembly.

## Install

tmaps-core is not on crates.io yet. Add it from the repository:

```sh
cargo add --git https://github.com/morellodev/tmaps tmaps-core
```

## Usage

```rust,no_run
use tmaps_core::style::Class;
use tmaps_core::tiles::DEFAULT_MAX_ZOOM;
use tmaps_core::view::View;
use tmaps_core::{LatLon, PreparedTile};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // An uncompressed tile, for example from OpenFreeMap.
    let bytes = std::fs::read("14-8610-5863.pbf")?;
    let tile = PreparedTile::decode(&bytes, "it")?;
    for class in Class::ALL {
        println!("{class:?}: {} shapes", tile.shapes(class).len());
    }

    // The tiles that a view of Milan needs, with their positions on an 800 × 600 screen.
    let view = View::new(LatLon::new(45.4642, 9.19)?, 14.5);
    for visible in view.visible_tiles(800.0, 600.0, DEFAULT_MAX_ZOOM) {
        println!("{:?} at {}, {}", visible.id, visible.left, visible.top);
    }
    Ok(())
}
```

To draw a full map, give the prepared tiles to a `Scene`. The `tmaps-braille` source shows a full renderer.

## Feature flags

- `test-util`: test fakes for the crates that use tmaps-core.

## Stability

tmaps-core is at version 0.x. A minor release can make changes that are not compatible with the API.

## License

[MIT](https://github.com/morellodev/tmaps/blob/main/LICENSE). Map data from OpenStreetMap is © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright). Each view of the map must show the text of the `ATTRIBUTION` constant.
