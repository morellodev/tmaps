use crate::label::{self, LabelCandidate};
use crate::mvt::{self, Geometry};
use crate::style::{self, Class, classify};
use crate::tiles::TileId;
use crate::view::{View, VisibleTile};
use crate::{LatLon, Result};

/// The features of a tile that renderers draw, grouped by class, and its label candidates.
/// Preparing a tile takes longer than drawing it, so callers prepare it once, off the UI thread.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PreparedTile {
    shapes: [Vec<Shape>; Class::ALL.len()],
    labels: Vec<LabelCandidate>,
}

/// One feature: the rings of an area, or its lines.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    min_zoom: f64,
    parts: Vec<Part>,
}

/// Points are fractions of the tile width, so they do not depend on the tile extent.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    points: Vec<[f32; 2]>,
    /// The top-left and bottom-right corners of the box around the points.
    corners: [[f32; 2]; 2],
}

impl Part {
    pub(crate) fn new(points: Vec<[f32; 2]>) -> Self {
        let corners = points.iter().fold(
            [[f32::INFINITY; 2], [f32::NEG_INFINITY; 2]],
            |[low, high], &[x, y]| {
                [
                    [low[0].min(x), low[1].min(y)],
                    [high[0].max(x), high[1].max(y)],
                ]
            },
        );
        Self { points, corners }
    }

    #[must_use]
    pub fn points(&self) -> &[[f32; 2]] {
        &self.points
    }

    /// Whether the part can draw in `clip`. A ring outside it adds an even number of crossings
    /// to each row on one side of `clip`, so the fill inside does not change.
    #[must_use]
    pub fn touches(&self, placement: &VisibleTile, clip: Rect) -> bool {
        // [`clip`] can round a point a few ulps outside the rectangle onto the edge and draw it.
        const SLACK: f64 = 1.0;
        let (left, top) = to_screen(placement, self.corners[0]);
        let (right, bottom) = to_screen(placement, self.corners[1]);
        left <= clip.right + SLACK
            && right >= clip.left - SLACK
            && top <= clip.bottom + SLACK
            && bottom >= clip.top - SLACK
    }
}

/// A position on the screen, in units.
pub type ScreenPoint = (f64, f64);

/// A rectangle on the screen, in units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Rect {
    /// The whole screen of `width` × `height` units.
    #[must_use]
    pub fn screen(width: f64, height: f64) -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            right: width,
            bottom: height,
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

impl PreparedTile {
    /// Decodes and prepares a tile, without the layers that renderers do not use.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidData`](crate::Error::InvalidData) if `bytes` is not a valid vector tile.
    pub fn decode(bytes: &[u8], lang: &str) -> Result<Self> {
        let uses = |name: &str| style::LAYERS.contains(&name) || label::LAYERS.contains(&name);
        Ok(Self::new(&mvt::Tile::decode_layers(bytes, uses)?, lang))
    }

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
                    .extend(LabelCandidate::from_feature(layer, feature, lang));
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
                        Part::new(
                            part.iter()
                                .map(|point| [point.x as f32 / extent, point.y as f32 / extent])
                                .collect(),
                        )
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

    #[must_use]
    pub fn labels(&self) -> &[LabelCandidate] {
        &self.labels
    }
}

impl AsRef<Self> for PreparedTile {
    fn as_ref(&self) -> &Self {
        self
    }
}

/// The tiles of a view on a screen of `width` × `height` units, and the shapes and labels that
/// a renderer draws from them. `T` lets a renderer keep its own data with each tile.
pub struct Scene<'a, T> {
    view: View,
    width: f64,
    height: f64,
    tiles: Vec<SceneTile<'a, T>>,
}

/// A tile to draw, at its place on the screen.
#[derive(Debug)]
pub struct SceneTile<'a, T> {
    pub placement: VisibleTile,
    /// The part of the screen that the tile draws in. An ancestor draws only in the area of the
    /// tile that is not loaded.
    pub clip: Rect,
    /// The labels of an ancestor would move when the tile arrives, so it shows none.
    pub shows_labels: bool,
    pub tile: &'a T,
}

