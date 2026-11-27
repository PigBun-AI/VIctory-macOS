//! `victory-census opcodes`: which GFX10 instructions the game's shader programs use.
//!
//! The scan is the translate stage's (every file of the game directory, SGD5 records unpacked,
//! raw `DXBC` containers by magic, one row per unique container by the BLAKE3 hash the game log
//! names it by), but it stops after the decoder: `victory_shader::native_programs` gives the
//! native programs of a container and `victory_shader::gfx10::decode` their instructions (the
//! decoder's own public interface, called read only: the shared crates are not changed). For each
//! program it counts the mnemonics (`Instruction::name`; `?<encoding>:<opcode>` when the table
//! has no name), the instructions with a DPP form or the GDS bit (which the name does not tell),
//! the program's LDS size and its hardware stage.
//!
//! * `<out>/opcodes.jsonl`: one row per container with its programs' counts (game-derived, stays
//!   outside the repository, like the translate rows).
//! * `<research>/opcodes.json` and `opcodes.md`: the totals per mnemonic and per class (hashes
//!   and counts only, no game names). A class is counted in programs (every native program of
//!   every unique container), in unique code (programs with distinct code bytes) and in
//!   instructions.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use serde_json::{Value, json};
use victory_shader::gfx10::{self, Dpp, Fields};
use victory_shader::{Container, FourCC, HwStage, native_programs};

use crate::translate::{MAX_FILE, sources};
use crate::{arg, jobs, jsonl, out_dir};

/// The classes the question is about: a name, what it is, and the test on one instruction
/// (mnemonic, and the facts the name does not carry).
pub struct Class {
    pub id: &'static str,
    pub what: &'static str,
    test: fn(&str, &Facts) -> bool,
}

/// What an instruction has that its name does not say.
#[derive(Default, Clone, Copy)]
pub struct Facts {
    pub dpp: bool,
    pub gds: bool,
}

fn any_of(name: &str, list: &[&str]) -> bool {
    list.contains(&name)
}

