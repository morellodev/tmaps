use std::borrow::Cow;

use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::geocode::{Place, SearchOptions};
use crate::http::body_snippet;
use crate::{Error, LatLon, Result};

/// Photon accepts only these values for `lang`; any other value is an HTTP 400.
const SUPPORTED_LANGS: [&str; 3] = ["de", "en", "fr"];

/// Geocoder backed by a [Photon](https://github.com/komoot/photon) server.
pub struct Photon {
    client: reqwest::Client,
    base_url: String,
}

impl Photon {
    #[must_use]
    pub fn new(client: reqwest::Client, base_url: &str) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_owned(),
        }
    }

    /// # Errors
    ///
    /// - [`Error::InvalidInput`] if `text` is blank. No request is sent.
    /// - [`Error::Network`] if the request fails.
    /// - [`Error::Provider`] if Photon returns an error status or an unexpected body.
    pub async fn search(&self, text: &str, options: &SearchOptions) -> Result<Vec<Place>> {
        let text = text.trim();
        if text.is_empty() {
            return Err(Error::InvalidInput("search text is empty".to_owned()));
        }
        let mut query = vec![("q", text.to_owned()), ("limit", options.limit.to_string())];
        if let Some(near) = options.near {
            query.push(("lat", near.lat().to_string()));
            query.push(("lon", near.lon().to_string()));
        }
        query.extend(photon_lang(options.lang.as_deref()).map(|lang| ("lang", lang.to_owned())));

        let collection: FeatureCollection = self.get("/api", &query).await?;
        Ok(collection.into_places())
    }

    /// Returns the place nearest to `location`, or `None` if Photon knows no place there.
    ///
    /// # Errors
    ///
    /// - [`Error::Network`] if the request fails.
    /// - [`Error::Provider`] if Photon returns an error status or an unexpected body.
    pub async fn reverse(&self, location: LatLon, lang: Option<&str>) -> Result<Option<Place>> {
        let mut query = vec![
            ("lat", location.lat().to_string()),
            ("lon", location.lon().to_string()),
        ];
        query.extend(photon_lang(lang).map(|lang| ("lang", lang.to_owned())));

        let collection: FeatureCollection = self.get("/reverse", &query).await?;
        Ok(collection.into_places().into_iter().next())
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        let response = self
            .client
            .get(format!("{}{path}", self.base_url))
            .query(query)
            .send()
            .await?;
        let status = response.status();
        let body = response.bytes().await?;
        if !status.is_success() {
            return Err(Error::Provider(format!(
                "Photon returned HTTP {status}: {}",
                body_snippet(&body)
            )));
        }
        serde_json::from_slice(&body)
            .map_err(|error| Error::Provider(format!("unexpected Photon response: {error}")))
    }
}

fn photon_lang(tag: Option<&str>) -> Option<&'static str> {
    let primary = tag?.split(['-', '_']).next()?;
    SUPPORTED_LANGS
        .into_iter()
        .find(|lang| lang.eq_ignore_ascii_case(primary))
}

#[derive(Deserialize)]
struct FeatureCollection {
    features: Vec<Feature>,
}

impl FeatureCollection {
    fn into_places(self) -> Vec<Place> {
        self.features
            .into_iter()
            .filter_map(Feature::into_place)
            .collect()
    }
}

#[derive(Deserialize)]
struct Feature {
    geometry: Geometry,
    properties: Properties,
}

#[derive(Deserialize)]
struct Geometry {
    /// `GeoJSON` order: longitude first.
    coordinates: (f64, f64),
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Properties {
    name: Option<String>,
    housenumber: Option<String>,
    street: Option<String>,
    city: Option<String>,
    country: Option<String>,
    countrycode: Option<String>,
    osm_key: Option<String>,
    osm_value: Option<String>,
}

impl Feature {
    fn into_place(self) -> Option<Place> {
        let (lon, lat) = self.geometry.coordinates;
        let location = LatLon::new(lat, lon).ok()?;
        let properties = self.properties;
        let label = properties.label().unwrap_or_else(|| location.to_string());
        let category = properties
            .osm_key
            .zip(properties.osm_value)
            .map(|(key, value)| format!("{key}:{value}"));
        Some(Place {
            label,
            name: properties.name,
            location,
            category,
            country_code: properties.countrycode,
        })
    }
}

impl Properties {
    fn label(&self) -> Option<String> {
        let street = match (&self.street, &self.housenumber) {
            (Some(street), Some(number)) => Some(Cow::Owned(format!("{street} {number}"))),
            (street, _) => street.as_deref().map(Cow::Borrowed),
        };
        let mut parts: Vec<Cow<'_, str>> = Vec::new();
        for part in [
            self.name.as_deref().map(Cow::Borrowed),
            street,
            self.city.as_deref().map(Cow::Borrowed),
            self.country.as_deref().map(Cow::Borrowed),
        ]
        .into_iter()
        .flatten()
        {
            if !parts.contains(&part) {
                parts.push(part);
            }
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod photon_lang {
        use super::*;

        #[test]
        fn keeps_supported_primary_subtag() {
            assert_eq!(photon_lang(Some("fr-CA")), Some("fr"));
        }

        #[test]
        fn drops_unsupported_language() {
            assert_eq!(photon_lang(Some("it-IT")), None);
        }
    }

    mod label {
        use super::*;

        #[test]
        fn joins_name_street_city_and_country() {
            let properties = Properties {
                name: Some("Duomo".to_owned()),
                street: Some("Piazza del Duomo".to_owned()),
                city: Some("Milano".to_owned()),
                country: Some("Italia".to_owned()),
                ..Properties::default()
            };
            assert_eq!(
                properties.label().as_deref(),
                Some("Duomo, Piazza del Duomo, Milano, Italia")
            );
        }

        #[test]
        fn appends_house_number_to_street() {
            let properties = Properties {
                street: Some("Via Roma".to_owned()),
                housenumber: Some("3".to_owned()),
                ..Properties::default()
            };
            assert_eq!(properties.label().as_deref(), Some("Via Roma 3"));
        }

        #[test]
        fn skips_city_equal_to_name() {
            let properties = Properties {
                name: Some("Milano".to_owned()),
                city: Some("Milano".to_owned()),
                country: Some("Italia".to_owned()),
                ..Properties::default()
            };
            assert_eq!(properties.label().as_deref(), Some("Milano, Italia"));
        }

        #[test]
        fn is_none_without_address_parts() {
            assert_eq!(Properties::default().label(), None);
        }
    }
}
