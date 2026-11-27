//! Golden bytes as hex text.
//!
//! The repository's asset policy (`cargo xtask policy`, docs/asset-policy.md)
//! only takes small text files, so golden byte streams are kept as
//! `NAME.txt`: `#` comment lines, then the bytes as two-digit hex words, 16
//! per line, spaces between. Included by the proto, back and front tests
//! (`#[path = "..."] mod hexfile;`), so there is one reader and one writer.
#![allow(dead_code)]

use std::path::Path;

/// The text form of `bytes`; `what` goes into the header comment.
pub fn to_text(what: &str, bytes: &[u8]) -> String {
    let mut s = format!(
        "# golden bytes of {what} ({} bytes), synthetic. Hex words, 16 per line.\n\
         # Regenerate: UPDATE_GOLDEN=1 cargo test (see the test that reads this file).\n",
        bytes.len()
    );
    for line in bytes.chunks(16) {
        let words: Vec<String> = line.iter().map(|b| format!("{b:02x}")).collect();
        s.push_str(&words.join(" "));
        s.push('\n');
    }
    s
}

/// The bytes of a text golden (`Err` names the first bad word).
pub fn parse(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        for w in line.split_whitespace() {
            if w.len() != 2 {
                return Err(format!("not a two-digit hex word: {w:?}"));
            }
            out.push(u8::from_str_radix(w, 16).map_err(|_| format!("not hex: {w:?}"))?);
        }
    }
    Ok(out)
}

/// Reads a text golden file.
pub fn read(path: &Path) -> Vec<u8> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; run with UPDATE_GOLDEN=1 to create it",
            path.display()
        )
    });
    parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Writes a text golden file.
pub fn write(path: &Path, what: &str, bytes: &[u8]) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(path, to_text(what, bytes)).unwrap();
}
