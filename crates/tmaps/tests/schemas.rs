use std::path::PathBuf;
use std::{env, fs};

fn schemas_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas")
}

#[test]
fn stored_schemas_match_the_output_types() {
    let update = env::var_os("TMAPS_UPDATE_SCHEMAS").is_some();
    let mut stale = Vec::new();
    for (name, schema) in tmaps::schemas() {
        let path = schemas_dir().join(format!("{name}.json"));
        let expected = serde_json::to_string_pretty(&schema).unwrap() + "\n";
        if update {
            fs::create_dir_all(schemas_dir()).unwrap();
            fs::write(&path, expected).unwrap();
        } else if fs::read_to_string(&path).ok().as_deref() != Some(expected.as_str()) {
            stale.push(name);
        }
    }
    assert!(
        stale.is_empty(),
        "stale schemas {stale:?}: run `TMAPS_UPDATE_SCHEMAS=1 cargo test -p tmaps --test schemas`"
    );
}
