//! A bitmap cache of the map. The map draws each cell of a grid at the floor of the zoom once,
//! into its own bitmap, and a frame only copies the bitmaps. A software canvas, as in Firefox,
//! takes much more time to stroke the roads of a tile than to copy its bitmap.

use std::collections::HashMap;

use tmaps_core::tiles::TileId;
use tmaps_core::view::{View, VisibleTile};

/// Device pixels around each bitmap. Positions on the screen round to whole device pixels, so
/// that a bitmap copies without a resample, and the border closes the gap that the rounding can
/// leave between two cells.
pub const BLEED: u32 = 1;

/// A bitmap drawn at one zoom stays in use until the zoom moves this many levels from it. A bitmap
/// that shows at another scale is less sharp, so it is drawn again when the view stops.
const ZOOM_STEP: f64 = 0.5;

/// How many levels up a missing cell looks for a bitmap to show in its place.
const FALLBACK_LEVELS: u8 = 3;

/// The cells of the grid on a screen of `width` × `height` units, nearest to the center first.
/// Past the zoom of the tile set, the cells are smaller than the tiles, so that each bitmap keeps
/// the size of one tile.
#[must_use]
pub fn grid(view: View, width: f64, height: f64) -> Vec<VisibleTile> {
    view.visible_tiles(width, height, u8::MAX)
}

/// The loaded tile that has the data of `cell`: the tile of the cell, or the nearest loaded
/// ancestor. Past `max_zoom`, the cell takes its data from its ancestor at `max_zoom`.
pub fn source(
    cell: &VisibleTile,
    max_zoom: u8,
    loaded: impl Fn(TileId) -> bool,
) -> Option<VisibleTile> {
    (0..=cell.id.z.min(max_zoom))
        .rev()
        .filter_map(|z| cell.ancestor(z))
        .find(|tile| loaded(tile.id))
}

/// The width and height in device pixels of the bitmap for a cell of `size` CSS pixels.
#[must_use]
pub fn bitmap_side(size: f64, ratio: f64) -> u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a cell is at most a few thousand device pixels"
    )]
    let pixels = (size * ratio).ceil() as u32;
    pixels + 2 * BLEED
}

/// What a bitmap shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub source: TileId,
    pub zoom: f64,
    /// Device pixels for each CSS pixel.
    pub ratio: f64,
    pub dark: bool,
}

/// Why a cell needs a new bitmap, the most urgent first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Need {
    /// The cell has no bitmap.
    Empty,
    /// The bitmap shows less detail, another palette, or another pixel ratio.
    Content,
    /// The bitmap shows another zoom, so that its lines are less sharp.
    Zoom,
}

struct Raster<B> {
    bitmap: B,
    look: Look,
    pixels: f64,
    used: u64,
}

pub struct RasterCache<B> {
    cells: HashMap<TileId, Raster<B>>,
    frame: u64,
}

impl<B> Default for RasterCache<B> {
    fn default() -> Self {
        Self {
            cells: HashMap::new(),
            frame: 0,
        }
    }
}

impl<B> RasterCache<B> {
    /// Starts a frame. [`RasterCache::evict`] keeps the bitmaps that the frame uses.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    /// Why `cell` needs a new bitmap to show `want`, or `None` if its bitmap is good. While the
    /// view moves (`settled` is `false`), a bitmap of a near zoom stays in use.
    #[must_use]
    pub fn need(&self, cell: TileId, want: Look, settled: bool) -> Option<Need> {
        let Some(raster) = self.cells.get(&cell) else {
            return Some(Need::Empty);
        };
        let look = raster.look;
        if look.dark != want.dark
            || (look.ratio - want.ratio).abs() > f64::EPSILON
            || look.source.z < want.source.z
        {
            return Some(Need::Content);
        }
        let moved = (want.zoom - look.zoom).abs();
        (moved >= ZOOM_STEP || settled && moved > 0.0).then_some(Need::Zoom)
    }

