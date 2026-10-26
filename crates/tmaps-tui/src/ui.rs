use std::fmt::Write;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols::line;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Clear, HighlightSpacing, List, ListItem, ListState, Padding, Paragraph,
    Scrollbar, ScrollbarOrientation, ScrollbarState, Widget,
};
use textwrap::core::display_width;
use tmaps_core::LatLon;
use tmaps_core::config::Units;
use tmaps_core::format::{distance, duration};
use tmaps_core::geocode::Place;
use tmaps_core::render::{Canvas, Class, DOTS_PER_CELL_X, DOTS_PER_CELL_Y, View, cursor_cell};
use tmaps_core::routing::{Maneuver, Mode, Route, Step};

use crate::app::{App, Level};
use crate::palette::Palette;
use crate::prompt::Prompt;
use crate::trip::{Routes, Trip, Waypoint};

const STATUS_HEIGHT: u16 = 1;
const PANEL_WIDTH: u16 = 40;
const DRAWER_MIN_HEIGHT: u16 = 6;
const DRAWER_MAX_HEIGHT: u16 = 12;
/// A drawer that leaves the map fewer rows hides, and the status line shows the route.
const MAP_MIN_HEIGHT: u16 = 8;
/// The inner width of the side panel, inside the borders and the padding.
const SUMMARY_WIDTH: u16 = PANEL_WIDTH - 4;
/// Narrower terminals get the panel as a drawer below the map, so the map keeps its width.
const SIDE_PANEL_MIN_WIDTH: u16 = 100;
const PROMPT_WIDTH: u16 = 60;
/// A smaller terminal gets only a request for more room.
const MIN_WIDTH: u16 = 30;
const MIN_HEIGHT: u16 = 8;
/// A detail that leaves less room for its label goes away.
const LABEL_MIN_WIDTH: usize = 12;
/// Shows water without colors, where it would otherwise look like land.
const MONO_WATER: char = '⠄';
/// Braille, as the map.
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const SELECTED: &str = "▶ ";
/// A key and what it does.
type Hint = (&'static str, &'static str);
type HelpGroup = (&'static str, &'static [Hint]);
const HELP: [HelpGroup; 4] = [
    (
        "Map",
        &[
            ("←↓↑→  hjkl", "pan"),
            ("Shift + pan", "pan faster"),
            ("+ -  wheel", "zoom"),
            ("drag", "pan"),
        ],
    ),
    (
        "Places",
        &[
            ("/", "search"),
            ("m", "save the place at +"),
            ("'", "go to a saved place"),
        ],
    ),
    (
        "Route",
        &[
            ("s  e", "start or end at +"),
            ("x  c", "swap or clear the points"),
            ("1 2 3", "car, bike, foot"),
            ("Tab", "next route"),
            ("]  [", "next or previous step"),
            ("p", "show or hide the panel"),
        ],
    ),
    (
        "App",
        &[("?  Esc", "show or hide this help"), ("q  Ctrl-C", "quit")],
    ),
];

struct Areas {
    map: Rect,
    panel: Option<Rect>,
    status: Rect,
}

fn areas(area: Rect, panel: bool) -> Areas {
    let [main, status] = area.layout(&Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(STATUS_HEIGHT),
    ]));
    let split = if !panel {
        None
    } else if area.width >= SIDE_PANEL_MIN_WIDTH {
        Some(main.layout(&Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(PANEL_WIDTH),
        ])))
    } else {
        let drawer = (main.height / 2).clamp(DRAWER_MIN_HEIGHT, DRAWER_MAX_HEIGHT);
        (main.height >= drawer + MAP_MIN_HEIGHT).then(|| {
            main.layout(&Layout::vertical([
                Constraint::Fill(1),
                Constraint::Length(drawer),
            ]))
        })
    };
    match split {
        Some([map, panel]) => Areas {
            map,
            panel: Some(panel),
            status,
        },
        None => Areas {
            map: main,
            panel: None,
            status,
        },
    }
}

/// The help covers all but the status line, so the keys of the help stay visible.
fn help_area(area: Rect) -> Rect {
    Rect {
        height: area.height.saturating_sub(STATUS_HEIGHT),
        ..area
    }
}

/// The size in cells of the map on a terminal of `width` × `height` cells.
pub fn map_size(width: u16, height: u16, panel: bool) -> (u16, u16) {
    let map = areas(Rect::new(0, 0, width, height), panel).map;
    (map.width, map.height)
}

pub fn draw(frame: &mut Frame, app: &App, palette: Palette, units: Units) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        draw_too_small(frame, area);
        return;
    }
    let areas = areas(area, app.panel_visible());
    let trip = app.trip();
    let markers = [(trip.start(), 'A'), (trip.end(), 'B')]
        .into_iter()
        .filter_map(|(waypoint, symbol)| {
            let (column, row) = cell_of(app.view(), waypoint?.location, areas.map)?;
            Some(Marker {
                column,
                row,
                symbol,
            })
        })
        .collect();
    frame.render_widget(
        MapWidget {
            canvas: &app.canvas(),
            palette,
            markers,
            dim: app.prompt().is_some() || app.help(),
        },
        areas.map,
    );
    let spinner = spinner(app);
    if let Some(panel) = areas.panel {
        draw_panel(frame, trip, palette, units, spinner, panel);
    }
    let status = Status {
        palette,
        units,
        panel_shown: areas.panel.is_some(),
    };
    draw_status(frame, app, status, areas.status);
    if let Some(prompt) = app.prompt() {
        draw_prompt(
            frame,
            &prompt_view(prompt, app, palette, units, spinner),
            palette,
            areas.map,
        );
    }
    if app.help() {
        draw_help(frame, app.help_scroll(), palette, help_area(area));
    }
}

fn draw_too_small(frame: &mut Frame, area: Rect) {
    let text = format!(
        "Terminal too small. Make it at least {MIN_WIDTH}×{MIN_HEIGHT}. Now: {}×{}.",
        area.width, area.height
    );
    let lines: Vec<Line> = textwrap::wrap(&text, usize::from(area.width.max(1)))
        .into_iter()
        .map(|line| Line::from(line.into_owned()).centered())
        .collect();
    let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    frame.render_widget(
        Paragraph::new(lines),
        area.centered_vertically(Constraint::Length(height)),
    );
}

