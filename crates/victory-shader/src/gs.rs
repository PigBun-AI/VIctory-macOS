//! Mac patch S4 (docs/mac/PATCHES.md): the title's geometry shaders.
//!
//! A geometry shader of the console's legacy on-chip pipeline (a hardware GS, wave64 on GFX10) runs one
//! primitive per lane. The vertex stage before it (the ES) leaves its outputs in the ESGS ring, which is
//! LDS: vertex `k` of the lane's primitive starts at the dword offset the hardware puts in the first
//! VGPRs (`v0`: vertices 0 and 1 in its low and high half, `v1`: vertex 2 and 3), attribute `row`, channel
//! `ch` of it at dword `4 row + ch` from there. The program writes the vertices it emits into its own
//! slice of LDS, `stride` dwords apart (every output register is four dwords whether it is used or not),
//! and `s_sendmsg GS_OP_EMIT` sends the vertex out, `GS_OP_CUT` ends the strip, `MSG_GS_DONE` the program.
//!
//! The host has none of that: its geometry shader reads `loadInput(element, row, column, vertex)`, writes
//! `storeOutput` and calls `emitStream` and `cutStream`. This module finds what each LDS access of a
//! program means in those terms, so that the lowering can replace it ([`Ring`]). The address of a read is
//! found by evaluating the program's integer arithmetic with offsets of our own choosing for the three
//! vertices (`vertex_offset`): the value that comes out says which vertex and which dword. The address
//! of a write is the lane's slice (a multiple of the slice size by the lane index, plus values the program
//! loads), and the dword is its immediate offset.
//!
//! shortcut: straight-line programs only (no branch; the 195 of the title are), reads of vertices 0-3 by
//! 32-bit loads, writes by 32-bit stores whose base is the lane's slice and whose immediate offsets pick
//! the vertex (an emitted vertex's dwords are at `vertex * stride`). A program that is none of these is
//! refused, not guessed.

use std::collections::HashMap;

use crate::ShaderError;
use crate::gfx10::{Fields, Operand};
use crate::ir::{Program, Reg, Value};
use crate::resource::Site;

/// `s_sendmsg` message ids: a geometry shader's emit/cut, and the end of the program.
pub const MSG_GS: u16 = 2;
pub const MSG_GS_DONE: u16 = 3;

/// The ring offset (in dwords) the evaluation gives vertex `k`: far enough apart that no immediate
/// offset reaches the next.
pub const fn vertex_offset(k: u32) -> u32 {
    (k + 1) << 10
}

/// What the hardware puts in `v0` and `v1` (vertices 0, 1 and 2, 3), as the evaluation sees them.
pub const fn vertex_pair(first: u32) -> u32 {
    vertex_offset(first) | vertex_offset(first + 1) << 16
}

/// `s3` as the evaluation and the lowering give it: 64 primitives in the wave (bits 8-15), no other
/// field read by the program but through `s_bfe`s that this value answers as the hardware does.
pub const WAVE_INFO: u32 = 64 << 8;

/// One dword read from the ESGS ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Read {
    /// The VGPR that receives it.
    pub dst: u8,
    pub vertex: u8,
    /// Dword within the vertex: `4 row + channel`.
    pub dword: u16,
}

/// One dword written to the lane's output slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Write {
    /// The VGPR it comes from, and the value that register holds there.
    pub src: u8,
    pub value: Value,
    pub vertex: u8,
    /// Dword within the vertex: `4 row + channel`.
    pub dword: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    Emit,
    Cut,
    /// `GS_OP_EMIT_CUT`.
    EmitCut,
    Done,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ring {
    pub reads: HashMap<Site, Vec<Read>>,
    pub writes: HashMap<Site, Vec<Write>>,
    pub messages: HashMap<Site, Message>,
    /// Dwords per emitted vertex (four per output register).
    pub stride: u32,
    /// Vertices the program emits: the `GS_OP_EMIT`s in its code.
    pub vertices: u32,
}

/// What a value is, as far as the addresses go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Abs {
    Const(u32),
    /// The lane index.
    Lane,
    /// `slice * lane + something uniform`, plus `add` bytes.
    Slice { bytes: u32, add: u32 },
    Top,
}

struct Eval<'a> {
    p: &'a Program,
    defs: HashMap<Value, (usize, usize)>,
    memo: HashMap<Value, Abs>,
}

