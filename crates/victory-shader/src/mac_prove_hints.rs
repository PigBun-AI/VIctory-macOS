//! S-I-002 (docs/mac/PATCHES.md G12): the scalar hint instructions the IR drops, and what it does with every other
//! SOPP instruction.
//!
//! Reference: AMD "RDNA 2 Instruction Set Architecture" (Nov 2020) section "SOPP: Scalar Program Control" and the
//! "Data dependency resolution" notes, with the names and opcodes of LLVM's AMDGPU assembler (the S-I-001 golden): an
//! instruction of the hint class changes no register, memory or control flow, so dropping it is exact when the host
//! runs each instruction to completion in order; every other SOPP instruction is control flow, a message to the
//! hardware, a wave state change or a trap, and has to be translated, or refused.
use super::*;

/// One SOPP instruction with a zero operand, then `s_endpgm`: what the IR makes of it.
fn outcome(opcode: u32) -> String {
    let code: Vec<u8> = [0xBF80_0000u32 | opcode << 16, 0xBF81_0000].iter().flat_map(|w| w.to_le_bytes()).collect();
    match build(&code, false, 256) {
        Err(e) => format!("refused: {e}"),
        Ok(p) => {
            let names: Vec<_> = p.blocks.iter().flat_map(|b| b.insts.iter().map(|i| i.name())).collect();
            let exit = format!("{:?}", p.blocks[0].exit);
            if names.is_empty() { format!("dropped, {exit}") } else { format!("kept {names:?}, {exit}") }
        }
    }
}

/// SOPP opcode -> LLVM's name (both RDNA targets), from the S-I-001 golden.
fn llvm_sopp() -> std::collections::BTreeMap<u32, &'static str> {
    let mut names = std::collections::BTreeMap::new();
    for text in [
        include_str!("../tests/data/gfx10_llvm_gfx1010_2.txt"),
        include_str!("../tests/data/gfx10_llvm_gfx1030_2.txt"),
    ] {
        for l in text.lines() {
            let f: Vec<&str> = l.split(' ').collect();
            let (t, count, variant) = (u32::from_str_radix(f[0], 16).unwrap(), f[1].parse::<u32>().unwrap(), f[2]);
            if variant == "0" && (0xBF80..0xC000).contains(&t) {
                for head in t..t + count {
                    names.entry(head - 0xBF80).or_insert(f[3]);
                }
            }
        }
    }
    names
}

/// What the ISA document says each SOPP instruction does: H = no architectural effect on registers, memory or control
/// flow (a hint, performance or debug aid); C = control flow; S = ends the wave; M = a message to the hardware, a trap
/// or a wave state change the host cannot do (to be translated where the host has the thing, refused otherwise: never
/// dropped). By name, so that opcode numbers come from the golden and not from this list.
fn isa_class(name: &str) -> Option<char> {
    Some(match name {
        "s_nop" | "s_waitcnt" | "s_waitcnt_vscnt" | "s_sleep" | "s_setprio" | "s_icache_inv" | "s_incperflevel" | "s_decperflevel"
        | "s_ttracedata" | "s_ttracedata_imm" | "s_code_end" | "s_inst_prefetch" | "s_clause" | "s_wait_idle" | "s_waitcnt_depctr"
        | "s_waitcnt_vmcnt" | "s_waitcnt_expcnt" | "s_waitcnt_lgkmcnt" | "s_subvector_loop_begin" | "s_subvector_loop_end" => 'H',
        "s_branch" | "s_cbranch_scc0" | "s_cbranch_scc1" | "s_cbranch_vccz" | "s_cbranch_vccnz" | "s_cbranch_execz" | "s_cbranch_execnz"
        | "s_cbranch_cdbgsys" | "s_cbranch_cdbguser" | "s_cbranch_cdbgsys_or_user" | "s_cbranch_cdbgsys_and_user" => 'C',
        "s_endpgm" | "s_endpgm_saved" | "s_endpgm_ordered_ps_done" => 'S',
        "s_wakeup" | "s_barrier" | "s_setkill" | "s_sethalt" | "s_sendmsg" | "s_sendmsghalt" | "s_trap" | "s_round_mode" | "s_denorm_mode" => 'M',
        _ => return None,
    })
}

#[test]
fn every_sopp_instruction_is_dropped_translated_or_refused_as_the_isa_says() {
    let mut bad = Vec::new();
    let names = llvm_sopp();
    assert!(names.len() > 30, "{} SOPP opcodes in the golden", names.len());
    for (&op, &name) in &names {
        let Some(class) = isa_class(name) else {
            bad.push(format!("{op}: {name} is not classified"));
            continue;
        };
        // our decoder must name it the same
        let head = (0xBF80_0000u32 | op << 16).to_le_bytes();
        let ours = crate::gfx10::decode(&[head, 0xBF81_0000u32.to_le_bytes()].concat()).ok().and_then(|v| v.first().and_then(|i| i.name()));
        if ours != Some(name) {
            bad.push(format!("{op}: decoder says {ours:?}, LLVM {name}"));
            continue;
        }
        let out = outcome(op);
        let ok = match class {
            // dropped, or kept as an instruction with no effect: never refused (a refusal loses a whole shader for nothing)
            'H' => !out.starts_with("refused") && !out.contains("Branch"),
            'C' => !out.starts_with("refused"),
            'S' => out.contains(", End"),
            // not silently dropped
            'M' => !out.starts_with("dropped"),
            _ => unreachable!(),
        };
        if !ok {
            bad.push(format!("{name} ({class}): {out}"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// The hints that the IR drops are exactly the ones the ISA calls effect-free, and vice versa nothing effectful is
/// in the dropped list (any encoding: SOPP, SOPK s_waitcnt_*, VOP1 v_nop and friends).
#[test]
fn the_dropped_list_holds_only_hints() {
    for name in DROPPED {
        assert_eq!(isa_class(name).or(matches!(*name, "v_nop" | "v_pipeflush" | "v_clrexcp").then_some('H')), Some('H'), "{name} is dropped");
    }
}
