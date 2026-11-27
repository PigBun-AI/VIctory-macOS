//! The whole suite, in process, at small sizes: every CPU/GPU check must
//! agree. Needs a Metal device (any Mac).

#![cfg(target_vendor = "apple")]

use victory_metal_bridge::unix::InProcess;
use victory_metal_probe::suite::{Config, run};

#[test]
fn quick_suite_has_no_failed_check() {
    let report = run(&InProcess, &Config::quick());
    assert!(
        report.failures.is_empty(),
        "failed checks: {:#?}",
        report.failures
    );
    assert!(
        report
            .lines
            .iter()
            .any(|l| l.starts_with("METRIC call.empty"))
    );
    assert!(
        report
            .lines
            .iter()
            .any(|l| l.starts_with("METRIC empty_cb.classic.round_trip"))
    );
}
