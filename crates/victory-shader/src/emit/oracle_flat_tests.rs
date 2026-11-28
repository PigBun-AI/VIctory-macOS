//! mac S6, oracle batch B: `global_*` (node `isa.mem.flat`): the title's memory by 40-bit address,
//! read through the page directory (`shader.rs` `SLOT_MEMORY`), as the translator reads it.
//!
//! References: GFX10 ISA manual "Flat, Scratch and Global Instructions" (the address is two VGPRs
//! plus the signed 13-bit instruction offset; `global_load_ubyte/sbyte/ushort/sshort` zero- or
//! sign-extend; `global_atomic_*` return the old value with GLC); D3D12: memory the title does not
//! map reads 0 and a write there is dropped (the host's page directory has no copy for it).
//! The title page at [`PAGE`] has a copy (heap slot 2); the pages around it have none.

use super::host::{Dispatch, Host, Root, Stride, View};
use super::oracle_mem_tests::{Cfg, Srv, merge, opc, pattern, smem, store_out, try_build};
use super::oracle_tests::{Outcome, report_family};
use super::valu_tests::{ONE_AT_A_TIME, vop1};
use super::HostShader;

/// The title page (64 KiB) whose copy the host holds.
const PAGE: u32 = 0x50;
const WORDS: usize = 64;

fn global(name: &str, vdst: u8, addr: u8, data: u8, offset: i16) -> [u32; 2] {
    [
        0x37 << 26 | opc(name) << 18 | 2 << 14 | (offset as u32 & 0x1FFF),
        0x7D << 16 | u32::from(vdst) << 24 | u32::from(data) << 8 | u32::from(addr),
    ]
}

/// Dispatches over the page directory: `copy` is the page's first bytes, `uav_copy` makes it a
/// zeroed UAV (read back as the second output).
fn run(shader: &HostShader, cases: &[[u32; 4]], copy: Option<&[u32]>, mut check: impl FnMut([u32; 4], &[Vec<u32>]) -> Option<String>) -> Outcome {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(host) = Host::new(false) else {
        return Outcome::Refused("no device".into());
    };
    let (root, pso) = match host.compute_pipeline(shader) {
        Ok(p) => p,
        Err(e) if e.contains("unsupported") => return Outcome::Refused(e),
        Err(e) => return Outcome::Mismatch { cases: 0, bad: 1, examples: vec![format!("compute_pipeline: {e}")] },
    };
    let mut directory = vec![0u32; 2 * super::MEMORY_PAGES as usize];
    directory[2 * PAGE as usize] = 2; // heap slot 2
    directory[2 * PAGE as usize + 1] = 0;
    let mut views: Vec<(u8, u8)> = shader
        .root_cbvs
        .iter()
        .map(|&r| (r, 0))
        .chain(shader.root_srvs.iter().map(|&r| (r, 1)))
        .chain(shader.root_uavs.iter().map(|&r| (r, 2)))
        .collect();
    views.sort_unstable();
    views.dedup();
    let (mut bad, mut examples, mut ran) = (0usize, Vec::new(), 0usize);
    for &c in cases {
        let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
        for u in &shader.user_data {
            if let (crate::native::RootType::Constants, 0) = (u.root, u.slot) {
                ud[u.index as usize] = c[u.dword as usize];
            }
        }
        let mut roots = vec![(0, Root::Constants(ud))];
        for (i, &(slot, kind)) in views.iter().enumerate() {
            roots.push((
                1 + i as u32,
                match (slot, kind) {
                    (super::SLOT_MEMORY, 1) => Root::View { srv: true, data: directory.clone() },
                    (_, 2) => Root::Uav(2 * super::MEMORY_FAULTS),
                    (_, k) => Root::View { srv: k == 1, data: vec![0; 64] },
                },
            ));
        }
        let filled = copy.map(<[u32]>::to_vec).unwrap_or_default();
        // the host's null view (first heap slot) is a page of zeros, as long as a page's copy
        let dummy = [0u32; WORDS];
        let heap = [
            View::Buffer { data: &dummy, stride: Stride::Raw },
            View::RawUav(16),
            if copy.is_some() { View::Buffer { data: &filled, stride: Stride::Raw } } else { View::RawUav(WORDS as u32) },
        ];
        let got = host.dispatch(&root, &pso, &Dispatch { roots, views: &heap, samplers: &[], groups: [1, 1, 1] });
        let got = match got {
            Ok(g) => g,
            Err(e) if e.contains("unsupported") => return Outcome::Refused(e),
            Err(e) => return Outcome::Mismatch { cases: ran, bad: bad.max(1), examples: vec![format!("dispatch: {e}")] },
        };
        ran += 1;
        if let Some(t) = check(c, &got) {
            bad += 1;
            if examples.len() < 3 {
                examples.push(t);
            }
        }
    }
    if bad == 0 { Outcome::Passed(ran) } else { Outcome::Mismatch { cases: ran, bad, examples } }
}

