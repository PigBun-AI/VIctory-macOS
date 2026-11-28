//! S-I-001 (docs/mac/PATCHES.md G12): the instruction decoder against LLVM's AMDGPU disassembler.
//!
//! Reference: LLVM 23.1.6 `llvm-mc -disassemble -triple=amdgcn -mcpu=gfx1030` (RDNA2, GFX10.3: the same encodings
//! as the console's, implemented independently from AMD's published instruction set documents "RDNA 2 Instruction
//! Set Architecture"). `tests/data/gfx10_llvm.txt` has one line per value of the top 16 bits of the first dword
//! (every encoding prefix and nearly every opcode field): the instruction head LLVM decodes, its mnemonic and
//! its length in dwords. The words are `tools/mac/llvm-isa-oracle/gen.py`'s (splitmix64 of the stratum).
//! Compared: whether a head is an instruction, its length (literals, DPP/SDWA words, extra MIMG address words)
//! and its opcode name.
use super::*;
use std::collections::BTreeMap;

const VARIANTS: u32 = 16 + 64;

/// The MIMG sweeps (variants 16 and up) only make sense for MIMG heads.
fn sampled(t: u64, variant: u32) -> bool {
    variant < 16 || (0xF000..0xF400).contains(&t)
}

fn splitmix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn words(top16: u64, variant: u32) -> Vec<u32> {
    match variant {
        0 => [vec![(top16 << 16) as u32], vec![0; 5]].concat(),
        3 => [vec![(top16 << 16) as u32, 0x007D_0000], vec![0; 4]].concat(),
        4 | 5 => [vec![(top16 << 16 | if variant == 4 { 0x4000 } else { 0x8000 }) as u32, 0x007D_0000], vec![0; 4]].concat(),
        6..=13 => [vec![(top16 << 16 | u64::from(variant - 6) << 2 | 0x0100) as u32, 0x0001_0101], vec![0; 4]].concat(),
        14 => [vec![(0xBE80_0000 | (top16 >> 8 & 0x7F) << 16 | (top16 & 0xFF) << 8 | 2) as u32], vec![0; 5]].concat(),
        15 => [vec![(0x7E00_0000 | (top16 >> 8) << 17 | (top16 & 0xFF) << 9 | 0x100) as u32], vec![0; 5]].concat(),
        16.. => [vec![(top16 << 16 | 0x9F00 | u64::from(variant - 16)) as u32, 0x0002_0004], vec![0; 4]].concat(),
        1 => [vec![(top16 << 16 | 0x0101) as u32, 0x0101_0101], vec![0; 4]].concat(),
        _ => {
            let mut w = vec![(top16 << 16 | splitmix(top16 * 8) & 0xFFFF) as u32];
            w.extend((1..6).map(|k| splitmix(top16 * 8 + k) as u32));
            w
        }
    }
}

/// (top16, variant) -> what LLVM says of it, for each of the two targets: (cpu, mnemonic, dwords).
type Golden = BTreeMap<(u64, u32), Vec<(&'static str, &'static str, usize)>>;

fn golden() -> Golden {
    let mut g = Golden::new();
    let files = [
        ("gfx1010", include_str!("../../tests/data/gfx10_llvm_gfx1010_0.txt")),
        ("gfx1010", include_str!("../../tests/data/gfx10_llvm_gfx1010_1.txt")),
        ("gfx1010", include_str!("../../tests/data/gfx10_llvm_gfx1010_2.txt")),
        ("gfx1010", include_str!("../../tests/data/gfx10_llvm_gfx1010_3.txt")),
        ("gfx1030", include_str!("../../tests/data/gfx10_llvm_gfx1030_0.txt")),
        ("gfx1030", include_str!("../../tests/data/gfx10_llvm_gfx1030_1.txt")),
        ("gfx1030", include_str!("../../tests/data/gfx10_llvm_gfx1030_2.txt")),
        ("gfx1030", include_str!("../../tests/data/gfx10_llvm_gfx1030_3.txt")),
    ];
    for (cpu, text) in files {
        for l in text.lines() {
            let mut f = l.split(' ');
            let t = u64::from_str_radix(f.next().unwrap(), 16).unwrap();
            let count: u64 = f.next().unwrap().parse().unwrap();
            let v = f.next().unwrap().parse().unwrap();
            let (name, len) = (f.next().unwrap(), f.next().unwrap().parse().unwrap());
            for t in t..t + count {
                g.entry((t, v)).or_default().push((cpu, name, len));
            }
        }
    }
    g
}

