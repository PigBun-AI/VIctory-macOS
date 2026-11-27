//! Stage 1: every shader container of the game package through the shared
//! translator (`victory_shader::emit::translate`), natively on arm64.
//!
//! The scan is the one `victory translate` makes (apps/victory `shader.rs`):
//! every file of the game directory up to 2 GiB, SGD5 records unpacked and
//! raw `DXBC` containers found by magic. Each container is translated once
//! (by the hash a run's log names it by: the first 16 hex digits of the
//! BLAKE3 of the container). The result is written under `--out`:
//!
//! * `translate.jsonl`: one row per container (hash, code hash, source,
//!   programs, result, error text, milliseconds, DXIL size)
//! * `vsh/<hash>.vsh`: the translation in the shader cache's byte form
//!   (`victory_shader::emit::wire`, VSH1) for the containers that translate

use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use serde_json::{Value, json};
use victory_game::sgd5;
use victory_shader::emit::wire;
use victory_shader::{Container, FourCC, native_programs};

use crate::{arg, defaults, jobs, jsonl, out_dir};

/// Files above this size are skipped, as `victory translate` does.
const MAX_FILE: u64 = 2 << 30;

enum Source<'a> {
    Raw { offset: usize, data: &'a [u8] },
    Packed(sgd5::Record<'a>),
}

impl Source<'_> {
    fn bytes(&self) -> Option<std::borrow::Cow<'_, [u8]>> {
        match self {
            Source::Raw { data, .. } => Some(std::borrow::Cow::Borrowed(data)),
            Source::Packed(r) => r.unpack().map(std::borrow::Cow::Owned),
        }
    }

    /// Where it was found (the record's name is the game's own: it stays in
    /// the private rows, never in a report).
    fn label(&self, file: &Path) -> String {
        let name = file
            .file_name()
            .map_or(String::new(), |n| n.to_string_lossy().into_owned());
        match self {
            Source::Raw { offset, .. } => format!("{name}@{offset:#x}"),
            Source::Packed(r) => format!("{name}@{:#x} {}", r.offset, r.name.unwrap_or("?")),
        }
    }
}

fn sources(data: &[u8]) -> Vec<Source<'_>> {
    let mut out: Vec<Source<'_>> = Vec::new();
    if sgd5::is_sgd5(data) {
        out.extend(sgd5::records(data).map(Source::Packed));
    }
    out.extend(
        memchr::memmem::find_iter(data, b"DXBC")
            .filter(|&offset| Container::parse(&data[offset..]).is_ok())
            .map(|offset| Source::Raw {
                offset,
                data: &data[offset..],
            }),
    );
    out
}

/// A panic's message, for the row.
fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into())
}

