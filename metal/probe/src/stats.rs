//! Medians and ranges of repeated measurements.

#[derive(Clone, Copy, Debug)]
pub struct Stats {
    pub median: f64,
    pub min: f64,
    pub max: f64,
    pub p99: f64,
    pub n: usize,
}

impl Stats {
    /// `None` for an empty sample set.
    pub fn of(samples: &[f64]) -> Option<Stats> {
        if samples.is_empty() {
            return None;
        }
        let mut v = samples.to_vec();
        v.sort_by(f64::total_cmp);
        let n = v.len();
        let median = if n % 2 == 1 {
            v[n / 2]
        } else {
            (v[n / 2 - 1] + v[n / 2]) / 2.0
        };
        Some(Stats {
            median,
            min: v[0],
            max: v[n - 1],
            p99: v[(n * 99 / 100).min(n - 1)],
            n,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_of_odd_and_even_sets() {
        let s = Stats::of(&[5.0, 1.0, 3.0]).unwrap();
        assert_eq!((s.median, s.min, s.max, s.n), (3.0, 1.0, 5.0, 3));
        assert_eq!(Stats::of(&[4.0, 1.0, 2.0, 3.0]).unwrap().median, 2.5);
        assert!(Stats::of(&[]).is_none());
    }
}