/// The cell of `point` on a map in `area`, relative to the area.
fn cell_of(view: View, point: LatLon, area: Rect) -> Option<(u16, u16)> {
    let (x, y) = view.screen_position(
        point,
        f64::from(area.width) * f64::from(DOTS_PER_CELL_X),
        f64::from(area.height) * f64::from(DOTS_PER_CELL_Y),
    );
    let index = |dots: f64, per_cell: u16, cells: u16| {
        let index = (dots / f64::from(per_cell)).floor();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "checked against the area"
        )]
        (0.0..f64::from(cells))
            .contains(&index)
            .then_some(index as u16)
    };
    Some((
        index(x, DOTS_PER_CELL_X, area.width)?,
        index(y, DOTS_PER_CELL_Y, area.height)?,
    ))
}

struct Marker {
    column: u16,
    row: u16,
    symbol: char,
}

struct MapWidget<'a> {
    canvas: &'a Canvas,
    palette: Palette,
    markers: Vec<Marker>,
    /// An open box takes the keys, so the map steps back.
    dim: bool,
}

impl Widget for MapWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        for (row, cells) in (area.y..area.bottom()).zip(self.canvas.rows()) {
            for (column, cell) in (area.x..area.right()).zip(cells) {
                let Some(target) = buf.cell_mut((column, row)) else {
                    continue;
                };
                let symbol = match (cell.dots, cell.bg) {
                    (0, Some(Class::Water)) if self.palette.is_mono() => MONO_WATER,
                    _ => cell.symbol(),
                };
                target
                    .set_char(symbol)
                    .set_fg(
                        cell.fg
                            .map_or(self.palette.text(), |class| self.palette.line(class)),
                    )
                    .set_bg(
                        cell.bg
                            .map_or(self.palette.background(), |class| self.palette.area(class)),
                    );
                if cell.fg == Some(Class::Route) {
                    target.set_style(Style::new().add_modifier(Modifier::BOLD));
                }
            }
        }
        for label in self.canvas.labels() {
            let (column, row) = (area.x + label.column, area.y + label.row);
            if column < area.right() && row < area.bottom() {
                buf.set_stringn(
                    column,
                    row,
                    &label.text,
                    usize::from(area.right() - column),
                    self.palette.label(label.class),
                );
            }
        }
        if self.dim {
            buf.set_style(area, Style::new().add_modifier(Modifier::DIM));
        }
        let (column, row) = cursor_cell(area.width, area.height);
        if let Some(target) = buf.cell_mut((area.x + column, area.y + row)) {
            target.set_char('+').set_style(
                Style::new()
                    .fg(self.palette.crosshair())
                    .add_modifier(Modifier::BOLD)
                    .remove_modifier(Modifier::DIM),
            );
        }
        // After the crosshair: a point set at the crosshair must show at once.
        for marker in &self.markers {
            if let Some(target) = buf.cell_mut((area.x + marker.column, area.y + marker.row)) {
                target
                    .set_char(marker.symbol)
                    .set_style(self.palette.marker().remove_modifier(Modifier::DIM));
            }
        }
    }
}

fn draw_panel(
    frame: &mut Frame,
    trip: &Trip,
    palette: Palette,
    units: Units,
    spinner: char,
    area: Rect,
) {
    let mut block = Block::bordered()
        .title(" Route ")
        .title_style(Style::new().add_modifier(Modifier::BOLD))
        .border_style(Style::new().fg(palette.muted()))
        .padding(Padding::horizontal(1))
        .style(Style::new().fg(palette.text()).bg(palette.background()));
    if let Some((route, step)) = trip.selected() {
        let steps = route.steps.len();
        let position = match step {
            Some(step) => format!(" step {}/{steps} ", step + 1),
            None => format!(" {} ", count(steps, "step")),
        };
        block = block.title_bottom(Line::from(position).right_aligned());
    }
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    // A drawer is wide and short, so the steps go next to the summary.
    let side_by_side = inner.width >= 2 * SUMMARY_WIDTH;
    let width = if side_by_side {
        SUMMARY_WIDTH
    } else {
        inner.width
    };
    let mut summary = vec![mode_line(trip.mode(), palette), Line::default()];
    summary.extend(waypoint_lines(
        'A',
        trip.start(),
        "s: start at +",
        palette,
        width,
    ));
    summary.extend(waypoint_lines(
        'B',
        trip.end(),
        "e: end at +",
        palette,
        width,
    ));
    summary.push(Line::default());
    match trip.routes() {
        Routes::None => {}
        Routes::Loading => summary.push(Line::from(format!("{spinner} Searching for routes…"))),
        Routes::Failed(error) => summary.extend(
            hanging("✗ ", error, width)
                .into_iter()
                .map(|line| Line::styled(line, palette.error())),
        ),
        Routes::Ready {
            routes, selected, ..
        } => summary.extend(routes.iter().enumerate().flat_map(|(index, route)| {
            let style = if index == *selected {
                Style::new()
                    .fg(palette.route())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            summary_lines(index, route, index == *selected, units, width)
                .into_iter()
                .map(move |line| Line::styled(line, style))
        })),
    }
    let [summary_area, steps_area] = if side_by_side {
        inner.layout(&Layout::horizontal([
            Constraint::Length(SUMMARY_WIDTH),
            Constraint::Fill(1),
        ]))
    } else {
        let height = u16::try_from(summary.len() + 1).unwrap_or(u16::MAX);
        inner.layout(&Layout::vertical([
            Constraint::Length(height),
            Constraint::Fill(1),
        ]))
    };
    frame.render_widget(Paragraph::new(summary), summary_area);
    if let Some((route, step)) = trip.selected() {
        let width = steps_area.width.saturating_sub(selected_width());
        let steps: Vec<ListItem> = route
            .steps
            .iter()
            .map(|step| step_item(step, units, (width, steps_area.height), palette))
            .collect();
        let heights: Vec<usize> = steps.iter().map(ListItem::height).collect();
        let mut state = ListState::default().with_selected(step);
        frame.render_stateful_widget(
            selectable(steps, palette).scroll_padding(1),
            steps_area,
            &mut state,
        );
        let top = heights[..state.offset().min(heights.len())].iter().sum();
        draw_scrollbar(
            frame,
            palette,
            Style::new().fg(palette.muted()),
            area,
            steps_area,
            heights.iter().sum(),
            top,
        );
    }
}

/// "1 step", "2 steps".
fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// A scrollbar on the right border of `block`, next to `rows`, when `content` rows do not fit.
/// `top` is the first visible row.
fn draw_scrollbar(
    frame: &mut Frame,
    palette: Palette,
    track: Style,
    block: Rect,
    rows: Rect,
    content: usize,
    top: usize,
) {
    let viewport = usize::from(rows.height);
    if content <= viewport || block.width == 0 {
        return;
    }
    let area = Rect {
        x: block.right() - 1,
        width: 1,
        ..rows
    };
    let mut state = ScrollbarState::new(content - viewport + 1).position(top);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some(line::VERTICAL))
            .track_style(track)
            .thumb_style(Style::new().fg(palette.text())),
        area,
        &mut state,
    );
}