fn addr_of(byte: u32) -> u32 {
    PAGE << 16 | byte
}

fn loads() -> Vec<(String, Outcome)> {
    let data = pattern(WORDS);
    let bytes: Vec<u8> = data.iter().flat_map(|d| d.to_le_bytes()).collect();
    // (mnemonic, dwords read, bytes of the field, signed)
    let rows: [(&str, usize, u32, bool); 8] = [
        ("global_load_dword", 1, 4, false),
        ("global_load_dwordx2", 2, 4, false),
        ("global_load_dwordx3", 3, 4, false),
        ("global_load_dwordx4", 4, 4, false),
        ("global_load_ubyte", 1, 1, false),
        ("global_load_sbyte", 1, 1, true),
        ("global_load_ushort", 1, 2, false),
        ("global_load_sshort", 1, 2, true),
    ];
    rows.into_iter()
        .map(|(name, n, width, signed)| {
            let mut parts = Vec::new();
            for imm in [0i16, 16, -8 + 64] {
                let mut body = global(name, 8, 1, 0, imm).to_vec();
                for k in 0..n {
                    body.extend(store_out(8 + k as u8, 4 * k as u16));
                }
                let shader = match try_build(&body, &Cfg::new(Srv::Buffer(0))) {
                    Ok(s) => s,
                    Err(o) => {
                        parts.push((format!("imm {imm}"), o));
                        continue;
                    }
                };
                // addresses: aligned to the field, inside the copy; and a page with no copy
                let step = u32::from(width.min(4));
                let mut cases: Vec<[u32; 4]> = (0..16u32).map(|i| [addr_of(i * step * n as u32 % 128), 0, 0, 0]).collect();
                cases.push([0x0060_0000, 0, 0, 0]); // no copy: zeros
                let o = run(&shader, &cases, Some(&data), |c, got| {
                    let a = c[0].wrapping_add(imm as u32);
                    let mapped = a >> 16 == PAGE;
                    let at = (a & 0xFFFF) as usize;
                    let want: Vec<u32> = (0..n)
                        .map(|k| {
                            if !mapped || at + (k + 1) * width as usize > bytes.len() {
                                return 0;
                            }
                            let p = at + k * width as usize;
                            let raw = match width {
                                4 => u32::from_le_bytes([bytes[p], bytes[p + 1], bytes[p + 2], bytes[p + 3]]),
                                2 => u32::from(bytes[p]) | u32::from(bytes[p + 1]) << 8,
                                _ => u32::from(bytes[p]),
                            };
                            if signed { ((raw << (32 - 8 * width)) as i32 >> (32 - 8 * width)) as u32 } else { raw }
                        })
                        .collect();
                    (got[0][..n] != want[..]).then(|| format!("address {a:#x}: got {:x?} want {want:x?}", &got[0][..n]))
                });
                parts.push((format!("imm {imm}"), o));
            }
            (name.to_string(), merge(parts))
        })
        .collect()
}

/// Stores `n` dwords at the address, then reads the copy back (the UAV copy is the second output).
fn stores() -> Vec<(String, Outcome)> {
    let datas = [0xA0A0_0001u32, 0xB0B0_0002, 0xC0C0_0003, 0xD0D0_0004];
    [("global_store_dword", 1usize), ("global_store_dwordx3", 3), ("global_store_dwordx4", 4)]
        .into_iter()
        .map(|(name, n)| {
            let mut body = Vec::new();
            for (k, &d) in datas.iter().enumerate() {
                body.extend(super::oracle_mem_tests::v_mov_lit(8 + k as u8, d));
            }
            body.extend(global(name, 0, 1, 8, 0));
            let o = match try_build(&body, &Cfg::new(Srv::Buffer(0))) {
                Err(o) => o,
                Ok(shader) => {
                    let cases: Vec<[u32; 4]> = [0u32, 4, 16, 32].iter().map(|&b| [addr_of(b), 0, 0, 0]).chain([[0x0060_0000, 0, 0, 0]]).collect();
                    run(&shader, &cases, None, |c, got| {
                        let mut want = vec![0u32; WORDS];
                        if c[0] >> 16 == PAGE {
                            let at = ((c[0] & 0xFFFF) / 4) as usize;
                            for k in 0..n {
                                want[at + k] = datas[k];
                            }
                        }
                        (got[1][..WORDS] != want[..]).then(|| format!("address {:#x}: copy {:x?} want {want:x?}", c[0], &got[1][..16]))
                    })
                }
            };
            (name.to_string(), o)
        })
        .collect()
}

