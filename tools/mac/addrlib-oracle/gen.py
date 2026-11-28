#!/usr/bin/env python3
"""Makes crates/victory-formats/tests/addrlib_golden.rs: texture layouts of AMD's addrlib (the oracle, build.sh)
for a fixed, deterministic set of cases; the test compares Layout against them.

    python3 tools/mac/addrlib-oracle/gen.py [oracle binary] [mesa commit note]
"""
import os, random, subprocess, sys

oracle = sys.argv[1] if len(sys.argv) > 1 and sys.argv[1] else os.path.join(os.environ.get("TMPDIR", "/tmp"), "addrlib-oracle")
mesa = sys.argv[2] if len(sys.argv) > 2 else "unknown"
OUT = os.path.join(os.path.dirname(__file__), "../../../crates/victory-formats/tests/addrlib_golden.rs")

MODES = [("LINEAR", 0), ("SW_256B_S", 1), ("SW_256B_D", 2), ("SW_4KB_S", 5), ("SW_4KB_D", 6), ("SW_64KB_S", 9), ("SW_64KB_D", 10)]
# (element bytes, texels per element side): every format family of the title's package
ELEMENTS = [(1, 1), (2, 1), (4, 1), (8, 1), (16, 1), (8, 4), (16, 4)]
EXTENTS2D = [(1, 1), (2, 2), (3, 5), (4, 4), (7, 13), (16, 16), (17, 31), (32, 64), (64, 64), (100, 60), (127, 129), (256, 256), (513, 77), (1000, 3), (1024, 512)]
rnd = random.Random(20261128)
MAX_ELEMENTS = 300_000

def levels(w, h, d, mips):
    out = []
    for m in range(mips):
        out.append((max(w >> m, 1), max(h >> m, 1), max(d >> m, 1)))
    return out

