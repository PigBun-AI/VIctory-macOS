//! The console device's two pipeline-cache methods, slots 0x1c8 and 0x218
//! (docs/mac/PATCHES.md, patch G9).
//!
//! What the title does with them (decoded from `game_scarlett_master.exe`
//! 1.0.2.1; the call sites are title+0x2477578 and title+0x24800d5):
//!
//! * Both are **cache hits for a pipeline the title can also make from the
//!   shader containers it loads**. The title keeps a pipeline cache object
//!   (global at title+0x75ddb90, filled from a file at startup). Its graphics
//!   half is warmed in a loop over cache entries, each entry one call of slot
//!   0x1c8 (`out` goes into a table keyed by the first shader's key); a
//!   failed call just leaves the entry out. When the title needs a graphics
//!   pipeline it asks the table first (title+0x2479f40), and on a miss builds
//!   the stream and calls slot 0x2b0 (title+0x2479a87). The compute half is
//!   one call of slot 0x218 per compute pipeline job (title+0x24800d5),
//!   keyed by the job's key; when it fails (returns 0) the job calls slot
//!   0x1f0 with the compute shader container it has loaded
//!   (title+0x247fe66). So a NOTIMPL here is a cache miss, not a lost draw:
//!   the pipelines come from 0x2b0 and 0x1f0, which are written.
//! * The description of both starts like PC's `D3D12_*_PIPELINE_STATE_DESC`
//!   and the plain methods 0x50/0x58 do: the root signature, then (pointer,
//!   size) pairs, but the pairs are not shader bytecode. The cache's records
//!   are what the console compiler wrote next to the native code
//!   (`victory_shader::native`'s `FSPX` entries), and the code itself, in GPU
//!   memory the title placed it in:
//!
//!   ```text
//!   slot 0x218 (compute), a record of 12 qwords:
//!     +0x00 ID3D12RootSignature* (ours: the title writes it into the record
//!           when it calls; the last record's slot is 0 until then)
//!     +0x08 pointer, +0x10 size 0x1c    FSPX entry 0, pipeline registers
//!     +0x18 pointer, +0x20 size 0x14    FSPX entry 1, pipeline metadata
//!     +0x28 GPU address, +0x30 size     FSPX entry 6, the GFX10 code
//!     +0x38 pointer, +0x40 size 0x18+32n FSPX entry 7, program metadata
//!     +0x48, +0x50 two pointers (not decoded), +0x58 a 64-bit hash
//!   slot 0x1c8 (graphics): the root signature, then at least six pairs;
//!     sizes seen: 0x184/0x18c, 0x78-0xbc, code (0x140-0x4a40, GPU memory),
//!     0x70-0x190, (code), 0x30-0x2f0 ... Not decoded: it needs samples.
//!   ```
//!
//! The evidence for the compute record is the sizes (0x1c bytes is `FSPX`
//! entry 0 as `victory_shader::native::fspx_program` reads it; the metadata
//! sizes in the 12 logged samples, 0x118, 0x138, 0x158, 0x198, 0x258, 0x298,
//! 0x438..., are all 0x18 plus a multiple of 32, which is that part's
//! layout) and the code sitting in GPU memory in 256-byte steps. It is read
//! from the log of one run, not from a sample of the whole record, so the
//! method is written to refuse anything that does not parse and to fall back
//! to NOTIMPL: the title then makes the pipeline from its container as it
//! always did.
//!
//! Two switches, both off by default:
//!
//! * `VICTORY_MAC_PSO_X=1`: slot 0x218 makes the pipeline from the record
//!   (a container with one `ACCC` part, which is where the shared layer
//!   already keeps compute programs of this form; slot 0x1c8 stays
//!   NOTIMPL on purpose: its containers would lack the signature parts the
//!   graphics translation reads, and the title's fallback is complete).
//! * `VICTORY_MAC_DUMP_NOTIMPL_PSO=<dir>`: each call of either slot writes
//!   what the title passed (see [`dump`]) so that the records can be read
//!   from a real run. `_BYTES`, `_MAX` and `_FOLLOW` tune it.

