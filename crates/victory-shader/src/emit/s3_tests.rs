//! mac S3: console wave64 compute programs on a host of 32-lane waves that stopped at "no
//! wave32 lowering": LDS in a lane branch (taken wave-wide with `VICTORY_WAVE_WIDE=1`; off by default, mac S3), groups of several
//! console waves, the first lane. The translation checks run everywhere; those that run on the
//! hardware device (waves of 32) only under Windows.

use super::{HostShader, lower};
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{ShaderError, io, ir, resource, structure};

const S_ENDPGM: u32 = 0xBF81_0000;

/// A wave64 compute program of `threads` threads and 512 bytes of LDS, with 4 root constants
/// in s0..s3 and a raw UAV's descriptor in s[8:11], then `body`.
fn native(body: &[u32], threads: u32) -> NativeProgram {
    let mut words = vec![
        0xBE85_0380, // s_mov_b32 s5, 0
        0x3D << 26 | 0x02 << 18 | 8 << 6 | 2,
        125 << 25, // s_load_dwordx4 s[8:11], s[4:5], 0
    ];
    words.extend_from_slice(body);
    words.push(S_ENDPGM);
    NativeProgram {
        stage: HwStage::Cs,
        wave64: true,
        vgprs: 16,
        layout: vec![
            RootParam { root: RootType::Constants, table_offset_dwords: 0, sgprs: 4, base_register: 0, user_data: 0, slot: 0 },
            RootParam { root: RootType::Table, table_offset_dwords: 0, sgprs: 1, base_register: 0, user_data: 4, slot: 1 },
        ],
        used: vec![UsedResource {
            kind: ResourceType::Uav,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords: 4,
            user_data: 4,
            slot: 1,
        }],
        io: StageIo::Cs(ComputeIo { rsrc2: 5 << 1 | 1 << 15, threads: [threads, 1, 1] }),
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    }
}

fn container() -> crate::Container<'static> {
    crate::Container { digest: [0; 16], version: (1, 0), parts: Vec::new(), bytes: &[] }
}

/// `emit` (the whole ladder) for a host of 32-lane waves.
fn emitted(native: &NativeProgram, p: &ir::Program) -> Result<HostShader, ShaderError> {
    let st = structure::structure(p).expect("structure");
    let res = resource::track(p, &native.layout, native.stage, Some(&st)).expect("resource::track");
    let io = io::raise(p, native).expect("io::raise");
    let was = lower::TEST_HOST_WAVE.replace(Some(32));
    let c = container();
    let r = super::emit(&c, native, p, &st, &io, &res);
    lower::TEST_HOST_WAVE.set(was);
    r
}

/// Thread t writes `1000 + t` to LDS[t] in every lane; then, inside `if (t < s0)`, it writes
/// `33 + t` to LDS[t] and reads LDS[(t + 32) & 63] (a lane of the other half of the console
/// wave) to dword t of the UAV. A lane outside the branch stores nothing.
fn lds_in_a_lane_branch(threads: u32) -> NativeProgram {
    native(
        &[
            0x3402_0082, // v_lshlrev_b32 v1, 2, v0
            0x4a0a_00ff, 1000, // v_add_nc_u32 v5, 1000, v0
            0xd834_0000, 0x0000_0501, // ds_write_b32 v1, v5
            0x4a04_00a0, // v_add_nc_u32 v2, 32, v0
            0x3604_04bf, // v_and_b32 v2, 63, v2
            0x3404_0482, // v_lshlrev_b32 v2, 2, v2
            0xd760_0006, 0x0001_4300, // v_readlane_b32 s6, v0, 33 (a lane of the upper half)
            0x4a08_0006, // v_add_nc_u32 v4, s6, v0
            0x7d88_0000, // v_cmp_gt_u32 vcc, s0, v0
            0xbe8c_246a, // s_and_saveexec_b64 s[12:13], vcc
            0xbf88_0000 | 7, // s_cbranch_execz (to the restore)
            0xd834_0000, 0x0000_0401, // ds_write_b32 v1, v4
            0xd8d8_0000, 0x0300_0002, // ds_read_b32 v3, v2
            0xbf8c_c07f, // s_waitcnt lgkmcnt(0)
            0xe070_1000, 0x8002_0301, // buffer_store_dword v3, v1, s[8:11], 0 offen
            0xbefe_040c, // s_mov_b64 exec, s[12:13]
        ],
        threads,
    )
}

