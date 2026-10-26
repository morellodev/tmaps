# tmaps plan

`tmaps` is a terminal map viewer with a route planner, and a CLI for AI agents. It uses OpenStreetMap data.

## Product

- `tmaps` with no arguments opens the TUI. Subcommands (`geocode`, `reverse`, `route`, `eta`, `places`) are the CLI for agents.
- Travel modes: car, bike, foot. Out of scope: public transit and live GPS guidance.
- The itinerary is static. It shows 0 to 2 alternative routes.
- No automatic location. The TUI opens at the last view, then `@home`, then a world view.
- MIT license. Public open-source project.

## Data providers

The config file can change each endpoint. Providers are concrete types for now. Add a trait when a second adapter exists (a fake for TUI tests or a commercial provider): a seam with one adapter is only hypothetical.

| Function | Default | Notes |
| --- | --- | --- |
| Vector tiles | OpenFreeMap | OpenMapTiles schema, z0–14. Overzoom to about z18. Read the dated tile URL from the TileJSON. |
| Routing | Valhalla (`valhalla1.openstreetmap.de`) | Client throttle: max 1 req/s. Shape is polyline precision 6. Alternates can be 0. |
| Geocoding | Photon (`photon.komoot.io`) | Search, autocomplete (300 ms debounce), reverse. `lang`: `default`, `de`, `en`, `fr` only. |

- Do not use Nominatim. Its policy does not allow autocomplete.
- Send a unique User-Agent: `tmaps/<version> (+<repo URL>)`.
- Show attribution in the TUI status bar and in an `attribution` field of each JSON output.
- The README tells heavy users to self-host.

## Architecture

- MSRV: Rust 1.96 (`std::assert_matches!`).
- Cargo workspace:
  - `tmaps-core`: providers, geo math, domain types, renderer.
  - `tmaps-tui`: ratatui + crossterm UI.
  - `tmaps`: the binary and the CLI (clap).
- A tokio worker pool fetches, decodes, and projects tiles. Channels send results to the UI loop. The UI never blocks.
- MVT decoding: hand-written `prost` structs for the small `vector_tile.proto` schema. This removes `protox` and `build.rs`.
- The renderer has no ratatui dependency, so snapshot tests can use it. It draws lines as braille dots. It fills polygons at cell resolution: a cell gets the area color when its center is inside the polygon (even-odd rule). A braille cell has one background color, so a triangulation with `earcutr` gives no better result. Labels are horizontal text: a place name centers on its point, and a road name needs a straight run of cells on one row of the road. Placement is greedy in class and rank order, and each text appears once. A screen holds about 100 labels, so the overlap check is a linear scan, not an `rstar` index. Geometry simplification waits until profiles show a need.
- Tile fetches: HTTP 204 and 404 give an empty tile. When the network fails, an expired cached copy is better than no tile. Some servers send gzipped tiles without `Content-Encoding`, so tmaps detects gzip by its magic bytes.
- Language and units come from the config file, then the system locale. With no usable locale (`C`, `POSIX`), the language is `en-US` and the units are metric.
- XDG paths on all platforms (`etcetera`):
  - config: `~/.config/tmaps/config.toml` (includes `[places]`)
  - state: `~/.local/state/tmaps/` (last view)
  - cache: `~/.cache/tmaps/` (500 MB LRU for tiles that obeys `Cache-Control`; no route cache). Autocomplete results stay in an in-memory LRU for the session only.

## TUI

- The cursor is a fixed crosshair at the center. The map moves under it.
- Keys:
  - arrows or `hjkl`: pan. `Shift`: large pan step. Mouse drag: pan.
  - `+`/`-` or mouse wheel: zoom.
  - `/`: search, biased to the viewport center.
  - `s`/`e`: set start/end. `x`: swap. `c`: clear.
  - `1`/`2`/`3`: car/bike/foot.
  - `Tab`: next alternative route.
  - `]`/`[`: next/previous step of the itinerary. The map centers on the step, at street zoom or closer.
  - `p`: toggle the panel.
  - `m`: save the cursor as a named place. `'`: jump to a place.
  - `?`: help. `q`: quit.
