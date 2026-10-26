use std::f64::consts::PI;

use crate::LatLon;
use crate::tiles::TileId;

/// Deepest zoom of the view. Above the zoom of the tile set, the renderer scales the deepest tiles.
pub const MAX_ZOOM: f64 = 18.0;
/// Width of a tile in braille dots at its own zoom. Dots are about square in common terminal fonts.
const TILE_DOTS: f64 = 256.0;
/// The part of the screen that [`View::fitting`] fills, so that the points keep a margin.
const FIT_FILL: f64 = 0.8;
/// Web Mercator stops at this latitude, where the projected world is square.
const MAX_LAT: f64 = 85.051_128_779_806_59;

/// A Web Mercator view: the point at the center of the screen and a fractional zoom level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// World coordinates: `x` and `y` from 0 to 1, with `(0, 0)` at the north-west corner.
    x: f64,
    y: f64,
    zoom: f64,
}

/// A tile to draw, and where its top-left corner is in screen dots.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleTile {
    pub id: TileId,
    pub left: f64,
    pub top: f64,
    /// Width and height in dots. More than 256 when the view overzooms the tile.
    pub size: f64,
}

impl View {
    #[must_use]
    pub fn new(center: LatLon, zoom: f64) -> Self {
        let (x, y) = project(center);
        Self {
            x,
            y,
            zoom: zoom.clamp(0.0, MAX_ZOOM),
        }
    }

    #[must_use]
    pub fn center(self) -> LatLon {
        unproject(self.x, self.y)
    }

    #[must_use]
    pub fn zoom(self) -> f64 {
        self.zoom
    }

    /// Moves the view by a distance in dots. Positive values move east and south.
    #[must_use]
    pub fn pan(self, dx: f64, dy: f64) -> Self {
        let scale = self.dots_per_world();
        Self {
            x: (self.x + dx / scale).rem_euclid(1.0),
            y: (self.y + dy / scale).clamp(0.0, 1.0),
            zoom: self.zoom,
        }
    }

    #[must_use]
    pub fn zoom_by(self, delta: f64) -> Self {
        Self {
            zoom: (self.zoom + delta).clamp(0.0, MAX_ZOOM),
            ..self
        }
    }

