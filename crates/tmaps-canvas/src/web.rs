use std::collections::HashMap;

use js_sys::{Array, Uint8Array};
use tmaps_core::label::LabelClass;
use tmaps_core::scene::{Rect, Scene, ScreenPoint};
use tmaps_core::style::Class;
use tmaps_core::tiles::{TileId, TileSet};
use tmaps_core::view::{TILE_SIZE, View, VisibleTile};
use tmaps_core::{LatLon, PreparedTile};
use wasm_bindgen::prelude::*;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, Path2d};

use crate::cache::TileCache;
use crate::fade::Fades;
use crate::labels::{self, Candidate};
use crate::paint::{Line, Palette};
use crate::tile::{self, CanvasTile, TilePath, UNITS};
use crate::wire;

// Tiles show at 512 px, the size that OpenMapTiles styles and MapLibre use, so a zoom level
// means the same scale as in MapLibre.
const CSS_PER_UNIT: f64 = 2.0;

/// The shortest time between two placements of the labels, in milliseconds. At low zoom, a
/// placement takes milliseconds. Between placements, the labels move with the map.
const PLACE_MS: f64 = 100.0;

/// A map on an HTML canvas. JavaScript fetches the `TileJSON` document and the tiles, and gives
/// them to the map: the map does no I/O. All positions are in CSS pixels.
#[wasm_bindgen]
pub struct WebMap {
    canvas: HtmlCanvasElement,
    context: CanvasRenderingContext2d,
    tile_set: Option<TileSet>,
    view: View,
    tiles: TileCache<CanvasTile>,
    route: Vec<LatLon>,
    dark: bool,
    widths: HashMap<LabelClass, HashMap<String, f64>>,
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

/// The position of a label on the map, and its position on the screen in the last frame.
#[derive(Clone, Copy)]
struct Spot {
    point: LatLon,
    screen: ScreenPoint,
}

#[wasm_bindgen]
impl WebMap {
    /// # Errors
    ///
    /// If the canvas has no 2D context.
    #[wasm_bindgen(constructor)]
    pub fn new(canvas: HtmlCanvasElement) -> Result<WebMap, JsValue> {
        let context = canvas
            .get_context("2d")?
            .ok_or_else(|| JsError::new("the canvas has no 2D context"))?
            .dyn_into::<CanvasRenderingContext2d>()?;
        Ok(Self {
            canvas,
            context,
            tile_set: None,
            view: View::new(LatLon::new(0.0, 0.0).map_err(js)?, 1.0),
            tiles: TileCache::default(),
            route: Vec::new(),
            dark: false,
            widths: HashMap::new(),
            fades: Fades::default(),
            placement: None,
            tiles_added: false,
        })
    }

