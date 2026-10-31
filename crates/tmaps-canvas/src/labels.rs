use std::collections::HashSet;

use tmaps_core::label::{Anchor, LabelCandidate, LabelClass};
use tmaps_core::scene::{Part, Rect, ScreenPoint, clip, to_screen};
use tmaps_core::view::VisibleTile;

/// Free space around each label box, in CSS pixels.
const PADDING: f64 = 3.0;

/// A label to draw, centered on `(x, y)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed<'a> {
    pub text: &'a str,
    pub class: LabelClass,
    pub x: f64,
    pub y: f64,
}

/// Places the candidates in the order that they come. A label that would cover another label or
/// cross the screen edge is left out. Each text appears once, so a road that crosses tiles gets
/// one label. `measure` gives the width and height of a text, or `None` if it cannot.
pub fn place<'a>(
    candidates: impl IntoIterator<Item = (VisibleTile, &'a LabelCandidate)>,
    screen: Rect,
    mut measure: impl FnMut(&str, LabelClass) -> Option<(f64, f64)>,
) -> Vec<Placed<'a>> {
    let mut placed = Vec::new();
    let mut texts = HashSet::new();
    // Labels never overlap, so the screen size limits their count: a linear scan needs no
    // spatial index.
    let mut taken: Vec<Rect> = Vec::new();
    for (placement, candidate) in candidates {
        if texts.contains(candidate.text.as_str()) {
            continue;
        }
        let Some((width, height)) = measure(&candidate.text, candidate.class) else {
            continue;
        };
        let spot = spots(&placement, candidate, screen, width).find(|&(x, y)| {
            let label = around((x, y), width, height);
            inside(label, screen) && !taken.iter().any(|&other| overlaps(label, other))
        });
        let Some((x, y)) = spot else {
            continue;
        };
        taken.push(around((x, y), width, height));
        texts.insert(candidate.text.as_str());
        placed.push(Placed {
            text: &candidate.text,
            class: candidate.class,
            x,
            y,
        });
    }
    placed
}

/// The centers to try for a label: its point, or the middle of each visible piece of its lines
/// that is long enough for the text, longest piece first. Text on canvas stays horizontal.
fn spots(
    placement: &VisibleTile,
    candidate: &LabelCandidate,
    screen: Rect,
    width: f64,
) -> impl Iterator<Item = ScreenPoint> {
    let spots = match &candidate.anchor {
        Anchor::Point(point) => vec![to_screen(placement, *point)],
        Anchor::Lines(lines) => {
            let mut pieces: Vec<(f64, ScreenPoint)> = lines
                .iter()
                .filter(|line| line.touches(placement, screen))
                .flat_map(|line| visible_pieces(placement, line, screen))
                .filter_map(|piece| middle(&piece))
                .filter(|&(length, _)| length >= width)
                .collect();
            pieces.sort_by(|a, b| b.0.total_cmp(&a.0));
            pieces.into_iter().map(|(_, middle)| middle).collect()
        }
    };
    spots.into_iter()
}

/// The parts of a line inside `screen`, as runs of connected points.
fn visible_pieces(placement: &VisibleTile, line: &Part, screen: Rect) -> Vec<Vec<ScreenPoint>> {
    let mut pieces: Vec<Vec<ScreenPoint>> = Vec::new();
    let mut current: Vec<ScreenPoint> = Vec::new();
    for segment in line.points().windows(2) {
        let (a, b) = (
            to_screen(placement, segment[0]),
            to_screen(placement, segment[1]),
        );
        let Some((start, end)) = clip(a, b, screen) else {
            pieces.extend((current.len() > 1).then(|| std::mem::take(&mut current)));
            current.clear();
            continue;
        };
        if current.last() != Some(&start) {
            pieces.extend((current.len() > 1).then(|| std::mem::take(&mut current)));
            current = vec![start];
        }
        current.push(end);
    }
    pieces.extend((current.len() > 1).then_some(current));
    pieces
}

/// The length of a polyline and the point halfway along it.
fn middle(points: &[ScreenPoint]) -> Option<(f64, ScreenPoint)> {
    let distance = |a: ScreenPoint, b: ScreenPoint| (b.0 - a.0).hypot(b.1 - a.1);
    let length: f64 = points
        .windows(2)
        .map(|pair| distance(pair[0], pair[1]))
        .sum();
    let mut left = length / 2.0;
    for pair in points.windows(2) {
        let step = distance(pair[0], pair[1]);
        if step > 0.0 && left <= step {
            let t = left / step;
            let (a, b) = (pair[0], pair[1]);
            return Some((length, (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1))));
        }
        left -= step;
    }
    None
}

fn around((x, y): ScreenPoint, width: f64, height: f64) -> Rect {
    Rect {
        left: x - width / 2.0 - PADDING,
        top: y - height / 2.0 - PADDING,
        right: x + width / 2.0 + PADDING,
        bottom: y + height / 2.0 + PADDING,
    }
}

