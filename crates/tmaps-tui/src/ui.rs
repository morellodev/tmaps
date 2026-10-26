use std::fmt::Write;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Padding, Paragraph, Widget};
use tmaps_core::LatLon;
use tmaps_core::config::Units;
use tmaps_core::format::{distance, duration};
use tmaps_core::render::{Canvas, Class, DOTS_PER_CELL_X, DOTS_PER_CELL_Y, View, cursor_cell};
use tmaps_core::routing::{Mode, Route, Step};

use crate::app::App;
use crate::palette::Palette;
use crate::trip::{Routes, Trip, Waypoint};

const STATUS_HEIGHT: u16 = 1;
const PANEL_WIDTH: u16 = 40;
const DRAWER_HEIGHT: u16 = 12;
/// The inner width of the side panel, inside the borders and the padding.
const SUMMARY_WIDTH: u16 = PANEL_WIDTH - 4;
/// Narrower terminals get the panel as a drawer below the map, so the map keeps its width.
const SIDE_PANEL_MIN_WIDTH: u16 = 100;
/// Shows water without colors, where it would otherwise look like land.
const MONO_WATER: char = '⠄';
const HELP: [(&str, &str); 12] = [
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
        },
        areas.map,
    );
    if let Some(panel) = areas.panel {
        draw_panel(frame, trip, palette, units, panel);
    }
    draw_status(frame, app, palette, areas.status);
    if app.help() {
        draw_help(frame, frame.area());
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
        let (column, row) = cursor_cell(area.width, area.height);
        if let Some(target) = buf.cell_mut((area.x + column, area.y + row)) {
            target.set_char('+').set_style(
                Style::new()
                    .fg(self.palette.crosshair())
                    .add_modifier(Modifier::BOLD),
            );
        }
        // After the crosshair: a point set at the crosshair must show at once.
        for marker in &self.markers {
            if let Some(target) = buf.cell_mut((area.x + marker.column, area.y + marker.row)) {
                target
                    .set_char(marker.symbol)
                    .set_style(self.palette.marker());
            }
        }
    }
}

fn draw_panel(frame: &mut Frame, trip: &Trip, palette: Palette, units: Units, area: Rect) {
    let block = Block::bordered()
        .title(" Route ")
        .padding(Padding::horizontal(1))
        .style(Style::new().fg(palette.text()).bg(palette.background()));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let mut summary = vec![
        mode_line(trip.mode()),
        Line::default(),
        waypoint_line('A', trip.start(), "s: start at +", palette),
        waypoint_line('B', trip.end(), "e: end at +", palette),
        Line::default(),
    ];
    match trip.routes() {
        Routes::None => {}
        Routes::Loading => summary.push(Line::from("Searching for routes…")),
        Routes::Failed(error) => summary.push(Line::from(error.as_str())),
        Routes::Ready {
            routes, selected, ..
        } => summary.extend(
            routes
                .iter()
                .enumerate()
                .map(|(index, route)| summary_line(index, route, index == *selected, units)),
        ),
    }
    // A drawer is wide and short, so the steps go next to the summary.
    let [summary_area, steps_area] = if inner.width >= 2 * SUMMARY_WIDTH {
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
        let steps: Vec<ListItem> = route
            .steps
            .iter()
            .map(|step| ListItem::new(step_line(step, units)))
            .collect();
        let mut state = ListState::default().with_selected(step);
        frame.render_stateful_widget(
            List::new(steps).highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
            steps_area,
            &mut state,
        );
    }
}

fn mode_line(mode: Mode) -> Line<'static> {
    let spans = [
        (Mode::Car, "1 car"),
        (Mode::Bike, "2 bike"),
        (Mode::Foot, "3 foot"),
    ]
    .into_iter()
    .flat_map(|(option, text)| {
        let style = if option == mode {
            Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::new()
        };
        [Span::styled(format!(" {text} "), style), Span::raw(" ")]
    });
    Line::from_iter(spans)
}

fn waypoint_line(
    symbol: char,
    waypoint: Option<&Waypoint>,
    hint: &'static str,
    palette: Palette,
) -> Line<'static> {
    let text = match waypoint {
        Some(Waypoint {
            name: Some(name), ..
        }) => name.clone(),
        Some(waypoint) => waypoint.location.to_string(),
        None => hint.to_owned(),
    };
    Line::from_iter([
        Span::styled(symbol.to_string(), palette.marker()),
        Span::raw(format!(" {text}")),
    ])
}

fn summary_line(index: usize, route: &Route, selected: bool, units: Units) -> Line<'static> {
    let mut text = format!(
        "{} {}. {} · {}",
        if selected { '▶' } else { ' ' },
        index + 1,
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
    let style = if selected {
        Style::new().add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    Line::styled(text, style)
}

/// The distance first, in a column: a long instruction loses its end at the panel edge.
fn step_line(step: &Step, units: Units) -> String {
    let distance = if step.distance_m > 0.0 {
        distance(step.distance_m, units)
    } else {
        String::new()
    };
    format!("{distance:>7}  {}", step.instruction)
}

fn draw_status(frame: &mut Frame, app: &App, palette: Palette, area: Rect) {
    let style = Style::new()
        .fg(palette.text())
        .bg(palette.status_background());
    let view = app.view();
    let center = view.center();
    let mut left = format!(
        " {:.5}, {:.5}  z{:.1}",
        center.lat(),
        center.lon(),
        view.zoom()
    );
    if app.loading() > 0 {
        let _ = write!(left, "  loading {}", app.loading());
    }
    if let Some(error) = app.error() {
        let _ = write!(left, "  error: {error}");
    }
    let left = Line::from(left);
    let attribution = Line::from(format!("{} ", app.attribution().unwrap_or_default()));
    // The attribution gets the space that the position and the error leave. A right-aligned line
    // that is too wide loses its start.
    let [left_area, attribution_area] = area.layout(&Layout::horizontal([
        Constraint::Length(u16::try_from(left.width() + 2).unwrap_or(u16::MAX)),
        Constraint::Fill(1),
    ]));
    frame.render_widget(Paragraph::new(left).style(style), left_area);
    frame.render_widget(attribution.right_aligned().style(style), attribution_area);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let key_width = HELP
        .iter()
        .map(|(keys, _)| keys.chars().count())
        .max()
        .unwrap_or(0);
    let lines: Vec<Line> = HELP
        .iter()
        .map(|(keys, action)| Line::from(format!(" {keys:key_width$}  {action} ")))
        .collect();
    let width = lines.iter().map(Line::width).max().unwrap_or(0) + 2;
    let popup = area.centered(
        Constraint::Length(u16::try_from(width).unwrap_or(u16::MAX)),
        Constraint::Length(u16::try_from(lines.len() + 2).unwrap_or(u16::MAX)),
    );
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Keys ")),
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

    use crate::message::{Message, SourceInfo};
    use crate::palette::{Depth, Shade};
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

        assert!(status_line(80, &app).contains("error: timeout"));
    }
}
