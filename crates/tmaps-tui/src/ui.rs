use std::fmt::Write;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Clear, HighlightSpacing, List, ListItem, ListState, Padding, Paragraph,
    Widget,
};
use tmaps_core::LatLon;
use tmaps_core::config::Units;
use tmaps_core::format::{distance, duration};
use tmaps_core::render::{Canvas, Class, DOTS_PER_CELL_X, DOTS_PER_CELL_Y, View, cursor_cell};
use tmaps_core::routing::{Mode, Route, Step};

use crate::app::{App, Level};
use crate::palette::Palette;
use crate::prompt::Prompt;
use crate::trip::{Routes, Trip, Waypoint};

const STATUS_HEIGHT: u16 = 1;
const PANEL_WIDTH: u16 = 40;
const DRAWER_HEIGHT: u16 = 12;
/// The inner width of the side panel, inside the borders and the padding.
const SUMMARY_WIDTH: u16 = PANEL_WIDTH - 4;
/// Narrower terminals get the panel as a drawer below the map, so the map keeps its width.
const SIDE_PANEL_MIN_WIDTH: u16 = 100;
const PROMPT_WIDTH: u16 = 60;
/// Shows water without colors, where it would otherwise look like land.
const MONO_WATER: char = '⠄';
/// Braille, as the map.
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const SELECTED: &str = "▶ ";
/// A key and what it does, for the status line.
type Hint = (&'static str, &'static str);
const HELP: [(&str, &str); 15] = [
    ("←↓↑→  hjkl", "pan"),
    ("Shift + pan", "pan faster"),
    ("+ -  wheel", "zoom"),
    ("drag", "pan"),
    ("s  e", "set the start or the end at +"),
    ("x  c", "swap or clear the points"),
    ("1 2 3", "car, bike, foot"),
    ("Tab", "next route"),
    ("]  [", "next or previous step"),
    ("p", "show or hide the route panel"),
    ("/", "search"),
    ("m", "save the place at +"),
    ("'", "go to a saved place"),
    ("?  Esc", "show or hide this help"),
    ("q  Ctrl-C", "quit"),
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
    if !panel {
        return Areas {
            map: main,
            panel: None,
            status,
        };
    }
    let [map, panel] = if area.width >= SIDE_PANEL_MIN_WIDTH {
        main.layout(&Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(PANEL_WIDTH),
        ]))
    } else {
        main.layout(&Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(DRAWER_HEIGHT),
        ]))
    };
    Areas {
        map,
        panel: Some(panel),
        status,
    }
}

/// The size in cells of the map on a terminal of `width` × `height` cells.
pub fn map_size(width: u16, height: u16, panel: bool) -> (u16, u16) {
    let map = areas(Rect::new(0, 0, width, height), panel).map;
    (map.width, map.height)
}

pub fn draw(frame: &mut Frame, app: &App, palette: Palette, units: Units) {
    let areas = areas(frame.area(), app.panel_visible());
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
    draw_status(frame, app, palette, areas.status);
    if let Some(prompt) = app.prompt() {
        draw_prompt(
            frame,
            &prompt_view(prompt, app, palette, spinner),
            palette,
            areas.map,
        );
    }
    if app.help() {
        draw_help(frame, palette, frame.area());
    }
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
    let block = Block::bordered()
        .title(" Route ")
        .title_style(Style::new().add_modifier(Modifier::BOLD))
        .border_style(Style::new().fg(palette.muted()))
        .padding(Padding::horizontal(1))
        .style(Style::new().fg(palette.text()).bg(palette.background()));
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
            .map(|step| step_item(step, units, width, palette))
            .collect();
        let mut state = ListState::default().with_selected(step);
        frame.render_stateful_widget(
            selectable(steps, palette).scroll_padding(1),
            steps_area,
            &mut state,
        );
    }
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

/// The distance first, in a muted column, and the instruction to its right.
fn step_item(step: &Step, units: Units, width: u16, palette: Palette) -> ListItem<'static> {
    let distance = if step.distance_m > 0.0 {
        distance(step.distance_m, units)
    } else {
        String::new()
    };
    let prefix = format!("{distance:>7}  ");
    let lines = hanging(&prefix, &step.instruction, width)
        .into_iter()
        .enumerate()
        .map(|(index, mut line)| {
            if index > 0 {
                return Line::from(line);
            }
            let instruction = line.split_off(prefix.len());
            Line::from_iter([
                Span::styled(line, Style::new().fg(palette.muted())),
                Span::raw(instruction),
            ])
        });
    ListItem::new(Text::from_iter(lines))
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

fn draw_status(frame: &mut Frame, app: &App, palette: Palette, area: Rect) {
    let style = Style::new()
        .fg(palette.text())
        .bg(palette.status_background());
    let (left, right) = status_lines(app, palette, area.width);
    let [left_area, right_area] = area.layout(&Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(u16::try_from(right.width()).unwrap_or(u16::MAX)),
    ]));
    frame.render_widget(Paragraph::new(left).style(style), left_area);
    frame.render_widget(Paragraph::new(right).style(style), right_area);
}

