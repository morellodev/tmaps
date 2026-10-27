use super::canvas::Canvas;
use super::label::{self, Candidate};
use super::style::{Class, classify};
use super::view::{View, VisibleTile};
use super::{DOTS_PER_CELL_X, DOTS_PER_CELL_Y};
use crate::LatLon;
use crate::mvt::{self, Geometry};
use crate::tiles::TileId;

/// The features of a tile that the renderer draws, grouped by class. Preparing a tile takes
/// longer than drawing it, so callers prepare it once, off the UI thread.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PreparedTile {
    shapes: [Vec<Shape>; Class::ALL.len()],
    labels: Vec<Candidate>,
}

#[derive(Debug, Clone, PartialEq)]
struct Shape {
    min_zoom: f64,
    /// Rings of an area, or lines. Points are fractions of the tile width, so they do not
    /// depend on the tile extent.
    parts: Vec<Vec<[f32; 2]>>,
}

pub(super) type DotPoint = (f64, f64);

/// A rectangle in dots. Lines stop at its edges, and areas fill the cells whose centers are in it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Bounds {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl Bounds {
    pub(super) fn of(canvas: &Canvas) -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            right: canvas.width_dots(),
            bottom: canvas.height_dots(),
        }
    }

    fn of_tile(placement: &VisibleTile) -> Self {
        Self {
            left: placement.left,
            top: placement.top,
            right: placement.left + placement.size,
            bottom: placement.top + placement.size,
        }
    }

    fn intersection(self, other: Self) -> Self {
        Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        }
    }
}

struct Drawn<'a> {
    placement: VisibleTile,
    tile: &'a PreparedTile,
    bounds: Bounds,
    /// The labels of an ancestor would move when the tile arrives, so it shows none.
    is_ancestor: bool,
}

impl PreparedTile {
    /// `lang` is a BCP 47 tag, for example `it-IT`. Labels use the names in its language when
    /// the tile has them.
    #[must_use]
    pub fn new(tile: &mvt::Tile, lang: &str) -> Self {
        let lang = lang.split(['-', '_']).next().unwrap_or_default();
        let mut prepared = Self::default();
        for layer in &tile.layers {
            #[expect(
                clippy::cast_precision_loss,
                reason = "extents are small powers of two"
            )]
            let extent = layer.extent.max(1) as f32;
            for feature in &layer.features {
                prepared
                    .labels
                    .extend(label::candidate(layer, feature, lang));
                let Some(style) = classify(layer, feature) else {
                    continue;
                };
                let ((Geometry::Polygons(parts), true) | (Geometry::Lines(parts), false)) =
                    (&feature.geometry, style.class.is_area())
                else {
                    continue;
                };
                #[expect(clippy::cast_precision_loss, reason = "tile units are below 2^24")]
                let parts = parts
                    .iter()
                    .map(|part| {
                        part.iter()
                            .map(|point| [point.x as f32 / extent, point.y as f32 / extent])
                            .collect()
                    })
                    .collect();
                prepared.shapes[style.class.index()].push(Shape {
                    min_zoom: style.min_zoom,
                    parts,
                });
            }
        }
        prepared
    }
}

/// A route that the renderer draws over the tiles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteLine<'a> {
    pub points: &'a [LatLon],
    /// The selected route covers the others.
    pub selected: bool,
}

/// Draws the view on a canvas of `width` × `height` cells, with the routes over the map.
/// `tile` returns a prepared tile, or `None` for a tile that is not loaded yet. The nearest
/// loaded ancestor fills the area of that tile, else the area stays empty.
pub fn render<'a>(
    view: View,
    width: u16,
    height: u16,
    max_zoom: u8,
    routes: &[RouteLine<'_>],
    tile: impl Fn(TileId) -> Option<&'a PreparedTile>,
) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    let canvas_bounds = Bounds::of(&canvas);
    let visible: Vec<Drawn> = view
        .visible_tiles(canvas.width_dots(), canvas.height_dots(), max_zoom)
        .into_iter()
        .filter_map(|placement| {
            if let Some(tile) = tile(placement.id) {
                return Some(Drawn {
                    placement,
                    tile,
                    bounds: canvas_bounds,
                    is_ancestor: false,
                });
            }
            let (ancestor, tile) = loaded_ancestor(&placement, &tile)?;
            Some(Drawn {
                placement: ancestor,
                tile,
                bounds: Bounds::of_tile(&placement).intersection(canvas_bounds),
                is_ancestor: true,
            })
        })
        .collect();
    // Class by class over all tiles, so that a tile does not cover the lines of its neighbor.
    for class in Class::ALL {
        for drawn in &visible {
            for shape in &drawn.tile.shapes[class.index()] {
                if view.zoom() < shape.min_zoom {
                    continue;
                }
                let parts: Vec<Vec<DotPoint>> = shape
                    .parts
                    .iter()
                    .map(|part| {
                        part.iter()
                            .map(|&point| to_dots(&drawn.placement, point))
                            .collect()
                    })
                    .collect();
                if class.is_area() {
                    fill_rings(&mut canvas, &parts, class, drawn.bounds);
                } else {
                    for part in &parts {
                        draw_line(&mut canvas, part, class, drawn.bounds);
                    }
                }
            }
        }
    }
    let (width_dots, height_dots) = (canvas.width_dots(), canvas.height_dots());
    for selected in [false, true] {
        for route in routes.iter().filter(|route| route.selected == selected) {
            let class = if selected {
                Class::Route
            } else {
                Class::Alternative
            };
            let points = view.line_on_screen(route.points, width_dots, height_dots);
            draw_line(&mut canvas, &points, class, canvas_bounds);
        }
    }
    label::place(
        &mut canvas,
        view,
        visible
            .iter()
            .filter(|drawn| !drawn.is_ancestor)
            .map(|drawn| (&drawn.placement, drawn.tile.labels.as_slice())),
    );
    canvas
}