/// Ours, for the words of a variant.
fn ours(top16: u64, variant: u32) -> Option<(Option<&'static str>, usize)> {
    let bytes: Vec<u8> = words(top16, variant).iter().flat_map(|w| w.to_le_bytes()).collect();
    instruction(&bytes, 0).ok().map(|i| (i.name(), i.words.len()))
}

/// Per (head, variant) the two LLVM targets decode: ours must decode it as one of them (name and length); and the
/// names either side has that the other never does.
fn compare() -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut head_diffs = Vec::new();
    let mut llvm_names = std::collections::BTreeSet::new();
    let mut our_names = std::collections::BTreeMap::new();
    for (&(t, v), says) in &golden() {
        for &(_, name, _) in says {
            llvm_names.insert(name);
        }
        let ours = ours(t, v);
        if !says.iter().any(|&(_, name, len)| matches!(ours, Some((Some(o), l)) if o == name && l == len)) {
            head_diffs.push(format!("{t:04x} variant {v}: llvm {says:?}, ours {ours:?}"));
        }
    }
    for t in 0..65536u64 {
        for v in (0..VARIANTS).filter(|&v| sampled(t, v)) {
            if let Some((Some(n), _)) = ours(t, v) {
                our_names.entry(n).or_insert((t, v));
            }
        }
    }
    let missing = llvm_names.iter().filter(|n| !our_names.contains_key(*n)).map(|n| (*n).to_string()).collect();
    let extra = our_names.iter().filter(|(n, _)| !llvm_names.contains(*n)).map(|(n, (t, v))| format!("{n}@{t:04x}v{v}")).collect();
    (head_diffs, missing, extra)
}

/// Why a head LLVM decodes is not decoded the same way by ours, for the differences that are known and accepted.
fn explained(diff: &str) -> Option<&'static str> {
    if diff.contains("_g16") || diff.contains("image_gather4h") || diff.contains("_cd_") {
        return Some("MIMG opcode our table has no name for (decoded with the right length, so decoding goes on); the console GPU is RDNA2 and the title's census counts by mnemonic: none");
    }
    if diff.contains("v_dot8_i32_i4") {
        return Some("RDNA2 PC dot-product extension, no name in our table; not in the title's census");
    }
    if diff.starts_with("d3a") {
        return Some("GFX9's VOP3 prefix (110100) for v_mad_mix: the console's code has the GFX10 VOP3P form (0xCCxx), which matches");
    }
    if diff.starts_with("d60") {
        return Some("GFX10 VOP3 form of interpolation (opcodes 0x200-0x202): decoded with the right length but unnamed; the title's interpolation is VINTRP");
    }
    if diff.contains("tbuffer_") {
        return Some("LLVM's disassembler takes soffset = 0xFF as a literal dword (its assembler refuses it); ours does not");
    }
    None
}

/// S-I-001: the decoder against LLVM, for both RDNA targets: every instruction head either decodes the same way
/// (name and length) or is one of the explained differences; the numbers are fixed, so that a change of the decoder
/// that moves them fails.
#[test]
fn the_decoder_agrees_with_llvm() {
    let (heads, missing, extra) = compare();
    let golden = golden();
    let unexplained: Vec<&String> = heads.iter().filter(|h| explained(h).is_none()).collect();
    assert!(unexplained.is_empty(), "{} unexplained, e.g. {:?}", unexplained.len(), &unexplained[..unexplained.len().min(5)]);
    assert_eq!(heads.len(), 225, "known differences");
    assert_eq!(golden.len(), 74_903, "samples LLVM decodes");
    assert_eq!(missing.len(), 12, "names LLVM has that ours has not: {missing:?}");
    assert_eq!(
        extra,
        ["s_getpc_b64@001fv14", "s_mov_fed_b32@0035v14"],
        "names ours has that no sample made LLVM decode (the sweep's operands are not what these take, or GFX9 only)"
    );
}

#[test]
#[ignore = "prints the differences, for reading"]
fn report() {
    let (heads, missing, extra) = compare();
    println!("== {} differences, {} golden samples", heads.len(), golden().len());
    println!("== missing: {missing:?}\n== extra: {extra:?}");
}