/// A list with the selected row marked by [`SELECTED`]. The mark has its column also when no
/// row is selected, so the rows do not move when a selection starts.
fn selectable<'a>(
    items: impl IntoIterator<Item = impl Into<ListItem<'a>>>,
    palette: Palette,
) -> List<'a> {
    List::new(items)
        .highlight_symbol(SELECTED)
        .highlight_spacing(HighlightSpacing::Always)
        .highlight_style(palette.selection())
}

fn selected_width() -> u16 {
    u16::try_from(Line::from(SELECTED).width()).unwrap_or(u16::MAX)
}

fn mode_line(mode: Mode, palette: Palette) -> Line<'static> {
    let spans = [
        (Mode::Car, "1 car"),
        (Mode::Bike, "2 bike"),
        (Mode::Foot, "3 foot"),
    ]
    .into_iter()
    .flat_map(|(option, text)| {
        let style = if option == mode {
            palette.selection().add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        [Span::styled(format!(" {text} "), style), Span::raw(" ")]
    });
    Line::from_iter(spans)
}

fn waypoint_lines(
    symbol: char,
    waypoint: Option<&Waypoint>,
    hint: &'static str,
    palette: Palette,
    width: u16,
) -> Vec<Line<'static>> {
    let text = match waypoint {
        Some(Waypoint {
            name: Some(name), ..
        }) => name.clone(),
        Some(waypoint) => coordinates(waypoint.location),
        None => hint.to_owned(),
    };
    let mut lines: Vec<Line> = hanging(&format!("{symbol} "), &text, width)
        .into_iter()
        .map(Line::from)
        .collect();
    if let Some(first) = lines.first_mut() {
        let rest = first.to_string().split_off(symbol.len_utf8());
        *first = Line::from_iter([
            Span::styled(symbol.to_string(), palette.marker()),
            Span::raw(rest),
        ]);
    }
    lines
}

/// 5 decimals, about 1 m, as in the status line.
fn coordinates(location: LatLon) -> String {
    format!("{:.5}, {:.5}", location.lat(), location.lon())
}

fn summary_lines(
    index: usize,
    route: &Route,
    selected: bool,
    units: Units,
    width: u16,
) -> Vec<String> {
    let prefix = format!("{} {}. ", if selected { '▶' } else { ' ' }, index + 1);
    let mut text = format!(
        "{} · {}",
        duration(route.duration_s),
        distance(route.distance_m, units)
    );
    for (present, flag) in [
        (route.has_toll, "tolls"),
        (route.has_highway, "highways"),
        (route.has_ferry, "ferries"),
    ] {
        if present {
            let _ = write!(text, " · {flag}");
        }
    }
    hanging(&prefix, &text, width)
}

/// The distance first, in a muted column, then the arrow and the instruction. A step has at
/// most `rows` rows: the list cannot show a step that is higher than the list.
fn step_item(
    step: &Step,
    units: Units,
    (width, rows): (u16, u16),
    palette: Palette,
) -> ListItem<'static> {
    let distance = if step.distance_m > 0.0 {
        distance(step.distance_m, units)
    } else {
        String::new()
    };
    let column = format!("{distance:>7}  ");
    let prefix = format!("{column}{} ", arrow(step.maneuver));
    let mut wrapped = hanging(&prefix, &step.instruction, width);
    let rows = usize::from(rows).max(1);
    if wrapped.len() > rows {
        wrapped.truncate(rows);
        if let Some(last) = wrapped.last_mut() {
            *last = cut(&format!("{last}…"), usize::from(width));
        }
    }
    let lines = wrapped.into_iter().enumerate().map(|(index, mut line)| {
        if index > 0 {
            return Line::from(line);
        }
        let instruction = line.split_off(line.floor_char_boundary(column.len()));
        Line::from_iter([
            Span::styled(line, Style::new().fg(palette.muted())),
            Span::raw(instruction),
        ])
    });
    ListItem::new(Text::from_iter(lines))
}

fn arrow(maneuver: Maneuver) -> char {
    match maneuver {
        Maneuver::Depart
        | Maneuver::Continue
        | Maneuver::RampStraight
        | Maneuver::KeepStraight
        | Maneuver::Merge => '↑',
        Maneuver::TurnSlightRight
        | Maneuver::KeepRight
        | Maneuver::RampRight
        | Maneuver::ExitRight
        | Maneuver::MergeRight => '↗',
        Maneuver::TurnRight => '→',
        Maneuver::TurnSharpRight => '↘',
        Maneuver::TurnSlightLeft
        | Maneuver::KeepLeft
        | Maneuver::RampLeft
        | Maneuver::ExitLeft
        | Maneuver::MergeLeft => '↖',
        Maneuver::TurnLeft => '←',
        Maneuver::TurnSharpLeft => '↙',
        Maneuver::UturnRight | Maneuver::UturnLeft => '↩',
        Maneuver::RoundaboutEnter | Maneuver::RoundaboutExit => '↻',
        Maneuver::FerryEnter | Maneuver::FerryExit => '~',
        Maneuver::Arrive => '◉',
        Maneuver::Other => ' ',
    }
}

