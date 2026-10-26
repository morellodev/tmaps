use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tmaps_core::LatLon;
use tmaps_core::render::View;

const FILE_NAME: &str = "view.toml";
const HOME_ZOOM: f64 = 14.0;
const WORLD_ZOOM: f64 = 1.0;

#[derive(Serialize, Deserialize)]
struct SavedView {
    lat: f64,
    lon: f64,
    zoom: f64,
}

/// The view of the last session, else `home` at street level, else the whole world.
pub fn initial(state_dir: &Path, home: Option<LatLon>) -> View {
    load(&state_dir.join(FILE_NAME))
        .or_else(|| home.map(|home| View::new(home, HOME_ZOOM)))
        .unwrap_or_else(|| View::new(LatLon::clamped(0.0, 0.0), WORLD_ZOOM))
}

pub fn save(state_dir: &Path, view: View) -> io::Result<()> {
    let center = view.center();
    let saved = SavedView {
        lat: center.lat(),
        lon: center.lon(),
        zoom: view.zoom(),
    };
    let text = toml::to_string(&saved).map_err(io::Error::other)?;
    fs::create_dir_all(state_dir)?;
    fs::write(state_dir.join(FILE_NAME), text)
}

/// A missing or invalid file gives `None`: the next quit writes a valid one.
fn load(path: &Path) -> Option<View> {
    let saved: SavedView = toml::from_str(&fs::read_to_string(path).ok()?).ok()?;
    let center = LatLon::new(saved.lat, saved.lon).ok()?;
    saved
        .zoom
        .is_finite()
        .then(|| View::new(center, saved.zoom))
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
        save(dir.path(), view).unwrap();
        let restored = initial(dir.path(), None);
        assert!(
            restored.center().distance_m(milan()) < 0.01 && (restored.zoom() - 12.5).abs() < 1e-9
        );
    }

    #[test]
    fn initial_starts_at_home_without_a_saved_view() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            initial(dir.path(), Some(milan())),
            View::new(milan(), HOME_ZOOM)
        );
    }

    #[test]
    fn initial_shows_the_world_without_a_saved_view_or_home() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(initial(dir.path(), None).zoom(), WORLD_ZOOM);
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
            initial(dir.path(), Some(milan())),
            View::new(milan(), HOME_ZOOM)
        );
    }
}
