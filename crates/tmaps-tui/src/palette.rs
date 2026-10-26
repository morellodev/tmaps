use std::env;
use std::ffi::OsStr;

use ratatui::style::{Color, Modifier, Style};
use terminal_colorsaurus::{QueryOptions, ThemeMode};
use tmaps_core::config::Theme;
use tmaps_core::render::{Class, LabelClass};

/// The colors of the map for one theme and color depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    depth: Depth,
    shade: Shade,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Rgb,
    /// The 256-color palette, for terminals without true color.
    Indexed,
    /// No colors, see <https://no-color.org>.
    Mono,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shade {
    Dark,
    Light,
}

/// A true color and its nearest color in the 256-color palette.
type Spec = ([u8; 3], u8);

impl Palette {
    pub fn new(depth: Depth, shade: Shade) -> Self {
        Self { depth, shade }
    }

    /// Reads the color depth from the environment. With [`Theme::Auto`], asks the terminal for
    /// its background color. Call it before the event reader starts, because the reader would
    /// take the answer of the terminal.
    pub fn detect(theme: Theme) -> Self {
        let depth = Depth::from_env(
            env::var_os("NO_COLOR").as_deref(),
            env::var_os("COLORTERM").as_deref(),
        );
        let shade = Shade::choose(theme, depth, || {
            match terminal_colorsaurus::theme_mode(QueryOptions::default()) {
                Ok(ThemeMode::Light) => Shade::Light,
                Ok(ThemeMode::Dark) | Err(_) => Shade::Dark,
            }
        });
        Self::new(depth, shade)
    }

    pub fn is_mono(self) -> bool {
        self.depth == Depth::Mono
    }

    pub fn background(self) -> Color {
        self.color(([22, 24, 28], 234), ([242, 239, 233], 255))
    }

    pub fn status_background(self) -> Color {
        self.color(([44, 46, 52], 236), ([218, 215, 208], 252))
    }

    pub fn text(self) -> Color {
        self.color(([200, 200, 200], 251), ([40, 40, 40], 236))
    }

    pub fn crosshair(self) -> Color {
        self.color(([255, 80, 80], 203), ([210, 30, 30], 160))
    }

    pub fn area(self, class: Class) -> Color {
        match class {
            Class::Green => self.color(([28, 48, 32], 22), ([205, 230, 190], 194)),
            Class::Water => self.color(([22, 40, 64], 17), ([170, 205, 225], 152)),
            Class::Building => self.color(([46, 46, 52], 237), ([222, 214, 206], 188)),
            _ => self.background(),
        }
    }

    pub fn line(self, class: Class) -> Color {
        match class {
            Class::Boundary => self.color(([150, 110, 160], 96), ([140, 90, 150], 96)),
            Class::Waterway => self.color(([70, 120, 190], 68), ([70, 130, 200], 68)),
            Class::Rail => self.color(([120, 120, 120], 244), ([110, 110, 110], 242)),
            Class::Path => self.color(([100, 100, 90], 241), ([160, 150, 135], 247)),
            Class::MajorRoad => self.color(([220, 190, 110], 179), ([215, 140, 40], 172)),
            Class::Highway => self.color(([240, 140, 70], 208), ([215, 90, 50], 166)),
            Class::MinorRoad | Class::Green | Class::Water | Class::Building => {
                self.color(([150, 150, 150], 248), ([140, 140, 140], 245))
            }
        }
    }

