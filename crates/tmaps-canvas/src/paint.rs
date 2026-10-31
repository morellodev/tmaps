use tmaps_core::label::LabelClass;
use tmaps_core::style::Class;

/// Colors and widths for a light or a dark page. Units are CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    dark: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub color: &'static str,
    pub width: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Text {
    /// A CSS `font` value.
    pub font: &'static str,
    /// The font size in CSS pixels, which sets the height of the label box.
    pub size: u8,
    pub color: &'static str,
}

/// The zoom at which lines have their base width.
const BASE_ZOOM: f64 = 14.0;

impl Palette {
    #[must_use]
    pub fn new(dark: bool) -> Self {
        Self { dark }
    }

    fn pick(self, light: &'static str, dark: &'static str) -> &'static str {
        if self.dark { dark } else { light }
    }

    #[must_use]
    pub fn background(self) -> &'static str {
        self.pick("#ece7df", "#1b1d22")
    }

    /// The fill of an area class, or `None` for a line class.
    #[must_use]
    pub fn area(self, class: Class) -> Option<&'static str> {
        match class {
            Class::Green => Some(self.pick("#d3e5c2", "#22301f")),
            Class::Water => Some(self.pick("#a9cfe0", "#1c2c44")),
            Class::Building => Some(self.pick("#ddd5ca", "#2c2e36")),
            _ => None,
        }
    }

    /// The stroke of a line class at `zoom`, or `None` for an area class. Map lines get wider
    /// as the view zooms in; routes keep one width, so that they stay readable at each zoom.
    #[must_use]
    pub fn line(self, class: Class, zoom: f64) -> Option<Line> {
        let (color, base) = match class {
            Class::Boundary => (self.pick("#a77aa8", "#b98bc0"), 1.0),
            Class::Waterway => (self.pick("#7eb3d3", "#3a6ba3"), 1.2),
            Class::Rail => (self.pick("#9a979c", "#5d5a62"), 1.0),
            Class::Path => (self.pick("#c19a7a", "#7b6450"), 0.8),
            Class::MinorRoad => (self.pick("#ffffff", "#3c4049"), 1.5),
            Class::MajorRoad => (self.pick("#f6d488", "#8a7440"), 2.5),
            Class::Highway => (self.pick("#f2a066", "#a15e30"), 3.0),
            Class::Alternative => return Some(self.route_line(false)),
            Class::Route => return Some(self.route_line(true)),
            Class::Green | Class::Water | Class::Building => return None,
        };
        let width = base * 2_f64.powf((zoom - BASE_ZOOM) / 2.0);
        Some(Line {
            color,
            width: width.clamp(0.5, base * 4.0),
        })
    }

    #[must_use]
    pub fn route_line(self, selected: bool) -> Line {
        if selected {
            Line {
                color: self.pick("#2f6fde", "#5b9bff"),
                width: 5.0,
            }
        } else {
            Line {
                color: self.pick("#8ea0bd", "#6b7a94"),
                width: 4.0,
            }
        }
    }

    #[must_use]
    pub fn text(self, class: LabelClass) -> Text {
        let ink = self.pick("#3a3a3a", "#d6d6d6");
        let quiet = self.pick("#5f5f5f", "#a9a9a9");
        let (font, size, color) = match class {
            LabelClass::Country => ("600 13px system-ui, sans-serif", 13, ink),
            LabelClass::City => ("600 15px system-ui, sans-serif", 15, ink),
            LabelClass::State => ("italic 12px system-ui, sans-serif", 12, quiet),
            LabelClass::Town => ("500 13px system-ui, sans-serif", 13, ink),
            LabelClass::Village | LabelClass::Suburb => ("12px system-ui, sans-serif", 12, quiet),
            LabelClass::Water => (
                "italic 12px system-ui, sans-serif",
                12,
                self.pick("#3b6ea5", "#7fa8d6"),
            ),
            LabelClass::Road => ("11px system-ui, sans-serif", 11, quiet),
        };
        Text { font, size, color }
    }

    /// The outline around label text, so that it stays readable over lines.
    #[must_use]
    pub fn halo(self) -> &'static str {
        self.background()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_class_is_an_area_or_a_line() {
        for dark in [false, true] {
            let palette = Palette::new(dark);
            for class in Class::ALL {
                assert_eq!(
                    palette.area(class).is_some(),
                    palette.line(class, 14.0).is_none(),
                    "{class:?}"
                );
                assert_eq!(palette.area(class).is_some(), class.is_area(), "{class:?}");
            }
        }
    }

    #[test]
    fn map_lines_get_wider_with_the_zoom_but_routes_do_not() {
        let palette = Palette::new(false);
        let width = |class, zoom| palette.line(class, zoom).unwrap().width;
        assert!(width(Class::MajorRoad, 16.0) > width(Class::MajorRoad, 14.0));
        assert_eq!(width(Class::MajorRoad, 14.0), 2.5);
        assert_eq!(width(Class::Route, 10.0), width(Class::Route, 18.0));
    }
}
