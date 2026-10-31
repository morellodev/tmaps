use std::collections::{HashMap, HashSet};

use tmaps_core::tiles::TileId;

const MAX_TILES: usize = 256;

/// The loaded tiles, and the tiles that the page fetches. When the cache is full, the tile that
/// the map used least recently goes first. The tiles of the current frame always stay, so the
/// cache grows past its limit when more tiles are visible. A tile that fails stays requested while
/// it is visible, so that the page does not ask for it in a loop.
#[derive(Debug)]
pub struct TileCache<T> {
    tiles: HashMap<TileId, (T, u64)>,
    requested: HashSet<TileId>,
    frame: u64,
}

impl<T> Default for TileCache<T> {
    fn default() -> Self {
        Self {
            tiles: HashMap::new(),
            requested: HashSet::new(),
            frame: 0,
        }
    }
}

impl<T> TileCache<T> {
    #[must_use]
    pub fn get(&self, id: TileId) -> Option<&T> {
        self.tiles.get(&id).map(|(tile, _)| tile)
    }

    /// Starts a frame that shows `visible`. These tiles and their loaded ancestors, which the map
    /// shows while a tile loads, count as used. Gives the visible tiles that are not loaded or
    /// requested yet. They count as requested from now.
    pub fn request(&mut self, visible: impl IntoIterator<Item = TileId>) -> Vec<TileId> {
        self.frame += 1;
        let mut missing = Vec::new();
        for id in visible {
            if !self.tiles.contains_key(&id) && self.requested.insert(id) {
                missing.push(id);
            }
            for used in (0..=id.z).filter_map(|z| id.ancestor(z)) {
                if let Some((_, frame)) = self.tiles.get_mut(&used) {
                    *frame = self.frame;
                }
            }
        }
        missing
    }

    /// Gives the requested tiles that are not loaded and not in `visible`, so that the page can
    /// stop their fetch. They do not count as requested from now.
    pub fn cancel(&mut self, visible: &[TileId]) -> Vec<TileId> {
        let stale: Vec<TileId> = self
            .requested
            .iter()
            .filter(|id| !self.tiles.contains_key(id) && !visible.contains(id))
            .copied()
            .collect();
        for id in &stale {
            self.requested.remove(id);
        }
        stale
    }

    pub fn add(&mut self, id: TileId, tile: T) {
        self.tiles.insert(id, (tile, self.frame));
        while self.tiles.len() > MAX_TILES {
            let oldest = self
                .tiles
                .iter()
                .filter(|(_, (_, frame))| *frame < self.frame)
                .min_by_key(|(_, (_, frame))| *frame)
                .map(|(&id, _)| id);
            let Some(oldest) = oldest else {
                break;
            };
            self.tiles.remove(&oldest);
            self.requested.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(x: u32) -> TileId {
        TileId { z: 14, x, y: 0 }
    }

    fn full_cache() -> (TileCache<()>, Vec<TileId>) {
        let mut cache = TileCache::<()>::default();
        let ids: Vec<TileId> = (0..u32::try_from(MAX_TILES).unwrap()).map(id).collect();
        cache.request(ids.iter().copied());
        for &tile in &ids {
            cache.add(tile, ());
        }
        (cache, ids)
    }

    #[test]
    fn requested_tile_is_not_requested_again() {
        let mut cache = TileCache::<()>::default();
        assert_eq!(cache.request([id(1), id(2)]), [id(1), id(2)]);
        assert_eq!(cache.request([id(1), id(3)]), [id(3)]);
    }

    #[test]
    fn loaded_tile_is_not_requested() {
        let mut cache = TileCache::<()>::default();
        cache.add(id(1), ());
        assert_eq!(cache.request([id(1)]), []);
    }

    #[test]
    fn tile_that_leaves_the_view_before_it_loads_is_cancelled() {
        let mut cache = TileCache::<()>::default();
        cache.request([id(1), id(2), id(3)]);
        cache.add(id(3), ());
        assert_eq!(cache.cancel(&[id(1)]), [id(2)]);
        assert_eq!(cache.cancel(&[id(1)]), []);
        assert_eq!(cache.request([id(1), id(2), id(3)]), [id(2)]);
    }

    #[test]
    fn full_cache_drops_the_least_recently_used_tile_and_can_request_it_again() {
        let (mut cache, ids) = full_cache();
        cache.request(ids[..1].iter().copied());
        cache.request([id(1000)]);
        cache.add(id(1000), ());
        assert!(cache.get(id(0)).is_some());
        let dropped: Vec<TileId> = ids[1..]
            .iter()
            .copied()
            .filter(|&tile| cache.get(tile).is_none())
            .collect();
        assert_eq!(dropped.len(), 1);
        assert_eq!(cache.request(dropped.iter().copied()), dropped);
    }

    #[test]
    fn full_cache_keeps_the_tiles_of_the_current_frame() {
        let (mut cache, mut ids) = full_cache();
        ids.push(id(1000));
        cache.request(ids.iter().copied());
        cache.add(id(1000), ());
        assert!(ids.iter().all(|&tile| cache.get(tile).is_some()));
        assert_eq!(cache.request(ids.iter().copied()), []);
    }

    #[test]
    fn full_cache_keeps_the_ancestor_of_a_visible_tile() {
        let (mut cache, _) = full_cache();
        let parent = TileId { z: 13, x: 0, y: 0 };
        cache.add(parent, ());
        cache.request([TileId { z: 14, x: 1, y: 1 }]);
        cache.add(id(1000), ());
        assert!(cache.get(parent).is_some());
    }
}
