//! Decoder for Mapbox Vector Tiles, spec v2.1: <https://github.com/mapbox/vector-tile-spec/tree/master/2.1>.

use prost::Message;

use crate::{Error, Result};

const DEFAULT_EXTENT: u32 = 4096;

#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    pub layers: Vec<Layer>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub name: String,
    /// Width and height of the tile in geometry units.
    pub extent: u32,
    pub features: Vec<Feature>,
    keys: Vec<String>,
    values: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Feature {
    pub geometry: Geometry,
    /// Pairs of indexes into the keys and values of the layer.
    tags: Vec<u32>,
}

/// Points are in tile units: `(0, 0)` is the top-left corner, `(extent, extent)` the bottom-right.
/// Geometry can extend past the tile edges into the buffer.
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    Points(Vec<Point>),
    Lines(Vec<Vec<Point>>),
    /// Exterior and interior rings of all polygons. An even-odd fill draws the holes.
    Polygons(Vec<Vec<Point>>),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    String(String),
    Float(f64),
    Int(i64),
    Bool(bool),
}

impl Value {
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Self::Int(number) => Some(number),
            #[expect(
                clippy::cast_possible_truncation,
                reason = "admin levels and ranks are small"
            )]
            Self::Float(number) if number.fract() == 0.0 => Some(number as i64),
            Self::Bool(flag) => Some(i64::from(flag)),
            _ => None,
        }
    }
}

impl Layer {
    #[must_use]
    pub fn property(&self, feature: &Feature, key: &str) -> Option<&Value> {
        feature.tags.as_chunks::<2>().0.iter().find_map(|pair| {
            let key_index = usize::try_from(pair[0]).ok()?;
            let value_index = usize::try_from(pair[1]).ok()?;
            (self.keys.get(key_index)? == key).then(|| self.values.get(value_index))?
        })
    }
}

impl Tile {
    /// # Errors
    ///
    /// [`Error::Provider`] if `bytes` is not a valid vector tile.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let tile = proto::Tile::decode(bytes).map_err(|error| invalid(&error))?;
        let layers = tile.layers.into_iter().map(Layer::from_proto).collect();
        Ok(Self { layers })
    }

    /// Like [`Tile::decode`], but keeps only the layers whose name passes `keep`. The other
    /// layers are not decoded, which saves most of the time on tiles with many points of interest.
    ///
    /// # Errors
    ///
    /// [`Error::Provider`] if `bytes` is not a valid vector tile.
    pub fn decode_layers(bytes: &[u8], keep: impl Fn(&str) -> bool) -> Result<Self> {
        let raw = proto::RawTile::decode(bytes).map_err(|error| invalid(&error))?;
        let mut layers = Vec::new();
        for layer in raw.layers {
            let name = proto::LayerName::decode(layer.as_slice())
                .map_err(|error| invalid(&error))?
                .name;
            if keep(&name) {
                let layer =
                    proto::Layer::decode(layer.as_slice()).map_err(|error| invalid(&error))?;
                layers.push(Layer::from_proto(layer));
            }
        }
        Ok(Self { layers })
    }
}

fn invalid(error: &prost::DecodeError) -> Error {
    Error::Provider(format!("invalid vector tile: {error}"))
}