#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use crate::object::{HRESULT, Iid, This};

/// Memory the title wrote (its own heap, or GPU memory it placed code in).
pub(crate) trait Memory {
    /// `len` bytes at `at`, or None if any of them is not readable.
    fn read(&self, at: u64, len: usize) -> Option<Vec<u8>>;
}

/// The title's memory in this process, read the way the rest of the shared
/// layer reads what a title pointer may no longer back: through the kernel.
pub(crate) struct Title;

impl Memory for Title {
    fn read(&self, at: u64, len: usize) -> Option<Vec<u8>> {
        if at < 0x10000 || len == 0 || at.checked_add(len as u64)? > 0x8000_0000_0000 {
            return None;
        }
        #[cfg(windows)]
        {
            let mut bytes = vec![0u8; len];
            crate::host::read_into(at as *const u8, &mut bytes).then_some(bytes)
        }
        #[cfg(not(windows))]
        {
            None
        }
    }
}

fn word(bytes: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().expect("8 bytes"))
}

fn switch(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| !matches!(v.trim(), "" | "0" | "off" | "false"))
}

// ---------------------------------------------------------------------------
// Slot 0x218: a compute pipeline from the cache's record.
// ---------------------------------------------------------------------------

/// `VICTORY_MAC_PSO_X`: whether slot 0x218 makes pipelines. Read once.
pub(crate) fn enabled() -> bool {
    #[cfg(test)]
    if FORCED.load(Relaxed) {
        return true;
    }
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| switch("VICTORY_MAC_PSO_X"))
}

#[cfg(test)]
static FORCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Qwords of a compute record.
const COMPUTE_WORDS: usize = 12;
/// `FSPX` entry kinds (`victory_shader::native`).
const KIND_PACKET: u32 = 0;
const KIND_HASH: u32 = 1;
const KIND_CODE: u32 = 6;
const KIND_META: u32 = 7;
/// What `fspx_program` reads of the packet: up to RSRC2 at +0x18.
const PACKET_MIN: u64 = 0x1c;
const META_HEADER: u64 = 0x18;
const MAX_CODE: u64 = 4 << 20;
const MAX_META: u64 = 1 << 20;
const MAX_SMALL: u64 = 0x1000;

/// Why a record was not taken (counted, and logged once each).
type Why = String;

/// A compute record's four parts, read.
pub(crate) struct ComputeParts {
    pub(crate) packet: Vec<u8>,
    pub(crate) hash: Vec<u8>,
    pub(crate) code: Vec<u8>,
    pub(crate) meta: Vec<u8>,
}

fn span(words: &[u8], i: usize, max: u64, what: &str, min: u64) -> Result<(u64, usize), Why> {
    let (at, len) = (word(words, i), word(words, i + 1));
    if at < 0x10000 || len < min || len > max {
        return Err(format!("{what}: pointer {at:#x}, size {len:#x} not as expected"));
    }
    Ok((at, len as usize))
}

/// The parts of the compute record at `desc`.
pub(crate) fn read_compute(mem: &dyn Memory, desc: u64) -> Result<ComputeParts, Why> {
    let words = mem
        .read(desc, COMPUTE_WORDS * 8)
        .ok_or_else(|| "record unreadable".to_owned())?;
    let part = |i: usize, max: u64, min: u64, what: &str| -> Result<Vec<u8>, Why> {
        let (at, len) = span(&words, i, max, what, min)?;
        mem.read(at, len)
            .ok_or_else(|| format!("{what} unreadable at {at:#x}+{len:#x}"))
    };
    let code_len = word(&words, 6);
    if code_len % 4 != 0 {
        return Err(format!("code size {code_len:#x} is not whole dwords"));
    }
    Ok(ComputeParts {
        packet: part(1, MAX_SMALL, PACKET_MIN, "packet")?,
        hash: part(3, MAX_SMALL, 1, "hash")?,
        code: part(5, MAX_CODE, 4, "code")?,
        meta: part(7, MAX_META, META_HEADER, "metadata")?,
    })
}

