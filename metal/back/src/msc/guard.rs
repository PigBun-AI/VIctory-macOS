//! What is refused before Metal Shader Converter sees it.
//!
//! MSC 4.0.3 does not return an error for some inputs: it prints
//! `metal-shaderconverter: [getMinMaxConstants:8246] Fatal: Unhandled LLVM
//! type for getMinMaxConstants` and aborts the process (`SIGABRT`; measured
//! with `victory-census convert --isolate`, 2026-11-28: the child dies in
//! under a second). Inside the game that abort takes the Wine process's
//! converting thread with it and the title stalls (qualify run 20261128-0217:
//! two Fatal lines, then 14 minutes of 300 % CPU with no further progress).
//! A `Result` cannot catch it, so the inputs that cause it are recognised
//! first and refused as [`MscError::Unsupported`]:
//!
//! * **Double precision.** The `SFI0` feature mask has
//!   `D3D_SHADER_REQUIRES_DOUBLES` (0x1) or
//!   `D3D_SHADER_REQUIRES_11_1_DOUBLE_EXTENSIONS` (0x20) set. The bit values
//!   are those of `d3d12shader.h` (`D3D_SHADER_REQUIRES_*`) and of DXC's
//!   `ShaderFeatureInfo_Doubles` / `ShaderFeatureInfo_DX11_1_DoubleExtensions`;
//!   a DXC 6.6 compute shader with double arithmetic carries `0x21`, with
//!   `WaveActiveSum(double)` `0x4021`. The shader translator
//!   (`victory-shader`) sets 0x1 only where it emits a `double`-typed
//!   operation (the wave sum of 64-bit floats); of the 25 780 shaders of the
//!   game, exactly the five `CS_TransmissionVolumetrics*` with it abort MSC.
//!   A container with no `SFI0` part (not made by DXC or the translator) is
//!   asked instead: its bitcode type table has a `double`.
//! * **A hash a census run found fatal** (`VICTORY_METAL_MSC_DENYLIST`): the
//!   FNV-1a hash of the converter's input bytes, as `hash_parts(&[dxil])`
//!   (the hash the front's and the back's drop statistics name a stage by),
//!   one per line; `#` starts a comment. `victory-census denylist` writes
//!   the file. The variable may name several files separated by `:` (the
//!   union applies), so the census lists each keep their own file.
//!
//! With `VICTORY_METAL_LOG=1` each refusal prints the hash.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use super::{MscError, dxil};
use crate::bitcode::{Bitcode, Block, Item};

/// The text of the double-precision refusal.
pub const DOUBLES_REASON: &str = "MSC 4.0.3 cannot convert double-precision";
/// The text of a denylist refusal.
pub const DENYLIST_REASON: &str = "census: MSC fatal";
/// The environment variable naming the denylist file.
pub const DENYLIST_ENV: &str = "VICTORY_METAL_MSC_DENYLIST";

/// The hash a stage is named by (front and back drop statistics, the
/// denylist): FNV-1a over the container.
pub fn stage_hash(bytes: &[u8]) -> u64 {
    victory_metal_proto::dropstats::hash_parts(&[bytes])
}

fn log_on() -> bool {
    std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0")
}

fn refuse(bytes: &[u8], reason: &str) -> MscError {
    if log_on() {
        eprintln!(
            "victory-metal back: MSC guard refused {:016x} ({} bytes): {reason}",
            stage_hash(bytes),
            bytes.len()
        );
    }
    MscError::Unsupported(reason.to_owned())
}

// ---- the denylist ---------------------------------------------------------

/// Hashes from a denylist file's text: the first token of each line, 16 hex
/// digits; what follows (the census hash, the reason) is for readers.
pub fn parse_denylist(text: &str) -> HashSet<u64> {
    text.lines()
        .filter_map(|l| {
            let tok = l.split('#').next()?.split_whitespace().next()?;
            u64::from_str_radix(tok, 16).ok()
        })
        .collect()
}

struct Loaded {
    /// The variable's value as read, and the modification time of each file
    /// it names (in order).
    path: OsString,
    mtimes: Vec<Option<SystemTime>>,
    hashes: Arc<HashSet<u64>>,
    pairs: Arc<HashSet<(u64, u64)>>,
}

