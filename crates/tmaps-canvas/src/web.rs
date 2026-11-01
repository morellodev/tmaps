use std::collections::{HashMap, HashSet};

use js_sys::{Array, Reflect, Uint8Array};
use tmaps_core::label::{LabelCandidate, LabelClass};
use tmaps_core::scene::{Rect, Scene, ScreenPoint};
use tmaps_core::style::{Class, Layer, Layers};
use tmaps_core::tiles::{TileId, TileSet};
use tmaps_core::view::{TILE_SIZE, View, VisibleTile};
use tmaps_core::{LatLon, PreparedTile};
use wasm_bindgen::prelude::*;
use web_sys::{
    CanvasRenderingContext2d, ContextAttributes2d, HtmlCanvasElement, ImageBitmap, OffscreenCanvas,
    OffscreenCanvasRenderingContext2d, Path2d,
};

use crate::cache::TileCache;
use crate::fade::Fades;
use crate::labels::{self, Candidate, Size};
use crate::line_text::Run;
use crate::paint::{Line, Palette};
use crate::raster::{self, BLEED, Look, RasterCache, Style};
use crate::tile::{self, CanvasTile, TilePath, UNITS};
use crate::wire;

// Tiles show at 512 px, the size that OpenMapTiles styles and MapLibre use, so a zoom level
// means the same scale as in MapLibre.
const CSS_PER_UNIT: f64 = 2.0;

/// The shortest time between two placements of the labels, in milliseconds. At low zoom, a
/// placement takes milliseconds. Between placements, the labels move with the map.
const PLACE_MS: f64 = 100.0;

/// The most label sizes that the map keeps for each label class. When a class has more, the map
/// removes them and measures each text again.
const MAX_SIZES: usize = 4096;

/// The longest time that one frame draws cell bitmaps, in milliseconds. A frame starts a cell only
/// if the mean time of a cell fits. While the zoom moves, a cell that does not fit waits, and a
/// scaled bitmap shows in its place. One cell can take longer than a frame.
const RASTER_BUDGET_MS: f64 = 6.0;

/// The most cells that one frame draws while the zoom moves. Safari draws a canvas in its GPU
/// process after the call returns, so the time of the call does not show the cost of a cell.
const MOVING_CELLS: usize = 1;

/// The weight of the last cell in the mean time to draw one cell.
const PAINT_MS_WEIGHT: f64 = 0.25;

/// The cell bitmaps that are not on the screen keep at most this many screens of pixels.
const RASTER_SCREENS: f64 = 3.0;

/// The time without a zoom change, in milliseconds, after which the cells are drawn again at the
/// zoom of the view. Between the steps of a wheel gesture, a new bitmap would be out of date soon.
const SETTLE_MS: f64 = 150.0;

/// A map on an HTML canvas. JavaScript fetches the `TileJSON` document and the tiles, and gives
/// them to the map: the map does no I/O. All positions are in CSS pixels.
#[wasm_bindgen]
pub struct WebMap {
    canvas: HtmlCanvasElement,
    context: CanvasRenderingContext2d,
    tile_set: Option<TileSet>,
    view: View,
    tiles: TileCache<CanvasTile>,
    rasters: RasterCache<ImageBitmap>,
    painter: Option<Painter>,
    /// The zoom of the view, and the time in milliseconds when the view got it.
    zoom_since: Option<(f64, f64)>,
    drawn_style: Option<Style>,
    route: Vec<LatLon>,
    dark: bool,
    layers: Layers,
    sizes: Sizes,
    fades: Fades<Spot>,
    placement: Option<Placement>,
    /// A tile came after the last placement of the labels.
    tiles_added: bool,
}

/// The frame that placed the labels last.
#[derive(Clone, Copy)]
struct Placement {
    time: f64,
    view: View,
    size: (f64, f64),
}

/// The [`Size`] of each label text, by class and by whether the label is on a line.
type Sizes = HashMap<(LabelClass, bool), HashMap<String, Size>>;