/// `FSPX` bytes from the four entries, in the order the compiler writes
/// them (offsets from the start of the part, 16-byte aligned).
pub(crate) fn fspx(p: &ComputeParts) -> Vec<u8> {
    let entries = [
        (KIND_PACKET, &p.packet),
        (KIND_HASH, &p.hash),
        (KIND_CODE, &p.code),
        (KIND_META, &p.meta),
    ];
    let table = 0x10 + 16 * entries.len();
    let mut at = table.next_multiple_of(16);
    let mut out = vec![0u8; at];
    out[0..4].copy_from_slice(b"FSPX");
    out[4..8].copy_from_slice(&1u32.to_le_bytes());
    out[8..12].copy_from_slice(&(entries.len() as u32).to_le_bytes());
    for (i, (kind, data)) in entries.iter().enumerate() {
        let e = 0x10 + 16 * i;
        out[e..e + 4].copy_from_slice(&kind.to_le_bytes());
        out[e + 4..e + 8].copy_from_slice(&(at as u32).to_le_bytes());
        out[e + 8..e + 12].copy_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        at += data.len();
        let padded = at.next_multiple_of(16);
        out.resize(padded, 0);
        at = padded;
    }
    let total = out.len() as u32;
    out[12..16].copy_from_slice(&total.to_le_bytes());
    out
}

/// A DXBC container with one part (the shape the shared layer parses for
/// the console's other containers; `Container::parse`).
pub(crate) fn container(fourcc: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let at = 0x24usize;
    let total = (at + 8 + data.len()).next_multiple_of(4);
    let mut out = vec![0u8; total];
    out[0..4].copy_from_slice(b"DXBC");
    out[4..20].copy_from_slice(&blake3::hash(data).as_bytes()[..16]);
    out[0x14..0x16].copy_from_slice(&1u16.to_le_bytes());
    out[0x18..0x1c].copy_from_slice(&(total as u32).to_le_bytes());
    out[0x1c..0x20].copy_from_slice(&1u32.to_le_bytes());
    out[0x20..0x24].copy_from_slice(&(at as u32).to_le_bytes());
    out[at..at + 4].copy_from_slice(fourcc);
    out[at + 4..at + 8].copy_from_slice(&(data.len() as u32).to_le_bytes());
    out[at + 8..at + 8 + data.len()].copy_from_slice(data);
    out
}

/// The compute container for the record at `desc`, checked: the programs in
/// it parse (what `Shader::translated` would find out on the first dispatch,
/// where it is too late to fall back), and it is one compute program.
pub(crate) fn compute_container(mem: &dyn Memory, desc: u64) -> Result<Vec<u8>, Why> {
    let parts = read_compute(mem, desc)?;
    let part = fspx(&parts);
    let programs = victory_shader::native_programs(&part).map_err(|e| format!("fspx: {e}"))?;
    match programs.as_slice() {
        [p] if p.stage == victory_shader::HwStage::Cs => {}
        other => return Err(format!("fspx: {} programs, not one compute program", other.len())),
    }
    Ok(container(b"ACCC", &part))
}

static MADE: AtomicU64 = AtomicU64::new(0);
static REFUSED: AtomicU64 = AtomicU64::new(0);
static WHY: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

fn refused(why: Why) {
    let n = REFUSED.fetch_add(1, Relaxed) + 1;
    let mut all = WHY.lock().unwrap_or_else(|e| e.into_inner());
    let count = all.get_or_insert_with(HashMap::new).entry(why.clone()).or_default();
    *count += 1;
    if *count == 1 {
        tracing::warn!(
            why,
            refused = n,
            made = MADE.load(Relaxed),
            "mac G9: slot 0x218 record not taken; the title makes this pipeline from its container (E_NOTIMPL)"
        );
    }
}

