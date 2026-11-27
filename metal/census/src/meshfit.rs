//! `victory-census meshfit`: [`victory_metal_back::mesh_fit::repack_outputs`]
//! on every translated mesh shader (or those of `--hashes FILE`): how many
//! the bitstream writer reproduces byte for byte, how many are repacked and
//! by how much, and why the others are not. `--dxil-out DIR` keeps the
//! repacked containers as `DIR/<hash>.dxil` for `convert --dxil-dir`.

use std::collections::BTreeMap;

use victory_metal_back::bitcode::Bitcode;
use victory_metal_back::mesh_fit::{self, FitError};
use victory_shader::emit::wire;

use crate::{arg, jsonl, out_dir};

pub fn run(args: &[String]) -> Result<(), String> {
    let out = out_dir(args);
    let keep = arg(args, "--dxil-out").map(std::path::PathBuf::from);
    if let Some(d) = &keep {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let only: Option<std::collections::HashSet<String>> = arg(args, "--hashes").map(|p| {
        std::fs::read_to_string(p)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split('\t').next())
            .filter(|h| h.len() == 16)
            .map(str::to_owned)
            .collect()
    });
    let mut hashes: Vec<String> = jsonl::read(&out.join("translate.jsonl"))
        .iter()
        .filter(|r| r["ok"] == true && r["stage"] == "Ms")
        .filter_map(|r| r["hash"].as_str().map(str::to_owned))
        .filter(|h| only.as_ref().is_none_or(|o| o.contains(h)))
        .collect();
    hashes.sort();
    let (mut identical, mut different, mut parsed_not) = (0, 0, 0);
    let mut repacked = 0;
    let mut unchanged = 0;
    let mut why: BTreeMap<String, usize> = BTreeMap::new();
    let mut saved: BTreeMap<u32, usize> = BTreeMap::new();
    let mut rows = String::from("hash\tresult\tbefore\tafter\telements\tstores\n");
    for h in &hashes {
        let Ok(bytes) = std::fs::read(out.join("vsh").join(format!("{h}.vsh"))) else {
            continue;
        };
        let Some(s) = wire::decode(&bytes) else {
            continue;
        };
        // The writer reproduces the stream?
        match bitcode_of(&s.dxil).map(|b| Bitcode::parse(b).and_then(|t| t.write()).map(|w| w == b))
        {
            Some(Ok(true)) => identical += 1,
            Some(Ok(false)) => different += 1,
            _ => parsed_not += 1,
        }
        match mesh_fit::repack_outputs(&s.dxil, None) {
            Ok(Some(f)) => {
                repacked += 1;
                *saved.entry(f.stride_before - f.stride_after).or_default() += 1;
                rows += &format!(
                    "{h}\trepacked\t{}\t{}\t{}\t{}\n",
                    f.stride_before, f.stride_after, f.elements, f.stores
                );
                if let Some(d) = &keep {
                    std::fs::write(d.join(format!("{h}.dxil")), &f.dxil)
                        .map_err(|e| e.to_string())?;
                }
            }
            Ok(None) => {
                unchanged += 1;
                rows += &format!("{h}\tunchanged\t\t\t\t\n");
            }
            Err(e) => {
                let kind = match &e {
                    FitError::Malformed(m) => format!("malformed: {m}"),
                    FitError::Unsupported(m) => format!("unsupported: {m}"),
                };
                *why.entry(kind.clone()).or_default() += 1;
                rows += &format!("{h}\terror\t{kind}\t\t\t\n");
            }
        }
    }
    let path = out.join("meshfit.tsv");
    std::fs::write(&path, rows).map_err(|e| e.to_string())?;
    println!(
        "meshfit: {} mesh shaders; stream reproduced byte for byte {identical}, differs {different}, unreadable {parsed_not}",
        hashes.len()
    );
    println!(
        "  repacked {repacked}, nothing to gain {unchanged}, refused {}",
        why.values().sum::<usize>()
    );
    for (k, v) in &why {
        println!("    {v:5}  {k}");
    }
    println!("  bytes per vertex saved (repacked): {saved:?}");
    println!("  rows: {}", path.display());
    Ok(())
}

/// The bitcode inside a DXIL container.
fn bitcode_of(c: &[u8]) -> Option<&[u8]> {
    let n = u32::from_le_bytes(c.get(28..32)?.try_into().ok()?) as usize;
    for i in 0..n {
        let at = u32::from_le_bytes(c.get(32 + 4 * i..36 + 4 * i)?.try_into().ok()?) as usize;
        if c.get(at..at + 4)? == b"DXIL" {
            let size = u32::from_le_bytes(c.get(at + 4..at + 8)?.try_into().ok()?) as usize;
            let part = c.get(at + 8..at + 8 + size)?;
            let off = u32::from_le_bytes(part.get(16..20)?.try_into().ok()?) as usize;
            let len = u32::from_le_bytes(part.get(20..24)?.try_into().ok()?) as usize;
            return part.get(8 + off..8 + off + len);
        }
    }
    None
}
