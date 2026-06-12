//! Guards the committed JSON Schema in `specs/schema/` against the Rust types.
//!
//! The Rust types in `lineage-core` are the canonical contract source; the JSON
//! Schema files are generated artifacts that downstream bindings (TS/zod) are
//! built from. This test fails when the types and the committed schema drift.
//! After an intentional type change, regenerate with:
//!
//! ```sh
//! LINEAGE_UPDATE_SCHEMAS=1 cargo test -p lineage-core --test schema_snapshot
//! ```

use std::path::PathBuf;

use schemars::schema_for;

use lineage_core::LineObject;

fn schema_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../specs/schema")
}

fn check_snapshot(name: &str, schema: schemars::Schema) {
    let path = schema_dir().join(name);
    // Trailing newline so the artifact is friendly to editors/diff tooling.
    let generated = format!(
        "{}\n",
        serde_json::to_string_pretty(schema.as_value()).expect("schema serializes")
    );

    if std::env::var_os("LINEAGE_UPDATE_SCHEMAS").is_some() {
        std::fs::create_dir_all(schema_dir()).expect("create schema dir");
        std::fs::write(&path, &generated).expect("write schema snapshot");
        return;
    }

    let committed = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing committed schema {}; regenerate with LINEAGE_UPDATE_SCHEMAS=1",
            path.display()
        )
    });
    assert_eq!(
        committed, generated,
        "{name} drifted from the lineage-core types; regenerate with \
         LINEAGE_UPDATE_SCHEMAS=1 cargo test -p lineage-core --test schema_snapshot"
    );
}

#[test]
fn line_object_schema_matches_types() {
    check_snapshot("line-object-v0.schema.json", schema_for!(LineObject));
}
