//! Edits `[places]` in the config file. Other keys, comments, and formatting stay unchanged.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use crate::file::write_atomically;
use crate::{Error, LatLon, Result};
use toml_edit::{DocumentMut, TableLike};

/// Saves `location` as `name`, and replaces a place with the same name.
///
/// # Errors
///
/// - [`Error::InvalidInput`] if [`validate_name`] rejects `name`.
/// - [`Error::Config`] if the config file cannot be read, parsed, or written.
pub fn add(path: &Path, name: &str, location: LatLon) -> Result<()> {
    let name = validate_name(name)?;
    edit(path, |places| {
        places.insert(name, toml_edit::value(location.to_string()));
        Ok(())
    })
}

/// # Errors
///
/// - [`Error::InvalidInput`] if no place has the name.
/// - [`Error::Config`] if the config file cannot be read, parsed, or written.
pub fn remove(path: &Path, name: &str) -> Result<()> {
    let name = name.strip_prefix('@').unwrap_or(name);
    edit(path, |places| {
        places
            .remove(name)
            .map(drop)
            .ok_or_else(|| Error::InvalidInput(format!("no saved place named @{name}")))
    })
}

/// Accepts names that work as `@name` in a shell without quotes, and returns the name without
/// the `@`.
///
/// # Errors
///
/// [`Error::InvalidInput`] for other names.
pub fn validate_name(name: &str) -> Result<&str> {
    let name = name.strip_prefix('@').unwrap_or(name);
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(name)
    } else {
        Err(Error::InvalidInput(format!(
            "invalid place name {name:?}: use letters, digits, '-', and '_'"
        )))
    }
}

fn edit(path: &Path, change: impl FnOnce(&mut dyn TableLike) -> Result<()>) -> Result<()> {
    let config_error = |message: String| Error::Config {
        path: path.to_owned(),
        message,
    };
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
        Err(error) => return Err(config_error(error.to_string())),
    };
    let mut document: DocumentMut = text
        .parse()
        .map_err(|error: toml_edit::TomlError| config_error(error.to_string()))?;
    let places = document
        .entry("places")
        .or_insert_with(toml_edit::table)
        .as_table_like_mut()
        .ok_or_else(|| config_error("places: expected a table".to_owned()))?;
    change(places)?;
    write_atomically(path, document.to_string().as_bytes())
        .map_err(|error| config_error(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use tempfile::TempDir;

    use super::*;

    fn config_with(text: &str) -> (TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, text).unwrap();
        (dir, path)
    }

    fn milan() -> LatLon {
        LatLon::new(45.46, 9.19).unwrap()
    }

    mod add {
        use super::*;

        #[test]
        fn keeps_comments_and_other_keys() {
            let (_dir, path) = config_with("# my settings\nunits = \"metric\"\n");
            add(&path, "home", milan()).unwrap();
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                "# my settings\nunits = \"metric\"\n\n[places]\nhome = \"45.46,9.19\"\n"
            );
        }

        #[test]
        fn creates_missing_file_and_directories() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("tmaps/config.toml");
            add(&path, "home", milan()).unwrap();
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                "[places]\nhome = \"45.46,9.19\"\n"
            );
        }

        #[test]
        fn replaces_place_with_same_name() {
            let (_dir, path) = config_with("[places]\nhome = \"1,2\"\n");
            add(&path, "@home", milan()).unwrap();
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                "[places]\nhome = \"45.46,9.19\"\n"
            );
        }

        #[test]
        fn rejects_name_with_space() {
            let (_dir, path) = config_with("");
            assert_matches!(add(&path, "my home", milan()), Err(Error::InvalidInput(_)));
        }

        #[test]
        fn rejects_places_that_is_not_a_table() {
            let (_dir, path) = config_with("places = 3\n");
            assert_matches!(add(&path, "home", milan()), Err(Error::Config { .. }));
        }
    }

    mod remove {
        use super::*;

        #[test]
        fn removes_only_the_named_place() {
            let (_dir, path) = config_with("[places]\nhome = \"1,2\"\nwork = \"3,4\"\n");
            remove(&path, "home").unwrap();
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                "[places]\nwork = \"3,4\"\n"
            );
        }

        #[test]
        fn rejects_unknown_name() {
            let (_dir, path) = config_with("[places]\n");
            assert_matches!(remove(&path, "home"), Err(Error::InvalidInput(_)));
        }
    }
}