/// The position of a label on the map, its position on the screen in the last frame, and the
/// parts of its text around that position at the zoom of its placement.
struct Spot {
    point: LatLon,
    screen: ScreenPoint,
    zoom: f64,
    runs: Vec<Run>,
}

#[wasm_bindgen]
impl WebMap {
    /// # Errors
    ///
    /// If the canvas has no 2D context.
    #[wasm_bindgen(constructor)]
    pub fn new(canvas: HtmlCanvasElement) -> Result<WebMap, JsValue> {
        let context = opaque_context(&canvas)?;
        Ok(Self {
            canvas,
            context,
            tile_set: None,
            view: View::new(LatLon::new(0.0, 0.0).map_err(js)?, 1.0),
            tiles: TileCache::default(),
            rasters: RasterCache::default(),
            painter: None,
            zoom_since: None,
            drawn_style: None,
            route: Vec::new(),
            dark: false,
            layers: Layers::ALL,
            sizes: Sizes::new(),
            fades: Fades::default(),
            placement: None,
            tiles_added: false,
        })
    }

    /// Removes the tiles of the last tile set. Stop their fetches, so that they do not come to
    /// [`WebMap::add_tile`].
    ///
    /// # Errors
    ///
    /// If `tilejson` is not a `TileJSON` document with a tile URL.
    pub fn set_tile_set(&mut self, tilejson: &str) -> Result<(), JsValue> {
        self.tile_set = Some(TileSet::from_json(tilejson.as_bytes()).map_err(js)?);
        self.tiles = TileCache::default();
        self.fades = Fades::default();
        self.placement = None;
        self.tiles_added = false;
        for bitmap in self.rasters.clear() {
            bitmap.close();
        }
        Ok(())
    }

    /// # Errors
    ///
    /// If the coordinates are out of range.
    pub fn set_view(&mut self, lat: f64, lon: f64, zoom: f64) -> Result<(), JsValue> {
        self.view = View::new(LatLon::new(lat, lon).map_err(js)?, zoom);
        Ok(())
    }

    #[must_use]
    pub fn lat(&self) -> f64 {
        self.view.center().lat()
    }

    #[must_use]
    pub fn lon(&self) -> f64 {
        self.view.center().lon()
    }

    #[must_use]
    pub fn zoom(&self) -> f64 {
        self.view.zoom()
    }

    /// Moves the view. Positive values move it east and south, so the map moves west and north.
    pub fn pan_by(&mut self, dx: f64, dy: f64) {
        self.view = self.view.pan(dx / CSS_PER_UNIT, dy / CSS_PER_UNIT);
    }

    /// Zooms by `delta` levels and keeps the point at `(x, y)` in its place, for example the
    /// point under the pointer.
    pub fn zoom_at(&mut self, delta: f64, x: f64, y: f64) {
        let (width, height) = self.units();
        let (x, y) = (x / CSS_PER_UNIT, y / CSS_PER_UNIT);
        let point = self.view.point_at(x, y, width, height);
        let zoomed = self.view.zoom_by(delta);
        let (moved_x, moved_y) = zoomed.screen_position(point, width, height);
        self.view = zoomed.pan(moved_x - x, moved_y - y);
    }

    pub fn set_dark(&mut self, dark: bool) {
        self.dark = dark;
    }

    /// Shows or hides a layer of the map: `roads`, `transit`, `buildings`, `nature`,
    /// `boundaries`, or `labels`. All layers show at the start.
    ///
    /// # Errors
    ///
    /// If `name` is not the name of a layer.
    pub fn set_layer(&mut self, name: &str, visible: bool) -> Result<(), JsValue> {
        let layer: Layer = name.parse().map_err(js)?;
        let before = self.layers;
        if before.contains(layer) != visible {
            self.layers = before.toggled(layer);
        }
        if !self.layers.shows_the_labels_of(before) {
            self.placement = None;
        }
        Ok(())
    }

