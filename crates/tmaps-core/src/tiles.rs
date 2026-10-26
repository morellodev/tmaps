//! Vector tiles from a `TileJSON` endpoint, with a disk cache.

mod cache;

use std::fmt;
use std::io::Read;
use std::time::{Duration, SystemTime};

use reqwest::StatusCode;
use reqwest::header::{CACHE_CONTROL, HeaderMap};
use serde::Deserialize;
use tokio::sync::OnceCell;

pub use self::cache::{DEFAULT_CACHE_BYTES, DiskCache};
use crate::http::body_snippet;
use crate::{Error, Result};

/// Highest zoom level that the default tiles have. The renderer overzooms past it.
pub const DEFAULT_MAX_ZOOM: u8 = 14;
const DEFAULT_MAX_AGE: Duration = Duration::from_hours(24);
/// Limits a `max-age` that would overflow the expiry time.
const MAX_CACHE_AGE: Duration = Duration::from_hours(365 * 24);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileId {
    /// The tile at zoom `z` that contains the tile, or `None` if `z` is deeper.
    #[must_use]
    pub fn ancestor(self, z: u8) -> Option<Self> {
        let levels = self.z.checked_sub(z)?;
        Some(Self {
            z,
            x: self.x >> levels,
            y: self.y >> levels,
        })
    }
}

impl fmt::Display for TileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}", self.z, self.x, self.y)
    }
}

/// The parts of a `TileJSON` document that tmaps uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileSet {
    template: String,
    pub min_zoom: u8,
    pub max_zoom: u8,
    /// Credit line in plain text. `TileJSON` gives it as HTML.
    pub attribution: String,
}

#[derive(Deserialize)]
struct TileJson {
    tiles: Vec<String>,
    minzoom: Option<u8>,
    maxzoom: Option<u8>,
    attribution: Option<String>,
}

impl TileSet {
    fn from_json(body: &[u8]) -> Result<Self> {
        let json: TileJson = serde_json::from_slice(body)
            .map_err(|error| Error::Provider(format!("unexpected TileJSON: {error}")))?;
        let template = json
            .tiles
            .into_iter()
            .next()
            .ok_or_else(|| Error::Provider("the TileJSON has no tile URL".to_owned()))?;
        Ok(Self {
            template,
            min_zoom: json.minzoom.unwrap_or(0),
            max_zoom: json.maxzoom.unwrap_or(DEFAULT_MAX_ZOOM),
            attribution: json
                .attribution
                .as_deref()
                .map(html_to_text)
                .unwrap_or_default(),
        })
    }

    #[must_use]
    pub fn url(&self, id: TileId) -> String {
        self.template
            .replace("{z}", &id.z.to_string())
            .replace("{x}", &id.x.to_string())
            .replace("{y}", &id.y.to_string())
    }
}

/// Fetches tiles through the disk cache. When the network fails, an expired tile in the cache
/// is better than no tile, so the source returns it.
pub struct TileSource {
    client: reqwest::Client,
    tilejson_url: String,
    cache: DiskCache,
    tile_set: OnceCell<TileSet>,
}

impl TileSource {
    #[must_use]
    pub fn new(client: reqwest::Client, tilejson_url: &str, cache: DiskCache) -> Self {
        Self {
            client,
            tilejson_url: tilejson_url.to_owned(),
            cache,
            tile_set: OnceCell::new(),
        }
    }

    /// # Errors
    ///
    /// [`Error::Network`] or [`Error::Provider`] if the `TileJSON` cannot be loaded.
    pub async fn tile_set(&self) -> Result<&TileSet> {
        self.tile_set
            .get_or_try_init(|| async {
                let body = self.get_cached(&self.tilejson_url).await?;
                TileSet::from_json(&body.unwrap_or_default())
            })
            .await
    }

    /// Returns the decompressed tile, or an empty body when the server has no tile there.
    ///
    /// # Errors
    ///
    /// [`Error::Network`] or [`Error::Provider`] if the tile cannot be loaded.
    pub async fn tile(&self, id: TileId) -> Result<Vec<u8>> {
        let url = self.tile_set().await?.url(id);
        let body = self.get_cached(&url).await?.unwrap_or_default();
        gunzip_if_needed(body)
    }

    #[must_use]
    pub fn cache(&self) -> &DiskCache {
        &self.cache
    }