/// The classes, in the order of the report.
pub const CLASSES: &[Class] = &[
    Class {
        id: "gds_ordered",
        what: "GDS / ordered append: ds_gws_*, ds_ordered_count, ds_append, ds_consume, ds_*_gs_reg_*, and any DS instruction with the GDS bit",
        test: |n, f| {
            n.starts_with("ds_gws_")
                || any_of(n, &["ds_ordered_count", "ds_append", "ds_consume"])
                || n.contains("_gs_reg")
                || (n.starts_with("ds_") && f.gds)
        },
    },
    Class {
        id: "bvh",
        what: "BVH / ray tracing: image_bvh_intersect_ray, image_bvh64_intersect_ray (and any image_bvh*)",
        test: |n, _| n.starts_with("image_bvh"),
    },
    Class {
        id: "s_denorm_mode",
        what: "s_denorm_mode (float denorm mode switched in the program)",
        test: |n, _| n == "s_denorm_mode",
    },
    Class {
        id: "s_round_mode",
        what: "s_round_mode (float rounding mode switched in the program)",
        test: |n, _| n == "s_round_mode",
    },
    Class {
        id: "mode_register_write",
        what: "s_setreg_b32 / s_setreg_imm32_b32 (writes of hardware registers, MODE among them)",
        test: |n, _| n == "s_setreg_b32" || n == "s_setreg_imm32_b32",
    },
    Class {
        id: "atomic64_x2",
        what: "64-bit atomics, buffer / global / flat / scratch / image: *_atomic_*_x2",
        test: |n, _| n.contains("atomic") && n.ends_with("_x2"),
    },
    Class {
        id: "atomic64_lds",
        what: "64-bit LDS atomics: ds_*_u64 / _i64 / _f64 / _b64 other than loads, stores and swizzles",
        test: |n, _| {
            n.starts_with("ds_")
                && ["_u64", "_i64", "_f64", "_b64"]
                    .iter()
                    .any(|s| n.ends_with(s))
                && !n.starts_with("ds_read")
                && !n.starts_with("ds_write")
                && !n.starts_with("ds_load")
                && !n.starts_with("ds_store")
                && !n.starts_with("ds_permute")
                && !n.starts_with("ds_bpermute")
        },
    },
    Class {
        id: "atomic_32",
        what: "32-bit atomics (buffer / global / flat / image / ds) for comparison: *atomic* without _x2",
        test: |n, _| {
            (n.contains("atomic") && !n.ends_with("_x2"))
                || (n.starts_with("ds_")
                    && (n.starts_with("ds_add")
                        || n.starts_with("ds_sub")
                        || n.starts_with("ds_inc")
                        || n.starts_with("ds_dec")
                        || n.starts_with("ds_min")
                        || n.starts_with("ds_max")
                        || n.starts_with("ds_and")
                        || n.starts_with("ds_or")
                        || n.starts_with("ds_xor")
                        || n.starts_with("ds_cmpst")
                        || n.starts_with("ds_mskor"))
                    && !n.ends_with("_u64")
                    && !n.ends_with("_i64")
                    && !n.ends_with("_f64")
                    && !n.ends_with("_b64"))
        },
    },
    Class {
        id: "readlane",
        what: "v_readlane_b32, v_readfirstlane_b32, v_writelane_b32",
        test: |n, _| {
            any_of(
                n,
                &["v_readlane_b32", "v_readfirstlane_b32", "v_writelane_b32"],
            )
        },
    },
    Class {
        id: "permlane",
        what: "v_permlane16_b32, v_permlanex16_b32",
        test: |n, _| n.starts_with("v_permlane"),
    },
    Class {
        id: "dpp",
        what: "VALU instructions in a DPP form (DPP16 and DPP8)",
        test: |_, f| f.dpp,
    },
    Class {
        id: "ds_swizzle_permute",
        what: "ds_swizzle_b32, ds_permute_b32, ds_bpermute_b32 (cross-lane through LDS hardware)",
        test: |n, _| any_of(n, &["ds_swizzle_b32", "ds_permute_b32", "ds_bpermute_b32"]),
    },
    Class {
        id: "saveexec_b64",
        what: "s_*_saveexec_b64 (64-lane exec mask save and update)",
        test: |n, _| n.starts_with("s_") && n.contains("saveexec") && n.ends_with("_b64"),
    },
    Class {
        id: "saveexec_b32",
        what: "s_*_saveexec_b32 (wave32 form, for comparison)",
        test: |n, _| n.starts_with("s_") && n.contains("saveexec") && n.ends_with("_b32"),
    },
    Class {
        id: "s_barrier",
        what: "s_barrier (thread-group barrier)",
        test: |n, _| n == "s_barrier",
    },
    Class {
        id: "lds_access",
        what: "LDS instructions without the GDS bit (ds_*)",
        test: |n, f| n.starts_with("ds_") && !f.gds,
    },
    Class {
        id: "interp",
        what: "pixel parameter interpolation: v_interp_p1*, v_interp_p2*, v_interp_mov_f32",
        test: |n, _| n.starts_with("v_interp_"),
    },
    Class {
        id: "interp_p1",
        what: "v_interp_p1* only",
        test: |n, _| n.starts_with("v_interp_p1"),
    },
    Class {
        id: "interp_p2",
        what: "v_interp_p2* only",
        test: |n, _| n.starts_with("v_interp_p2"),
    },
    Class {
        id: "interp_mov",
        what: "v_interp_mov_f32 (reads one vertex's parameter: P0/P10/P20)",
        test: |n, _| n == "v_interp_mov_f32",
    },
    Class {
        id: "f64",
        what: "64-bit float VALU (names ending _f64)",
        test: |n, _| n.starts_with("v_") && n.ends_with("_f64"),
    },
    Class {
        id: "hints",
        what: "scalar hints: s_nop, s_waitcnt*, s_waitcnt_vscnt, s_inst_prefetch, s_clause, s_sleep, s_code_end, s_icache_inv, s_depctr",
        test: |n, _| {
            n == "s_nop"
                || n.starts_with("s_waitcnt")
                || any_of(
                    n,
                    &[
                        "s_inst_prefetch",
                        "s_clause",
                        "s_sleep",
                        "s_code_end",
                        "s_icache_inv",
                        "s_waitcnt_depctr",
                    ],
                )
        },
    },
];

