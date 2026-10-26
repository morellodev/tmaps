use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tmaps_core::LatLon;
use tmaps_core::providers::photon::Photon;
use tmaps_core::providers::valhalla::Valhalla;
use tmaps_core::routing::{Mode, RouteRequest};
use tokio::sync::mpsc;

use crate::message::Message;

const ALTERNATIVES: u8 = 2;
/// A held key (`x`, `1`) asks for many routes. The router serves one request each second, so
/// only the last request in this time goes to the router.
const ROUTE_DEBOUNCE: Duration = Duration::from_millis(250);

/// Work for the providers. The number of a request comes back in its [`Message`].
#[derive(Debug, Clone, Copy, PartialEq)]
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
}

/// Sends requests to the geocoder and the router in background tasks.
pub struct Services {
    photon: Arc<Photon>,
    valhalla: Arc<Valhalla>,
    /// BCP 47 tag for the names and the instructions.
    lang: Arc<str>,
    sender: mpsc::UnboundedSender<Message>,
    last_route: Arc<AtomicU64>,
}

impl Services {
    pub fn new(
        photon: Photon,
        valhalla: Valhalla,
        lang: &str,
        sender: mpsc::UnboundedSender<Message>,
    ) -> Self {
        Self {
            photon: Arc::new(photon),
            valhalla: Arc::new(valhalla),
            lang: Arc::from(lang),
            sender,
            last_route: Arc::default(),
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
        }
    }
}
