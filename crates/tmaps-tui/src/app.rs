use std::collections::{BTreeMap, HashMap, HashSet};
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use lru::LruCache;
use ratatui::layout::{Position, Rect};
use tmaps_core::LatLon;
use tmaps_core::geocode::{Place, PlaceKind};
use tmaps_core::places::validate_name;
use tmaps_core::render::{
    Canvas, DOTS_PER_CELL_X, DOTS_PER_CELL_Y, PreparedTile, RouteLine, View, render,
};
use tmaps_core::routing::{Mode, Route};
use tmaps_core::tiles::TileId;

use crate::message::{Message, SourceInfo};
use crate::prompt::{self, Prompt, Search};
use crate::regions::Regions;
use crate::services::Request;
use crate::trip::{End, Routes, Trip};

/// Holds more tiles than the largest screen shows, so that a pan back does not reload them.
const TILE_CACHE_SIZE: NonZeroUsize = NonZeroUsize::new(64).unwrap();
const RETRY_AFTER: Duration = Duration::from_secs(10);
const PAN_STEP: f64 = 0.1;
const FAST_PAN_STEP: f64 = 0.5;
/// A step of the itinerary is a street corner, so the map zooms in at least to street level.
const STEP_ZOOM: f64 = 16.0;
/// A place without an extent or a kind, such as a saved place, shows its neighborhood.
const PLACE_ZOOM: f64 = 15.0;
/// A deeper fit of a single building shows no context around it.
const HOUSE_ZOOM: f64 = 17.0;
const INFO_TIME: Duration = Duration::from_secs(3);

pub struct App {
    view: View,
    map_size: (u16, u16),
    source: SourceState,
    tiles: LruCache<TileId, PreparedTile>,
    pending: HashSet<TileId>,
    failed: HashMap<TileId, Failure>,
    source_error: Option<String>,
    help: bool,
    /// The help rows above the visible part.
    help_scroll: usize,
    /// The help rows that do not fit on the screen.
    help_overflow: usize,
    quit: bool,
    drag_from: Option<(u16, u16)>,
    regions: Regions,
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

struct Failure {
    at: Instant,
    error: String,
}

impl Failure {
    fn retry_at(&self) -> Instant {
        self.at + RETRY_AFTER
    }
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
            source_error: None,
            help: false,
            help_scroll: 0,
            help_overflow: 0,
            quit: false,
            drag_from: None,
            regions: Regions::default(),
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

    /// Where the last frame drew the parts that take mouse clicks.
    pub fn set_regions(&mut self, regions: Regions) {
        self.regions = regions;
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
            .or(self
                .source_error
                .as_deref()
                .map(|error| (Level::Error, error)))
            .or(self.tile_error().map(|error| (Level::Error, error)))
    }

    fn tile_error(&self) -> Option<&str> {
        self.failed
            .values()
            .max_by_key(|failure| failure.at)
            .map(|failure| failure.error.as_str())
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
                self.source_error = None;
            }
            Message::Source(Err(error)) => {
                self.source = SourceState::Failed(now);
                self.source_error = Some(error);
            }
            Message::Tile(id, Ok(tile)) => {
                self.pending.remove(&id);
                self.failed.remove(&id);
                self.tiles.put(id, *tile);
            }
            Message::Tile(id, Err(error)) => {
                self.pending.remove(&id);
                self.failed.insert(id, Failure { at: now, error });
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
                self.refresh_saved_places();
            }
            Message::PlaceRemoved(name) => {
                self.notice = Some(Notice {
                    level: Level::Info,
                    text: format!("Removed @{name}"),
                    until: Some(now + INFO_TIME),
                });
                self.places.remove(&name);
                self.refresh_saved_places();
            }
            Message::PlaceFailed(error) => {
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
        let visible = self.visible_tiles();
        // A failure stays during its retry, so that its error does not blink.
        self.failed.retain(|id, _| visible.contains(id));
        let mut load = Vec::new();
        for id in visible {
            // `get`, not `contains`: a visible tile becomes the most recently used.
            if self.tiles.get(&id).is_some()
                || self.pending.contains(&id)
                || self
                    .failed
                    .get(&id)
                    .is_some_and(|failure| now < failure.retry_at())
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
            .iter()
            .filter(|(id, _)| !self.pending.contains(id))
            .map(|(_, failure)| failure.retry_at())
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
                self.scroll_help(-1);
            }
            KeyCode::Down | KeyCode::Char('j') if self.help && self.help_overflow > 0 => {
                self.scroll_help(1);
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
            KeyCode::Char('m') => self.open_save_place(self.view.center(), None, None),
            KeyCode::Char('@' | '\'') => self.open_saved_places(),
            _ => return false,
        }
        true
    }

