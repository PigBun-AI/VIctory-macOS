//! `victory-census meshstat`: what every translated mesh shader declares
//! (maximum vertices and primitives, per-vertex and per-primitive outputs,
//! group-shared bytes) and how big Metal counts its output
//! (`docs/metal/mesh32k.md`).
//!
//! Reads `translate.jsonl` and `vsh/` under `--out`, and the failure text of
//! `convert-ms.jsonl` for the size Metal named; writes `meshstat.tsv` under
//! `--out` (outside the repository: rows are per hash). The size is also
//! computed from the DXIL signature with the layout measured in
//! `back/tests/mesh_limit.rs`, so the two can be compared.

use victory_metal_back::mesh_size::{self, Output};
use victory_shader::emit::wire;
use victory_shader::{Container, FourCC};

use crate::{arg, jsonl, out_dir, sig};

/// The mesh facts of a DXIL container: `PSV0` counts and the two signatures.
pub struct MeshFacts {
    pub max_vertices: u32,
    pub max_primitives: u32,
    pub shared_bytes: u32,
    pub threads: [u32; 3],
    pub vertex: Vec<Output>,
    pub primitive: Vec<Output>,
}

fn outputs(els: &[sig::Element]) -> Vec<Output> {
    els.iter()
        // Registers of -1 are system values that take no interpolant slot
        // other than the ones the layout names (position, distances).
        .map(|e| Output {
            name: format!("{}{}", e.name, e.index),
            system_value: e.system_value,
            components: e.mask.count_ones(),
            bytes_per_component: 4,
            allocated: e.register != u32::MAX,
        })
        .collect()
}

pub fn facts(dxil: &[u8]) -> Option<MeshFacts> {
    let c = Container::parse(dxil).ok()?;
    let psv = c.part(FourCC::PSV0)?.data;
    let size = u32::from_le_bytes(psv.get(0..4)?.try_into().ok()?) as usize;
    let info = psv.get(4..4 + size).filter(|i| i.len() >= 48)?;
    if info[24] != 13 {
        return None;
    }
    let half = |at: usize| u32::from(u16::from_le_bytes([info[at], info[at + 1]]));
    let word = |at: usize| u32::from_le_bytes(info[at..at + 4].try_into().unwrap());
    let part = |cc| c.part(cc).map(|p| sig::parse(p.data)).unwrap_or_default();
    Some(MeshFacts {
        max_vertices: half(12),
        max_primitives: half(14),
        shared_bytes: word(0),
        threads: [word(36), word(40), word(44)],
        vertex: outputs(&part(FourCC::OSG1)),
        primitive: outputs(&part(FourCC::PSG1)),
    })
}

pub fn run(args: &[String]) -> Result<(), String> {
    let out = out_dir(args);
    let translate = jsonl::read(&out.join("translate.jsonl"));
    let convert = jsonl::read(
        &out.join(
            arg(args, "--convert")
                .as_deref()
                .unwrap_or("convert-ms.jsonl"),
        ),
    );
    let mut metal: std::collections::HashMap<String, (bool, Option<u64>)> = Default::default();
    for r in &convert {
        let Some(h) = r["hash"].as_str() else {
            continue;
        };
        let size = r["error"].as_str().and_then(|e| {
            let i = e.find("Total mesh size (")? + "Total mesh size (".len();
            e[i..].split(')').next()?.parse().ok()
        });
        metal.insert(h.to_owned(), (r["ok"] == true, size));
    }
    let mut rows = String::from(
        "hash\tmetal\tmetal_size\tmodel_size\tnv\tnp\tthreads\tshared\tvertex_bytes\tprim_bytes\tvertex_elems\tprim_elems\n",
    );
    let mut n = 0;
    for r in translate
        .iter()
        .filter(|r| r["ok"] == true && r["stage"] == "Ms")
    {
        let Some(h) = r["hash"].as_str() else {
            continue;
        };
        let Ok(bytes) = std::fs::read(out.join("vsh").join(format!("{h}.vsh"))) else {
            continue;
        };
        let Some(s) = wire::decode(&bytes) else {
            continue;
        };
        let Some(f) = facts(&s.dxil) else { continue };
        let vb = mesh_size::stride(&f.vertex, true);
        let pb = mesh_size::stride(&f.primitive, false);
        let model = mesh_size::total(f.max_vertices, vb, f.max_primitives, pb);
        let (ok, size) = metal.get(h).copied().unwrap_or((false, None));
        let desc = |els: &[Output]| {
            els.iter()
                .map(|e| format!("{}:{}", e.name, e.components))
                .collect::<Vec<_>>()
                .join(",")
        };
        rows += &format!(
            "{h}\t{}\t{}\t{model}\t{}\t{}\t{}\t{}\t{vb}\t{pb}\t{}\t{}\n",
            match (metal.contains_key(h), ok) {
                (false, _) => "none",
                (true, true) => "ok",
                (true, false) => "fail",
            },
            size.map_or(String::from("-"), |s| s.to_string()),
            f.max_vertices,
            f.max_primitives,
            f.threads.iter().product::<u32>(),
            f.shared_bytes,
            desc(&f.vertex),
            desc(&f.primitive),
        );
        n += 1;
    }
    let path = out.join("meshstat.tsv");
    std::fs::write(&path, rows).map_err(|e| e.to_string())?;
    println!("meshstat: {n} mesh shaders -> {}", path.display());
    Ok(())
}
