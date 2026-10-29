use std::collections::HashSet;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::canvas::Canvas;
use super::scene::{Bounds, DotPoint, Part, clip, to_dots};
use super::style::Class;
use super::view::{View, VisibleTile};
use super::{DOTS_PER_CELL_X, DOTS_PER_CELL_Y};
use crate::mvt::{Feature, Geometry, Layer, Value};

/// What a label names. The order is the placement order: a label covers no earlier label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LabelClass {
    Country,
    City,
    State,
    Town,
    Village,
    Suburb,
    Water,
    Road,
}

/// Text on the map, in cells. `column` is the first cell of the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub column: u16,
    pub row: u16,
    pub text: String,
    pub class: LabelClass,
}

/// The cell of the cursor at the center of a map of `width` × `height` cells. Labels keep it
/// free.
#[must_use]
pub fn cursor_cell(width: u16, height: u16) -> (u16, u16) {
    (width / 2, height / 2)
}

/// A label that the renderer places when the view shows it and other labels leave space.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Candidate {
    class: LabelClass,
    /// Lower ranks go first within a class.
    rank: i64,
    min_zoom: f64,
    max_zoom: f64,
    text: String,
    /// The cells of the text, measured once because grapheme segmentation is slow.
    width: i32,
    /// Two lines of a point label, for when one line does not fit.
    wrapped: Option<[(String, i32); 2]>,
    /// Points are fractions of the tile width, as in the shapes of a prepared tile.
    anchor: Anchor,
}

#[derive(Debug, Clone, PartialEq)]
enum Anchor {
    Point([f32; 2]),
    Lines(Vec<Part>),
}

/// The layers that [`candidate`] can read a label from.
pub(super) const LAYERS: [&str; 3] = ["place", "water_name", "transportation_name"];

/// Reads a label from a feature of the `place`, `water_name`, or `transportation_name` layer
/// of the `OpenMapTiles` schema. `lang` is the language subtag of the names, for example `it`.
pub(super) fn candidate(layer: &Layer, feature: &Feature, lang: &str) -> Option<Candidate> {
    let text = |key: &str| layer.property(feature, key).and_then(Value::as_str);
    let rank = layer
        .property(feature, "rank")
        .and_then(Value::as_i64)
        .unwrap_or(i64::MAX);
    let (class, rank, min_zoom, max_zoom) = match layer.name.as_str() {
        "place" => match text("class")? {
            "country" => (LabelClass::Country, rank, 0.0, 7.0),
            "state" | "province" => (LabelClass::State, rank, 0.0, 9.0),
            "city" => (LabelClass::City, rank, 0.0, 15.0),
            "town" => (LabelClass::Town, rank, 0.0, 16.0),
            "village" => (LabelClass::Village, rank, 11.0, f64::INFINITY),
            "hamlet" => (LabelClass::Village, rank, 13.0, f64::INFINITY),
            "suburb" => (LabelClass::Suburb, rank, 12.0, f64::INFINITY),
            "quarter" => (LabelClass::Suburb, rank, 14.0, f64::INFINITY),
            "neighbourhood" => (LabelClass::Suburb, rank, 15.0, f64::INFINITY),
            _ => return None,
        },
        "water_name" => {
            let rank = match text("class")? {
                "ocean" => 0,
                "sea" => 1,
                _ => 2,
            };
            (LabelClass::Water, rank, 0.0, f64::INFINITY)
        }
        "transportation_name" => {
            let (rank, min_zoom) = match text("class")? {
                "motorway" | "trunk" => (0, 0.0),
                "primary" => (1, 0.0),
                "secondary" => (2, 0.0),
                "tertiary" => (3, 15.0),
                "minor" => (4, 15.0),
                "service" | "track" | "path" => (5, 16.0),
                _ => return None,
            };
            (LabelClass::Road, rank, min_zoom, f64::INFINITY)
        }
        _ => return None,
    };
    let name = |key: &str| text(key).and_then(clean);
    let text = name(&format!("name:{lang}"))
        .or_else(|| name("name"))
        .or_else(|| (class == LabelClass::Road).then(|| name("ref")).flatten())?;
    let extent = layer.extent.max(1);
    let anchor = match &feature.geometry {
        Geometry::Points(points) => Anchor::Point(normalize(*points.first()?, extent)),
        Geometry::Lines(lines) => Anchor::Lines(
            lines
                .iter()
                .map(|line| Part::new(line.iter().map(|&point| normalize(point, extent)).collect()))
                .collect(),
        ),
        _ => return None,
    };
    Some(Candidate {
        class,
        rank,
        min_zoom,
        max_zoom,
        width: i32::try_from(display_width(&text)).ok()?,
        wrapped: matches!(anchor, Anchor::Point(_))
            .then(|| wrap(&text))
            .flatten(),
        text,
        anchor,
    })
}