    /// Sets the route to draw over the map, as `[lat, lon, lat, lon, …]`. An empty array removes
    /// it.
    ///
    /// # Errors
    ///
    /// If the array has an odd length or a coordinate is out of range.
    pub fn set_route(&mut self, points: &[f64]) -> Result<(), JsValue> {
        let (pairs, rest) = points.as_chunks::<2>();
        if !rest.is_empty() {
            return Err(JsError::new("the route needs pairs of lat and lon").into());
        }
        self.route = pairs
            .iter()
            .map(|&[lat, lon]| LatLon::new(lat, lon))
            .collect::<tmaps_core::Result<_>>()
            .map_err(js)?;
        Ok(())
    }

    /// The visible tiles to fetch, as `[z, x, y, z, x, y, …]`. Each tile comes once: fetch it and
    /// give it to [`WebMap::add_tile`].
    #[must_use]
    pub fn missing_tiles(&mut self) -> Vec<u32> {
        let visible = self.visible_tiles();
        flat_ids(self.tiles.request(visible))
    }

    /// The tiles that [`WebMap::missing_tiles`] gave and that left the view before they came, as
    /// `[z, x, y, z, x, y, …]`. Stop their fetch: they come again from
    /// [`WebMap::missing_tiles`] when they are visible again.
    #[must_use]
    pub fn stale_tiles(&mut self) -> Vec<u32> {
        let visible = self.visible_tiles();
        flat_ids(self.tiles.cancel(&visible))
    }

    #[must_use]
    pub fn tile_url(&self, z: u8, x: u32, y: u32) -> Option<String> {
        Some(self.tile_set.as_ref()?.url(TileId { z, x, y }))
    }

    /// Adds a tile that [`prepare_tile`] prepared. `labels` and `paths` are its results, with
    /// each path data in a `Path2D`.
    ///
    /// # Errors
    ///
    /// If `labels` or `paths` does not have the form of the results of [`prepare_tile`].
    pub fn add_tile(
        &mut self,
        z: u8,
        x: u32,
        y: u32,
        labels: &[u8],
        paths: &Array,
    ) -> Result<(), JsValue> {
        let labels =
            wire::decode(labels).ok_or_else(|| JsError::new("the tile labels are not valid"))?;
        let paths = paths
            .iter()
            .map(|path| {
                let path: Array = path.dyn_into()?;
                let class = path.get(0).as_f64().and_then(index);
                let min_zoom = path.get(1).as_f64();
                let (Some(class), Some(min_zoom)) = (class, min_zoom) else {
                    return Err(JsError::new("a tile path needs a class and a min zoom").into());
                };
                let data = path.get(2).dyn_into()?;
                Ok(TilePath {
                    class,
                    min_zoom,
                    data,
                })
            })
            .collect::<Result<Vec<_>, JsValue>>()?;
        self.tiles
            .add(TileId { z, x, y }, CanvasTile::new(labels, paths));
        self.tiles_added = true;
        Ok(())
    }

