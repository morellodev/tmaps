---
name: tmaps
description: Directions, travel times, and geocoding from OpenStreetMap with the `tmaps` CLI. Use when a task needs a route, an ETA or arrival time, the coordinates of a place or address, or the address at a coordinate.
---

# tmaps

`tmaps` answers map questions from the terminal. It writes JSON to stdout when stdout is not a terminal, so you can read its output directly.

## Pick the command

| Question | Command |
| --- | --- |
| How long does it take, and when do I arrive? | `tmaps eta --from A --to B [--mode car\|bike\|foot] [--depart-at T]` |
| Which way do I go? | `tmaps route --from A --to B [--mode …] [--alternatives 0-2]` |
| Where is this place or address? | `tmaps geocode "text" [--near LOCATION]` |
| What is at these coordinates? | `tmaps reverse LAT,LON` |
| Save a location for later | `tmaps places add NAME LOCATION`, then use `@NAME` |

Use `eta` when you only need time and distance: its output is small. Use `route` for the steps.
Run `tmaps <command> --help` for every flag. For the JSON fields, the error kinds, and the exit codes, read [references/cli-contract.md](references/cli-contract.md).

## Locations

A LOCATION is one of these:

- `"45.4642,9.19"`: latitude first, then longitude.
- `@home`: a saved place (`tmaps places list`).
- Free text, for example `"Milano Centrale"`: `tmaps` uses the best geocoder match.

The output gives each resolved location in `from`/`to` (`label` and `location`). Read `label` and confirm that it is the place that the user means. To prevent a wrong guess, add `--strict`: text that matches places more than 1 km apart then fails with exit code 2, and `error.candidates` lists the matches. Ask the user to choose one, or add a city to the text.

`route` resolves `--to` near `--from`, so "Central Station" means the station in the same city.

## Read the result

- Units are always SI: `distance_m` in meters, `duration_s` in seconds. Convert them for the user.
- `arrive_at` and `depart_at` are RFC 3339 in the local time zone. `--depart-at` accepts RFC 3339, `YYYY-MM-DDTHH:MM`, or `HH:MM` (today).
- Times are free-flow estimates (`"traffic": "none"`). Tell the user that real traffic can make the trip longer.
- Instructions are in the configured language. Add `--lang en-US` to get them in English. Place names change only for `de`, `en`, and `fr`.
- Show the `attribution` text when you show map data to the user.

## Errors

Errors go to stderr in the output format. In JSON, an error is `{"schema": "tmaps/error@1", "error": {"kind", "message"}}`. A usage error is JSON whenever stdout is not a terminal. The exit code tells you what to do next:

| Exit | Kind | Next action |
| --- | --- | --- |
| 1 | `config`, `output` | Show the message to the user. |
| 2 | `invalid_input`, `ambiguous`, `usage` | Correct the arguments, or ask the user. |
| 3 | `no_route` | No road connects the points, or a point is far from a road. Try another mode or a nearer point. |
| 4 | `network`, `provider` | Wait a few seconds, then try one more time. |

The public servers allow few requests. Call `tmaps` once for each question, and keep the results that you need again.