/// `global_atomic_add/sub` with the old value back (glc), onto a dword the program stored first;
/// `global_atomic_fmin/fmax` on floats.
fn atomics() -> Vec<(String, Outcome)> {
    let initial = [3.5f32.to_bits(), (-1.25f32).to_bits(), 0x1000_0001, 7.5f32.to_bits()];
    let operands = [2.0f32.to_bits(), 9.0f32.to_bits(), (-4.0f32).to_bits(), 5.5f32.to_bits()];
    type Op = fn(u32, u32) -> u32;
    let fmin: Op = |m, d| if f32::from_bits(d) < f32::from_bits(m) { d } else { m };
    let fmax: Op = |m, d| if f32::from_bits(d) > f32::from_bits(m) { d } else { m };
    let rows: [(&str, Op); 4] = [
        ("global_atomic_add", |m, d| m.wrapping_add(d)),
        ("global_atomic_sub", |m, d| m.wrapping_sub(d)),
        ("global_atomic_fmin", fmin),
        ("global_atomic_fmax", fmax),
    ];
    rows.into_iter()
        .map(|(name, f)| {
            let mut parts = Vec::new();
            // v1:v2 = address (c0, c1); v3 = initial (c2); v4 = operand (c3)
            let mut body = Vec::new();
            body.push(vop1(0x001, 8, 259)); // v_mov_b32 v8, v3  (initial)
            body.push(vop1(0x001, 9, 260)); // v9 = v4 (operand)
            body.extend(global("global_store_dword", 0, 1, 8, 0));
            let mut atomic = global(name, 10, 1, 9, 0);
            atomic[0] |= 1 << 16; // glc: the old value comes back
            body.extend(atomic);
            body.extend(store_out(10, 0));
            match try_build(&body, &Cfg::new(Srv::Buffer(0))) {
                Err(o) => parts.push((name.to_string(), o)),
                Ok(shader) => {
                    let cases: Vec<[u32; 4]> = (0..4).map(|i| [addr_of(8), 0, initial[i], operands[i]]).collect();
                    parts.push((
                        name.to_string(),
                        run(&shader, &cases, None, |c, got| {
                            let want_new = f(c[2], c[3]);
                            let (old, new) = (got[0][0], got[1][2]);
                            (old != c[2] || new != want_new).then(|| format!("mem {:#x} operand {:#x}: old {old:#x} new {new:#x}, want old {:#x} new {want_new:#x}", c[2], c[3], c[2]))
                        }),
                    ));
                }
            }
            (name.to_string(), merge(parts.into_iter().collect()))
        })
        .collect()
}

/// `s_load_dword*` from the title's memory: the address in s[0:1] (root constants c0, c1), the
/// dwords to s[16...], then to the output.
fn scalar_loads() -> Vec<(String, Outcome)> {
    let data = pattern(WORDS);
    [
        ("s_load_dword", 1usize),
        ("s_load_dwordx2", 2),
        ("s_load_dwordx4", 4),
        ("s_load_dwordx8", 8),
        ("s_load_dwordx16", 16),
    ]
    .into_iter()
    .map(|(name, n)| {
        let mut parts = Vec::new();
        for imm in [0u32, 8, 32] {
            let mut body = smem(name, 16, 0, imm, None).to_vec();
            for k in 0..n {
                body.push(vop1(0x001, 8 + k as u8, 16 + k as u16)); // v_mov_b32 v8+k, s16+k
            }
            for k in 0..n {
                body.extend(store_out(8 + k as u8, 4 * k as u16));
            }
            let mut cfg = Cfg::new(Srv::Buffer(0));
            cfg.vgprs = 32;
            let shader = match try_build(&body, &cfg) {
                Ok(s) => s,
                Err(o) => {
                    parts.push((format!("imm {imm}"), o));
                    continue;
                }
            };
            let cases: Vec<[u32; 4]> = [0u32, 64, 128].iter().map(|&b| [addr_of(b), 0, 0, 0]).chain([[0x0060_0000, 0, 0, 0]]).collect();
            let o = run(&shader, &cases, Some(&data), |c, got| {
                let a = c[0] + imm;
                let want: Vec<u32> = (0..n)
                    .map(|k| {
                        let at = (a & 0xFFFF) as usize / 4 + k;
                        if a >> 16 == PAGE && at < WORDS { data[at] } else { 0 }
                    })
                    .collect();
                (got[0][..n] != want[..]).then(|| format!("address {a:#x}: got {:x?} want {want:x?}", &got[0][..n]))
            });
            parts.push((format!("imm {imm}"), o));
        }
        (name.to_string(), merge(parts))
    })
    .collect()
}

fn rows_test(family: &str, rows: impl FnOnce() -> Vec<(String, Outcome)>) {
    if Host::new(false).is_err() {
        eprintln!("{family}: no device; not run");
        return;
    }
    report_family(family, rows());
}

#[test]
fn oracle_flat_loads() {
    rows_test("flat_load", loads);
}

#[test]
fn oracle_flat_stores() {
    rows_test("flat_store", stores);
}

#[test]
fn oracle_flat_atomics() {
    rows_test("flat_atomic", atomics);
}

#[test]
fn oracle_scalar_loads_of_memory() {
    rows_test("smem_memory", scalar_loads);
}