/// Slot 0x218's pipeline from the record at `desc`. `None`: not made (the
/// switch is off or the record does not parse); the caller answers NOTIMPL
/// and counts it, and the title goes on with its container.
pub(crate) fn create_compute(desc: *const u8, iid: *const Iid, out: *mut This) -> Option<HRESULT> {
    if !enabled() || out.is_null() {
        return None;
    }
    match compute_container(&Title, desc as u64) {
        Ok(c) => {
            let n = MADE.fetch_add(1, Relaxed) + 1;
            if n.is_power_of_two() {
                tracing::info!(made = n, refused = REFUSED.load(Relaxed), "mac G9: slot 0x218 pipelines made from the cache's records");
            }
            crate::mac::drop_stats::creating("compute@0x218");
            Some(crate::pipeline::compute_from_container(&c, iid, out))
        }
        Err(why) => {
            refused(why);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// The dump: what the title passes, for both slots.
// ---------------------------------------------------------------------------

/// Largest (pointer, size) pair followed, bytes.
const FOLLOW_MAX: u64 = 256 << 10;
/// A lone pointer's peek, bytes.
const PEEK: usize = 0x80;
/// The root signature object's bytes read.
const OBJECT: usize = 0x80;

struct Dumper {
    dir: PathBuf,
    /// Bytes of the description read (a multiple of 8).
    bytes: usize,
    follow: u64,
    max: u64,
    start: Instant,
    calls: [AtomicU64; 2],
    written: AtomicU64,
    seen: Mutex<HashSet<[u8; 16]>>,
    blobs: Mutex<HashSet<[u8; 16]>>,
}

fn dumper() -> Option<&'static Dumper> {
    static DUMPER: OnceLock<Option<Dumper>> = OnceLock::new();
    DUMPER
        .get_or_init(|| {
            let dir = PathBuf::from(std::env::var_os("VICTORY_MAC_DUMP_NOTIMPL_PSO").filter(|v| !v.is_empty())?);
            let num = |name: &str, default: u64| {
                std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
            };
            let d = Dumper::new(
                dir,
                num("VICTORY_MAC_DUMP_NOTIMPL_PSO_BYTES", 256).clamp(64, 4096) as usize / 8 * 8,
                num("VICTORY_MAC_DUMP_NOTIMPL_PSO_FOLLOW", FOLLOW_MAX).clamp(16, 4 << 20),
                num("VICTORY_MAC_DUMP_NOTIMPL_PSO_MAX", 60_000),
            );
            match d.prepare() {
                Ok(()) => {
                    tracing::info!(dir = %d.dir.display(), bytes = d.bytes, "mac G9: dumping the descriptions of slots 0x1c8 and 0x218");
                    Some(d)
                }
                Err(e) => {
                    tracing::warn!(dir = %d.dir.display(), error = %e, "mac G9: the dump directory cannot be made; nothing is dumped");
                    None
                }
            }
        })
        .as_ref()
}

/// A call of slot `slot` (0x1c8 or 0x218) with the description at `desc`.
/// With `VICTORY_MAC_DUMP_NOTIMPL_PSO` unset: one load.
pub(crate) fn dump(slot: usize, desc: *const u8) {
    if let Some(d) = dumper() {
        d.sample(&Title, slot, desc as u64);
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn short(hash: &blake3::Hash) -> [u8; 16] {
    hash.as_bytes()[..16].try_into().expect("16 bytes")
}

impl Dumper {
    fn new(dir: PathBuf, bytes: usize, follow: u64, max: u64) -> Self {
        Dumper {
            dir,
            bytes,
            follow,
            max,
            start: Instant::now(),
            calls: Default::default(),
            written: AtomicU64::new(0),
            seen: Mutex::default(),
            blobs: Mutex::default(),
        }
    }

    fn prepare(&self) -> std::io::Result<()> {
        for sub in ["0x1c8", "0x218", "blobs"] {
            std::fs::create_dir_all(self.dir.join(sub))?;
        }
        std::fs::write(self.dir.join("README.txt"), README)
    }

    /// Reads as much of the description as is there: the whole, else halves.
    fn description(&self, mem: &dyn Memory, at: u64) -> Option<Vec<u8>> {
        let mut len = self.bytes;
        while len >= 64 {
            if let Some(b) = mem.read(at, len) {
                return Some(b);
            }
            len /= 2;
        }
        None
    }

    /// Whether `v` could be a pointer into the title's memory.
    fn pointer(v: u64) -> bool {
        (0x10000..0x8000_0000_0000).contains(&v)
    }

    /// Stores a blob once, by content; its name.
    fn blob(&self, bytes: &[u8]) -> String {
        let hash = blake3::hash(bytes);
        let key = short(&hash);
        let name = hex(&key);
        if self.blobs.lock().unwrap_or_else(|e| e.into_inner()).insert(key) {
            let _ = std::fs::write(self.dir.join("blobs").join(format!("{name}.bin")), bytes);
        }
        name
    }

    /// One call: counts it, and, if its content is new, writes it. What it
    /// follows: the root signature object (the first qword) for its own
    /// pairs, then each (pointer, size) pair of the description (a pointer
    /// followed by a plausible size), then a peek at each lone pointer.
    /// The name is a hash of everything but the pointer values and the
    /// root signature, so the same pipeline of another run, or in another
    /// place, is the same file.
    fn sample(&self, mem: &dyn Memory, slot: usize, desc: u64) {
        let which = usize::from(slot == 0x218);
        let call = self.calls[which].fetch_add(1, Relaxed) + 1;
        let Some(bytes) = self.description(mem, desc) else {
            tracing::warn!(slot = format_args!("{slot:#x}"), call, desc = format_args!("{desc:#x}"), "mac G9: description unreadable");
            return;
        };
        let words = bytes.len() / 8;
        let mut text = String::new();
        let mut finger = blake3::Hasher::new();
        finger.update(&(slot as u64).to_le_bytes());
        let _ = writeln!(text, "slot {slot:#x}\ncall {call}\nms {}\nbytes {}", self.start.elapsed().as_millis(), bytes.len());
        let mut used = vec![false; words];
        let mut notes: Vec<String> = vec![String::new(); words];
        // Word 0 is an object of ours (the title's pRootSignature); only its pairs are of use.
        let root = word(&bytes, 0);
        used[0] = true;
        notes[0] = "root signature object".to_owned();
        if let Some(object) = mem.read(root, OBJECT) {
            let _ = writeln!(text, "root-object {}", hex(&object));
            for line in self.pairs(mem, &object, &mut finger) {
                let _ = writeln!(text, "root-object {line}");
            }
        } else {
            let _ = writeln!(text, "root-object unreadable");
        }
        // Pairs, then lone pointers.
        let mut i = 1;
        while i < words {
            let (v, next) = (word(&bytes, i), if i + 1 < words { word(&bytes, i + 1) } else { 0 });
            if Self::pointer(v) && (1..=self.follow).contains(&next) && i + 1 < words {
                match mem.read(v, next as usize) {
                    Some(content) => {
                        let name = self.blob(&content);
                        finger.update(b"P").update(&content);
                        notes[i] = format!("pointer, size in w{:02} = {next:#x}, blob {name}", i + 1);
                    }
                    None => {
                        finger.update(b"U").update(&next.to_le_bytes());
                        notes[i] = format!("pointer, size in w{:02} = {next:#x}, UNREADABLE", i + 1);
                    }
                }
                notes[i + 1] = format!("size of w{i:02}");
                used[i] = true;
                used[i + 1] = true;
                i += 2;
            } else {
                i += 1;
            }
        }
        for i in 1..words {
            let v = word(&bytes, i);
            if used[i] {
                continue;
            }
            if Self::pointer(v)
                && let Some(content) = mem.read(v, PEEK)
            {
                let name = self.blob(&content);
                finger.update(b"K").update(&content);
                notes[i] = format!("lone pointer, first {PEEK:#x} bytes: blob {name}");
            } else {
                finger.update(b"V").update(&v.to_le_bytes());
            }
        }
        for i in 0..words {
            let _ = writeln!(text, "w{i:02} {:016x} {}", word(&bytes, i), notes[i]);
        }
        let _ = writeln!(text, "raw {}", hex(&bytes));
        let key: [u8; 16] = finger.finalize().as_bytes()[..16].try_into().expect("16 bytes");
        if !self.seen.lock().unwrap_or_else(|e| e.into_inner()).insert(key) {
            return;
        }
        if self.written.fetch_add(1, Relaxed) >= self.max {
            return;
        }
        let path = self.dir.join(format!("{slot:#x}")).join(format!("{}.txt", hex(&key)));
        if let Err(e) = std::fs::write(&path, text) {
            tracing::warn!(error = %e, "mac G9: a description cannot be written");
        }
        if call.is_power_of_two() {
            tracing::info!(
                slot = format_args!("{slot:#x}"),
                calls = call,
                files = self.written.load(Relaxed),
                blobs = self.blobs.lock().unwrap_or_else(|e| e.into_inner()).len(),
                "mac G9: dumped"
            );
        }
    }

    /// The (pointer, size) pairs inside `bytes` (the root signature object):
    /// each blob is stored and enters the name.
    fn pairs(&self, mem: &dyn Memory, bytes: &[u8], finger: &mut blake3::Hasher) -> Vec<String> {
        let words = bytes.len() / 8;
        let mut out = Vec::new();
        let mut i = 0;
        while i + 1 < words {
            let (v, n) = (word(bytes, i), word(bytes, i + 1));
            if Self::pointer(v) && (1..=self.follow).contains(&n)
                && let Some(content) = mem.read(v, n as usize)
            {
                let name = self.blob(&content);
                finger.update(b"R").update(&content);
                out.push(format!("w{i:02} pointer, size {n:#x}: blob {name}"));
                i += 2;
            } else {
                i += 1;
            }
        }
        out
    }
}

const README: &str = "\
Descriptions the title passed to the console device's pipeline cache methods
(0x1c8 graphics, 0x218 compute), VICTORY_MAC_DUMP_NOTIMPL_PSO (docs/mac/PATCHES.md, G9).

  0x1c8/<id>.txt, 0x218/<id>.txt   one file per different call (id: a hash of the
                                   description's non-pointer words and of the memory
                                   its (pointer, size) pairs name)
  blobs/<id>.bin                   that memory, once per content

Each file: slot, call number, ms since the first call, then 'root-object' lines
(our root signature object: its bytes and the blobs its pairs name), then one
line per qword w00.. (value, what it was taken for) and the raw bytes.
Pairs are a pointer followed by a size of 1..FOLLOW bytes; a pointer that is
not part of a pair is shown by its first 0x80 bytes ('lone pointer').
";

#[cfg(test)]
mod tests {
    use super::*;
    use victory_shader::{Container, FourCC, HwStage};

    /// A few ranges of "title memory".
    #[derive(Default)]
    struct Fake(Vec<(u64, Vec<u8>)>);

    impl Fake {
        fn put(&mut self, at: u64, bytes: Vec<u8>) {
            self.0.push((at, bytes));
        }
        fn words(&mut self, at: u64, w: &[u64]) {
            self.put(at, w.iter().flat_map(|v| v.to_le_bytes()).collect());
        }
    }

    impl Memory for Fake {
        fn read(&self, at: u64, len: usize) -> Option<Vec<u8>> {
            self.0.iter().find_map(|(base, b)| {
                let off = at.checked_sub(*base)? as usize;
                b.get(off..off + len).map(<[u8]>::to_vec)
            })
        }
    }

    /// An `FSPX` compute record's entries as the compiler lays them out:
    /// 64 x 1 x 1 threads, wave64, one 0x41 resource record and the hash
    /// record that ends the metadata.
    fn parts(code_dwords: usize) -> ComputeParts {
        let mut packet = vec![0u8; 0x1c];
        for (i, n) in [64u16, 1, 1].into_iter().enumerate() {
            packet[0x0c + 2 * i..0x0e + 2 * i].copy_from_slice(&n.to_le_bytes());
        }
        packet[0x14..0x18].copy_from_slice(&0x0000_0003u32.to_le_bytes());
        let mut meta = vec![0u8; 0x18 + 2 * 32];
        meta[0..4].copy_from_slice(&0x403u32.to_le_bytes());
        let size = meta.len() as u32 - 0x14;
        meta[4..8].copy_from_slice(&size.to_le_bytes());
        meta[8..12].copy_from_slice(&(code_dwords as u32).to_le_bytes());
        meta[0x14] = 1;
        // A table of SRVs at user data 0: type 0 (SRV), root type 1 (table).
        let r = 0x18;
        meta[r] = 0x41;
        meta[r + 4] = 0;
        meta[r + 5] = 1;
        meta[r + 20] = 8;
        meta[r + 21] = 0;
        meta[r + 22] = 0;
        meta[r + 32] = 0x43;
        ComputeParts {
            packet,
            hash: vec![7u8; 0x14],
            // s_endpgm
            code: [0xbf81_0000u32].repeat(code_dwords).into_iter().flat_map(u32::to_le_bytes).collect(),
            meta,
        }
    }

    /// The record at 0x2000 with its four parts elsewhere.
    fn record(p: &ComputeParts) -> Fake {
        let mut m = Fake::default();
        m.put(0x1_0000, p.packet.clone());
        m.put(0x1_1000, p.hash.clone());
        m.put(0xe18_0200_00, p.code.clone());
        m.put(0x1_2000, p.meta.clone());
        m.words(
            0x2000,
            &[
                0x16d1_dbd0,
                0x1_0000,
                p.packet.len() as u64,
                0x1_1000,
                p.hash.len() as u64,
                0xe18_0200_00,
                p.code.len() as u64,
                0x1_2000,
                p.meta.len() as u64,
                0x14ba_d26e0,
                0x14ba_a1de0,
                0xd7e7_b1a2_2f0c_3802,
                0,
                0,
                0,
                0,
            ],
        );
        m
    }

    #[test]
    fn a_record_becomes_a_compute_container() {
        let p = parts(24);
        let c = compute_container(&record(&p), 0x2000).expect("record parses");
        let parsed = Container::parse(&c).unwrap();
        let accc = parsed.part(FourCC::ACCC).expect("an ACCC part");
        assert!(parsed.part(FourCC::DXIL).is_none());
        let programs = victory_shader::native_programs(accc.data).unwrap();
        assert_eq!(programs.len(), 1);
        assert_eq!(programs[0].stage, HwStage::Cs);
        assert!(programs[0].wave64);
        assert_eq!(programs[0].used.len(), 1);
        // The four entries are the ones the title gave, byte for byte.
        let f = fspx(&p);
        assert_eq!(&f[..4], b"FSPX");
        assert_eq!(accc.data, f);
        assert!(f.windows(p.code.len()).any(|w| w == p.code));
    }

    #[test]
    fn records_that_do_not_parse_are_refused() {
        // unreadable record
        assert!(compute_container(&Fake::default(), 0x2000).is_err());
        // the code's size is not what the metadata says
        let mut p = parts(24);
        p.meta[8..12].copy_from_slice(&25u32.to_le_bytes());
        let why = compute_container(&record(&p), 0x2000).unwrap_err();
        assert!(why.contains("fspx") && why.contains("code size"), "{why}");
        // a part is not there
        let p = parts(24);
        let mut m = record(&p);
        m.0.retain(|(at, _)| *at != 0x1_2000);
        assert!(compute_container(&m, 0x2000).unwrap_err().contains("metadata unreadable"));
        // sizes that are not a compute record's
        let mut m = record(&p);
        m.words(0x3000, &[0, 0x1_0000, 0x10, 0x1_1000, 0x14, 0xe18_0200_00, 96, 0x1_2000, 0x58, 0, 0, 0]);
        assert!(compute_container(&m, 0x3000).unwrap_err().contains("packet"));
        // code that is not whole dwords
        let mut m = record(&p);
        m.words(0x4000, &[0, 0x1_0000, 0x1c, 0x1_1000, 0x14, 0xe18_0200_00, 97, 0x1_2000, 0x58, 0, 0, 0]);
        assert!(compute_container(&m, 0x4000).unwrap_err().contains("dwords"));
    }

    #[test]
    fn a_container_makes_a_pipeline() {
        use crate::object::S_OK;
        let p = parts(8);
        let c = compute_container(&record(&p), 0x2000).unwrap();
        let mut pso = std::ptr::null();
        let iid = crate::pipeline::PIPELINE_IIDS[0];
        assert_eq!(crate::pipeline::compute_from_container(&c, &iid, &mut pso), S_OK);
        assert!(!pso.is_null());
    }

    #[test]
    fn the_dump_names_content_not_addresses() {
        let dir = tempfile::tempdir().unwrap();
        let d = Dumper::new(dir.path().to_path_buf(), 128, FOLLOW_MAX, 100);
        d.prepare().unwrap();
        let p = parts(24);
        let mut m = record(&p);
        // A root signature object of ours: a vtable pointer, then (pointer, size) = its blob.
        m.words(0x16d1_dbd0, &[0x1_4074_95e8, 0x3_0000_0000, 0x518, 0x16b9_d920, 0x20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        m.put(0x16b9_d920, vec![0x5a; 0x20]);
        // Two lone pointers' targets.
        m.put(0x14ba_d26e0, vec![1; 0x100]);
        m.put(0x14ba_a1de0, vec![2; 0x100]);
        d.sample(&m, 0x218, 0x2000);
        let files = |sub: &str| std::fs::read_dir(dir.path().join(sub)).unwrap().count();
        assert_eq!(files("0x218"), 1);
        // packet, hash, code, meta, the root object's blob, two lone pointers
        assert_eq!(files("blobs"), 7);
        // The same pipeline made again somewhere else is the same file.
        let mut m2 = Fake::default();
        m2.put(0x5_0000, p.packet.clone());
        m2.put(0x5_1000, p.hash.clone());
        m2.put(0xe19_0000_00, p.code.clone());
        m2.put(0x5_2000, p.meta.clone());
        m2.put(0x14ba_d26e0, vec![1; 0x100]);
        m2.put(0x14ba_a1de0, vec![2; 0x100]);
        m2.words(0x16d1_dbd0, &[0x1_4074_95e8, 0x3_0000_0000, 0x518, 0x16b9_d920, 0x20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        m2.put(0x16b9_d920, vec![0x5a; 0x20]);
        m2.words(
            0x6000,
            &[0x16d1_dbd0, 0x5_0000, 0x1c, 0x5_1000, 0x14, 0xe19_0000_00, p.code.len() as u64, 0x5_2000, p.meta.len() as u64, 0x14ba_d26e0, 0x14ba_a1de0, 0xd7e7_b1a2_2f0c_3802, 0, 0, 0, 0],
        );
        d.sample(&m2, 0x218, 0x6000);
        assert_eq!(files("0x218"), 1);
        assert_eq!(files("blobs"), 7);
        // Another slot, or other content, is another file.
        d.sample(&m2, 0x1c8, 0x6000);
        assert_eq!(files("0x1c8"), 1);
        let other = parts(32);
        d.sample(&record(&other), 0x218, 0x2000);
        assert_eq!(files("0x218"), 2);
        let text = std::fs::read_dir(dir.path().join("0x218")).unwrap().next().unwrap().unwrap().path();
        let text = std::fs::read_to_string(text).unwrap();
        assert!(text.contains("slot 0x218") && text.contains("w01 ") && text.contains("blob "), "{text}");
        assert!(text.contains("size of w01"), "{text}");
    }
}