/// The left part has the notice, the work on the way, and the key hints. The right part has the
/// position and the attribution. A narrow line drops whole parts, never a part of a word, in
/// this order: the position, the work and the hints except the last, the long attribution, the
/// last hint, the attribution.
fn status_lines(app: &App, palette: Palette, width: u16) -> (Line<'static>, Line<'static>) {
    let notice = notice_spans(app, palette);
    let work = work_spans(app, palette);
    let hints = hints(app);
    let last_hint = &hints[hints.len().saturating_sub(1)..];
    let view = app.view();
    let position = format!("{}  z{:.1}", coordinates(view.center()), view.zoom());
    let attribution = app.attribution().unwrap_or_default();
    let short = short_attribution(attribution);
    let candidates: [(bool, &[Hint], &str, &str); 6] = [
        (true, hints, &position, attribution),
        (true, hints, "", attribution),
        (false, last_hint, "", attribution),
        (false, last_hint, "", &short),
        (false, &[], "", &short),
        (false, &[], "", ""),
    ];
    let mut fallback = (Line::default(), Line::default());
    for (with_work, hints, position, attribution) in candidates {
        let segments = notice
            .iter()
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

/// The spinner for work that nothing else on the screen shows.
fn work_spans(app: &App, palette: Palette) -> Option<Vec<Span<'static>>> {
    let work = if app.loading() {
        "loading the map"
    } else if !app.panel_visible() && matches!(app.trip().routes(), Routes::Loading) {
        "searching for routes"
    } else {
        return None;
    };
    Some(vec![Span::styled(
        format!("{} {work}", spinner(app)),
        Style::new().fg(palette.muted()),
    )])
}

fn spinner(app: &App) -> char {
    SPINNER[app.ticks() % SPINNER.len()]
}

/// The keys for what has the keys now. The last hint opens or closes the help, or closes the
/// box, so the status line keeps it longest.
fn hints(app: &App) -> &'static [Hint] {
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
    items: Vec<String>,
    selected: Option<usize>,
    note: Option<(String, Style)>,
}

fn prompt_view(prompt: &Prompt, app: &App, palette: Palette, spinner: char) -> PromptView {
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
                    .map(|place| place.label.clone())
                    .collect(),
                selected: Some(search.selected),
                note,
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
        },
        Prompt::Places { selected } => PromptView {
            title: " Saved places ",
            input: None,
            items: app
                .places()
                .iter()
                .map(|(name, location)| format!("@{name}  {}", coordinates(*location)))
                .collect(),
            selected: Some(*selected),
            note: app.places().is_empty().then(|| {
                (
                    "No saved places. m: save the place at +".to_owned(),
                    Style::new(),
                )
            }),
        },
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
    let block = box_block(view.title, palette).padding(Padding::horizontal(1));
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
    let mut state = ListState::default().with_selected(view.selected);
    frame.render_stateful_widget(
        selectable(view.items.iter().map(String::as_str), palette),
        items_area,
        &mut state,
    );
    frame.render_widget(Paragraph::new(note), note_area);
}

fn draw_help(frame: &mut Frame, palette: Palette, area: Rect) {
    let key_width = HELP
        .iter()
        .map(|(keys, _)| keys.chars().count())
        .max()
        .unwrap_or(0);
    let key_style = Style::new()
        .fg(palette.route())
        .add_modifier(Modifier::BOLD);
    let lines: Vec<Line> = HELP
        .iter()
        .map(|(keys, action)| {
            Line::from_iter([
                Span::styled(format!(" {keys:key_width$}"), key_style),
                Span::raw(format!("  {action} ")),
            ])
        })
        .collect();
    let width = lines.iter().map(Line::width).max().unwrap_or(0) + 2;
    let popup = area.centered(
        Constraint::Length(u16::try_from(width).unwrap_or(u16::MAX)),
        Constraint::Length(u16::try_from(lines.len() + 2).unwrap_or(u16::MAX)),
    );
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(box_block(" Keys ", palette)),
        popup,
    );
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
            .draw(|frame| draw_status(frame, app, palette, frame.area()))
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
            (80, 40 - STATUS_HEIGHT - DRAWER_HEIGHT)
        );
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
        let mut app = App::new(View::new(milan, 14.0));
        app.set_map_size(80, 39);
        let press = |code| Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
        app.handle(&press(KeyCode::Char('s')));
        app.handle(&press(KeyCode::Char('L')));
        app.handle(&press(KeyCode::Char('e')));
        app.take_requests();
        app.receive(
            Message::Routes(
                3,
                Ok(vec![sample_route(&[milan, rome]), sample_route(&[milan])]),
            ),
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
            " 100 m  step 0",
        ]
        .map(|expected| text.contains(expected));
        assert_eq!(found, [true; 4]);
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
            category: None,
            country_code: None,
        };
        app.receive(Message::SearchResults(id, Ok(vec![place])), Instant::now());
        let text = screen(80, 24, &app);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[2].contains("› Duomo") && lines[3].contains("▶ Piazza del Duomo, Milano"));
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
