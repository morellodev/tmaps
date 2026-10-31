use crate::mvt::{Feature, Geometry, Layer, Value};
use crate::scene::Part;

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

/// A label that a renderer places when the view shows it and other labels leave space.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelCandidate {
    pub class: LabelClass,
    /// Lower ranks go first within a class.
    pub rank: i64,
    pub min_zoom: f64,
    pub max_zoom: f64,
    pub text: String,
    /// Points are fractions of the tile width, as in the shapes of a prepared tile.
    pub anchor: Anchor,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Anchor {
    Point([f32; 2]),
    Lines(Vec<Part>),
}

/// The layers that [`LabelCandidate::from_feature`] can read a label from.
pub(crate) const LAYERS: [&str; 3] = ["place", "water_name", "transportation_name"];

impl LabelCandidate {
    /// Reads a label from a feature of the `place`, `water_name`, or `transportation_name` layer
    /// of the `OpenMapTiles` schema. `lang` is the language subtag of the names, for example `it`.
    #[must_use]
    pub fn from_feature(layer: &Layer, feature: &Feature, lang: &str) -> Option<Self> {
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
                    .map(|line| {
                        Part::new(line.iter().map(|&point| normalize(point, extent)).collect())
                    })
                    .collect(),
            ),
            _ => return None,
        };
        Some(Self {
            class,
            rank,
            min_zoom,
            max_zoom,
            text,
            anchor,
        })
    }
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

#[expect(clippy::cast_precision_loss, reason = "tile units are below 2^24")]
fn normalize(point: crate::mvt::Point, extent: u32) -> [f32; 2] {
    let extent = extent as f32;
    [point.x as f32 / extent, point.y as f32 / extent]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string(text: &str) -> Value {
        Value::String(text.to_owned())
    }

    fn line(points: &[(i32, i32)]) -> Geometry {
        Geometry::Lines(vec![
            points
                .iter()
                .map(|&(x, y)| crate::mvt::Point { x, y })
                .collect(),
        ])
    }

    fn road(properties: Vec<(&str, Value)>, geometry: Geometry) -> Vec<String> {
        let layer = Layer::with_features("transportation_name", 40, vec![(geometry, properties)]);
        layer
            .features
            .iter()
            .filter_map(|feature| LabelCandidate::from_feature(&layer, feature, "it"))
            .map(|candidate| candidate.text)
            .collect()
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
        assert_eq!(found, ["Via Milano"]);
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
        assert_eq!(found, ["Hauptstraße"]);
    }

    #[test]
    fn road_without_a_name_gets_its_ref() {
        let found = road(
            vec![("class", string("motorway")), ("ref", string("A4"))],
            line(&[(0, 0), (40, 0)]),
        );
        assert_eq!(found, ["A4"]);
    }

    #[test]
    fn unknown_road_class_gets_no_label() {
        let found = road(
            vec![("class", string("ferry")), ("name", string("Traghetto"))],
            line(&[(0, 0), (40, 0)]),
        );
        assert_eq!(found, Vec::<String>::new());
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
}
