use std::collections::{HashMap, HashSet};

use tmaps_core::label::{Anchor, LabelCandidate, LabelClass};
use tmaps_core::scene::{Part, Rect, ScreenPoint, to_screen};
use tmaps_core::view::VisibleTile;
use unicode_segmentation::UnicodeSegmentation;

use crate::line_text::{self, Cluster, Path, Run};

/// Free space around each label box, in CSS pixels.
const PADDING: f64 = 3.0;

// The smallest distance between the points that a road label can take, in CSS pixels: the
// default `symbol-spacing` of MapLibre.
const SPACING: f64 = 250.0;

/// A line gets points at its middle, then at its quarters, its eighths, and so on, down to this
/// distance between the points, in tile widths.
const MIN_SPACING: f64 = 1.0 / 32.0;

/// How far a label that shows can go past the screen edge before it leaves, in CSS pixels. So a
/// label moves off the screen with the map, and no other label takes its place at the edge.
const EDGE: f64 = 100.0;

/// The distance in CSS pixels that a label can move and still keep its place. The same point in
/// a tile and in its child tile can differ by one tile unit.
const KEEP: f64 = 2.0;

/// A point on a line where its label can go, in tile fractions. The point is fixed on the map,
/// so the label does not slide along the line when the map moves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineAnchor {
    pub at: [f32; 2],
    /// The index of the line in the lines of the label.
    pub line: usize,
    /// The distance from the start of the line to `at`, in tile widths.
    pub along: f32,
    /// 0 for the middle of a line, 1 for its quarters, 2 for its eighths, and so on.
    pub level: u8,
    /// The distance to the nearest point of the same or a lower level, in tile widths.
    pub spacing: f32,
    /// The length of line around the point, in tile widths: the widest text that fits.
    pub room: f32,
}

/// The points where the label of `candidate` can go, if it labels lines. Points outside the tile
/// are left out: the tile next to it has the same line, and gives the points on its side.
#[must_use]
pub fn line_anchors(candidate: &LabelCandidate) -> Vec<LineAnchor> {
    let Anchor::Lines(lines) = &candidate.anchor else {
        return Vec::new();
    };
    let mut anchors = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let points: Vec<[f64; 2]> = line
            .points()
            .iter()
            .map(|&[x, y]| [f64::from(x), f64::from(y)])
            .collect();
        let length: f64 = points
            .windows(2)
            .map(|pair| distance(pair[0], pair[1]))
            .sum();
        for level in 0..u8::MAX {
            let spacing = length / f64::from(2_u32 << level);
            if level > 0 && spacing < MIN_SPACING || length == 0.0 {
                break;
            }
            for step in (1..2_u32 << level).step_by(2) {
                let along = f64::from(step) * spacing;
                let at = point_along(&points, along);
                if at.iter().all(|value| (0.0..1.0).contains(value)) {
                    #[expect(clippy::cast_possible_truncation, reason = "tile fractions")]
                    anchors.push(LineAnchor {
                        at: at.map(|value| value as f32),
                        line: index,
                        along: along as f32,
                        level,
                        spacing: spacing as f32,
                        room: (2.0 * along.min(length - along)) as f32,
                    });
                }
            }
        }
    }
    anchors
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

fn point_along(points: &[[f64; 2]], mut along: f64) -> [f64; 2] {
    for pair in points.windows(2) {
        let step = distance(pair[0], pair[1]);
        if step > 0.0 && along <= step {
            let t = along / step;
            let (a, b) = (pair[0], pair[1]);
            return [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
        }
        along -= step;
    }
    points.last().copied().unwrap_or_default()
}

/// A label that a visible tile can show.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub placement: VisibleTile,
    pub label: &'a LabelCandidate,
    /// The [`line_anchors`] of the label.
    pub anchors: &'a [LineAnchor],
}

/// A label to draw at `(x, y)`: its point, or its anchor on a line.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed<'a> {
    pub text: &'a str,
    pub class: LabelClass,
    pub x: f64,
    pub y: f64,
    /// The parts of the text, in the order that they draw.
    pub runs: Vec<Run>,
    /// The label is at its position in `shown`.
    pub kept: bool,
}