    /// Labels keep the background of the cells under them.
    pub fn label(self, class: LabelClass) -> Style {
        let (color, modifier) = match class {
            LabelClass::Country => (
                self.color(([235, 235, 235], 255), ([30, 30, 30], 235)),
                Modifier::BOLD,
            ),
            LabelClass::City => (
                self.color(([245, 245, 245], 255), ([20, 20, 20], 233)),
                Modifier::BOLD,
            ),
            LabelClass::State => (
                self.color(([170, 170, 180], 249), ([100, 100, 110], 242)),
                Modifier::empty(),
            ),
            LabelClass::Town => (
                self.color(([220, 220, 220], 253), ([40, 40, 40], 236)),
                Modifier::empty(),
            ),
            LabelClass::Village | LabelClass::Suburb => (
                self.color(([185, 185, 185], 250), ([80, 80, 80], 239)),
                Modifier::empty(),
            ),
            LabelClass::Water => (
                self.color(([120, 170, 230], 111), ([40, 90, 170], 25)),
                Modifier::ITALIC,
            ),
            LabelClass::Road => (
                self.color(([210, 210, 200], 252), ([60, 60, 60], 238)),
                Modifier::empty(),
            ),
        };
        Style::new().fg(color).add_modifier(modifier)
    }

    fn color(self, dark: Spec, light: Spec) -> Color {
        let ([r, g, b], index) = match self.shade {
            Shade::Dark => dark,
            Shade::Light => light,
        };
        match self.depth {
            Depth::Rgb => Color::Rgb(r, g, b),
            Depth::Indexed => Color::Indexed(index),
            Depth::Mono => Color::Reset,
        }
    }
}

impl Shade {
    /// Calls `query_terminal` only for [`Theme::Auto`] with colors.
    fn choose(theme: Theme, depth: Depth, query_terminal: impl FnOnce() -> Self) -> Self {
        match theme {
            Theme::Dark => Self::Dark,
            Theme::Light => Self::Light,
            Theme::Auto if depth == Depth::Mono => Self::Dark,
            Theme::Auto => query_terminal(),
        }
    }
}

impl Depth {
    fn from_env(no_color: Option<&OsStr>, colorterm: Option<&OsStr>) -> Self {
        if no_color.is_some_and(|value| !value.is_empty()) {
            Self::Mono
        } else if colorterm.is_some_and(|value| value == "truecolor" || value == "24bit") {
            Self::Rgb
        } else {
            Self::Indexed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_color_wins_over_colorterm() {
        let depth = Depth::from_env(Some(OsStr::new("1")), Some(OsStr::new("truecolor")));
        assert_eq!(depth, Depth::Mono);
    }

    #[test]
    fn empty_no_color_is_ignored() {
        let depth = Depth::from_env(Some(OsStr::new("")), Some(OsStr::new("24bit")));
        assert_eq!(depth, Depth::Rgb);
    }

    #[test]
    fn terminal_without_truecolor_gets_the_indexed_palette() {
        assert_eq!(Depth::from_env(None, None), Depth::Indexed);
    }

    fn no_query() -> Shade {
        panic!("the terminal must not get a query")
    }

    #[test]
    fn explicit_theme_does_not_query_the_terminal() {
        assert_eq!(
            Shade::choose(Theme::Light, Depth::Rgb, no_query),
            Shade::Light
        );
    }

    #[test]
    fn auto_theme_without_colors_does_not_query_the_terminal() {
        assert_eq!(
            Shade::choose(Theme::Auto, Depth::Mono, no_query),
            Shade::Dark
        );
    }

    #[test]
    fn auto_theme_uses_the_answer_of_the_terminal() {
        let shade = Shade::choose(Theme::Auto, Depth::Indexed, || Shade::Light);
        assert_eq!(shade, Shade::Light);
    }

    #[test]
    fn labels_keep_the_background_of_the_map() {
        let palette = Palette::new(Depth::Rgb, Shade::Light);
        assert_eq!(palette.label(LabelClass::Water).bg, None);
    }

    #[test]
    fn light_theme_has_a_light_background() {
        let palette = Palette::new(Depth::Rgb, Shade::Light);
        assert_eq!(palette.background(), Color::Rgb(242, 239, 233));
    }

    #[test]
    fn mono_palette_uses_the_terminal_colors() {
        let palette = Palette::new(Depth::Mono, Shade::Light);
        assert_eq!(palette.line(Class::Highway), Color::Reset);
    }
}