/// Wraps `text` after `prefix` to `width` columns. The next lines start under the text, so the
/// column of the prefix stays clear.
fn hanging(prefix: &str, text: &str, width: u16) -> Vec<String> {
    let indent = Line::from(prefix).width();
    let text_width = usize::from(width).saturating_sub(indent).max(1);
    textwrap::wrap(text, text_width)
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                format!("{prefix}{line}")
            } else {
                format!("{:indent$}{line}", "")
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct Status {
    palette: Palette,
    units: Units,
    /// A hidden panel puts the route in the status line.
    panel_shown: bool,
}

fn draw_status(frame: &mut Frame, app: &App, status: Status, area: Rect) {
    let palette = status.palette;
    let style = Style::new()
        .fg(palette.text())
        .bg(palette.status_background());
    let (left, right) = status_lines(app, status, area.width);
    let [left_area, right_area] = area.layout(&Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(u16::try_from(right.width()).unwrap_or(u16::MAX)),
    ]));
    frame.render_widget(Paragraph::new(left).style(style), left_area);
    frame.render_widget(Paragraph::new(right).style(style), right_area);
}

/// The left part has the notice, the route when the panel is hidden, the work on the way, and
/// the key hints. The right part has the position and the attribution. A narrow line drops
/// whole parts, never a part of a word, in this order: the position, the work and the hints
/// except the last, the long attribution, the short attribution when a route shows, the route,
/// the last hint, the attribution. When the short attribution does not fit alone, the last
/// hint gets the line.
fn status_lines(app: &App, status: Status, width: u16) -> (Line<'static>, Line<'static>) {
    let palette = status.palette;
    let notice = notice_spans(app, palette);
    let route = route_spans(app, status);
    let work = work_spans(app, status);
    let hints = hints(app);
    let last_hint = &hints[hints.len().saturating_sub(1)..];
    let view = app.view();
    let position = format!("{}  z{:.1}", coordinates(view.center()), view.zoom());
    let attribution = app.attribution().unwrap_or_default();
    let short = short_attribution(attribution);
    // With the route, with the work, the hints, the position, the attribution.
    let candidates: [(bool, bool, &[Hint], &str, &str); 9] = [
        (true, true, hints, &position, attribution),
        (true, true, hints, "", attribution),
        (true, false, last_hint, "", attribution),
        (true, false, last_hint, "", &short),
        (true, false, last_hint, "", ""),
        (false, false, last_hint, "", &short),
        (false, false, &[], "", &short),
        (false, false, last_hint, "", ""),
        (false, false, &[], "", ""),
    ];
    let mut fallback = (Line::default(), Line::default());
    for (with_route, with_work, hints, position, attribution) in candidates {
        let segments = notice
            .iter()
            .chain(route.iter().filter(|_| with_route))
            .chain(work.iter().filter(|_| with_work))
            .cloned()
            .chain(hints.iter().map(|hint| hint_spans(*hint, palette)));
        let mut left = vec![Span::raw(" ")];
        for (index, segment) in segments.enumerate() {
            if index > 0 {
                left.push(Span::raw("  "));
            }
            left.extend(segment);
        }
        let right: Vec<&str> = [position, attribution]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        let right = if right.is_empty() {
            Line::default()
        } else {
            Line::from(format!("{} ", right.join("  ")))
        };
        let left = Line::from(left);
        if left.width() + 2 + right.width() <= usize::from(width) {
            return (left, right);
        }
        fallback = (left, right);
    }
    fallback
}

fn notice_spans(app: &App, palette: Palette) -> Option<Vec<Span<'static>>> {
    let span = match app.status()? {
        (Level::Error, text) => Span::styled(format!("✗ {text}"), palette.error()),
        (Level::Info, text) => Span::styled(format!("✓ {text}"), Style::new().fg(palette.route())),
    };
    Some(vec![span])
}

/// The selected route and step, when the panel does not show them.
fn route_spans(app: &App, status: Status) -> Option<Vec<Span<'static>>> {
    if status.panel_shown {
        return None;
    }
    let Routes::Ready {
        routes,
        selected,
        step,
    } = app.trip().routes()
    else {
        return None;
    };
    let route = routes.get(*selected)?;
    let mut text = format!(
        "route {}/{}: {} · {}",
        selected + 1,
        routes.len(),
        duration(route.duration_s),
        distance(route.distance_m, status.units)
    );
    if let Some(step) = step {
        let _ = write!(text, " · step {}/{}", step + 1, route.steps.len());
    }
    Some(vec![Span::styled(
        text,
        Style::new().fg(status.palette.route()),
    )])
}

/// The spinner for work that nothing else on the screen shows.
fn work_spans(app: &App, status: Status) -> Option<Vec<Span<'static>>> {
    let work = if app.loading() {
        "loading the map"
    } else if !status.panel_shown && matches!(app.trip().routes(), Routes::Loading) {
        "searching for routes"
    } else {
        return None;
    };
    Some(vec![Span::styled(
        format!("{} {work}", spinner(app)),
        Style::new().fg(status.palette.muted()),
    )])
}

fn spinner(app: &App) -> char {
    SPINNER[app.ticks() % SPINNER.len()]
}