/// Translates one container and describes the outcome as a row; the
/// translation itself is returned for the caller to store.
pub fn translate_one(
    bytes: &[u8],
    hash: &str,
    label: &str,
) -> (Value, Option<victory_shader::emit::HostShader>) {
    let mut row = json!({"hash": hash, "src": label, "container_bytes": bytes.len()});
    let programs = Container::parse(bytes).ok().and_then(|c| {
        c.part(FourCC::DXIL).or(c.part(FourCC::ACCC)).map(|p| {
            (
                blake3::hash(p.data).to_hex()[..8].to_owned(),
                native_programs(p.data),
            )
        })
    });
    match programs {
        None => {
            row["programs"] = json!([]);
            row["ok"] = json!(false);
            row["error"] = json!("no DXIL part (SM5 or older)");
            return (row, None);
        }
        Some((code8, Ok(list))) => {
            row["code8"] = json!(code8);
            row["programs"] = json!(
                list.iter()
                    .map(|p| format!("{:?}", p.stage))
                    .collect::<Vec<_>>()
            );
            row["wave64"] = json!(list.iter().any(|p| p.wave64));
        }
        Some((code8, Err(e))) => {
            row["code8"] = json!(code8);
            row["programs"] = json!([]);
            row["ok"] = json!(false);
            row["error"] = json!(e.to_string());
            return (row, None);
        }
    }
    let started = Instant::now();
    let done = catch_unwind(AssertUnwindSafe(|| victory_shader::emit::translate(bytes)));
    row["ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
    match done {
        Ok(Ok(shader)) => {
            row["ok"] = json!(true);
            row["stage"] = json!(format!("{:?}", shader.stage));
            row["dxil_bytes"] = json!(shader.dxil.len());
            (row, Some(shader))
        }
        Ok(Err(e)) => {
            row["ok"] = json!(false);
            row["error"] = json!(e.to_string());
            (row, None)
        }
        Err(p) => {
            row["ok"] = json!(false);
            row["panic"] = json!(true);
            row["error"] = json!(format!("PANIC: {}", panic_text(&*p)));
            (row, None)
        }
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let game = arg(args, "--game")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::project_root().join("GTA6.Xbox.v1.0.2.1"));
    let out = out_dir(args);
    let threads = jobs(args);
    let limit: usize = arg(args, "--limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(usize::MAX);
    let wave: u32 = arg(args, "--host-wave")
        .and_then(|v| v.parse().ok())
        .unwrap_or(defaults::HOST_WAVE);
    // Game-derived output must stay outside the repository.
    if out.starts_with(env!("CARGO_MANIFEST_DIR")) {
        return Err(format!("--out {} is inside the repository", out.display()));
    }
    std::fs::create_dir_all(out.join("vsh")).map_err(|e| e.to_string())?;
    // The host the translation is for (what `victory-gpu` tells the translator).
    victory_shader::emit::set_host_max_wave(wave);
    victory_shader::emit::set_view_heap_limit(defaults::VIEW_LIMIT);
    victory_shader::emit::set_raw_views(defaults::RAW_TWINS);
    victory_shader::emit::set_host_rdna(false);
    // Panics are rows, not noise.
    std::panic::set_hook(Box::new(|_| {}));

    let rows_path = out.join("translate.jsonl");
    let done = jsonl::seen(&rows_path, "hash");
    let rows = jsonl::Appender::open(&rows_path)?;
    let seen = Mutex::new(done.clone());
    let taken = AtomicUsize::new(0);
    let started = Instant::now();
    println!(
        "translate: game {} -> {} ({} threads, host wave {wave}, {} done already)",
        game.display(),
        out.display(),
        threads,
        done.len()
    );
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(&game)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && e.metadata().map_or(0, |m| m.len()) <= MAX_FILE)
        .map(|e| e.into_path())
        .collect();
    files.sort();
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
                        let Some(bytes) = job.bytes() else {
                            rows.push(
                                &json!({"hash": "corrupt", "src": job.label(file), "ok": false,
                                "programs": [], "error": "corrupt record"}),
                            );
                            continue;
                        };
                        let hash = blake3::hash(&bytes).to_hex()[..16].to_owned();
                        if !seen
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(hash.clone())
                        {
                            continue;
                        }
                        let n = taken.fetch_add(1, Ordering::Relaxed) + 1;
                        let (row, shader) = translate_one(&bytes, &hash, &job.label(file));
                        if let Some(shader) = shader {
                            let _ = std::fs::write(
                                out.join("vsh").join(format!("{hash}.vsh")),
                                wire::encode(&shader),
                            );
                        }
                        rows.push(&row);
                        if n.is_multiple_of(2000) {
                            println!("  {n} translated, {:.0} s", started.elapsed().as_secs_f64());
                        }
                    }
                });
            }
        });
    }
    rows.flush();
    let rows = jsonl::read(&rows_path);
    let ok = rows.iter().filter(|r| r["ok"] == true).count();
    let hashes: HashSet<&str> = rows.iter().filter_map(|r| r["hash"].as_str()).collect();
    println!(
        "translate: {} containers ({} unique), {ok} translated, {} failed, {:.0} s",
        rows.len(),
        hashes.len(),
        rows.len() - ok,
        started.elapsed().as_secs_f64()
    );
    // The settings and the scan, for the report.
    let meta = json!({
        "game_dir": game.display().to_string(),
        "host_wave": wave,
        "view_limit": defaults::VIEW_LIMIT,
        "raw_views": defaults::RAW_TWINS,
        "rdna": false,
        "date": "2026-11-27",
    });
    std::fs::write(out.join("translate-meta.json"), meta.to_string()).map_err(|e| e.to_string())?;
    Ok(())
}