    /// # Errors
    ///
    /// If `tilejson` is not a `TileJSON` document with a tile URL.
    pub fn set_tile_set(&mut self, tilejson: &str) -> Result<(), JsValue> {
        self.tile_set = Some(TileSet::from_json(tilejson.as_bytes()).map_err(js)?);
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
        let Some(tile_set) = &self.tile_set else {
            return Vec::new();
        };
        let (width, height) = self.units();
        let visible = self.view.visible_tiles(width, height, tile_set.max_zoom);
        self.tiles
            .request(visible.into_iter().map(|tile| tile.id))
            .into_iter()
            .flat_map(|id| [u32::from(id.z), id.x, id.y])
            .collect()
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
    /// gives. Returns `true` while labels fade in or out: draw one more frame then.
    ///
    /// # Errors
    ///
    /// If the browser rejects a canvas call.
    pub fn draw(&mut self, now: f64) -> Result<bool, JsValue> {
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
        let scene = Scene::new(self.view, units_x, units_y, tile_set.max_zoom, |id| {
            tiles.get(id)
        });
        context.set_line_join("round");
        context.set_line_cap("round");
        for class in Class::ALL {
            draw_class(context, &scene, class, palette)?;
        }
        if self.route.len() > 1 {
            let path = Path2d::new()?;
            let points = scene.line_on_screen(&self.route);
            add_line(
                &path,
                points
                    .into_iter()
                    .map(|(x, y)| (x * CSS_PER_UNIT, y * CSS_PER_UNIT)),
            );
            stroke(context, &path, palette.route_line(true));
        }
        let view = self.view;
        let world = TILE_SIZE * view.zoom().exp2() * CSS_PER_UNIT;
        for fade in self.fades.labels_mut() {
            let (x, y) = view.screen_position(fade.at.point, units_x, units_y);
            // When the world repeats on the screen, the label stays on its copy of the world.
            let x = x * CSS_PER_UNIT;
            let x = x + ((fade.at.screen.0 - x) / world).round() * world;
            fade.at.screen = (x, y * CSS_PER_UNIT);
        }
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
            place_labels(
                context,
                &scene,
                units,
                &mut self.fades,
                &mut self.widths,
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
        context.set_text_align("center");
        context.set_text_baseline("middle");
        context.set_line_width(3.0);
        context.set_stroke_style_str(palette.halo());
        for (label, fade) in self.fades.labels() {
            let text = palette.text(fade.class);
            let (x, y) = fade.at.screen;
            context.set_global_alpha(fade.opacity);
            context.set_font(text.font);
            context.set_fill_style_str(text.color);
            context.stroke_text(label, x, y)?;
            context.fill_text(label, x, y)?;
        }
        context.set_global_alpha(1.0);
        Ok(fading || stale && !due)
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
}

/// Places the labels of `scene` on a screen of `units`, and sets the placed labels to fade in.
fn place_labels(
    context: &CanvasRenderingContext2d,
    scene: &Scene<'_, CanvasTile>,
    (units_x, units_y): (f64, f64),
    fades: &mut Fades<Spot>,
    widths: &mut HashMap<LabelClass, HashMap<String, f64>>,
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
        |text, class| {
            let width = measure(context, widths, palette, text, class)?;
            Some((width, f64::from(palette.text(class).size)))
        },
    );
    let view = scene.view();
    fades.place(placed.iter().map(|label| {
        let point = view.point_at(
            label.x / CSS_PER_UNIT,
            label.y / CSS_PER_UNIT,
            units_x,
            units_y,
        );
        let at = Spot {
            point,
            screen: (label.x, label.y),
        };
        (label.text, label.class, at, label.kept)
    }));
}

/// Fills or strokes the paths of `class`, with one transform for each tile.
fn draw_class(
    context: &CanvasRenderingContext2d,
    scene: &Scene<'_, CanvasTile>,
    class: Class,
    palette: Palette,
) -> Result<(), JsValue> {
    let zoom = scene.view().zoom();
    let (area, line) = (palette.area(class), palette.line(class, zoom));
    for scene_tile in scene.tiles() {
        let mut paths = scene_tile.tile.paths(class, zoom).peekable();
        if paths.peek().is_none() {
            continue;
        }
        context.save();
        clip_to(context, css_rect(scene_tile.clip));
        let VisibleTile {
            left, top, size, ..
        } = css_tile(scene_tile.placement);
        let scale = size / UNITS;
        context.transform(scale, 0.0, 0.0, scale, left, top)?;
        for path in paths {
            if let Some(color) = area {
                context.set_fill_style_str(color);
                context.fill_with_path_2d(path);
            } else if let Some(line) = line {
                stroke(
                    context,
                    path,
                    Line {
                        width: line.width / scale,
                        ..line
                    },
                );
            }
        }
        context.restore();
    }
    Ok(())
}

fn css_tile(placement: VisibleTile) -> VisibleTile {
    VisibleTile {
        left: placement.left * CSS_PER_UNIT,
        top: placement.top * CSS_PER_UNIT,
        size: placement.size * CSS_PER_UNIT,
        ..placement
    }
}

fn css_rect(rect: Rect) -> Rect {
    Rect {
        left: rect.left * CSS_PER_UNIT,
        top: rect.top * CSS_PER_UNIT,
        right: rect.right * CSS_PER_UNIT,
        bottom: rect.bottom * CSS_PER_UNIT,
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

/// The width of `text` in its label font. `measureText` is slow, so each width is measured once.
fn measure(
    context: &CanvasRenderingContext2d,
    widths: &mut HashMap<LabelClass, HashMap<String, f64>>,
    palette: Palette,
    text: &str,
    class: LabelClass,
) -> Option<f64> {
    let widths = widths.entry(class).or_default();
    if let Some(&width) = widths.get(text) {
        return Some(width);
    }
    context.set_font(palette.text(class).font);
    let width = context.measure_text(text).ok()?.width();
    widths.insert(text.to_owned(), width);
    Some(width)
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
