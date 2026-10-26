use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use lru::LruCache;
use tmaps_core::LatLon;
use tmaps_core::geocode::{Place, SearchOptions};
use tmaps_core::places;
use tmaps_core::providers::photon::Photon;
use tmaps_core::providers::valhalla::Valhalla;
use tmaps_core::routing::{Mode, RouteRequest};
use tokio::sync::mpsc;

use crate::message::Message;

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
    last_route: Arc<AtomicU64>,
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
            last_route: Arc::default(),
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
                let last_route = Arc::clone(&self.last_route);
                last_route.store(id, Ordering::Relaxed);
                tokio::spawn(async move {
                    tokio::time::sleep(ROUTE_DEBOUNCE).await;
                    if last_route.load(Ordering::Relaxed) != id {
                        return;
                    }
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
                        .map_err(|error| error.to_string());
                    let _ = sender.send(Message::Routes(id, routes));
                });
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
                        Err(error) => Message::SaveFailed(format!("cannot save @{name}: {error}")),
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
            let result = result.map_err(|error| error.to_string());
            let _ = sender.send(Message::SearchResults(id, result));
        });
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
    use super::*;

    #[test]
    fn search_key_ignores_case_spaces_and_a_small_pan() {
        let near = |lat, lon| LatLon::new(lat, lon).unwrap();
        assert_eq!(
            search_key(" Duomo", near(45.4641, 9.1901)),
            search_key("duomo ", near(45.4642, 9.19))
        );
    }
}