/// The first line of a name, without control characters: tile data must not send escape
/// sequences to the terminal, or reorder the text around a label with bidi overrides.
fn clean(name: &str) -> Option<String> {
    let text: String = name
        .lines()
        .next()?
        .chars()
        .filter(|&c| !c.is_control() && !is_bidi_control(c))
        .collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The explicit formatting characters of UAX #9: <https://www.unicode.org/reports/tr9/#Explicit_Directional_Embeddings>.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{61C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// The cells that the terminal gives to the text. Measured per grapheme, as ratatui draws it:
/// for the whole string, `unicode-width` joins ligatures such as lam-alef into one cell.
fn display_width(text: &str) -> usize {
    text.graphemes(true)
        .map(|grapheme| {
            // ratatui gives a halfwidth (han)dakuten its own cell, unlike `unicode-width`.
            let sound_marks = grapheme
                .chars()
                .filter(|c| matches!(c, '\u{FF9E}' | '\u{FF9F}'));
            grapheme.width() + sound_marks.count()
        })
        .sum()
}

/// The text in two lines, split at the space that makes the longer line shortest.
fn wrap(text: &str) -> Option<[(String, i32); 2]> {
    let line = |text: &str| {
        let width = i32::try_from(display_width(text)).ok()?;
        (!text.is_empty()).then(|| (text.to_owned(), width))
    };
    text.match_indices(' ')
        .filter_map(|(index, _)| {
            Some([
                line(text[..index].trim_end())?,
                line(text[index..].trim_start())?,
            ])
        })
        .min_by_key(|[(_, first), (_, second)]| *first.max(second))
}

#[expect(clippy::cast_precision_loss, reason = "tile units are below 2^24")]
fn normalize(point: crate::mvt::Point, extent: u32) -> [f32; 2] {
    let extent = extent as f32;
    [point.x as f32 / extent, point.y as f32 / extent]
}

/// Adds the labels of the visible tiles to the canvas, in the order of [`LabelClass`]. A label
/// that would cover another label, the cursor, a route, or the canvas edge is left out. Each text
/// appears once, so a road that crosses tiles gets one label.
pub(super) fn place<'a>(
    canvas: &mut Canvas,
    view: View,
    visible: impl IntoIterator<Item = (&'a VisibleTile, &'a [Candidate])>,
) {
    let mut candidates: Vec<(&VisibleTile, &Candidate)> = visible
        .into_iter()
        .flat_map(|(placement, candidates)| {
            candidates
                .iter()
                .filter(|candidate| (candidate.min_zoom..candidate.max_zoom).contains(&view.zoom()))
                .map(move |candidate| (placement, candidate))
        })
        .collect();
    candidates.sort_by_key(|(_, candidate)| (candidate.class, candidate.rank));
    let (cursor_column, cursor_row) = cursor_cell(canvas.width(), canvas.height());
    // A screen holds about 100 labels, so a linear scan of `taken` is fast enough without a
    // spatial index.
    let mut taken = vec![Span {
        row: i32::from(cursor_row),
        start: i32::from(cursor_column),
        end: i32::from(cursor_column) + 1,
    }];
    let mut placed = HashSet::new();
    for (placement, candidate) in candidates {
        if placed.contains(candidate.text.as_str()) {
            continue;
        }
        let text = (candidate.text.as_str(), candidate.width);
        // A free row around a road name keeps the names of near roads from stacking into a block.
        let free_rows = i32::from(candidate.class == LabelClass::Road);
        let blocks = match &candidate.anchor {
            Anchor::Point(point) => {
                let point = to_dots(placement, *point);
                let rows = i32::from(canvas.height());
                let mut blocks = point_spots(point, &[text], rows);
                if let Some([first, second]) = &candidate.wrapped {
                    let lines = [(first.0.as_str(), first.1), (second.0.as_str(), second.1)];
                    blocks.extend(point_spots(point, &lines, rows));
                }
                blocks
            }
            Anchor::Lines(lines) => line_spots(canvas, placement, lines, text.1)
                .into_iter()
                .map(|spot| vec![(spot, text.0)])
                .collect(),
        };
        let fits = |spot: &Span| {
            let (Ok(row), Ok(start), Ok(end)) = (
                u16::try_from(spot.row),
                u16::try_from(spot.start),
                u16::try_from(spot.end),
            ) else {
                return false;
            };
            end <= canvas.width()
                && row < canvas.height()
                && !canvas.has_route(row, start..end)
                && !taken.iter().any(|other| spot.crowds(other, free_rows))
        };
        let Some(block) = blocks
            .into_iter()
            .find(|block| block.iter().all(|(spot, _)| fits(spot)))
        else {
            continue;
        };
        placed.insert(candidate.text.as_str());
        for (spot, line) in block {
            let (Ok(column), Ok(row)) = (u16::try_from(spot.start), u16::try_from(spot.row)) else {
                continue;
            };
            taken.push(spot);
            canvas.add_label(Label {
                column,
                row,
                text: line.to_owned(),
                class: candidate.class,
            });
        }
    }
}