- `s` and `e` take the point under the crosshair. Its name comes from a reverse geocode. Until the name arrives, or when there is no name, the panel shows the coordinates. The `A` and `B` markers cover the crosshair.
- The route request starts when both points are set or when the mode changes. Each request has a number, so a late result of an older request is dropped. When new routes arrive, the view fits them.
- The selected route clears the map dots in its cells and labels keep away from it, so it stays readable.
- The itinerary panel is on the right. Below 100 columns, it becomes a bottom drawer, with the steps next to the summary. It shows when a point is set. It shows the mode, the points, a summary for each alternative, and the steps, with the distance first.
- `/` opens a search box at the top of the map. After a 300 ms pause in the typing, Photon gets the text with a bias to the map center. `↑`/`↓` select a result, `Enter` centers the map on it, and `Esc` closes the box.
- `m` asks for a name and saves the point at the crosshair in `[places]` of the config file. `'` lists the saved places, and `Enter` centers the map on one.
- An open box takes all keys, except `Ctrl-C`. It has round corners in the accent color, and the map under it is dimmed.
- The status line shows the keys for what has the keys now, such as `/ search  s start  e end  ' places  ? keys`. A narrow line drops whole parts, never a part of a word: the position first, then the hints except the last, then the long attribution. The short attribution names only the copyright holders.
- A braille spinner shows while tiles, a search, or routes load. The screen redraws 10 times per second only while work is pending.
- Lists mark the selected row with `▶` and reverse video, so the selection shows also without colors.
- Colors: truecolor with a 256-color fallback. Dark and light themes. `theme = "auto"` (the default) asks the terminal for its background color with `terminal-colorsaurus` before the event reader starts. A terminal that does not answer gets the dark theme after a 1 s timeout. Obey `NO_COLOR`. The route uses a strong accent color. Alternatives are dimmed.
- When the network fails, show cached tiles and an error in the status line. Errors are short texts that tell what failed and, when possible, what to do. They start with `✗` and use the error color. Route errors also show in the panel, and search errors in the search box. A route or tile error stays until the route or the tiles change. The answer to an action has priority: an error, such as a failed save, stays until the next key, and a confirmation, such as `✓ Saved @work`, closes after 3 s or at the next key.
- Colors: map lines have at least 3:1 contrast against the background, and muted text at least 4.5:1 against the status line. Tests check both.

## CLI

- Location input: `lat,lon`, free text, or `@place`. For ambiguous text, use the top result and show the resolved point. `--strict` fails with exit code 2 and lists the candidates when a match is more than 1 km from the top result.
- `route --from X --to Y [--mode car|bike|foot] [--alternatives N] [--depart-at T] [--geometry polyline6|geojson]`
- `eta --from X --to Y`: duration and distance only.
- `geocode <query> [--near <place>]`, `reverse <lat,lon>`, `places add|list|rm`.
- Output: JSON when stdout is not a TTY, text when it is a TTY. `--format json|text` overrides.
- JSON has versioned schemas (`"schema": "tmaps/route@1"`). Publish the JSON Schemas.
- JSON uses SI units every time. `--units` changes only the text output and the TUI. `--lang` sets the language of names and instructions in both formats, because the providers write that text.
- Fields for each step: `instruction`, `maneuver`, `street_names`, `distance_m`, `duration_s`, `elapsed_s`, `location`.
- Each point in JSON is an object: `{"lat": …, "lon": …}`. Only GeoJSON geometry uses `[lon, lat]`.
- Route JSON includes `"traffic": "none"` and `"eta_basis": "free-flow"`.
- `--depart-at` only calculates the arrival time in tmaps. It does not go to Valhalla, because time-dependent routes have no alternates.
- Geometry is not in the output by default.
- Errors go to stderr in the output format. Usage errors are JSON when stdout is not a terminal. Exit codes: 0 ok, 1 config or output error, 2 ambiguous or invalid input, 3 no route, 4 network or provider error.
- `--depart-at` accepts RFC 3339, `YYYY-MM-DDTHH:MM`, or `HH:MM` in the local time zone. The default is now.
- schemars generates the JSON Schemas from the output types into `schemas/`. A test fails when a stored schema is stale.

## Quality

- `insta` snapshot tests of the renderer with fixture tiles in the repo.
- `wiremock` tests for providers with recorded HTTP fixtures.
- `assert_cmd` CLI tests that validate output against the JSON Schemas.
- Contract tests against live endpoints in a nightly CI job only.

## Release

- `cargo-dist`: macOS (arm64, x86_64), Linux (x86_64, arm64), Windows (x86_64, best effort).
- GitHub releases and a Homebrew tap.
- `docs/agents.md` and `skills/tmaps/SKILL.md` ship in M1.

## Milestones

1. M0: workspace, config, providers (Photon, Valhalla), domain types.
2. M1: CLI (`geocode`, `reverse`, `route`, `eta`, `places`), JSON Schemas, agent docs.
3. M2: tile pipeline, braille renderer, pan and zoom.
4. M3: labels and themes.
5. M4: TUI search, point selection, routes, itinerary panel.
6. M5: release packaging.
7. v1.1: `tmaps mcp` (`rmcp`, stdio) and waypoints.

Backlog: `nearby` (Overpass), `isochrone`, `matrix`, raster mode (Kitty/iTerm2/Sixel), offline regions (PMTiles + local Valhalla).
