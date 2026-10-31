use std::fmt::Write as _;

use tmaps_core::PreparedTile;
use tmaps_core::scene::Shape;
use tmaps_core::style::Class;
use wasm_bindgen::JsValue;
use web_sys::Path2d;

/// The width of a tile in path units. Canvas engines treat very short segments as empty, so
/// paths use the MVT extent instead of fractions of the tile.
pub const UNITS: f64 = 4096.0;

/// A tile with its shapes as canvas paths, in [`UNITS`]. The browser builds the paths once, when
/// the tile arrives, so that a frame only moves them with a transform. The tile keeps only the
/// labels of its [`PreparedTile`].
pub struct CanvasTile {
    prepared: PreparedTile,
    /// For each class in [`Class::ALL`] order: one path for each min zoom of its shapes.
    paths: Vec<Vec<(f64, Path2d)>>,
}

impl CanvasTile {
    /// # Errors
    ///
    /// If the `Path2D` constructor throws. Invalid path data does not make it throw: the path
    /// stops at the first error.
    pub fn new(mut prepared: PreparedTile) -> Result<Self, JsValue> {
        let paths = Class::ALL
            .iter()
            .map(|&class| {
                path_data(prepared.shapes(class), class.is_area())
                    .into_iter()
                    .map(|(min_zoom, data)| Ok((min_zoom, Path2d::new_with_path_string(&data)?)))
                    .collect()
            })
            .collect::<Result<_, JsValue>>()?;
        prepared.clear_shapes();
        Ok(Self { prepared, paths })
    }

    /// The paths of `class` that `zoom` shows.
    pub fn paths(&self, class: Class, zoom: f64) -> impl Iterator<Item = &Path2d> {
        let index = Class::ALL.iter().position(|&each| each == class);
        index
            .map_or(&[][..], |index| &self.paths[index])
            .iter()
            .filter(move |(min_zoom, _)| zoom >= *min_zoom)
            .map(|(_, path)| path)
    }
}

impl AsRef<PreparedTile> for CanvasTile {
    fn as_ref(&self) -> &PreparedTile {
        &self.prepared
    }
}

/// SVG path data of `shapes`, one string for each min zoom. Areas close their rings. One string
/// holds many features, so fill it with the nonzero rule: MVT 2.1 (4.3.4.4) winds holes against
/// their exterior rings, so holes stay empty, and overlapping features stay filled.
fn path_data(shapes: &[Shape], closed: bool) -> Vec<(f64, String)> {
    let mut groups: Vec<(f64, String)> = Vec::new();
    for shape in shapes {
        let index = groups
            .iter()
            .position(|(min_zoom, _)| min_zoom.to_bits() == shape.min_zoom().to_bits())
            .unwrap_or_else(|| {
                groups.push((shape.min_zoom(), String::new()));
                groups.len() - 1
            });
        let data = &mut groups[index].1;
        for part in shape.parts() {
            for (step, &[x, y]) in part.points().iter().enumerate() {
                let command = if step == 0 { 'M' } else { 'L' };
                let _ = write!(data, "{command}{} {}", unit(x), unit(y));
            }
            if closed && !part.points().is_empty() {
                data.push('Z');
            }
        }
    }
    groups.retain(|(_, data)| !data.is_empty());
    groups
}

/// A tile fraction in whole path units. Most tiles use the 4096 extent, so this is exact for
/// them, and integers format much faster than floats.
fn unit(fraction: f32) -> i32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "tile geometry stays within a few tile widths of the tile"
    )]
    let unit = (f64::from(fraction) * UNITS).round() as i32;
    unit
}

#[cfg(test)]
mod tests {
    use tmaps_core::mvt::{Geometry, Layer, Point, Tile, Value};

    use super::*;

    /// A layer name, a geometry, and the properties of one feature.
    type Feature<'a> = (&'a str, Geometry, Vec<(&'a str, Value)>);

    fn tile(features: Vec<Feature>) -> PreparedTile {
        let layers = features
            .into_iter()
            .map(|(name, geometry, properties)| {
                Layer::with_features(name, 4096, vec![(geometry, properties)])
            })
            .collect();
        PreparedTile::new(&Tile { layers }, "it")
    }

    fn points(points: &[(i32, i32)]) -> Vec<Point> {
        points.iter().map(|&(x, y)| Point { x, y }).collect()
    }

    fn class(name: &str) -> Vec<(&'static str, Value)> {
        vec![("class", Value::String(name.to_owned()))]
    }

    #[test]
    fn area_rings_close_and_lines_stay_open() {
        let tile = tile(vec![
            (
                "water",
                Geometry::Polygons(vec![points(&[(0, 0), (2048, 0), (2048, 2048)])]),
                vec![],
            ),
            (
                "transportation",
                Geometry::Lines(vec![points(&[(10, 20), (30, 40)])]),
                class("primary"),
            ),
        ]);
        assert_eq!(
            path_data(tile.shapes(Class::Water), true),
            [(0.0, "M0 0L2048 0L2048 2048Z".to_owned())]
        );
        assert_eq!(
            path_data(tile.shapes(Class::MajorRoad), false),
            [(0.0, "M10 20L30 40".to_owned())]
        );
    }

    #[test]
    fn shapes_of_one_class_split_by_min_zoom() {
        let road = |name, x| {
            (
                "transportation",
                Geometry::Lines(vec![points(&[(x, 0), (x, 10)])]),
                class(name),
            )
        };
        let tile = tile(vec![
            road("minor", 1),
            road("service", 2),
            road("tertiary", 3),
        ]);
        assert_eq!(
            path_data(tile.shapes(Class::MinorRoad), false),
            [
                (0.0, "M1 0L1 10M3 0L3 10".to_owned()),
                (15.0, "M2 0L2 10".to_owned()),
            ]
        );
    }

    #[test]
    fn class_without_shapes_has_no_path() {
        assert_eq!(
            path_data(PreparedTile::default().shapes(Class::Rail), false),
            []
        );
    }
}
