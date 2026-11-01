# tmaps-braille

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/morellodev/tmaps/blob/main/LICENSE)

The terminal renderer of [tmaps](https://github.com/morellodev/tmaps). It draws the vector tiles of [`tmaps-core`](../tmaps-core) as braille dots, with map classes for each cell and labels that do not overlap.

Each terminal cell holds 2 × 4 braille dots, so the map has more detail than one character for each cell. The output is a `Grid` of cells and labels. tmaps-braille does not depend on a terminal library, so you can show the grid with ratatui, crossterm, or plain text. tmaps-braille does no I/O.

## Install

tmaps-braille is not on crates.io yet. Add it from the repository:

```sh
cargo add --git https://github.com/morellodev/tmaps tmaps-braille tmaps-core
```

## Usage

```rust,no_run
use tmaps_braille::{BrailleTile, render};
use tmaps_core::tiles::{DEFAULT_MAX_ZOOM, TileId};
use tmaps_core::view::View;
use tmaps_core::{LatLon, PreparedTile};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // An uncompressed tile, for example from OpenFreeMap.
    let bytes = std::fs::read("5-16-11.pbf")?;
    let id = TileId { z: 5, x: 16, y: 11 };
    let tile = BrailleTile::prepare(PreparedTile::decode(&bytes, "en")?);

    let view = View::new(LatLon::new(45.4642, 9.19)?, 5.0);
    let grid = render(view, 80, 24, DEFAULT_MAX_ZOOM, &[], &[], |requested| {
        (requested == id).then_some(&tile)
    });
    for row in grid.rows() {
        println!("{}", row.iter().map(|cell| cell.symbol()).collect::<String>());
    }
    Ok(())
}
```

Prepare a tile one time, away from the UI thread, because preparation takes longer than drawing. Each cell also has a foreground and a background map class. Use them to give the cell its colors.

## Stability

tmaps-braille is at version 0.x. A minor release can make changes that are not compatible with the API.

## License

[MIT](https://github.com/morellodev/tmaps/blob/main/LICENSE). Map data from OpenStreetMap is © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright).
