//! Rig of the dx.op node tests (`dxop_*.rs`; map-plan/inventory.md, "DXIL ->
//! MSC"): HLSL through DXC to DXIL, MSC, a Metal 4 compute dispatch, the
//! result read back for a CPU reference. Every program is checked to declare
//! the `dx.op.*` functions it is meant to prove, so a test cannot pass by DXC
//! having folded the operation away.
//!
//! A program sees `K` (eight root constants, b0), `I` (a root SRV byte
//! buffer, t0) and `O` (a root UAV byte buffer, u0): see [`PRELUDE`].
//! Without a DXC (`VICTORY_HELPER_DXC`, `DXC_LIB`; common_msc.rs) the tests
//! print `SKIPPED` and pass.
#![allow(dead_code)]
#![cfg(target_vendor = "apple")]

use std::collections::BTreeSet;
use std::sync::{Mutex, MutexGuard, OnceLock};

use victory_metal_back::bitcode::{Bitcode, Item};
use victory_metal_back::device::Device;
use victory_metal_back::memory::BufferKind;
use victory_metal_back::msc::{
    ArgKind, CompileOptions, Param, ParamKind, Reflection, RootSignatureDesc, Visibility,
};
use victory_metal_back::pipeline::ComputePipeline;
use victory_metal_back::queue::Queue;

use crate::common_msc::dxil_or_skip;

/// Declarations every compute test program starts with.
pub const PRELUDE: &str = "cbuffer K : register(b0) { uint k0; uint k1; uint k2; uint k3; uint k4; uint k5; uint k6; uint k7; }\n\
ByteAddressBuffer I : register(t0);\n\
RWByteAddressBuffer O : register(u0);\n";

pub struct Rig {
    _g: MutexGuard<'static, ()>,
    pub d: Device,
    pub q: Queue,
}

pub fn rig() -> Rig {
    static LOCK: Mutex<()> = Mutex::new(());
    static D: OnceLock<Device> = OnceLock::new();
    let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let d = D
        .get_or_init(|| Device::new().expect("Metal 4 device"))
        .clone();
    let q = Queue::new(&d).expect("queue");
    Rig { _g: g, d, q }
}

/// The `dx.op.*` functions a DXIL container declares (the names in the
/// module's value symbol table).
pub fn declared_dxops(dxil: &[u8]) -> BTreeSet<String> {
    let part = victory_metal_back::msc::dxil::part(dxil, b"DXIL").expect("a DXIL part");
    assert_eq!(&part[8..12], b"DXIL");
    let off = u32::from_le_bytes(part[16..20].try_into().unwrap()) as usize;
    let size = u32::from_le_bytes(part[20..24].try_into().unwrap()) as usize;
    let bc = Bitcode::parse(&part[8 + off..8 + off + size]).expect("bitcode");
    let mut out = BTreeSet::new();
    for module in bc.blocks.iter().filter(|b| b.id == 8) {
        for item in &module.items {
            let Item::Block(vst) = item else { continue };
            if vst.id != 14 {
                continue;
            }
            for r in vst.items.iter().filter_map(|i| match i {
                Item::Record(r) => Some(r),
                _ => None,
            }) {
                let skip = match r.code {
                    1 => 1,
                    3 => 2,
                    _ => continue,
                };
                let name: String = r.ops[skip..].iter().map(|&c| c as u8 as char).collect();
                if name.starts_with("dx.op.") {
                    out.insert(name);
                }
            }
        }
    }
    out
}

/// Panics unless the DXIL declares every one of `ops` (`dx.op.<name>.<overload>`).
pub fn assert_declares(name: &str, dxil: &[u8], ops: &[&str]) {
    let have = declared_dxops(dxil);
    for op in ops {
        assert!(
            have.contains(*op),
            "{name}: the DXIL does not declare {op}; it declares {have:?}"
        );
    }
}

/// DXIL of `hlsl` for `profile`, with the declared-op check, or `None`
/// (SKIPPED, no DXC).
pub fn dxil_checked(name: &str, profile: &str, hlsl: &str, ops: &[&str]) -> Option<Vec<u8>> {
    let dxil = dxil_or_skip(name, profile, hlsl)?;
    assert_declares(name, &dxil, ops);
    Some(dxil)
}

pub struct Prog {
    pub pipeline: ComputePipeline,
    pub reflection: Reflection,
    consts_at: u64,
    srv_at: u64,
    uav_at: u64,
}

