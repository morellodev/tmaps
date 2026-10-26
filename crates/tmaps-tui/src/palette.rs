use std::env;
use std::ffi::OsString;

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
        let depth = Depth::from_env(|name| env::var_os(name));
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
        self.color(([44, 46, 52], 236), ([218, 215, 208], 253))
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
            Class::Path => self.color(([115, 115, 105], 243), ([130, 120, 105], 101)),
            Class::MajorRoad => self.color(([220, 190, 110], 179), ([185, 110, 20], 130)),
            Class::Highway => self.color(([240, 140, 70], 208), ([200, 70, 35], 166)),
            Class::Route => self.route(),
            Class::Alternative => self.color(([80, 110, 140], 66), ([105, 135, 175], 67)),
            Class::MinorRoad | Class::Green | Class::Water | Class::Building => {
                self.color(([150, 150, 150], 248), ([120, 120, 120], 243))
            }
        }
    }

    /// The strong accent of the selected route, see [`Class::Route`].
    pub fn route(self) -> Color {
        self.color(([60, 190, 255], 39), ([0, 80, 190], 25))
    }

    /// The start and end points: reversed, so that they show also without colors.
    pub fn marker(self) -> Style {
        self.selection().add_modifier(Modifier::BOLD)
    }

    /// The selected row of a list. Reversed, so that it shows also without colors.
    pub fn selection(self) -> Style {
        Style::new()
            .fg(self.route())
            .add_modifier(Modifier::REVERSED)
    }

    /// Less important text, such as key descriptions and borders. At least 4.5:1 against the
    /// status line.
    pub fn muted(self) -> Color {
        self.color(([155, 155, 165], 247), ([90, 90, 90], 240))
    }

    /// Bold, so that an error shows also without colors.
    pub fn error(self) -> Style {
        Style::new()
            .fg(self.color(([255, 110, 110], 203), ([180, 30, 30], 124)))
            .add_modifier(Modifier::BOLD)
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
    /// `FORCE_COLOR` wins over `NO_COLOR` and `TERM=dumb`, see <https://force-color.org>.
    fn from_env(var: impl Fn(&str) -> Option<OsString>) -> Self {
        let set = |name| var(name).is_some_and(|value| !value.is_empty());
        let dumb = var("TERM").is_some_and(|value| value == "dumb");
        if !set("FORCE_COLOR") && (set("NO_COLOR") || dumb) {
            Self::Mono
        } else if var("COLORTERM").is_some_and(|value| value == "truecolor" || value == "24bit") {
            Self::Rgb
        } else {
            Self::Indexed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn depth(vars: &[(&str, &str)]) -> Depth {
        Depth::from_env(|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        })
    }

    #[test]
    fn no_color_wins_over_colorterm() {
        let depth = depth(&[("NO_COLOR", "1"), ("COLORTERM", "truecolor")]);
        assert_eq!(depth, Depth::Mono);
    }

    #[test]
    fn empty_no_color_is_ignored() {
        let depth = depth(&[("NO_COLOR", ""), ("COLORTERM", "24bit")]);
        assert_eq!(depth, Depth::Rgb);
    }

    #[test]
    fn terminal_without_truecolor_gets_the_indexed_palette() {
        assert_eq!(depth(&[("TERM", "xterm-256color")]), Depth::Indexed);
    }

    #[test]
    fn dumb_terminal_gets_no_colors() {
        assert_eq!(depth(&[("TERM", "dumb")]), Depth::Mono);
    }

    #[test]
    fn force_color_wins_over_no_color() {
        let depth = depth(&[("NO_COLOR", "1"), ("FORCE_COLOR", "1"), ("TERM", "dumb")]);
        assert_eq!(depth, Depth::Indexed);
    }

    #[test]
    fn force_color_zero_also_forces_colors() {
        let depth = depth(&[("FORCE_COLOR", "0"), ("COLORTERM", "truecolor")]);
        assert_eq!(depth, Depth::Rgb);
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

    /// The WCAG relative luminance of an sRGB color.
    fn luminance(color: Color) -> f64 {
        let Color::Rgb(r, g, b) = color else {
            panic!("not an RGB color: {color:?}")
        };
        let linear = |channel: u8| {
            let c = f64::from(channel) / 255.0;
            if c <= 0.040_45 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
    }

    /// The xterm values of the 256-color palette, from index 16.
    fn rgb(color: Color) -> Color {
        let Color::Indexed(index) = color else {
            return color;
        };
        match index {
            16..=231 => {
                let level = |n: u8| if n == 0 { 0 } else { 55 + 40 * n };
                let n = index - 16;
                Color::Rgb(level(n / 36), level(n / 6 % 6), level(n % 6))
            }
            232..=255 => {
                let gray = 8 + 10 * (index - 232);
                Color::Rgb(gray, gray, gray)
            }
            _ => panic!("the system colors have no fixed value: {index}"),
        }
    }

    fn contrast(a: Color, b: Color) -> f64 {
        let (a, b) = (rgb(a), rgb(b));
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn map_lines_have_3_to_1_contrast_with_the_background() {
        let classes = [
            Class::Boundary,
            Class::Waterway,
            Class::Rail,
            Class::Path,
            Class::MinorRoad,
            Class::MajorRoad,
            Class::Highway,
            Class::Route,
            Class::Alternative,
        ];
        let low: Vec<_> = palettes()
            .flat_map(|palette| {
                classes.into_iter().filter_map(move |class| {
                    let ratio = contrast(palette.line(class), palette.background());
                    (ratio < 3.0).then_some((palette, class, ratio))
                })
            })
            .collect();
        assert_eq!(low, []);
    }

    #[test]
    fn status_line_text_has_4_5_to_1_contrast() {
        let low: Vec<_> = palettes()
            .flat_map(|palette| {
                [palette.muted(), palette.route()]
                    .into_iter()
                    .filter_map(move |color| {
                        let ratio = contrast(color, palette.status_background());
                        (ratio < 4.5).then_some((palette, color, ratio))
                    })
            })
            .collect();
        assert_eq!(low, []);
    }

    fn palettes() -> impl Iterator<Item = Palette> {
        [Depth::Rgb, Depth::Indexed].into_iter().flat_map(|depth| {
            [Shade::Dark, Shade::Light]
                .into_iter()
                .map(move |shade| Palette::new(depth, shade))
        })
    }

    #[test]
    fn mono_palette_uses_the_terminal_colors() {
        let palette = Palette::new(Depth::Mono, Shade::Light);
        assert_eq!(palette.line(Class::Highway), Color::Reset);
    }
}