/// The keys for what has the keys now. The last hint opens or closes the help, or closes the
/// box, so the status line keeps it longest.
fn hints(app: &App) -> &'static [Hint] {
    if app.help() && app.help_overflow() > 0 {
        return &[("↑↓", "scroll"), ("Esc", "close")];
    }
    if app.help() {
        return &[("Esc", "close")];
    }
    match app.prompt() {
        Some(Prompt::Search(_) | Prompt::Places { .. }) => {
            &[("↑↓", "select"), ("Enter", "go"), ("Esc", "close")]
        }
        Some(Prompt::SavePlace { .. }) => &[("Enter", "save"), ("Esc", "cancel")],
        None if app.trip().is_empty() => &[
            ("/", "search"),
            ("s", "start"),
            ("e", "end"),
            ("'", "places"),
            ("?", "keys"),
        ],
        None => &[
            ("Tab", "route"),
            ("] [", "step"),
            ("p", "panel"),
            ("c", "clear"),
            ("?", "keys"),
        ],
    }
}

fn hint_spans((key, action): Hint, palette: Palette) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            key,
            Style::new()
                .fg(palette.route())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {action}"), Style::new().fg(palette.muted())),
    ]
}

/// Only the copyright holders of the default source, without the other words. An attribution
/// that names another holder after a `©` stays complete.
fn short_attribution(attribution: &str) -> String {
    const HOLDERS: [&str; 2] = ["OpenMapTiles", "OpenStreetMap"];
    let only_known = attribution.split('©').skip(1).all(|after| {
        HOLDERS
            .iter()
            .any(|holder| after.trim_start().starts_with(holder))
    });
    let holders: Vec<String> = HOLDERS
        .iter()
        .filter(|holder| attribution.contains(*holder))
        .map(|holder| format!("© {holder}"))
        .collect();
    if only_known && !holders.is_empty() {
        holders.join(" ")
    } else {
        attribution.to_owned()
    }
}

/// What a prompt shows: an optional text input, then a list or a note.
struct PromptView {
    title: &'static str,
    input: Option<String>,
    /// A label, and a detail for the right side.
    items: Vec<(String, String)>,
    selected: Option<usize>,
    note: Option<(String, Style)>,
    /// The number of items, for the bottom border.
    count: Option<String>,
}

fn prompt_view(
    prompt: &Prompt,
    app: &App,
    palette: Palette,
    units: Units,
    spinner: char,
) -> PromptView {
    match prompt {
        Prompt::Search(search) => {
            let note = if !search.results.is_empty() {
                None
            } else if let Some(error) = &search.error {
                Some((format!("✗ {error} Type to try again."), palette.error()))
            } else if search.pending.is_some() {
                Some((format!("{spinner} Searching…"), Style::new()))
            } else if search.text.trim().is_empty() {
                Some((
                    "Type a place or an address. Enter: go there.".to_owned(),
                    Style::new(),
                ))
            } else {
                Some(("No places found.".to_owned(), Style::new()))
            };
            PromptView {
                title: " Search ",
                input: Some(format!("› {}", search.text)),
                items: search
                    .results
                    .iter()
                    .map(|place| {
                        let detail = place_detail(place, app.view().center(), units);
                        (place.label.clone(), detail)
                    })
                    .collect(),
                selected: Some(search.selected),
                note,
                count: (!search.results.is_empty()).then(|| count(search.results.len(), "result")),
            }
        }
        Prompt::SavePlace { name, invalid } => PromptView {
            title: " Save the place at + ",
            input: Some(format!("@{name}")),
            items: Vec::new(),
            selected: None,
            note: Some(if *invalid {
                (
                    "✗ Not a valid name. Use letters, digits, '-', and '_'.".to_owned(),
                    palette.error(),
                )
            } else {
                (
                    "Letters, digits, '-', and '_'. Enter: save.".to_owned(),
                    Style::new(),
                )
            }),
            count: None,
        },
        Prompt::Places { selected } => PromptView {
            title: " Saved places ",
            input: None,
            items: app
                .places()
                .iter()
                .map(|(name, location)| (format!("@{name}"), coordinates(*location)))
                .collect(),
            selected: Some(*selected),
            note: app.places().is_empty().then(|| {
                (
                    "No saved places. m: save the place at +".to_owned(),
                    Style::new(),
                )
            }),
            count: (!app.places().is_empty()).then(|| count(app.places().len(), "place")),
        },
    }
}

/// The kind of place, then its distance from the center of the map.
fn place_detail(place: &Place, center: LatLon, units: Units) -> String {
    let away = distance(center.distance_m(place.location), units);
    let kind = place
        .category
        .as_deref()
        .and_then(|category| category.split_once(':'))
        .map(|(key, value)| if value == "yes" { key } else { value });
    match kind {
        Some(kind) => format!("{} · {away}", kind.replace('_', " ")),
        None => away,
    }
}

/// The label at the left, cut with "…" when the detail at the right needs its room.
fn item_line(label: &str, detail: &str, width: usize, palette: Palette) -> Line<'static> {
    let detail_width = display_width(detail);
    let room = width.saturating_sub(detail_width + 2);
    if room < LABEL_MIN_WIDTH {
        return Line::from(cut(label, width));
    }
    let label = cut(label, room);
    let gap = width.saturating_sub(display_width(&label) + detail_width);
    Line::from_iter([
        Span::raw(label),
        Span::raw(" ".repeat(gap)),
        Span::styled(detail.to_owned(), Style::new().fg(palette.muted())),
    ])
}

/// `text` in at most `width` columns. A cut text ends with "…".
fn cut(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_owned();
    }
    let mut kept = String::new();
    let mut used = 0;
    for c in text.chars() {
        let char_width = display_width(c.encode_utf8(&mut [0; 4]));
        if used + char_width + 1 > width {
            break;
        }
        kept.push(c);
        used += char_width;
    }
    if width == 0 {
        kept
    } else {
        format!("{}…", kept.trim_end())
    }
}

/// An open box takes the keys. Its round corners show that also without colors.
fn box_block(title: &'static str, palette: Palette) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(palette.route()))
        .title(title)
        .title_style(
            Style::new()
                .fg(palette.route())
                .add_modifier(Modifier::BOLD),
        )
        .style(Style::new().fg(palette.text()).bg(palette.background()))
}

