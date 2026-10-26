use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use lru::LruCache;
use tmaps_core::render::{
    Canvas, DOTS_PER_CELL_X, DOTS_PER_CELL_Y, PreparedTile, RouteLine, View, render,
};
use tmaps_core::routing::{Mode, Route};
use tmaps_core::tiles::TileId;

use crate::message::{Message, SourceInfo};
use crate::services::Request;
use crate::trip::{End, Routes, Trip};

/// Holds more tiles than the largest screen shows, so that a pan back does not reload them.
const TILE_CACHE_SIZE: NonZeroUsize = NonZeroUsize::new(64).unwrap();
const RETRY_AFTER: Duration = Duration::from_secs(10);
const PAN_STEP: f64 = 0.1;
const FAST_PAN_STEP: f64 = 0.5;
/// A step of the itinerary is a street corner, so the map zooms in at least to street level.
const STEP_ZOOM: f64 = 16.0;

pub struct App {
    view: View,
    map_size: (u16, u16),
    source: SourceState,
    tiles: LruCache<TileId, PreparedTile>,
    pending: HashSet<TileId>,
    failed: HashMap<TileId, Instant>,
    error: Option<String>,
    help: bool,
    quit: bool,
    drag_from: Option<(u16, u16)>,
    trip: Trip,
    panel: bool,
}

enum SourceState {
    Idle,
    Loading,
    Ready(SourceInfo),
    Failed(Instant),
}

impl App {
    pub fn new(view: View) -> Self {
        Self {
            view,
            map_size: (0, 0),
            source: SourceState::Idle,
            tiles: LruCache::new(TILE_CACHE_SIZE),
            pending: HashSet::new(),
            failed: HashMap::new(),
            error: None,
            help: false,
            quit: false,
            drag_from: None,
            trip: Trip::default(),
            panel: true,
        }
    }

    pub fn view(&self) -> View {
        self.view
    }

    pub fn set_map_size(&mut self, width: u16, height: u16) {
        self.map_size = (width, height);
    }

    pub fn help(&self) -> bool {
        self.help
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// A route error first: it is the answer to the last action of the user.
    pub fn error(&self) -> Option<&str> {
        self.trip.error().or(self.error.as_deref())
    }

    pub fn trip(&self) -> &Trip {
        &self.trip
    }

    /// The itinerary panel shows when a point is set, until the user hides it.
    pub fn panel_visible(&self) -> bool {
        self.panel && !self.trip.is_empty()
    }

    /// The requests for the providers since the last call.
    pub fn take_requests(&mut self) -> Vec<Request> {
        self.trip.take_requests()
    }

    pub fn attribution(&self) -> Option<&str> {
        match &self.source {
            SourceState::Ready(info) => Some(&info.attribution),
            _ => None,
        }
    }

    pub fn loading(&self) -> usize {
        self.pending.len() + usize::from(matches!(self.source, SourceState::Loading))
    }

    /// Returns `true` when the screen must be drawn again.
    pub fn handle(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key(*key),
            Event::Mouse(mouse) => self.on_mouse(*mouse),
            Event::Resize(..) => true,
            _ => false,
        }
    }

    pub fn receive(&mut self, message: Message, now: Instant) {
        match message {
            Message::Source(Ok(info)) => {
                self.source = SourceState::Ready(info);
                self.error = None;
            }
            Message::Source(Err(error)) => {
                self.source = SourceState::Failed(now);
                self.error = Some(error);
            }
            Message::Tile(id, Ok(tile)) => {
                self.pending.remove(&id);
                self.tiles.put(id, *tile);
                self.error = None;
            }
            Message::Tile(id, Err(error)) => {
                self.pending.remove(&id);
                self.failed.insert(id, now);
                self.error = Some(error);
            }
            Message::Skipped(id) => {
                self.pending.remove(&id);
            }
            Message::Routes(id, result) => {
                if let Some(routes) = self.trip.receive_routes(id, result) {
                    let points: Vec<_> = routes
                        .iter()
                        .flat_map(|route| route.geometry.iter().copied())
                        .collect();
                    let (width, height) = self.map_size_dots();
                    if let Some(view) = View::fitting(&points, width, height) {
                        self.view = view;
                    }
                }
            }
            Message::PlaceName(id, name) => self.trip.receive_name(id, name),
        }
    }

