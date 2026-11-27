//! Distribution numbers.

use serde_json::{Value, json};

/// The value at fraction `p` of the sorted values (nearest rank), 0 for none.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// count, median, p95, max and total of `values` (milliseconds).
pub fn summary(values: &[f64]) -> Value {
    let mut v: Vec<f64> = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let r = |x: f64| (x * 100.0).round() / 100.0;
    json!({
        "n": v.len(),
        "median_ms": r(percentile(&v, 0.5)),
        "p95_ms": r(percentile(&v, 0.95)),
        "max_ms": r(v.last().copied().unwrap_or(0.0)),
        "total_s": r(v.iter().sum::<f64>() / 1000.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&v, 0.5), 51.0);
        assert_eq!(percentile(&v, 0.95), 95.0);
        assert_eq!(percentile(&v, 1.0), 100.0);
        assert_eq!(percentile(&[], 0.5), 0.0);
    }

    #[test]
    fn the_summary_sorts_and_totals() {
        let s = summary(&[3000.0, 1000.0, 2000.0]);
        assert_eq!(s["n"], 3);
        assert_eq!(s["median_ms"], 2000.0);
        assert_eq!(s["max_ms"], 3000.0);
        assert_eq!(s["total_s"], 6.0);
    }
}