    /// Stores the bitmap of `cell`, of `pixels` device pixels. Returns the bitmap that it replaces.
    pub fn insert(&mut self, cell: TileId, bitmap: B, look: Look, pixels: f64) -> Option<B> {
        let used = self.frame;
        self.cells
            .insert(
                cell,
                Raster {
                    bitmap,
                    look,
                    pixels,
                    used,
                },
            )
            .map(|raster| raster.bitmap)
    }

    /// The bitmap of `cell` and what it shows, for this frame.
    pub fn get(&mut self, cell: TileId) -> Option<(&B, Look)> {
        let frame = self.frame;
        let raster = self.cells.get_mut(&cell)?;
        raster.used = frame;
        Some((&raster.bitmap, raster.look))
    }

    /// The cells that have a bitmap to show in place of `cell`, which has none, at their places
    /// on the screen: the nearest ancestor with a bitmap, else the children with a bitmap.
    pub fn fallback(&self, cell: &VisibleTile) -> Vec<VisibleTile> {
        let lowest = cell.id.z.saturating_sub(FALLBACK_LEVELS);
        let ancestor = (lowest..cell.id.z)
            .rev()
            .filter_map(|z| cell.ancestor(z))
            .find(|ancestor| self.cells.contains_key(&ancestor.id));
        if let Some(ancestor) = ancestor {
            return vec![ancestor];
        }
        children(cell)
            .into_iter()
            .filter(|child| self.cells.contains_key(&child.id))
            .collect()
    }

    /// Removes the bitmaps that this frame did not use, the least recent first, until they have at
    /// most `max_pixels`. Returns the removed bitmaps.
    pub fn evict(&mut self, max_pixels: f64) -> Vec<B> {
        let old = self
            .cells
            .iter()
            .filter(|(_, raster)| raster.used < self.frame);
        let mut total: f64 = old.clone().map(|(_, raster)| raster.pixels).sum();
        let mut old: Vec<_> = old.map(|(&id, raster)| (raster.used, id)).collect();
        old.sort_unstable();
        let mut removed = Vec::new();
        for (_, id) in old {
            if total <= max_pixels {
                break;
            }
            if let Some(raster) = self.cells.remove(&id) {
                total -= raster.pixels;
                removed.push(raster.bitmap);
            }
        }
        removed
    }

    pub fn clear(&mut self) -> Vec<B> {
        self.cells
            .drain()
            .map(|(_, raster)| raster.bitmap)
            .collect()
    }
}

fn children(cell: &VisibleTile) -> [VisibleTile; 4] {
    let size = cell.size / 2.0;
    [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(dx, dy)| VisibleTile {
        id: TileId {
            z: cell.id.z + 1,
            x: cell.id.x * 2 + dx,
            y: cell.id.y * 2 + dy,
        },
        left: cell.left + f64::from(dx) * size,
        top: cell.top + f64::from(dy) * size,
        size,
    })
}

#[cfg(test)]
mod tests {
    use tmaps_core::LatLon;

    use super::*;

    fn id(z: u8, x: u32, y: u32) -> TileId {
        TileId { z, x, y }
    }

    fn cell(z: u8, x: u32, y: u32) -> VisibleTile {
        VisibleTile {
            id: id(z, x, y),
            left: 0.0,
            top: 0.0,
            size: 256.0,
        }
    }

    fn look(source: TileId, zoom: f64) -> Look {
        Look {
            source,
            zoom,
            ratio: 2.0,
            dark: false,
        }
    }

    #[test]
    fn grid_follows_the_zoom_past_the_tile_set() {
        let view = View::new(LatLon::new(45.46, 9.19).unwrap(), 16.4);
        let cells = grid(view, 600.0, 400.0);
        assert!(cells.iter().all(|cell| cell.id.z == 16));
        assert!(cells.iter().all(|cell| cell.size < 2.0 * 256.0));
    }