impl<T> Clone for SceneTile<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for SceneTile<'_, T> {}

impl<'a, T: AsRef<PreparedTile>> Scene<'a, T> {
    /// `tile` returns a loaded tile, or `None` for a tile that is not loaded yet. The nearest
    /// loaded ancestor fills the area of that tile, else the area stays empty.
    pub fn new(
        view: View,
        width: f64,
        height: f64,
        max_zoom: u8,
        tile: impl Fn(TileId) -> Option<&'a T>,
    ) -> Self {
        let screen = Rect::screen(width, height);
        let tiles = view
            .visible_tiles(width, height, max_zoom)
            .into_iter()
            .filter_map(|placement| {
                if let Some(tile) = tile(placement.id) {
                    return Some(SceneTile {
                        placement,
                        clip: screen,
                        shows_labels: true,
                        tile,
                    });
                }
                let (ancestor, tile) = loaded_ancestor(&placement, &tile)?;
                Some(SceneTile {
                    placement: ancestor,
                    clip: Rect::of_tile(&placement).intersection(screen),
                    shows_labels: false,
                    tile,
                })
            })
            .collect();
        Self {
            view,
            width,
            height,
            tiles,
        }
    }

    #[must_use]
    pub fn view(&self) -> View {
        self.view
    }

    #[must_use]
    pub fn screen(&self) -> Rect {
        Rect::screen(self.width, self.height)
    }

    /// The tiles, nearest to the center first.
    #[must_use]
    pub fn tiles(&self) -> &[SceneTile<'a, T>] {
        &self.tiles
    }

    /// The shapes of `class` that the zoom shows, tile by tile. Each shape is one feature: fill
    /// the rings of one shape together, so that holes stay empty.
    pub fn shapes(&self, class: Class) -> impl Iterator<Item = (&SceneTile<'a, T>, &'a Shape)> {
        let zoom = self.view.zoom();
        self.tiles.iter().flat_map(move |scene_tile| {
            let tile: &'a PreparedTile = scene_tile.tile.as_ref();
            tile.shapes[class.index()]
                .iter()
                .filter(move |shape| zoom >= shape.min_zoom)
                .map(move |shape| (scene_tile, shape))
        })
    }

    /// The label candidates that the zoom shows, with the index of each candidate in its tile.
    /// They come in placement order: by class and rank, then by tile and by order in the tile.
    /// A renderer that places each text once keeps the first candidate of that text.
    #[must_use]
    pub fn labels(&self) -> Vec<(&SceneTile<'a, T>, usize, &'a LabelCandidate)> {
        let zoom = self.view.zoom();
        let mut labels: Vec<_> = self
            .tiles
            .iter()
            .filter(|scene_tile| scene_tile.shows_labels)
            .flat_map(|scene_tile| {
                let tile: &'a PreparedTile = scene_tile.tile.as_ref();
                tile.labels
                    .iter()
                    .enumerate()
                    .filter(move |(_, candidate)| {
                        (candidate.min_zoom..candidate.max_zoom).contains(&zoom)
                    })
                    .map(move |(index, candidate)| (scene_tile, index, candidate))
            })
            .collect();
        labels.sort_by_key(|(_, _, candidate)| (candidate.class, candidate.rank));
        labels
    }

    #[must_use]
    pub fn line_on_screen(&self, points: &[LatLon]) -> Vec<ScreenPoint> {
        self.view.line_on_screen(points, self.width, self.height)
    }
}

impl<T> SceneTile<'_, T> {
    /// The parts of `shape` that can draw in the clip of the tile.
    pub fn parts<'s>(&self, shape: &'s Shape) -> impl Iterator<Item = &'s Part> {
        let (placement, clip) = (self.placement, self.clip);
        shape
            .parts
            .iter()
            .filter(move |part| part.touches(&placement, clip))
    }

    #[must_use]
    pub fn to_screen(&self, point: [f32; 2]) -> ScreenPoint {
        to_screen(&self.placement, point)
    }
}