/// The counts of one program.
struct Program {
    stage: &'static str,
    wave64: bool,
    lds_bytes: u32,
    insts: usize,
    code: String,
    ops: BTreeMap<String, u64>,
    dpp: u64,
    gds: u64,
    /// `s_setreg_*` by target: `<hwreg id>@<offset>+<size>[=<immediate>]`.
    setreg: BTreeMap<String, u64>,
    error: Option<String>,
}

fn stage_name(s: HwStage) -> &'static str {
    match s {
        HwStage::Ls => "ls",
        HwStage::Hs => "hs",
        HwStage::Es => "es",
        HwStage::Gs => "gs",
        HwStage::Vs => "vs",
        HwStage::Ps => "ps",
        HwStage::Ms => "ms",
        HwStage::Cs => "cs",
    }
}

fn program_row(p: &victory_shader::NativeProgram) -> Program {
    let lds_bytes = match &p.io {
        victory_shader::native::StageIo::Cs(c) => c.lds_bytes(),
        victory_shader::native::StageIo::Ms(m) => m.lds_bytes,
        _ => 0,
    };
    let mut row = Program {
        stage: stage_name(p.stage),
        wave64: p.wave64,
        lds_bytes,
        insts: 0,
        code: blake3::hash(&p.code).to_hex()[..16].to_owned(),
        ops: BTreeMap::new(),
        dpp: 0,
        gds: 0,
        setreg: BTreeMap::new(),
        error: None,
    };
    match gfx10::decode(&p.code) {
        Err(e) => row.error = Some(e.to_string()),
        Ok(list) => {
            row.insts = list.len();
            for i in &list {
                let name = i.name().map_or_else(
                    || format!("?{:?}:{:#x}", i.encoding, i.opcode),
                    str::to_owned,
                );
                *row.ops.entry(name).or_default() += 1;
                // The operand fields tell DPP and the GDS bit; an instruction they cannot
                // describe still counts by name.
                let f = std::panic::catch_unwind(|| i.fields(p.wave64));
                if let Ok(Fields::Valu(v)) = &f
                    && matches!(v.dpp, Some(Dpp::Row { .. } | Dpp::Lanes { .. }))
                {
                    row.dpp += 1;
                }
                if let Ok(Fields::Ds { gds: true, .. }) = &f {
                    row.gds += 1;
                }
                // Which register an `s_setreg` writes (hwreg 1 is MODE: float round and
                // denorm modes, IEEE and DX10 clamp bits).
                if let Ok(Fields::Sop {
                    imm: Some(imm),
                    src,
                    ..
                }) = &f
                    && name_is_setreg(i.name())
                {
                    let (id, off, size) = (imm & 0x3f, (imm >> 6) & 0x1f, ((imm >> 11) & 0x1f) + 1);
                    let mut key = format!("hwreg{id}@{off}+{size}");
                    if let Some(victory_shader::gfx10::Operand::Literal(v)) = src.first() {
                        key.push_str(&format!("={v:#x}"));
                    }
                    *row.setreg.entry(key).or_default() += 1;
                }
            }
        }
    }
    row
}

fn name_is_setreg(n: Option<&str>) -> bool {
    matches!(n, Some("s_setreg_b32" | "s_setreg_imm32_b32"))
}

fn program_json(p: &Program) -> Value {
    json!({
        "stage": p.stage, "wave64": p.wave64, "lds_bytes": p.lds_bytes, "insts": p.insts,
        "code": p.code, "ops": p.ops, "dpp": p.dpp, "gds": p.gds, "setreg": p.setreg,
        "error": p.error,
    })
}