    /// Returns `true` once, and again after a failure and a wait, until the source is ready.
    pub fn wants_source(&mut self, now: Instant) -> bool {
        let wants = match self.source {
            SourceState::Idle => true,
            SourceState::Failed(at) => now >= at + RETRY_AFTER,
            SourceState::Loading | SourceState::Ready(_) => false,
        };
        if wants {
            self.source = SourceState::Loading;
        }
        wants
    }

    pub fn visible_tiles(&self) -> Vec<TileId> {
        let Some(max_zoom) = self.max_zoom() else {
            return Vec::new();
        };
        let (width, height) = self.map_size_dots();
        let mut seen = HashSet::new();
        self.view
            .visible_tiles(width, height, max_zoom)
            .into_iter()
            .map(|tile| tile.id)
            .filter(|id| seen.insert(*id))
            .collect()
    }

    /// The visible tiles to request now, nearest to the center first. They count as pending
    /// until a [`Message`] arrives for them.
    pub fn tiles_to_load(&mut self, now: Instant) -> Vec<TileId> {
        self.failed.retain(|_, at| now < *at + RETRY_AFTER);
        let mut load = Vec::new();
        for id in self.visible_tiles() {
            // `get`, not `contains`: a visible tile becomes the most recently used.
            if self.tiles.get(&id).is_some()
                || self.pending.contains(&id)
                || self.failed.contains_key(&id)
            {
                continue;
            }
            self.pending.insert(id);
            load.push(id);
        }
        load
    }

    /// When the next failed request can be tried again.
    pub fn next_retry(&self) -> Option<Instant> {
        let source_retry = match self.source {
            SourceState::Failed(at) => Some(at + RETRY_AFTER),
            _ => None,
        };
        self.failed
            .values()
            .map(|at| *at + RETRY_AFTER)
            .chain(source_retry)
            .min()
    }

    pub fn canvas(&self) -> Canvas {
        let (width, height) = self.map_size;
        let Some(max_zoom) = self.max_zoom() else {
            return Canvas::new(width, height);
        };
        let routes = match self.trip.routes() {
            Routes::Ready {
                routes, selected, ..
            } => route_lines(routes, *selected),
            _ => Vec::new(),
        };
        render(self.view, width, height, max_zoom, &routes, |id| {
            self.tiles.peek(&id)
        })
    }

    fn max_zoom(&self) -> Option<u8> {
        match &self.source {
            SourceState::Ready(info) => Some(info.max_zoom),
            _ => None,
        }
    }

    fn map_size_dots(&self) -> (f64, f64) {
        (
            f64::from(self.map_size.0) * f64::from(DOTS_PER_CELL_X),
            f64::from(self.map_size.1) * f64::from(DOTS_PER_CELL_Y),
        )
    }

    fn on_key(&mut self, key: KeyEvent) -> bool {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let step = if shift { FAST_PAN_STEP } else { PAN_STEP };
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            _ if key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                return false;
            }
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.help = !self.help,
            KeyCode::Esc => self.help = false,
            KeyCode::Left | KeyCode::Char('h') => self.pan_screens(-step, 0.0),
            KeyCode::Right | KeyCode::Char('l') => self.pan_screens(step, 0.0),
            KeyCode::Up | KeyCode::Char('k') => self.pan_screens(0.0, -step),
            KeyCode::Down | KeyCode::Char('j') => self.pan_screens(0.0, step),
            KeyCode::Char('H') => self.pan_screens(-FAST_PAN_STEP, 0.0),
            KeyCode::Char('L') => self.pan_screens(FAST_PAN_STEP, 0.0),
            KeyCode::Char('K') => self.pan_screens(0.0, -FAST_PAN_STEP),
            KeyCode::Char('J') => self.pan_screens(0.0, FAST_PAN_STEP),
            KeyCode::Char('+' | '=') => self.view = self.view.zoom_by(1.0),
            KeyCode::Char('-' | '_') => self.view = self.view.zoom_by(-1.0),
            KeyCode::Char('s') => self.trip.set(End::Start, self.view.center()),
            KeyCode::Char('e') => self.trip.set(End::End, self.view.center()),
            KeyCode::Char('x') => self.trip.swap(),
            KeyCode::Char('c') => self.trip.clear(),
            KeyCode::Char('1') => self.trip.set_mode(Mode::Car),
            KeyCode::Char('2') => self.trip.set_mode(Mode::Bike),
            KeyCode::Char('3') => self.trip.set_mode(Mode::Foot),
            KeyCode::Tab => self.trip.next_alternative(),
            KeyCode::Char('p') if !self.trip.is_empty() => self.panel = !self.panel,
            KeyCode::Char(']') => self.show_step(1),
            KeyCode::Char('[') => self.show_step(-1),
            _ => return false,
        }
        true
    }

    fn on_mouse(&mut self, mouse: MouseEvent) -> bool {
        let position = (mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::ScrollUp => self.view = self.view.zoom_by(1.0),
            MouseEventKind::ScrollDown => self.view = self.view.zoom_by(-1.0),
            MouseEventKind::Down(MouseButton::Left) => {
                self.drag_from = Some(position);
                return false;
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some((column, row)) = self.drag_from.replace(position) else {
                    return false;
                };
                // The map follows the pointer, so the view moves the other way.
                self.view = self.view.pan(
                    (f64::from(column) - f64::from(position.0)) * f64::from(DOTS_PER_CELL_X),
                    (f64::from(row) - f64::from(position.1)) * f64::from(DOTS_PER_CELL_Y),
                );
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag_from = None;
                return false;
            }
            _ => return false,
        }
        true
    }

    fn show_step(&mut self, delta: isize) {
        if let Some(location) = self.trip.move_step(delta) {
            self.view = View::new(location, self.view.zoom().max(STEP_ZOOM));
        }
    }

    /// Pans by a fraction of the map width and height.
    fn pan_screens(&mut self, x: f64, y: f64) {
        let (width, height) = self.map_size_dots();
        self.view = self.view.pan(x * width, y * height);
    }
}