    /// Draws the map at the size of the canvas element, with sharp lines on high-density screens.
    /// `now` is the frame time in milliseconds, for example the time that `requestAnimationFrame`
    /// gives. Returns `true` while labels fade in or out or cells wait for their bitmaps: draw
    /// one more frame then.
    ///
    /// # Errors
    ///
    /// If the browser rejects a canvas call.
    pub fn draw(&mut self, now: f64) -> Result<bool, JsValue> {
        let cell_style = self.style();
        let (settled, restyled) = self.changes(now, cell_style);
        let (width, height) = self.size();
        let ratio = web_sys::window().map_or(1.0, |window| window.device_pixel_ratio());
        let (pixels_x, pixels_y) = (device_pixels(width * ratio), device_pixels(height * ratio));
        if self.canvas.width() != pixels_x || self.canvas.height() != pixels_y {
            self.canvas.set_width(pixels_x);
            self.canvas.set_height(pixels_y);
        }
        let context = &self.context;
        context.set_transform(ratio, 0.0, 0.0, ratio, 0.0, 0.0)?;
        let palette = Palette::new(self.dark);
        context.set_fill_style_str(palette.background());
        context.fill_rect(0.0, 0.0, width, height);
        let Some(tile_set) = &self.tile_set else {
            return Ok(false);
        };
        let tiles = &self.tiles;
        let (units_x, units_y) = self.units();
        let cells_wait = draw_cells(
            context,
            &mut self.painter,
            &mut self.rasters,
            tiles,
            Frame {
                view: self.view,
                units: (units_x, units_y),
                max_zoom: tile_set.max_zoom,
                ratio,
                style: cell_style,
                settled,
                restyled,
            },
        )?;
        context.set_transform(ratio, 0.0, 0.0, ratio, 0.0, 0.0)?;
        context.set_line_join("round");
        context.set_line_cap("round");
        if self.route.len() > 1 {
            let path = Path2d::new()?;
            let points = self.view.line_on_screen(&self.route, units_x, units_y);
            add_line(
                &path,
                points
                    .into_iter()
                    .map(|(x, y)| (x * CSS_PER_UNIT, y * CSS_PER_UNIT)),
            );
            stroke(context, &path, palette.route_line(true));
        }
        let view = self.view;
        move_labels_with_map(&mut self.fades, view, (units_x, units_y));
        let size = (width, height);
        let stale = self.tiles_added
            || self
                .placement
                .is_none_or(|last| last.view != view || last.size != size);
        let due = self
            .placement
            .is_none_or(|last| !(0.0..PLACE_MS).contains(&(now - last.time)));
        if stale && due {
            let units = (units_x, units_y);
            let scene = Scene::new(
                view,
                units_x,
                units_y,
                tile_set.max_zoom,
                self.layers,
                |id| tiles.get(id),
            );
            place_labels(
                context,
                &scene,
                units,
                &mut self.fades,
                &mut self.sizes,
                palette,
            );
            self.placement = Some(Placement {
                time: now,
                view,
                size,
            });
            self.tiles_added = false;
        }
        let fading = self.fades.step(now);
        draw_labels(context, &self.fades, palette, ratio, view.zoom())?;
        Ok(fading || stale && !due || cells_wait)
    }
}

/// Decodes a tile and prepares it for [`WebMap::add_tile`]. This is most of the work on a tile,
/// so a worker can do it. `bytes` is the body of the tile response, without gzip. `lang` is a
/// BCP 47 tag, for example `it-IT`: labels use the names in its language when the tile has them.
///
/// Returns `[labels, paths]`: the label candidates as bytes, and the SVG path data of the shapes
/// as `[[class, min_zoom, data], …]`.
///
/// # Errors
///
/// If `bytes` is not a valid vector tile.
#[wasm_bindgen]
pub fn prepare_tile(bytes: &[u8], lang: &str) -> Result<Array, JsValue> {
    let tile = PreparedTile::decode(bytes, lang).map_err(js)?;
    let paths: Array = tile::tile_paths(&tile)
        .into_iter()
        .map(|path| {
            #[expect(clippy::cast_precision_loss, reason = "a class index is small")]
            let class = path.class as f64;
            Array::of3(&class.into(), &path.min_zoom.into(), &path.data.into())
        })
        .collect();
    let labels = Uint8Array::from(wire::encode(tile.labels()).as_slice());
    Ok(Array::of2(&labels, &paths))
}

fn index(value: f64) -> Option<usize> {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is a small whole number"
    )]
    let index = value as usize;
    (value.fract() == 0.0 && value >= 0.0).then_some(index)
}

impl WebMap {
    fn size(&self) -> (f64, f64) {
        (
            f64::from(self.canvas.client_width()),
            f64::from(self.canvas.client_height()),
        )
    }

    fn units(&self) -> (f64, f64) {
        let (width, height) = self.size();
        (width / CSS_PER_UNIT, height / CSS_PER_UNIT)
    }

    fn visible_tiles(&self) -> Vec<TileId> {
        let Some(tile_set) = &self.tile_set else {
            return Vec::new();
        };
        let (width, height) = self.units();
        self.view
            .visible_tiles(width, height, tile_set.max_zoom)
            .into_iter()
            .map(|tile| tile.id)
            .collect()
    }
}

fn flat_ids(ids: Vec<TileId>) -> Vec<u32> {
    ids.into_iter()
        .flat_map(|id| [u32::from(id.z), id.x, id.y])
        .collect()
}