def elements(case):
    _, dim, bpe, block, w, h, d, mips = case
    n = 0
    for mw, mh, md in levels(w, h, d, mips):
        n += -(-mw // block) * -(-mh // block) * (md if dim == 3 else d)
    return n

def maxmips(w, h, d=1):
    n = 1
    while max(w, h, d) >> n:
        n += 1
    return n

cases = []
for name, mode in MODES:
    for bpe, block in ELEMENTS:
        for extent in rnd.sample(EXTENTS2D, 5):
            w, h = extent
            if block == 4:
                w, h = max(w, 1), max(h, 1)
            top = maxmips(w, h)
            for mips in sorted({1, top, rnd.randint(1, top)}):
                for slices in (1, 3) if rnd.random() < 0.3 else (1,):
                    cases.append((mode, 2, bpe, block, w, h, slices, mips))
        # 3D: the modes the layout allows (linear, 4 KiB and 64 KiB standard)
        if mode in (0, 5, 9):
            for w, h, d in rnd.sample([(4, 4, 4), (8, 8, 8), (16, 16, 64), (33, 17, 40), (64, 32, 16), (5, 3, 9), (1, 1, 17)], 4):
                top = maxmips(w, h, d)
                for mips in sorted({1, top, rnd.randint(1, top)}):
                    cases.append((mode, 3, bpe, block, w, h, d, mips))
        if mode == 0 and block == 1:
            for w in (1, 100, 1000, 4096):
                cases.append((0, 1, bpe, 1, w, 1, 1, min(maxmips(w, 1), 6)))
cases = [c for c in dict.fromkeys(cases) if elements(c) <= MAX_ELEMENTS]

def probes(case):
    _, dim, bpe, block, w, h, d, mips = case
    pts = set()
    for _ in range(14):
        m = rnd.randrange(mips)
        mw, mh, md = levels(w, h, d, mips)[m]
        ew, eh = -(-mw // block), -(-mh // block)
        z = rnd.randrange(md if dim == 3 else d)
        pts.add((m, z, rnd.randrange(eh), rnd.randrange(ew)))
    last = levels(w, h, d, mips)
    for m in (0, mips - 1):
        mw, mh, md = last[m]
        pts.add((m, (md if dim == 3 else d) - 1, -(-mh // block) - 1, -(-mw // block) - 1))
    return sorted(pts)   # the order the oracle visits them: mip, slice, row, column

lines, kept = [], []
for c in cases:
    ps = probes(c)
    line = " ".join(map(str, c)) + " @ " + " ".join(f"{m} {x} {y} {z}" for m, z, y, x in ps)
    lines.append(line)
    kept.append((c, ps))
res = subprocess.run([oracle], input="\n".join(lines) + "\n", capture_output=True, text=True, check=True).stdout.strip().split("\n")
assert len(res) == len(kept), (len(res), len(kept))
rows = []
for (c, ps), r in zip(kept, res):
    if r.startswith("ERR"):
        continue
    p = r.split()
    size, tail, h64, count = int(p[0]), int(p[1]), p[2], int(p[3])
    addrs = list(map(int, p[4:]))
    assert len(addrs) == len(ps), (c, r)
    rows.append((c, size, tail, h64, count, ps, addrs))

# the modes with XOR: size and mip tail only (their addresses need each surface's pipe-bank XOR)
XOR_MODES = [17, 18, 21, 22, 24, 25, 26, 27]
xcases = []
for mode in XOR_MODES:
    for bpe, block in ELEMENTS:
        for w, h in rnd.sample(EXTENTS2D, 6):
            top = maxmips(w, h)
            for mips in sorted({1, top, rnd.randint(1, top)}):
                xcases.append((mode, 2, bpe, block, w, h, 1, mips))
                if rnd.random() < 0.15:
                    xcases.append((mode, 2, bpe, block, w, h, 4, mips))
        if mode in (17, 21, 25, 26, 27, 24):
            for w, h, d in rnd.sample([(4, 4, 4), (8, 8, 8), (16, 16, 64), (33, 17, 40), (64, 32, 16), (5, 3, 9)], 3):
                top = maxmips(w, h, d)
                for mips in sorted({1, top}):
                    xcases.append((mode, 3, bpe, block, w, h, d, mips))
xcases = list(dict.fromkeys(xcases))
xres = subprocess.run([oracle], input="\n".join(" ".join(map(str, c)) for c in xcases) + "\n", capture_output=True, text=True, check=True).stdout.strip().split("\n")
assert len(xres) == len(xcases)
xrows = [(c, int(r.split()[0]), int(r.split()[1])) for c, r in zip(xcases, xres) if not r.startswith("ERR")]
modename = {m: n for n, m in MODES}
with open(OUT, "w") as f:
    f.write(f"""//! Texture layouts as AMD's addrlib computes them, against `Layout` (C-V-003, docs/mac/translation-matrix-mac.csv.txt).
//!
//! Reference: addrlib of Mesa (`src/amd/addrlib`, MIT; commit {mesa}), the library AMD's drivers use,
//! `Addr2ComputeSurfaceInfo` and `Addr2ComputeSurfaceAddrFromCoord`, Navi 21 (GFX10.3), no pipe-bank XOR
//! (`pipeBankXor` 0: the modes without XOR give the same addresses on every GFX10 part). The file is made by
//! `tools/mac/addrlib-oracle/gen.py` (the oracle is `tools/mac/addrlib-oracle/oracle.cpp.txt`); do not edit it by hand.
//! Per case: mode, dimension (1, 2, 3), element bytes, texels per element side, width, height, depth or array
//! size, mips; then the surface size in bytes, the first mip in the mip tail (the number of mips: none),
//! FNV-1a of the address of every element (mips in order, then slices, rows, columns; 8 little-endian bytes
//! each) and the number of elements; then some addresses by (mip, slice, row, column).
//! Covered: every mode of GFX10 without XOR (`LINEAR`, `256B_S/D`, `4KB_S/D`, `64KB_S/D`) with every element size
//! 1-16 bytes, 4x4 block compression, odd extents, mip chains (whole and partial), arrays, volumes. The modes with
//! XOR (`_T`, `_X`) are not addressable by `Layout` (matrix row C-V-003).
use victory_formats::texture::{{Desc, Dimension, Layout, Swizzle}};

struct Case {{
    mode: u32,
    dimension: u32,
    element_bytes: u32,
    block: u32,
    extent: [u32; 3],
    mips: u32,
    size: u64,
    first_tail: u32,
    hash: u64,
    elements: u64,
    probes: &'static [([u32; 4], u64)],
}}

#[rustfmt::skip]
const CASES: &[Case] = &[
""")
    for c, size, tail, h64, count, ps, addrs in rows:
        mode, dim, bpe, block, w, h, d, mips = c
        pr = ", ".join(f"([{m}, {z}, {y}, {x}], {a})" for (m, z, y, x), a in zip(ps, addrs))
        f.write(f"    Case {{ mode: {mode}, dimension: {dim}, element_bytes: {bpe}, block: {block}, extent: [{w}, {h}, {d}], mips: {mips}, size: {size}, first_tail: {tail}, hash: {h64}, elements: {count}, probes: &[{pr}] }},\n")
    f.write("];\n")
    f.write("\n/// The modes with XOR: mode, dimension, element bytes, block, extent, mips, size, first tail mip.\n#[rustfmt::skip]\nconst XOR_CASES: &[(u32, u32, u32, u32, [u32; 3], u32, u64, u32)] = &[\n")
    for (mode, dim, bpe, block, w, h, d, mips), size, tail in xrows:
        f.write(f"    ({mode}, {dim}, {bpe}, {block}, [{w}, {h}, {d}], {mips}, {size}, {tail}),\n")
    f.write("];\n")
    f.write("""
fn layout_of(c: &Case) -> Layout {
    let dimension = match c.dimension {
        1 => Dimension::Tex1d,
        2 => Dimension::Tex2d,
        _ => Dimension::Tex3d,
    };
    Layout::new(&Desc {
        dimension,
        width: c.extent[0],
        height: c.extent[1],
        depth: c.extent[2],
        mips: c.mips,
        element_bytes: c.element_bytes,
        block: c.block,
        samples: 1,
        swizzle: Swizzle(c.mode),
    })
    .unwrap_or_else(|e| panic!("mode {} {:?} {}x{}x{} {} bytes: {e:?}", c.mode, dimension, c.extent[0], c.extent[1], c.extent[2], c.element_bytes))
}

/// Size, tail and the address of every element, as addrlib has them.
#[test]
fn every_address_matches_addrlib() {
    assert!(CASES.len() > 250, "{} cases", CASES.len());
    for c in CASES {
        let l = layout_of(c);
        let three_d = c.dimension == 3;
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut n = 0u64;
        for (mip, m) in (0..).zip(&l.mips) {
            let slices = if three_d { m.depth } else { c.extent[2] };
            for z in 0..slices {
                for y in 0..m.height {
                    for x in 0..m.width {
                        for byte in l.offset(mip, x, y, z).unwrap().to_le_bytes() {
                            h = (h ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
                        }
                        n += 1;
                    }
                }
            }
        }
        let who = || format!("mode {} dim {} {}B block {} {:?} mips {}", c.mode, c.dimension, c.element_bytes, c.block, c.extent, c.mips);
        assert_eq!(n, c.elements, "{}", who());
        assert_eq!((l.size, l.first_tail_mip), (c.size, c.first_tail), "{}", who());
        assert_eq!(h, c.hash, "{}", who());
        for &([mip, z, y, x], address) in c.probes {
            assert_eq!(l.offset(mip, x, y, z), Ok(address), "{} at mip {mip} ({x},{y},{z})", who());
        }
    }
}

/// The modes with XOR (the title's render targets and depth): sizes and tails, which decide the memory the title
/// allocates and the offsets it streams to; their addresses are not computed (matrix row C-V-003).
#[test]
fn sizes_of_the_xor_modes_match_addrlib() {
    assert!(XOR_CASES.len() > 200, "{} cases", XOR_CASES.len());
    let mut bad = Vec::new();
    for &(mode, dimension, element_bytes, block, extent, mips, size, first_tail) in XOR_CASES {
        let c = Case { mode, dimension, element_bytes, block, extent, mips, size, first_tail, hash: 0, elements: 0, probes: &[] };
        let got = std::panic::catch_unwind(|| {
            let l = layout_of(&c);
            (l.size, l.first_tail_mip)
        });
        if got.ok() != Some((size, first_tail)) {
            bad.push(format!("mode {mode} dim {dimension} {element_bytes}B block {block} {extent:?} mips {mips}: addrlib {size} tail {first_tail}"));
        }
    }
    assert!(bad.is_empty(), "{} of {} differ:\\n{}", bad.len(), XOR_CASES.len(), bad.join("\\n"));
}

/// Data: elements put at the addresses addrlib gives come out of `detile` where they belong, and `tile` puts
/// them at those addresses (what the title wrote in console memory, what the host texture gets).
#[test]
fn data_tiled_at_addrlib_addresses_detiles_to_rows() {
    for c in CASES.iter().filter(|c| c.size <= 1 << 22) {
        let l = layout_of(c);
        let bpe = c.element_bytes as usize;
        let at = |mip: u32, x: u32, y: u32, z: u32| {
            let mut v = [0u8; 16];
            let k = u64::from(mip) << 48 ^ u64::from(z) << 36 ^ u64::from(y) << 20 ^ u64::from(x) ^ 0x5a5a;
            for (i, b) in v.iter_mut().enumerate() {
                *b = (k.wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(i as u32 * 7) >> 24) as u8 ^ i as u8;
            }
            v
        };
        let mut tiled = vec![0u8; l.size as usize];
        // the elements of mip `mip` of array slice or depth slice `z`, in rows
        for mip in 0..c.mips {
            let m = &l.mips[mip as usize];
            let slices = if c.dimension == 3 { 1 } else { c.extent[2] };
            for slice in 0..slices {
                let depth = if c.dimension == 3 { m.depth } else { 1 };
                let row = m.width as usize * bpe;
                let mut rows = vec![0u8; row * m.height as usize * depth as usize];
                for z in 0..depth {
                    for y in 0..m.height {
                        for x in 0..m.width {
                            let from = ((z * m.height + y) * m.width + x) as usize * bpe;
                            rows[from..from + bpe].copy_from_slice(&at(mip, x, y, slice + z)[..bpe]);
                        }
                    }
                }
                l.tile(&mut tiled, mip, slice, &rows, row, row * m.height as usize).unwrap();
            }
        }
        // every sampled address holds that element's bytes
        for &([mip, z, y, x], address) in c.probes {
            let m = &l.mips[mip as usize];
            let (slice, zz) = if c.dimension == 3 { (0, z) } else { (z, 0) };
            let _ = (slice, m);
            let want = at(mip, x, y, z);
            let a = address as usize;
            assert_eq!(&tiled[a..a + bpe], &want[..bpe], "mode {} {:?} mip {mip} ({x},{y},{z}) at {address}", c.mode, c.extent);
            let _ = zz;
        }
        // and detile gives the rows back
        for mip in 0..c.mips {
            let m = &l.mips[mip as usize];
            let slices = if c.dimension == 3 { 1 } else { c.extent[2] };
            for slice in 0..slices {
                let depth = if c.dimension == 3 { m.depth } else { 1 };
                let row = m.width as usize * bpe;
                let mut rows = vec![0u8; row * m.height as usize * depth as usize];
                l.detile(&tiled, mip, slice, &mut rows, row, row * m.height as usize).unwrap();
                for z in 0..depth {
                    for y in 0..m.height {
                        for x in 0..m.width {
                            let from = ((z * m.height + y) * m.width + x) as usize * bpe;
                            assert_eq!(&rows[from..from + bpe], &at(mip, x, y, slice + z)[..bpe], "mode {} {:?} mip {mip} ({x},{y},{z})", c.mode, c.extent);
                        }
                    }
                }
            }
        }
    }
}
""")
print(f"{len(rows)} cases -> {OUT}")
