use std::collections::HashSet;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::canvas::Canvas;
use super::scene::{Bounds, DotPoint, Part, clip, to_dots};
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
    /// Points are fractions of the tile width, as in the shapes of a prepared tile.
    anchor: Anchor,
}

#[derive(Debug, Clone, PartialEq)]
enum Anchor {
    Point([f32; 2]),
    Lines(Vec<Part>),
}

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
        let width = candidate.width;
        let spots = match &candidate.anchor {
            Anchor::Point(point) => point_spots(
                to_dots(placement, *point),
                width,
                i32::from(canvas.height()),
            ),
            Anchor::Lines(lines) => line_spots(canvas, placement, lines, width),
        };
        let free = spots.into_iter().find(|spot| {
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
                && !taken.iter().any(|other| spot.padded().overlaps(other))
        });
        let Some(spot) = free else {
            continue;
        };
        let (Ok(column), Ok(row)) = (u16::try_from(spot.start), u16::try_from(spot.row)) else {
            continue;
        };
        taken.push(spot);
        placed.insert(candidate.text.as_str());
        canvas.add_label(Label {
            column,
            row,
            text: candidate.text.clone(),
            class: candidate.class,
        });
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
    /// One free cell on each side keeps two labels apart.
    fn padded(self) -> Self {
        Self {
            start: self.start - 1,
            end: self.end + 1,
            ..self
        }
    }

    fn overlaps(self, other: &Self) -> bool {
        self.row == other.row && self.start < other.end && other.start < self.end
    }
}

/// The text centered on the cell of the point, then on the row above or below it. Without the
/// other rows, the cursor would hide the name of the place at the center. A point outside the
/// `rows` of the canvas gets no spot.
fn point_spots((x, y): DotPoint, width: i32, rows: i32) -> Vec<Span> {
    let (Some(column), Some(row)) = (
        cell_index(x, DOTS_PER_CELL_X),
        cell_index(y, DOTS_PER_CELL_Y),
    ) else {
        return Vec::new();
    };
    if !(0..rows).contains(&row) {
        return Vec::new();
    }
    let start = column - width / 2;
    [row, row - 1, row + 1]
        .into_iter()
        .map(|row| Span {
            row,
            start,
            end: start + width,
        })
        .collect()
}

/// Places for the text on the lines, longest first: the middle of each horizontal run of
/// cells that is wide enough. Terminal text cannot follow a curve.
fn line_spots(canvas: &Canvas, placement: &VisibleTile, lines: &[Part], width: i32) -> Vec<Span> {
    let bounds = Bounds::of(canvas);
    let mut runs = Vec::new();
    for line in lines.iter().filter(|line| line.touches(placement, bounds)) {
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
        runs.extend(horizontal_runs(&cells));
    }
    runs.retain(|run| run.end - run.start >= width);
    runs.sort_by_key(|run| std::cmp::Reverse(run.end - run.start));
    runs.into_iter()
        .map(|run| {
            let start = run.start + (run.end - run.start - width) / 2;
            Span {
                row: run.row,
                start,
                end: start + width,
            }
        })
        .collect()
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

    fn labels(tiles: &[&[Candidate]], zoom: f64) -> Vec<Label> {
        let tile = VisibleTile {
            id: TileId { z: 0, x: 0, y: 0 },
            left: 0.0,
            top: 0.0,
            size: f64::from(EXTENT),
        };
        let mut canvas = Canvas::new(20, 10);
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
    fn diagonal_road_gets_no_label() {
        let found = road(
            vec![("class", string("primary")), ("name", string("Via"))],
            line(&[(0, 0), (39, 39)]),
        );
        assert_eq!(labels(&[&found], 5.0), []);
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
    fn padded_spans_keep_a_free_cell_between_labels() {
        assert!(span(0, 5, 8).padded().overlaps(&span(0, 8, 10)));
    }

    #[test]
    fn spans_on_other_rows_do_not_overlap() {
        assert!(!span(0, 5, 8).padded().overlaps(&span(1, 5, 8)));
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
        assert_eq!(point_spots((21.0, 9.0), 4, 10)[0], span(2, 8, 12));
    }
}
