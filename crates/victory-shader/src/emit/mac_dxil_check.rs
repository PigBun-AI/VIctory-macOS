//! S-D-001 (docs/mac/PATCHES.md G12): an independent reader for every DXIL container the translator's own tests
//! produce on the Mac, where `dxil.dll` (the Windows validator) does not exist. Called from `emit_program` in test
//! builds that are not Windows; a malformed container or bitstream fails the test that made it.
//!
//! References: the DXBC container (header, part table) and the DXIL program header
//! (`DxilProgramHeader` / `DxilBitcodeHeader` of the DirectXShaderCompiler `DxilContainer.h`: program version, size in
//! dwords, 'DXIL' magic, DXIL version, bitcode offset from the magic, bitcode size); the LLVM bitstream
//! container format (magic 'BC' 0xC0DE, abbreviation ids, block lengths in dwords; LLVM Bitcode Format reference),
//! Known, tolerated: parts whose size is not a multiple of 4 (ISG1/OSG1 as `container::signature` writes them, names not
//! padded) leave the next part at an unaligned offset; DXC pads; `dxil.dll` and MSC take both (matrix S-D-001, finding F2).
//! read by `llvm-bcanalyzer` (Homebrew LLVM), which knows nothing of DXIL and checks the stream itself.
use std::io::Write;
use std::process::Command;

fn u32_at(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(d[at..at + 4].try_into().expect("4 bytes"))
}

/// Where `llvm-bcanalyzer` is, if anywhere (`VICTORY_BCANALYZER`, Homebrew's LLVM, the PATH).
fn analyzer() -> Option<&'static str> {
    static FOUND: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    FOUND
        .get_or_init(|| {
            let from_env = std::env::var("VICTORY_BCANALYZER").ok();
            [from_env, Some("/opt/homebrew/opt/llvm/bin/llvm-bcanalyzer".into()), Some("llvm-bcanalyzer".into())]
                .into_iter()
                .flatten()
                .find(|p| Command::new(p).arg("--version").output().is_ok_and(|o| o.status.success()))
        })
        .as_deref()
}

/// The container's structure: returns the bitcode bytes of its DXIL part (if it has one).
pub(crate) fn container(d: &[u8]) -> Result<Option<&[u8]>, String> {
    if d.len() < 0x20 || &d[..4] != b"DXBC" {
        return Err("no DXBC magic".into());
    }
    let total = u32_at(d, 0x18) as usize;
    if total != d.len() {
        return Err(format!("container size field {total}, data {}", d.len()));
    }
    let count = u32_at(d, 0x1c) as usize;
    if count == 0 || count > 64 || 0x20 + 4 * count > d.len() {
        return Err(format!("{count} parts"));
    }
    let mut end_of_previous = 0x20 + 4 * count;
    let mut bitcode = None;
    for i in 0..count {
        let at = u32_at(d, 0x20 + 4 * i) as usize;
        if at < end_of_previous || at + 8 > d.len() {
            let previous = if i > 0 { u32_at(d, 0x20 + 4 * (i - 1)) as usize } else { 0 };
            return Err(format!(
                "part {i} at {at:#x}, the table ends at {end_of_previous:#x}, container {:#x}; part {} is {} (size {})",
                d.len(),
                i.wrapping_sub(1),
                if i > 0 { String::from_utf8_lossy(&d[previous..previous + 4]).into_owned() } else { String::new() },
                if i > 0 { u32_at(d, previous + 4) } else { 0 },
            ));
        }
        let size = u32_at(d, at + 4) as usize;
        if at + 8 + size > d.len() {
            return Err(format!("part {i} runs past the container"));
        }
        end_of_previous = at + 8 + size;
        if &d[at..at + 4] == b"DXIL" {
            let p = &d[at + 8..at + 8 + size];
            if p.len() < 24 || &p[8..12] != b"DXIL" {
                return Err("DXIL part without its program header".into());
            }
            // the size field counts dwords of the whole part
            if u32_at(p, 4) as usize * 4 != p.len() {
                return Err(format!("program header says {} dwords, the part has {} bytes", u32_at(p, 4), p.len()));
            }
            let (offset, len) = (u32_at(p, 16) as usize, u32_at(p, 20) as usize);
            let start = 8 + offset;
            if start + len > p.len() || len % 4 != 0 || len < 8 {
                return Err(format!("bitcode at {start}, {len} bytes, part {}", p.len()));
            }
            let bc = &p[start..start + len];
            if bc[..4] != [b'B', b'C', 0xC0, 0xDE] {
                return Err("no bitcode magic".into());
            }
            bitcode = Some(bc);
        }
    }
    if end_of_previous != d.len() {
        return Err(format!("{} bytes after the last part", d.len() - end_of_previous));
    }
    Ok(bitcode)
}

/// Panics, naming what is wrong, when `dxil` is not a well-formed container with a well-formed bitstream.
pub(crate) fn check(dxil: &[u8]) {
    let bitcode = match container(dxil) {
        Ok(b) => b,
        Err(e) => panic!("the translator's container is malformed: {e}"),
    };
    let (Some(bitcode), Some(tool)) = (bitcode, analyzer()) else { return };
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!("victory-dxil-{}-{}.bc", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    std::fs::File::create(&path).and_then(|mut f| f.write_all(bitcode)).expect("a temporary file");
    let out = Command::new(tool).arg(&path).output().expect("llvm-bcanalyzer runs");
    let _ = std::fs::remove_file(&path);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "llvm-bcanalyzer rejects the bitstream: {}{text}", String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("Block ID #8 (MODULE_BLOCK)") || text.contains("MODULE_BLOCK"), "no module block in:\n{text}");
}

#[test]
fn the_check_catches_damage() {
    // a container made of one part with a short program header
    let mut d = Vec::new();
    d.extend(b"DXBC");
    d.extend([0u8; 16]);
    d.extend([1, 0, 0, 0]);
    d.extend(0u32.to_le_bytes());
    d.extend(1u32.to_le_bytes());
    d.extend(0x24u32.to_le_bytes());
    d.extend(b"DXIL");
    d.extend(8u32.to_le_bytes());
    d.extend([0u8; 8]);
    let len = d.len() as u32;
    d[0x18..0x1c].copy_from_slice(&len.to_le_bytes());
    assert!(container(&d).is_err(), "a DXIL part without a program header");
    assert!(container(&d[..d.len() - 4]).is_err(), "a truncated container");
    let mut wrong = d.clone();
    wrong[0] = b'X';
    assert!(container(&wrong).is_err(), "no magic");
}
