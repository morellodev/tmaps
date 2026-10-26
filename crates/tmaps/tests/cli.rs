use std::fs;
use std::path::PathBuf;
use std::process::Output;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;
use wiremock::matchers::{body_partial_json, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PHOTON_SEARCH: &str = include_str!("../../tmaps-core/tests/fixtures/photon_search.json");
const PHOTON_REVERSE: &str = include_str!("../../tmaps-core/tests/fixtures/photon_reverse.json");
const VALHALLA_ROUTE: &str = include_str!("../../tmaps-core/tests/fixtures/valhalla_route.json");
const VALHALLA_NO_ROUTE: &str =
    include_str!("../../tmaps-core/tests/fixtures/valhalla_error_154.json");
/// Two matches 250 km apart: the cathedrals of Milan and Florence.
const PHOTON_SEARCH_FAR_APART: &str = r#"{"type":"FeatureCollection","features":[
{"type":"Feature","properties":{"name":"Duomo di Milano","city":"Milano"},"geometry":{"type":"Point","coordinates":[9.1916,45.4642]}},
{"type":"Feature","properties":{"name":"Duomo di Firenze","city":"Firenze"},"geometry":{"type":"Point","coordinates":[11.2560,43.7731]}}]}"#;
const FROM: &str = "45.4642,9.19";
const TO: &str = "45.4848,9.2047";

struct Env {
    server: MockServer,
    _dir: TempDir,
    config: PathBuf,
}

impl Env {
    async fn new() -> Self {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        fs::write(
            &config,
            format!(
                "# saved by hand\nlang = \"en-US\"\n\n[endpoints]\nphoton = \"{uri}\"\nvalhalla = \"{uri}\"\n\n[places]\nhome = \"{FROM}\"\n",
                uri = server.uri()
            ),
        )
        .unwrap();
        Self {
            server,
            _dir: dir,
            config,
        }
    }

    async fn mount(&self, route: &str, status: u16, body: &str) {
        Mock::given(path(route))
            .respond_with(ResponseTemplate::new(status).set_body_raw(body, "application/json"))
            .mount(&self.server)
            .await;
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(assert_cmd::cargo::cargo_bin!("tmaps"))
            .args(args)
            .env("TMAPS_CONFIG", &self.config)
            .env("TZ", "UTC")
            .output()
            .unwrap()
    }
}

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

