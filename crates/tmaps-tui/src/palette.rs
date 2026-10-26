use std::env;
use std::ffi::OsStr;

use ratatui::style::Color;
use tmaps_core::render::Class;

/// A fixed dark palette, so that the map looks the same on light and dark terminal themes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Palette {
    Rgb,
    /// The 256-color palette, for terminals without true color.
    Indexed,
    /// No colors, see <https://no-color.org>.
    Mono,
}

impl Palette {
    pub fn detect() -> Self {
        Self::from_env(
            env::var_os("NO_COLOR").as_deref(),
            env::var_os("COLORTERM").as_deref(),
        )
    }

    fn from_env(no_color: Option<&OsStr>, colorterm: Option<&OsStr>) -> Self {
        if no_color.is_some_and(|value| !value.is_empty()) {
            Self::Mono
        } else if colorterm.is_some_and(|value| value == "truecolor" || value == "24bit") {
            Self::Rgb
        } else {
            Self::Indexed
        }
    }

    pub fn background(self) -> Color {
        self.color([22, 24, 28], 234)
    }

    pub fn status_background(self) -> Color {
        self.color([44, 46, 52], 236)
    }

    pub fn text(self) -> Color {
        self.color([200, 200, 200], 251)
    }

    pub fn crosshair(self) -> Color {
        self.color([255, 80, 80], 203)
    }

    pub fn area(self, class: Class) -> Color {
        match class {
            Class::Green => self.color([28, 48, 32], 22),
            Class::Water => self.color([22, 40, 64], 17),
            Class::Building => self.color([46, 46, 52], 237),
            _ => self.background(),
        }
    }

    pub fn line(self, class: Class) -> Color {
        match class {
            Class::Boundary => self.color([150, 110, 160], 96),
            Class::Waterway => self.color([70, 120, 190], 68),
            Class::Rail => self.color([120, 120, 120], 244),
            Class::Path => self.color([100, 100, 90], 241),
            Class::MajorRoad => self.color([220, 190, 110], 179),
            Class::Highway => self.color([240, 140, 70], 208),
            Class::MinorRoad | Class::Green | Class::Water | Class::Building => {
                self.color([150, 150, 150], 248)
            }
        }
    }

    fn color(self, [r, g, b]: [u8; 3], index: u8) -> Color {
        match self {
            Self::Rgb => Color::Rgb(r, g, b),
            Self::Indexed => Color::Indexed(index),
            Self::Mono => Color::Reset,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_color_wins_over_colorterm() {
        let palette = Palette::from_env(Some(OsStr::new("1")), Some(OsStr::new("truecolor")));
        assert_eq!(palette, Palette::Mono);
    }

    #[test]
    fn empty_no_color_is_ignored() {
        let palette = Palette::from_env(Some(OsStr::new("")), Some(OsStr::new("24bit")));
        assert_eq!(palette, Palette::Rgb);
    }

    #[test]
    fn terminal_without_truecolor_gets_the_indexed_palette() {
        assert_eq!(Palette::from_env(None, None), Palette::Indexed);
    }
}