impl Rig {
    /// A compute program: `body` after [`PRELUDE`], `ops` the `dx.op`
    /// functions it must declare.
    pub fn program(&self, name: &str, body: &str, ops: &[&str]) -> Option<Prog> {
        self.program_in(name, "cs_6_6", &format!("{PRELUDE}{body}"), ops)
    }

    /// As [`Rig::program`] with a profile and the whole source.
    pub fn program_in(&self, name: &str, profile: &str, hlsl: &str, ops: &[&str]) -> Option<Prog> {
        let dxil = dxil_checked(name, profile, hlsl, ops)?;
        Some(self.prog_from(&dxil))
    }

    pub fn prog_from(&self, dxil: &[u8]) -> Prog {
        let root = RootSignatureDesc {
            flags: 0,
            params: vec![
                Param {
                    kind: ParamKind::Constants {
                        register: 0,
                        space: 0,
                        num_32bit: 8,
                    },
                    visibility: Visibility::All,
                },
                Param {
                    kind: ParamKind::Srv {
                        register: 0,
                        space: 0,
                    },
                    visibility: Visibility::All,
                },
                Param {
                    kind: ParamKind::Uav {
                        register: 0,
                        space: 0,
                    },
                    visibility: Visibility::All,
                },
            ],
            static_samplers: vec![],
        };
        let sp = self
            .d
            .compute_pipeline_from_dxil(dxil, Some(&root), &CompileOptions::default())
            .expect("MSC and Metal accept the program");
        let at = |kind: ArgKind| {
            u64::from(
                sp.reflection
                    .layout
                    .entries
                    .iter()
                    .find(|e| e.kind == kind)
                    .map(|e| e.offset)
                    .unwrap_or_else(|| panic!("no {kind:?} entry in the layout")),
            )
        };
        Prog {
            consts_at: at(ArgKind::Constants),
            srv_at: at(ArgKind::Srv),
            uav_at: at(ArgKind::Uav),
            pipeline: sp.pipeline,
            reflection: sp.reflection,
        }
    }

    /// Runs `p` over `groups` of `threads`: `k` are the root constants,
    /// `input` the words of `I`, and `out_words` words of `O` (filled with
    /// 0xDEADBEEF first) come back.
    pub fn run(
        &self,
        p: &Prog,
        groups: [u32; 3],
        threads: [u32; 3],
        k: &[u32],
        input: &[u32],
        out_words: usize,
    ) -> Vec<u32> {
        let mut kk = [0u32; 8];
        kk[..k.len()].copy_from_slice(k);
        let inb = self
            .d
            .create_buffer(BufferKind::Shared, 4 * input.len().max(4) as u64)
            .unwrap();
        inb.write_u32s(0, input).unwrap();
        let out = self
            .d
            .create_buffer(BufferKind::Shared, 4 * out_words.max(1) as u64)
            .unwrap();
        out.write_u32s(0, &vec![0xDEAD_BEEF; out_words]).unwrap();
        let tlab = self
            .q
            .alloc_constants(u64::from(p.reflection.layout.size), 8)
            .unwrap();
        tlab.zero();
        tlab.write_u32s(p.consts_at, &kk).unwrap();
        tlab.write_u64(p.srv_at, inb.gpu_va()).unwrap();
        tlab.write_u64(p.uav_at, out.gpu_va()).unwrap();
        let mut l = self.q.begin_list().unwrap();
        l.set_root_arguments(&tlab).unwrap();
        l.dispatch(&p.pipeline, groups, threads).unwrap();
        l.end().unwrap();
        let v = self.q.submit(vec![l]).expect("submit");
        self.q.wait_for(v, 20_000).expect("completes");
        out.read_u32s(0, out_words).unwrap()
    }
}

/// Words that differ: `(index, got, want)`, the first `limit`.
pub fn diffs(got: &[u32], want: &[u32], limit: usize) -> Vec<(usize, u32, u32)> {
    assert_eq!(got.len(), want.len(), "lengths");
    got.iter()
        .zip(want)
        .enumerate()
        .filter(|(_, (g, w))| g != w)
        .map(|(i, (g, w))| (i, *g, *w))
        .take(limit)
        .collect()
}

/// `assert!` that `got == want`, showing the first differences in hex.
#[track_caller]
pub fn assert_words(what: &str, got: &[u32], want: &[u32]) {
    let d = diffs(got, want, 8);
    assert!(
        d.is_empty(),
        "{what}: {} of {} words differ; first (index, got, want): {}",
        diffs(got, want, usize::MAX).len(),
        got.len(),
        d.iter()
            .map(|(i, g, w)| format!("({i}, {g:#010x}, {w:#010x})"))
            .collect::<Vec<_>>()
            .join(" ")
    );
}