fn route_lines(routes: &[Route], selected: usize) -> Vec<RouteLine<'_>> {
    routes
        .iter()
        .enumerate()
        .map(|(index, route)| RouteLine {
            points: &route.geometry,
            selected: index == selected,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use tmaps_core::LatLon;

    use super::*;
    use crate::trip::sample_route;

    fn milan() -> LatLon {
        LatLon::new(45.4642, 9.19).unwrap()
    }

    fn ready_app() -> App {
        let mut app = App::new(View::new(milan(), 14.0));
        app.set_map_size(100, 40);
        app.wants_source(Instant::now());
        app.receive(
            Message::Source(Ok(SourceInfo {
                max_zoom: 14,
                attribution: "OpenFreeMap".to_owned(),
            })),
            Instant::now(),
        );
        app
    }

    fn press(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn right_arrow_pans_a_tenth_of_the_map_east() {
        let mut app = ready_app();
        app.handle(&press(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.view(), View::new(milan(), 14.0).pan(20.0, 0.0));
    }

    #[test]
    fn shift_pans_half_the_map() {
        let mut app = ready_app();
        app.handle(&press(KeyCode::Char('J'), KeyModifiers::SHIFT));
        assert_eq!(app.view(), View::new(milan(), 14.0).pan(0.0, 80.0));
    }

    #[test]
    fn plus_zooms_in_by_one_level() {
        let mut app = ready_app();
        app.handle(&press(KeyCode::Char('+'), KeyModifiers::NONE));
        assert_eq!(app.view().zoom(), 15.0);
    }

    #[test]
    fn drag_moves_the_map_with_the_pointer() {
        let mut app = ready_app();
        app.handle(&mouse(MouseEventKind::Down(MouseButton::Left), 50, 20));
        app.handle(&mouse(MouseEventKind::Drag(MouseButton::Left), 40, 20));
        assert_eq!(app.view(), View::new(milan(), 14.0).pan(20.0, 0.0));
    }

    #[test]
    fn q_quits() {
        let mut app = ready_app();
        app.handle(&press(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.should_quit());
    }

    fn key(app: &mut App, code: KeyCode) {
        app.handle(&press(code, KeyModifiers::NONE));
    }

    /// Sets the start at the center, the end one screen east, and returns the route request.
    fn plan_route(app: &mut App) -> Request {
        key(app, KeyCode::Char('s'));
        key(app, KeyCode::Char('L'));
        key(app, KeyCode::Char('L'));
        key(app, KeyCode::Char('e'));
        *app.take_requests().last().unwrap()
    }

    fn route_id(request: Request) -> u64 {
        match request {
            Request::Route { id, .. } => id,
            Request::PlaceName { .. } => panic!("not a route request"),
        }
    }

    #[test]
    fn route_goes_from_the_start_to_the_end() {
        let mut app = ready_app();
        let end = View::new(milan(), 14.0).pan(200.0, 0.0).center();
        assert_eq!(
            plan_route(&mut app),
            Request::Route {
                id: 3,
                from: View::new(milan(), 14.0).center(),
                to: end,
                mode: Mode::Car
            }
        );
    }

    #[test]
    fn new_routes_fit_the_view() {
        let mut app = ready_app();
        let id = route_id(plan_route(&mut app));
        let rome = LatLon::new(41.9028, 12.4964).unwrap();
        app.receive(
            Message::Routes(id, Ok(vec![sample_route(&[milan(), rome])])),
            Instant::now(),
        );
        assert!(app.view().zoom() < 8.0);
    }

    #[test]
    fn bracket_centers_the_map_on_the_next_step() {
        let mut app = ready_app();
        let id = route_id(plan_route(&mut app));
        let step = LatLon::new(45.47, 9.2).unwrap();
        app.receive(
            Message::Routes(id, Ok(vec![sample_route(&[step, milan()])])),
            Instant::now(),
        );
        key(&mut app, KeyCode::Char(']'));
        assert_eq!(app.view(), View::new(step, STEP_ZOOM));
    }

    #[test]
    fn route_error_shows_before_a_tile_error() {
        let mut app = ready_app();
        let id = route_id(plan_route(&mut app));
        let now = Instant::now();
        app.receive(
            Message::Tile(TileId { z: 14, x: 0, y: 0 }, Err("timeout".to_owned())),
            now,
        );
        app.receive(Message::Routes(id, Err("no route found".to_owned())), now);
        assert_eq!(app.error(), Some("no route found"));
    }

    #[test]
    fn p_hides_the_panel() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('s'));
        key(&mut app, KeyCode::Char('p'));
        assert!(!app.panel_visible());
    }

    #[test]
    fn p_without_points_does_not_hide_the_next_panel() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('p'));
        key(&mut app, KeyCode::Char('s'));
        assert!(app.panel_visible());
    }

    #[test]
    fn alt_c_does_not_clear() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('s'));
        app.handle(&press(KeyCode::Char('c'), KeyModifiers::ALT));
        assert!(!app.trip().is_empty());
    }

    #[test]
    fn panel_is_hidden_without_points() {
        assert!(!ready_app().panel_visible());
    }

    #[test]
    fn ctrl_c_quits_and_c_clears() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('s'));
        key(&mut app, KeyCode::Char('c'));
        let cleared = app.trip().is_empty() && !app.should_quit();
        app.handle(&press(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(cleared && app.should_quit());
    }

    #[test]
    fn no_tiles_load_before_the_source_is_ready() {
        let mut app = App::new(View::new(milan(), 14.0));
        app.set_map_size(100, 40);
        assert_eq!(app.tiles_to_load(Instant::now()), Vec::<TileId>::new());
    }

    #[test]
    fn tiles_to_load_starts_with_the_center_tile() {
        let mut app = ready_app();
        let tiles = app.tiles_to_load(Instant::now());
        assert_eq!(
            tiles[0],
            TileId {
                z: 14,
                x: 8610,
                y: 5863
            }
        );
    }

    #[test]
    fn pending_tiles_are_not_requested_again() {
        let mut app = ready_app();
        let now = Instant::now();
        app.tiles_to_load(now);
        assert_eq!(app.tiles_to_load(now), Vec::<TileId>::new());
    }

    #[test]
    fn failed_tile_waits_before_a_retry() {
        let mut app = ready_app();
        let now = Instant::now();
        let id = app.tiles_to_load(now)[0];
        app.receive(Message::Tile(id, Err("offline".to_owned())), now);
        let soon = app.tiles_to_load(now + Duration::from_secs(1));
        let later = app.tiles_to_load(now + RETRY_AFTER);
        assert_eq!((soon.contains(&id), later.contains(&id)), (false, true));
    }

    #[test]
    fn skipped_tile_is_requested_again_when_visible() {
        let mut app = ready_app();
        let now = Instant::now();
        let id = app.tiles_to_load(now)[0];
        app.receive(Message::Skipped(id), now);
        assert!(app.tiles_to_load(now).contains(&id));
    }

    #[test]
    fn source_failure_is_retried_after_a_wait() {
        let mut app = App::new(View::new(milan(), 14.0));
        let now = Instant::now();
        app.wants_source(now);
        app.receive(Message::Source(Err("offline".to_owned())), now);
        assert_eq!(
            (app.wants_source(now), app.wants_source(now + RETRY_AFTER)),
            (false, true)
        );
    }
}