fn loaded_ancestor<'a>(
    placement: &VisibleTile,
    tile: impl Fn(TileId) -> Option<&'a PreparedTile>,
) -> Option<(VisibleTile, &'a PreparedTile)> {
    let id = placement.id;
    (0..id.z).rev().find_map(|z| {
        let ancestor = id.ancestor(z)?;
        let loaded = tile(ancestor)?;
        let levels = id.z - z;
        let ancestor_placement = VisibleTile {
            id: ancestor,
            left: placement.left - f64::from(id.x - (ancestor.x << levels)) * placement.size,
            top: placement.top - f64::from(id.y - (ancestor.y << levels)) * placement.size,
            size: placement.size * f64::from(1_u32 << levels),
        };
        Some((ancestor_placement, loaded))
    })
}

pub(super) fn to_dots(placement: &VisibleTile, [x, y]: [f32; 2]) -> DotPoint {
    (
        placement.left + f64::from(x) * placement.size,
        placement.top + f64::from(y) * placement.size,
    )
}

/// Fills each cell whose center is inside the rings and the bounds, by the even-odd rule.
fn fill_rings(canvas: &mut Canvas, rings: &[Vec<DotPoint>], class: Class, bounds: Bounds) {
    let cell_w = f64::from(DOTS_PER_CELL_X);
    let cell_h = f64::from(DOTS_PER_CELL_Y);
    let (min_y, max_y) = rings.iter().flatten().fold(
        (f64::INFINITY, f64::NEG_INFINITY),
        |(low, high), &(_, y)| (low.min(y), high.max(y)),
    );
    let first_row = (min_y.max(bounds.top) / cell_h - 0.5).ceil();
    let last_row = (max_y / cell_h - 0.5)
        .floor()
        .min((bounds.bottom / cell_h - 0.5).ceil() - 1.0);
    let mut crossings = Vec::new();
    let mut row = first_row;
    while row <= last_row {
        let y = (row + 0.5) * cell_h;
        crossings.clear();
        for ring in rings {
            for edge in ring.windows(2) {
                let ((x0, y0), (x1, y1)) = (edge[0], edge[1]);
                if (y0 <= y) != (y1 <= y) {
                    crossings.push(x0 + (y - y0) / (y1 - y0) * (x1 - x0));
                }
            }
        }
        crossings.sort_by(f64::total_cmp);
        for span in crossings.as_chunks::<2>().0 {
            let first_column = (span[0].max(bounds.left) / cell_w - 0.5).ceil();
            let last_column = (span[1].min(bounds.right) / cell_w - 0.5).ceil();
            let mut column = first_column;
            while column < last_column {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "clamped to the canvas"
                )]
                canvas.fill(column as u16, row as u16, class);
                column += 1.0;
            }
        }
        row += 1.0;
    }
}

fn draw_line(canvas: &mut Canvas, points: &[DotPoint], class: Class, bounds: Bounds) {
    for segment in points.windows(2) {
        let Some(((x0, y0), (x1, y1))) = clip(segment[0], segment[1], bounds) else {
            continue;
        };
        let steps = (x1 - x0).abs().max((y1 - y0).abs()).ceil().max(1.0);
        let mut step = 0.0;
        while step <= steps {
            let t = step / steps;
            #[expect(clippy::cast_possible_truncation, reason = "clipped to the canvas")]
            canvas.set_dot(
                (x0 + t * (x1 - x0)).floor() as i64,
                (y0 + t * (y1 - y0)).floor() as i64,
                class,
            );
            step += 1.0;
        }
    }
}

