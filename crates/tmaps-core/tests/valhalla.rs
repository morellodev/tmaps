use std::assert_matches;

use tmaps_core::providers::valhalla::Valhalla;
use tmaps_core::routing::{Maneuver, Mode, Route, RouteRequest};
use tmaps_core::{Error, LatLon, http};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ROUTE_FIXTURE: &str = include_str!("fixtures/valhalla_route.json");
const DISTANCE_LIMIT_FIXTURE: &str = include_str!("fixtures/valhalla_error_154.json");

fn valhalla(server: &MockServer) -> Valhalla {
    Valhalla::new(http::client().unwrap(), &server.uri())
}

fn milan_request(mode: Mode) -> RouteRequest {
    RouteRequest {
        from: LatLon::new(45.4642, 9.19).unwrap(),
        to: LatLon::new(45.4848, 9.2047).unwrap(),
        mode,
        alternatives: 2,
        lang: Some("en-US".to_owned()),
    }
}

async fn fixture_routes() -> Vec<Route> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/route"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(ROUTE_FIXTURE, "application/json"))
        .mount(&server)
        .await;
    valhalla(&server)
        .route(&milan_request(Mode::Car))
        .await
        .unwrap()
}

mod route {
    use super::*;

    #[tokio::test]
    async fn returns_primary_route_and_alternatives() {
        assert_eq!(fixture_routes().await.len(), 3);
    }

    #[tokio::test]
    async fn converts_summary_kilometers_to_meters() {
        assert_eq!(fixture_routes().await[0].distance_m, 4232.0);
    }

    #[tokio::test]
    async fn first_step_is_a_departure() {
        assert_eq!(
            fixture_routes().await[0].steps[0].maneuver,
            Maneuver::Depart
        );
    }

    #[tokio::test]
    async fn first_step_starts_at_the_start_of_the_geometry() {
        let routes = fixture_routes().await;
        assert_eq!(routes[0].steps[0].location, routes[0].geometry[0]);
    }

    #[tokio::test]
    async fn last_step_elapsed_time_matches_route_duration() {
        let routes = fixture_routes().await;
        let route = &routes[0];
        let last = route.steps.last().unwrap();

        assert!(
            (last.elapsed_s - route.duration_s).abs() < 1.0,
            "elapsed {} vs duration {}",
            last.elapsed_s,
            route.duration_s
        );
    }

    #[tokio::test]
    async fn sends_costing_for_the_mode() {
        let server = MockServer::start().await;
        Mock::given(body_partial_json(
            serde_json::json!({"costing": "pedestrian", "alternates": 2, "language": "en-US"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_raw(ROUTE_FIXTURE, "application/json"))
        .expect(1)
        .mount(&server)
        .await;

        valhalla(&server)
            .route(&milan_request(Mode::Foot))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn caps_alternatives_at_two() {
        let server = MockServer::start().await;
        Mock::given(body_partial_json(serde_json::json!({"alternates": 2})))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(ROUTE_FIXTURE, "application/json"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let request = RouteRequest {
            alternatives: 200,
            ..milan_request(Mode::Car)
        };

        valhalla(&server).route(&request).await.unwrap();
    }

    #[tokio::test]
    async fn maps_distance_limit_error_to_no_route() {
        let server = MockServer::start().await;
        Mock::given(path("/route"))
            .respond_with(
                ResponseTemplate::new(400).set_body_raw(DISTANCE_LIMIT_FIXTURE, "application/json"),
            )
            .mount(&server)
            .await;

        let result = valhalla(&server).route(&milan_request(Mode::Car)).await;

        assert_matches!(result, Err(Error::NoRoute(_)));
    }
}