    /// `None` means that the server has no content at `url` (HTTP 204 or 404).
    async fn get_cached(&self, url: &str) -> Result<Option<Vec<u8>>> {
        let cached = self.cache.get(url).await;
        if let Some(entry) = &cached
            && entry.is_fresh()
        {
            return Ok(Some(entry.body.clone()));
        }
        match self.get(url).await {
            Ok(Some((body, max_age))) => {
                if let Some(max_age) = max_age {
                    self.cache
                        .put(url, &body, SystemTime::now() + max_age)
                        .await;
                }
                Ok(Some(body))
            }
            Ok(None) => Ok(None),
            Err(error) => cached.map(|entry| Some(entry.body)).ok_or(error),
        }
    }

    /// Returns the body and how long the cache can keep it (`None`: do not store it).
    async fn get(&self, url: &str) -> Result<Option<(Vec<u8>, Option<Duration>)>> {
        let response = self.client.get(url).send().await?;
        let status = response.status();
        if status == StatusCode::NO_CONTENT || status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let max_age = cache_lifetime(response.headers());
        let body = response.bytes().await?;
        if !status.is_success() {
            return Err(Error::Provider(format!(
                "the tile server returned HTTP {status}: {}",
                body_snippet(&body)
            )));
        }
        Ok(Some((body.to_vec(), max_age)))
    }
}

/// Reads `Cache-Control`. A response without it stays in the cache for one day.
fn cache_lifetime(headers: &HeaderMap) -> Option<Duration> {
    let mut max_age = DEFAULT_MAX_AGE;
    for directive in headers
        .get_all(CACHE_CONTROL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
    {
        if directive.eq_ignore_ascii_case("no-store") || directive.eq_ignore_ascii_case("no-cache")
        {
            return None;
        }
        if let Some(seconds) = directive.strip_prefix("max-age=")
            && let Ok(seconds) = seconds.parse()
        {
            max_age = Duration::from_secs(seconds).min(MAX_CACHE_AGE);
        }
    }
    Some(max_age)
}

/// Some servers send gzipped tiles without `Content-Encoding`, so reqwest does not decode them.
fn gunzip_if_needed(body: Vec<u8>) -> Result<Vec<u8>> {
    const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
    if !body.starts_with(&GZIP_MAGIC) {
        return Ok(body);
    }
    let mut decoded = Vec::new();
    flate2::read::MultiGzDecoder::new(body.as_slice())
        .read_to_end(&mut decoded)
        .map_err(|error| Error::Provider(format!("invalid gzip tile: {error}")))?;
    Ok(decoded)
}

/// Removes tags and decodes the entities that attribution lines use.
fn html_to_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    let text = text
        .replace("&copy;", "©")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use reqwest::header::HeaderValue;

    use super::*;

    #[test]
    fn ancestor_divides_coordinates() {
        let tile = TileId {
            z: 14,
            x: 8610,
            y: 5863,
        };
        assert_eq!(
            tile.ancestor(12),
            Some(TileId {
                z: 12,
                x: 2152,
                y: 1465
            })
        );
    }

    #[test]
    fn tile_set_reads_openfreemap_tilejson() {
        let body = br#"{"tiles":["https://t.example/planet/20261018/{z}/{x}/{y}.pbf"],"maxzoom":14,
            "attribution":"<a href=\"https://openfreemap.org\">OpenFreeMap</a> <a href=\"x\">&copy; OpenMapTiles</a>"}"#;
        let tile_set = TileSet::from_json(body).unwrap();
        assert_eq!(
            (
                tile_set.url(TileId { z: 3, x: 4, y: 2 }),
                tile_set.attribution.as_str()
            ),
            (
                "https://t.example/planet/20261018/3/4/2.pbf".to_owned(),
                "OpenFreeMap © OpenMapTiles"
            )
        );
    }

    mod cache_lifetime {
        use super::*;

        fn headers(values: &[&'static str]) -> HeaderMap {
            let mut headers = HeaderMap::new();
            for value in values {
                headers.append(CACHE_CONTROL, HeaderValue::from_static(value));
            }
            headers
        }

        #[test]
        fn reads_max_age_across_headers() {
            assert_eq!(
                cache_lifetime(&headers(&["max-age=3600", "public"])),
                Some(Duration::from_secs(3600))
            );
        }

        #[test]
        fn rejects_no_store() {
            assert_eq!(cache_lifetime(&headers(&["public, no-store"])), None);
        }

        #[test]
        fn defaults_to_one_day() {
            assert_eq!(cache_lifetime(&HeaderMap::new()), Some(DEFAULT_MAX_AGE));
        }
    }

    #[test]
    fn gunzip_if_needed_decodes_gzip() {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(b"tile").unwrap();
        assert_eq!(
            gunzip_if_needed(encoder.finish().unwrap()).unwrap(),
            b"tile"
        );
    }
}
