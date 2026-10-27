use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use lru::LruCache;
use tmaps_core::geocode::{Place, SearchOptions};
use tmaps_core::places;
use tmaps_core::providers::photon::Photon;
use tmaps_core::providers::valhalla::Valhalla;
use tmaps_core::routing::{Mode, RouteRequest};
use tmaps_core::{Error, LatLon};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::message::{Message, Server, describe, no_route};

const ALTERNATIVES: u8 = 2;
/// A held key (`x`, `1`) asks for many routes. The router serves one request each second, so
/// only the last request in this time goes to the router.
const ROUTE_DEBOUNCE: Duration = Duration::from_millis(250);
/// Photon allows autocomplete, but not a request for each typed character.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);
const SEARCH_CACHE_SIZE: NonZeroUsize = NonZeroUsize::new(64).unwrap();

/// Work for the providers and the config file. The number of a request comes back in its
/// [`Message`].
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Route {
        id: u64,
        from: LatLon,
        to: LatLon,
        mode: Mode,
    },
    PlaceName {
        id: u64,
        location: LatLon,
    },
    Search {
        id: u64,
        text: String,
        near: LatLon,
    },
    /// Drops the result of the last search request.
    CancelSearch,
    SavePlace {
        name: String,
        location: LatLon,
    },
}

/// Sends requests to the geocoder and the router in background tasks.
pub struct Services {
    photon: Arc<Photon>,
    valhalla: Arc<Valhalla>,
    /// BCP 47 tag for the names and the instructions.
    lang: Arc<str>,
    config_file: Arc<PathBuf>,
    sender: mpsc::UnboundedSender<Message>,
    /// The next route request aborts it, also inside the router throttle.
    route_task: Mutex<Option<AbortHandle>>,
    last_search: Arc<AtomicU64>,
    searches: Arc<Mutex<LruCache<String, Vec<Place>>>>,
    /// Saves read, change, and write the config file, so they must not overlap.
    config_lock: Arc<Mutex<()>>,
}

impl Services {
    pub fn new(
        photon: Photon,
        valhalla: Valhalla,
        lang: &str,
        config_file: PathBuf,
        sender: mpsc::UnboundedSender<Message>,
    ) -> Self {
        Self {
            photon: Arc::new(photon),
            valhalla: Arc::new(valhalla),
            lang: Arc::from(lang),
            config_file: Arc::new(config_file),
            sender,
            route_task: Mutex::default(),
            last_search: Arc::default(),
            searches: Arc::new(Mutex::new(LruCache::new(SEARCH_CACHE_SIZE))),
            config_lock: Arc::default(),
        }
    }

    pub fn send(&self, request: Request) {
        let sender = self.sender.clone();
        let lang = Arc::clone(&self.lang);
        match request {
            Request::Route { id, from, to, mode } => {
                let valhalla = Arc::clone(&self.valhalla);
                let task = tokio::spawn(async move {
                    tokio::time::sleep(ROUTE_DEBOUNCE).await;
                    let request = RouteRequest {
                        from,
                        to,
                        mode,
                        alternatives: ALTERNATIVES,
                        lang: Some(lang.to_string()),
                    };
                    let routes = valhalla
                        .route(&request)
                        .await
                        .map_err(|error| route_error(&error, mode));
                    let _ = sender.send(Message::Routes(id, routes));
                });
                let previous = self
                    .route_task
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .replace(task.abort_handle());
                if let Some(previous) = previous {
                    previous.abort();
                }
            }
            Request::PlaceName { id, location } => {
                let photon = Arc::clone(&self.photon);
                tokio::spawn(async move {
                    let place = photon.reverse(location, Some(&lang)).await;
                    let name = place
                        .ok()
                        .flatten()
                        .map(|place| place.name.unwrap_or(place.label));
                    let _ = sender.send(Message::PlaceName(id, name));
                });
            }
            Request::Search { id, text, near } => self.search(id, &text, near),
            // Request numbers start at 1.
            Request::CancelSearch => self.last_search.store(0, Ordering::Relaxed),
            Request::SavePlace { name, location } => {
                let config_file = Arc::clone(&self.config_file);
                let config_lock = Arc::clone(&self.config_lock);
                tokio::task::spawn_blocking(move || {
                    let _guard = config_lock.lock().unwrap_or_else(PoisonError::into_inner);
                    let message = match places::add(&config_file, &name, location) {
                        Ok(()) => Message::PlaceSaved(name, location),
                        Err(error) => Message::SaveFailed(format!("Cannot save @{name}: {error}")),
                    };
                    let _ = sender.send(message);
                });
            }
        }
    }

