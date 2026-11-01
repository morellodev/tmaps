use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tmaps_core::LatLon;
use tmaps_core::style::{Layer, Layers};
use tmaps_core::view::View;

use crate::file::write_atomically;

const FILE_NAME: &str = "view.toml";
const HOME_ZOOM: f64 = 14.0;
const WORLD_ZOOM: f64 = 1.0;

#[derive(Serialize, Deserialize)]
struct SavedView {
    lat: f64,
    lon: f64,
    zoom: f64,
    /// The file keeps the hidden layers, so that a layer of a newer version shows by default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    hidden_layers: Vec<String>,
}

impl SavedView {
    fn view(&self) -> Option<View> {
        let center = LatLon::new(self.lat, self.lon).ok()?;
        self.zoom.is_finite().then(|| View::new(center, self.zoom))
    }

    fn layers(&self) -> Layers {
        Layer::ALL
            .into_iter()
            .filter(|layer| !self.hidden_layers.iter().any(|name| name == layer.name()))
            .collect()
    }
}

/// The view and the layers of the last session. Without a saved view, the view shows `home` at
/// street level, else the whole world.
pub fn initial(state_dir: &Path, home: Option<LatLon>) -> (View, Layers) {
    let saved = load(&state_dir.join(FILE_NAME));
    let view = saved
        .as_ref()
        .and_then(SavedView::view)
        .or_else(|| home.map(|home| View::new(home, HOME_ZOOM)))
        .unwrap_or_else(|| View::new(LatLon::clamped(0.0, 0.0), WORLD_ZOOM));
    let layers = saved.map_or(Layers::ALL, |saved| saved.layers());
    (view, layers)
}

pub fn save(state_dir: &Path, view: View, layers: Layers) -> io::Result<()> {
    let center = view.center();
    let saved = SavedView {
        lat: center.lat(),
        lon: center.lon(),
        zoom: view.zoom(),
        hidden_layers: Layer::ALL
            .into_iter()
            .filter(|&layer| !layers.contains(layer))
            .map(|layer| layer.name().to_owned())
            .collect(),
    };
    let text = toml::to_string(&saved).map_err(io::Error::other)?;
    write_atomically(&state_dir.join(FILE_NAME), text.as_bytes())
}

/// A missing or invalid file gives `None`: the next quit writes a valid one.
fn load(path: &Path) -> Option<SavedView> {
    toml::from_str(&fs::read_to_string(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn milan() -> LatLon {
        LatLon::new(45.4642, 9.19).unwrap()
    }

    #[test]
    fn initial_restores_the_saved_view() {
        let dir = tempfile::tempdir().unwrap();
        let view = View::new(milan(), 12.5);
        save(dir.path(), view, Layers::ALL).unwrap();
        let (restored, _) = initial(dir.path(), None);
        assert!(
            restored.center().distance_m(milan()) < 0.01 && (restored.zoom() - 12.5).abs() < 1e-9
        );
    }

    #[test]
    fn initial_starts_at_home_without_a_saved_view() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            initial(dir.path(), Some(milan())).0,
            View::new(milan(), HOME_ZOOM)
        );
    }

    #[test]
    fn initial_shows_the_world_without_a_saved_view_or_home() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(initial(dir.path(), None).0.zoom(), WORLD_ZOOM);
    }

    #[test]
    fn initial_ignores_an_invalid_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(FILE_NAME),
            "lat = 200.0\nlon = 0.0\nzoom = 3.0\n",
        )
        .unwrap();
        assert_eq!(
            initial(dir.path(), Some(milan())).0,
            View::new(milan(), HOME_ZOOM)
        );
    }

    #[test]
    fn initial_restores_the_saved_layers() {
        let dir = tempfile::tempdir().unwrap();
        let layers = Layers::ALL.toggled(Layer::Buildings);
        save(dir.path(), View::new(milan(), 12.0), layers).unwrap();
        assert_eq!(initial(dir.path(), None).1, layers);
    }

    #[test]
    fn initial_shows_all_layers_of_a_file_without_layers() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(FILE_NAME),
            "lat = 45.0\nlon = 9.0\nzoom = 3.0\n",
        )
        .unwrap();
        assert_eq!(initial(dir.path(), None).1, Layers::ALL);
    }

    #[test]
    fn initial_ignores_a_hidden_layer_that_it_does_not_know() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(FILE_NAME),
            "lat = 45.0\nlon = 9.0\nzoom = 3.0\nhidden_layers = [\"traffic\", \"nature\"]\n",
        )
        .unwrap();
        assert_eq!(
            initial(dir.path(), None).1,
            Layers::ALL.toggled(Layer::Nature)
        );
    }
}