/// Places the candidates in the order that they come. A label that would cover another label is
/// left out. Each text appears once, so a road that crosses tiles gets one label.
///
/// `shown` gives the screen position of each label that the last frame showed. In each class,
/// these labels go first, and keep their position if they can. There, they can also go past the
/// screen edge, which a label at a new position cannot.
///
/// A label on a line follows the line. `measure` gives the width and height of a text, or `None`
/// if it cannot.
pub fn place<'a>(
    candidates: impl IntoIterator<Item = Candidate<'a>>,
    screen: Rect,
    shown: &HashMap<&str, ScreenPoint>,
    mut measure: impl FnMut(&str, LabelClass) -> Option<(f64, f64)>,
) -> Vec<Placed<'a>> {
    let mut candidates: Vec<_> = candidates.into_iter().collect();
    candidates.sort_by_key(|candidate| candidate.label.class);
    let past_edge = Rect {
        left: screen.left - EDGE,
        top: screen.top - EDGE,
        right: screen.right + EDGE,
        bottom: screen.bottom + EDGE,
    };
    let mut placed = Vec::new();
    let mut texts = HashSet::new();
    // Labels never overlap, so the screen size limits their count: a linear scan needs no
    // spatial index.
    let mut taken: Vec<Collider> = Vec::new();
    for class in candidates.chunk_by(|a, b| a.label.class == b.label.class) {
        // A text can have more than one candidate, so all of them try to keep the last position
        // before one of them takes a new position.
        for keep in [true, false] {
            for candidate in class {
                let label = candidate.label;
                let last = shown.get(label.text.as_str());
                if texts.contains(label.text.as_str()) || keep && last.is_none() {
                    continue;
                }
                let Some(size) = Size::measure(label, &mut measure) else {
                    continue;
                };
                let near = |spot: ScreenPoint| match last {
                    Some(&last) if keep => squared(spot, last) <= KEEP * KEEP,
                    _ => true,
                };
                let area = if keep { past_edge } else { screen };
                let fits = |layout: &Layout| {
                    inside(layout.collider.bounds, area)
                        && !taken.iter().any(|other| layout.collider.overlaps(other))
                };
                let mut paths = HashMap::new();
                let Some(layout) = spots(candidate, &size)
                    .filter(|&(spot, _)| near(spot))
                    .find_map(|(spot, anchor)| {
                        layout(candidate, &size, spot, anchor, &mut paths).filter(fits)
                    })
                else {
                    continue;
                };
                let (x, y) = layout.at;
                taken.push(layout.collider);
                texts.insert(label.text.as_str());
                placed.push(Placed {
                    text: &label.text,
                    class: label.class,
                    x,
                    y,
                    runs: layout.runs,
                    kept: keep,
                });
            }
        }
    }
    placed
}

/// The width and height of a label text. A label on a line also has the width of each cluster.
struct Size {
    width: f64,
    height: f64,
    clusters: Vec<Cluster>,
    right_to_left: bool,
}