pub fn run(args: &[String]) -> Result<(), String> {
    let game = arg(args, "--game")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::project_root().join("GTA6.Xbox.v1.0.2.1"));
    let out = out_dir(args);
    let research = arg(args, "--research")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::project_root().join("research").join("census"));
    if out.starts_with(env!("CARGO_MANIFEST_DIR"))
        || research.starts_with(env!("CARGO_MANIFEST_DIR"))
    {
        return Err("--out and --research must be outside the repository".into());
    }
    let threads = jobs(args);
    let limit: usize = arg(args, "--limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(usize::MAX);
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&research).map_err(|e| e.to_string())?;
    std::panic::set_hook(Box::new(|_| {}));
    let rows_path = out.join("opcodes.jsonl");
    std::fs::write(&rows_path, "").map_err(|e| e.to_string())?;
    let rows = jsonl::Appender::open(&rows_path)?;
    let seen: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
    let taken = AtomicUsize::new(0);
    let started = Instant::now();
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(&game)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && e.metadata().map_or(0, |m| m.len()) <= MAX_FILE)
        .map(|e| e.into_path())
        .collect();
    files.sort();
    println!("opcodes: game {} ({} threads)", game.display(), threads);
    for file in &files {
        let Ok(data) = std::fs::read(file) else {
            continue;
        };
        let jobs = sources(&data);
        if jobs.is_empty() {
            continue;
        }
        let next = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..threads.min(jobs.len()) {
                s.spawn(|| {
                    while let Some(job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                        if taken.load(Ordering::Relaxed) >= limit {
                            return;
                        }
                        let Some(bytes) = job.bytes() else { continue };
                        let hash = blake3::hash(&bytes).to_hex()[..16].to_owned();
                        if !seen
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(hash.clone())
                        {
                            continue;
                        }
                        let n = taken.fetch_add(1, Ordering::Relaxed) + 1;
                        let mut row = json!({"hash": hash, "src": job.label(file)});
                        let programs = Container::parse(&bytes).ok().and_then(|c| {
                            c.part(FourCC::DXIL)
                                .or(c.part(FourCC::ACCC))
                                .map(|p| native_programs(p.data))
                        });
                        match programs {
                            None => row["error"] = json!("no DXIL part (SM5 or older)"),
                            Some(Err(e)) => row["error"] = json!(e.to_string()),
                            Some(Ok(list)) => {
                                let done: Vec<Value> = list
                                    .iter()
                                    .map(|p| {
                                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                                            || program_json(&program_row(p)),
                                        ))
                                        .unwrap_or_else(
                                            |_| json!({"error": "PANIC in the decoder"}),
                                        )
                                    })
                                    .collect();
                                row["programs"] = json!(done);
                            }
                        }
                        rows.push(&row);
                        if n.is_multiple_of(5000) {
                            println!("  {n} containers, {:.0} s", started.elapsed().as_secs_f64());
                        }
                    }
                });
            }
        });
    }
    rows.flush();
    let rows = jsonl::read(&rows_path);
    println!(
        "opcodes: {} containers, {:.0} s",
        rows.len(),
        started.elapsed().as_secs_f64()
    );
    summarize(&rows, &research)
}

/// Totals of one mnemonic or class.
#[derive(Default)]
struct Tally {
    programs: u64,
    unique_code: HashSet<String>,
    count: u64,
    by_stage: BTreeMap<&'static str, u64>,
    samples: Vec<String>,
}

impl Tally {
    fn add(&mut self, hash: &str, p: &Value, n: u64) {
        self.programs += 1;
        self.unique_code
            .insert(p["code"].as_str().unwrap_or("").to_owned());
        self.count += n;
        let stage = stage_static(p["stage"].as_str().unwrap_or("?"));
        *self.by_stage.entry(stage).or_default() += 1;
        if self.samples.len() < 5 && !self.samples.iter().any(|s| s == hash) {
            self.samples.push(hash.to_owned());
        }
    }

    fn json(&self) -> Value {
        json!({
            "programs": self.programs, "unique_code": self.unique_code.len(),
            "instructions": self.count, "by_stage": self.by_stage, "samples": self.samples,
        })
    }
}

fn stage_static(s: &str) -> &'static str {
    ["ls", "hs", "es", "gs", "vs", "ps", "ms", "cs"]
        .into_iter()
        .find(|x| *x == s)
        .unwrap_or("?")
}

