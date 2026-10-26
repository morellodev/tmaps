use std::collections::{BTreeMap, HashMap, HashSet};
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use lru::LruCache;
use tmaps_core::LatLon;
use tmaps_core::places::validate_name;
use tmaps_core::render::{
    Canvas, DOTS_PER_CELL_X, DOTS_PER_CELL_Y, PreparedTile, RouteLine, View, render,
};
use tmaps_core::routing::{Mode, Route};
use tmaps_core::tiles::TileId;

use crate::message::{Message, SourceInfo};
use crate::prompt::{self, Prompt, Search};
use crate::services::Request;
use crate::trip::{End, Routes, Trip};

/// Holds more tiles than the largest screen shows, so that a pan back does not reload them.
const TILE_CACHE_SIZE: NonZeroUsize = NonZeroUsize::new(64).unwrap();
const RETRY_AFTER: Duration = Duration::from_secs(10);
const PAN_STEP: f64 = 0.1;
const FAST_PAN_STEP: f64 = 0.5;
/// A step of the itinerary is a street corner, so the map zooms in at least to street level.
const STEP_ZOOM: f64 = 16.0;
/// A search result or a saved place has no extent, so the map shows its neighborhood.
const PLACE_ZOOM: f64 = 15.0;
const INFO_TIME: Duration = Duration::from_secs(3);

pub struct App {
    view: View,
    map_size: (u16, u16),
    source: SourceState,
    tiles: LruCache<TileId, PreparedTile>,
    pending: HashSet<TileId>,
    failed: HashMap<TileId, Instant>,
    error: Option<String>,
    help: bool,
    /// The help rows above the visible part.
    help_scroll: usize,
    /// The help rows that do not fit on the screen.
    help_overflow: usize,
    quit: bool,
    drag_from: Option<(u16, u16)>,
    trip: Trip,
    panel: bool,
    prompt: Option<Prompt>,
    places: BTreeMap<String, LatLon>,
    last_search: u64,
    /// The answer to the last action of the user, until the next key.
    notice: Option<Notice>,
    /// Frames of the spinner since the start.
    ticks: usize,
    requests: Vec<Request>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Error,
}