    fn on_prompt_key(&mut self, key: KeyEvent) -> bool {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return true;
        }
        // A key that only cancels the question still changes the screen.
        match self.prompt.as_mut().and_then(Prompt::take_removal) {
            Some(name) if key.code == KeyCode::Enter => {
                self.requests.push(Request::RemovePlace { name });
            }
            Some(_) if key.code == KeyCode::Esc => {}
            Some(_) => {
                self.on_prompt_input(key);
            }
            None if key.code == KeyCode::Esc => self.leave_prompt(),
            None => return self.on_prompt_input(key),
        }
        true
    }

    fn on_prompt_input(&mut self, key: KeyEvent) -> bool {
        let (width, height) = self.map_size_dots();
        let Some(prompt) = &mut self.prompt else {
            return false;
        };
        match prompt {
            Prompt::Search(search) => {
                let control = key.modifiers.contains(KeyModifiers::CONTROL);
                let saved = search.saved_query().is_some();
                if key.code == KeyCode::Char('d') && control && saved {
                    search.removing = search.chosen().and_then(|place| place.name.clone());
                } else if key.code == KeyCode::Char('s') && control && !saved {
                    let Some(place) = search.chosen() else {
                        return false;
                    };
                    let (location, label) = (place.location, place.label.clone());
                    let search = std::mem::take(search);
                    self.open_save_place(location, Some(label), Some(search));
                } else if key.code == KeyCode::Enter {
                    let view = search
                        .chosen()
                        .map(|place| place_view(place, width, height));
                    self.close_with(view);
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
            Prompt::SavePlace {
                name,
                invalid,
                location,
                ..
            } => {
                if key.code == KeyCode::Enter {
                    match validate_name(name) {
                        Ok(valid) => {
                            let name = valid.to_owned();
                            let location = *location;
                            self.save_place(name, location);
                        }
                        Err(_) => *invalid = true,
                    }
                } else if prompt::edit(name, key) {
                    *invalid = false;
                } else {
                    return false;
                }
            }
        }
        true
    }

    fn refresh_saved_places(&mut self) {
        let Some(Prompt::Search(search)) = &mut self.prompt else {
            return;
        };
        if let Some(results) = search
            .saved_query()
            .map(|query| saved_places(&self.places, query))
        {
            let chosen = search.chosen().and_then(|place| place.name.clone());
            search.results = results;
            search.selected = search
                .results
                .iter()
                .position(|place| place.name == chosen)
                .unwrap_or_else(|| search.selected.min(search.results.len().saturating_sub(1)));
            search
                .removing
                .take_if(|name| !self.places.contains_key(name));
        }
    }

    fn open_saved_places(&mut self) {
        self.open(Prompt::Search(Search {
            text: "@".to_owned(),
            ..Search::default()
        }));
        self.request_search();
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

    fn close_with(&mut self, view: Option<View>) {
        self.close_prompt();
        if let Some(view) = view {
            self.view = view;
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
        let saved = search
            .saved_query()
            .map(|query| saved_places(&self.places, query));
        if search.text.trim().is_empty() || saved.is_some() {
            if pending {
                self.requests.push(Request::CancelSearch);
            }
            search.results = saved.unwrap_or_default();
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

    fn open_save_place(&mut self, location: LatLon, label: Option<String>, search: Option<Search>) {
        self.close_prompt();
        self.open(Prompt::SavePlace {
            name: String::new(),
            invalid: false,
            location: rounded(location),
            label,
            search,
        });
    }

    fn leave_prompt(&mut self) {
        match self.prompt.take() {
            Some(Prompt::SavePlace {
                search: Some(search),
                ..
            }) => self.prompt = Some(Prompt::Search(search)),
            prompt => {
                self.prompt = prompt;
                self.close_prompt();
            }
        }
    }

    fn save_place(&mut self, name: String, location: LatLon) {
        self.close_prompt();
        self.requests.push(Request::SavePlace { name, location });
    }

    fn on_mouse(&mut self, mouse: MouseEvent) -> bool {
        let position = Position::new(mouse.column, mouse.row);
        if mouse.kind == MouseEventKind::Up(MouseButton::Left) {
            self.drag_from = None;
            return false;
        }
        if self.prompt.is_some() {
            return self.on_prompt_mouse(mouse.kind, position);
        }
        let over = |area: Option<Rect>| area.is_some_and(|area| area.contains(position));
        let on_help = self.help && over(self.regions.help);
        let scrolls_help = on_help && self.help_overflow > 0;
        let on_panel = !on_help && over(self.regions.panel);
        let on_map = !scrolls_help && self.regions.map.contains(position);
        match mouse.kind {
            MouseEventKind::ScrollUp if scrolls_help => return self.scroll_help(-1),
            MouseEventKind::ScrollDown if scrolls_help => return self.scroll_help(1),
            MouseEventKind::ScrollUp if on_panel => self.show_step(-1),
            MouseEventKind::ScrollDown if on_panel => self.show_step(1),
            MouseEventKind::ScrollUp if on_map => self.view = self.view.zoom_by(1.0),
            MouseEventKind::ScrollDown if on_map => self.view = self.view.zoom_by(-1.0),
            MouseEventKind::Down(MouseButton::Left) => {
                let drags = on_map && !on_help;
                self.drag_from = drags.then_some((mouse.column, mouse.row));
                let step = self
                    .regions
                    .steps
                    .as_ref()
                    .and_then(|steps| steps.item_at(position).filter(|_| on_panel));
                let Some(location) = step.and_then(|index| self.trip.select_step(index)) else {
                    return false;
                };
                self.center_on_step(location);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(from) = self.drag_from.as_mut() else {
                    return false;
                };
                let (column, row) = std::mem::replace(from, (mouse.column, mouse.row));
                // The map follows the pointer, so the view moves the other way.
                self.view = self.view.pan(
                    (f64::from(column) - f64::from(mouse.column)) * f64::from(DOTS_PER_CELL_X),
                    (f64::from(row) - f64::from(mouse.row)) * f64::from(DOTS_PER_CELL_Y),
                );
            }
            _ => return false,
        }
        true
    }

    /// The wheel moves the selection like the arrows. A click selects an item, and a click on
    /// the selected item chooses it like Enter.
    fn on_prompt_mouse(&mut self, kind: MouseEventKind, position: Position) -> bool {
        let key = match kind {
            MouseEventKind::ScrollUp => KeyCode::Up,
            MouseEventKind::ScrollDown => KeyCode::Down,
            MouseEventKind::Down(MouseButton::Left) => {
                // Else a click on the selected row turns into an Enter that confirms the removal.
                let asked = self
                    .prompt
                    .as_mut()
                    .and_then(Prompt::take_removal)
                    .is_some();
                let item = self
                    .regions
                    .prompt_items
                    .as_ref()
                    .and_then(|items| items.item_at(position));
                let selected = self.prompt.as_mut().and_then(Prompt::selected_mut);
                let (Some(item), Some(selected)) = (item, selected) else {
                    return asked;
                };
                if *selected != item {
                    *selected = item;
                    return true;
                }
                KeyCode::Enter
            }
            _ => return false,
        };
        self.on_prompt_key(KeyEvent::from(key))
    }

    /// Returns `true` when the help moved.
    fn scroll_help(&mut self, delta: isize) -> bool {
        let scroll = self
            .help_scroll
            .saturating_add_signed(delta)
            .min(self.help_overflow);
        std::mem::replace(&mut self.help_scroll, scroll) != scroll
    }

    fn show_step(&mut self, delta: isize) {
        if let Some(location) = self.trip.move_step(delta) {
            self.center_on_step(location);
        }
    }

    fn center_on_step(&mut self, location: LatLon) {
        self.view = View::new(location, self.view.zoom().max(STEP_ZOOM));
    }

    /// Pans by a fraction of the map width and height.
    fn pan_screens(&mut self, x: f64, y: f64) {
        let (width, height) = self.map_size_dots();
        self.view = self.view.pan(x * width, y * height);
    }
}

fn saved_places(places: &BTreeMap<String, LatLon>, query: &str) -> Vec<Place> {
    let query = query.to_lowercase();
    let (starts, contains): (Vec<_>, Vec<_>) = places
        .iter()
        .map(|(name, location)| (name.to_lowercase(), name, *location))
        .filter(|(lower, ..)| lower.contains(&query))
        .partition(|(lower, ..)| lower.starts_with(&query));
    starts
        .into_iter()
        .chain(contains)
        .map(|(_, name, location)| Place {
            label: format!("@{name}"),
            name: Some(name.clone()),
            location,
            category: None,
            country_code: None,
            kind: None,
            extent: None,
        })
        .collect()
}

fn place_view(place: &Place, width: f64, height: f64) -> View {
    place
        .extent
        .and_then(|extent| View::fitting(&[extent.north_west, extent.south_east], width, height))
        .map_or_else(
            || View::new(place.location, kind_zoom(place.kind)),
            |view| View::new(view.center(), view.zoom().min(HOUSE_ZOOM)),
        )
}

fn kind_zoom(kind: Option<PlaceKind>) -> f64 {
    match kind {
        Some(PlaceKind::Country) => 5.0,
        Some(PlaceKind::State) => 7.0,
        Some(PlaceKind::County) => 9.0,
        Some(PlaceKind::City) => 12.0,
        Some(PlaceKind::District | PlaceKind::Locality) => 14.0,
        Some(PlaceKind::Street) => 16.0,
        Some(PlaceKind::House) => HOUSE_ZOOM,
        Some(PlaceKind::Other) | None => PLACE_ZOOM,
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

    use tmaps_core::geocode::Extent;

    use super::*;
    use crate::regions::ListRegion;
    use crate::trip::sample_route;

    fn milan() -> LatLon {
        LatLon::new(45.4642, 9.19).unwrap()
    }

    fn ready_app() -> App {
        let mut app = App::new(View::new(milan(), 14.0));
        app.set_map_size(100, 40);
        app.set_regions(Regions {
            map: Rect::new(0, 0, 100, 40),
            ..Regions::default()
        });
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

    /// A map on the left, and a panel with one row for each step on the right.
    fn split_regions(steps: usize) -> Regions {
        Regions {
            map: Rect::new(0, 0, 60, 40),
            panel: Some(Rect::new(60, 0, 40, 40)),
            steps: Some(ListRegion {
                area: Rect::new(61, 10, 38, 29),
                offset: 0,
                heights: vec![1; steps],
            }),
            ..Regions::default()
        }
    }

    fn app_with_steps(steps: &[LatLon]) -> App {
        let mut app = ready_app();
        let id = route_id(&plan_route(&mut app));
        app.receive(
            Message::Routes(id, Ok(vec![sample_route(steps)])),
            Instant::now(),
        );
        app.set_regions(split_regions(steps.len()));
        app
    }

    #[test]
    fn click_on_a_step_centers_the_map_on_it() {
        let step = LatLon::new(45.47, 9.2).unwrap();
        let mut app = app_with_steps(&[milan(), step]);
        app.handle(&mouse(MouseEventKind::Down(MouseButton::Left), 70, 11));
        assert_eq!(app.view(), View::new(step, STEP_ZOOM));
    }

    #[test]
    fn wheel_over_the_panel_shows_the_next_step() {
        let step = LatLon::new(45.47, 9.2).unwrap();
        let mut app = app_with_steps(&[step, milan()]);
        app.handle(&mouse(MouseEventKind::ScrollDown, 70, 30));
        assert_eq!(app.view(), View::new(step, STEP_ZOOM));
    }

    #[test]
    fn drag_from_the_panel_does_not_pan() {
        let mut app = ready_app();
        app.set_regions(split_regions(0));
        app.handle(&mouse(MouseEventKind::Down(MouseButton::Left), 70, 20));
        app.handle(&mouse(MouseEventKind::Drag(MouseButton::Left), 50, 20));
        app.handle(&mouse(MouseEventKind::Drag(MouseButton::Left), 30, 20));
        assert_eq!(app.view(), View::new(milan(), 14.0));
    }

    #[test]
    fn wheel_scrolls_an_overflowing_help() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('?'));
        app.set_help_overflow(3);
        app.set_regions(Regions {
            help: Some(Rect::new(20, 5, 40, 20)),
            ..app.regions.clone()
        });
        app.handle(&mouse(MouseEventKind::ScrollDown, 30, 10));
        assert_eq!(
            (app.help_scroll(), app.view()),
            (1, View::new(milan(), 14.0))
        );
    }

    #[test]
    fn wheel_over_a_help_that_fits_zooms_the_map() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('?'));
        app.set_regions(Regions {
            help: Some(Rect::new(20, 5, 40, 20)),
            ..app.regions.clone()
        });
        app.handle(&mouse(MouseEventKind::ScrollUp, 30, 10));
        assert_eq!(app.view(), View::new(milan(), 15.0));
    }

    #[test]
    fn wheel_over_the_status_line_does_not_zoom() {
        let mut app = ready_app();
        app.set_regions(Regions {
            map: Rect::new(0, 0, 100, 39),
            ..Regions::default()
        });
        app.handle(&mouse(MouseEventKind::ScrollUp, 30, 39));
        assert_eq!(app.view(), View::new(milan(), 14.0));
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
            kind: None,
            extent: None,
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

    fn go_to(app: &mut App, place: Place) {
        key(app, KeyCode::Char('/'));
        type_text(app, "x");
        let id = last_search(app);
        app.receive(Message::SearchResults(id, Ok(vec![place])), Instant::now());
        key(app, KeyCode::Enter);
    }

    #[test]
    fn a_country_fits_the_screen() {
        let mut app = ready_app();
        app.view = View::new(milan(), 16.0);
        let italy = Place {
            extent: Some(Extent {
                north_west: LatLon::new(47.1, 6.6).unwrap(),
                south_east: LatLon::new(35.3, 18.8).unwrap(),
            }),
            ..place("Italia", 42.6, 12.7)
        };
        go_to(&mut app, italy);
        let (width, height) = app.map_size_dots();
        let corner = |lat, lon| {
            app.view()
                .screen_position(LatLon::new(lat, lon).unwrap(), width, height)
        };
        let ((west, north), (east, south)) = (corner(47.1, 6.6), corner(35.3, 18.8));
        assert!(
            app.view().zoom() < 6.0
                && west >= 0.0
                && north >= 0.0
                && east <= width
                && south <= height,
            "{:?}",
            app.view()
        );
    }

    #[test]
    fn a_building_does_not_zoom_past_the_house_zoom() {
        let mut app = ready_app();
        let kiosk = Place {
            extent: Some(Extent {
                north_west: LatLon::new(45.4643, 9.1915).unwrap(),
                south_east: LatLon::new(45.4642, 9.1916).unwrap(),
            }),
            ..place("Edicola", 45.4642, 9.1916)
        };
        go_to(&mut app, kiosk);
        assert_eq!(app.view().zoom(), HOUSE_ZOOM);
    }

    #[test]
    fn the_kind_sets_the_zoom_without_an_extent() {
        let mut app = ready_app();
        app.view = View::new(milan(), 16.0);
        let rome = Place {
            kind: Some(PlaceKind::City),
            ..place("Roma", 41.9, 12.5)
        };
        go_to(&mut app, rome);
        assert_eq!(
            app.view(),
            View::new(LatLon::new(41.9, 12.5).unwrap(), 12.0)
        );
    }

    /// A search with two results, drawn in rows 3 and 4.
    fn app_with_results() -> App {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "Roma");
        let id = last_search(&mut app);
        let results = vec![
            place("Roma, Lazio", 41.9, 12.5),
            place("Roma, Texas", 26.4, -99.0),
        ];
        app.receive(Message::SearchResults(id, Ok(results)), Instant::now());
        app.set_regions(Regions {
            prompt_items: Some(ListRegion {
                area: Rect::new(20, 3, 58, 2),
                offset: 0,
                heights: vec![1, 1],
            }),
            ..app.regions.clone()
        });
        app
    }

    #[test]
    fn second_click_on_a_result_goes_there() {
        let mut app = app_with_results();
        let click = mouse(MouseEventKind::Down(MouseButton::Left), 30, 4);
        app.handle(&click);
        let open_after_one_click = app.prompt().is_some();
        app.handle(&click);
        assert_eq!(
            (open_after_one_click, app.view()),
            (true, View::new(place("", 26.4, -99.0).location, 15.0))
        );
    }

    #[test]
    fn wheel_moves_the_selected_result() {
        let mut app = app_with_results();
        app.handle(&mouse(MouseEventKind::ScrollDown, 50, 30));
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.view(), View::new(place("", 26.4, -99.0).location, 15.0));
    }

    #[test]
    fn drag_under_a_prompt_does_not_pan() {
        let mut app = app_with_results();
        app.handle(&mouse(MouseEventKind::Down(MouseButton::Left), 50, 30));
        app.handle(&mouse(MouseEventKind::Drag(MouseButton::Left), 40, 30));
        assert_eq!(app.view(), View::new(milan(), 14.0));
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
    fn ctrl_s_saves_the_selected_search_result() {
        let mut app = app_with_results();
        key(&mut app, KeyCode::Down);
        app.handle(&press(KeyCode::Char('s'), KeyModifiers::CONTROL));
        type_text(&mut app, "texas");
        key(&mut app, KeyCode::Enter);
        assert_eq!(
            app.take_requests(),
            [Request::SavePlace {
                name: "texas".to_owned(),
                location: LatLon::new(26.4, -99.0).unwrap()
            }]
        );
        assert_eq!(app.view(), View::new(milan(), 14.0));
    }

    #[test]
    fn ctrl_s_in_a_saved_place_search_saves_nothing() {
        let mut app = app_with_places(&["home"]);
        key(&mut app, KeyCode::Char('\''));
        app.take_requests();
        app.handle(&press(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert_matches!(app.prompt(), Some(Prompt::Search(search)) if search.text == "@");
        assert_eq!(app.take_requests(), []);
    }

    #[test]
    fn ctrl_s_without_a_result_saves_nothing() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('/'));
        app.handle(&press(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert_matches!(app.prompt(), Some(Prompt::Search(search)) if search.text.is_empty());
    }

    #[test]
    fn ctrl_s_rounds_the_location_of_the_result() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "Roma");
        let id = last_search(&mut app);
        let roma = place("Roma", 41.902_782_4, 12.496_365_7);
        app.receive(Message::SearchResults(id, Ok(vec![roma])), Instant::now());
        app.handle(&press(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert_matches!(
            app.prompt(),
            Some(Prompt::SavePlace { location, .. })
                if *location == LatLon::new(41.902_782, 12.496_366).unwrap()
        );
    }

    #[test]
    fn key_that_only_cancels_the_removal_redraws_the_screen() {
        let mut app = app_with_places(&["home"]);
        key(&mut app, KeyCode::Char('\''));
        ctrl_d(&mut app);
        let redraw = app.handle(&press(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(redraw && removing(&app).is_none());
    }

    #[test]
    fn esc_in_the_name_box_goes_back_to_the_search() {
        let mut app = app_with_results();
        key(&mut app, KeyCode::Down);
        app.handle(&press(KeyCode::Char('s'), KeyModifiers::CONTROL));
        key(&mut app, KeyCode::Esc);
        assert_matches!(
            app.prompt(),
            Some(Prompt::Search(search)) if search.text == "Roma" && search.selected == 1
        );
        assert_eq!(result_labels(&app), ["Roma, Lazio", "Roma, Texas"]);
    }

    #[test]
    fn esc_in_the_name_box_of_the_crosshair_closes_it() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('m'));
        key(&mut app, KeyCode::Esc);
        assert!(app.prompt().is_none());
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
    fn a_saved_place_zooms_out_to_its_neighborhood() {
        let mut app = ready_app();
        app.view = View::new(milan(), 18.0);
        let rome = LatLon::new(41.9, 12.5).unwrap();
        app.receive(Message::PlaceSaved("rome".to_owned(), rome), Instant::now());
        key(&mut app, KeyCode::Char('\''));
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.view(), View::new(rome, PLACE_ZOOM));
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
            Message::PlaceFailed("cannot save @work".to_owned()),
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
    fn saved_place_search_closes_the_help() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('?'));
        key(&mut app, KeyCode::Char('\''));
        assert!(!app.help());
    }

    fn removing(app: &App) -> Option<String> {
        let Some(Prompt::Search(search)) = app.prompt() else {
            panic!("the search is not open");
        };
        search.removing.clone()
    }

    fn ctrl_d(app: &mut App) {
        app.handle(&press(KeyCode::Char('d'), KeyModifiers::CONTROL));
    }

    #[test]
    fn ctrl_d_asks_before_it_removes_a_saved_place() {
        let mut app = app_with_places(&["home", "work"]);
        key(&mut app, KeyCode::Char('\''));
        key(&mut app, KeyCode::Down);
        ctrl_d(&mut app);
        assert_eq!(
            (removing(&app), app.take_requests()),
            (Some("work".to_owned()), Vec::new())
        );
    }

    #[test]
    fn enter_confirms_the_removal_and_keeps_the_search_open() {
        let mut app = app_with_places(&["home"]);
        key(&mut app, KeyCode::Char('\''));
        ctrl_d(&mut app);
        key(&mut app, KeyCode::Enter);
        assert_eq!(
            app.take_requests(),
            [Request::RemovePlace {
                name: "home".to_owned()
            }]
        );
        assert_eq!(removing(&app), None);
    }

    #[test]
    fn esc_keeps_the_saved_place_and_the_search() {
        let mut app = app_with_places(&["home"]);
        key(&mut app, KeyCode::Char('\''));
        ctrl_d(&mut app);
        key(&mut app, KeyCode::Esc);
        assert_eq!((removing(&app), app.take_requests()), (None, Vec::new()));
    }

    #[test]
    fn another_key_cancels_the_removal() {
        let mut app = app_with_places(&["home", "work"]);
        key(&mut app, KeyCode::Char('\''));
        ctrl_d(&mut app);
        key(&mut app, KeyCode::Down);
        assert_eq!(removing(&app), None);
    }

    #[test]
    fn ctrl_d_in_a_place_search_removes_nothing() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "Roma");
        let id = last_search(&mut app);
        app.receive(
            Message::SearchResults(id, Ok(vec![place("Roma", 41.9, 12.5)])),
            Instant::now(),
        );
        ctrl_d(&mut app);
        assert_eq!(removing(&app), None);
    }

    #[test]
    fn click_on_the_selected_place_does_not_confirm_the_removal() {
        let mut app = app_with_places(&["home", "work"]);
        key(&mut app, KeyCode::Char('\''));
        app.set_regions(Regions {
            prompt_items: Some(ListRegion {
                area: Rect::new(20, 3, 58, 2),
                offset: 0,
                heights: vec![1, 1],
            }),
            ..app.regions.clone()
        });
        ctrl_d(&mut app);
        app.handle(&mouse(MouseEventKind::Down(MouseButton::Left), 30, 3));
        assert!(
            !app.take_requests()
                .iter()
                .any(|request| matches!(request, Request::RemovePlace { .. }))
        );
    }

    #[test]
    fn question_about_a_removed_place_goes_away() {
        let mut app = app_with_places(&["home", "work"]);
        key(&mut app, KeyCode::Char('\''));
        ctrl_d(&mut app);
        app.receive(Message::PlaceRemoved("home".to_owned()), Instant::now());
        assert_eq!(removing(&app), None);
    }

    #[test]
    fn removal_shows_a_notice() {
        let mut app = app_with_places(&["home"]);
        app.receive(Message::PlaceRemoved("home".to_owned()), Instant::now());
        assert_eq!(app.status(), Some((Level::Info, "Removed @home")));
    }

    #[test]
    fn removed_place_leaves_the_list() {
        let mut app = app_with_places(&["home", "work"]);
        key(&mut app, KeyCode::Char('\''));
        key(&mut app, KeyCode::Down);
        app.receive(Message::PlaceRemoved("work".to_owned()), Instant::now());
        let Some(Prompt::Search(search)) = app.prompt() else {
            panic!("the search is not open");
        };
        assert_eq!(
            (result_labels(&app), search.selected, app.places().len()),
            (vec!["@home".to_owned()], 0, 1)
        );
    }

    #[test]
    fn backspace_on_the_at_sign_turns_to_a_place_search() {
        let mut app = app_with_places(&["home"]);
        key(&mut app, KeyCode::Char('\''));
        key(&mut app, KeyCode::Backspace);
        type_text(&mut app, "Roma");
        assert_matches!(
            app.take_requests().last(),
            Some(Request::Search { text, .. }) if text == "Roma"
        );
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
            Message::PlaceFailed("cannot save @work".to_owned()),
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
        app.receive(Message::PlaceFailed("Cannot save @work".to_owned()), now);
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
    fn quote_goes_to_the_selected_saved_place() {
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

    fn app_with_places(names: &[&str]) -> App {
        let places = names
            .iter()
            .map(|name| ((*name).to_owned(), milan()))
            .collect();
        ready_app().with_places(places)
    }

    fn result_labels(app: &App) -> Vec<String> {
        let Some(Prompt::Search(search)) = app.prompt() else {
            panic!("the search is not open");
        };
        search
            .results
            .iter()
            .map(|place| place.label.clone())
            .collect()
    }

    #[test]
    fn at_sign_lists_the_saved_places_that_start_with_the_text_first() {
        let mut app = app_with_places(&["home", "office", "omega", "rome"]);
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "@om");
        assert_eq!(result_labels(&app), ["@omega", "@home", "@rome"]);
    }

    #[test]
    fn spaces_after_the_at_sign_change_nothing() {
        let mut app = app_with_places(&["home", "work"]);
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "@ ho");
        assert_eq!(result_labels(&app), ["@home"]);
    }

    #[test]
    fn a_new_saved_place_keeps_the_selected_place() {
        let mut app = app_with_places(&["home", "work"]);
        key(&mut app, KeyCode::Char('\''));
        key(&mut app, KeyCode::Down);
        app.receive(
            Message::PlaceSaved("bar".to_owned(), milan()),
            Instant::now(),
        );
        let Some(Prompt::Search(search)) = app.prompt() else {
            panic!("the search is not open");
        };
        assert_eq!(
            search.chosen().map(|place| place.label.as_str()),
            Some("@work")
        );
    }

    #[test]
    fn at_sign_on_the_map_lists_the_saved_places() {
        let mut app = app_with_places(&["home", "work"]);
        app.handle(&press(KeyCode::Char('@'), KeyModifiers::SHIFT));
        assert_eq!(result_labels(&app), ["@home", "@work"]);
    }

    #[test]
    fn quote_opens_a_saved_place_search_that_filters() {
        let mut app = app_with_places(&["home", "work"]);
        key(&mut app, KeyCode::Char('\''));
        type_text(&mut app, "w");
        assert_eq!(result_labels(&app), ["@work"]);
    }

    #[test]
    fn at_sign_matches_the_saved_places_without_case() {
        let mut app = app_with_places(&["Home", "work"]);
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "@hO");
        assert_eq!(result_labels(&app), ["@Home"]);
    }

    #[test]
    fn bare_at_sign_lists_all_saved_places() {
        let mut app = app_with_places(&["home", "work"]);
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "@");
        assert_eq!(result_labels(&app), ["@home", "@work"]);
    }

    #[test]
    fn at_sign_sends_no_search_request() {
        let mut app = app_with_places(&["home"]);
        key(&mut app, KeyCode::Char('/'));
        app.take_requests();
        type_text(&mut app, "@home");
        assert_eq!(app.take_requests(), []);
    }

    #[test]
    fn open_at_sign_search_shows_a_place_saved_later() {
        let mut app = ready_app();
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "@");
        app.receive(
            Message::PlaceSaved("home".to_owned(), milan()),
            Instant::now(),
        );
        assert_eq!(result_labels(&app), ["@home"]);
    }

