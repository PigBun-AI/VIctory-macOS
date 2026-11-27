//! What ran in a submission that failed.
//!
//! A command buffer the card did not finish (a timeout, a page fault, out of memory) fails with an
//! `NSError` that names no pipeline. With `VICTORY_METAL_LOG` on, every list keeps a journal of
//! the pipelines and copies the executor put into it (runs of the same label are counted, not
//! repeated), kept by the timeline value of the submission until its commit feedback arrives; the
//! feedback handler prints the journal of a failed submission together with the error and the
//! encoder infos Metal attaches (`MTLCommandBufferEncoderInfoErrorKey`).
//!
//! Without the log the journal is not kept and costs nothing; the failure is still signalled to
//! the timeline (see `queue.rs`), which is the part that keeps the title from waiting for ever.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Whether journals are kept (`VICTORY_METAL_LOG` set and not `0`).
pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0"))
}

/// Runs kept per list; the rest is counted.
const MAX_RUNS: usize = 400;

/// The ops of one list, as runs of one label.
#[derive(Default)]
pub(crate) struct Journal {
    runs: Vec<(String, u32)>,
    dropped: u32,
}

impl Journal {
    pub(crate) fn new() -> Option<Journal> {
        enabled().then(Journal::default)
    }

    /// An op named `label` was put into the list.
    pub(crate) fn note(&mut self, label: &str) {
        if let Some((last, n)) = self.runs.last_mut()
            && last == label
        {
            *n += 1;
        } else if self.runs.len() < MAX_RUNS {
            self.runs.push((label.to_owned(), 1));
        } else {
            self.dropped += 1;
        }
    }

    fn render(&self) -> String {
        let mut out = String::new();
        for (label, n) in &self.runs {
            if *n > 1 {
                out.push_str(&format!("    {label} x{n}\n"));
            } else {
                out.push_str(&format!("    {label}\n"));
            }
        }
        if self.dropped > 0 {
            out.push_str(&format!("    ... {} more runs not kept\n", self.dropped));
        }
        out
    }
}

fn journals() -> MutexGuard<'static, BTreeMap<u64, Vec<Journal>>> {
    static J: OnceLock<Mutex<BTreeMap<u64, Vec<Journal>>>> = OnceLock::new();
    J.get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// A list was committed as part of the submission `value`.
pub(crate) fn queue(value: u64, journal: Journal) {
    journals().entry(value).or_default().push(journal);
}

/// The commit feedback of submission `value` arrived: its journals are no longer needed. Returns
/// them as text when `failed`.
pub(crate) fn finish(value: u64, failed: bool) -> String {
    let lists = journals().remove(&value);
    if !failed {
        return String::new();
    }
    match lists {
        None => "  (no journal: VICTORY_METAL_LOG was off when the lists were recorded)\n".into(),
        Some(lists) => {
            let mut out = String::new();
            for (i, l) in lists.iter().enumerate() {
                out.push_str(&format!(
                    "  list {} of {} in the submission:\n",
                    i + 1,
                    lists.len()
                ));
                out.push_str(&l.render());
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_of_one_label_are_counted() {
        let mut j = Journal::default();
        for l in ["cs a", "cs a", "cs b", "cs a"] {
            j.note(l);
        }
        assert_eq!(j.render(), "    cs a x2\n    cs b\n    cs a\n");
    }

    #[test]
    fn a_list_beyond_the_limit_says_so() {
        let mut j = Journal::default();
        for i in 0..MAX_RUNS + 3 {
            j.note(&format!("cs {i}"));
        }
        assert!(j.render().ends_with("    ... 3 more runs not kept\n"));
    }
}
