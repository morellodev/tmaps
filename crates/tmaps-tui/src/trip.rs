use std::mem;

use tmaps_core::LatLon;
use tmaps_core::routing::{Mode, Route};

use crate::message::no_route;
use crate::services::Request;

/// The start and end points, the travel mode, and the routes between the points.
#[derive(Debug, Default)]
pub struct Trip {
    start: Option<Waypoint>,
    end: Option<Waypoint>,
    mode: Mode,
    routes: Routes,
    /// The number of the last request. A route result counts only for `route_request`.
    last_request: u64,
    route_request: Option<u64>,
    requests: Vec<Request>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Waypoint {
    pub location: LatLon,
    /// Comes from the geocoder after the point is set.
    pub name: Option<String>,
    name_request: u64,
}

#[derive(Debug, Default)]
pub enum Routes {
    #[default]
    None,
    Loading,
    Ready {
        routes: Vec<Route>,
        selected: usize,
        step: Option<usize>,
    },
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Start,
    End,
}

impl Trip {
    pub fn start(&self) -> Option<&Waypoint> {
        self.start.as_ref()
    }

    pub fn end(&self) -> Option<&Waypoint> {
        self.end.as_ref()
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn routes(&self) -> &Routes {
        &self.routes
    }

    pub fn is_empty(&self) -> bool {
        self.start.is_none() && self.end.is_none()
    }

    pub fn error(&self) -> Option<&str> {
        match &self.routes {
            Routes::Failed(error) => Some(error),
            _ => None,
        }
    }

    /// The selected route and its selected step.
    pub fn selected(&self) -> Option<(&Route, Option<usize>)> {
        match &self.routes {
            Routes::Ready {
                routes,
                selected,
                step,
            } => Some((routes.get(*selected)?, *step)),
            _ => None,
        }
    }

    /// The requests since the last call, in order.
    pub fn take_requests(&mut self) -> Vec<Request> {
        mem::take(&mut self.requests)
    }

    /// Sets a point and asks for its name. Asks for routes when both points are set.
    pub fn set(&mut self, end: End, location: LatLon) {
        let current = match end {
            End::Start => &self.start,
            End::End => &self.end,
        };
        if current
            .as_ref()
            .is_some_and(|waypoint| waypoint.location == location)
        {
            return;
        }
        let id = self.next_id();
        self.requests.push(Request::PlaceName { id, location });
        let waypoint = Some(Waypoint {
            location,
            name: None,
            name_request: id,
        });
        match end {
            End::Start => self.start = waypoint,
            End::End => self.end = waypoint,
        }
        self.request_routes();
    }

    pub fn swap(&mut self) {
        mem::swap(&mut self.start, &mut self.end);
        self.request_routes();
    }

    pub fn clear(&mut self) {
        *self = Self {
            mode: self.mode,
            last_request: self.last_request,
            requests: mem::take(&mut self.requests),
            ..Self::default()
        };
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if mode != self.mode {
            self.mode = mode;
            self.request_routes();
        }
    }

    pub fn next_alternative(&mut self) {
        self.select_alternative(|selected, len| (selected + 1) % len);
    }

    pub fn previous_alternative(&mut self) {
        self.select_alternative(|selected, len| (selected + len - 1) % len);
    }

    /// `pick` gets the selected route and the route count.
    fn select_alternative(&mut self, pick: impl FnOnce(usize, usize) -> usize) {
        if let Routes::Ready {
            routes,
            selected,
            step,
        } = &mut self.routes
        {
            *selected = pick(*selected, routes.len());
            *step = None;
        }
    }

    /// Moves the step selection by `delta` steps, and returns the location of the new step.
    /// Without a selected step, a move forward selects the first step.
    pub fn move_step(&mut self, delta: isize) -> Option<LatLon> {
        let Routes::Ready {
            routes,
            selected,
            step,
        } = &mut self.routes
        else {
            return None;
        };
        let steps = &routes.get(*selected)?.steps;
        let last = steps.len().checked_sub(1)?;
        let next = match *step {
            Some(current) => current.saturating_add_signed(delta).min(last),
            None if delta > 0 => 0,
            None => return None,
        };
        *step = Some(next);
        Some(steps[next].location)
    }

    /// Selects step `index` of the selected route, and returns its location.
    pub fn select_step(&mut self, index: usize) -> Option<LatLon> {
        let Routes::Ready {
            routes,
            selected,
            step,
        } = &mut self.routes
        else {
            return None;
        };
        let location = routes.get(*selected)?.steps.get(index)?.location;
        *step = Some(index);
        Some(location)
    }

    /// Takes the result of a route request. Returns the new routes when they are the answer to
    /// the last route request.
    pub fn receive_routes(
        &mut self,
        id: u64,
        result: Result<Vec<Route>, String>,
    ) -> Option<&[Route]> {
        if self.route_request != Some(id) {
            return None;
        }
        self.route_request = None;
        self.routes = match result {
            Ok(routes) if routes.is_empty() => Routes::Failed(no_route(self.mode)),
            Ok(routes) => Routes::Ready {
                routes,
                selected: 0,
                step: None,
            },
            Err(error) => Routes::Failed(error),
        };
        match &self.routes {
            Routes::Ready { routes, .. } => Some(routes),
            _ => None,
        }
    }

    pub fn receive_name(&mut self, id: u64, name: Option<String>) {
        let waypoint = [&mut self.start, &mut self.end]
            .into_iter()
            .flatten()
            .find(|waypoint| waypoint.name_request == id);
        if let Some(waypoint) = waypoint {
            waypoint.name = name;
        }
    }

    fn request_routes(&mut self) {
        let (Some(start), Some(end)) = (&self.start, &self.end) else {
            self.routes = Routes::None;
            self.route_request = None;
            return;
        };
        let (from, to) = (start.location, end.location);
        let id = self.next_id();
        self.route_request = Some(id);
        self.routes = Routes::Loading;
        self.requests.push(Request::Route {
            id,
            from,
            to,
            mode: self.mode,
        });
    }

    fn next_id(&mut self) -> u64 {
        self.last_request += 1;
        self.last_request
    }
}

/// A route of 1 km in 2 minutes, with one step at each of `steps`, along a line through them.
#[cfg(test)]
pub fn sample_route(steps: &[LatLon]) -> Route {
    use tmaps_core::routing::{Maneuver, Step};

    Route {
        distance_m: 1000.0,
        duration_s: 120.0,
        has_toll: false,
        has_highway: false,
        has_ferry: false,
        steps: steps
            .iter()
            .enumerate()
            .map(|(index, &location)| Step {
                instruction: format!("step {index}"),
                maneuver: Maneuver::Continue,
                street_names: Vec::new(),
                distance_m: 100.0,
                duration_s: 10.0,
                elapsed_s: 0.0,
                location,
            })
            .collect(),
        geometry: steps.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;

    fn point(lat: f64, lon: f64) -> LatLon {
        LatLon::new(lat, lon).unwrap()
    }

    fn route(steps: u8) -> Route {
        let points: Vec<LatLon> = (0..steps)
            .map(|index| point(45.0, 9.0 + f64::from(index)))
            .collect();
        sample_route(&points)
    }

    fn route_request(trip: &mut Trip) -> u64 {
        trip.take_requests()
            .into_iter()
            .find_map(|request| match request {
                Request::Route { id, .. } => Some(id),
                _ => None,
            })
            .unwrap()
    }

    /// A trip with both points set and two routes of three steps.
    fn ready_trip() -> Trip {
        let mut trip = Trip::default();
        trip.set(End::Start, point(45.0, 9.0));
        trip.set(End::End, point(45.1, 9.1));
        let id = route_request(&mut trip);
        trip.receive_routes(id, Ok(vec![route(3), route(3)]));
        trip
    }

    #[test]
    fn one_point_asks_only_for_its_name() {
        let mut trip = Trip::default();
        trip.set(End::Start, point(45.0, 9.0));
        assert_eq!(
            trip.take_requests(),
            [Request::PlaceName {
                id: 1,
                location: point(45.0, 9.0)
            }]
        );
    }

    #[test]
    fn same_point_again_asks_for_nothing() {
        let mut trip = ready_trip();
        trip.set(End::End, point(45.1, 9.1));
        assert_eq!(trip.take_requests(), []);
    }

    #[test]
    fn second_point_asks_for_routes() {
        let mut trip = Trip::default();
        trip.set(End::End, point(45.1, 9.1));
        trip.set(End::Start, point(45.0, 9.0));
        assert_matches!(
            trip.take_requests().last(),
            Some(Request::Route { from, mode: Mode::Car, .. }) if *from == point(45.0, 9.0)
        );
    }

    #[test]
    fn mode_change_asks_for_new_routes() {
        let mut trip = ready_trip();
        trip.set_mode(Mode::Foot);
        assert_matches!(
            trip.take_requests()[..],
            [Request::Route {
                mode: Mode::Foot,
                ..
            }]
        );
    }

    #[test]
    fn swap_asks_for_the_reverse_route() {
        let mut trip = ready_trip();
        trip.swap();
        assert_matches!(
            trip.take_requests().last(),
            Some(Request::Route { to, .. }) if *to == point(45.0, 9.0)
        );
    }

    #[test]
    fn old_route_result_is_ignored() {
        let mut trip = Trip::default();
        trip.set(End::Start, point(45.0, 9.0));
        trip.set(End::End, point(45.1, 9.1));
        let old = route_request(&mut trip);
        trip.set_mode(Mode::Bike);
        trip.receive_routes(old, Ok(vec![route(1)]));
        assert_matches!(trip.routes(), Routes::Loading);
    }

    #[test]
    fn clear_drops_the_routes_that_are_loading() {
        let mut trip = Trip::default();
        trip.set(End::Start, point(45.0, 9.0));
        trip.set(End::End, point(45.1, 9.1));
        let id = route_request(&mut trip);
        trip.clear();
        trip.receive_routes(id, Ok(vec![route(1)]));
        assert_matches!(trip.routes(), Routes::None);
    }

    #[test]
    fn route_error_is_the_trip_error() {
        let mut trip = Trip::default();
        trip.set(End::Start, point(45.0, 9.0));
        trip.set(End::End, point(45.1, 9.1));
        let id = route_request(&mut trip);
        trip.receive_routes(id, Err("no route found: too far".to_owned()));
        assert_eq!(trip.error(), Some("no route found: too far"));
    }

    #[test]
    fn name_goes_to_its_waypoint() {
        let mut trip = Trip::default();
        trip.set(End::Start, point(45.0, 9.0));
        trip.set(End::End, point(45.1, 9.1));
        trip.receive_name(2, Some("Duomo".to_owned()));
        assert_eq!(
            (
                trip.start().unwrap().name.as_deref(),
                trip.end().unwrap().name.as_deref()
            ),
            (None, Some("Duomo"))
        );
    }

    #[test]
    fn tab_selects_the_next_alternative_and_wraps() {
        let mut trip = ready_trip();
        trip.next_alternative();
        trip.next_alternative();
        assert_matches!(trip.routes(), Routes::Ready { selected: 0, .. });
    }

    #[test]
    fn shift_tab_selects_the_previous_alternative_and_wraps() {
        let mut trip = Trip::default();
        trip.set(End::Start, point(45.0, 9.0));
        trip.set(End::End, point(45.1, 9.1));
        let id = route_request(&mut trip);
        trip.receive_routes(id, Ok(vec![route(3), route(3), route(3)]));
        trip.previous_alternative();
        assert_matches!(trip.routes(), Routes::Ready { selected: 2, .. });
    }

    #[test]
    fn first_step_move_selects_the_first_step() {
        let mut trip = ready_trip();
        assert_eq!(trip.move_step(1), Some(point(45.0, 9.0)));
    }

    #[test]
    fn step_selection_stops_at_the_last_step() {
        let mut trip = ready_trip();
        for _ in 0..5 {
            trip.move_step(1);
        }
        assert_eq!(trip.selected().unwrap().1, Some(2));
    }
}