/// Returns the validation errors of `document` against the stored schema `name`.
fn schema_errors(name: &str, document: &Value) -> Vec<String> {
    let path = format!("{}/../../schemas/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let schema: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    validator
        .iter_errors(document)
        .map(|error| format!("{} at {}", error, error.instance_path()))
        .collect()
}

mod geocode {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn output_matches_the_schema() {
        let env = Env::new().await;
        env.mount("/api", 200, PHOTON_SEARCH).await;

        let output = env.run(&["geocode", "duomo"]);

        assert_eq!(schema_errors("geocode@1", &json(&output.stdout)), [""; 0]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn text_format_lists_labels() {
        let env = Env::new().await;
        env.mount("/api", 200, PHOTON_SEARCH).await;

        let output = env.run(&["geocode", "duomo", "--format", "text"]);

        assert!(
            String::from_utf8_lossy(&output.stdout)
                .starts_with("1. Duomo, Piazza del Duomo, Milano, Italia\n"),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

mod reverse {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn output_matches_the_schema() {
        let env = Env::new().await;
        env.mount("/reverse", 200, PHOTON_REVERSE).await;

        let output = env.run(&["reverse", FROM]);

        assert_eq!(schema_errors("reverse@1", &json(&output.stdout)), [""; 0]);
    }
}

mod route {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn output_matches_the_schema() {
        let env = Env::new().await;
        env.mount("/route", 200, VALHALLA_ROUTE).await;

        let output = env.run(&[
            "route",
            "--from",
            FROM,
            "--to",
            TO,
            "--alternatives",
            "2",
            "--geometry",
            "geojson",
        ]);

        assert_eq!(schema_errors("route@1", &json(&output.stdout)), [""; 0]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn arrival_time_adds_duration_to_departure() {
        let env = Env::new().await;
        env.mount("/route", 200, VALHALLA_ROUTE).await;

        let output = env.run(&[
            "route",
            "--from",
            FROM,
            "--to",
            TO,
            "--depart-at",
            "2026-10-26T14:00:00Z",
        ]);

        assert_eq!(
            json(&output.stdout)["routes"][0]["arrive_at"],
            "2026-10-26T14:15:17+00:00"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn omits_geometry_by_default() {
        let env = Env::new().await;
        env.mount("/route", 200, VALHALLA_ROUTE).await;

        let output = env.run(&["route", "--from", FROM, "--to", TO]);

        assert_eq!(json(&output.stdout)["routes"][0].get("geometry"), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn resolves_saved_place() {
        let env = Env::new().await;
        Mock::given(path("/route"))
            .and(body_partial_json(serde_json::json!({
                "locations": [{"lat": 45.4642, "lon": 9.19}, {"lat": 45.4848, "lon": 9.2047}]
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(VALHALLA_ROUTE, "application/json"),
            )
            .expect(1)
            .mount(&env.server)
            .await;

        let output = env.run(&["route", "--from", "@home", "--to", TO]);

        assert!(output.status.success(), "{output:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exits_3_when_no_route_exists() {
        let env = Env::new().await;
        env.mount("/route", 400, VALHALLA_NO_ROUTE).await;

        let output = env.run(&["route", "--from", FROM, "--to", TO]);

        assert_eq!(output.status.code(), Some(3));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn writes_no_route_error_as_json_to_stderr() {
        let env = Env::new().await;
        env.mount("/route", 400, VALHALLA_NO_ROUTE).await;

        let output = env.run(&["route", "--from", FROM, "--to", TO]);

        assert_eq!(json(&output.stderr)["error"]["kind"], "no_route");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn error_output_matches_the_schema() {
        let env = Env::new().await;
        env.mount("/route", 400, VALHALLA_NO_ROUTE).await;

        let output = env.run(&["route", "--from", FROM, "--to", TO]);

        assert_eq!(schema_errors("error@1", &json(&output.stderr)), [""; 0]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exits_4_when_the_router_fails() {
        let env = Env::new().await;
        env.mount("/route", 500, "{}").await;

        let output = env.run(&["route", "--from", FROM, "--to", TO]);

        assert_eq!(output.status.code(), Some(4));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn strict_mode_rejects_matches_far_apart() {
        let env = Env::new().await;
        env.mount("/api", 200, PHOTON_SEARCH_FAR_APART).await;

        let output = env.run(&["route", "--strict", "--from", "duomo", "--to", TO]);

        assert_eq!(json(&output.stderr)["error"]["kind"], "ambiguous");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn strict_mode_accepts_matches_close_together() {
        let env = Env::new().await;
        env.mount("/api", 200, PHOTON_SEARCH).await;
        env.mount("/route", 200, VALHALLA_ROUTE).await;

        let output = env.run(&["route", "--strict", "--from", "duomo", "--to", TO]);

        assert_eq!(output.status.code(), Some(0));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn out_of_range_coordinates_exit_2() {
        let env = Env::new().await;

        let output = env.run(&["route", "--from", "91,0", "--to", TO]);

        assert_eq!(output.status.code(), Some(2));
    }
}

mod eta {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn output_matches_the_schema() {
        let env = Env::new().await;
        env.mount("/route", 200, VALHALLA_ROUTE).await;

        let output = env.run(&["eta", "--from", FROM, "--to", TO]);

        assert_eq!(schema_errors("eta@1", &json(&output.stdout)), [""; 0]);
    }
}

mod places {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn list_output_matches_the_schema() {
        let env = Env::new().await;

        let output = env.run(&["places", "list"]);

        assert_eq!(schema_errors("places@1", &json(&output.stdout)), [""; 0]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn add_lists_the_new_place() {
        let env = Env::new().await;

        let output = env.run(&["places", "add", "work", TO]);

        assert_eq!(json(&output.stdout)["places"][1]["name"], "work");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn add_keeps_comments_in_the_config_file() {
        let env = Env::new().await;

        env.run(&["places", "add", "work", TO]);

        assert!(
            fs::read_to_string(&env.config)
                .unwrap()
                .starts_with("# saved by hand\n")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rm_removes_the_place() {
        let env = Env::new().await;

        let output = env.run(&["places", "rm", "@home"]);

        assert_eq!(json(&output.stdout)["places"], serde_json::json!([]));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rm_removes_a_place_with_an_invalid_location() {
        let env = Env::new().await;
        fs::write(&env.config, "[places]\nbad = \"91,0\"\n").unwrap();

        let output = env.run(&["places", "rm", "bad"]);

        assert_eq!(json(&output.stdout)["places"], serde_json::json!([]));
    }
}

mod errors {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn usage_error_is_json_when_stdout_is_not_a_terminal() {
        let env = Env::new().await;

        let output = env.run(&["route", "--from", FROM]);

        assert_eq!(json(&output.stderr)["error"]["kind"], "usage");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn invalid_lang_is_a_usage_error() {
        let env = Env::new().await;

        let output = env.run(&["places", "list", "--lang", "not a tag"]);

        assert_eq!(json(&output.stderr)["error"]["kind"], "usage");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn map_without_a_terminal_exits_2() {
        let env = Env::new().await;

        let output = env.run(&[]);

        assert_eq!(output.status.code(), Some(2));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn invalid_config_exits_1() {
        let env = Env::new().await;
        fs::write(&env.config, "units = 3").unwrap();

        let output = env.run(&["places", "list"]);

        assert_eq!(output.status.code(), Some(1));
    }
}