/// Cells `start..end` of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    row: i32,
    start: i32,
    end: i32,
}

impl Span {
    /// Whether `other` is less than one free cell away, on this row or on `rows` rows around it.
    fn crowds(self, other: &Self, rows: i32) -> bool {
        (self.row - other.row).abs() <= rows
            && self.start - 1 < other.end
            && other.start < self.end + 1
    }

    fn width(self) -> i32 {
        self.end - self.start
    }

    fn columns(self) -> std::ops::Range<i32> {
        self.start..self.end
    }
}

/// Blocks of one or two lines centered on the point: on its row first, then above or below it,
/// so that the cursor does not hide the name of the place at the center.
fn point_spots<'t>(
    (x, y): DotPoint,
    lines: &[(&'t str, i32)],
    rows: i32,
) -> Vec<Vec<(Span, &'t str)>> {
    let (Some(column), Some(row)) = (
        cell_index(x, DOTS_PER_CELL_X),
        cell_index(y, DOTS_PER_CELL_Y),
    ) else {
        return Vec::new();
    };
    if !(0..rows).contains(&row) {
        return Vec::new();
    }
    let tops: &[i32] = if lines.len() == 1 {
        &[row, row - 1, row + 1]
    } else {
        &[row - 1, row, row - 2, row + 1]
    };
    tops.iter()
        .map(|&top| {
            (top..)
                .zip(lines)
                .map(|(row, &(text, width))| {
                    let start = column - width / 2;
                    let spot = Span {
                        row,
                        start,
                        end: start + width,
                    };
                    (spot, text)
                })
                .collect()
        })
        .collect()
}

/// Terminal text cannot follow a curve: it goes on a row where the line is about horizontal,
/// else beside a steep part. Spots near the middle of the line come first.
fn line_spots(canvas: &Canvas, placement: &VisibleTile, lines: &[Part], width: i32) -> Vec<Span> {
    let bounds = Bounds::of(canvas);
    let mut on_line = Vec::new();
    let mut beside = Vec::new();
    for line in lines.iter().filter(|line| line.touches(placement, bounds)) {
        let cells = line_cells(placement, line, bounds);
        let Some(&(middle_column, middle_row)) = cells.get(cells.len() / 2) else {
            continue;
        };
        // A cell is about twice as high as it is wide.
        let distance = |spot: Span| {
            (i32::midpoint(spot.start, spot.end) - middle_column).abs()
                + 2 * (spot.row - middle_row).abs()
        };
        let path: HashSet<(i32, i32)> = cells.iter().copied().collect();
        // Tunnels and the lines of other zoom levels have names but no dots.
        let drawn = |run: Span| {
            let dotted = run
                .columns()
                .filter(|&column| has_dots(canvas, column, run.row));
            2 * dotted.count() >= run.columns().len()
        };
        for run in horizontal_runs(&cells)
            .into_iter()
            .filter(|&run| drawn(run))
        {
            let start = run.start + (run.width() - width) / 2;
            let centered = Span {
                row: run.row,
                start,
                end: start + width,
            };
            if let Some(cells_on_row) = cells_under(&path, centered) {
                on_line.push((
                    std::cmp::Reverse(cells_on_row),
                    distance(centered),
                    centered,
                ));
                continue;
            }
            beside.extend(beside_run(canvas, &path, run, width).map(|spot| (distance(spot), spot)));
        }
    }
    on_line.sort_by_key(|&(cells_on_row, distance, _)| (cells_on_row, distance));
    beside.sort_by_key(|&(distance, _)| distance);
    on_line
        .into_iter()
        .map(|(_, _, spot)| spot)
        .chain(beside.into_iter().map(|(_, spot)| spot))
        .collect()
}

/// Room for the sidewalks of a road between the road and a text beside it.
const MAX_GAP: i32 = 6;

/// The text right and left of a run. It skips paths and its own line, but no other line, which
/// it could name. Two blank cells on the far side keep it nearer to its own line.
fn beside_run(
    canvas: &Canvas,
    path: &HashSet<(i32, i32)>,
    run: Span,
    width: i32,
) -> impl Iterator<Item = Span> {
    let blank = |column: i32| !has_dots(canvas, column, run.row);
    let skippable = |column: i32| {
        let cell = u16::try_from(column)
            .ok()
            .zip(u16::try_from(run.row).ok())
            .and_then(|(column, row)| canvas.cell(column, row));
        blank(column)
            || path.contains(&(column, run.row))
            || cell.and_then(|cell| cell.fg) == Some(Class::Path)
    };
    let side = |edge: i32, step: i32| {
        for offset in 0..=MAX_GAP {
            let gap = edge + step * offset;
            if !skippable(gap) {
                return None;
            }
            if !blank(gap) {
                continue;
            }
            let start = if step > 0 { gap + 1 } else { gap - width };
            let spot = Span {
                row: run.row,
                start,
                end: start + width,
            };
            let room = if step > 0 {
                spot.start - 1..=spot.end + 1
            } else {
                spot.start - 2..=spot.end
            };
            if room.into_iter().all(blank) {
                return Some(spot);
            }
        }
        None
    };
    [side(run.end, 1), side(run.start - 1, -1)]
        .into_iter()
        .flatten()
}

fn has_dots(canvas: &Canvas, column: i32, row: i32) -> bool {
    let (Ok(column), Ok(row)) = (u16::try_from(column), u16::try_from(row)) else {
        return false;
    };
    canvas.cell(column, row).is_some_and(|cell| cell.dots != 0)
}

fn line_cells(placement: &VisibleTile, line: &Part, bounds: Bounds) -> Vec<(i32, i32)> {
    let mut cells: Vec<(i32, i32)> = Vec::new();
    for segment in line.points.windows(2) {
        let Some((a, b)) = clip(
            to_dots(placement, segment[0]),
            to_dots(placement, segment[1]),
            bounds,
        ) else {
            continue;
        };
        let steps = (b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil().max(1.0);
        let mut step = 0.0;
        while step <= steps {
            let t = step / steps;
            let cell = cell_index(a.0 + t * (b.0 - a.0), DOTS_PER_CELL_X)
                .zip(cell_index(a.1 + t * (b.1 - a.1), DOTS_PER_CELL_Y));
            if let Some(cell) = cell
                && cells.last() != Some(&cell)
            {
                cells.push(cell);
            }
            step += 1.0;
        }
    }
    cells
}

/// How many cells of the spot are on the path, if each column of the spot has the path on its
/// row or a row next to it.
fn cells_under(path: &HashSet<(i32, i32)>, spot: Span) -> Option<usize> {
    let mut on_row = 0;
    for column in spot.columns() {
        if path.contains(&(column, spot.row)) {
            on_row += 1;
        } else if !path.contains(&(column, spot.row - 1)) && !path.contains(&(column, spot.row + 1))
        {
            return None;
        }
    }
    Some(on_row)
}

/// Splits a path of `(column, row)` cells into runs of neighbor cells on one row.
fn horizontal_runs(cells: &[(i32, i32)]) -> Vec<Span> {
    let mut runs = Vec::new();
    let mut run: Option<Span> = None;
    for &(column, row) in cells {
        run = match run {
            Some(mut current) if current.row == row && column == current.end => {
                current.end += 1;
                Some(current)
            }
            Some(mut current) if current.row == row && column == current.start - 1 => {
                current.start -= 1;
                Some(current)
            }
            previous => {
                runs.extend(previous);
                Some(Span {
                    row,
                    start: column,
                    end: column + 1,
                })
            }
        };
    }
    runs.extend(run);
    runs
}

fn cell_index(dots: f64, dots_per_cell: u16) -> Option<i32> {
    let index = (dots / f64::from(dots_per_cell)).floor();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "checked against the i32 range"
    )]
    (f64::from(i32::MIN)..=f64::from(i32::MAX))
        .contains(&index)
        .then_some(index as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LatLon;
    use crate::mvt::Point;
    use crate::tiles::TileId;

    /// The canvas has 20 × 10 cells, 40 × 40 dots, and the cursor at cell (10, 5). The tile
    /// covers it, and one tile unit is one dot.
    const EXTENT: u32 = 40;

    fn span(row: i32, start: i32, end: i32) -> Span {
        Span { row, start, end }
    }

    fn string(text: &str) -> Value {
        Value::String(text.to_owned())
    }

    fn at_cell(column: i32, row: i32) -> Geometry {
        Geometry::Points(vec![Point {
            x: column * 2 + 1,
            y: row * 4 + 2,
        }])
    }

    fn line(points: &[(i32, i32)]) -> Geometry {
        Geometry::Lines(vec![points.iter().map(|&(x, y)| Point { x, y }).collect()])
    }

    fn candidates(name: &str, features: Vec<(Geometry, Vec<(&str, Value)>)>) -> Vec<Candidate> {
        let layer = Layer::with_features(name, EXTENT, features);
        layer
            .features
            .iter()
            .filter_map(|feature| candidate(&layer, feature, "it"))
            .collect()
    }

    fn place_named(class: &str, text: &str, column: i32, row: i32) -> Vec<Candidate> {
        candidates(
            "place",
            vec![(
                at_cell(column, row),
                vec![("class", string(class)), ("name", string(text))],
            )],
        )
    }

    fn road(properties: Vec<(&str, Value)>, geometry: Geometry) -> Vec<Candidate> {
        candidates("transportation_name", vec![(geometry, properties)])
    }

    fn texts(candidates: &[Candidate]) -> Vec<&str> {
        candidates.iter().map(|c| c.text.as_str()).collect()
    }

    /// Draws the roads of the candidates, then places the labels.
    fn labels(tiles: &[&[Candidate]], zoom: f64) -> Vec<Label> {
        let tile = VisibleTile {
            id: TileId { z: 0, x: 0, y: 0 },
            left: 0.0,
            top: 0.0,
            size: f64::from(EXTENT),
        };
        let mut canvas = Canvas::new(20, 10);
        let bounds = Bounds::of(&canvas);
        for candidate in tiles.iter().copied().flatten() {
            if let Anchor::Lines(lines) = &candidate.anchor {
                for (column, row) in lines
                    .iter()
                    .flat_map(|line| line_cells(&tile, line, bounds))
                {
                    canvas.set_dot(i64::from(column) * 2, i64::from(row) * 4, Class::MinorRoad);
                }
            }
        }
        let view = View::new(LatLon::new(0.0, 0.0).unwrap(), zoom);
        place(
            &mut canvas,
            view,
            tiles.iter().map(|&tile_labels| (&tile, tile_labels)),
        );
        canvas.labels().to_vec()
    }

    fn label(column: u16, row: u16, text: &str, class: LabelClass) -> Label {
        Label {
            column,
            row,
            text: text.to_owned(),
            class,
        }
    }

    #[test]
    fn candidate_uses_the_name_in_the_language() {
        let found = road(
            vec![
                ("class", string("primary")),
                ("name", string("Mailänder Straße")),
                ("name:it", string("Via Milano")),
            ],
            line(&[(0, 0), (40, 0)]),
        );
        assert_eq!(texts(&found), ["Via Milano"]);
    }

    #[test]
    fn candidate_skips_a_blank_name_in_the_language() {
        let found = road(
            vec![
                ("class", string("primary")),
                ("name:it", string(" ")),
                ("name", string("Hauptstraße")),
            ],
            line(&[(0, 0), (40, 0)]),
        );
        assert_eq!(texts(&found), ["Hauptstraße"]);
    }

    #[test]
    fn road_without_a_name_gets_its_ref() {
        let found = road(
            vec![("class", string("motorway")), ("ref", string("A4"))],
            line(&[(0, 0), (40, 0)]),
        );
        assert_eq!(texts(&found), ["A4"]);
    }

    #[test]
    fn unknown_road_class_gets_no_label() {
        let found = road(
            vec![("class", string("ferry")), ("name", string("Traghetto"))],
            line(&[(0, 0), (40, 0)]),
        );
        assert_eq!(found, []);
    }

    #[test]
    fn villages_are_hidden_below_their_zoom() {
        let village = place_named("village", "Lodi", 3, 2);
        assert_eq!(labels(&[&village], 10.0), []);
    }

    #[test]
    fn label_moves_off_the_cursor() {
        let city = place_named("city", "Roma", 10, 5);
        assert_eq!(
            labels(&[&city], 5.0),
            [label(8, 4, "Roma", LabelClass::City)]
        );
    }

    #[test]
    fn label_that_crosses_the_canvas_edge_is_left_out() {
        let city = place_named("city", "Milano", 0, 2);
        assert_eq!(labels(&[&city], 5.0), []);
    }

    #[test]
    fn place_above_the_canvas_gets_no_label() {
        let city = place_named("city", "Pavia", 4, -1);
        assert_eq!(labels(&[&city], 5.0), []);
    }

    #[test]
    fn text_in_two_tiles_gets_one_label() {
        let first = place_named("town", "Monza", 4, 1);
        let second = place_named("town", "Monza", 4, 8);
        assert_eq!(labels(&[&first, &second], 5.0).len(), 1);
    }

    #[test]
    fn road_label_is_centered_on_a_westward_line() {
        let found = road(
            vec![("class", string("primary")), ("name", string("Via Roma"))],
            line(&[(39, 10), (0, 10)]),
        );
        assert_eq!(
            labels(&[&found], 5.0),
            [label(6, 2, "Via Roma", LabelClass::Road)]
        );
    }

    #[test]
    fn diagonal_road_gets_a_label_beside_the_line() {
        let found = road(
            vec![("class", string("primary")), ("name", string("Via Roma"))],
            line(&[(0, 0), (39, 39)]),
        );
        assert_eq!(
            labels(&[&found], 5.0),
            [label(1, 5, "Via Roma", LabelClass::Road)]
        );
    }

    #[test]
    fn gently_sloped_road_gets_a_label_on_the_line() {
        let found = road(
            vec![("class", string("primary")), ("name", string("Via Roma"))],
            line(&[(0, 6), (39, 18)]),
        );
        assert_eq!(
            labels(&[&found], 5.0),
            [label(10, 3, "Via Roma", LabelClass::Road)]
        );
    }

    #[test]
    fn road_labels_keep_a_free_row_between_them() {
        let found = candidates(
            "transportation_name",
            vec![
                (
                    line(&[(0, 9), (39, 9)]),
                    vec![("class", string("primary")), ("name", string("Via Roma"))],
                ),
                (
                    line(&[(0, 13), (39, 13)]),
                    vec![("class", string("primary")), ("name", string("Via Po"))],
                ),
            ],
        );
        assert_eq!(
            labels(&[&found], 5.0),
            [label(6, 2, "Via Roma", LabelClass::Road)]
        );
    }

    #[test]
    fn text_beside_a_road_skips_a_path_but_not_another_road() {
        let mut canvas = Canvas::new(20, 1);
        canvas.set_dot(20, 0, Class::MinorRoad);
        canvas.set_dot(24, 0, Class::Path);
        canvas.set_dot(16, 0, Class::MinorRoad);
        let path = HashSet::from([(10, 0)]);
        let spots: Vec<Span> = beside_run(&canvas, &path, span(0, 10, 11), 3).collect();
        assert_eq!(spots, [span(0, 14, 17)]);
    }

    #[test]
    fn text_beside_a_road_does_not_skip_a_crossing_road() {
        let mut canvas = Canvas::new(20, 1);
        canvas.set_dot(20, 0, Class::MinorRoad);
        canvas.set_dot(24, 0, Class::MinorRoad);
        let path = HashSet::from([(10, 0)]);
        let spots: Vec<Span> = beside_run(&canvas, &path, span(0, 10, 11), 3).collect();
        assert_eq!(spots, [span(0, 6, 9)]);
    }

    #[test]
    fn text_beside_a_road_keeps_two_blank_cells_on_the_far_side() {
        let mut canvas = Canvas::new(20, 1);
        canvas.set_dot(20, 0, Class::MinorRoad);
        canvas.set_dot(34, 0, Class::MinorRoad);
        canvas.set_dot(8, 0, Class::MinorRoad);
        let path = HashSet::from([(10, 0)]);
        let spots: Vec<Span> = beside_run(&canvas, &path, span(0, 10, 11), 3).collect();
        assert_eq!(spots, [span(0, 12, 15)]);
    }

    #[test]
    fn display_width_counts_each_lam_alef_grapheme() {
        assert_eq!(display_width("العلا"), 5);
    }

    #[test]
    fn clean_removes_bidi_overrides() {
        assert_eq!(clean("\u{202E}abc\u{2069}").as_deref(), Some("abc"));
    }

    #[test]
    fn clean_keeps_the_first_line() {
        assert_eq!(clean("Milano\nMailand").as_deref(), Some("Milano"));
    }

    #[test]
    fn clean_removes_escape_sequences() {
        assert_eq!(clean("\u{1b}[31mRed").as_deref(), Some("[31mRed"));
    }

    #[test]
    fn clean_rejects_blank_names() {
        assert_eq!(clean(" \t"), None);
    }

    #[test]
    fn span_needs_a_free_cell_beside_other_labels() {
        assert!(span(0, 5, 8).crowds(&span(0, 8, 10), 0));
        assert!(!span(0, 5, 8).crowds(&span(0, 9, 10), 0));
    }

    #[test]
    fn spans_on_other_rows_crowd_only_within_the_free_rows() {
        assert!(!span(0, 5, 8).crowds(&span(1, 5, 8), 0));
        assert!(span(0, 5, 8).crowds(&span(1, 5, 8), 1));
        assert!(!span(0, 5, 8).crowds(&span(2, 5, 8), 1));
    }

    #[test]
    fn horizontal_runs_split_at_row_changes() {
        let cells = [(0, 0), (1, 0), (2, 0), (2, 1), (3, 1)];
        assert_eq!(horizontal_runs(&cells), [span(0, 0, 3), span(1, 2, 4)]);
    }

    #[test]
    fn horizontal_runs_follow_lines_drawn_westward() {
        let cells = [(5, 2), (4, 2), (3, 2)];
        assert_eq!(horizontal_runs(&cells), [span(2, 3, 6)]);
    }

    #[test]
    fn point_spots_center_the_text_on_the_point() {
        assert_eq!(
            point_spots((21.0, 9.0), &[("Roma", 4)], 10)[0],
            [(span(2, 8, 12), "Roma")]
        );
    }

    #[test]
    fn wrap_splits_at_the_space_that_makes_the_longer_line_shortest() {
        assert_eq!(
            wrap("Piazza Guglielmo Oberdan"),
            Some([
                ("Piazza Guglielmo".to_owned(), 16),
                ("Oberdan".to_owned(), 7)
            ])
        );
    }

    #[test]
    fn wrap_needs_a_space() {
        assert_eq!(wrap("Milano"), None);
    }

    #[test]
    fn place_name_wider_than_the_map_goes_on_two_lines() {
        let town = place_named("town", "San Giuliano Milanese", 10, 2);
        assert_eq!(
            labels(&[&town], 5.0),
            [
                label(4, 1, "San Giuliano", LabelClass::Town),
                label(6, 2, "Milanese", LabelClass::Town),
            ]
        );
    }

    #[test]
    fn road_name_stays_on_one_line() {
        let found = road(
            vec![("class", string("primary")), ("name", string("Via Roma"))],
            line(&[(0, 10), (39, 10)]),
        );
        assert_eq!(found[0].wrapped, None);
    }
}