    #[test]
    fn new_at_sign_text_selects_the_first_saved_place() {
        let rome = LatLon::new(41.9, 12.5).unwrap();
        let mut app = ready_app().with_places(BTreeMap::from([
            ("home".to_owned(), milan()),
            ("rome".to_owned(), rome),
        ]));
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "@");
        key(&mut app, KeyCode::Down);
        type_text(&mut app, "r");
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.view(), View::new(rome, PLACE_ZOOM));
    }

    #[test]
    fn enter_goes_to_the_saved_place_from_the_search() {
        let rome = LatLon::new(41.9, 12.5).unwrap();
        let mut app = ready_app().with_places(BTreeMap::from([
            ("home".to_owned(), milan()),
            ("rome".to_owned(), rome),
        ]));
        key(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "@ro");
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.view(), View::new(rome, PLACE_ZOOM));
    }

    #[test]
    fn enter_without_saved_places_closes_the_search() {
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
    fn tile_error_stays_after_another_tile_loads() {
        let mut app = ready_app();
        let now = Instant::now();
        let ids = app.tiles_to_load(now);
        app.receive(Message::Tile(ids[0], Err("offline".to_owned())), now);
        app.receive(Message::Tile(ids[1], Ok(Box::default())), now);
        assert_eq!(app.status(), Some((Level::Error, "offline")));
    }

    #[test]
    fn tile_error_stays_during_the_retry_and_clears_when_the_tile_loads() {
        let mut app = ready_app();
        let now = Instant::now();
        let id = app.tiles_to_load(now)[0];
        app.receive(Message::Tile(id, Err("offline".to_owned())), now);
        let retry = app.tiles_to_load(now + RETRY_AFTER);
        let during = app.status().map(|(level, text)| (level, text.to_owned()));
        app.receive(Message::Tile(id, Ok(Box::default())), now + RETRY_AFTER);
        assert_eq!(
            (retry.contains(&id), during, app.status()),
            (true, Some((Level::Error, "offline".to_owned())), None)
        );
    }

    #[test]
    fn tile_error_clears_when_the_tile_leaves_the_screen() {
        let mut app = ready_app();
        let now = Instant::now();
        let id = app.tiles_to_load(now)[0];
        app.receive(Message::Tile(id, Err("offline".to_owned())), now);
        for _ in 0..10 {
            app.handle(&press(KeyCode::Char('L'), KeyModifiers::SHIFT));
        }
        let visible = app.tiles_to_load(now);
        assert_eq!((visible.contains(&id), app.status()), (false, None));
    }

    #[test]
    fn retried_tile_sets_no_deadline() {
        let mut app = ready_app();
        let now = Instant::now();
        let id = app.tiles_to_load(now)[0];
        app.receive(Message::Tile(id, Err("offline".to_owned())), now);
        app.tiles_to_load(now + RETRY_AFTER);
        assert_eq!(app.next_deadline(), None);
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
