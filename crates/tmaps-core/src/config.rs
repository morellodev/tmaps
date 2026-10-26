use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use etcetera::BaseStrategy;
use serde::{Deserialize, Serialize};

use crate::{Error, LatLon, Result};

const APP_NAME: &str = "tmaps";
const DEFAULT_LANG: &str = "en-US";
const IMPERIAL_REGIONS: [&str; 4] = ["US", "GB", "LR", "MM"];

pub const DEFAULT_TILEJSON_URL: &str = "https://tiles.openfreemap.org/planet";
pub const DEFAULT_VALHALLA_URL: &str = "https://valhalla1.openstreetmap.de";
pub const DEFAULT_PHOTON_URL: &str = "https://photon.komoot.io";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Units {
    Metric,
    Imperial,
}

impl FromStr for Units {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "metric" => Ok(Self::Metric),
            "imperial" => Ok(Self::Imperial),
            _ => Err(Error::InvalidInput(format!(
                "unknown units {s:?}, expected metric or imperial"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub tilejson: String,
    pub valhalla: String,
    pub photon: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub units: Units,
    /// BCP 47 language tag, for example `it-IT`.
    pub lang: String,
    pub endpoints: Endpoints,
    pub places: BTreeMap<String, LatLon>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigFile {
    units: Option<Units>,
    lang: Option<String>,
    endpoints: EndpointsFile,
    places: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct EndpointsFile {
    tilejson: Option<String>,
    valhalla: Option<String>,
    photon: Option<String>,
}

impl Config {
    /// Reads the config file at `path`. A missing file gives the defaults.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] if the file cannot be read or its content is invalid.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
            Err(error) => return Err(config_error(path, error.to_string())),
        };
        let locale = sys_locale::get_locales().find(|locale| normalize_locale(locale).is_some());
        Self::parse(&text, locale.as_deref()).map_err(|message| config_error(path, message))
    }

    fn parse(text: &str, system_locale: Option<&str>) -> Result<Self, String> {
        let file: ConfigFile = toml::from_str(text).map_err(|error| error.to_string())?;
        let known_lang = file
            .lang
            .as_deref()
            .and_then(normalize_locale)
            .or_else(|| system_locale.and_then(normalize_locale));
        let units = file
            .units
            .or_else(|| known_lang.as_deref().map(default_units))
            .unwrap_or(Units::Metric);
        let lang = known_lang.unwrap_or_else(|| DEFAULT_LANG.to_owned());
        let endpoints = Endpoints {
            tilejson: endpoint("tilejson", file.endpoints.tilejson, DEFAULT_TILEJSON_URL)?,
            valhalla: endpoint("valhalla", file.endpoints.valhalla, DEFAULT_VALHALLA_URL)?,
            photon: endpoint("photon", file.endpoints.photon, DEFAULT_PHOTON_URL)?,
        };
        let places = file
            .places
            .into_iter()
            .map(|(name, value)| match value.parse() {
                Ok(point) => Ok((name, point)),
                Err(error) => Err(format!("places.{name}: {error}")),
            })
            .collect::<Result<_, _>>()?;

        Ok(Self {
            units,
            lang,
            endpoints,
            places,
        })
    }
}

fn config_error(path: &Path, message: String) -> Error {
    Error::Config {
        path: path.to_owned(),
        message,
    }
}

/// Turns POSIX locales such as `en_US.UTF-8` into BCP 47 tags with canonical case, such as `en-US`.
#[must_use]
pub fn normalize_locale(locale: &str) -> Option<String> {
    let tag = locale.split(['.', '@']).next()?;
    if tag.is_empty() || tag == "C" || tag == "POSIX" {
        return None;
    }
    let subtags: Vec<String> = tag
        .split(['-', '_'])
        .enumerate()
        .map(|(index, subtag)| match (index, subtag.len()) {
            (0, _) => subtag.to_ascii_lowercase(),
            (_, 2) => subtag.to_ascii_uppercase(),
            (_, 4) => {
                let (first, rest) = subtag.split_at(1);
                first.to_ascii_uppercase() + &rest.to_ascii_lowercase()
            }
            _ => subtag.to_owned(),
        })
        .collect();
    Some(subtags.join("-"))
}

fn default_units(lang: &str) -> Units {
    let region = lang.rsplit('-').next().unwrap_or_default();
    if IMPERIAL_REGIONS.contains(&region) {
        Units::Imperial
    } else {
        Units::Metric
    }
}

fn endpoint(key: &str, value: Option<String>, default: &str) -> Result<String, String> {
    let Some(value) = value else {
        return Ok(default.to_owned());
    };
    let url = reqwest::Url::parse(&value).map_err(|error| format!("endpoints.{key}: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("endpoints.{key}: expected an http or https URL"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(format!("endpoints.{key}: remove the query and fragment"));
    }
    Ok(value.trim_end_matches('/').to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl Paths {
    /// XDG directories on Linux and macOS, Known Folders on Windows.
    ///
    /// # Errors
    ///
    /// [`Error::NoHomeDir`] if the home directory is unknown.
    pub fn detect() -> Result<Self> {
        let strategy = etcetera::choose_base_strategy().map_err(|_| Error::NoHomeDir)?;
        Ok(Self {
            config_dir: strategy.config_dir().join(APP_NAME),
            state_dir: strategy
                .state_dir()
                .unwrap_or_else(|| strategy.data_dir())
                .join(APP_NAME),
            cache_dir: strategy.cache_dir().join(APP_NAME),
        })
    }

    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod parse {
        use super::*;

        #[test]
        fn takes_lang_from_system_locale_when_file_is_empty() {
            let config = Config::parse("", Some("it-IT")).unwrap();
            assert_eq!(config.lang, "it-IT");
        }

        #[test]
        fn normalizes_posix_locale_to_bcp47() {
            let config = Config::parse("", Some("en_US.UTF-8")).unwrap();
            assert_eq!(config.lang, "en-US");
        }

        #[test]
        fn falls_back_to_en_us_for_c_locale() {
            let config = Config::parse("", Some("C")).unwrap();
            assert_eq!(config.lang, DEFAULT_LANG);
        }

        #[test]
        fn picks_imperial_units_for_us_region() {
            let config = Config::parse("", Some("en-US")).unwrap();
            assert_eq!(config.units, Units::Imperial);
        }

        #[test]
        fn picks_metric_units_for_other_regions() {
            let config = Config::parse("", Some("it-IT")).unwrap();
            assert_eq!(config.units, Units::Metric);
        }

        #[test]
        fn file_units_override_locale_units() {
            let config = Config::parse("units = \"metric\"", Some("en-US")).unwrap();
            assert_eq!(config.units, Units::Metric);
        }

        #[test]
        fn normalizes_case_and_separator_of_file_lang() {
            let config = Config::parse("lang = \"en_gb\"", None).unwrap();
            assert_eq!(config.lang, "en-GB");
        }

        #[test]
        fn picks_imperial_units_for_lowercase_file_region() {
            let config = Config::parse("lang = \"en-us\"", None).unwrap();
            assert_eq!(config.units, Units::Imperial);
        }

        #[test]
        fn keeps_script_subtag_in_title_case() {
            let config = Config::parse("", Some("zh_hans_CN")).unwrap();
            assert_eq!(config.lang, "zh-Hans-CN");
        }

        #[test]
        fn falls_back_to_en_us_for_c_utf8_locale() {
            let config = Config::parse("", Some("C.UTF-8")).unwrap();
            assert_eq!(config.lang, DEFAULT_LANG);
        }

        #[test]
        fn falls_back_to_metric_units_for_c_locale() {
            let config = Config::parse("", Some("C.UTF-8")).unwrap();
            assert_eq!(config.units, Units::Metric);
        }

        #[test]
        fn rejects_endpoint_with_non_http_scheme() {
            let error = Config::parse("[endpoints]\nphoton = \"file:///tmp\"", None).unwrap_err();
            assert!(error.starts_with("endpoints.photon:"), "{error}");
        }

        #[test]
        fn rejects_endpoint_with_query() {
            let error =
                Config::parse("[endpoints]\nphoton = \"https://x.test/?k=v\"", None).unwrap_err();
            assert!(error.starts_with("endpoints.photon:"), "{error}");
        }

        #[test]
        fn file_lang_overrides_system_locale() {
            let config = Config::parse("lang = \"de-DE\"", Some("en-US")).unwrap();
            assert_eq!(config.lang, "de-DE");
        }

        #[test]
        fn uses_default_endpoints_when_file_is_empty() {
            let config = Config::parse("", None).unwrap();
            assert_eq!(config.endpoints.photon, DEFAULT_PHOTON_URL);
        }

        #[test]
        fn trims_trailing_slash_from_endpoint() {
            let config =
                Config::parse("[endpoints]\nvalhalla = \"http://localhost:8002/\"", None).unwrap();
            assert_eq!(config.endpoints.valhalla, "http://localhost:8002");
        }

        #[test]
        fn rejects_endpoint_that_is_not_a_url() {
            let error = Config::parse("[endpoints]\nphoton = \"not a url\"", None).unwrap_err();
            assert!(error.starts_with("endpoints.photon:"), "{error}");
        }

        #[test]
        fn parses_place_coordinates() {
            let config = Config::parse("[places]\nhome = \"45.46, 9.19\"", None).unwrap();
            assert_eq!(config.places["home"], LatLon::new(45.46, 9.19).unwrap());
        }

        #[test]
        fn names_the_key_of_an_invalid_place() {
            let error = Config::parse("[places]\nwork = \"nowhere\"", None).unwrap_err();
            assert!(error.starts_with("places.work:"), "{error}");
        }

        #[test]
        fn rejects_unknown_keys() {
            assert!(Config::parse("colour = \"red\"", None).is_err());
        }
    }

    mod load {
        use super::*;

        #[test]
        fn returns_defaults_when_file_is_missing() {
            let dir = tempfile::tempdir().unwrap();
            let config = Config::load(&dir.path().join("config.toml")).unwrap();
            assert_eq!(config.endpoints.valhalla, DEFAULT_VALHALLA_URL);
        }

        #[test]
        fn reports_the_path_of_an_invalid_file() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            fs::write(&path, "units = 3").unwrap();
            let error = Config::load(&path).unwrap_err();
            assert!(error.to_string().contains("config.toml"), "{error}");
        }
    }
}