/// Places the labels of `scene` on a screen of `units`, and sets the placed labels to fade in.
fn place_labels(
    context: &CanvasRenderingContext2d,
    scene: &Scene<'_, CanvasTile>,
    (units_x, units_y): (f64, f64),
    fades: &mut Fades<Spot>,
    sizes: &mut Sizes,
    palette: Palette,
) {
    let shown = fades
        .positions()
        .map(|(text, at)| (text, at.screen))
        .collect();
    let placed = labels::place(
        scene
            .labels()
            .into_iter()
            .map(|(scene_tile, index, label)| Candidate {
                placement: css_tile(scene_tile.placement),
                label,
                anchors: scene_tile.tile.anchors(index),
            }),
        Rect::screen(units_x * CSS_PER_UNIT, units_y * CSS_PER_UNIT),
        &shown,
        |label| measure(context, sizes, palette, label),
    );
    let view = scene.view();
    fades.place(placed.into_iter().map(|label| {
        let point = view.point_at(
            label.x / CSS_PER_UNIT,
            label.y / CSS_PER_UNIT,
            units_x,
            units_y,
        );
        let at = Spot {
            point,
            screen: (label.x, label.y),
            zoom: view.zoom(),
            runs: label.runs,
        };
        (label.text, label.class, at, label.kept)
    }));
}

/// Draws each run of each label, turned with its line. `ratio` is the device pixel ratio.
fn draw_labels(
    context: &CanvasRenderingContext2d,
    fades: &Fades<Spot>,
    palette: Palette,
    ratio: f64,
    zoom: f64,
) -> Result<(), JsValue> {
    context.set_text_align("center");
    context.set_text_baseline("middle");
    context.set_line_width(3.0);
    context.set_stroke_style_str(palette.halo());
    for (label, fade) in fades.labels() {
        let text = palette.text(fade.class);
        let (x, y) = fade.at.screen;
        context.set_global_alpha(fade.opacity);
        context.set_font(text.font);
        context.set_fill_style_str(text.color);
        // Between placements, the line under a label scales with the zoom, and its text does not.
        let scale = (zoom - fade.at.zoom).exp2();
        // All halos go first, so that the halo of a run does not cover the text of the run
        // next to it.
        for fill in [false, true] {
            for run in &fade.at.runs {
                let Some(part) = label.get(run.range.clone()) else {
                    continue;
                };
                // web-sys has no setter for `direction`.
                let direction = if run.right_to_left { "rtl" } else { "ltr" };
                Reflect::set(context, &"direction".into(), &direction.into())?;
                let (sin, cos) = run.angle.sin_cos();
                let (dx, dy) = (run.offset.0 * scale, run.offset.1 * scale);
                context.set_transform(
                    ratio * cos,
                    ratio * sin,
                    -ratio * sin,
                    ratio * cos,
                    ratio * (x + dx),
                    ratio * (y + dy),
                )?;
                if fill {
                    context.fill_text(part, 0.0, 0.0)?;
                } else {
                    context.stroke_text(part, 0.0, 0.0)?;
                }
            }
        }
    }
    context.set_transform(ratio, 0.0, 0.0, ratio, 0.0, 0.0)?;
    context.set_global_alpha(1.0);
    Ok(())
}

fn move_labels_with_map(fades: &mut Fades<Spot>, view: View, (units_x, units_y): (f64, f64)) {
    let world = TILE_SIZE * view.zoom().exp2() * CSS_PER_UNIT;
    for fade in fades.labels_mut() {
        let (x, y) = view.screen_position(fade.at.point, units_x, units_y);
        // When the world repeats on the screen, the label stays on its copy of the world.
        let x = x * CSS_PER_UNIT;
        let x = x + ((fade.at.screen.0 - x) / world).round() * world;
        fade.at.screen = (x, y * CSS_PER_UNIT);
    }
}

