# tmaps for AI agents

This document is the contract of the `tmaps` CLI for scripts and AI agents. For a short procedure that an agent can load, use the skill in [`skills/tmaps/SKILL.md`](../skills/tmaps/SKILL.md).

## Output

- stdout gives JSON when it is not a terminal, and text when it is a terminal. `--format json|text` overrides this.
- Each JSON document is one line and has a `schema` field, for example `"tmaps/route@1"`. A change that is not compatible increases the number after `@`.
- The JSON Schema of each document is in [`schemas/`](../schemas). The file name is the schema name without `tmaps/`, for example `schemas/route@1.json`.
- Each point is an object: `{"lat": 45.4642, "lon": 9.19}`. Only GeoJSON geometry uses `[lon, lat]` positions.
- JSON always uses SI units: meters (`*_m`) and seconds (`*_s`). `--units` changes only the text output.
- `--lang` (or `lang` in the config file) sets the language of route instructions. Photon translates place names only for `de`, `en`, and `fr`. An invalid tag is a usage error.
- Each document has an `attribution` field. Show it with the data.

| Command | Schema |
| --- | --- |
| `geocode` | `tmaps/geocode@1` |
| `reverse` | `tmaps/reverse@1` |
| `route` | `tmaps/route@1` |
| `eta` | `tmaps/eta@1` |
| `places add\|list\|rm` | `tmaps/places@1` |
| any error (stderr) | `tmaps/error@1` |

## Locations

A LOCATION argument is `lat,lon`, `@name` (a saved place), or free text.

- Free text goes to the geocoder. `tmaps` uses the first match and reports it in `from.label` and `to.label`.
- With `--strict`, text fails with exit code 2 and kind `ambiguous` when a match is more than 1 km from the first match. Matches closer together, such as a station and its metro stop, are one place. The error lists up to 5 matches in `error.candidates`.
- `route` and `eta` resolve `--to` near the resolved `--from`.
- Two numbers with a comma are always coordinates. Out-of-range values fail with exit code 2. They do not go to the geocoder.

## Routes and ETAs

- `route` returns `routes`: the best route first, then up to `--alternatives` (0–2) alternatives. The router can return fewer.
- Each step has `instruction`, `maneuver`, `street_names`, `distance_m`, `duration_s`, `elapsed_s` (time from departure to the start of the step), and `location`.
- `--geometry polyline6|geojson` adds `geometry` to each route. The default output has no geometry.
- `eta` returns only `duration_s`, `distance_m`, and `arrive_at` of the best route.
- Estimates use free-flow speeds: `"traffic": "none"` and `"eta_basis": "free-flow"`. `--depart-at` changes only `depart_at` and `arrive_at`. The default departure time is now.
- `--depart-at` accepts RFC 3339 (`2026-10-26T14:00:00Z`), a local date and time (`2026-10-26T14:00`), or a local time of day today (`14:00`).

## Errors and exit codes

Errors go to stderr, in the output format: `{"schema": "tmaps/error@1", "error": {"kind": …, "message": …}}` in JSON, one line in text. A usage error is JSON when stdout is not a terminal, also with `--format text`, because `tmaps` cannot read `--format` from arguments that do not parse.

| Exit | Kinds | Meaning |
| --- | --- | --- |
| 0 | | Success. |
| 1 | `config`, `output`, `map` | The config file is not valid, the home directory is unknown, stdout cannot be written, or the map failed. |
| 2 | `invalid_input`, `ambiguous`, `usage` | An argument is not valid, a location is ambiguous with `--strict`, or no place matches the text. |
| 3 | `no_route` | No route connects the points, a point is too far from a road, or the trip is longer than the server limit. |
| 4 | `network`, `provider` | The request failed, or the server returned an error. |

## Saved places

- `tmaps places add NAME LOCATION` saves a place in the `[places]` table of the config file. Comments and other keys stay unchanged.
- A name has letters, digits, `-`, and `_`. Use it as `@NAME`.
- `add` and `rm` print the full list after the change.

## Configuration

The config file is `~/.config/tmaps/config.toml`. `--config PATH` or the `TMAPS_CONFIG` environment variable selects another file.

```toml
units = "metric"   # text output only
lang = "en-US"     # place names and instructions
theme = "auto"     # map colors: auto, dark, or light

[endpoints]
photon = "https://photon.komoot.io"
valhalla = "https://valhalla1.openstreetmap.de"

[places]
home = "45.4642,9.19"
```

## Fair use

The default servers are free public services with low rate limits. Each `tmaps` process is a new client, so space the calls yourself. For frequent requests, run your own [Photon](https://github.com/komoot/photon) and [Valhalla](https://github.com/valhalla/valhalla) servers, and set them in `[endpoints]`.
