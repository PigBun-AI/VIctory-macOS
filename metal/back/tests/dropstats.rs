//! `VICTORY_METAL_DROPSTATS` through the interpreter: the ops of a block are
//! counted as submitted, a failing one and everything the failure takes with
//! it as dropped, with the reason, and a frame's row comes out at its end.
//! One test only: the collector is process-wide and reads the variable once.
#![cfg(target_vendor = "apple")]

mod exec_support;

use exec_support::*;
use victory_metal_proto::cmd::Command;
use victory_metal_proto::dropstats::{HEADER, split_pairs};
use victory_metal_proto::render::{Draw, DrawIndexed};
use victory_metal_proto::status::Status;
use victory_metal_proto::wire::Wire;

fn rows(dir: &std::path::Path) -> Vec<std::collections::HashMap<String, String>> {
    let text = std::fs::read_to_string(dir.join("back.csv")).unwrap();
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().unwrap().split(',').collect();
    assert_eq!(header.join(","), HEADER);
    lines
        .map(|l| {
            header
                .iter()
                .map(|h| h.to_string())
                .zip(l.split(',').map(str::to_owned))
                .collect()
        })
        .collect()
}

#[test]
fn a_failing_op_and_what_it_takes_with_it_are_counted_as_dropped() {
    let dir = std::env::temp_dir().join(format!("victory-dropstats-exec-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // SAFETY: nothing else in this test binary reads the environment concurrently.
    unsafe { std::env::set_var("VICTORY_METAL_DROPSTATS", &dir) };
    // The stop-the-block behaviour (`strict_block`); `exec_isolation.rs` has the default.
    let fx = Fx::with_options(victory_metal_back::exec::ExecOptions {
        strict_block: true,
        ..small_options()
    });
    let q = fx.queue();
    // Block 1: a dispatch with no pipeline bound fails the block; the draw and
    // the dispatch after it never run.
    let draw = Command::Draw(<Draw as Wire>::ZERO);
    let indexed = Command::DrawIndexed(<DrawIndexed as Wire>::ZERO);
    let status = fx.submit(&[
        begin(1, q),
        dispatch(1, 1, 1),
        draw,
        indexed,
        dispatch(2, 1, 1),
        end(1),
    ]);
    assert_ne!(status, Status::OK);
    // Block 2: nothing to run, so nothing counted.
    assert_eq!(fx.submit(&[begin(2, q), end(2)]), Status::OK);
    victory_metal_back::dropstats::end_frame();
    // A block the validation refuses (an end with no begin) counts its ops too.
    let refused = fx.submit(&[dispatch(1, 1, 1)]);
    assert_ne!(refused, Status::OK);
    victory_metal_back::dropstats::end_frame();
    let rows = rows(&dir);
    assert_eq!(rows.len(), 2);
    let r = &rows[0];
    assert_eq!(r["frame"], "1");
    assert_eq!(
        (
            r["dispatch"].as_str(),
            r["draw"].as_str(),
            r["draw_indexed"].as_str()
        ),
        ("2", "1", "1")
    );
    assert_eq!(
        (
            r["submitted_draw"].as_str(),
            r["submitted_dispatch"].as_str()
        ),
        ("2", "2")
    );
    // every op was dropped: the first failed, the others went with the block
    assert_eq!(
        (r["dropped_draw"].as_str(), r["dropped_dispatch"].as_str()),
        ("2", "2")
    );
    let reasons = split_pairs(&r["reasons"]);
    assert_eq!(reasons.iter().map(|(_, n)| n).sum::<u64>(), 4);
    assert!(
        reasons
            .iter()
            .any(|(k, n)| k.starts_with("not run after a stop: block stopped") && *n == 3),
        "{reasons:?}"
    );
    assert_eq!(r["sub_none"], "4");
    let r = &rows[1];
    assert_eq!(
        (
            r["submitted_dispatch"].as_str(),
            r["dropped_dispatch"].as_str()
        ),
        ("1", "1")
    );
    assert!(
        r["reasons"].contains("block refused before it ran"),
        "{}",
        r["reasons"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}