/// Draws the prompt at the top of the map, so the place at the crosshair stays visible.
fn draw_prompt(frame: &mut Frame, view: &PromptView, palette: Palette, area: Rect) {
    let width = PROMPT_WIDTH.min(area.width);
    let mut block = box_block(view.title, palette).padding(Padding::horizontal(1));
    if let Some(count) = &view.count {
        block = block.title_bottom(Line::from(format!(" {count} ")).right_aligned());
    }
    let text_width = block.inner(Rect::new(0, 0, width, 3)).width;
    let note: Vec<Line> = view.note.as_ref().map_or_else(Vec::new, |(text, style)| {
        hanging("", text, text_width)
            .into_iter()
            .map(|line| Line::styled(line, *style))
            .collect()
    });
    let input_height = u16::from(view.input.is_some());
    let list_height = view.items.len() + note.len();
    let height = u16::try_from(list_height)
        .unwrap_or(u16::MAX)
        .saturating_add(input_height + 2);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + u16::from(area.height > height),
        width,
        height,
    }
    .intersection(area);
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let [input_area, list_area] = inner.layout(&Layout::vertical([
        Constraint::Length(input_height),
        Constraint::Fill(1),
    ]));
    if let Some(input) = &view.input {
        let input = Line::from(input.as_str());
        let column = u16::try_from(input.width()).unwrap_or(u16::MAX);
        frame.render_widget(input, input_area);
        if input_area.width > 0 {
            frame.set_cursor_position((
                input_area.x + column.min(input_area.width - 1),
                input_area.y,
            ));
        }
    }
    let [items_area, note_area] = list_area.layout(&Layout::vertical([
        Constraint::Length(u16::try_from(view.items.len()).unwrap_or(u16::MAX)),
        Constraint::Fill(1),
    ]));
    let item_width = usize::from(items_area.width.saturating_sub(selected_width()));
    let items = view
        .items
        .iter()
        .map(|(label, detail)| item_line(label, detail, item_width, palette));
    let mut state = ListState::default().with_selected(view.selected);
    frame.render_stateful_widget(selectable(items, palette), items_area, &mut state);
    frame.render_widget(Paragraph::new(note), note_area);
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HelpRow {
    Heading(&'static str),
    /// The keys, or "" on the next line of a long action.
    Key(&'static str, String),
    Blank,
}

impl HelpRow {
    fn width(&self, key_width: usize) -> usize {
        match self {
            Self::Heading(name) => display_width(name) + 1,
            Self::Key(_, action) => key_width + display_width(action) + 4,
            Self::Blank => 0,
        }
    }

    fn line(&self, key_width: usize, palette: Palette) -> Line<'static> {
        match self {
            Self::Heading(name) => Line::styled(
                format!(" {name}"),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Self::Key(keys, action) => Line::from_iter([
                Span::styled(
                    format!(" {keys:key_width$}"),
                    Style::new()
                        .fg(palette.route())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("  {action} ")),
            ]),
            Self::Blank => Line::default(),
        }
    }
}

fn help_key_width() -> usize {
    HELP.iter()
        .flat_map(|(_, hints)| hints.iter())
        .map(|(keys, _)| keys.chars().count())
        .max()
        .unwrap_or(0)
}

fn grouped(groups: &[HelpGroup]) -> Vec<HelpRow> {
    groups
        .iter()
        .enumerate()
        .flat_map(|(index, (name, hints))| {
            (index > 0)
                .then_some(HelpRow::Blank)
                .into_iter()
                .chain([HelpRow::Heading(name)])
                .chain(
                    hints
                        .iter()
                        .map(|(keys, action)| HelpRow::Key(keys, (*action).to_owned())),
                )
        })
        .collect()
}

/// All keys without headings. A long action wraps under its start.
fn compact(width: u16, key_width: usize) -> Vec<HelpRow> {
    let action_width = usize::from(width).saturating_sub(key_width + 6).max(1);
    HELP.iter()
        .flat_map(|(_, hints)| hints.iter())
        .flat_map(|(keys, action)| {
            textwrap::wrap(action, action_width)
                .into_iter()
                .enumerate()
                .map(move |(index, line)| {
                    HelpRow::Key(if index == 0 { keys } else { "" }, line.into_owned())
                })
        })
        .collect()
}

/// The help in the largest form that fits `area`: groups in one column, groups in two columns,
/// or one list without headings, which scrolls when it does not fit.
fn help_layout(area: Rect) -> Vec<Vec<HelpRow>> {
    let key_width = help_key_width();
    [
        vec![grouped(&HELP)],
        vec![grouped(&HELP[..2]), grouped(&HELP[2..])],
    ]
    .into_iter()
    .find(|columns| {
        let (width, height) = help_size(columns, key_width);
        width <= area.width && height <= area.height
    })
    .unwrap_or_else(|| vec![compact(area.width, key_width)])
}

fn column_width(rows: &[HelpRow], key_width: usize) -> u16 {
    let width = rows.iter().map(|row| row.width(key_width)).max();
    u16::try_from(width.unwrap_or(0)).unwrap_or(u16::MAX)
}

/// The size with the borders.
fn help_size(columns: &[Vec<HelpRow>], key_width: usize) -> (u16, u16) {
    let width = columns
        .iter()
        .map(|rows| column_width(rows, key_width))
        .fold(2, u16::saturating_add);
    let height = columns.iter().map(Vec::len).max().unwrap_or(0) + 2;
    (width, u16::try_from(height).unwrap_or(u16::MAX))
}

/// The help rows that do not fit on a terminal of `width` × `height` cells.
pub fn help_overflow(width: u16, height: u16) -> usize {
    let area = help_area(Rect::new(0, 0, width, height));
    let rows = help_layout(area).iter().map(Vec::len).max().unwrap_or(0);
    rows.saturating_sub(usize::from(area.height.saturating_sub(2)))
}

/// `scroll` is the number of rows above the visible part.
fn draw_help(frame: &mut Frame, scroll: usize, palette: Palette, area: Rect) {
    let key_width = help_key_width();
    let columns = help_layout(area);
    let (width, height) = help_size(&columns, key_width);
    let popup = area
        .centered(Constraint::Length(width), Constraint::Length(height))
        .intersection(area);
    let rows = columns.iter().map(Vec::len).max().unwrap_or(0);
    let mut block = box_block(" Keys ", palette);
    if rows + 2 > usize::from(popup.height) {
        block = block.title_bottom(Line::from(" ↑↓ scroll ").right_aligned());
    }
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    let widths = columns
        .iter()
        .map(|rows| Constraint::Length(column_width(rows, key_width)));
    let offset = u16::try_from(scroll).unwrap_or(u16::MAX);
    for (rows, column) in columns
        .iter()
        .zip(Layout::horizontal(widths).split(inner).iter())
    {
        let lines: Vec<Line> = rows
            .iter()
            .map(|row| row.line(key_width, palette))
            .collect();
        frame.render_widget(Paragraph::new(lines).scroll((offset, 0)), *column);
    }
    let track = Style::new().fg(palette.route());
    draw_scrollbar(frame, palette, track, popup, inner, rows, scroll);
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tmaps_core::LatLon;
    use tmaps_core::render::View;
    use tmaps_core::tiles::TileId;

    use super::*;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    use tmaps_core::geocode::Place;

    use crate::message::{Message, SourceInfo};
    use crate::palette::{Depth, Shade};
    use crate::services::Request;
    use crate::trip::sample_route;

    fn status_line(width: u16, app: &App) -> String {
        let palette = Palette::new(Depth::Mono, Shade::Dark);
        let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
        terminal
            .draw(|frame| {
                let status = Status {
                    palette,
                    units: Units::Metric,
                    panel_shown: app.panel_visible(),
                };
                draw_status(frame, app, status, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn crosshair_is_on_the_cursor_cell() {
        let canvas = Canvas::new(8, 6);
        let palette = Palette::new(Depth::Mono, Shade::Dark);
        let area = Rect::new(0, 0, 8, 6);
        let mut buffer = Buffer::empty(area);
        MapWidget {
            canvas: &canvas,
            palette,
            markers: Vec::new(),
            dim: false,
        }
        .render(area, &mut buffer);
        assert_eq!(buffer[cursor_cell(8, 6)].symbol(), "+");
    }

    #[test]
    fn panel_is_a_drawer_on_narrow_terminals() {
        assert_eq!(
            map_size(80, 40, true),
            (80, 40 - STATUS_HEIGHT - DRAWER_MAX_HEIGHT)
        );
    }

    #[test]
    fn short_terminal_hides_the_drawer() {
        assert_eq!(map_size(80, 14, true), (80, 14 - STATUS_HEIGHT));
    }

    #[test]
    fn hidden_drawer_puts_the_route_in_the_status_line() {
        let text = screen(80, 14, &app_with_routes());
        assert!(text.lines().last().unwrap().contains("route 1/2: 2 min"));
    }

    #[test]
    fn narrow_status_drops_the_route_before_the_last_hint() {
        let mut app = app_with_routes();
        app.handle(&Event::Key(KeyEvent::new(
            KeyCode::Char('p'),
            KeyModifiers::NONE,
        )));
        assert_eq!(status_line(30, &app).trim(), "? keys");
    }

    #[test]
    fn tall_step_is_cut_to_the_list_height() {
        let step = Step {
            instruction: "Turn left onto a street with a very long name in a narrow list"
                .to_owned(),
            ..sample_route(&[LatLon::new(45.4642, 9.19).unwrap()]).steps[0].clone()
        };
        let item = step_item(
            &step,
            Units::Metric,
            (24, 2),
            Palette::new(Depth::Mono, Shade::Dark),
        );
        assert_eq!(item.height(), 2);
    }

    #[test]
    fn tiny_terminal_asks_for_more_room() {
        assert!(screen(29, 20, &app_with_routes()).contains("too small"));
    }

    #[test]
    fn panel_is_at_the_side_on_wide_terminals() {
        assert_eq!(
            map_size(120, 40, true),
            (120 - PANEL_WIDTH, 40 - STATUS_HEIGHT)
        );
    }

    #[test]
    fn point_at_the_center_is_on_the_cursor_cell() {
        let center = LatLon::new(45.4642, 9.19).unwrap();
        let area = Rect::new(0, 0, 8, 6);
        assert_eq!(
            cell_of(View::new(center, 14.0), center, area),
            Some(cursor_cell(8, 6))
        );
    }

    fn screen(width: u16, height: u16, app: &App) -> String {
        let palette = Palette::new(Depth::Mono, Shade::Dark);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw(frame, app, palette, Units::Metric))
            .unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(usize::from(width))
            .map(|row| {
                row.iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect::<String>()
                    + "\n"
            })
            .collect()
    }

    /// An app with a start, an end, and two routes from Milan to Rome.
    fn app_with_routes() -> App {
        let milan = LatLon::new(45.4642, 9.19).unwrap();
        let rome = LatLon::new(41.9028, 12.4964).unwrap();
        app_with_steps(&[milan, rome])
    }

    /// An app with two routes. The first route has a step at each point.
    fn app_with_steps(points: &[LatLon]) -> App {
        let milan = LatLon::new(45.4642, 9.19).unwrap();
        let mut app = App::new(View::new(milan, 14.0));
        app.set_map_size(80, 39);
        let press = |code| Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
        app.handle(&press(KeyCode::Char('s')));
        app.handle(&press(KeyCode::Char('L')));
        app.handle(&press(KeyCode::Char('e')));
        app.take_requests();
        app.receive(
            Message::Routes(3, Ok(vec![sample_route(points), sample_route(&[milan])])),
            Instant::now(),
        );
        app
    }

    #[test]
    fn long_text_wraps_under_its_start() {
        assert_eq!(
            hanging("  100 m  ", "Svolta a sinistra su Via Roma", 20),
            [
                "  100 m  Svolta a",
                "         sinistra su",
                "         Via Roma"
            ]
        );
    }

    #[test]
    fn panel_shows_the_routes_and_the_steps() {
        let text = screen(120, 20, &app_with_routes());
        let found = [
            "1 car",
            "▶ 1. 2 min · 1.0 km",
            "  2. 2 min",
            " 100 m  ↑ step 0",
            " 2 steps ",
        ]
        .map(|expected| text.contains(expected));
        assert_eq!(found, [true; 5]);
    }

    #[test]
    fn panel_title_shows_the_selected_step() {
        let mut app = app_with_routes();
        type_keys(&mut app, "]");
        assert!(screen(120, 20, &app).contains(" step 1/2 "));
    }

    #[test]
    fn long_step_list_gets_a_scrollbar() {
        let milan = LatLon::new(45.4642, 9.19).unwrap();
        let text = screen(120, 20, &app_with_steps(&[milan; 30]));
        assert!(text.contains('█'));
    }

    #[test]
    fn drawer_shows_the_steps_next_to_the_summary() {
        let text = screen(80, 24, &app_with_routes());
        assert!(
            text.lines()
                .any(|line| line.contains("1 car") && line.contains("step 0"))
        );
    }

    #[test]
    fn help_shows_every_key_above_the_drawer() {
        let mut app = app_with_routes();
        app.handle(&Event::Key(KeyEvent::new(
            KeyCode::Char('?'),
            KeyModifiers::NONE,
        )));
        assert!(screen(80, 24, &app).contains("Ctrl-C"));
    }

    #[test]
    fn help_keeps_the_status_line_on_a_short_terminal() {
        let mut app = app_with_source();
        type_keys(&mut app, "?");
        app.set_help_overflow(help_overflow(44, 14));
        let text = screen(44, 14, &app);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[12].contains("↑↓ scroll") && lines[13].contains("Esc close"));
    }

    #[test]
    fn help_overflow_counts_the_hidden_rows() {
        // 15 rows of keys, and 11 visible rows: 13 above the status line, less the borders.
        assert_eq!(help_overflow(44, 14), 4);
    }

    fn type_keys(app: &mut App, keys: &str) {
        for c in keys.chars() {
            app.handle(&Event::Key(KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::NONE,
            )));
        }
    }

    #[test]
    fn search_shows_the_results_below_the_text() {
        let mut app = App::new(View::new(LatLon::new(45.4642, 9.19).unwrap(), 14.0));
        type_keys(&mut app, "/Duomo");
        let id = app
            .take_requests()
            .into_iter()
            .rev()
            .find_map(|request| match request {
                Request::Search { id, .. } => Some(id),
                _ => None,
            })
            .unwrap();
        let place = Place {
            label: "Piazza del Duomo, Milano".to_owned(),
            name: None,
            location: LatLon::new(45.4641, 9.1919).unwrap(),
            category: Some("tourism:attraction".to_owned()),
            country_code: None,
        };
        app.receive(Message::SearchResults(id, Ok(vec![place])), Instant::now());
        let text = screen(80, 24, &app);
        let lines: Vec<&str> = text.lines().collect();
        let found = [
            lines[2].contains("› Duomo"),
            lines[3].contains("▶ Piazza del Duomo, Milano"),
            lines[3].contains("attraction · 150 m"),
            lines[4].contains(" 1 result "),
        ];
        assert_eq!(found, [true; 4]);
    }

    #[test]
    fn long_label_is_cut_before_the_detail() {
        let line = item_line(
            "Piazza del Duomo, Milano",
            "1.2 km",
            20,
            Palette::new(Depth::Mono, Shade::Dark),
        );
        assert_eq!(line.to_string(), "Piazza del…   1.2 km");
    }

    #[test]
    fn empty_place_list_tells_how_to_save_a_place() {
        let mut app = App::new(View::new(LatLon::new(45.4642, 9.19).unwrap(), 14.0));
        type_keys(&mut app, "'");
        assert!(screen(80, 24, &app).contains("No saved places."));
    }

    #[test]
    fn status_keeps_the_error_when_the_attribution_does_not_fit() {
        let mut app = App::new(View::new(LatLon::new(45.4642, 9.19).unwrap(), 14.0));
        let now = Instant::now();
        app.receive(
            Message::Source(Ok(SourceInfo {
                max_zoom: 14,
                attribution: "OpenFreeMap © OpenMapTiles Data from OpenStreetMap".to_owned(),
            })),
            now,
        );
        app.receive(
            Message::Tile(TileId { z: 14, x: 0, y: 0 }, Err("timeout".to_owned())),
            now,
        );

        assert!(status_line(80, &app).contains("✗ timeout"));
    }

    fn app_with_source() -> App {
        let mut app = App::new(View::new(LatLon::new(45.4642, 9.19).unwrap(), 14.0));
        app.receive(
            Message::Source(Ok(SourceInfo {
                max_zoom: 14,
                attribution: "OpenFreeMap © OpenMapTiles Data from OpenStreetMap".to_owned(),
            })),
            Instant::now(),
        );
        app
    }

    #[test]
    fn dimmed_map_keeps_the_crosshair_bright() {
        let canvas = Canvas::new(8, 6);
        let area = Rect::new(0, 0, 8, 6);
        let mut buffer = Buffer::empty(area);
        MapWidget {
            canvas: &canvas,
            palette: Palette::new(Depth::Mono, Shade::Dark),
            markers: Vec::new(),
            dim: true,
        }
        .render(area, &mut buffer);
        assert!(!buffer[cursor_cell(8, 6)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn very_narrow_status_keeps_the_help_key() {
        let line = status_line(30, &app_with_source());
        assert_eq!(line.trim(), "? keys");
    }

    #[test]
    fn other_copyright_holders_keep_the_long_attribution() {
        let attribution = "© MapTiler © OpenStreetMap contributors";
        assert_eq!(short_attribution(attribution), attribution);
    }

    #[test]
    fn empty_map_shows_how_to_search_and_get_help() {
        let line = status_line(120, &app_with_source());
        assert!(line.contains("/ search") && line.contains("? keys"));
    }

    #[test]
    fn narrow_status_shortens_the_attribution_and_keeps_the_help_key() {
        let line = status_line(44, &app_with_source());
        assert_eq!(
            line.split_whitespace().collect::<Vec<_>>().join(" "),
            "? keys © OpenMapTiles © OpenStreetMap"
        );
    }
}
