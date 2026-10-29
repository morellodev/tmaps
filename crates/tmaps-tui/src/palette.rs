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

/// A true color and a near color in the 256-color palette. The index is not always the nearest
/// one, because it must also keep the contrast and the color-blind separation of the tests.
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
        self.color(([255, 70, 45], 209), ([190, 20, 80], 88))
    }

    pub fn area(self, class: Class) -> Color {
        match class {
            Class::Green => self.color(([30, 50, 26], 22), ([205, 230, 172], 187)),
            Class::Water => self.color(([20, 36, 66], 17), ([192, 220, 245], 153)),
            Class::Building => self.color(([56, 55, 62], 237), ([220, 214, 208], 188)),
            _ => self.background(),
        }
    }

    pub fn line(self, class: Class) -> Color {
        match class {
            Class::Boundary => self.color(([255, 150, 225], 177), ([175, 60, 160], 128)),
            Class::Waterway => self.color(([80, 130, 215], 39), ([15, 110, 195], 25)),
            Class::Rail => self.color(([140, 135, 145], 248), ([70, 55, 60], 237)),
            Class::Path => self.color(([175, 140, 105], 180), ([85, 40, 0], 52)),
            Class::MajorRoad => self.color(([235, 205, 95], 185), ([155, 110, 5], 130)),
            Class::Highway => self.color(([250, 135, 55], 214), ([150, 45, 10], 124)),
            Class::Route => self.route(),
            Class::Alternative => self.color(([125, 150, 195], 110), ([90, 100, 115], 59)),
            Class::MinorRoad | Class::Green | Class::Water | Class::Building => {
                self.color(([175, 175, 170], 250), ([125, 120, 125], 243))
            }
        }
    }

    /// The strong accent of the selected route, see [`Class::Route`].
    pub fn route(self) -> Color {
        self.color(([80, 200, 255], 45), ([0, 60, 170], 19))
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
                self.color(([235, 235, 235], 255), ([30, 30, 30], 234)),
                Modifier::BOLD,
            ),
            LabelClass::City => (
                self.color(([245, 245, 245], 255), ([20, 20, 20], 233)),
                Modifier::BOLD,
            ),
            LabelClass::State => (
                self.color(([190, 190, 200], 251), ([80, 80, 90], 239)),
                Modifier::empty(),
            ),
            LabelClass::Town => (
                self.color(([220, 220, 220], 253), ([40, 40, 40], 235)),
                Modifier::empty(),
            ),
            LabelClass::Village | LabelClass::Suburb => (
                self.color(([200, 200, 200], 251), ([65, 65, 65], 238)),
                Modifier::empty(),
            ),
            LabelClass::Water => (
                self.color(([135, 185, 245], 117), ([25, 75, 150], 24)),
                Modifier::ITALIC,
            ),
            LabelClass::Road => (
                self.color(([210, 210, 200], 252), ([60, 60, 60], 237)),
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
        let [r, g, b] = linear_rgb(color);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    fn linear_rgb(color: Color) -> [f64; 3] {
        let Color::Rgb(r, g, b) = rgb(color) else {
            panic!("not an RGB color: {color:?}")
        };
        [r, g, b].map(|channel| {
            let c = f64::from(channel) / 255.0;
            if c <= 0.040_45 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        })
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
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// The crosshair and the lines, which show on top of the fills.
    fn marks(palette: Palette) -> Vec<Color> {
        let lines = Class::ALL.into_iter().filter(|class| !class.is_area());
        let lines = lines.map(|class| palette.line(class));
        [palette.crosshair()].into_iter().chain(lines).collect()
    }

    fn labels(palette: Palette) -> Vec<Color> {
        [
            LabelClass::Country,
            LabelClass::State,
            LabelClass::City,
            LabelClass::Town,
            LabelClass::Village,
            LabelClass::Suburb,
            LabelClass::Water,
            LabelClass::Road,
        ]
        .into_iter()
        .filter_map(|class| palette.label(class).fg)
        .collect()
    }

    /// Returns the colors with less contrast than `minimum` on the background or on an area.
    fn low_contrast(
        colors: fn(Palette) -> Vec<Color>,
        minimum: f64,
    ) -> Vec<(Palette, Color, Color, f64)> {
        let mut low = Vec::new();
        for palette in palettes() {
            let areas =
                [Class::Green, Class::Water, Class::Building].map(|class| palette.area(class));
            for color in colors(palette) {
                for fill in [palette.background()].into_iter().chain(areas) {
                    let ratio = contrast(color, fill);
                    if ratio < minimum {
                        low.push((palette, color, fill, ratio));
                    }
                }
            }
        }
        low
    }

    /// WCAG 2.2 SC 1.4.11 asks 3:1 for graphical objects.
    #[test]
    fn map_lines_have_3_to_1_contrast_with_the_background_and_the_areas() {
        assert_eq!(low_contrast(marks, 3.0), []);
    }

    /// WCAG 2.2 SC 1.4.3 asks 4.5:1 for text. A terminal has no large text.
    #[test]
    fn labels_have_4_5_to_1_contrast_with_the_background_and_the_areas() {
        assert_eq!(low_contrast(labels, 4.5), []);
    }

    /// Normal vision, then protanopia, deuteranopia and tritanopia in the model of Machado,
    /// Oliveira and Fernandes (2009) at severity 1, for linear sRGB:
    /// <https://www.inf.ufrgs.br/~oliveira/pubs_files/CVD_Simulation/CVD_Simulation.html>.
    const VISIONS: [[[f64; 3]; 3]; 4] = [
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        [
            [0.152_286, 1.052_583, -0.204_868],
            [0.114_503, 0.786_281, 0.099_216],
            [-0.003_882, -0.048_116, 1.051_998],
        ],
        [
            [0.367_322, 0.860_646, -0.227_968],
            [0.280_085, 0.672_501, 0.047_413],
            [-0.011_820, 0.042_940, 0.968_881],
        ],
        [
            [1.255_528, -0.076_749, -0.178_779],
            [-0.078_411, 0.930_809, 0.147_602],
            [0.004_733, 0.691_367, 0.303_900],
        ],
    ];

    fn multiply(matrix: &[[f64; 3]; 3], vector: [f64; 3]) -> [f64; 3] {
        matrix.map(|row| row.iter().zip(vector).map(|(m, v)| m * v).sum())
    }

    /// Oklab, see <https://bottosson.github.io/posts/oklab/>.
    fn oklab(linear_rgb: [f64; 3]) -> [f64; 3] {
        const TO_LMS: [[f64; 3]; 3] = [
            [0.412_221_470_8, 0.536_332_536_3, 0.051_445_992_9],
            [0.211_903_498_2, 0.680_699_545_1, 0.107_396_956_6],
            [0.088_302_461_9, 0.281_718_837_6, 0.629_978_700_5],
        ];
        const TO_LAB: [[f64; 3]; 3] = [
            [0.210_454_255_3, 0.793_617_785_0, -0.004_072_046_8],
            [1.977_998_495_1, -2.428_592_205_0, 0.450_593_709_9],
            [0.025_904_037_1, 0.782_771_766_2, -0.808_675_766_0],
        ];
        let lms = multiply(&TO_LMS, linear_rgb.map(|c| c.clamp(0.0, 1.0)));
        multiply(&TO_LAB, lms.map(f64::cbrt))
    }

    /// The smallest Oklab distance between two colors over all [`VISIONS`].
    fn smallest_difference(a: Color, b: Color) -> f64 {
        let (a, b) = (linear_rgb(a), linear_rgb(b));
        VISIONS
            .iter()
            .map(|vision| {
                let (a, b) = (oklab(multiply(vision, a)), oklab(multiply(vision, b)));
                a.iter()
                    .zip(b)
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f64>()
                    .sqrt()
            })
            .fold(f64::INFINITY, f64::min)
    }

    /// About two times the Oklab difference that CSS Color 4 uses as just noticeable (0.02),
    /// because braille dots are thin. The 256-color cube permits no larger value in the dark theme.
    const MIN_DIFFERENCE: f64 = 0.045;

    #[test]
    fn map_lines_stay_apart_with_color_blindness() {
        let mut close = Vec::new();
        for palette in palettes() {
            let marks = marks(palette);
            for (i, &a) in marks.iter().enumerate() {
                for &b in &marks[i + 1..] {
                    let difference = smallest_difference(a, b);
                    if difference < MIN_DIFFERENCE {
                        close.push((palette, a, b, difference));
                    }
                }
            }
        }
        assert_eq!(close, []);
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