fn inside(rect: Rect, screen: Rect) -> bool {
    rect.left >= screen.left
        && rect.top >= screen.top
        && rect.right <= screen.right
        && rect.bottom <= screen.bottom
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

#[cfg(test)]
mod tests {
    use tmaps_core::PreparedTile;
    use tmaps_core::mvt::{Geometry, Layer, Point, Tile, Value};
    use tmaps_core::tiles::TileId;

    use super::*;

    /// One tile unit is one CSS pixel, and the tile fills a 400 × 400 screen.
    const EXTENT: u32 = 400;
    const PLACEMENT: VisibleTile = VisibleTile {
        id: TileId { z: 0, x: 0, y: 0 },
        left: 0.0,
        top: 0.0,
        size: 400.0,
    };

    fn tile(name: &str, features: Vec<(Geometry, Vec<(&str, Value)>)>) -> PreparedTile {
        let layers = vec![Layer::with_features(name, EXTENT, features)];
        PreparedTile::new(&Tile { layers }, "it")
    }

    fn city(name: &str, x: i32, y: i32) -> (Geometry, Vec<(&'static str, Value)>) {
        (
            Geometry::Points(vec![Point { x, y }]),
            vec![
                ("class", Value::String("city".to_owned())),
                ("name", Value::String(name.to_owned())),
            ],
        )
    }

    fn road(name: &str, points: &[(i32, i32)]) -> (Geometry, Vec<(&'static str, Value)>) {
        (
            Geometry::Lines(vec![points.iter().map(|&(x, y)| Point { x, y }).collect()]),
            vec![
                ("class", Value::String("primary".to_owned())),
                ("name", Value::String(name.to_owned())),
            ],
        )
    }

    /// Each character is 10 × 10 pixels.
    fn labels(tiles: &[&PreparedTile]) -> Vec<(String, f64, f64)> {
        let candidates = tiles
            .iter()
            .flat_map(|tile| tile.labels().iter().map(|candidate| (PLACEMENT, candidate)));
        #[expect(clippy::cast_precision_loss, reason = "short test texts")]
        let measure = |text: &str, _| Some((text.chars().count() as f64 * 10.0, 10.0));
        place(candidates, Rect::screen(400.0, 400.0), measure)
            .into_iter()
            .map(|label| (label.text.to_owned(), label.x, label.y))
            .collect()
    }

    #[test]
    fn point_label_is_centered_on_its_point() {
        let tile = tile("place", vec![city("Roma", 200, 100)]);
        assert_eq!(labels(&[&tile]), [("Roma".to_owned(), 200.0, 100.0)]);
    }

    #[test]
    fn label_that_covers_an_earlier_label_is_left_out() {
        let tile = tile(
            "place",
            vec![city("Milano", 200, 100), city("Monza", 210, 105)],
        );
        assert_eq!(labels(&[&tile]), [("Milano".to_owned(), 200.0, 100.0)]);
    }

    #[test]
    fn label_that_crosses_the_screen_edge_is_left_out() {
        let tile = tile("place", vec![city("Pavia", 5, 200)]);
        assert_eq!(labels(&[&tile]), []);
    }

    #[test]
    fn text_in_two_tiles_gets_one_label() {
        let first = tile("place", vec![city("Lodi", 100, 100)]);
        let second = tile("place", vec![city("Lodi", 300, 300)]);
        assert_eq!(labels(&[&first, &second]).len(), 1);
    }

    #[test]
    fn road_label_goes_at_the_middle_of_the_visible_line() {
        let tile = tile(
            "transportation_name",
            vec![road("Via Po", &[(-200, 50), (100, 50), (100, 350)])],
        );
        assert_eq!(labels(&[&tile]), [("Via Po".to_owned(), 100.0, 150.0)]);
    }

    #[test]
    fn road_shorter_than_its_name_gets_no_label() {
        let tile = tile(
            "transportation_name",
            vec![road("Via Roma", &[(100, 100), (150, 100)])],
        );
        assert_eq!(labels(&[&tile]), []);
    }

    #[test]
    fn middle_is_halfway_along_the_line() {
        assert_eq!(
            middle(&[(0.0, 0.0), (30.0, 0.0), (30.0, 10.0)]),
            Some((40.0, (20.0, 0.0)))
        );
    }

    #[test]
    fn visible_pieces_split_where_the_line_leaves_the_screen() {
        let tile = tile(
            "transportation_name",
            vec![road(
                "Via Po",
                &[(50, 200), (200, 200), (200, 600), (350, 600)],
            )],
        );
        let Anchor::Lines(lines) = &tile.labels()[0].anchor else {
            panic!("a road has lines");
        };
        let pieces = visible_pieces(&PLACEMENT, &lines[0], Rect::screen(400.0, 400.0));
        assert_eq!(
            pieces,
            [vec![(50.0, 200.0), (200.0, 200.0), (200.0, 400.0)]]
        );
    }
}
