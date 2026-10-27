use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use tmaps_core::render::PreparedTile;
use tmaps_core::tiles::{TileId, TileSource};
use tokio::sync::{Semaphore, mpsc};

use crate::message::{Message, Server, SourceInfo, describe};

/// More parallel requests than a browser makes to one host would be unfair to a free server.
const MAX_REQUESTS: usize = 6;

/// Loads tiles in background tasks and sends each result as a [`Message`].
pub struct Loader {
    source: Arc<TileSource>,
    permits: Arc<Semaphore>,
    wanted: Arc<Mutex<HashSet<TileId>>>,
    lang: Arc<str>,
    sender: mpsc::UnboundedSender<Message>,
}

impl Loader {
    /// `lang` is the BCP 47 tag for the names on the labels.
    pub fn new(source: TileSource, lang: &str, sender: mpsc::UnboundedSender<Message>) -> Self {
        Self {
            source: Arc::new(source),
            permits: Arc::new(Semaphore::new(MAX_REQUESTS)),
            wanted: Arc::default(),
            lang: Arc::from(lang),
            sender,
        }
    }

    pub fn load_source(&self) {
        let (source, sender) = (Arc::clone(&self.source), self.sender.clone());
        tokio::spawn(async move {
            let info = source
                .tile_set()
                .await
                .map(|tile_set| SourceInfo {
                    max_zoom: tile_set.max_zoom,
                    attribution: tile_set.attribution.clone(),
                })
                .map_err(|error| describe(&error, Server::Map));
            let _ = sender.send(Message::Source(info));
        });
    }

    /// Tiles that are not in `tiles` when their request can start are skipped.
    pub fn set_wanted(&self, tiles: impl IntoIterator<Item = TileId>) {
        let mut wanted = self.wanted.lock().unwrap_or_else(PoisonError::into_inner);
        wanted.clear();
        wanted.extend(tiles);
    }

    pub fn load(&self, id: TileId) {
        let source = Arc::clone(&self.source);
        let permits = Arc::clone(&self.permits);
        let wanted = Arc::clone(&self.wanted);
        let lang = Arc::clone(&self.lang);
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let Ok(_permit) = permits.acquire_owned().await else {
                return;
            };
            let is_wanted = wanted
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(&id);
            let message = if is_wanted {
                Message::Tile(id, prepare(&source, id, lang).await)
            } else {
                Message::Skipped(id)
            };
            let _ = sender.send(message);
        });
    }
}

async fn prepare(
    source: &TileSource,
    id: TileId,
    lang: Arc<str>,
) -> Result<Box<PreparedTile>, String> {
    let body = source
        .tile(id)
        .await
        .map_err(|error| describe(&error, Server::Map))?;
    tokio::task::spawn_blocking(move || PreparedTile::decode(&body, &lang).map(Box::new))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|_| format!("The map server sent tile {id}, but tmaps cannot read it."))
}
