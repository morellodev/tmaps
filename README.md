# tmaps

`tmaps` shows OpenStreetMap maps in the terminal and plans routes by car, bike, or foot. It also has a CLI that gives JSON for scripts and AI agents.

The map uses braille characters, so it works in each terminal that shows Unicode. It does not need a graphics protocol.

## Install

Install from source. You need Rust 1.96 or later.

```sh
cargo install --locked --git https://github.com/morellodev/tmaps tmaps
```

## Map

Run `tmaps` without a subcommand. The map opens at the last view, then at the saved place `@home`, then at a world view. The crosshair at the center is the cursor.

Press `?` to see all keys. Set a start with `s` and an end with `e`, and tmaps shows the route.

## CLI

Each subcommand writes JSON when stdout is not a terminal, and text when it is a terminal. Use `--format json|text` to select the format.

```sh
tmaps geocode "Piazza del Duomo, Milano"
tmaps reverse 45.4642,9.19
tmaps route --from @home --to "Colosseo, Roma" --mode bike
tmaps eta --from 45.4642,9.19 --to @work
tmaps places add work 45.4781,9.2256
```

A location is `lat,lon`, a saved place `@name`, or free text. [`docs/agents.md`](docs/agents.md) gives the full contract: the JSON schemas, the exit codes, and the errors. [`skills/tmaps/SKILL.md`](skills/tmaps/SKILL.md) is a skill for AI agents.

## Configuration

The config file is `~/.config/tmaps/config.toml`. Use `--config` or `TMAPS_CONFIG` to use a different file. All keys are optional.

```toml
units = "metric"   # or "imperial"
lang = "it-IT"     # BCP 47 tag for place names and route instructions
theme = "auto"     # or "dark", "light"

[endpoints]          # your own servers, see "Data providers"
tilejson = "https://tiles.example.org/planet"
valhalla = "https://valhalla.example.org"
photon = "https://photon.example.org"

[places]
home = "45.4642,9.19"
```

Without `units` and `lang`, tmaps uses the system locale. The map uses no colors when `NO_COLOR` is set.

tmaps keeps the last view in `~/.local/state/tmaps/` and caches tiles in `~/.cache/tmaps/`.

## Data providers

tmaps uses free public services:

- [OpenFreeMap](https://openfreemap.org) for the map tiles
- [Valhalla](https://github.com/valhalla/valhalla) on the FOSSGIS server for the routes
- [Photon](https://photon.komoot.io) by Komoot for the search

These services have fair-use limits. tmaps sends at most one route request each second, and it waits for a pause in the typing before it searches. If you send many requests, host the services yourself and set `[endpoints]` in the config file.

Map data © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright).

## Development

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
```

## License

[MIT](LICENSE)