    #[test]
    fn cell_takes_its_data_from_the_nearest_loaded_tile() {
        let deep = cell(16, 40, 24);
        let source = source(&deep, 14, |tile| tile == id(14, 10, 6)).unwrap();
        assert_eq!(source.id, id(14, 10, 6));
        assert_eq!(source.size, 4.0 * 256.0);
        let own = super::source(&cell(5, 3, 2), 14, |_| true).unwrap();
        assert_eq!(own.id, id(5, 3, 2));
        assert!(super::source(&deep, 14, |_| false).is_none());
    }

    #[test]
    fn bitmap_has_a_border_of_device_pixels() {
        assert_eq!(bitmap_side(512.0, 2.0), 1024 + 2 * BLEED);
        assert_eq!(bitmap_side(512.4, 1.0), 513 + 2 * BLEED);
    }

    #[test]
    fn bitmap_is_drawn_again_when_its_content_changes() {
        let mut cache = RasterCache::default();
        let tile = id(12, 1, 1);
        assert_eq!(
            cache.need(tile, look(id(11, 0, 0), 12.0), true),
            Some(Need::Empty)
        );
        cache.insert(tile, (), look(id(11, 0, 0), 12.0), 1.0);
        assert_eq!(
            cache.need(tile, look(tile, 12.0), false),
            Some(Need::Content)
        );
        cache.insert(tile, (), look(tile, 12.0), 1.0);
        assert_eq!(cache.need(tile, look(id(11, 0, 0), 12.0), true), None);
        let dark = Look {
            dark: true,
            ..look(tile, 12.0)
        };
        assert_eq!(cache.need(tile, dark, false), Some(Need::Content));
    }

    #[test]
    fn bitmap_of_a_near_zoom_stays_until_the_view_stops() {
        let mut cache = RasterCache::default();
        let tile = id(12, 1, 1);
        cache.insert(tile, (), look(tile, 12.2), 1.0);
        assert_eq!(cache.need(tile, look(tile, 12.4), false), None);
        assert_eq!(cache.need(tile, look(tile, 12.4), true), Some(Need::Zoom));
        assert_eq!(cache.need(tile, look(tile, 12.7), false), Some(Need::Zoom));
        assert_eq!(cache.need(tile, look(tile, 12.2), true), None);
    }

    #[test]
    fn missing_cell_shows_its_ancestor_else_its_children() {
        let mut cache = RasterCache::default();
        let missing = VisibleTile {
            id: id(10, 4, 6),
            left: 100.0,
            top: 50.0,
            size: 300.0,
        };
        assert_eq!(cache.fallback(&missing), Vec::new());
        cache.insert(id(11, 9, 12), (), look(id(11, 9, 12), 11.0), 1.0);
        let children = cache.fallback(&missing);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, id(11, 9, 12));
        assert_eq!(
            (children[0].left, children[0].top, children[0].size),
            (250.0, 50.0, 150.0)
        );
        cache.insert(id(8, 1, 1), (), look(id(8, 1, 1), 8.0), 1.0);
        let ancestor = cache.fallback(&missing);
        assert_eq!(ancestor.len(), 1);
        assert_eq!(ancestor[0].id, id(8, 1, 1));
        assert_eq!(ancestor[0].size, 1200.0);
    }

    #[test]
    fn eviction_keeps_the_bitmaps_of_this_frame() {
        let mut cache = RasterCache::default();
        cache.insert(id(1, 0, 0), "old", look(id(1, 0, 0), 1.0), 10.0);
        cache.begin_frame();
        cache.insert(id(1, 1, 0), "older", look(id(1, 1, 0), 1.0), 10.0);
        cache.begin_frame();
        cache.insert(id(1, 0, 1), "new", look(id(1, 0, 1), 1.0), 10.0);
        let _ = cache.get(id(1, 0, 0));
        assert_eq!(cache.evict(15.0), Vec::<&str>::new());
        assert_eq!(cache.evict(5.0), vec!["older"]);
        assert_eq!(cache.evict(0.0), Vec::<&str>::new());
        assert!(cache.get(id(1, 0, 0)).is_some());
    }
}