impl Layer {
    fn from_proto(layer: proto::Layer) -> Self {
        Self {
            name: layer.name,
            extent: layer.extent.unwrap_or(DEFAULT_EXTENT),
            features: layer
                .features
                .into_iter()
                .map(|feature| Feature {
                    geometry: decode_geometry(feature.r#type, &feature.geometry),
                    tags: feature.tags,
                })
                .collect(),
            keys: layer.keys,
            values: layer.values.into_iter().map(Value::from).collect(),
        }
    }
}

impl From<proto::Value> for Value {
    fn from(value: proto::Value) -> Self {
        if let Some(text) = value.string {
            Self::String(text)
        } else if let Some(number) = value.double {
            Self::Float(number)
        } else if let Some(number) = value.float {
            Self::Float(f64::from(number))
        } else if let Some(number) = value.int.or(value.sint) {
            Self::Int(number)
        } else if let Some(number) = value.uint {
            Self::Int(i64::try_from(number).unwrap_or(i64::MAX))
        } else {
            Self::Bool(value.bool.unwrap_or_default())
        }
    }
}

const MOVE_TO: u32 = 1;
const LINE_TO: u32 = 2;
const CLOSE_PATH: u32 = 7;

/// Runs the command stream of section 4.3 of the spec. A malformed stream ends the geometry
/// early instead of failing the whole tile.
fn decode_geometry(kind: Option<i32>, commands: &[u32]) -> Geometry {
    let mut parts: Vec<Vec<Point>> = Vec::new();
    let mut cursor = Point { x: 0, y: 0 };
    let mut words = commands.iter().copied();
    while let Some(header) = words.next() {
        let (command, count) = (header & 0x7, header >> 3);
        match command {
            MOVE_TO | LINE_TO => {
                for _ in 0..count {
                    let (Some(dx), Some(dy)) = (words.next(), words.next()) else {
                        return finish(kind, parts);
                    };
                    cursor.x = cursor.x.wrapping_add(zigzag(dx));
                    cursor.y = cursor.y.wrapping_add(zigzag(dy));
                    if command == MOVE_TO || parts.is_empty() {
                        parts.push(vec![cursor]);
                    } else if let Some(part) = parts.last_mut() {
                        part.push(cursor);
                    }
                }
            }
            CLOSE_PATH => {
                if let Some(part) = parts.last_mut()
                    && let Some(&first) = part.first()
                {
                    part.push(first);
                }
            }
            _ => return finish(kind, parts),
        }
    }
    finish(kind, parts)
}

fn finish(kind: Option<i32>, parts: Vec<Vec<Point>>) -> Geometry {
    match kind.and_then(|kind| proto::GeomType::try_from(kind).ok()) {
        Some(proto::GeomType::Point) => Geometry::Points(parts.into_iter().flatten().collect()),
        Some(proto::GeomType::Linestring) => Geometry::Lines(parts),
        Some(proto::GeomType::Polygon) => Geometry::Polygons(parts),
        _ => Geometry::Unknown,
    }
}

#[expect(
    clippy::cast_possible_wrap,
    reason = "zigzag maps u32 onto the full i32 range"
)]
fn zigzag(value: u32) -> i32 {
    ((value >> 1) as i32) ^ -((value & 1) as i32)
}

/// Messages of `vector_tile.proto` v2.1, written by hand because the spec is frozen.
mod proto {
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Tile {
        #[prost(message, repeated, tag = "3")]
        pub layers: Vec<Layer>,
    }