/// The report from the rows: `opcodes.json` and `opcodes.md` under `research`.
pub fn summarize(rows: &[Value], research: &std::path::Path) -> Result<(), String> {
    let mut mnemonics: BTreeMap<String, Tally> = BTreeMap::new();
    let mut classes: Vec<Tally> = CLASSES.iter().map(|_| Tally::default()).collect();
    let (mut containers, mut containers_no_programs, mut programs, mut instructions) =
        (0u64, 0u64, 0u64, 0u64);
    let mut decode_errors: BTreeMap<String, u64> = BTreeMap::new();
    let mut stages: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut wave64_programs = 0u64;
    let mut unique_code: HashSet<String> = HashSet::new();
    let mut lds: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut lds_max = (0u32, String::new());
    let mut lds_users = 0u64;
    let mut setregs: BTreeMap<String, Tally> = BTreeMap::new();
    for row in rows {
        containers += 1;
        let hash = row["hash"].as_str().unwrap_or("?");
        let Some(list) = row["programs"].as_array().filter(|l| !l.is_empty()) else {
            containers_no_programs += 1;
            continue;
        };
        for p in list {
            programs += 1;
            unique_code.insert(p["code"].as_str().unwrap_or("").to_owned());
            let stage = p["stage"].as_str().unwrap_or("?").to_owned();
            let e = stages.entry(stage.clone()).or_default();
            e.0 += 1;
            e.1 += p["insts"].as_u64().unwrap_or(0);
            if p["wave64"] == true {
                wave64_programs += 1;
            }
            if let Some(err) = p["error"].as_str() {
                let key = err.split(':').next_back().unwrap_or(err).trim().to_owned();
                *decode_errors.entry(key).or_default() += 1;
                continue;
            }
            instructions += p["insts"].as_u64().unwrap_or(0);
            let bytes = p["lds_bytes"].as_u64().unwrap_or(0) as u32;
            let bucket = match bytes {
                0 => "0",
                1..=1024 => "1..1 KiB",
                1025..=4096 => "1..4 KiB",
                4097..=16384 => "4..16 KiB",
                16385..=32768 => "16..32 KiB",
                _ => "over 32 KiB",
            };
            if stage == "cs" || stage == "ms" {
                *lds.entry(bucket).or_default() += 1;
            }
            if bytes > lds_max.0 {
                lds_max = (bytes, hash.to_owned());
            }
            if bytes > 0 {
                lds_users += 1;
            }
            for (key, n) in p["setreg"].as_object().into_iter().flatten() {
                setregs
                    .entry(key.clone())
                    .or_default()
                    .add(hash, p, n.as_u64().unwrap_or(0));
            }
            let ops = p["ops"].as_object().cloned().unwrap_or_default();
            let (dpp, gds) = (
                p["dpp"].as_u64().unwrap_or(0),
                p["gds"].as_u64().unwrap_or(0),
            );
            for (name, n) in &ops {
                mnemonics
                    .entry(name.clone())
                    .or_default()
                    .add(hash, p, n.as_u64().unwrap_or(0));
            }
            for (class, tally) in CLASSES.iter().zip(&mut classes) {
                // Counts per instruction: by name, plus the DPP and GDS facts. A DS instruction
                // with the GDS bit is told apart only in total (`gds`), not per mnemonic.
                let mut n: u64 = ops
                    .iter()
                    .filter(|(name, _)| (class.test)(name, &Facts::default()))
                    .map(|(_, v)| v.as_u64().unwrap_or(0))
                    .sum();
                match class.id {
                    "dpp" => n = dpp,
                    "gds_ordered" => n += gds,
                    "lds_access" => n = n.saturating_sub(gds),
                    _ => {}
                }
                if n > 0 {
                    tally.add(hash, p, n);
                }
            }
        }
    }
    let class_json: Vec<Value> = CLASSES
        .iter()
        .zip(&classes)
        .map(|(c, t)| {
            let mut v = t.json();
            v["id"] = json!(c.id);
            v["what"] = json!(c.what);
            v
        })
        .collect();
    let mnemonic_json: BTreeMap<&String, Value> =
        mnemonics.iter().map(|(k, v)| (k, v.json())).collect();
    let summary = json!({
        "date": "2026-11-27",
        "containers": containers, "containers_without_native_programs": containers_no_programs,
        "programs": programs, "unique_code": unique_code.len(), "wave64_programs": wave64_programs,
        "instructions": instructions, "decode_errors": decode_errors,
        "stages": stages.iter().map(|(k, v)| (k.clone(), json!({"programs": v.0, "instructions": v.1}))).collect::<BTreeMap<_, _>>(),
        "lds": {"cs_ms_programs_by_size": lds, "programs_with_lds": lds_users, "max_bytes": lds_max.0, "max_sample": lds_max.1},
        "setreg": setregs.iter().map(|(k, v)| (k.clone(), v.json())).collect::<BTreeMap<_, _>>(),
        "classes": class_json,
        "mnemonics": mnemonic_json,
    });
    std::fs::write(
        research.join("opcodes.json"),
        serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(
        research.join("opcodes.md"),
        markdown(&summary, &mnemonics, &setregs),
    )
    .map_err(|e| e.to_string())?;
    println!("opcodes: wrote {}", research.join("opcodes.json").display());
    Ok(())
}

fn markdown(
    s: &Value,
    mnemonics: &BTreeMap<String, Tally>,
    setregs: &BTreeMap<String, Tally>,
) -> String {
    let mut o = String::new();
    o.push_str("# GFX10 instruction census (opcodes), 2026-11-27\n\n");
    o.push_str("Made by `victory-census opcodes` (metal/census, branch `gaps`): every unique shader container of the game package (BLAKE3 hash as the game log names it), its native programs through `victory_shader::native_programs`, their instructions through `victory_shader::gfx10::decode` (reachable instructions from offset 0), counted by AMD mnemonic. Counts are per *program* (a container may hold several), 'unique code' counts programs with distinct code bytes, 'instructions' is the decoded instruction count. Hashes only, no game names.\n\n");
    o.push_str(&format!(
        "{} unique containers, {} without native programs (SM5-era or empty), {} programs ({} unique code, {} wave64), {} decoded instructions, decode failures: {}.\n\n",
        s["containers"], s["containers_without_native_programs"], s["programs"], s["unique_code"], s["wave64_programs"], s["instructions"], s["decode_errors"]
    ));
    o.push_str("Programs by hardware stage: ");
    if let Some(m) = s["stages"].as_object() {
        let parts: Vec<String> = m
            .iter()
            .map(|(k, v)| format!("{k} {} ({} instructions)", v["programs"], v["instructions"]))
            .collect();
        o.push_str(&parts.join(", "));
    }
    o.push_str("\n\n## The classes asked about\n\n| class | what | programs | unique code | instructions | by stage | sample containers |\n|---|---|---:|---:|---:|---|---|\n");
    for c in s["classes"].as_array().into_iter().flatten() {
        let stages: Vec<String> = c["by_stage"]
            .as_object()
            .map(|m| m.iter().map(|(k, v)| format!("{k} {v}")).collect())
            .unwrap_or_default();
        let samples: Vec<String> = c["samples"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|h| format!("`{h}`")))
                    .collect()
            })
            .unwrap_or_default();
        o.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            c["id"].as_str().unwrap_or(""),
            c["what"].as_str().unwrap_or(""),
            c["programs"],
            c["unique_code"],
            c["instructions"],
            stages.join(", "),
            samples.join(" ")
        ));
    }
    o.push_str(&format!(
        "\n## LDS\n\nCompute and mesh programs by LDS bytes per thread group (from the program's registers): {}. Programs with LDS > 0: {}. Largest: {} bytes (`{}`).\n",
        s["lds"]["cs_ms_programs_by_size"], s["lds"]["programs_with_lds"], s["lds"]["max_bytes"], s["lds"]["max_sample"].as_str().unwrap_or("")
    ));
    o.push_str("\n## s_setreg targets\n\n`hwreg<id>@<offset>+<size>[=<immediate>]` (hwreg 1 is MODE: bits 0-3 float round modes, 4-7 denorm modes, 9 DX10 clamp, 10 IEEE).\n\n| target | programs | instructions |\n|---|---:|---:|\n");
    let mut regs: Vec<(&String, &Tally)> = setregs.iter().collect();
    regs.sort_by(|a, b| b.1.programs.cmp(&a.1.programs).then(a.0.cmp(b.0)));
    for (k, t) in regs.iter().take(40) {
        o.push_str(&format!("| {k} | {} | {} |\n", t.programs, t.count));
    }
    o.push_str("\n## Every mnemonic, by number of programs\n\n| mnemonic | programs | unique code | instructions |\n|---|---:|---:|---:|\n");
    let mut all: Vec<(&String, &Tally)> = mnemonics.iter().collect();
    all.sort_by(|a, b| b.1.programs.cmp(&a.1.programs).then(a.0.cmp(b.0)));
    for (k, t) in all {
        o.push_str(&format!(
            "| {k} | {} | {} | {} |\n",
            t.programs,
            t.unique_code.len(),
            t.count
        ));
    }
    o
}
