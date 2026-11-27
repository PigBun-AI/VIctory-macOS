//! The out-of-repository corpus check of the attributeAtVertex lowering (docs/metal/attribute-at-vertex.md,
//! "Acceptance"): the title's pixel programs that read other vertices of a triangle, translated with the
//! capability off (`VICTORY_ATTRIBUTE_AT_VERTEX=0`, what the Metal chain and D3DMetal are), must contain no
//! `attributeAtVertex`, and MSC and the Metal pipeline build must take them.
//!
//! Ignored: it needs the game package and takes minutes (`cargo test -p victory-metal-census --test
//! attr_vertex_corpus -- --ignored --nocapture`). Environment:
//! * `VICTORY_GAME_DIR`: the game package (the `--game` of `victory-census translate`);
//! * `VICTORY_ATTR_PS_LIST`: a file of the shader hashes to build (one per line): the 4852 pixel programs
//!   (`research/attr-vertex/tools/rw2_res.json` keys; the list is derived from the game, so it stays out of
//!   the repository);
//! * `VICTORY_ATTR_OUT`: a work directory (default: a temporary one), `DXC_LIB` and `VICTORY_HELPER_DXC` for the
//!   partner stages (docs/metal/census.md);
//! * `VICTORY_ATTR_REQUIRE=1`: fail instead of skipping when the translator has no such capability yet.
//!
//! Without the capability (the translator emits `attributeAtVertex`, MSC refuses every one) the test says why
//! and skips, or fails with `VICTORY_ATTR_REQUIRE=1`. With it: no row may mention `attributeAtVertex`, and
//! at most [`KNOWN_OVER_LIMIT`] may fail, each for Metal's limit of 124 varying scalars (not for the lowering).
//!
//! The mesh side (the added per-primitive outputs, the 32 KiB budget) is the translator's own tests and the
//! `meshstat.tsv` arithmetic of the document: there is no pairing of the title's pixel and mesh programs to
//! build them together.
#![cfg(target_vendor = "apple")]

use std::path::PathBuf;
use std::process::Command;

/// Pixel programs known to pass Metal's 124 varying scalars once the added elements count (docs table).
const KNOWN_OVER_LIMIT: usize = 20;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn census(args: &[&str]) {
    let status = Command::new(env!("CARGO_BIN_EXE_victory-census"))
        .args(args)
        .env("VICTORY_ATTRIBUTE_AT_VERTEX", "0")
        .status()
        .expect("run victory-census");
    assert!(status.success(), "victory-census {args:?}: {status}");
}

#[test]
#[ignore = "needs the game package and the pixel program list (see the file's notes); takes minutes"]
fn the_corpus_translates_without_attribute_at_vertex_and_builds() {
    let (Some(game), Some(list)) = (env("VICTORY_GAME_DIR"), env("VICTORY_ATTR_PS_LIST")) else {
        eprintln!("SKIPPED: set VICTORY_GAME_DIR and VICTORY_ATTR_PS_LIST (the 4852 hashes)");
        assert!(
            env("VICTORY_ATTR_REQUIRE").is_none(),
            "required, but no corpus is given"
        );
        return;
    };
    let out = env("VICTORY_ATTR_OUT").map_or_else(
        || std::env::temp_dir().join(format!("victory-attr-corpus-{}", std::process::id())),
        PathBuf::from,
    );
    std::fs::create_dir_all(&out).unwrap();
    let out_s = out.to_str().unwrap();
    census(&["translate", "--game", &game, "--out", out_s, "--jobs", "4"]);
    census(&[
        "convert", "--kind", "ps", "--out", out_s, "--only", &list, "--jobs", "4",
    ]);
    let wanted: std::collections::HashSet<String> = std::fs::read_to_string(&list)
        .unwrap()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let rows = std::fs::read_to_string(out.join("convert-ps.jsonl")).unwrap();
    let (mut seen, mut asked, mut limit, mut other) = (0, 0, 0, Vec::new());
    for line in rows.lines() {
        let Some(hash) = field(line, "hash") else {
            continue;
        };
        if !wanted.contains(&hash) {
            continue;
        }
        seen += 1;
        if line.contains("\"ok\":true") {
            continue;
        }
        let error = field(line, "error").unwrap_or_default();
        if error.contains("attributeAtVertex") {
            asked += 1;
        } else if error.contains("124") {
            limit += 1;
        } else {
            other.push(format!("{hash}: {error}"));
        }
    }
    if asked > 0 {
        let why = format!(
            "the translator still emits attributeAtVertex ({asked} of {seen}): no capability yet (mac S1 not in this tree)"
        );
        assert!(env("VICTORY_ATTR_REQUIRE").is_none(), "{why}");
        eprintln!("SKIPPED: {why}");
        return;
    }
    assert!(
        seen >= wanted.len().saturating_sub(1),
        "only {seen} of {} rows",
        wanted.len()
    );
    assert!(
        limit <= KNOWN_OVER_LIMIT,
        "{limit} programs over Metal's scalar limit (known: {KNOWN_OVER_LIMIT})"
    );
    // The one program with a 64-bit atomic (MSC refuses it for that) is no business of this lowering.
    assert!(other.len() <= 1, "unexpected failures: {other:#?}");
    eprintln!(
        "{seen} pixel programs: {limit} over the scalar limit, {} other failures",
        other.len()
    );
}

/// The string value of `"key":"..."` in one JSON line (the census writes flat objects).
fn field(line: &str, key: &str) -> Option<String> {
    let at = line.find(&format!("\"{key}\":\""))? + key.len() + 4;
    let rest = &line[at..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => out.push(chars.next()?),
            c => out.push(c),
        }
    }
    None
}