/// Pairs from a denylist file's text: a line whose first token is
/// `<mesh hash, 16 hex>:<pixel hash, 16 hex>` (the `kind=pair` lines of
/// `victory-census s1pair --denylist`): a mesh pipeline whose compiler build
/// crashed Apple's GPU compiler service. A line that is no pair is left to
/// [`parse_denylist`].
pub fn parse_pair_denylist(text: &str) -> HashSet<(u64, u64)> {
    text.lines()
        .filter_map(|l| {
            let tok = l.split('#').next()?.split_whitespace().next()?;
            let (m, p) = tok.split_once(':')?;
            Some((
                u64::from_str_radix(m, 16).ok()?,
                u64::from_str_radix(p, 16).ok()?,
            ))
        })
        .collect()
}

static LOADED: Mutex<Option<Loaded>> = Mutex::new(None);

/// The denylist `VICTORY_METAL_MSC_DENYLIST` names: the union of the files
/// of its `:`-separated path list (read again when the value or any file's
/// modification time changes; none if unset). An unreadable file is reported
/// on stderr and counts as empty.
fn denylist() -> Option<Arc<HashSet<u64>>> {
    loaded().map(|l| l.0)
}

/// The pairs of the same file (see [`parse_pair_denylist`]).
pub fn denied_pair(ms: &[u8], ps: &[u8]) -> bool {
    loaded().is_some_and(|l| l.1.contains(&(stage_hash(ms), stage_hash(ps))))
}

type Lists = (Arc<HashSet<u64>>, Arc<HashSet<(u64, u64)>>);

/// The files the variable names: paths separated by `:` (like `PATH`;
/// `std::env::split_paths`), so a census can write its own file without
/// overwriting the others (the double-precision census list, the `kind=pair`
/// list of the mesh pipelines). Empty entries are skipped.
pub fn denylist_paths(value: &OsStr) -> Vec<PathBuf> {
    std::env::split_paths(value)
        .filter(|p| !p.as_os_str().is_empty())
        .collect()
}

/// The union of the files' hashes and pairs; a file that cannot be read is
/// reported on stderr and counts as empty (the others still apply).
fn load_files(files: &[PathBuf]) -> (HashSet<u64>, HashSet<(u64, u64)>) {
    let mut hashes = HashSet::new();
    let mut pairs = HashSet::new();
    for f in files {
        match std::fs::read_to_string(f) {
            Ok(t) => {
                hashes.extend(parse_denylist(&t));
                pairs.extend(parse_pair_denylist(&t));
            }
            Err(e) => eprintln!("victory-metal back: {DENYLIST_ENV} {f:?}: {e}"),
        }
    }
    (hashes, pairs)
}

fn loaded() -> Option<Lists> {
    let path = std::env::var_os(DENYLIST_ENV).filter(|p| !p.is_empty())?;
    let files = denylist_paths(&path);
    let mtimes: Vec<Option<SystemTime>> = files
        .iter()
        .map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok())
        .collect();
    let mut cur = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(l) = cur.as_ref()
        && l.path == path
        && l.mtimes == mtimes
    {
        return Some((l.hashes.clone(), l.pairs.clone()));
    }
    let (hashes, pairs) = load_files(&files);
    if log_on() {
        eprintln!(
            "victory-metal back: MSC denylist {path:?}: {} files, {} hashes, {} pairs",
            files.len(),
            hashes.len(),
            pairs.len()
        );
    }
    let (hashes, pairs) = (Arc::new(hashes), Arc::new(pairs));
    *cur = Some(Loaded {
        path,
        mtimes,
        hashes: hashes.clone(),
        pairs: pairs.clone(),
    });
    Some((hashes, pairs))
}

// ---- doubles ---------------------------------------------------------------

/// `TYPE_CODE_DOUBLE` of LLVM's bitcode type table (`LLVMBitCodes.h`), the
/// block id of that table, and of the module that holds it.
const TYPE_CODE_DOUBLE: u64 = 4;
const TYPE_BLOCK: u32 = 17;
const MODULE_BLOCK: u32 = 8;

fn block_has_double(b: &Block) -> bool {
    b.items.iter().any(|i| match i {
        Item::Record(r) => b.id == TYPE_BLOCK && r.code == TYPE_CODE_DOUBLE,
        Item::Block(sub) => block_has_double(sub),
        Item::Define(_) => false,
    })
}

