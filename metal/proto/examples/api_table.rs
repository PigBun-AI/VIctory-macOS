//! Prints the API tables of `docs/metal/proto.md`; with `--write` rewrites the
//! generated block of that file in place.
//!
//!   cargo run -p victory-metal-proto --example api_table [-- --write]

use std::path::PathBuf;
use victory_metal_proto::registry::{layout_fingerprint, markdown_tables};

const BEGIN: &str = "<!-- BEGIN GENERATED: api_table -->";
const END: &str = "<!-- END GENERATED: api_table -->";

fn main() {
    let tables = markdown_tables();
    if !std::env::args().any(|a| a == "--write") {
        print!("{tables}");
        eprintln!("layout fingerprint: {:#018x}", layout_fingerprint());
        return;
    }
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/metal/proto.md");
    let doc = std::fs::read_to_string(&path).expect("docs/metal/proto.md");
    let start = doc.find(BEGIN).expect("begin marker") + BEGIN.len();
    let end = doc.find(END).expect("end marker");
    let out = format!("{}\n\n{}\n{}", &doc[..start], tables.trim(), &doc[end..]);
    std::fs::write(&path, out).expect("write");
    eprintln!("updated {}", path.display());
}
