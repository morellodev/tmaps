use std::collections::HashMap;

use tmaps_core::label::LabelClass;
use tmaps_core::scene::{Rect, Scene, ScreenPoint};
use tmaps_core::style::Class;
use tmaps_core::tiles::{TileId, TileSet};
use tmaps_core::view::{View, VisibleTile};
use tmaps_core::{ATTRIBUTION, LatLon, PreparedTile};
use wasm_bindgen::prelude::*;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, Path2d};

use crate::cache::TileCache;
use crate::labels;
use crate::paint::{Line, Palette};
use crate::tile::{CanvasTile, UNITS};

/// A map on an HTML canvas. JavaScript fetches the `TileJSON` document and the tiles, and gives
/// them to the map: the map does no I/O. All positions are in CSS pixels.
#[wasm_bindgen]
pub struct WebMap {
    canvas: HtmlCanvasElement,
    context: CanvasRenderingContext2d,
    lang: String,
    tile_set: Option<TileSet>,
    view: View,
    tiles: TileCache<CanvasTile>,
    route: Vec<LatLon>,
    dark: bool,
    widths: HashMap<LabelClass, HashMap<String, f64>>,
}

#[wasm_bindgen]
impl WebMap {
    /// `lang` is a BCP 47 tag, for example `it-IT`. Labels use the names in its language when the
    /// tiles have them.
    ///
    /// # Errors
    ///
    /// If the canvas has no 2D context.
    #[wasm_bindgen(constructor)]
    pub fn new(canvas: HtmlCanvasElement, lang: String) -> Result<WebMap, JsValue> {
        let context = canvas
            .get_context("2d")?
            .ok_or_else(|| JsError::new("the canvas has no 2D context"))?
            .dyn_into::<CanvasRenderingContext2d>()?;
        Ok(Self {
            canvas,
            context,
            lang,
            tile_set: None,
            view: View::new(LatLon::new(0.0, 0.0).map_err(js)?, 1.0),
            tiles: TileCache::default(),
            route: Vec::new(),
            dark: false,
            widths: HashMap::new(),
        })
    }

    /// # Errors
    ///
    /// If `tilejson` is not a `TileJSON` document with a tile URL.
    pub fn set_tile_set(&mut self, tilejson: &str) -> Result<(), JsValue> {
        self.tile_set = Some(TileSet::from_json(tilejson.as_bytes()).map_err(js)?);
        Ok(())
    }

    /// The credit line to show with the map, in plain text.
    #[must_use]
    pub fn attribution(&self) -> String {
        credit(
            self.tile_set
                .as_ref()
                .map_or("", |tile_set| &tile_set.attribution),
        )
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
        self.view = self.view.pan(dx, dy);
    }

    /// Zooms by `delta` levels and keeps the point at `(x, y)` in its place, for example the
    /// point under the pointer.
    pub fn zoom_at(&mut self, delta: f64, x: f64, y: f64) {
        let (width, height) = self.size();
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
        let (width, height) = self.size();
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

    /// Adds a fetched tile. `bytes` is the body of the response, without gzip.
    ///
    /// # Errors
    ///
    /// If `bytes` is not a valid vector tile.
    pub fn add_tile(&mut self, z: u8, x: u32, y: u32, bytes: &[u8]) -> Result<(), JsValue> {
        let tile = PreparedTile::decode(bytes, &self.lang).map_err(js)?;
        self.tiles.add(TileId { z, x, y }, CanvasTile::new(tile)?);
        Ok(())
    }

    /// Draws the map at the size of the canvas element, with sharp lines on high-density screens.
    ///
    /// # Errors
    ///
    /// If the browser rejects a canvas call.
    pub fn draw(&mut self) -> Result<(), JsValue> {
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
            return Ok(());
        };
        let tiles = &self.tiles;
        let scene = Scene::new(self.view, width, height, tile_set.max_zoom, |id| {
            tiles.get(id)
        });
        context.set_line_join("round");
        context.set_line_cap("round");
        for class in Class::ALL {
            draw_class(context, &scene, class, palette)?;
        }
        if self.route.len() > 1 {
            let path = Path2d::new()?;
            add_line(&path, scene.line_on_screen(&self.route));
            stroke(context, &path, palette.route_line(true));
        }
        let widths = &mut self.widths;
        let placed = labels::place(
            scene
                .labels()
                .into_iter()
                .map(|(scene_tile, _, candidate)| (scene_tile.placement, candidate)),
            scene.screen(),
            |text, class| {
                let width = measure(context, widths, palette, text, class)?;
                Some((width, f64::from(palette.text(class).size)))
            },
        );
        context.set_text_align("center");
        context.set_text_baseline("middle");
        context.set_line_width(3.0);
        context.set_stroke_style_str(palette.halo());
        for label in placed {
            let text = palette.text(label.class);
            context.set_font(text.font);
            context.set_fill_style_str(text.color);
            context.stroke_text(label.text, label.x, label.y)?;
            context.fill_text(label.text, label.x, label.y)?;
        }
        Ok(())
    }
}

impl WebMap {
    fn size(&self) -> (f64, f64) {
        (
            f64::from(self.canvas.client_width()),
            f64::from(self.canvas.client_height()),
        )
    }
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
        clip_to(context, scene_tile.clip);
        let VisibleTile {
            left, top, size, ..
        } = scene_tile.placement;
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

/// The credit of the tiles, with the OpenStreetMap credit when the tiles do not give it.
fn credit(tiles: &str) -> String {
    if tiles.is_empty() {
        ATTRIBUTION.to_owned()
    } else if tiles.contains("OpenStreetMap") {
        tiles.to_owned()
    } else {
        format!("{tiles} · {ATTRIBUTION}")
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_names_openstreetmap_once() {
        assert_eq!(credit(""), ATTRIBUTION);
        assert_eq!(
            credit("OpenFreeMap © OpenMapTiles Data from OpenStreetMap"),
            "OpenFreeMap © OpenMapTiles Data from OpenStreetMap"
        );
        assert_eq!(credit("Tiles by X"), format!("Tiles by X · {ATTRIBUTION}"));
    }
}