/// Whether the DXIL part's bitcode has a `double` type. `false` when it
/// cannot be read (then MSC is asked).
fn bitcode_has_double(container: &[u8]) -> bool {
    let Some(part) = dxil::part(container, b"DXIL") else {
        return false;
    };
    // Program header: version, size in words, `DXIL`, DXIL version, bitcode
    // offset from the `DXIL` word, bitcode size.
    let word = |at: usize| -> Option<usize> {
        Some(u32::from_le_bytes(part.get(at..at + 4)?.try_into().ok()?) as usize)
    };
    let (Some(off), Some(size)) = (word(16), word(20)) else {
        return false;
    };
    let Some(bc) = part.get(8 + off..8 + off + size) else {
        return false;
    };
    match Bitcode::parse(bc) {
        Ok(m) => m
            .blocks
            .iter()
            .any(|b| b.id == MODULE_BLOCK && block_has_double(b)),
        Err(_) => false,
    }
}

/// Whether the container needs double precision: by its `SFI0` mask, and
/// only when it has none, by its types.
pub fn uses_doubles(container: &[u8], info: Option<&dxil::Info>) -> bool {
    match info.and_then(|i| i.features) {
        Some(f) => f & dxil::feature::ANY_DOUBLES != 0,
        None => bitcode_has_double(container),
    }
}

/// The refusal for the converter's input, if there is one. `original` is
/// what the caller passed (its hash is the denylist's key); `dxil_bytes`
/// the DXIL container (the DXBC converter's output for a DXBC input) and
/// `info` its parsed header.
pub fn check_with(
    original: &[u8],
    dxil_bytes: &[u8],
    info: Option<&dxil::Info>,
    denylist: Option<&HashSet<u64>>,
) -> Result<(), MscError> {
    // Doubles first: the more telling reason for a shader that is on the
    // list for that very abort.
    if uses_doubles(dxil_bytes, info) {
        return Err(refuse(original, DOUBLES_REASON));
    }
    if let Some(d) = denylist
        && d.contains(&stage_hash(original))
    {
        return Err(refuse(original, DENYLIST_REASON));
    }
    Ok(())
}

