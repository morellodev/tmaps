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
  - `m`: save the cursor as a named place. `@` or `'`: jump to a place. `Ctrl-D` in the place list: remove a place. `Ctrl-S` in the search results: save a result.
  - `?`: help. `q`: quit.
- `s` and `e` take the point under the crosshair. Its name comes from a reverse geocode. Until the name arrives, or when there is no name, the panel shows the coordinates. The `A` and `B` markers cover the crosshair.
- The route request starts when both points are set or when the mode changes. Each request has a number, so a late result of an older request is dropped. When new routes arrive, the view fits them.
- The selected route clears the map dots in its cells and labels keep away from it, so it stays readable.
- The itinerary panel is on the right, with a quarter of the terminal width, from 40 to 56 columns. Below 100 columns, it becomes a bottom drawer, with the steps next to the summary. Below 52 columns, the steps go under the summary, and they keep at least 3 rows. It shows when a point is set. It shows the mode, the points, a summary for each alternative, and the steps, with the distance and a turn arrow first. The bottom border shows the step count, or the selected step as `step 3/16`. A long step list has a scrollbar on the right border.
- The drawer height is half of the rows above the status line, from 6 to 12 rows. A step that is higher than the step list is cut with `…`. When the map would get fewer than 8 rows, the drawer hides. Then the status line does not show the `p` hint, because `p` cannot show the drawer. When the panel is hidden, the status line shows the selected route, such as `route 1/3: 15 min · 4.2 km · step 2/16`.
- A terminal smaller than 30×8 shows only a request for more room.
- `?` opens the help over the map. The status line stays visible. The keys are in groups (Map, Places, Route, App), in one column. When one column does not fit, they go in two columns. When two columns also do not fit, the keys are in one list without headings, with wrapped descriptions. When the list does not fit, `↑`/`↓` (or `j`/`k`) scroll it, and the box shows a scrollbar and `↑↓ scroll`. The help does not take the other keys.
- `/` opens a search box at the top of the map, with half of the map width, from 60 to 100 columns. After a 300 ms pause in the typing, Photon gets the text with a bias to the map center. Each result shows its kind and its distance from the map center at the right, and the bottom border shows the result count. `↑`/`↓` select a result, `Enter` centers the map on it, and `Esc` closes the box. When the text is longer than the box, the box shows its end.
- A search text that starts with `@` gets no Photon request. It lists the saved places whose name starts with the rest of the text, without the spaces after `@`, then the ones that contain it, without case. A bare `@` lists all saved places.
- `m` asks for a name and saves the point at the crosshair in `[places]` of the config file. In a search that does not start with `@`, `Ctrl-S` asks for a name for the selected result, and the map does not move. `Esc` in this name box goes back to the search and its results. The name box shows what it saves: the result label, or `The place at +`, with the coordinates at the right. When a place has the typed name, the box says that `Enter` replaces it. `@` and `'` open the search box with the text `@`, so it lists all saved places. `Enter` centers the map on a place, at neighborhood zoom.
- In a search for `@`, `Ctrl-D` asks to remove the selected place: `Remove @home? Enter: remove. Esc: keep.` Another key or a click cancels the question, then works as usual. After the removal, the box stays open and the list loses the place. A place that is not in the config file leaves the list without an error. When the list changes, the same place stays selected. The status line shows `Ctrl-S` and `Ctrl-D` only when a result is selected, and a search without results shows only `Esc close`.
- An open box, except the help, takes all keys, except `Ctrl-C`. It has round corners in the accent color, and the map under it is dimmed. A note that only helps, such as the allowed characters or `Searching…`, uses the muted color. A note that needs an answer or tells about an error uses the error color. A row that the user cannot select has no space for the `▶` marker, so it lines up with the typed text.
- Text boxes take the shell keys: `Ctrl-U` clears the text, and `Ctrl-W` or `Alt-Backspace` removes the last word. There is no cursor movement.
- Mouse:
  - Only on the map, a drag pans and the wheel zooms. A drag that starts on the panel, the help box, or the status line does not pan.
  - On the panel, a click on a step and the wheel select a step, like `]`/`[`. A click on the empty rows under the steps does nothing.
  - On a help that does not fit, the wheel scrolls the help. On a help that fits, and outside the help box, the wheel zooms the map.
  - While a box is open, the map does not move. In a box with a list, the wheel moves the selection, a click selects a row, and a click on the selected row acts like `Enter`.
- The status line shows the keys for what has the keys now, such as `/ search  s start  e end  @ places  ? keys`. A narrow line drops whole parts, never a part of a word: the position first, then the hints except the last, then the long attribution. When a route shows, the short attribution goes before the route. The short attribution names only the copyright holders. When the short attribution does not fit alone, the last hint gets the line.
- A braille spinner shows while tiles, a search, or routes load. The screen redraws 10 times per second only while work is pending.
- Lists mark the selected row with `▶` and reverse video, so the selection shows also without colors.
- Colors: truecolor with a 256-color fallback. Dark and light themes. `theme = "auto"` (the default) asks the terminal for its background color with `terminal-colorsaurus` before the event reader starts. A terminal that does not answer gets the dark theme after a 1 s timeout. Obey `NO_COLOR` and `TERM=dumb`. A non-empty `FORCE_COLOR` overrides both (force-color.org). The route uses a strong accent color. Alternatives are dimmed.
- When the network fails, show cached tiles and an error in the status line. Errors are short texts that tell what failed and, when possible, what to do. They start with `✗` and use the error color. Route errors also show in the panel, and search errors in the search box. A route error stays until the route changes. A tile error stays until the tile loads or leaves the screen, also during a retry. The answer to an action has priority: an error, such as a failed save, stays until the next key, and a confirmation, such as `✓ Saved @work`, closes after 3 s or at the next key.
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

- `cargo-dist`: macOS (arm64, x86_64) and Linux (x86_64, arm64). Windows is out of scope.
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
