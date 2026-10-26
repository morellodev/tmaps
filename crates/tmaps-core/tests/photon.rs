use std::assert_matches;

use tmaps_core::geocode::SearchOptions;
use tmaps_core::providers::photon::Photon;
use tmaps_core::{Error, LatLon, http};
use wiremock::matchers::{any, header, method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SEARCH_FIXTURE: &str = include_str!("fixtures/photon_search.json");
const REVERSE_FIXTURE: &str = include_str!("fixtures/photon_reverse.json");

fn photon(server: &MockServer) -> Photon {
    Photon::new(http::client().unwrap(), &server.uri())
}

fn json(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "application/json")
}

mod search {
    use super::*;

    #[tokio::test]
    async fn returns_places_with_labels() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api"))
            .and(query_param("q", "duomo milano"))
            .respond_with(json(SEARCH_FIXTURE))
            .mount(&server)
            .await;

        let places = photon(&server)
            .search("duomo milano", &SearchOptions::default())
            .await
            .unwrap();

        assert_eq!(places[0].label, "Duomo, Piazza del Duomo, Milano, Italia");
    }

    #[tokio::test]
    async fn reads_geojson_coordinates_as_lon_lat() {
        let server = MockServer::start().await;
        Mock::given(path("/api"))
            .respond_with(json(SEARCH_FIXTURE))
            .mount(&server)
            .await;

        let places = photon(&server)
            .search("duomo", &SearchOptions::default())
            .await
            .unwrap();

        assert_eq!(
            places[0].location,
            LatLon::new(45.464_138_5, 9.188_554_8).unwrap()
        );
    }

    #[tokio::test]
    async fn sends_the_tmaps_user_agent() {
        let server = MockServer::start().await;
        Mock::given(header("user-agent", http::USER_AGENT))
            .respond_with(json(SEARCH_FIXTURE))
            .expect(1)
            .mount(&server)
            .await;

        photon(&server)
            .search("duomo", &SearchOptions::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn sends_location_bias() {
        let server = MockServer::start().await;
        Mock::given(query_param("lat", "45.46"))
            .and(query_param("lon", "9.19"))
            .respond_with(json(SEARCH_FIXTURE))
            .expect(1)
            .mount(&server)
            .await;
        let options = SearchOptions {
            near: Some(LatLon::new(45.46, 9.19).unwrap()),
            ..SearchOptions::default()
        };

        photon(&server).search("duomo", &options).await.unwrap();
    }

    #[tokio::test]
    async fn omits_unsupported_language() {
        let server = MockServer::start().await;
        Mock::given(query_param_is_missing("lang"))
            .respond_with(json(SEARCH_FIXTURE))
            .expect(1)
            .mount(&server)
            .await;
        let options = SearchOptions {
            lang: Some("it-IT".to_owned()),
            ..SearchOptions::default()
        };

        photon(&server).search("duomo", &options).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_blank_text_without_a_request() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let result = photon(&server)
            .search("  ", &SearchOptions::default())
            .await;

        assert_matches!(result, Err(Error::InvalidInput(_)));
    }

    #[tokio::test]
    async fn maps_http_errors_to_provider_error() {
        let server = MockServer::start().await;
        Mock::given(path("/api"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        let result = photon(&server)
            .search("duomo", &SearchOptions::default())
            .await;

        assert_matches!(result, Err(Error::Provider(_)));
    }
}

mod reverse {
    use super::*;

    #[tokio::test]
    async fn returns_the_nearest_place() {
        let server = MockServer::start().await;
        Mock::given(path("/reverse"))
            .and(query_param("lat", "45.4642"))
            .and(query_param("lon", "9.19"))
            .respond_with(json(REVERSE_FIXTURE))
            .mount(&server)
            .await;

        let place = photon(&server)
            .reverse(LatLon::new(45.4642, 9.19).unwrap(), None)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(place.category.as_deref(), Some("shop:ticket"));
    }

    #[tokio::test]
    async fn returns_none_for_empty_collection() {
        let server = MockServer::start().await;
        Mock::given(path("/reverse"))
            .respond_with(json(r#"{"type":"FeatureCollection","features":[]}"#))
            .mount(&server)
            .await;

        let place = photon(&server)
            .reverse(LatLon::new(0.0, -30.0).unwrap(), None)
            .await
            .unwrap();

        assert_eq!(place, None);
    }
}