    /// The tile zoom for this view: the integer part of the zoom, at most `max_zoom`.
    #[must_use]
    pub fn tile_zoom(self, max_zoom: u8) -> u8 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "zoom is 0 to 18"
        )]
        let zoom = self.zoom.floor() as u8;
        zoom.min(max_zoom)
    }

    /// The tiles that cover a screen of `width` × `height` dots, nearest to the center first.
    /// A tile can appear more than once when the world is narrower than the screen.
    #[must_use]
    pub fn visible_tiles(self, width: f64, height: f64, max_zoom: u8) -> Vec<VisibleTile> {
        let tile_zoom = self.tile_zoom(max_zoom);
        let tiles_per_side = f64::from(1_u32 << tile_zoom);
        let size = self.dots_per_world() / tiles_per_side;
        let (left, top) = self.screen_origin(width, height);
        let first_x = (left / size).floor();
        let last_x = ((left + width) / size).ceil();
        let first_y = (top / size).floor().max(0.0);
        let last_y = ((top + height) / size).ceil().min(tiles_per_side);

        let mut tiles = Vec::new();
        let mut tile_y = first_y;
        while tile_y < last_y {
            let mut tile_x = first_x;
            while tile_x < last_x {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "in 0..2^14"
                )]
                let id = TileId {
                    z: tile_zoom,
                    x: tile_x.rem_euclid(tiles_per_side) as u32,
                    y: tile_y as u32,
                };
                tiles.push(VisibleTile {
                    id,
                    left: tile_x * size - left,
                    top: tile_y * size - top,
                    size,
                });
                tile_x += 1.0;
            }
            tile_y += 1.0;
        }
        let distance = |tile: &VisibleTile| {
            let (cx, cy) = (
                tile.left + size / 2.0 - width / 2.0,
                tile.top + size / 2.0 - height / 2.0,
            );
            cx.hypot(cy)
        };
        tiles.sort_by(|a, b| distance(a).total_cmp(&distance(b)));
        tiles
    }

    /// The point at a position on a screen of `width` × `height` dots.
    #[must_use]
    pub fn point_at(self, x: f64, y: f64, width: f64, height: f64) -> LatLon {
        self.pan(x - width / 2.0, y - height / 2.0).center()
    }

    /// The position in dots of `point` on a screen of `width` × `height` dots. Of the copies of
    /// the world, the point takes the one nearest to the center.
    #[must_use]
    pub fn screen_position(self, point: LatLon, width: f64, height: f64) -> (f64, f64) {
        let (x, y) = project(point);
        self.world_to_screen(nearest_copy(x, self.x), y, width, height)
    }

    /// The screen positions in dots of a line. Each point takes the world copy nearest to the
    /// previous point, so a line across the antimeridian stays connected.
    #[must_use]
    pub fn line_on_screen(self, points: &[LatLon], width: f64, height: f64) -> Vec<(f64, f64)> {
        unwrap(points, self.x)
            .map(|(x, y)| self.world_to_screen(x, y, width, height))
            .collect()
    }

    /// The view that shows all `points` with a margin on a screen of `width` × `height` dots,
    /// or `None` without points.
    #[must_use]
    pub fn fitting(points: &[LatLon], width: f64, height: f64) -> Option<Self> {
        let (first_x, _) = project(*points.first()?);
        let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
        let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for (x, y) in unwrap(points, first_x) {
            (min_x, max_x) = (min_x.min(x), max_x.max(x));
            (min_y, max_y) = (min_y.min(y), max_y.max(y));
        }
        let zoom_for = |screen: f64, span: f64| (screen * FIT_FILL / (span * TILE_DOTS)).log2();
        let zoom = zoom_for(width, max_x - min_x).min(zoom_for(height, max_y - min_y));
        Some(Self {
            x: f64::midpoint(min_x, max_x).rem_euclid(1.0),
            y: f64::midpoint(min_y, max_y),
            // NaN comes from one point on a screen of size 0.
            zoom: if zoom.is_nan() {
                MAX_ZOOM
            } else {
                zoom.clamp(0.0, MAX_ZOOM)
            },
        })
    }

    fn world_to_screen(self, x: f64, y: f64, width: f64, height: f64) -> (f64, f64) {
        let scale = self.dots_per_world();
        (
            (x - self.x) * scale + width / 2.0,
            (y - self.y) * scale + height / 2.0,
        )
    }

    fn dots_per_world(self) -> f64 {
        TILE_DOTS * self.zoom.exp2()
    }

    /// World position of the top-left screen corner, in dots at this zoom.
    fn screen_origin(self, width: f64, height: f64) -> (f64, f64) {
        let scale = self.dots_per_world();
        (self.x * scale - width / 2.0, self.y * scale - height / 2.0)
    }
}

fn project(point: LatLon) -> (f64, f64) {
    let lat = point.lat().clamp(-MAX_LAT, MAX_LAT).to_radians();
    let x = (point.lon() + 180.0) / 360.0;
    let y = (1.0 - lat.tan().asinh() / PI) / 2.0;
    (x, y)
}

/// The world coordinates of `points`. Each `x` takes the world copy nearest to the previous one,
/// and the first the copy nearest to `start_x`.
fn unwrap(points: &[LatLon], start_x: f64) -> impl Iterator<Item = (f64, f64)> {
    let mut previous = start_x;
    points.iter().map(move |&point| {
        let (x, y) = project(point);
        previous = nearest_copy(x, previous);
        (previous, y)
    })
}

/// `x` moved by whole worlds to within half a world of `reference`.
fn nearest_copy(x: f64, reference: f64) -> f64 {
    x + (reference - x + 0.5).floor()
}