/// Every thread stores the first lane's `3 * t + 7` of its console wave.
fn first_lane(threads: u32) -> NativeProgram {
    native(
        &[
            0x3402_0082, // v_lshlrev_b32 v1, 2, v0
            0x1604_0083, // v_mul_u32_u24 v2, 3, v0
            0x4a04_0487, // v_add_nc_u32 v2, 7, v2
            0xd760_0006, 0x0001_4300, // v_readlane_b32 s6, v0, 33 (the builders have such a read: the halves exchange)
            0x7e08_0502, // v_readfirstlane_b32 s4, v2
            0x7e06_0204, // v_mov_b32 v3, s4
            0xe070_1000, 0x8002_0301, // buffer_store_dword v3, v1, s[8:11], 0 offen
        ],
        threads,
    )
}

fn names(p: &ir::Program) -> Vec<&'static str> {
    p.blocks.iter().flat_map(|b| b.insts.iter().map(|i| i.name())).collect()
}

#[test]
fn the_programs_are_the_instructions_meant() {
    let p = ir::build(&lds_in_a_lane_branch(64).code, true, 16).unwrap();
    let n = names(&p);
    for want in ["ds_write_b32", "ds_read_b32", "s_and_saveexec_b64", "s_cbranch_execz", "buffer_store_dword"] {
        assert!(n.contains(&want), "{want} in {n:?}");
    }
    let p = ir::build(&first_lane(64).code, true, 16).unwrap();
    let n = names(&p);
    for want in ["v_readfirstlane_b32", "v_mul_u32_u24", "buffer_store_dword"] {
        assert!(n.contains(&want), "{want} in {n:?}");
    }
}

/// `emit` under `VICTORY_WAVE_WIDE=value` (empty: unset), whatever the process environment says.
fn emitted_with_wide(native: &NativeProgram, p: &ir::Program, value: &str) -> Result<HostShader, ShaderError> {
    let snap = crate::emit::live::snapshot_of(&[("VICTORY_WAVE_WIDE".to_string(), value.to_string())]).unwrap(); // mac S3
    crate::emit::live::scoped(&snap, || emitted(native, p))
}

/// One console wave of 64 threads: the halves meet inside the lane branch only if the wave
/// takes it together, which `emit` reaches with `VICTORY_WAVE_WIDE=1` (mac S3: off by default).
#[test]
fn lds_written_and_read_in_a_lane_branch_of_one_console_wave_translates() {
    let native = lds_in_a_lane_branch(64);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    emitted_with_wide(&native, &p, "1").expect("translates on a host of 32-lane waves"); // mac S3
}

/// mac S3: the wide branch (a group barrier inside a divergent path, which hangs a single-thread
/// group on Apple GPUs) is tried only with `VICTORY_WAVE_WIDE=1` (and for the two scene kernels).
#[test]
fn the_wave_wide_branch_is_off_by_default() {
    use std::ffi::OsStr;
    assert!(!super::wide_branches_on(None, false), "unset");
    assert!(!super::wide_branches_on(Some(OsStr::new("0")), false), "=0");
    assert!(!super::wide_branches_on(Some(OsStr::new("")), false), "empty");
    assert!(super::wide_branches_on(Some(OsStr::new("1")), false), "=1");
    assert!(super::wide_branches_on(None, true), "the two scene kernels keep upstream's default");
    assert!(!super::wide_branches_on(Some(OsStr::new("0")), true), "=0 beats the scene kernels");
}