    /// A [`Tile`] with the encoded bytes of each layer, to read the names first.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct RawTile {
        #[prost(bytes = "vec", repeated, tag = "3")]
        pub layers: Vec<Vec<u8>>,
    }

    /// Only the name of a [`Layer`]. Decoding skips the features.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct LayerName {
        #[prost(string, required, tag = "1")]
        pub name: String,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Layer {
        #[prost(uint32, required, tag = "15")]
        pub version: u32,
        #[prost(string, required, tag = "1")]
        pub name: String,
        #[prost(message, repeated, tag = "2")]
        pub features: Vec<Feature>,
        #[prost(string, repeated, tag = "3")]
        pub keys: Vec<String>,
        #[prost(message, repeated, tag = "4")]
        pub values: Vec<Value>,
        #[prost(uint32, optional, tag = "5")]
        pub extent: Option<u32>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Feature {
        #[prost(uint64, optional, tag = "1")]
        pub id: Option<u64>,
        #[prost(uint32, repeated, packed = "true", tag = "2")]
        pub tags: Vec<u32>,
        #[prost(enumeration = "GeomType", optional, tag = "3")]
        pub r#type: Option<i32>,
        #[prost(uint32, repeated, packed = "true", tag = "4")]
        pub geometry: Vec<u32>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Value {
        #[prost(string, optional, tag = "1")]
        pub string: Option<String>,
        #[prost(float, optional, tag = "2")]
        pub float: Option<f32>,
        #[prost(double, optional, tag = "3")]
        pub double: Option<f64>,
        #[prost(int64, optional, tag = "4")]
        pub int: Option<i64>,
        #[prost(uint64, optional, tag = "5")]
        pub uint: Option<u64>,
        #[prost(sint64, optional, tag = "6")]
        pub sint: Option<i64>,
        #[prost(bool, optional, tag = "7")]
        pub bool: Option<bool>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
    #[repr(i32)]
    pub enum GeomType {
        Unknown = 0,
        Point = 1,
        Linestring = 2,
        Polygon = 3,
    }
}

#[cfg(test)]
impl Layer {
    /// A layer with one feature for each `(geometry, properties)` pair.
    pub(crate) fn with_features(
        name: &str,
        extent: u32,
        features: Vec<(Geometry, Vec<(&str, Value)>)>,
    ) -> Self {
        let mut layer = Self {
            name: name.to_owned(),
            extent,
            features: Vec::new(),
            keys: Vec::new(),
            values: Vec::new(),
        };
        for (geometry, properties) in features {
            let mut tags = Vec::new();
            for (key, value) in properties {
                tags.extend(
                    [layer.keys.len(), layer.values.len()].map(|i| u32::try_from(i).unwrap()),
                );
                layer.keys.push(key.to_owned());
                layer.values.push(value);
            }
            layer.features.push(Feature { geometry, tags });
        }
        layer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: i32, y: i32) -> Point {
        Point { x, y }
    }

    /// The examples of section 4.3.5 of the spec.
    mod decode_geometry {
        use super::*;

        const POINT: Option<i32> = Some(proto::GeomType::Point as i32);
        const LINE: Option<i32> = Some(proto::GeomType::Linestring as i32);
        const POLYGON: Option<i32> = Some(proto::GeomType::Polygon as i32);

        #[test]
        fn reads_a_point() {
            assert_eq!(
                decode_geometry(POINT, &[9, 50, 34]),
                Geometry::Points(vec![point(25, 17)])
            );
        }

        #[test]
        fn reads_two_lines() {
            let commands = [9, 4, 4, 18, 0, 16, 16, 0, 9, 17, 17, 10, 4, 8];
            assert_eq!(
                decode_geometry(LINE, &commands),
                Geometry::Lines(vec![
                    vec![point(2, 2), point(2, 10), point(10, 10)],
                    vec![point(1, 1), point(3, 5)],
                ])
            );
        }

        #[test]
        fn closes_a_polygon_ring() {
            assert_eq!(
                decode_geometry(POLYGON, &[9, 6, 12, 18, 10, 12, 24, 44, 15]),
                Geometry::Polygons(vec![vec![
                    point(3, 6),
                    point(8, 12),
                    point(20, 34),
                    point(3, 6)
                ]])
            );
        }

        #[test]
        fn keeps_the_parts_before_a_truncated_command() {
            assert_eq!(
                decode_geometry(LINE, &[9, 4, 4, 18, 0]),
                Geometry::Lines(vec![vec![point(2, 2)]])
            );
        }
    }

    #[test]
    fn decode_layers_keeps_only_the_layers_that_pass() {
        let layer = |name: &str| proto::Layer {
            version: 2,
            name: name.to_owned(),
            ..proto::Layer::default()
        };
        let bytes = proto::Tile {
            layers: vec![layer("poi"), layer("water")],
        }
        .encode_to_vec();
        let tile = Tile::decode_layers(&bytes, |name| name == "water").unwrap();
        let names: Vec<&str> = tile
            .layers
            .iter()
            .map(|layer| layer.name.as_str())
            .collect();
        assert_eq!(names, ["water"]);
    }

    #[test]
    fn property_finds_value_by_key() {
        let layer = Layer {
            name: "transportation".to_owned(),
            extent: DEFAULT_EXTENT,
            features: Vec::new(),
            keys: vec!["class".to_owned(), "brunnel".to_owned()],
            values: vec![
                Value::String("bridge".to_owned()),
                Value::String("primary".to_owned()),
            ],
        };
        let feature = Feature {
            geometry: Geometry::Unknown,
            tags: vec![1, 0, 0, 1],
        };
        assert_eq!(
            layer.property(&feature, "class"),
            Some(&Value::String("primary".to_owned()))
        );
    }
}
