use std::f64::consts::PI;

use tmaps_braille::{BrailleTile, Frame, Grid, RouteLine, render};
use tmaps_core::label::LabelClass;
use tmaps_core::style::{Class, Layer, Layers};
use tmaps_core::tiles::{DEFAULT_MAX_ZOOM, TileId};
use tmaps_core::view::View;
use tmaps_core::{LatLon, PreparedTile};

const WIDTH: u16 = 100;
const HEIGHT: u16 = 40;

fn fixture(id: TileId, lang: &str) -> BrailleTile {
    let path = format!(
        "{}/../tmaps-core/tests/fixtures/tiles/{}-{}-{}.pbf",
        env!("CARGO_MANIFEST_DIR"),
        id.z,
        id.x,
        id.y
    );
    BrailleTile::prepare(PreparedTile::decode(&std::fs::read(path).unwrap(), lang).unwrap())
}

fn center_of(id: TileId) -> LatLon {
    let tiles = f64::from(1_u32 << id.z);
    let x = (f64::from(id.x) + 0.5) / tiles;
    let y = (f64::from(id.y) + 0.5) / tiles;
    let lat = (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees();
    LatLon::new(lat, x * 360.0 - 180.0).unwrap()
}

fn snapshot(id: TileId, zoom: f64, lang: &str) -> String {
    snapshot_with_routes(id, zoom, lang, &[])
}

/// Renders the view with only the fixture tile loaded: braille rows, class rows, then labels.
fn snapshot_with_routes(id: TileId, zoom: f64, lang: &str, routes: &[RouteLine<'_>]) -> String {
    let grid = render_fixture(id, zoom, lang, routes, Layers::ALL);
    format!(
        "{}\n\n{}\n\n{}",
        braille_rows(&grid),
        class_rows(&grid),
        labels(&grid)
    )
}

fn render_fixture(
    id: TileId,
    zoom: f64,
    lang: &str,
    routes: &[RouteLine<'_>],
    layers: Layers,
) -> Grid {
    let tile = fixture(id, lang);
    let frame = Frame {
        view: View::new(center_of(id), zoom),
        columns: WIDTH,
        rows: HEIGHT,
        max_zoom: DEFAULT_MAX_ZOOM,
        layers,
    };
    let cursor = (WIDTH / 2, HEIGHT / 2);
    render(frame, routes, &[cursor], |requested| {
        (requested == id).then_some(&tile)
    })
}

fn road_cells(grid: &Grid) -> usize {
    grid.rows()
        .flat_map(|row| row.iter())
        .filter(|cell| {
            [cell.fg, cell.bg]
                .into_iter()
                .flatten()
                .any(|class| class.layer() == Some(Layer::Roads))
        })
        .count()
}

fn labels(grid: &Grid) -> String {
    let lines: Vec<String> = grid
        .labels()
        .iter()
        .map(|label| {
            format!(
                "{:>2},{:>3} {:?}: {}",
                label.row, label.column, label.class, label.text
            )
        })
        .collect();
    lines.join("\n")
}

fn braille_rows(grid: &Grid) -> String {
    let rows: Vec<String> = grid
        .rows()
        .map(|row| row.iter().map(|cell| cell.symbol()).collect())
        .collect();
    rows.join("\n")
}

fn class_rows(grid: &Grid) -> String {
    let rows: Vec<String> = grid
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
        Class::Alternative => 'a',
        Class::Route => 'R',
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
    insta::assert_snapshot!(snapshot(MILAN_Z14, 14.0, "it-IT"));
}

#[test]
fn render_overzoomed_buildings() {
    insta::assert_snapshot!(snapshot(MILAN_Z14, 16.0, "it-IT"));
}

#[test]
fn render_region() {
    insta::assert_snapshot!(snapshot(MILAN_Z5, 5.0, "it-IT"));
}

#[test]
fn render_region_in_english() {
    insta::assert_snapshot!(snapshot(MILAN_Z5, 5.0, "en-US"));
}

#[test]
fn render_parent_while_tiles_load() {
    insta::assert_snapshot!(snapshot(MILAN_Z5, 6.0, "it-IT"));
}

#[test]
fn render_route_over_the_streets() {
    let center = center_of(MILAN_Z14);
    let at = |dlat: f64, dlon: f64| LatLon::new(center.lat() + dlat, center.lon() + dlon).unwrap();
    let route = [at(0.004, -0.008), at(0.004, 0.002), at(-0.004, 0.006)];
    let alternative = [at(0.004, -0.008), at(-0.004, -0.004), at(-0.004, 0.006)];
    let routes = [
        RouteLine {
            points: &route,
            selected: true,
        },
        RouteLine {
            points: &alternative,
            selected: false,
        },
    ];
    insta::assert_snapshot!(snapshot_with_routes(MILAN_Z14, 14.0, "it-IT", &routes));
}

#[test]
fn render_city_streets_draws_road_cells() {
    let grid = render_fixture(MILAN_Z14, 14.0, "it-IT", &[], Layers::ALL);
    assert_ne!(road_cells(&grid), 0);
}

#[test]
fn render_without_roads_draws_no_road_cells() {
    let layers = Layers::ALL.toggled(Layer::Roads);
    let grid = render_fixture(MILAN_Z14, 14.0, "it-IT", &[], layers);
    assert_eq!(road_cells(&grid), 0);
}

#[test]
fn render_overzoomed_streets_places_road_labels() {
    let grid = render_fixture(MILAN_Z14, 16.0, "it-IT", &[], Layers::ALL);
    assert!(
        grid.labels()
            .iter()
            .any(|label| label.class == LabelClass::Road)
    );
}

#[test]
fn render_without_roads_places_no_road_labels() {
    let layers = Layers::ALL.toggled(Layer::Roads);
    let grid = render_fixture(MILAN_Z14, 16.0, "it-IT", &[], layers);
    assert!(
        grid.labels()
            .iter()
            .all(|label| label.class != LabelClass::Road)
    );
}

#[test]
fn render_without_labels_places_no_labels() {
    let layers = Layers::ALL.toggled(Layer::Labels);
    let grid = render_fixture(MILAN_Z14, 16.0, "it-IT", &[], layers);
    assert_eq!(grid.labels().len(), 0);
}
