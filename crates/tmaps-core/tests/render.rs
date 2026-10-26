use std::f64::consts::PI;

use tmaps_core::LatLon;
use tmaps_core::mvt::Tile;
use tmaps_core::render::{Canvas, Class, PreparedTile, View, render};
use tmaps_core::tiles::{DEFAULT_MAX_ZOOM, TileId};

const WIDTH: u16 = 100;
const HEIGHT: u16 = 40;

fn fixture(id: TileId) -> PreparedTile {
    let path = format!(
        "{}/tests/fixtures/tiles/{}-{}-{}.pbf",
        env!("CARGO_MANIFEST_DIR"),
        id.z,
        id.x,
        id.y
    );
    PreparedTile::new(&Tile::decode(&std::fs::read(path).unwrap()).unwrap())
}

fn center_of(id: TileId) -> LatLon {
    let tiles = f64::from(1_u32 << id.z);
    let x = (f64::from(id.x) + 0.5) / tiles;
    let y = (f64::from(id.y) + 0.5) / tiles;
    let lat = (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees();
    LatLon::new(lat, x * 360.0 - 180.0).unwrap()
}

/// Renders the view with only the fixture tile loaded, as braille rows and then class rows.
fn snapshot(id: TileId, zoom: f64) -> String {
    let tile = fixture(id);
    let view = View::new(center_of(id), zoom);
    let canvas = render(view, WIDTH, HEIGHT, DEFAULT_MAX_ZOOM, |requested| {
        (requested == id).then_some(&tile)
    });
    format!("{}\n\n{}", braille_rows(&canvas), class_rows(&canvas))
}

fn braille_rows(canvas: &Canvas) -> String {
    let rows: Vec<String> = canvas
        .rows()
        .map(|row| row.iter().map(|cell| cell.symbol()).collect())
        .collect();
    rows.join("\n")
}

fn class_rows(canvas: &Canvas) -> String {
    let rows: Vec<String> = canvas
        .rows()
        .map(|row| {
            row.iter()
                .map(|cell| cell.fg.or(cell.bg).map_or(' ', letter))
                .collect()
        })
        .collect();
    rows.join("\n")
}

fn letter(class: Class) -> char {
    match class {
        Class::Green => 'g',
        Class::Water => 'w',
        Class::Building => 'b',
        Class::Boundary => 'B',
        Class::Waterway => 'r',
        Class::Rail => 't',
        Class::Path => 'p',
        Class::MinorRoad => 'm',
        Class::MajorRoad => 'M',
        Class::Highway => 'H',
    }
}

const MILAN_Z14: TileId = TileId {
    z: 14,
    x: 8610,
    y: 5863,
};
const MILAN_Z5: TileId = TileId { z: 5, x: 16, y: 11 };

#[test]
fn render_city_streets() {
    insta::assert_snapshot!(snapshot(MILAN_Z14, 14.0));
}

#[test]
fn render_overzoomed_buildings() {
    insta::assert_snapshot!(snapshot(MILAN_Z14, 16.0));
}

#[test]
fn render_region() {
    insta::assert_snapshot!(snapshot(MILAN_Z5, 5.0));
}
