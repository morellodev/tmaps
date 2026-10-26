use crate::mvt::{Feature, Layer, Value};

/// What a feature is on the map. The order is the draw order: later classes cover earlier ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    Green,
    Water,
    Building,
    Boundary,
    Waterway,
    Rail,
    Path,
    MinorRoad,
    MajorRoad,
    Highway,
    /// A route that is not selected. Routes come from the overlay, not from tiles.
    Alternative,
    Route,
}

impl Class {
    pub const ALL: [Self; 12] = [
        Self::Green,
        Self::Water,
        Self::Building,
        Self::Boundary,
        Self::Waterway,
        Self::Rail,
        Self::Path,
        Self::MinorRoad,
        Self::MajorRoad,
        Self::Highway,
        Self::Alternative,
        Self::Route,
    ];

    /// Areas fill the background of cells. The other classes draw lines of dots.
    #[must_use]
    pub fn is_area(self) -> bool {
        matches!(self, Self::Green | Self::Water | Self::Building)
    }

    #[must_use]
    pub fn is_route(self) -> bool {
        matches!(self, Self::Alternative | Self::Route)
    }

    pub(super) fn index(self) -> usize {
        self as usize
    }
}

pub(super) struct Style {
    pub class: Class,
    /// The view hides the feature below this zoom, where it is too dense to read.
    pub min_zoom: f64,
}

/// Picks the style of a feature from its layer and properties in the `OpenMapTiles` schema:
/// <https://openmaptiles.org/schema/>.
pub(super) fn classify(layer: &Layer, feature: &Feature) -> Option<Style> {
    let text = |key| layer.property(feature, key).and_then(Value::as_str);
    let style = |class, min_zoom| Some(Style { class, min_zoom });
    match layer.name.as_str() {
        "water" => style(Class::Water, 0.0),
        "park" => style(Class::Green, 0.0),
        "landcover" => match text("class")? {
            "wood" | "grass" => style(Class::Green, 0.0),
            _ => None,
        },
        "building" => style(Class::Building, 15.0),
        "waterway" => match text("class")? {
            "river" | "canal" => style(Class::Waterway, 0.0),
            _ => style(Class::Waterway, 13.0),
        },
        "transportation" if text("brunnel") != Some("tunnel") => match text("class")? {
            "motorway" | "trunk" => style(Class::Highway, 0.0),
            "primary" | "secondary" => style(Class::MajorRoad, 0.0),
            "tertiary" | "minor" => style(Class::MinorRoad, 0.0),
            "service" => style(Class::MinorRoad, 15.0),
            "track" | "path" => style(Class::Path, 15.0),
            "rail" | "transit" => style(Class::Rail, 0.0),
            _ => None,
        },
        "boundary" => {
            let number = |key| layer.property(feature, key).and_then(Value::as_i64);
            let is_land_border = number("admin_level").is_some_and(|level| level <= 4)
                && number("maritime") != Some(1);
            if is_land_border {
                style(Class::Boundary, 0.0)
            } else {
                None
            }
        }
        _ => None,
    }
}