struct Notice {
    level: Level,
    text: String,
    /// An info closes also after a time. An error stays until the next key.
    until: Option<Instant>,
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
            help_scroll: 0,
            help_overflow: 0,
            quit: false,
            drag_from: None,
            trip: Trip::default(),
            panel: true,
            prompt: None,
            places: BTreeMap::new(),
            last_search: 0,
            notice: None,
            ticks: 0,
            requests: Vec::new(),
        }
    }

    /// The saved places that `'` lists.
    #[must_use]
    pub fn with_places(mut self, places: BTreeMap<String, LatLon>) -> Self {
        self.places = places;
        self
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

    pub fn help_scroll(&self) -> usize {
        self.help_scroll
    }

    pub fn help_overflow(&self) -> usize {
        self.help_overflow
    }

    pub fn set_help_overflow(&mut self, rows: usize) {
        self.help_overflow = rows;
        self.help_scroll = self.help_scroll.min(rows);
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// The text for the status line. A notice first: it is the answer to the last action. A
    /// route error stays until the route changes, and the panel also shows it.
    pub fn status(&self) -> Option<(Level, &str)> {
        self.notice
            .as_ref()
            .map(|notice| (notice.level, notice.text.as_str()))
            .or(self.trip.error().map(|error| (Level::Error, error)))
            .or(self.error.as_deref().map(|error| (Level::Error, error)))
    }

    /// Tiles, a search, or a route are on the way.
    pub fn busy(&self) -> bool {
        self.loading()
            || matches!(&self.prompt, Some(Prompt::Search(search)) if search.pending.is_some())
            || matches!(self.trip.routes(), Routes::Loading)
    }

    pub fn tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
    }

    pub fn ticks(&self) -> usize {
        self.ticks
    }

    /// Closes an info notice after its time. Returns `true` when the screen must be drawn again.
    pub fn expire(&mut self, now: Instant) -> bool {
        let expired = self
            .notice
            .as_ref()
            .and_then(|notice| notice.until)
            .is_some_and(|until| now >= until);
        if expired {
            self.notice = None;
        }
        expired
    }

    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
    }

    pub fn places(&self) -> &BTreeMap<String, LatLon> {
        &self.places
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
        let mut requests = self.trip.take_requests();
        requests.append(&mut self.requests);
        requests
    }

    pub fn attribution(&self) -> Option<&str> {
        match &self.source {
            SourceState::Ready(info) => Some(&info.attribution),
            _ => None,
        }
    }

    pub fn loading(&self) -> bool {
        !self.pending.is_empty() || matches!(self.source, SourceState::Loading)
    }

    /// Returns `true` when the screen must be drawn again.
    pub fn handle(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let had_notice = self.notice.take().is_some();
                self.on_key(*key) || had_notice
            }
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
            Message::SearchResults(id, result) => {
                if let Some(Prompt::Search(search)) = &mut self.prompt
                    && search.pending == Some(id)
                {
                    search.pending = None;
                    search.selected = 0;
                    match result {
                        Ok(places) => search.results = places,
                        Err(error) => {
                            search.results.clear();
                            search.error = Some(error);
                        }
                    }
                }
            }
            Message::PlaceSaved(name, location) => {
                self.notice = Some(Notice {
                    level: Level::Info,
                    text: format!("Saved @{name}"),
                    until: Some(now + INFO_TIME),
                });
                self.places.insert(name, location);
            }
            Message::SaveFailed(error) => {
                self.notice = Some(Notice {
                    level: Level::Error,
                    text: error,
                    until: None,
                });
            }
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

    /// When the next failed request can be tried again, or an info notice closes.
    pub fn next_deadline(&self) -> Option<Instant> {
        let source_retry = match self.source {
            SourceState::Failed(at) => Some(at + RETRY_AFTER),
            _ => None,
        };
        self.failed
            .values()
            .map(|at| *at + RETRY_AFTER)
            .chain(source_retry)
            .chain(self.notice.as_ref().and_then(|notice| notice.until))
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
        if self.prompt.is_some() {
            return self.on_prompt_key(key);
        }
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
            KeyCode::Char('?') => {
                self.help = !self.help;
                self.help_scroll = 0;
            }
            KeyCode::Esc => self.help = false,
            KeyCode::Up | KeyCode::Char('k') if self.help && self.help_overflow > 0 => {
                self.help_scroll = self.help_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if self.help && self.help_overflow > 0 => {
                self.help_scroll = (self.help_scroll + 1).min(self.help_overflow);
            }
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
            KeyCode::Char('/') => self.open(Prompt::Search(Search::default())),
            KeyCode::Char('m') => self.open(Prompt::SavePlace {
                name: String::new(),
                invalid: false,
            }),
            KeyCode::Char('\'') => self.open(Prompt::Places { selected: 0 }),
            _ => return false,
        }
        true
    }

    fn on_prompt_key(&mut self, key: KeyEvent) -> bool {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return true;
        }
        if key.code == KeyCode::Esc {
            self.close_prompt();
            return true;
        }
        let Some(prompt) = &mut self.prompt else {
            return false;
        };
        match prompt {
            Prompt::Search(search) => {
                if key.code == KeyCode::Enter {
                    let location = search.chosen().map(|place| place.location);
                    self.close_at(location);
                } else if let Some(selected) =
                    prompt::select(search.selected, search.results.len(), key.code)
                {
                    search.selected = selected;
                } else if prompt::edit(&mut search.text, key) {
                    self.request_search();
                } else {
                    return false;
                }
            }
            Prompt::SavePlace { name, invalid } => {
                if key.code == KeyCode::Enter {
                    match validate_name(name) {
                        Ok(valid) => {
                            let name = valid.to_owned();
                            self.save_place(name);
                        }
                        Err(_) => *invalid = true,
                    }
                } else if prompt::edit(name, key) {
                    *invalid = false;
                } else {
                    return false;
                }
            }
            Prompt::Places { selected } => {
                if key.code == KeyCode::Enter {
                    let location = self.places.values().nth(*selected).copied();
                    self.close_at(location);
                } else if let Some(next) = prompt::select(*selected, self.places.len(), key.code) {
                    *selected = next;
                } else {
                    return false;
                }
            }
        }
        true
    }

    /// Help covers the prompt, so it closes.
    fn open(&mut self, prompt: Prompt) {
        self.help = false;
        self.prompt = Some(prompt);
    }

    fn close_prompt(&mut self) {
        if let Some(Prompt::Search(Search {
            pending: Some(_), ..
        })) = self.prompt.take()
        {
            self.requests.push(Request::CancelSearch);
        }
    }

    /// Closes the prompt and centers the map on `location`.
    fn close_at(&mut self, location: Option<LatLon>) {
        self.close_prompt();
        if let Some(location) = location {
            self.view = View::new(location, self.view.zoom().max(PLACE_ZOOM));
        }
    }

    fn request_search(&mut self) {
        let Some(Prompt::Search(search)) = &mut self.prompt else {
            return;
        };
        let pending = search.pending.is_some();
        *search = Search {
            text: std::mem::take(&mut search.text),
            ..Search::default()
        };
        if search.text.trim().is_empty() {
            if pending {
                self.requests.push(Request::CancelSearch);
            }
            return;
        }
        self.last_search += 1;
        search.pending = Some(self.last_search);
        self.requests.push(Request::Search {
            id: self.last_search,
            text: search.text.clone(),
            near: self.view.center(),
        });
    }

    fn save_place(&mut self, name: String) {
        let location = rounded(self.view.center());
        self.close_prompt();
        self.requests.push(Request::SavePlace { name, location });
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

/// To 6 decimals, about 0.1 m: more digits in the config file only add noise.
fn rounded(location: LatLon) -> LatLon {
    let round = |degrees: f64| (degrees * 1e6).round() / 1e6;
    LatLon::new(round(location.lat()), round(location.lon())).unwrap_or(location)
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
    use std::assert_matches;

    use tmaps_core::geocode::Place;

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
        app.take_requests().pop().unwrap()
    }

    fn route_id(request: &Request) -> u64 {
        match request {
            Request::Route { id, .. } => *id,
            _ => panic!("not a route request"),
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
        let id = route_id(&plan_route(&mut app));
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
        let id = route_id(&plan_route(&mut app));
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
        let id = route_id(&plan_route(&mut app));
        let now = Instant::now();
        app.receive(
            Message::Tile(TileId { z: 14, x: 0, y: 0 }, Err("timeout".to_owned())),
            now,
        );
        app.receive(Message::Routes(id, Err("No route found.".to_owned())), now);
        assert_eq!(app.status(), Some((Level::Error, "No route found.")));
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

    fn place(label: &str, lat: f64, lon: f64) -> Place {
        Place {
            label: label.to_owned(),
            name: None,
            location: LatLon::new(lat, lon).unwrap(),
            category: None,
            country_code: None,
        }
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            key(app, KeyCode::Char(c));
        }
    }

    /// The number of the last search request.
    fn last_search(app: &mut App) -> u64 {
        app.take_requests()
            .into_iter()
            .rev()
            .find_map(|request| match request {
                Request::Search { id, .. } => Some(id),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn typing_searches_near_the_center() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "Duomo");
        assert_eq!(
            app.take_requests().last(),
            Some(&Request::Search {
                id: 5,
                text: "Duomo".to_owned(),
                near: View::new(milan(), 14.0).center(),
            })
        );
    }

    #[test]
    fn search_keys_do_not_move_the_map() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "hjkl+q");
        assert!(app.view() == View::new(milan(), 14.0) && !app.should_quit());
    }

    #[test]
    fn blank_search_sends_no_request() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, " ");
        assert_eq!(app.take_requests(), []);
    }

    #[test]
    fn enter_centers_the_map_on_the_selected_result() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "Roma");
        let id = last_search(&mut app);
        let results = vec![
            place("Roma, Lazio", 41.9, 12.5),
            place("Roma, Texas", 26.4, -99.0),
        ];
        app.receive(Message::SearchResults(id, Ok(results)), Instant::now());
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert!(
            app.prompt().is_none()
                && app.view() == View::new(place("", 26.4, -99.0).location, 15.0)
        );
    }

    #[test]
    fn old_search_results_are_ignored() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "Ro");
        let old = last_search(&mut app);
        type_text(&mut app, "m");
        app.receive(
            Message::SearchResults(old, Ok(vec![place("Rovereto", 45.9, 11.0)])),
            Instant::now(),
        );
        assert_matches!(app.prompt(), Some(Prompt::Search(search)) if search.results.is_empty());
    }

    #[test]
    fn search_error_stays_in_the_search_box() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "Roma");
        let id = last_search(&mut app);
        app.receive(
            Message::SearchResults(id, Err("timeout".to_owned())),
            Instant::now(),
        );
        assert_matches!(
            app.prompt(),
            Some(Prompt::Search(search)) if search.error.as_deref() == Some("timeout")
        );
    }

    #[test]
    fn esc_closes_the_search_and_keeps_the_view() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "Roma");
        key(&mut app, KeyCode::Esc);
        assert!(app.prompt().is_none() && app.view() == View::new(milan(), 14.0));
    }

    #[test]
    fn m_saves_the_center_as_a_place() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('m'));
        type_text(&mut app, "work");
        key(&mut app, KeyCode::Enter);
        assert_eq!(
            app.take_requests(),
            [Request::SavePlace {
                name: "work".to_owned(),
                location: milan()
            }]
        );
    }

    #[test]
    fn saved_place_joins_the_list() {
        let mut app = ready_app();
        app.receive(
            Message::PlaceSaved("work".to_owned(), milan()),
            Instant::now(),
        );
        assert_eq!(app.places().get("work"), Some(&milan()));
    }

    #[test]
    fn enter_ignores_the_results_of_the_previous_text() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "Ro");
        let id = last_search(&mut app);
        app.receive(
            Message::SearchResults(id, Ok(vec![place("Rovereto", 45.9, 11.0)])),
            Instant::now(),
        );
        type_text(&mut app, "ma");
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.view(), View::new(milan(), 14.0));
    }

    #[test]
    fn esc_cancels_the_pending_search() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "Roma");
        app.take_requests();
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.take_requests(), [Request::CancelSearch]);
    }

    #[test]
    fn next_key_clears_the_search_error() {
        let mut app = ready_app();
        app.receive(
            Message::SaveFailed("cannot save @work".to_owned()),
            Instant::now(),
        );
        key(&mut app, KeyCode::Right);
        assert_eq!(app.status(), None);
    }

    #[test]
    fn arrows_scroll_the_help_up_to_its_end() {
        let mut app = ready_app();
        app.set_help_overflow(2);
        key(&mut app, KeyCode::Char('?'));
        for _ in 0..4 {
            key(&mut app, KeyCode::Down);
        }
        assert_eq!(app.help_scroll(), 2);
    }

    #[test]
    fn help_opens_at_its_top() {
        let mut app = ready_app();
        app.set_help_overflow(2);
        key(&mut app, KeyCode::Char('?'));
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Char('?'));
        key(&mut app, KeyCode::Char('?'));
        assert_eq!(app.help_scroll(), 0);
    }

    #[test]
    fn arrows_pan_under_a_help_that_fits() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('?'));
        key(&mut app, KeyCode::Down);
        assert_ne!(app.view(), View::new(milan(), 14.0));
    }

    #[test]
    fn search_closes_the_help() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('?'));
        key(&mut app, KeyCode::Char('/'));
        assert!(!app.help());
    }

    #[test]
    fn invalid_place_name_keeps_the_prompt_open() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('m'));
        type_text(&mut app, "my work");
        key(&mut app, KeyCode::Enter);
        assert_matches!(app.prompt(), Some(Prompt::SavePlace { invalid: true, .. }));
    }

    #[test]
    fn save_error_goes_to_the_status_line() {
        let mut app = ready_app();
        app.receive(
            Message::SaveFailed("cannot save @work".to_owned()),
            Instant::now(),
        );
        assert_eq!(app.status(), Some((Level::Error, "cannot save @work")));
    }

    #[test]
    fn save_error_shows_before_a_route_error() {
        let mut app = ready_app();
        let id = route_id(&plan_route(&mut app));
        let now = Instant::now();
        app.receive(Message::Routes(id, Err("No route found.".to_owned())), now);
        app.receive(Message::SaveFailed("Cannot save @work".to_owned()), now);
        assert_eq!(app.status(), Some((Level::Error, "Cannot save @work")));
    }

    #[test]
    fn saved_place_notice_closes_after_its_time() {
        let mut app = ready_app();
        let now = Instant::now();
        app.receive(Message::PlaceSaved("work".to_owned(), milan()), now);
        let shown = app.status() == Some((Level::Info, "Saved @work"));
        app.expire(now + INFO_TIME);
        assert!(shown && app.status().is_none());
    }

    #[test]
    fn pending_search_makes_the_app_busy() {
        let mut app = ready_app();
        let idle = !app.busy();
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "Roma");
        assert!(idle && app.busy());
    }

    #[test]
    fn quote_goes_to_the_selected_place() {
        let rome = LatLon::new(41.9, 12.5).unwrap();
        let mut app = ready_app().with_places(BTreeMap::from([
            ("home".to_owned(), milan()),
            ("rome".to_owned(), rome),
        ]));
        key(&mut app, KeyCode::Char('\''));
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.view(), View::new(rome, 15.0));
    }

    #[test]
    fn enter_without_places_closes_the_list() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('\''));
        key(&mut app, KeyCode::Enter);
        assert!(app.prompt().is_none() && app.view() == View::new(milan(), 14.0));
    }

    #[test]
    fn ctrl_c_quits_from_a_prompt() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('/'));
        app.handle(&press(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit());
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