fn loaded_ancestor<'a, T>(
    placement: &VisibleTile,
    tile: impl Fn(TileId) -> Option<&'a T>,
) -> Option<(VisibleTile, &'a T)> {
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

/// `point` in fractions of the tile, on the screen.
#[must_use]
pub fn to_screen(placement: &VisibleTile, [x, y]: [f32; 2]) -> ScreenPoint {
    (
        placement.left + f64::from(x) * placement.size,
        placement.top + f64::from(y) * placement.size,
    )
}

/// Liang–Barsky clipping of a segment to `rect`. Without it, a long segment of an overzoomed
/// tile would step through millions of units outside the screen.
#[must_use]
pub fn clip(a: ScreenPoint, b: ScreenPoint, rect: Rect) -> Option<(ScreenPoint, ScreenPoint)> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [
        (-dx, a.0 - rect.left),
        (dx, rect.right - a.0),
        (-dy, a.1 - rect.top),
        (dy, rect.bottom - a.1),
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

    fn square(start: f64, end: f64) -> Rect {
        Rect {
            left: start,
            top: start,
            right: end,
            bottom: end,
        }
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
    fn ancestor_clips_to_the_tiles_that_are_not_loaded_and_shows_no_labels() {
        let (root, empty) = (PreparedTile::default(), PreparedTile::default());
        let view = View::new(LatLon::new(0.0, 0.0).unwrap(), 1.0);
        let north_west = TileId { z: 1, x: 0, y: 0 };
        let scene = Scene::new(view, 16.0, 16.0, 14, |id| match id.z {
            0 => Some(&root),
            _ => (id == north_west).then_some(&empty),
        });
        let mut tiles: Vec<(u8, Rect, bool)> = scene
            .tiles()
            .iter()
            .map(|tile| (tile.placement.id.z, tile.clip, tile.shows_labels))
            .collect();
        tiles.sort_by(|a, b| {
            (a.1.top, a.1.left)
                .partial_cmp(&(b.1.top, b.1.left))
                .unwrap()
        });
        let rect = |left, top| Rect {
            left,
            top,
            right: left + 8.0,
            bottom: top + 8.0,
        };
        assert_eq!(
            tiles,
            [
                (1, square(0.0, 16.0), true),
                (0, rect(8.0, 0.0), false),
                (0, rect(0.0, 8.0), false),
                (0, rect(8.0, 8.0), false),
            ]
        );
    }

    #[test]
    fn labels_come_by_class_then_rank_and_skip_the_zooms_that_hide_them() {
        let place = |class: &str, name: &str, rank: i64| {
            (
                Geometry::Points(vec![mvt::Point { x: 0, y: 0 }]),
                vec![
                    ("class", mvt::Value::String(class.to_owned())),
                    ("name", mvt::Value::String(name.to_owned())),
                    ("rank", mvt::Value::Int(rank)),
                ],
            )
        };
        let layer = mvt::Layer::with_features(
            "place",
            4096,
            vec![
                place("town", "Monza", 2),
                place("village", "Lodi", 1),
                place("city", "Bergamo", 2),
                place("town", "Pavia", 1),
                place("city", "Milano", 1),
            ],
        );
        let tile = PreparedTile::new(
            &mvt::Tile {
                layers: vec![layer],
            },
            "it",
        );
        let view = View::new(LatLon::new(0.0, 0.0).unwrap(), 10.0);
        let loaded = TileId {
            z: 10,
            x: 512,
            y: 512,
        };
        let scene = Scene::new(view, 16.0, 16.0, 14, |id| (id == loaded).then_some(&tile));
        let texts: Vec<&str> = scene
            .labels()
            .into_iter()
            .map(|(_, _, candidate)| candidate.text.as_str())
            .collect();
        assert_eq!(texts, ["Milano", "Bergamo", "Pavia", "Monza"]);
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
