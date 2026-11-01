use tmaps_core::PreparedTile;
use tmaps_core::mvt::Tile;
use tmaps_core::tiles::TileId;

fn fixture_bytes(id: TileId) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/tiles/{}-{}-{}.pbf",
        env!("CARGO_MANIFEST_DIR"),
        id.z,
        id.x,
        id.y
    );
    std::fs::read(path).unwrap()
}

const MILAN_Z14: TileId = TileId {
    z: 14,
    x: 8610,
    y: 5863,
};
const MILAN_Z5: TileId = TileId { z: 5, x: 16, y: 11 };

#[test]
fn decode_skips_only_the_layers_that_prepare_drops() {
    for id in [MILAN_Z14, MILAN_Z5] {
        let full = PreparedTile::new(&Tile::decode(&fixture_bytes(id)).unwrap(), "en");
        let decoded = PreparedTile::decode(&fixture_bytes(id), "en").unwrap();
        assert_eq!(decoded, full, "tile {id}");
    }
}