/// `p` with the ring accesses analysed ([`analyse`]) and the arithmetic that only addressed them removed,
/// and what they mean. The addresses (a table the hardware keeps the ring's sizes in, the lane's slice)
/// are of no use to the host, and the loads behind them trace to no root parameter.
pub fn prepare(mut p: Program) -> Result<(Program, Ring), ShaderError> {
    let mut ring = analyse(&p)?;
    // The program sets EXEC from the wave's info (`s_bfm_b64 exec`, `s_cselect_b64 exec, -1, exec`): the first
    // primitives of the wave run. Each host thread is one primitive that runs, so EXEC stays what the hardware
    // started with: the writes go, and what read their results reads the starting value.
    let mut start: HashMap<Reg, Value> = HashMap::new();
    for &(reg, v) in &p.inputs {
        if matches!(reg, Reg::S(126) | Reg::S(127)) {
            start.insert(reg, v);
        }
    }
    // (Where the program writes it first, the hardware's value is a new input.)
    let mut next = p
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .flat_map(|i| i.reads.iter().chain(&i.writes))
        .map(|r| r.1 .0)
        .chain(p.inputs.iter().map(|i| i.1 .0))
        .max()
        .map_or(0, |m| m + 1);
    for reg in [Reg::S(126), Reg::S(127)] {
        if !start.contains_key(&reg) {
            start.insert(reg, Value(next));
            p.inputs.push((reg, Value(next)));
            next += 1;
        }
    }
    let mut same: HashMap<Value, Value> = HashMap::new();
    for inst in &p.blocks[0].insts {
        for &(reg, v) in &inst.writes {
            if let Reg::S(126 | 127) = reg {
                same.insert(v, *start.get(&reg).ok_or(ShaderError::Unsupported {
                    offset: inst.machine.offset,
                    what: "a write to EXEC before any read of it",
                })?);
            }
        }
    }
    let insts = std::mem::take(&mut p.blocks[0].insts);
    let mut insts: Vec<crate::ir::Inst> = insts;
    for inst in &mut insts {
        for r in &mut inst.reads {
            while let Some(&s) = same.get(&r.1) {
                r.1 = s;
            }
        }
    }
    // Backwards: an instruction is needed if it sends a message, or a value it writes is needed. A ring
    // access needs its data only, not its address.
    let mut needed: std::collections::HashSet<Value> = std::collections::HashSet::new();
    let mut keep = vec![false; insts.len()];
    for (i, inst) in insts.iter().enumerate().rev() {
        let ds = matches!(inst.fields, Fields::Ds { .. });
        let write = ring.writes.contains_key(&(0, i));
        let by_value = inst.writes.iter().any(|w| needed.contains(&w.1));
        let effect = matches!(inst.fields, Fields::Sopp { .. });
        if !(by_value || effect || write) {
            continue;
        }
        keep[i] = true;
        if ds {
            if let Some(ws) = ring.writes.get(&(0, i)) {
                needed.extend(ws.iter().map(|w| w.value));
            }
        } else {
            needed.extend(inst.reads.iter().map(|r| r.1));
        }
    }
    let mut remap = vec![usize::MAX; insts.len()];
    let mut kept = Vec::new();
    for (i, inst) in insts.into_iter().enumerate() {
        if keep[i] {
            remap[i] = kept.len();
            kept.push(inst);
        }
    }
    p.blocks[0].insts = kept;
    ring.reads = ring
        .reads
        .into_iter()
        .filter(|(s, _)| remap[s.1] != usize::MAX)
        .map(|(s, v)| ((0, remap[s.1]), v))
        .collect();
    ring.writes = ring
        .writes
        .into_iter()
        .filter(|(s, _)| remap[s.1] != usize::MAX)
        .map(|(s, v)| ((0, remap[s.1]), v))
        .collect();
    ring.messages = ring
        .messages
        .into_iter()
        .filter(|(s, _)| remap[s.1] != usize::MAX)
        .map(|(s, v)| ((0, remap[s.1]), v))
        .collect();
    Ok((p, ring))
}