/// Liang–Barsky clipping of a segment to the bounds. Without it, a long segment of an
/// overzoomed tile would step through millions of dots outside the screen.
pub(super) fn clip(a: DotPoint, b: DotPoint, bounds: Bounds) -> Option<(DotPoint, DotPoint)> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [
        (-dx, a.0 - bounds.left),
        (dx, bounds.right - a.0),
        (-dy, a.1 - bounds.top),
        (dy, bounds.bottom - a.1),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
        }
    }
    (t0 <= t1).then_some((
        (a.0 + t0 * dx, a.1 + t0 * dy),
        (a.0 + t1 * dx, a.1 + t1 * dy),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(canvas: &Canvas) -> Vec<String> {
        canvas
            .rows()
            .map(|row| {
                row.iter()
                    .map(|cell| match cell.bg {
                        Some(_) => '#',
                        None => cell.symbol(),
                    })
                    .collect()
            })
            .collect()
    }

    fn square(start: f64, end: f64) -> Bounds {
        Bounds {
            left: start,
            top: start,
            right: end,
            bottom: end,
        }
    }

    #[test]
    fn fill_rings_leaves_the_hole_empty() {
        let mut canvas = Canvas::new(5, 3);
        let outer = vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 12.0),
            (0.0, 12.0),
            (0.0, 0.0),
        ];
        let hole = vec![(4.0, 4.0), (6.0, 4.0), (6.0, 8.0), (4.0, 8.0), (4.0, 4.0)];
        let bounds = Bounds::of(&canvas);
        fill_rings(&mut canvas, &[outer, hole], Class::Water, bounds);
        assert_eq!(text(&canvas), ["#####", "## ##", "#####"]);
    }

    #[test]
    fn fill_rings_stops_at_the_bounds() {
        let mut canvas = Canvas::new(4, 1);
        let ring = vec![(0.0, 0.0), (8.0, 0.0), (8.0, 4.0), (0.0, 4.0), (0.0, 0.0)];
        fill_rings(&mut canvas, &[ring], Class::Water, square(2.0, 6.0));
        assert_eq!(text(&canvas), [" ## "]);
    }

    #[test]
    fn missing_tile_gets_its_nearest_loaded_ancestor() {
        let loaded = PreparedTile::default();
        let child = VisibleTile {
            id: TileId { z: 3, x: 5, y: 6 },
            left: 10.0,
            top: 20.0,
            size: 100.0,
        };
        let (placement, _) = loaded_ancestor(&child, |id| (id.z <= 1).then_some(&loaded)).unwrap();
        assert_eq!(
            placement,
            VisibleTile {
                id: TileId { z: 1, x: 1, y: 1 },
                left: -90.0,
                top: -180.0,
                size: 400.0,
            }
        );
    }

    #[test]
    fn ancestor_fills_only_the_tiles_that_are_not_loaded() {
        let square = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]];
        let mut water = PreparedTile::default();
        water.shapes[Class::Water.index()].push(Shape {
            min_zoom: 0.0,
            parts: vec![square],
        });
        let empty = PreparedTile::default();
        let view = View::new(LatLon::new(0.0, 0.0).unwrap(), 1.0);
        let north_west = TileId { z: 1, x: 0, y: 0 };
        let canvas = render(view, 8, 4, 14, &[], |id| match id.z {
            0 => Some(&water),
            _ => (id == north_west).then_some(&empty),
        });
        assert_eq!(
            text(&canvas),
            ["    ####", "    ####", "########", "########"]
        );
    }

    #[test]
    fn draw_line_sets_dots_along_the_segment() {
        let mut canvas = Canvas::new(2, 1);
        let bounds = Bounds::of(&canvas);
        draw_line(
            &mut canvas,
            &[(0.0, 1.5), (3.9, 1.5)],
            Class::MinorRoad,
            bounds,
        );
        assert_eq!(text(&canvas), ["⠒⠒"]);
    }

    #[test]
    fn clip_drops_a_segment_outside_the_screen() {
        assert_eq!(clip((-5.0, -5.0), (-1.0, 3.0), square(0.0, 10.0)), None);
    }

    #[test]
    fn clip_shortens_a_segment_across_the_screen() {
        assert_eq!(
            clip((-10.0, 5.0), (20.0, 5.0), square(0.0, 10.0)),
            Some(((0.0, 5.0), (10.0, 5.0)))
        );
    }
}
