use std::fmt;
use std::str::FromStr;

use crate::label::LabelClass;
use crate::mvt::{self, Feature, Value};
use crate::{Error, Result};

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

    /// Areas are filled. The other classes are lines.
    #[must_use]
    pub fn is_area(self) -> bool {
        matches!(self, Self::Green | Self::Water | Self::Building)
    }

    #[must_use]
    pub fn is_route(self) -> bool {
        matches!(self, Self::Alternative | Self::Route)
    }

    /// The layer that shows or hides the class. Classes without a layer always show.
    #[must_use]
    pub fn layer(self) -> Option<Layer> {
        match self {
            Self::Highway | Self::MajorRoad | Self::MinorRoad | Self::Path => Some(Layer::Roads),
            Self::Rail => Some(Layer::Transit),
            Self::Building => Some(Layer::Buildings),
            Self::Green => Some(Layer::Nature),
            Self::Boundary => Some(Layer::Boundaries),
            Self::Water | Self::Waterway | Self::Alternative | Self::Route => None,
        }
    }

    pub(crate) fn index(self) -> usize {
        self as usize
    }
}

/// A group of classes that the user can show or hide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layer {
    Roads,
    Transit,
    Buildings,
    Nature,
    Boundaries,
    /// All map labels. A label of another layer shows only when that layer shows too.
    Labels,
}

impl Layer {
    pub const ALL: [Self; 6] = [
        Self::Roads,
        Self::Transit,
        Self::Buildings,
        Self::Nature,
        Self::Boundaries,
        Self::Labels,
    ];

    /// The name that [`Layer::from_str`] reads, for example `roads`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Roads => "roads",
            Self::Transit => "transit",
            Self::Buildings => "buildings",
            Self::Nature => "nature",
            Self::Boundaries => "boundaries",
            Self::Labels => "labels",
        }
    }

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Layer {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|layer| layer.name() == name)
            .ok_or_else(|| Error::InvalidInput(format!("unknown layer {name:?}")))
    }
}

/// The layers that a map shows. The default shows all layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Layers(u8);

impl Layers {
    pub const ALL: Self = Self((1 << Layer::ALL.len()) - 1);
    pub const NONE: Self = Self(0);

    #[must_use]
    pub fn contains(self, layer: Layer) -> bool {
        self.0 & layer.bit() != 0
    }

    /// The same layers, with `layer` hidden if it shows, else shown.
    #[must_use]
    pub fn toggled(self, layer: Layer) -> Self {
        Self(self.0 ^ layer.bit())
    }

    #[must_use]
    pub fn without(self, layer: Layer) -> Self {
        Self(self.0 & !layer.bit())
    }

    #[must_use]
    pub fn shows(self, class: Class) -> bool {
        class.layer().is_none_or(|layer| self.contains(layer))
    }

    #[must_use]
    pub fn shows_label(self, class: LabelClass) -> bool {
        self.contains(Layer::Labels) && class.layer().is_none_or(|layer| self.contains(layer))
    }

    #[must_use]
    pub fn shows_the_labels_of(self, other: Self) -> bool {
        LabelClass::ALL
            .into_iter()
            .all(|class| self.shows_label(class) == other.shows_label(class))
    }
}

impl Default for Layers {
    fn default() -> Self {
        Self::ALL
    }
}

impl FromIterator<Layer> for Layers {
    fn from_iter<I: IntoIterator<Item = Layer>>(layers: I) -> Self {
        Self(layers.into_iter().fold(0, |bits, layer| bits | layer.bit()))
    }
}

pub(crate) struct Style {
    pub class: Class,
    /// The view hides the feature below this zoom, where it is too dense to read.
    pub min_zoom: f64,
}

/// The layers that [`classify`] can give a style.
pub(crate) const LAYERS: [&str; 7] = [
    "water",
    "park",
    "landcover",
    "building",
    "waterway",
    "transportation",
    "boundary",
];

/// Picks the style of a feature from its layer and properties in the `OpenMapTiles` schema:
/// <https://openmaptiles.org/schema/>.
pub(crate) fn classify(layer: &mvt::Layer, feature: &Feature) -> Option<Style> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_hide_the_classes_of_a_hidden_layer() {
        assert!(!Layers::ALL.toggled(Layer::Transit).shows(Class::Rail));
    }

    #[test]
    fn layers_show_the_classes_of_other_layers() {
        assert!(Layers::ALL.toggled(Layer::Transit).shows(Class::Highway));
    }

    #[test]
    fn layers_always_show_classes_without_a_layer() {
        assert!(Layers::NONE.shows(Class::Water));
    }

    #[test]
    fn layers_hide_road_labels_with_the_roads() {
        assert!(!Layers::NONE.shows_label(LabelClass::Road));
    }

    #[test]
    fn layers_without_labels_hide_every_label() {
        assert!(
            !Layers::ALL
                .toggled(Layer::Labels)
                .shows_label(LabelClass::City)
        );
    }

    #[test]
    fn layers_show_a_label_without_a_layer_while_labels_show() {
        let labels: Layers = [Layer::Labels].into_iter().collect();
        assert!(labels.shows_label(LabelClass::City));
    }

    #[test]
    fn layers_without_labels_still_show_the_shapes() {
        assert!(Layers::ALL.toggled(Layer::Labels).shows(Class::Highway));
    }

    #[test]
    fn layers_without_a_shown_layer_hide_it() {
        assert!(!Layers::ALL.without(Layer::Labels).contains(Layer::Labels));
    }

    #[test]
    fn layers_without_a_hidden_layer_are_unchanged() {
        let layers = Layers::ALL.toggled(Layer::Roads);
        assert_eq!(layers.without(Layer::Roads), layers);
    }

    #[test]
    fn layers_without_buildings_show_the_same_labels() {
        let layers = Layers::ALL.toggled(Layer::Buildings);
        assert!(layers.shows_the_labels_of(Layers::ALL));
    }

    #[test]
    fn layers_without_roads_show_other_labels() {
        let layers = Layers::ALL.toggled(Layer::Roads);
        assert!(!layers.shows_the_labels_of(Layers::ALL));
    }

    #[test]
    fn layers_toggled_twice_are_unchanged() {
        let layers = Layers::ALL.toggled(Layer::Nature).toggled(Layer::Nature);
        assert_eq!(layers, Layers::ALL);
    }

    #[test]
    fn layers_from_all_layers_are_all() {
        assert_eq!(Layer::ALL.into_iter().collect::<Layers>(), Layers::ALL);
    }

    #[test]
    fn layers_from_some_layers_contain_only_those() {
        let layers: Layers = [Layer::Roads, Layer::Nature].into_iter().collect();
        let shown: Vec<Layer> = Layer::ALL
            .into_iter()
            .filter(|&layer| layers.contains(layer))
            .collect();
        assert_eq!(shown, [Layer::Roads, Layer::Nature]);
    }

    #[test]
    fn layer_reads_its_own_name() {
        let read: Vec<Layer> = Layer::ALL
            .into_iter()
            .filter_map(|layer| layer.name().parse().ok())
            .collect();
        assert_eq!(read, Layer::ALL);
    }

    #[test]
    fn layer_rejects_an_unknown_name() {
        assert!("traffic".parse::<Layer>().is_err());
    }
}