impl Size {
    fn measure(
        label: &LabelCandidate,
        measure: &mut impl FnMut(&str, LabelClass) -> Option<(f64, f64)>,
    ) -> Option<Self> {
        let text = label.text.as_str();
        if let Anchor::Point(_) = label.anchor {
            let (width, height) = measure(text, label.class)?;
            return Some(Self {
                width,
                height,
                clusters: Vec::new(),
                right_to_left: false,
            });
        }
        let mut height = 0.0;
        let clusters = text
            .grapheme_indices(true)
            .map(|(start, cluster)| {
                let (width, cluster_height) = measure(cluster, label.class)?;
                height = cluster_height;
                Some(Cluster {
                    range: start..start + cluster.len(),
                    width,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            width: clusters.iter().map(|cluster| cluster.width).sum(),
            height,
            clusters,
            right_to_left: text.chars().any(line_text::is_right_to_left),
        })
    }
}

/// A label at a position on the screen: how it draws, and what it covers.
struct Layout {
    at: ScreenPoint,
    runs: Vec<Run>,
    collider: Collider,
}

/// The boxes that a label covers, with padding, and the box around them.
struct Collider {
    bounds: Rect,
    boxes: Vec<Rect>,
}

impl Collider {
    fn new(boxes: Vec<Rect>) -> Self {
        let bounds = boxes.iter().fold(
            Rect {
                left: f64::INFINITY,
                top: f64::INFINITY,
                right: f64::NEG_INFINITY,
                bottom: f64::NEG_INFINITY,
            },
            |all, rect| Rect {
                left: all.left.min(rect.left),
                top: all.top.min(rect.top),
                right: all.right.max(rect.right),
                bottom: all.bottom.max(rect.bottom),
            },
        );
        Self { bounds, boxes }
    }

    fn overlaps(&self, other: &Self) -> bool {
        overlaps(self.bounds, other.bounds)
            && self
                .boxes
                .iter()
                .any(|&rect| other.boxes.iter().any(|&each| overlaps(rect, each)))
    }
}

/// The positions to try for a label: its point, or the anchors of its lines that the zoom shows
/// and that have room for the text, middles first.
fn spots<'c>(
    candidate: &'c Candidate,
    size: &Size,
) -> impl Iterator<Item = (ScreenPoint, Option<&'c LineAnchor>)> {
    let placement = candidate.placement;
    let mut spots: Vec<_> = match candidate.label.anchor {
        Anchor::Point(point) => vec![(to_screen(&placement, point), None)],
        Anchor::Lines(_) => {
            let on_screen = |tile_widths: f32| f64::from(tile_widths) * placement.size;
            candidate
                .anchors
                .iter()
                .filter(|anchor| {
                    (anchor.level == 0 || on_screen(anchor.spacing) >= SPACING)
                        && on_screen(anchor.room) >= size.width
                })
                .map(|anchor| (to_screen(&placement, anchor.at), Some(anchor)))
                .collect()
        }
    };
    spots.sort_by_key(|(_, anchor)| anchor.map(|anchor| anchor.level));
    spots.into_iter()
}

/// The label of `candidate` at `spot`, or `None` if it does not fit on its line there. `paths`
/// keeps the lines of the candidate on the screen, by index, for its next spots.
fn layout(
    candidate: &Candidate,
    size: &Size,
    spot: ScreenPoint,
    anchor: Option<&LineAnchor>,
    paths: &mut HashMap<usize, Path>,
) -> Option<Layout> {
    let text = candidate.label.text.as_str();
    let Some(anchor) = anchor else {
        return Some(Layout {
            at: spot,
            runs: vec![Run::level(text)],
            collider: Collider::new(vec![around(spot, size.width, size.height)]),
        });
    };
    let Anchor::Lines(lines) = &candidate.label.anchor else {
        return None;
    };
    let placement = candidate.placement;
    let line = lines.get(anchor.line).map(Part::points)?;
    let path = paths
        .entry(anchor.line)
        .or_insert_with(|| Path::new(line.iter().map(|&point| to_screen(&placement, point))));
    let laid = line_text::lay_out(
        path,
        f64::from(anchor.along) * placement.size,
        &size.clusters,
        size.height,
        size.right_to_left,
    )?;
    let boxes = laid.boxes.into_iter().map(padded).collect();
    Some(Layout {
        at: spot,
        runs: laid.runs,
        collider: Collider::new(boxes),
    })
}

fn squared(a: ScreenPoint, b: ScreenPoint) -> f64 {
    (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)
}

fn around((x, y): ScreenPoint, width: f64, height: f64) -> Rect {
    padded(Rect {
        left: x - width / 2.0,
        top: y - height / 2.0,
        right: x + width / 2.0,
        bottom: y + height / 2.0,
    })
}

fn padded(rect: Rect) -> Rect {
    Rect {
        left: rect.left - PADDING,
        top: rect.top - PADDING,
        right: rect.right + PADDING,
        bottom: rect.bottom + PADDING,
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

    /// Places the labels of `tiles` on a 400 × 400 screen. Each character is 10 × 10 pixels.
    fn place_on(
        tiles: &[(&PreparedTile, VisibleTile)],
        shown: &[(&str, ScreenPoint)],
    ) -> Vec<(String, f64, f64, bool)> {
        place_with(tiles, shown, |label| {
            (label.text.to_owned(), label.x, label.y, label.kept)
        })
    }

    fn place_with<T>(
        tiles: &[(&PreparedTile, VisibleTile)],
        shown: &[(&str, ScreenPoint)],
        map: impl FnMut(Placed) -> T,
    ) -> Vec<T> {
        let anchors: Vec<Vec<Vec<LineAnchor>>> = tiles
            .iter()
            .map(|(tile, _)| tile.labels().iter().map(line_anchors).collect())
            .collect();
        let candidates = tiles
            .iter()
            .zip(&anchors)
            .flat_map(|((tile, placement), anchors)| {
                tile.labels()
                    .iter()
                    .zip(anchors)
                    .map(|(label, anchors)| Candidate {
                        placement: *placement,
                        label,
                        anchors,
                    })
            });
        #[expect(clippy::cast_precision_loss, reason = "short test texts")]
        let measure = |text: &str, _| Some((text.chars().count() as f64 * 10.0, 10.0));
        let shown = shown.iter().copied().collect();
        place(candidates, Rect::screen(400.0, 400.0), &shown, measure)
            .into_iter()
            .map(map)
            .collect()
    }

    fn labels(tiles: &[&PreparedTile]) -> Vec<(String, f64, f64)> {
        let tiles: Vec<_> = tiles.iter().map(|&tile| (tile, PLACEMENT)).collect();
        place_on(&tiles, &[])
            .into_iter()
            .map(|(text, x, y, _)| (text, x, y))
            .collect()
    }

    fn moved(dx: f64, dy: f64) -> VisibleTile {
        VisibleTile {
            left: PLACEMENT.left + dx,
            top: PLACEMENT.top + dy,
            ..PLACEMENT
        }
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
    fn new_label_that_crosses_the_screen_edge_is_left_out() {
        let tile = tile("place", vec![city("Pavia", 5, 200)]);
        assert_eq!(labels(&[&tile]), []);
    }

    #[test]
    fn shown_label_stays_until_it_is_far_past_the_screen_edge() {
        let tile = tile("place", vec![city("Pavia", 25, 200)]);
        let shown = [("Pavia", (25.0, 200.0))];
        assert_eq!(
            place_on(&[(&tile, PLACEMENT)], &shown),
            [("Pavia".to_owned(), 25.0, 200.0, true)]
        );
        assert_eq!(place_on(&[(&tile, moved(-100.0, 0.0))], &shown), []);
    }

    #[test]
    fn shown_label_keeps_its_place_against_a_new_label_of_its_class() {
        let tile = tile(
            "place",
            vec![city("Milano", 200, 100), city("Monza", 225, 100)],
        );
        assert_eq!(
            place_on(&[(&tile, PLACEMENT)], &[("Monza", (225.0, 100.0))]),
            [("Monza".to_owned(), 225.0, 100.0, true)]
        );
    }

    #[test]
    fn text_in_two_tiles_gets_one_label() {
        let first = tile("place", vec![city("Lodi", 100, 100)]);
        let second = tile("place", vec![city("Lodi", 300, 300)]);
        assert_eq!(labels(&[&first, &second]).len(), 1);
    }

    #[test]
    fn shown_text_keeps_the_place_of_its_later_copy() {
        let first = tile("place", vec![city("Lodi", 100, 100)]);
        let second = tile("place", vec![city("Lodi", 300, 300)]);
        assert_eq!(
            place_on(
                &[(&first, PLACEMENT), (&second, PLACEMENT)],
                &[("Lodi", (300.0, 300.0))]
            ),
            [("Lodi".to_owned(), 300.0, 300.0, true)]
        );
    }

    #[test]
    fn road_label_moves_with_the_map() {
        let tile = tile(
            "transportation_name",
            vec![road("Via Po", &[(-200, 50), (400, 50)])],
        );
        assert_eq!(labels(&[&tile]), [("Via Po".to_owned(), 100.0, 50.0)]);
        assert_eq!(
            place_on(&[(&tile, moved(37.5, 120.0))], &[]),
            [("Via Po".to_owned(), 137.5, 170.0, false)]
        );
    }

    #[test]
    fn shown_road_label_keeps_its_anchor_or_takes_a_new_one_on_the_screen() {
        let tile = tile(
            "transportation_name",
            vec![road("Via Po", &[(0, 200), (400, 200)])],
        );
        let big = VisibleTile {
            size: 1200.0,
            ..moved(-250.0, -400.0)
        };
        assert_eq!(
            place_on(&[(&tile, big)], &[]),
            [("Via Po".to_owned(), 350.0, 200.0, false)]
        );
        assert_eq!(
            place_on(&[(&tile, big)], &[("Via Po", (51.0, 200.0))]),
            [("Via Po".to_owned(), 50.0, 200.0, true)]
        );
        assert_eq!(
            place_on(&[(&tile, big)], &[("Via Po", (60.0, 200.0))]),
            [("Via Po".to_owned(), 350.0, 200.0, false)]
        );
    }

    #[test]
    fn road_label_over_a_sharp_corner_is_left_out() {
        let tile = tile(
            "transportation_name",
            vec![road("Via Po", &[(-200, 50), (100, 50), (100, 350)])],
        );
        assert_eq!(labels(&[&tile]), []);
    }

    #[test]
    fn road_label_turns_with_its_road() {
        let tile = tile(
            "transportation_name",
            vec![road("Via Po", &[(0, 400), (400, 0)])],
        );
        let runs = place_with(&[(&tile, PLACEMENT)], &[], |label| label.runs);
        assert_eq!(runs.len(), 1);
        let angles: Vec<_> = runs[0].iter().map(|run| run.angle).collect();
        assert_eq!(angles, [-std::f64::consts::FRAC_PI_4]);
    }

    #[test]
    fn diagonal_road_label_leaves_room_beside_its_text() {
        let place = tile("place", vec![city("Ab", 225, 225)]);
        let road = tile(
            "transportation_name",
            vec![road("Via Po", &[(0, 400), (400, 0)])],
        );
        assert_eq!(labels(&[&place, &road]).len(), 2);
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
    fn line_anchors_halve_the_line_until_they_are_close() {
        let tile = tile(
            "transportation_name",
            vec![road("Via Po", &[(0, 100), (100, 100)])],
        );
        let anchors = line_anchors(&tile.labels()[0]);
        let levels: Vec<_> = anchors
            .iter()
            .map(|anchor| (anchor.at, anchor.level))
            .collect();
        assert_eq!(
            levels,
            [
                ([0.125, 0.25], 0),
                ([0.0625, 0.25], 1),
                ([0.1875, 0.25], 1),
                ([0.03125, 0.25], 2),
                ([0.09375, 0.25], 2),
                ([0.15625, 0.25], 2),
                ([0.21875, 0.25], 2),
            ]
        );
        assert!(anchors.iter().all(|anchor| anchor.spacing >= 1.0 / 32.0));
        assert_eq!(anchors[0].room, 0.25);
        assert_eq!(anchors[1].room, 0.125);
    }

    #[test]
    fn line_anchors_outside_the_tile_are_left_out() {
        let tile = tile(
            "transportation_name",
            vec![road("Via Po", &[(-300, 100), (100, 100)])],
        );
        let anchors = line_anchors(&tile.labels()[0]);
        let xs: Vec<_> = anchors.iter().map(|anchor| anchor.at[0]).collect();
        assert_eq!(
            xs,
            [
                0.0, 0.125, 0.0625, 0.1875, 0.03125, 0.09375, 0.15625, 0.21875
            ]
        );
    }
}
