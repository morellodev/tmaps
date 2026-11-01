# tmaps-canvas

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/morellodev/tmaps/blob/main/LICENSE)

The web renderer of [tmaps](https://github.com/morellodev/tmaps). It draws the vector tiles of [`tmaps-core`](../tmaps-core) on an HTML canvas, from WebAssembly.

tmaps-canvas does no I/O. The page fetches the tiles and controls the map through the `WebMap` class:

1. `WebMap` gives the IDs of the tiles that the view needs.
2. The page fetches each tile, then prepares it with `prepare_tile`, for example in a Web Worker.
3. The page gives the prepared tile to `WebMap`, and `WebMap` draws it.

`WebMap` also pans and zooms the view, shows a route, and has a dark theme.

## Build

tmaps-canvas is not on crates.io. Build it with the `wasm32-unknown-unknown` target and `wasm-bindgen-cli`. The version of `wasm-bindgen-cli` must be the same as the version of `wasm-bindgen` in `Cargo.lock`.

[`examples/canvas`](../../examples/canvas) is a demo page that builds the crate and uses it. Start with that page.

## License

[MIT](https://github.com/morellodev/tmaps/blob/main/LICENSE). Map data from OpenStreetMap is © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright).