    /// Answers from the cache at once. Otherwise waits for the user to stop typing, then asks
    /// Photon.
    fn search(&self, id: u64, text: &str, near: LatLon) {
        self.last_search.store(id, Ordering::Relaxed);
        let key = search_key(text, near);
        let cached = self
            .searches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned();
        if let Some(places) = cached {
            let _ = self.sender.send(Message::SearchResults(id, Ok(places)));
            return;
        }
        let last_search = Arc::clone(&self.last_search);
        let photon = Arc::clone(&self.photon);
        let searches = Arc::clone(&self.searches);
        let sender = self.sender.clone();
        let options = SearchOptions {
            near: Some(near),
            lang: Some(self.lang.to_string()),
            ..SearchOptions::default()
        };
        let text = text.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(SEARCH_DEBOUNCE).await;
            if last_search.load(Ordering::Relaxed) != id {
                return;
            }
            let result = photon.search(&text, &options).await;
            if let Ok(places) = &result {
                searches
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .put(key, places.clone());
            }
            let result = result.map_err(|error| describe(&error, Server::Search));
            let _ = sender.send(Message::SearchResults(id, result));
        });
    }
}

fn route_error(error: &Error, mode: Mode) -> String {
    match error {
        Error::NoRoute(_) => no_route(mode),
        _ => describe(error, Server::Routing),
    }
}

/// The center is rounded to about 1 km, so a small pan usually keeps the cached results.
fn search_key(text: &str, near: LatLon) -> String {
    format!(
        "{:.2},{:.2} {}",
        near.lat(),
        near.lon(),
        text.trim().to_lowercase()
    )
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    #[tokio::test]
    async fn new_route_request_aborts_one_that_waits_for_the_router() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let client = tmaps_core::http::client().unwrap();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let services = Services::new(
            Photon::new(client.clone(), &server.uri()),
            Valhalla::new(client, &server.uri()),
            "en",
            PathBuf::new(),
            sender,
        );
        let route = |id| Request::Route {
            id,
            from: LatLon::new(45.4642, 9.19).unwrap(),
            to: LatLon::new(41.9028, 12.4964).unwrap(),
            mode: Mode::Car,
        };
        let past_debounce = ROUTE_DEBOUNCE + Duration::from_millis(50);
        services.send(route(1));
        tokio::time::sleep(past_debounce).await;
        // The router throttle holds request 2 for about one second.
        services.send(route(2));
        tokio::time::sleep(past_debounce).await;
        services.send(route(3));
        let mut ids = Vec::new();
        while ids.len() < 2 {
            if let Some(Message::Routes(id, _)) = receiver.recv().await {
                ids.push(id);
            }
        }
        let requests = server.received_requests().await.unwrap().len();
        assert_eq!((ids, requests), (vec![1, 3], 2));
    }

    #[test]
    fn no_route_error_tells_the_other_modes() {
        let error = Error::NoRoute("too far".to_owned());
        assert_eq!(
            route_error(&error, Mode::Bike),
            "No route by bike. Press 1 or 3 for another mode, or move a point."
        );
    }

    #[test]
    fn search_key_ignores_case_spaces_and_a_small_pan() {
        let near = |lat, lon| LatLon::new(lat, lon).unwrap();
        assert_eq!(
            search_key(" Duomo", near(45.4641, 9.1901)),
            search_key("duomo ", near(45.4642, 9.19))
        );
    }
}