fn unproject(x: f64, y: f64) -> LatLon {
    let lat = (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees();
    let lon = x * 360.0 - 180.0;
    LatLon::clamped(lat, lon)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn milan() -> LatLon {
        LatLon::new(45.4642, 9.19).unwrap()
    }

    fn assert_near(actual: LatLon, expected: LatLon) {
        assert!(
            actual.distance_m(expected) < 0.01,
            "{actual} is not near {expected}"
        );
    }

    #[test]
    fn center_survives_projection() {
        assert_near(View::new(milan(), 14.0).center(), milan());
    }

    #[test]
    fn pan_by_a_tile_at_zoom_zero_wraps_around_the_world() {
        let view = View::new(milan(), 0.0).pan(256.0, 0.0);
        assert_near(view.center(), milan());
    }

    #[test]
    fn pan_stops_at_the_projection_edge() {
        let view = View::new(milan(), 0.0).pan(0.0, -10_000.0);
        assert!((view.center().lat() - MAX_LAT).abs() < 1e-6);
    }

    #[test]
    fn center_is_at_the_middle_of_the_screen() {
        let position = View::new(milan(), 14.0).screen_position(milan(), 200.0, 100.0);
        assert_eq!(position, (100.0, 50.0));
    }

    #[test]
    fn line_across_the_antimeridian_stays_connected() {
        let line = [
            LatLon::new(-17.0, 179.9).unwrap(),
            LatLon::new(-17.0, -179.9).unwrap(),
        ];
        let view = View::new(line[0], 10.0);
        let points = view.line_on_screen(&line, 200.0, 100.0);
        assert!((points[1].0 - points[0].0).abs() < 200.0);
    }

    #[test]
    fn fitting_view_shows_every_point() {
        let rome = LatLon::new(41.9028, 12.4964).unwrap();
        let view = View::fitting(&[milan(), rome], 200.0, 100.0).unwrap();
        let inside = [milan(), rome].map(|point| {
            let (x, y) = view.screen_position(point, 200.0, 100.0);
            (0.0..=200.0).contains(&x) && (0.0..=100.0).contains(&y)
        });
        assert_eq!(inside, [true, true]);
    }

    #[test]
    fn fitting_one_point_uses_the_max_zoom() {
        let view = View::fitting(&[milan()], 200.0, 100.0).unwrap();
        assert_eq!(view.zoom(), MAX_ZOOM);
    }

    #[test]
    fn tile_zoom_does_not_pass_max_zoom() {
        assert_eq!(View::new(milan(), 16.7).tile_zoom(14), 14);
    }

    #[test]
    fn visible_tiles_start_with_the_tile_at_the_center() {
        let tiles = View::new(milan(), 14.0).visible_tiles(400.0, 200.0, 14);
        assert_eq!(
            tiles[0].id,
            TileId {
                z: 14,
                x: 8610,
                y: 5863
            }
        );
    }

    #[test]
    fn visible_tiles_cover_the_screen() {
        let tiles = View::new(milan(), 14.5).visible_tiles(400.0, 200.0, 14);
        let covers = |x: f64, y: f64| {
            tiles.iter().any(|tile| {
                (tile.left..tile.left + tile.size).contains(&x)
                    && (tile.top..tile.top + tile.size).contains(&y)
            })
        };
        assert!(covers(0.0, 0.0) && covers(399.0, 199.0));
    }

    #[test]
    fn overzoom_scales_tiles() {
        let tiles = View::new(milan(), 16.0).visible_tiles(400.0, 200.0, 14);
        assert!((tiles[0].size - 1024.0).abs() < 1e-9);
    }

    #[test]
    fn point_at_the_screen_center_is_the_view_center() {
        let view = View::new(milan(), 12.0);
        assert_near(view.point_at(200.0, 100.0, 400.0, 200.0), milan());
    }
}