impl WebMap {
    /// Whether the zoom did not change for [`SETTLE_MS`] before `now`, and whether `style` is not
    /// the cell style of the last frame.
    fn changes(&mut self, now: f64, style: Style) -> (bool, bool) {
        let zoom = self.view.zoom();
        if self
            .zoom_since
            .is_none_or(|(last, _)| (last - zoom).abs() > f64::EPSILON)
        {
            self.zoom_since = Some((zoom, now));
        }
        let settled = self
            .zoom_since
            .is_some_and(|(_, since)| now - since >= SETTLE_MS);
        (settled, self.drawn_style.replace(style) != Some(style))
    }

    /// The style of the cell bitmaps. Labels draw over the bitmaps, so a change of only the labels
    /// keeps the bitmaps.
    fn style(&self) -> Style {
        Style {
            dark: self.dark,
            layers: self.layers.without(Layer::Labels),
        }
    }
}

impl Drop for WebMap {
    fn drop(&mut self) {
        for bitmap in self.rasters.clear() {
            bitmap.close();
        }
    }
}

/// The canvas that draws the cells, one at a time. Each cell becomes an `ImageBitmap`, which does
/// not change, so a browser can keep it where its screen canvas reads it fast. Firefox reads
/// a canvas that is still a draw target back from the GPU at each copy.
struct Painter {
    canvas: OffscreenCanvas,
    context: OffscreenCanvasRenderingContext2d,
    /// The mean time to draw one cell, in milliseconds.
    mean_ms: f64,
}

impl Painter {
    fn new() -> Result<Self, JsValue> {
        let canvas = OffscreenCanvas::new(1, 1)?;
        let options = ContextAttributes2d::new();
        options.set_alpha(false);
        let context = canvas
            .get_context_with_context_options("2d", &options)?
            .ok_or_else(|| JsError::new("the offscreen canvas has no 2D context"))?
            .dyn_into::<OffscreenCanvasRenderingContext2d>()?;
        Ok(Self {
            canvas,
            context,
            mean_ms: 0.0,
        })
    }
}

#[derive(Clone, Copy)]
struct Frame {
    view: View,
    units: (f64, f64),
    max_zoom: u8,
    ratio: f64,
    style: Style,
    /// The zoom did not change for [`SETTLE_MS`].
    settled: bool,
    /// The style changed, so that all cells are drawn in one frame. A bitmap of the old style does
    /// not show, so the cells that wait show as holes.
    restyled: bool,
}