/// The ring accesses and messages of `p`.
pub fn analyse(p: &Program) -> Result<Ring, ShaderError> {
    let unsupported = |offset, what| ShaderError::Unsupported { offset, what };
    if p.blocks.len() != 1 {
        return Err(unsupported(0, "a geometry shader with branches"));
    }
    let mut e = Eval {
        p,
        defs: p
            .blocks
            .iter()
            .enumerate()
            .flat_map(|(b, block)| {
                block
                    .insts
                    .iter()
                    .enumerate()
                    .flat_map(move |(i, inst)| inst.writes.iter().map(move |w| (w.1, (b, i))))
            })
            .collect(),
        memo: HashMap::new(),
    };
    let mut ring = Ring::default();
    // The message sites first: the stride needs the number of emits.
    for (i, inst) in p.blocks[0].insts.iter().enumerate() {
        if inst.name() != "s_sendmsg" {
            continue;
        }
        let Fields::Sopp { imm } = inst.fields else {
            continue;
        };
        let at = inst.machine.offset;
        let (id, op, stream) = (imm & 0xF, imm >> 4 & 3, imm >> 8 & 3);
        let m = match (id, op) {
            (MSG_GS, 2) => Message::Emit,
            (MSG_GS, 1) => Message::Cut,
            (MSG_GS, 3) => Message::EmitCut,
            (MSG_GS_DONE, _) => Message::Done,
            // The other messages (allocation requests...) are no part of this stage.
            _ => return Err(unsupported(at, "a message a geometry shader does not send")),
        };
        if stream != 0 && id == MSG_GS {
            return Err(unsupported(at, "a geometry shader stream other than 0"));
        }
        ring.messages.insert((0, i), m);
    }
    ring.vertices = ring
        .messages
        .values()
        .filter(|m| matches!(m, Message::Emit | Message::EmitCut))
        .count() as u32;
    if ring.vertices == 0 {
        return Err(unsupported(0, "a geometry shader that emits nothing"));
    }
    let mut slice = None;
    for (i, inst) in p.blocks[0].insts.iter().enumerate() {
        let Fields::Ds {
            dst,
            addr,
            data,
            offset0,
            offset1,
            gds,
        } = &inst.fields
        else {
            continue;
        };
        let at = inst.machine.offset;
        let name = inst.name();
        if *gds {
            return Err(unsupported(at, "GDS"));
        }
        let Some(Operand::Vector { reg: areg, .. }) = *addr else {
            return Err(unsupported(at, "a ring access without an address"));
        };
        let a = inst
            .read(Reg::V(areg))
            .map_or(Abs::Top, |v| e.eval(v));
        let (offset0, offset1) = (u32::from(*offset0), u32::from(*offset1));
        let wide = offset0 | offset1 << 8;
        if name.starts_with("ds_read") {
            let Some(Operand::Vector { reg: d, dwords }) = *dst else {
                return Err(unsupported(at, "a ring read without a destination"));
            };
            // (dword offsets of the loads, dwords each)
            let (offsets, per): (Vec<u32>, u32) = match name {
                "ds_read_b32" => (vec![wide / 4], 1),
                "ds_read2_b32" => (vec![offset0, offset1], 1),
                _ => return Err(unsupported(at, "a ring read of this width")),
            };
            if u32::from(dwords) != offsets.len() as u32 * per || (name == "ds_read_b32" && wide % 4 != 0) {
                return Err(unsupported(at, "a ring read of this width"));
            }
            let Abs::Const(base) = a else {
                return Err(unsupported(at, "a ring read at an address that is no vertex's"));
            };
            let mut reads = Vec::new();
            for (n, off) in offsets.into_iter().enumerate() {
                let dword = base / 4 + off;
                let (k, within) = (dword >> 10, dword & 0x3FF);
                if k == 0 || k > 4 {
                    return Err(unsupported(at, "a ring read outside the vertices of the primitive"));
                }
                reads.push(Read {
                    dst: d + n as u8,
                    vertex: (k - 1) as u8,
                    dword: within as u16,
                });
            }
            ring.reads.insert((0, i), reads);
        } else if name.starts_with("ds_write") {
            let (offsets, count): (Vec<u32>, usize) = match name {
                "ds_write_b32" if wide % 4 == 0 => (vec![wide / 4], 1),
                "ds_write2_b32" => (vec![offset0, offset1], 2),
                _ => return Err(unsupported(at, "a ring write of this width")),
            };
            let Abs::Slice { bytes, add } = a else {
                return Err(unsupported(at, "a ring write at an address that is no lane's slice"));
            };
            if add % 4 != 0 {
                return Err(unsupported(at, "an unaligned ring write"));
            }
            match slice {
                None => slice = Some(bytes),
                Some(s) if s == bytes => {}
                Some(_) => return Err(unsupported(at, "ring writes to slices of two sizes")),
            }
            if data.len() != count {
                return Err(unsupported(at, "a ring write's data"));
            }
            // Each write needs the stride: filled in below, once the slice is known.
            let writes = offsets
                .iter()
                .zip(data)
                .map(|(&off, d)| {
                    let Operand::Vector { reg, .. } = *d else {
                        return Err(unsupported(at, "a ring write's data"));
                    };
                    let value = inst.read(Reg::V(reg)).ok_or(unsupported(at, "a ring write's data"))?;
                    Ok(Write {
                        src: reg,
                        value,
                        vertex: 0,
                        dword: (off + add / 4) as u16,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            ring.writes.insert((0, i), writes);
        }
    }
    // The slice holds the emitted vertices one after the other.
    if let Some(bytes) = slice {
        let dwords = bytes / 4;
        if dwords % ring.vertices != 0 || dwords / ring.vertices == 0 || (dwords / ring.vertices) % 4 != 0 {
            return Err(unsupported(0, "a ring slice that is no whole number of output vertices"));
        }
        ring.stride = dwords / ring.vertices;
        for writes in ring.writes.values_mut() {
            for w in writes {
                let at = u32::from(w.dword);
                if at >= dwords {
                    return Err(unsupported(0, "a ring write outside the lane's slice"));
                }
                w.vertex = (at / ring.stride) as u8;
                w.dword = (at % ring.stride) as u16;
            }
        }
    }
    Ok(ring)
}

impl Eval<'_> {
    fn eval(&mut self, v: Value) -> Abs {
        if let Some(&a) = self.memo.get(&v) {
            return a;
        }
        let a = self.compute(v);
        self.memo.insert(v, a);
        a
    }

    fn compute(&mut self, v: Value) -> Abs {
        if let Some(&(reg, _)) = self.p.inputs.iter().find(|i| i.1 == v) {
            return match reg {
                Reg::V(0) => Abs::Const(vertex_pair(0)),
                Reg::V(1) => Abs::Const(vertex_pair(2)),
                Reg::S(3) => Abs::Const(WAVE_INFO),
                _ => Abs::Top,
            };
        }
        let Some(&(b, i)) = self.defs.get(&v) else {
            return Abs::Top;
        };
        let inst = &self.p.blocks[b].insts[i];
        let name = inst.name();
        let (src, sdwa) = match &inst.fields {
            Fields::Valu(x) => (x.src.clone(), x.sdwa),
            Fields::Sop { src, .. } => (src.clone(), None),
            _ => return Abs::Top,
        };
        let arg = |this: &mut Self, k: usize| -> Abs {
            match src.get(k) {
                Some(&Operand::Const(c @ 128..=192)) => Abs::Const(u32::from(c) - 128),
                Some(&Operand::Const(c @ 193..=208)) => Abs::Const((-(i32::from(c) - 192)) as u32),
                Some(&Operand::Literal(l)) => Abs::Const(l),
                Some(&Operand::Vector { reg, .. }) => {
                    inst.read(Reg::V(reg)).map_or(Abs::Top, |v| this.eval(v))
                }
                Some(&Operand::Scalar { code, .. }) => {
                    inst.read(Reg::S(code)).map_or(Abs::Top, |v| this.eval(v))
                }
                _ => Abs::Top,
            }
        };
        // SDWA selects of a source (the word the instruction means).
        let select = |x: Abs, k: usize| -> Abs {
            let Some(sd) = sdwa else { return x };
            let Abs::Const(c) = x else {
                return if sd.src_sel[k] == 6 { x } else { Abs::Top };
            };
            Abs::Const(match sd.src_sel[k] {
                n @ 0..=3 => c >> (8 * n) & 0xFF,
                4 => c & 0xFFFF,
                5 => c >> 16,
                _ => c,
            })
        };
        use Abs::*;
        match name {
            "v_mov_b32" | "s_mov_b32" | "v_mov_b32_e32" => {
                let x = arg(self, 0);
                select(x, 0)
            }
            "v_lshlrev_b32" | "v_lshlrev_b32_e32" | "s_lshl_b32" => {
                let (s, x) = if name == "s_lshl_b32" {
                    (arg(self, 1), arg(self, 0))
                } else {
                    (arg(self, 0), arg(self, 1))
                };
                let (x, s) = (select(x, if name == "s_lshl_b32" { 0 } else { 1 }), select(s, 0));
                match (s, x) {
                    (Const(s), Const(x)) => Const(x << (s & 31)),
                    _ => Top,
                }
            }
            "v_lshrrev_b32" | "s_lshr_b32" => {
                let (s, x) = if name == "s_lshr_b32" {
                    (arg(self, 1), arg(self, 0))
                } else {
                    (arg(self, 0), arg(self, 1))
                };
                match (s, x) {
                    (Const(s), Const(x)) => Const(x >> (s & 31)),
                    _ => Top,
                }
            }
            "v_and_b32" | "s_and_b32" => match (arg(self, 0), arg(self, 1)) {
                (Const(a), Const(b)) => Const(a & b),
                _ => Top,
            },
            "v_or_b32" | "s_or_b32" => match (arg(self, 0), arg(self, 1)) {
                (Const(a), Const(b)) => Const(a | b),
                _ => Top,
            },
            "v_add_nc_u32" | "v_add_u32" | "s_add_i32" | "s_add_u32" => match (arg(self, 0), arg(self, 1)) {
                (Const(a), Const(b)) => Const(a.wrapping_add(b)),
                (Slice { bytes, add }, Const(c)) | (Const(c), Slice { bytes, add }) => Slice {
                    bytes,
                    add: add.wrapping_add(c),
                },
                _ => Top,
            },
            // The lane index: the low and then the high half of the all-ones mask's count.
            "v_mbcnt_lo_u32_b32" => match (arg(self, 0), arg(self, 1)) {
                (Const(u32::MAX), Const(0)) => Lane,
                _ => Top,
            },
            "v_mbcnt_hi_u32_b32" => match (arg(self, 0), arg(self, 1)) {
                (Const(u32::MAX), Lane) => Lane,
                _ => Top,
            },
            // The lane's slice: the slice size (bytes) times the lane, plus a base the program loads.
            "v_mad_u32_u24" => match (arg(self, 0), arg(self, 1)) {
                (Const(k), Lane) | (Lane, Const(k)) => Slice { bytes: k, add: 0 },
                _ => Top,
            },
            _ => Top,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `v_lshlrev_b32 v2, 2, v1; v_and_b32 v2, 0x3fffc, v2; ds_read_b32 v3, v2 offset:8;
    /// s_sendmsg(GS, EMIT); s_sendmsg(GS_DONE); s_endpgm`: the read of the third vertex's dword 2.
    fn read_program() -> Program {
        let words: [u32; 8] = [
            0x1A << 25 | 2 << 17 | 1 << 9 | 130,
            0x1B << 25 | 2 << 17 | 2 << 9 | 255,
            0x0003_FFFC,
            0b110110 << 26 | 0x36 << 18 | 8,
            3 << 24 | 2,
            0xBF90_0000 | 0x22,
            0xBF90_0000 | 3,
            0xBF81_0000,
        ];
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        crate::ir::build(&code, true, 8).expect("the program builds")
    }

    #[test]
    fn a_ring_read_is_the_dword_of_the_vertex_its_address_names() {
        let ring = analyse(&read_program()).expect("analysed");
        let reads: Vec<_> = ring.reads.values().flatten().collect();
        assert_eq!(reads, [&Read { dst: 3, vertex: 2, dword: 2 }]);
        assert_eq!(ring.vertices, 1);
        let mut messages: Vec<_> = ring.messages.values().copied().collect();
        messages.sort_by_key(|m| *m as u8);
        assert_eq!(messages, [Message::Emit, Message::Done]);
    }

    #[test]
    fn a_program_that_emits_nothing_is_refused() {
        let words: [u32; 2] = [0xBF90_0000 | 3, 0xBF81_0000];
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let p = crate::ir::build(&code, true, 8).unwrap();
        assert!(analyse(&p).is_err());
    }

    #[test]
    fn the_ring_arithmetic_that_only_addressed_goes_and_so_does_a_read_nothing_uses() {
        let (p, ring) = prepare(read_program()).expect("prepared");
        let names: Vec<_> = p.blocks[0].insts.iter().map(|i| i.name()).collect();
        // the address arithmetic is no use to the host; the read stays for its destination
        assert!(!names.contains(&"v_lshlrev_b32") && !names.contains(&"v_and_b32"), "{names:?}");
        assert_eq!(ring.reads.len(), 0, "a read nothing uses is dropped with its address");
        assert!(names.iter().all(|n| n.starts_with("s_")));
    }

    #[test]
    fn the_vertices_of_the_evaluation_are_far_apart() {
        assert_eq!(vertex_pair(0), 0x0800_0400);
        assert_eq!(vertex_pair(2), 0x1000_0C00);
        assert!((0..4).all(|k| vertex_offset(k) < 0x10000));
        // One dword of a vertex's attribute is within the room of the next vertex's offset.
        assert!(vertex_offset(1) - vertex_offset(0) > 255 + 4 * 32);
    }
}
