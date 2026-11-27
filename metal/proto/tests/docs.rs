//! `docs/metal/proto.md` carries the API tables generated from the registry
//! between two markers; they must be current.
//!
//! Regenerate: cargo run -p victory-metal-proto --example api_table -- --write

use std::path::PathBuf;
use victory_metal_proto::registry::markdown_tables;

const BEGIN: &str = "<!-- BEGIN GENERATED: api_table -->";
const END: &str = "<!-- END GENERATED: api_table -->";

fn doc_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/metal/proto.md")
}

#[test]
fn the_api_tables_in_the_docs_are_current() {
    let doc = std::fs::read_to_string(doc_path()).expect("docs/metal/proto.md exists");
    let start = doc.find(BEGIN).expect("begin marker") + BEGIN.len();
    let end = doc.find(END).expect("end marker");
    let have = doc[start..end].trim();
    let want = markdown_tables();
    assert_eq!(
        have,
        want.trim(),
        "docs/metal/proto.md is stale: cargo run -p victory-metal-proto --example api_table -- --write"
    );
}
