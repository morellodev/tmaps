//! Tile coordinates and `TileJSON` documents.

use std::fmt;

use serde::Deserialize;

use crate::{Error, Result};

/// Highest zoom level that the default tiles have. The renderer overzooms past it.
pub const DEFAULT_MAX_ZOOM: u8 = 14;

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
    /// # Errors
    ///
    /// [`Error::InvalidData`] if `body` is not a `TileJSON` document with a tile URL.
    pub fn from_json(body: &[u8]) -> Result<Self> {
        let json: TileJson = serde_json::from_slice(body)
            .map_err(|error| Error::InvalidData(format!("unexpected TileJSON: {error}")))?;
        let template = json
            .tiles
            .into_iter()
            .next()
            .ok_or_else(|| Error::InvalidData("the TileJSON has no tile URL".to_owned()))?;
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
}
