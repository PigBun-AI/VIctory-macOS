//! One JSON object per line, appended; the census resumes from what a file
//! already holds.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;

pub struct Appender {
    file: Mutex<std::io::BufWriter<std::fs::File>>,
}

impl Appender {
    pub fn open(path: &Path) -> Result<Appender, String> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Appender {
            file: Mutex::new(std::io::BufWriter::new(file)),
        })
    }

    pub fn push(&self, row: &Value) {
        let mut f = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(f, "{row}");
        // A Metal assertion aborts the process: the rows so far must be on disk.
        let _ = f.flush();
    }

    pub fn flush(&self) {
        let mut f = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let _ = f.flush();
    }
}

/// Every row of a file (a missing file is no rows; a damaged line is skipped).
pub fn read(path: &Path) -> Vec<Value> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect()
}

/// The values of string field `key` over the rows of a file.
pub fn seen(path: &Path, key: &str) -> HashSet<String> {
    read(path)
        .iter()
        .filter_map(|r| r.get(key).and_then(Value::as_str).map(str::to_owned))
        .collect()
}