/// A group of two console waves: the waves can take the branch apart, and the group barriers
/// the halves' meeting needs would wait for waves that never come: refused where one of them
/// lies in a wave-wide branch, never translated into a hang.
#[test]
fn a_group_barrier_in_a_wave_wide_branch_of_several_console_waves_is_refused() {
    let native = lds_in_a_lane_branch(128);
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let st = structure::structure(&p).expect("structure");
    let res = resource::track(&p, &native.layout, native.stage, Some(&st)).expect("resource::track");
    let io = io::raise(&p, &native).expect("io::raise");
    let was = lower::TEST_HOST_WAVE.replace(Some(32));
    let c = container();
    let r = super::widen(&c, &native, &p, true, false);
    let first = super::emit_program(&c, &native, &p, &st, &io, &res);
    lower::TEST_HOST_WAVE.set(was);
    eprintln!("widen: {:?}; plain: {:?}", r.as_ref().map(|x| x.1), first.as_ref().map(|x| x.1));
    if let Ok((_, wave)) = &r {
        // translated without a wide branch holding a barrier: then it must be exact on its own
        assert_eq!(*wave, Some(32));
    } else {
        assert!(matches!(&r, Err(ShaderError::Unsupported { what, .. }) if what.contains("several console waves")), "{:?}", r.err());
    }
}

/// Hardware: the same programs run on the device (waves of 32) and compared with the console's
/// results. They return where there is no device or it does not take the program.
#[cfg(windows)]
mod hardware {
    use super::*;
    use crate::emit::host::{Dispatch, Host, Root, View};
    use crate::emit::validator;

    /// The dwords of the UAV after one group of `shader`, with the root constant s0 = `below`.
    fn group_of(shader: &HostShader, below: u32) -> Option<Vec<u32>> {
        let _one = crate::emit::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(host) = Host::new(false) else {
            eprintln!("no SM 6.6 hardware device; not run");
            return None;
        };
        let (root, pso) = match host.compute_pipeline(shader) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("the hardware device does not take this program (waves of 32?): {e}; not run");
                return None;
            }
        };
        let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
        for u in &shader.user_data {
            if let (RootType::Constants, 0, 0) = (u.root, u.slot, u.dword) {
                ud[u.index as usize] = below;
            }
        }
        let out = host
            .dispatch(
                &root,
                &pso,
                &Dispatch {
                    roots: vec![(0, Root::Constants(ud))],
                    views: &[View::RawUav(256)],
                    samplers: &[],
                    groups: [1, 1, 1],
                },
            )
            .unwrap_or_else(|e| panic!("dispatch failed: {e}"));
        Some(out[0].clone())
    }

    fn validated(shader: &HostShader) {
        if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
            let mut copy = shader.dxil.clone();
            v.validate(&mut copy).expect("DXIL validation failed");
        }
    }

    /// LDS written and read in one lane branch of one console wave, with a read of a lane of
    /// the other half: the console's order holds. A lane reads what the lane of the other
    /// half wrote a moment before, or what every lane wrote before the branch where that
    /// lane stays out.
    #[test]
    fn lds_in_a_lane_branch_of_one_console_wave_keeps_the_consoles_order() {
        let native = lds_in_a_lane_branch(64);
        let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
        let shader = emitted(&native, &p).expect("translates");
        validated(&shader);
        for below in [64u32, 40, 33, 32, 31, 10, 1, 0] {
            let Some(got) = group_of(&shader, below) else { return };
            for t in 0..64u32 {
                let u = (t + 32) & 63;
                let expected = if t >= below { 0 } else if u < below { 33 + u } else { 1000 + u };
                assert_eq!(got[t as usize], expected, "{below} threads in the branch, thread {t}");
            }
        }
        eprintln!("ran on the hardware device: LDS order of the console wave kept in a lane branch");
    }

    /// `v_readfirstlane_b32` in a console wave run as two host waves: the first lane is the
    /// console wave's, not each host wave's own (the upper half reads lane 0 of the lower
    /// one), for one console wave and for two in a group (the BVH builder's prefix programs).
    #[test]
    fn the_first_lane_of_a_console_wave_is_read_by_both_its_host_waves() {
        for threads in [64u32, 128] {
            let native = first_lane(threads);
            let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
            let shader = emitted(&native, &p).unwrap_or_else(|e| panic!("{threads} threads: {e}"));
            validated(&shader);
            let Some(got) = group_of(&shader, 0) else { return };
            for t in 0..threads {
                assert_eq!(got[t as usize], 3 * (t & !63) + 7, "{threads} threads, thread {t}");
            }
        }
        eprintln!("ran on the hardware device: the console wave's first lane for both halves");
    }
}

