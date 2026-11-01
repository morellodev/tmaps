use tmaps_core::LatLon;
use tmaps_core::scene::{Rect, Scene, ScreenPoint, clip as clip_segment};
use tmaps_core::style::Class;
use tmaps_core::tiles::TileId;
use tmaps_core::view::View;

use crate::grid::Grid;
use crate::label;
use crate::tile::BrailleTile;
use crate::{DOTS_PER_CELL_X, DOTS_PER_CELL_Y};

/// A route that the renderer draws over the tiles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteLine<'a> {
    pub points: &'a [LatLon],
    /// The selected route covers the others.
    pub selected: bool,
}

/// Draws the view on a grid of `columns` × `rows` cells, with the routes over the map. Labels
/// leave the cells in `keep_free` free, for example the cell of a cursor. `tile` returns a loaded
/// tile, or `None` for a tile that is not loaded yet. The nearest loaded ancestor fills the area
/// of that tile, else the area stays empty.
pub fn render<'a>(
    view: View,
    columns: u16,
    rows: u16,
    max_zoom: u8,
    routes: &[RouteLine<'_>],
    keep_free: &[(u16, u16)],
    tile: impl Fn(TileId) -> Option<&'a BrailleTile>,
) -> Grid {
    let mut grid = Grid::new(columns, rows);
    let scene = Scene::new(view, grid.width_dots(), grid.height_dots(), max_zoom, tile);
    let mut crossings = Vec::new();
    // Class by class over all tiles, so that a tile does not cover the lines of its neighbor.
    for class in Class::ALL {
        for (scene_tile, shape) in scene.shapes(class) {
            let parts = scene_tile.parts(shape).map(|part| {
                part.points()
                    .iter()
                    .map(|&point| scene_tile.to_screen(point))
            });
            if class.is_area() {
                fill_rings(&mut grid, parts, class, scene_tile.clip, &mut crossings);
            } else {
                for part in parts {
                    draw_line(&mut grid, part, class, scene_tile.clip, Stroke::Thin);
                }
            }
        }
    }
    for selected in [false, true] {
        for route in routes.iter().filter(|route| route.selected == selected) {
            let (class, stroke) = if selected {
                (Class::Route, Stroke::Wide)
            } else {
                (Class::Alternative, Stroke::Thin)
            };
            let points = scene.line_on_screen(route.points);
            draw_line(&mut grid, points, class, scene.screen(), stroke);
        }
    }
    let labels = scene
        .labels()
        .into_iter()
        .filter_map(|(scene_tile, index, candidate)| {
            Some((
                scene_tile.placement,
                candidate,
                scene_tile.tile.text(index)?,
            ))
        });
    label::place(&mut grid, labels, keep_free);
    grid
}