/// Draws the cell bitmaps that are missing or out of date, for at most [`RASTER_BUDGET_MS`], then
/// copies the bitmaps to the screen. Returns `true` if a cell still needs a new bitmap.
fn draw_cells(
    context: &CanvasRenderingContext2d,
    painter: &mut Option<Painter>,
    rasters: &mut RasterCache<ImageBitmap>,
    tiles: &TileCache<CanvasTile>,
    frame: Frame,
) -> Result<bool, JsValue> {
    let Frame {
        view,
        units: (units_x, units_y),
        ratio,
        ..
    } = frame;
    let zoom = view.zoom();
    let cells: Vec<_> = raster::grid(view, units_x, units_y)
        .into_iter()
        .map(|cell| {
            let source = raster::source(&cell, frame.max_zoom, |id| tiles.get(id).is_some());
            (css_tile(cell), source.map(css_tile))
        })
        .collect();
    let look = |source: &VisibleTile| Look {
        source: source.id,
        zoom,
        ratio,
        style: frame.style,
    };
    rasters.begin_frame();
    let mut jobs: Vec<_> = cells
        .iter()
        .filter_map(|(cell, source)| {
            let need = rasters.need(cell.id, look(source.as_ref()?), frame.settled)?;
            Some((
                need,
                rasters.shows(cell, frame.style),
                cell,
                source.as_ref()?,
            ))
        })
        .collect();
    // A cell without a bitmap of the style shows as a hole, so it comes first. All cells have
    // the same zoom, so a drawn cell does not change what another cell shows.
    jobs.sort_by_key(|&(need, shows, ..)| (shows, need));
    let performance = web_sys::window().and_then(|window| window.performance());
    let now = || performance.as_ref().map_or(0.0, web_sys::Performance::now);
    let start = now();
    let mut drawn = HashSet::new();
    for (need, shows, cell, source) in jobs {
        let hole = !shows;
        let mean_ms = painter.as_ref().map_or(0.0, |painter| painter.mean_ms);
        let fits = now() - start + mean_ms <= RASTER_BUDGET_MS
            && (frame.settled || drawn.len() < MOVING_CELLS);
        if !(frame.restyled || fits || drawn.is_empty() && (frame.settled || hole)) {
            break;
        }
        // A cell shows more than once when the world is narrower than the screen.
        if !drawn.insert(cell.id)
            || rasters.need(cell.id, look(source), frame.settled) != Some(need)
        {
            continue;
        }
        let Some(tile) = tiles.get(source.id) else {
            continue;
        };
        let painter = match painter {
            Some(painter) => painter,
            None => painter.insert(Painter::new()?),
        };
        let paint_start = now();
        let bitmap = paint_cell(painter, cell, source, tile, look(source))?;
        painter.mean_ms += (now() - paint_start - painter.mean_ms) * PAINT_MS_WEIGHT;
        let side = f64::from(bitmap.width());
        if let Some(old) = rasters.insert(cell.id, bitmap, look(source), side * side) {
            old.close();
        }
    }

    context.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0)?;
    for (cell, _) in &cells {
        if let Some((bitmap, drawn)) = rasters.get(cell.id, frame.style) {
            blit(context, bitmap, cell, drawn, zoom, ratio)?;
            continue;
        }
        let fallback = rasters.fallback(cell, frame.style);
        if fallback.is_empty() {
            continue;
        }
        context.save();
        clip_to(context, device_rect(cell, ratio));
        for other in fallback {
            if let Some((bitmap, drawn)) = rasters.get(other.id, frame.style) {
                blit(context, bitmap, &other, drawn, zoom, ratio)?;
            }
        }
        context.restore();
    }
    let screen = f64::from(
        context
            .canvas()
            .map_or(0, |canvas| canvas.width() * canvas.height()),
    );
    for bitmap in rasters.evict(RASTER_SCREENS * screen) {
        bitmap.close();
    }
    Ok(cells.iter().any(|(cell, source)| {
        source
            .as_ref()
            .is_some_and(|source| rasters.need(cell.id, look(source), true).is_some())
    }))
}

/// Draws the shapes of `tile`, at `source` on the screen, into a new bitmap of `cell`. Both
/// placements are in CSS pixels.
fn paint_cell(
    painter: &Painter,
    cell: &VisibleTile,
    source: &VisibleTile,
    tile: &CanvasTile,
    look: Look,
) -> Result<ImageBitmap, JsValue> {
    let Look {
        zoom,
        ratio,
        style: Style { dark, layers },
        ..
    } = look;
    let palette = Palette::new(dark);
    let side = raster::bitmap_side(cell.size, ratio);
    let Painter {
        canvas, context, ..
    } = painter;
    if canvas.width() != side || canvas.height() != side {
        canvas.set_width(side);
        canvas.set_height(side);
    }
    context.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0)?;
    context.set_fill_style_str(palette.background());
    context.fill_rect(0.0, 0.0, f64::from(side), f64::from(side));
    context.set_line_join("round");
    context.set_line_cap("round");
    let scale = source.size / UNITS;
    let bleed = f64::from(BLEED);
    context.set_transform(
        ratio * scale,
        0.0,
        0.0,
        ratio * scale,
        bleed + (source.left - cell.left) * ratio,
        bleed + (source.top - cell.top) * ratio,
    )?;
    for class in Class::ALL.into_iter().filter(|&class| layers.shows(class)) {
        let (area, line) = (palette.area(class), palette.line(class, zoom));
        for path in tile.paths(class, zoom) {
            if let Some(color) = area {
                context.set_fill_style_str(color);
                context.fill_with_path_2d(path);
            } else if let Some(line) = line {
                context.set_stroke_style_str(line.color);
                context.set_line_width(line.width / scale);
                context.stroke_with_path(path);
            }
        }
    }
    canvas.transfer_to_image_bitmap()
}

