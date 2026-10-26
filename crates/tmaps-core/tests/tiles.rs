use std::time::SystemTime;

use tmaps_core::http;
use tmaps_core::tiles::{DEFAULT_CACHE_BYTES, DiskCache, TileId, TileSource};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TILE: TileId = TileId { z: 14, x: 1, y: 2 };
/// Below the `TileJSON` path, as on `OpenFreeMap`.
const TILE_PATH: &str = "/planet/20261018/14/1/2.pbf";

struct Fixture {
    server: MockServer,
    _cache_dir: tempfile::TempDir,
    source: TileSource,
}

async fn fixture() -> Fixture {
    let server = MockServer::start().await;
    let tilejson = serde_json::json!({
        "tiles": [format!("{}/planet/20261018/{{z}}/{{x}}/{{y}}.pbf", server.uri())],
        "minzoom": 0,
        "maxzoom": 14,
        "attribution": "<a href=\"https://openfreemap.org\">OpenFreeMap</a> &copy; OpenStreetMap",
    });
    Mock::given(method("GET"))
        .and(path("/planet"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tilejson))
        .mount(&server)
        .await;
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = DiskCache::new(cache_dir.path().to_owned(), DEFAULT_CACHE_BYTES);
    let source = TileSource::new(
        http::client().unwrap(),
        &format!("{}/planet", server.uri()),
        cache,
    );
    Fixture {
        server,
        _cache_dir: cache_dir,
        source,
    }
}

async fn mount_tile(server: &MockServer, response: ResponseTemplate, calls: u64) {
    Mock::given(method("GET"))
        .and(path(TILE_PATH))
        .respond_with(response)
        .expect(calls)
        .mount(server)
        .await;
}

#[tokio::test]
async fn tile_set_reads_the_tilejson() {
    let fixture = fixture().await;
    let tile_set = fixture.source.tile_set().await.unwrap();
    assert_eq!(
        (tile_set.max_zoom, tile_set.attribution.as_str()),
        (14, "OpenFreeMap © OpenStreetMap")
    );
}

#[tokio::test]
async fn tile_returns_the_body() {
    let fixture = fixture().await;
    mount_tile(
        &fixture.server,
        ResponseTemplate::new(200).set_body_bytes(b"tile"),
        1,
    )
    .await;
    assert_eq!(fixture.source.tile(TILE).await.unwrap(), b"tile");
}

#[tokio::test]
async fn tile_comes_from_the_cache_the_second_time() {
    let fixture = fixture().await;
    let response = ResponseTemplate::new(200)
        .insert_header("cache-control", "max-age=60")
        .set_body_bytes(b"tile");
    mount_tile(&fixture.server, response, 1).await;
    fixture.source.tile(TILE).await.unwrap();
    assert_eq!(fixture.source.tile(TILE).await.unwrap(), b"tile");
}

#[tokio::test]
async fn tile_with_a_huge_max_age_is_cached() {
    let fixture = fixture().await;
    let response = ResponseTemplate::new(200)
        .insert_header("cache-control", format!("max-age={}", u64::MAX))
        .set_body_bytes(b"tile");
    mount_tile(&fixture.server, response, 1).await;
    fixture.source.tile(TILE).await.unwrap();
    assert_eq!(fixture.source.tile(TILE).await.unwrap(), b"tile");
}

#[tokio::test]
async fn tile_with_no_store_is_fetched_again() {
    let fixture = fixture().await;
    let response = ResponseTemplate::new(200)
        .insert_header("cache-control", "no-store")
        .set_body_bytes(b"tile");
    mount_tile(&fixture.server, response, 2).await;
    fixture.source.tile(TILE).await.unwrap();
    fixture.source.tile(TILE).await.unwrap();
}

#[tokio::test]
async fn tile_without_content_is_empty() {
    let fixture = fixture().await;
    mount_tile(&fixture.server, ResponseTemplate::new(204), 1).await;
    assert_eq!(fixture.source.tile(TILE).await.unwrap(), Vec::<u8>::new());
}

#[tokio::test]
async fn tile_falls_back_to_the_expired_copy_when_the_server_fails() {
    let fixture = fixture().await;
    let url = format!("{}{TILE_PATH}", fixture.server.uri());
    fixture
        .source
        .cache()
        .put(&url, b"stale tile", SystemTime::UNIX_EPOCH)
        .await;
    mount_tile(&fixture.server, ResponseTemplate::new(500), 1).await;
    assert_eq!(fixture.source.tile(TILE).await.unwrap(), b"stale tile");
}

#[tokio::test]
async fn tile_error_without_a_cached_copy_is_an_error() {
    let fixture = fixture().await;
    mount_tile(&fixture.server, ResponseTemplate::new(500), 1).await;
    assert!(fixture.source.tile(TILE).await.is_err());
}
