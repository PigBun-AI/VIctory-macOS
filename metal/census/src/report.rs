//! The census numbers and the two failure lists, from the rows the earlier
//! stages wrote under `--out`: `census.json` (machine-readable, everything),
//! `mac-side.md` (our backend, MSC, Metal limits, what we lack),
//! `shared-translator.md` (for the Windows team; the translator's own
//! failures). Only hashes, stages and error text go in; no game bytes.
//! Output lands in `--research` (default `<project>/research/census`).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{arg, jsonl, out_dir, stats};

/// The kinds of the conversion stage, in the order of the chain.
const KINDS: [&str; 5] = ["cs", "vs", "ps", "ms", "gs"];
/// How many sample hashes a failure group shows in `census.json`.
const SAMPLES: usize = 5;

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_owned()
}

fn ms(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

/// `GFX10 code at 0x1d70: x` to `x`, so the offsets do not split a group.
fn normalize(e: &str) -> String {
    match e.strip_prefix("GFX10 code at ") {
        Some(rest) => rest.split_once(": ").map_or(rest, |(_, t)| t).to_owned(),
        None => e.to_owned(),
    }
}

/// Digits of a message become `N` (sizes in a limit message).
fn squash(e: &str) -> String {
    let mut out = String::new();
    let mut digits = false;
    for c in e.chars() {
        if c.is_ascii_digit() {
            if !digits {
                out.push('N');
            }
            digits = true;
        } else {
            digits = false;
            out.push(c);
        }
    }
    out
}

struct Group {
    key: String,
    count: usize,
    hashes: Vec<String>,
    example: String,
    detail: BTreeMap<String, usize>,
}

fn groups(rows: &[(String, String, String, String)]) -> Vec<Group> {
    // (key, hash, error, detail)
    let mut map: BTreeMap<String, Group> = BTreeMap::new();
    for (key, hash, error, detail) in rows {
        let g = map.entry(key.clone()).or_insert_with(|| Group {
            key: key.clone(),
            count: 0,
            hashes: Vec::new(),
            example: error.clone(),
            detail: BTreeMap::new(),
        });
        g.count += 1;
        g.hashes.push(hash.clone());
        *g.detail.entry(detail.clone()).or_default() += 1;
    }
    let mut v: Vec<Group> = map.into_values().collect();
    for g in &mut v {
        g.hashes.sort();
    }
    v.sort_by(|a, b| b.count.cmp(&a.count).then(a.key.cmp(&b.key)));
    v
}

fn group_json(g: &Group) -> Value {
    json!({
        "class": g.key,
        "count": g.count,
        "sample_hashes": g.hashes.iter().take(SAMPLES).collect::<Vec<_>>(),
        "example": g.example,
        "detail": g.detail,
    })
}

fn timing(rows: &[Value], key: &str) -> Value {
    let v: Vec<f64> = rows.iter().filter_map(|r| ms(r, key)).collect();
    stats::summary(&v)
}

fn timings(rows: &[Value]) -> Value {
    json!({
        "msc": timing(rows, "msc_ms"),
        "library": timing(rows, "lib_ms"),
        "pipeline": timing(rows, "pso_ms"),
    })
}

fn read_json(path: &Path) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

pub fn run(args: &[String]) -> Result<(), String> {
    let out = out_dir(args);
    let research = arg(args, "--research")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::project_root().join("research").join("census"));
    std::fs::create_dir_all(&research).map_err(|e| e.to_string())?;
    let translate = jsonl::read(&out.join("translate.jsonl"));
    if translate.is_empty() {
        return Err(format!("no translate.jsonl under {}", out.display()));
    }
    let kind_rows: BTreeMap<&str, Vec<Value>> = KINDS
        .iter()
        .map(|k| (*k, jsonl::read(&out.join(format!("convert-{k}.jsonl")))))
        .collect();

    // Programs and their translation.
    let mut programs: BTreeMap<String, usize> = BTreeMap::new();
    for r in &translate {
        for p in r["programs"].as_array().into_iter().flatten() {
            *programs
                .entry(p.as_str().unwrap_or("?").to_owned())
                .or_default() += 1;
        }
    }
    let ok: Vec<&Value> = translate.iter().filter(|r| r["ok"] == true).collect();
    let mut translated: BTreeMap<String, usize> = BTreeMap::new();
    for r in &ok {
        *translated.entry(s(r, "stage")).or_default() += 1;
    }
    // A failed container is counted under its programs' stage (the last one: ES+VS is a VS).
    let mut translate_failed: BTreeMap<String, usize> = BTreeMap::new();
    let failures: Vec<(String, String, String, String)> = translate
        .iter()
        .filter(|r| r["ok"] != true)
        .map(|r| {
            let stage = r["programs"]
                .as_array()
                .and_then(|a| a.last())
                .and_then(Value::as_str)
                .unwrap_or("none")
                .to_owned();
            *translate_failed.entry(stage.clone()).or_default() += 1;
            let text = normalize(&s(r, "error"));
            (
                format!("{stage}: {text}"),
                s(r, "hash"),
                s(r, "error"),
                stage,
            )
        })
        .collect();
    let translate_groups = groups(&failures);
    let translate_ms: Vec<f64> = translate.iter().filter_map(|r| ms(r, "ms")).collect();
    let translate_by_stage: BTreeMap<String, Value> = translated
        .keys()
        .map(|st| {
            let v: Vec<f64> = translate
                .iter()
                .filter(|r| r["stage"] == st.as_str())
                .filter_map(|r| ms(r, "ms"))
                .collect();
            (st.clone(), stats::summary(&v))
        })
        .collect();
    let serial_translate = jsonl::read(&out.join("serial").join("translate.jsonl"));

    // The same scan with a host that has wave64: which failures were the host's.
    let w64 = jsonl::read(&out.join("w64").join("translate.jsonl"));
    let w64_ok: std::collections::HashSet<String> = w64
        .iter()
        .filter(|r| r["ok"] == true)
        .map(|r| s(r, "hash"))
        .collect();
    let recovered: Vec<&(String, String, String, String)> =
        failures.iter().filter(|f| w64_ok.contains(&f.1)).collect();

    // The conversion stage.
    let mut convert = serde_json::Map::new();
    let mut mac: Vec<(String, String, String, String)> = Vec::new(); // class, hash, error, kind
    let mut chain_pass: BTreeMap<&str, usize> = BTreeMap::new();
    for k in KINDS {
        let rows = &kind_rows[k];
        let ok_rows: Vec<Value> = rows.iter().filter(|r| r["ok"] == true).cloned().collect();
        let failed: Vec<(String, String, String, String)> = rows
            .iter()
            .filter(|r| r["ok"] != true)
            .map(|r| {
                let key = format!("{}: {}", s(r, "fail_stage"), s(r, "fail_class"));
                let e = s(r, "error");
                (key, s(r, "hash"), e, k.to_owned())
            })
            .collect();
        for (key, hash, e, kind) in &failed {
            mac.push((
                format!("{kind} / {key}"),
                hash.clone(),
                e.clone(),
                kind.clone(),
            ));
        }
        chain_pass.insert(k, ok_rows.len());
        let serial = jsonl::read(&out.join(format!("convert-{k}-benchA.jsonl")));
        let serial_ok: Vec<Value> = serial.iter().filter(|r| r["ok"] == true).cloned().collect();
        let metallib: u64 = ok_rows
            .iter()
            .filter_map(|r| r["metallib_bytes"].as_u64())
            .sum();
        convert.insert(
            k.to_owned(),
            json!({
                "n": rows.len(),
                "ok": ok_rows.len(),
                "failed": rows.len() - ok_rows.len(),
                "failures": groups(&failed.iter().map(|(a, b, c, _)| (a.clone(), b.clone(), squash(c), c.clone())).collect::<Vec<_>>())
                    .iter().map(group_json).collect::<Vec<_>>(),
                "timing_parallel": timings(&ok_rows),
                "timing_serial_sample": { "n": serial_ok.len(), "steps": timings(&serial_ok) },
                "metallib_bytes_total": metallib,
            }),
        );
    }

    // The whole chain: a program passes when its translation and every
    // later stage passed; failures of the translation are counted per stage.
    let chain = json!({
        "Cs": { "programs": programs.get("Cs"), "translated": translated.get("Cs"), "pipeline_ok": chain_pass["cs"] },
        "Vs": { "programs": programs.get("Vs"), "translated": translated.get("Vs"), "pipeline_ok_with_default_ps": chain_pass["vs"], "rect_list_gs_ok": chain_pass["gs"] },
        "Ps": { "programs": programs.get("Ps"), "translated": translated.get("Ps"), "pipeline_ok_with_fullscreen_vs": chain_pass["ps"] },
        "Ms": { "programs": programs.get("Ms"), "translated": translated.get("Ms"), "pipeline_ok_with_default_ps": chain_pass["ms"] },
        "Gs": { "programs": programs.get("Gs"), "translated": translated.get("Gs").copied().unwrap_or(0), "note": "the title's own geometry programs: the translator makes no host shader for them" },
    });

    // The archive measurement: three serial runs over the same pipelines.
    let bench = |tag: &str| -> Value {
        let mut rows = Vec::new();
        for k in ["cs", "vs", "ps", "ms"] {
            rows.extend(jsonl::read(&out.join(format!("convert-{k}{tag}.jsonl"))));
        }
        let ok: Vec<&Value> = rows.iter().filter(|r| r["ok"] == true).collect();
        let pso: Vec<f64> = ok.iter().filter_map(|r| ms(r, "pso_ms")).collect();
        let mut v = stats::summary(&pso);
        v["pipelines"] = json!(ok.len());
        v["meta"] = read_json(&out.join(format!("convert-meta{tag}.json")));
        v
    };
    let mut full_archives = serde_json::Map::new();
    for k in ["cs", "vs", "ps", "ms"] {
        let made = read_json(&out.join(format!("convert-meta-archive-{k}.json")));
        let opened = read_json(&out.join(format!("convert-meta-lookup-{k}.json")));
        let built = jsonl::read(&out.join(format!("convert-{k}-archive-{k}.jsonl")));
        let looked = jsonl::read(&out.join(format!("convert-{k}-lookup-{k}.jsonl")));
        let ok_of = |rows: &[Value]| {
            rows.iter()
                .filter(|r| r["ok"] == true)
                .cloned()
                .collect::<Vec<_>>()
        };
        let looked = ok_of(&looked);
        let pso: Vec<f64> = looked.iter().filter_map(|r| ms(r, "pso_ms")).collect();
        full_archives.insert(
            k.to_owned(),
            json!({
                "pipelines": ok_of(&built).len(),
                "bytes": made["archive_bytes"],
                "flush_s": made["archive_flush_s"],
                "open_ms": opened["archive_open_ms"],
                "lookup_serial_pipeline_ms": stats::summary(&pso),
            }),
        );
    }
    let archive = json!({
        "cold_compile": bench("-benchA"),
        "no_archive_system_cache_warm": bench("-benchC"),
        "from_archive": bench("-benchB"),
        "full_archives": full_archives,
    });

    let census = json!({
        "date": "2026-11-27",
        "chain": "Xbox ISA -> DXIL (victory_shader::emit::translate) -> Metal Shader Converter 4.0.3 -> MTL4Compiler",
        "translate_settings": read_json(&out.join("translate-meta.json")),
        "containers_unique": translate.len(),
        "programs": programs,
        "translate": {
            "ok": ok.len(),
            "failed": translate.len() - ok.len(),
            "translated_by_stage": translated,
            "failed_by_stage": translate_failed,
            "failures": translate_groups.iter().map(group_json).collect::<Vec<_>>(),
            "translated_with_host_wave64": recovered.len(),
            "timing_parallel": stats::summary(&translate_ms),
            "timing_by_stage_parallel": translate_by_stage,
            "timing_serial_sample": stats::summary(&serial_translate.iter().filter_map(|r| ms(r, "ms")).collect::<Vec<_>>()),
        },
        "convert": convert,
        "chain_summary": chain,
        "archive": archive,
    });
    std::fs::write(
        research.join("census.json"),
        serde_json::to_string_pretty(&census).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    // shared-translator.md
    let mut md = String::new();
    let _ = writeln!(
        md,
        "# Shared translator: what does not translate (2026-11-27)\n"
    );
    let _ = writeln!(
        md,
        "Run: `victory_shader::emit::translate` on every container of the package, natively on arm64 (the pure-Rust\npart, no validator), host settings wave 32, view limit {}, raw views {}, RDNA off. Container hash = first 16 hex\ndigits of the BLAKE3 of the container (the hash the game log prints). {} unique containers, {} translated, {} failed.\n",
        crate::defaults::VIEW_LIMIT,
        crate::defaults::RAW_TWINS,
        translate.len(),
        ok.len(),
        translate.len() - ok.len()
    );
    let _ = writeln!(md, "Windows line: the translator is the same code, so a failure that does not name the host fails there as well.\nThe wave64 group depends on the host's widest wave: the same scan with host wave 64 translates {} of the {} failures
(all of the `no wave32 lowering` group); a Windows GPU that runs wave32 only (NVIDIA, Intel) fails the same way as the Mac.\n", recovered.len(), failures.len());
    for g in &translate_groups {
        let _ = writeln!(
            md,
            "## {} ({})\n\nTranslator text: `{}`\n",
            g.key, g.count, g.example
        );
        let _ = writeln!(md, "| hash | stage | error (verbatim) |\n|---|---|---|");
        let mut rows: Vec<&(String, String, String, String)> =
            failures.iter().filter(|f| f.0 == g.key).collect();
        rows.sort_by(|a, b| a.1.cmp(&b.1));
        for (_, hash, e, stage) in rows {
            let _ = writeln!(md, "| {hash} | {stage} | {e} |");
        }
        md.push('\n');
    }
    std::fs::write(research.join("shared-translator.md"), md).map_err(|e| e.to_string())?;

    // mac-side.md, with the long groups in a tsv.
    let mut md = String::new();
    let mut tsv = String::from("hash\tkind\tclass\terror\n");
    let _ = writeln!(
        md,
        "# Mac side: what our backend, MSC 4.0.3 and Metal do not take (2026-11-27)\n"
    );
    let _ = writeln!(
        md,
        "Chain per program: translated DXIL -> MSC with the host root signature -> metallib -> MTL4Compiler pipeline.\nPartner stages: VS gets a one-output pixel shader, PS a full-screen-triangle VS that writes its inputs, MS a one-output PS.\nHashes as in `shared-translator.md`. Every row is in `mac-side.tsv` (hash, kind, class, error); the Windows line\ndoes not use MSC, so these do not occur there unless the text says the DXIL itself is the problem.\n"
    );
    let groups_all = groups(&mac);
    let _ = writeln!(
        md,
        "| kind | programs built | pipeline ok | failed |\n|---|---|---|---|"
    );
    for k in KINDS {
        let n = kind_rows[k].len();
        let ok = chain_pass[k];
        let _ = writeln!(md, "| {k} | {n} | {ok} | {} |", n - ok);
    }
    md.push('\n');
    for g in &groups_all {
        let _ = writeln!(
            md,
            "## {} ({})\n\nExample: `{}`\n\nSample hashes: {}\n",
            g.key,
            g.count,
            g.example,
            g.hashes
                .iter()
                .take(20)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
        if let Some(note) = note(&g.key, &g.example) {
            let _ = writeln!(md, "{note}\n");
        }
    }
    for (class, hash, e, kind) in &mac {
        let _ = writeln!(tsv, "{hash}\t{kind}\t{class}\t{e}");
    }
    std::fs::write(research.join("mac-side.md"), md).map_err(|e| e.to_string())?;
    std::fs::write(research.join("mac-side.tsv"), tsv).map_err(|e| e.to_string())?;
    println!("report: {}", research.display());
    Ok(())
}

/// What a failure group means, where the census can say (offline reading of
/// the translator and the converter's own text).
fn note(key: &str, example: &str) -> Option<&'static str> {
    if example.contains("attributeAtVertex") {
        Some(
            "Cause: the translator emits `dx.op.attributeAtVertex` when a pixel program computes its own barycentrics or reads P10 / P20 (`emit/lower.rs`, `per_vertex`); MSC 4.0.3 does not implement it. A feature gap on our side: the pixel stage needs either a lowering MSC takes (`SV_Barycentrics` plus per-vertex values the vertex stage writes out) or a different interpolation path. The Windows line is not affected (D3D12 shader model 6.1 has the instruction).",
        )
    } else if example.contains("atomicBinOp.i64") {
        Some(
            "Cause: a 64-bit atomic on a buffer the converter does not take (known: i64 atomics on a heap buffer). Windows is not affected.",
        )
    } else if key.contains("total-mesh-size") {
        Some(
            "Cause: Metal caps the total threadgroup memory of a mesh pipeline (vertices, primitives and payload) at 32768 bytes; these D3D12 mesh shaders declare more output. A hardware limit: the fix is splitting the work across more groups, which changes the shader. Windows is not affected.",
        )
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_do_not_split_translator_groups() {
        assert_eq!(
            normalize("GFX10 code at 0x1d70: a descriptor read only as data is not supported"),
            "a descriptor read only as data is not supported"
        );
        assert_eq!(normalize("not a DXBC container"), "not a DXBC container");
    }

    #[test]
    fn sizes_in_a_limit_message_are_squashed() {
        assert_eq!(
            squash("size (34816) exceeds (32768)"),
            "size (N) exceeds (N)"
        );
    }

    #[test]
    fn groups_count_and_sort() {
        let rows = vec![
            (
                "a".to_owned(),
                "h2".to_owned(),
                "e".to_owned(),
                "x".to_owned(),
            ),
            (
                "b".to_owned(),
                "h3".to_owned(),
                "e".to_owned(),
                "x".to_owned(),
            ),
            (
                "a".to_owned(),
                "h1".to_owned(),
                "e".to_owned(),
                "x".to_owned(),
            ),
        ];
        let g = groups(&rows);
        assert_eq!(g[0].key, "a");
        assert_eq!(g[0].count, 2);
        assert_eq!(g[0].hashes, ["h1", "h2"]);
    }
}