/// Fills each cell whose center is inside the rings and `clip`, by the even-odd rule.
/// `crossings` is scratch space that the caller reuses.
///
/// Each edge adds crossings only for the rows that it spans: testing every edge on every row is
/// slow for merged multipolygons, which can have thousands of rings.
///
/// A braille cell has one background color, so a cell is in or out as a whole. An area narrower
/// than one cell can miss every cell center, and then it does not show.
fn fill_rings(
    grid: &mut Grid,
    rings: impl IntoIterator<Item = impl IntoIterator<Item = ScreenPoint>>,
    class: Class,
    clip: Rect,
    crossings: &mut Vec<(u16, f64)>,
) {
    let cell_w = f64::from(DOTS_PER_CELL_X);
    let cell_h = f64::from(DOTS_PER_CELL_Y);
    let first_row = (clip.top / cell_h - 0.5).ceil().max(0.0);
    let end_row = (clip.bottom / cell_h - 0.5)
        .ceil()
        .min(f64::from(grid.height()));
    crossings.clear();
    for ring in rings {
        let mut points = ring.into_iter();
        let Some(mut start) = points.next() else {
            continue;
        };
        for end in points {
            let ((x0, y0), (x1, y1)) = (start, end);
            start = end;
            // A row crosses the edge when one end is at or above its center and the other below.
            let mut row = (y0.min(y1) / cell_h - 0.5).ceil().max(first_row);
            let end_of_edge = (y0.max(y1) / cell_h - 0.5).ceil().min(end_row);
            while row < end_of_edge {
                let y = (row + 0.5) * cell_h;
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "clamped to the grid"
                )]
                crossings.push((row as u16, x0 + (y - y0) / (y1 - y0) * (x1 - x0)));
                row += 1.0;
            }
        }
    }
    crossings.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    for row_crossings in crossings.chunk_by(|a, b| a.0 == b.0) {
        for span in row_crossings.as_chunks::<2>().0 {
            let ((row, start), (_, end)) = (span[0], span[1]);
            let first_column = (start.max(clip.left) / cell_w - 0.5).ceil();
            let last_column = (end.min(clip.right) / cell_w - 0.5).ceil();
            let mut column = first_column;
            while column < last_column {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "clamped to the grid"
                )]
                grid.fill(column as u16, row, class);
                column += 1.0;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stroke {
    Thin,
    /// Two dots wide, because a route as thin as the roads does not stand out from them.
    Wide,
}

fn draw_line(
    grid: &mut Grid,
    points: impl IntoIterator<Item = ScreenPoint>,
    class: Class,
    clip: Rect,
    stroke: Stroke,
) {
    let mut points = points.into_iter();
    let Some(mut start) = points.next() else {
        return;
    };
    for end in points {
        let segment = (start, end);
        start = end;
        let Some(((x0, y0), (x1, y1))) = clip_segment(segment.0, segment.1, clip) else {
            continue;
        };
        let flat = (x1 - x0).abs() >= (y1 - y0).abs();
        let steps = (x1 - x0).abs().max((y1 - y0).abs()).ceil().max(1.0);
        let mut step = 0.0;
        while step <= steps {
            let t = step / steps;
            #[expect(clippy::cast_possible_truncation, reason = "clipped to the grid")]
            let (x, y) = (
                (x0 + t * (x1 - x0)).floor() as i64,
                (y0 + t * (y1 - y0)).floor() as i64,
            );
            grid.set_dot(x, y, class);
            // The extra dot stays in the same cell: a route dot hides the map dots of its cell.
            if stroke == Stroke::Wide {
                if flat {
                    grid.set_dot(x, y ^ 1, class);
                } else {
                    grid.set_dot(x ^ 1, y, class);
                }
            }
            step += 1.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use tmaps_core::PreparedTile;
    use tmaps_core::mvt::{Geometry, Layer, Point, Tile};

    use super::*;

    fn text(grid: &Grid) -> Vec<String> {
        grid.rows()
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

    fn square(start: f64, end: f64) -> Rect {
        Rect {
            left: start,
            top: start,
            right: end,
            bottom: end,
        }
    }

    fn screen(grid: &Grid) -> Rect {
        Rect::screen(grid.width_dots(), grid.height_dots())
    }

    #[test]
    fn fill_rings_leaves_the_hole_empty() {
        let mut grid = Grid::new(5, 3);
        let outer = vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 12.0),
            (0.0, 12.0),
            (0.0, 0.0),
        ];
        let hole = vec![(4.0, 4.0), (6.0, 4.0), (6.0, 8.0), (4.0, 8.0), (4.0, 4.0)];
        let clip = screen(&grid);
        fill_rings(
            &mut grid,
            [outer, hole],
            Class::Water,
            clip,
            &mut Vec::new(),
        );
        assert_eq!(text(&grid), ["#####", "## ##", "#####"]);
    }

    #[test]
    fn fill_rings_stops_at_the_clip() {
        let mut grid = Grid::new(4, 1);
        let ring = vec![(0.0, 0.0), (8.0, 0.0), (8.0, 4.0), (0.0, 4.0), (0.0, 0.0)];
        fill_rings(
            &mut grid,
            [ring],
            Class::Water,
            square(2.0, 6.0),
            &mut Vec::new(),
        );
        assert_eq!(text(&grid), [" ## "]);
    }

    #[test]
    fn ancestor_fills_only_the_tiles_that_are_not_loaded() {
        let square = [(0, 0), (1, 0), (1, 1), (0, 1), (0, 0)].map(|(x, y)| Point { x, y });
        let layer = Layer::with_features(
            "water",
            1,
            vec![(Geometry::Polygons(vec![square.to_vec()]), Vec::new())],
        );
        let water = BrailleTile::prepare(PreparedTile::new(
            &Tile {
                layers: vec![layer],
            },
            "en",
        ));
        let empty = BrailleTile::default();
        let view = View::new(LatLon::new(0.0, 0.0).unwrap(), 1.0);
        let north_west = TileId { z: 1, x: 0, y: 0 };
        let grid = render(view, 8, 4, 14, &[], &[], |id| match id.z {
            0 => Some(&water),
            _ => (id == north_west).then_some(&empty),
        });
        assert_eq!(
            text(&grid),
            ["    ####", "    ####", "########", "########"]
        );
    }

    #[test]
    fn draw_line_sets_dots_along_the_segment() {
        let mut grid = Grid::new(2, 1);
        let clip = screen(&grid);
        draw_line(
            &mut grid,
            [(0.0, 1.5), (3.9, 1.5)],
            Class::MinorRoad,
            clip,
            Stroke::Thin,
        );
        assert_eq!(text(&grid), ["⠒⠒"]);
    }

    #[test]
    fn wide_flat_line_stays_inside_its_cells() {
        let mut grid = Grid::new(2, 2);
        let clip = screen(&grid);
        draw_line(
            &mut grid,
            [(0.0, 3.5), (3.9, 3.5)],
            Class::Route,
            clip,
            Stroke::Wide,
        );
        assert_eq!(text(&grid), ["⣤⣤", "  "]);
    }

    #[test]
    fn wide_steep_line_stays_inside_its_cell() {
        let mut grid = Grid::new(2, 1);
        let clip = screen(&grid);
        draw_line(
            &mut grid,
            [(1.5, 0.0), (1.5, 3.9)],
            Class::Route,
            clip,
            Stroke::Wide,
        );
        assert_eq!(text(&grid), ["⣿ "]);
    }
}
