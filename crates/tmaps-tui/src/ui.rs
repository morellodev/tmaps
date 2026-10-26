use std::fmt::Write;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph, Widget};
use tmaps_core::render::{Canvas, Class, cursor_cell};

use crate::app::App;
use crate::palette::Palette;

pub const STATUS_HEIGHT: u16 = 1;
/// Shows water without colors, where it would otherwise look like land.
const MONO_WATER: char = '⠄';
const HELP: [(&str, &str); 6] = [
    ("←↓↑→  hjkl", "pan"),
    ("Shift + pan", "pan faster"),
    ("+ -  wheel", "zoom"),
    ("drag", "pan"),
    ("?  Esc", "show or hide this help"),
    ("q  Ctrl-C", "quit"),
];

pub fn draw(frame: &mut Frame, app: &App, palette: Palette) {
    let [map_area, status_area] = frame.area().layout(&Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(STATUS_HEIGHT),
    ]));
    frame.render_widget(
        MapWidget {
            canvas: &app.canvas(),
            palette,
        },
        map_area,
    );
    draw_status(frame, app, palette, status_area);
    if app.help() {
        draw_help(frame, map_area);
    }
}

struct MapWidget<'a> {
    canvas: &'a Canvas,
    palette: Palette,
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
    }
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
    use crate::loader::{Message, SourceInfo};
    use crate::palette::{Depth, Shade};

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
        }
        .render(area, &mut buffer);
        assert_eq!(buffer[cursor_cell(8, 6)].symbol(), "+");
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