/// Copies the bitmap of `cell`, drawn as `drawn` shows, to its place on the screen at `zoom` and
/// `ratio`. The context has no transform: the positions are in device pixels.
fn blit(
    context: &CanvasRenderingContext2d,
    bitmap: &ImageBitmap,
    cell: &VisibleTile,
    drawn: Look,
    zoom: f64,
    ratio: f64,
) -> Result<(), JsValue> {
    let bleed = f64::from(BLEED);
    let scale = (zoom - drawn.zoom).exp2() * ratio / drawn.ratio;
    if (scale - 1.0).abs() <= f64::EPSILON {
        return context.draw_image_with_image_bitmap(
            bitmap,
            (cell.left * ratio).round() - bleed,
            (cell.top * ratio).round() - bleed,
        );
    }
    let side = f64::from(bitmap.width()) * scale;
    context.draw_image_with_image_bitmap_and_dw_and_dh(
        bitmap,
        cell.left * ratio - bleed * scale,
        cell.top * ratio - bleed * scale,
        side,
        side,
    )
}

fn device_rect(cell: &VisibleTile, ratio: f64) -> Rect {
    Rect {
        left: (cell.left * ratio).round(),
        top: (cell.top * ratio).round(),
        right: ((cell.left + cell.size) * ratio).round(),
        bottom: ((cell.top + cell.size) * ratio).round(),
    }
}

/// The 2D context of `canvas`, without an alpha channel: the map covers the whole canvas, and an
/// opaque canvas is faster to draw and to show.
fn opaque_context(canvas: &HtmlCanvasElement) -> Result<CanvasRenderingContext2d, JsValue> {
    let options = ContextAttributes2d::new();
    options.set_alpha(false);
    Ok(canvas
        .get_context_with_context_options("2d", &options)?
        .ok_or_else(|| JsError::new("the canvas has no 2D context"))?
        .dyn_into::<CanvasRenderingContext2d>()?)
}

fn css_tile(placement: VisibleTile) -> VisibleTile {
    VisibleTile {
        left: placement.left * CSS_PER_UNIT,
        top: placement.top * CSS_PER_UNIT,
        size: placement.size * CSS_PER_UNIT,
        ..placement
    }
}

fn add_line(path: &Path2d, points: impl IntoIterator<Item = ScreenPoint>) {
    let mut points = points.into_iter();
    let Some((x, y)) = points.next() else {
        return;
    };
    path.move_to(x, y);
    for (x, y) in points {
        path.line_to(x, y);
    }
}

fn clip_to(context: &CanvasRenderingContext2d, rect: Rect) {
    context.begin_path();
    context.rect(
        rect.left,
        rect.top,
        rect.right - rect.left,
        rect.bottom - rect.top,
    );
    context.clip();
}

fn stroke(context: &CanvasRenderingContext2d, path: &Path2d, line: Line) {
    context.set_stroke_style_str(line.color);
    context.set_line_width(line.width);
    context.stroke_with_path(path);
}

/// The size of the text of `label` in its label font. `measureText` is slow, so each text is
/// measured once.
fn measure(
    context: &CanvasRenderingContext2d,
    sizes: &mut Sizes,
    palette: Palette,
    label: &LabelCandidate,
) -> Option<Size> {
    let on_line = labels::on_line(label);
    let sizes = sizes.entry((label.class, on_line)).or_default();
    if let Some(size) = sizes.get(&label.text) {
        return Some(size.clone());
    }
    if sizes.len() >= MAX_SIZES {
        sizes.clear();
    }
    let text = palette.text(label.class);
    context.set_font(text.font);
    let width_of = |part: &str| Some(context.measure_text(part).ok()?.width());
    let height = f64::from(text.size);
    let size = if on_line {
        Size::measure(&label.text, height, width_of)?
    } else {
        Size::of_point(&label.text, width_of(&label.text)?, height)
    };
    sizes.insert(label.text.clone(), size.clone());
    Some(size)
}

fn device_pixels(css: f64) -> u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the u32 range"
    )]
    let pixels = css.round().clamp(0.0, f64::from(u32::MAX)) as u32;
    pixels
}

fn js(error: tmaps_core::Error) -> JsValue {
    JsError::from(error).into()
}