/// [`check_with`] with the denylist the environment names.
pub(super) fn check(
    original: &[u8],
    dxil_bytes: &[u8],
    info: Option<&dxil::Info>,
) -> Result<(), MscError> {
    let d = denylist();
    check_with(original, dxil_bytes, info, d.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_lines_are_read_and_stage_lines_are_not_pairs() {
        let text = "# pairs\n00000000000000aa:00000000000000bb\tkind=pair\tcrash\n00000000000000cc\tcs\tAbort\nzz:yy\n";
        let pairs = parse_pair_denylist(text);
        assert_eq!(pairs.len(), 1);
        assert!(pairs.contains(&(0xaa, 0xbb)));
        let stages = parse_denylist(text);
        assert_eq!(stages.len(), 1);
        assert!(stages.contains(&0xcc));
    }

    #[test]
    fn the_variable_names_files_separated_by_colons() {
        let v = OsString::from("/a/one.txt::/b/two.txt:/c d/three.txt:");
        let p = denylist_paths(&v);
        assert_eq!(
            p,
            vec![
                PathBuf::from("/a/one.txt"),
                PathBuf::from("/b/two.txt"),
                PathBuf::from("/c d/three.txt")
            ]
        );
        assert_eq!(denylist_paths(OsStr::new("x")), vec![PathBuf::from("x")]);
        assert!(denylist_paths(OsStr::new("")).is_empty());
    }

    #[test]
    fn several_files_are_one_union_and_a_missing_one_is_skipped() {
        let dir = std::env::temp_dir().join(format!("victory-denylist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b, gone) = (dir.join("a.txt"), dir.join("b.txt"), dir.join("gone.txt"));
        std::fs::write(&a, "00000000000000aa\tcs\tAbort\n").unwrap();
        std::fs::write(
            &b,
            "# pairs\n00000000000000b1:00000000000000b2\tkind=pair\n00000000000000bb\n",
        )
        .unwrap();
        let (hashes, pairs) = load_files(&[a.clone(), gone, b.clone()]);
        assert_eq!(hashes, HashSet::from([0xaa, 0xbb]));
        assert_eq!(pairs, HashSet::from([(0xb1, 0xb2)]));
        // The value of the variable, as the runner writes it.
        let joined = std::env::join_paths([&a, &b]).unwrap();
        assert_eq!(denylist_paths(&joined), vec![a, b]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A container with an `SFI0` part of `flags` (or none) and a DXIL part
    /// of `dxil_part` (program header and bitcode, as the container holds it).
    fn container(flags: Option<u64>, dxil_part: &[u8]) -> Vec<u8> {
        let mut parts: Vec<([u8; 4], Vec<u8>)> = Vec::new();
        if let Some(f) = flags {
            parts.push((*b"SFI0", f.to_le_bytes().to_vec()));
        }
        parts.push((*b"DXIL", dxil_part.to_vec()));
        let header = 32 + 4 * parts.len();
        let mut out = b"DXBC".to_vec();
        out.extend([0u8; 16]);
        out.extend(1u16.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(0u32.to_le_bytes());
        out.extend((parts.len() as u32).to_le_bytes());
        let mut off = header;
        for (_, d) in &parts {
            out.extend((off as u32).to_le_bytes());
            off += 8 + d.len();
        }
        for (f, d) in &parts {
            out.extend(f);
            out.extend((d.len() as u32).to_le_bytes());
            out.extend(d);
        }
        out
    }

    /// Just the program version (compute, 6.6), no bitcode.
    fn header_only() -> Vec<u8> {
        0x0005_0066u32.to_le_bytes().to_vec()
    }

    fn check_flags(flags: Option<u64>) -> Result<(), MscError> {
        let c = container(flags, &header_only());
        let info = dxil::inspect(&c);
        check_with(&c, &c, info.as_ref(), None)
    }

    #[test]
    fn either_double_bit_is_refused_with_the_reason() {
        for bits in [
            dxil::feature::DOUBLES,
            dxil::feature::DOUBLE_EXTENSIONS_11_1,
            0x4021,
            0x200_c001,
        ] {
            let e = check_flags(Some(bits)).unwrap_err();
            assert_eq!(e, MscError::Unsupported(DOUBLES_REASON.into()), "{bits:#x}");
            // The text callers see, through the crate's error.
            let top = crate::Error::from(e);
            assert!(
                matches!(&top, crate::Error::Unsupported(m) if m.contains("MSC 4.0.3 cannot convert double-precision")),
                "{top:?}"
            );
        }
    }

    #[test]
    fn other_feature_bits_pass() {
        for bits in [
            0,
            dxil::feature::WAVE_OPS,
            dxil::feature::INT64_OPS,
            dxil::feature::RESOURCE_DESCRIPTOR_HEAP_INDEXING | dxil::feature::STENCIL_REF,
            0x2004000,
            // 0x2 early depth, 0x4 UAVs at every stage, 0x10 min precision,
            // 0x40 shader extensions: not double.
            0x2 | 0x4 | 0x10 | 0x40,
        ] {
            assert_eq!(check_flags(Some(bits)), Ok(()), "{bits:#x}");
        }
    }

    #[test]
    fn a_container_without_features_is_asked_for_its_types() {
        // No SFI0 and no readable bitcode: not refused (MSC decides).
        assert_eq!(check_flags(None), Ok(()));
    }

    #[test]
    fn the_denylist_is_the_first_token_of_each_line() {
        let d = parse_denylist(
            "# census denylist\n\
             00000000000000ff\t5dc3b25986622949\tcensus: process abort\n\
             \n\
             1234abcd5678ef90 # trailing comment\n\
             not-a-hash\n\
             12345\n",
        );
        assert!(d.contains(&0xff));
        assert!(d.contains(&0x1234_abcd_5678_ef90));
        assert!(d.contains(&0x12345));
        assert_eq!(d.len(), 3);
    }

    #[test]
    fn a_denylisted_hash_is_refused_whatever_the_features() {
        let c = container(Some(0), &header_only());
        let info = dxil::inspect(&c);
        let mut d = HashSet::new();
        assert_eq!(check_with(&c, &c, info.as_ref(), Some(&d)), Ok(()));
        d.insert(stage_hash(&c));
        assert_eq!(
            check_with(&c, &c, info.as_ref(), Some(&d)),
            Err(MscError::Unsupported(DENYLIST_REASON.into()))
        );
        // Another container is not touched.
        let other = container(Some(dxil::feature::WAVE_OPS), &header_only());
        let oi = dxil::inspect(&other);
        assert_eq!(check_with(&other, &other, oi.as_ref(), Some(&d)), Ok(()));
    }
}
