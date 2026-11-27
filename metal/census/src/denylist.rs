//! The MSC denylist: the shaders a census run found to take Metal Shader
//! Converter (or the pipeline build behind it) down, as the file
//! `VICTORY_METAL_MSC_DENYLIST` names for the back (`msc/guard.rs`).
//!
//! ```text
//! victory-census denylist [--out DIR] [--file PATH] [--merge]
//! ```
//!
//! A row of `convert-<kind>.jsonl` (`cs`, `vs`, `ps`, `ms`; no `--tag` file)
//! counts when it failed with a process abort (`ProcessAbort`, `MscFatal`) or
//! a timeout (`ProcessTimeout`: `--isolate` recorded them). The back's key is
//! the FNV-1a hash of the shader's DXIL container (`hash_parts(&[dxil])`, the
//! hash its drop statistics name a stage by), found through
//! `vsh/<census hash>.vsh`. Geometry-shader rows are left out: their input is
//! a vertex shader that the `vs` kind converts on its own, so its hash cannot
//! tell the abort of the pair from a vertex shader that converts.
//!
//! One line per shader: `<fnv hash, 16 hex> <TAB> <census hash> <TAB> <kind> <TAB> <class>`;
//! `#` starts a comment. The file is written whole (replacing what was there)
//! unless `--merge`, which keeps the lines already in it: a run over a subset
//! of the shaders must not forget the others. `convert --isolate` writes it
//! when it ends (`<out>/msc-denylist.txt`, or `--denylist PATH`; merged when
//! the run was a subset, and then the rows of its own `--tag`ged file count
//! too).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use victory_metal_back::msc::guard::stage_hash;
use victory_shader::emit::wire;

use crate::{arg, flag, jsonl, out_dir};

/// The kinds with a back-side key (see the module docs).
const KINDS: [&str; 4] = ["cs", "vs", "ps", "ms"];
/// The failure classes that mean the process did not survive.
pub const FATAL_CLASSES: [&str; 3] = ["ProcessAbort", "MscFatal", "ProcessTimeout"];

pub fn default_file(out: &Path) -> PathBuf {
    out.join("msc-denylist.txt")
}

/// `<fnv>` -> (census hash, kind, class), from the rows of `out` and from
/// `extra` row files (`(kind, path)`: the `--tag`ged file an isolated run
/// has just written).
fn collect(out: &Path, extra: &[(&str, PathBuf)]) -> BTreeMap<u64, (String, String, String)> {
    let mut found = BTreeMap::new();
    let mut files: Vec<(&str, PathBuf)> = KINDS
        .iter()
        .map(|k| (*k, out.join(format!("convert-{k}.jsonl"))))
        .collect();
    files.extend(extra.iter().filter(|(k, _)| KINDS.contains(k)).cloned());
    for (kind, path) in files {
        for row in jsonl::read(&path) {
            let class = row["fail_class"].as_str().unwrap_or("");
            if row["ok"] == true || !FATAL_CLASSES.contains(&class) {
                continue;
            }
            let Some(hash) = row["hash"].as_str() else {
                continue;
            };
            let Ok(bytes) = std::fs::read(out.join("vsh").join(format!("{hash}.vsh"))) else {
                eprintln!("denylist: no vsh for {hash}, left out");
                continue;
            };
            let Some(shader) = wire::decode(&bytes) else {
                eprintln!("denylist: {hash}.vsh does not decode, left out");
                continue;
            };
            found.insert(
                stage_hash(&shader.dxil),
                (hash.to_owned(), kind.to_owned(), class.to_owned()),
            );
        }
    }
    found
}

/// The lines of an existing file as they are (key, whole line).
fn existing(file: &Path) -> BTreeMap<u64, String> {
    let mut m = BTreeMap::new();
    for l in std::fs::read_to_string(file).unwrap_or_default().lines() {
        if let Some(h) = l
            .split_whitespace()
            .next()
            .and_then(|t| u64::from_str_radix(t, 16).ok())
            && !l.starts_with('#')
        {
            m.insert(h, l.to_owned());
        }
    }
    m
}

/// Writes the denylist for the rows under `out` to `file` (merging into what
/// is there when `merge`); returns how many shaders it names.
pub fn write(
    out: &Path,
    file: &Path,
    merge: bool,
    extra: &[(&str, PathBuf)],
) -> Result<usize, String> {
    let mut lines = if merge {
        existing(file)
    } else {
        BTreeMap::new()
    };
    for (h, (census, kind, class)) in collect(out, extra) {
        lines.insert(h, format!("{h:016x}\t{census}\t{kind}\t{class}"));
    }
    let mut text = String::from(
        "# victory-census denylist: shaders that took Metal Shader Converter down (process abort or timeout).\n\
         # Set VICTORY_METAL_MSC_DENYLIST to this file; the Metal back then refuses them (Unsupported: census: MSC fatal).\n\
         # hash = FNV-1a of the DXIL container (victory-metal-proto hash_parts), census hash, kind, class\n",
    );
    for l in lines.values() {
        text.push_str(l);
        text.push('\n');
    }
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Written beside and renamed: a running game reads the file by its time stamp.
    let tmp = file.with_extension("txt.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, file).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(lines.len())
}

pub fn run(args: &[String]) -> Result<(), String> {
    let out = out_dir(args);
    let file = arg(args, "--file")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_file(&out));
    let n = write(&out, &file, flag(args, "--merge"), &[])?;
    println!("denylist: {n} shaders in {}", file.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use victory_metal_back::msc::guard::parse_denylist;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("victory-census-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_merge_keeps_the_lines_a_rewrite_drops_and_the_back_reads_both() {
        let out = temp("denylist");
        let file = default_file(&out);
        std::fs::write(
            &file,
            "# old\n00000000000000aa\tdeadbeefdeadbeef\tcs\tProcessAbort\n",
        )
        .unwrap();
        // A row that failed, but whose shader file is missing: left out.
        std::fs::write(
            out.join("convert-cs.jsonl"),
            r#"{"hash":"0123456789abcdef","kind":"cs","ok":false,"fail_class":"ProcessAbort"}
{"hash":"1123456789abcdef","kind":"cs","ok":false,"fail_class":"Refused:Doubles"}
"#,
        )
        .unwrap();
        assert_eq!(write(&out, &file, true, &[]).unwrap(), 1);
        let kept = parse_denylist(&std::fs::read_to_string(&file).unwrap());
        assert!(kept.contains(&0xaa) && kept.len() == 1);
        assert_eq!(write(&out, &file, false, &[]).unwrap(), 0);
        assert!(parse_denylist(&std::fs::read_to_string(&file).unwrap()).is_empty());
        let _ = std::fs::remove_dir_all(&out);
    }
}
