//! Making a mesh shader's output fit Metal's 32 KB (docs/metal/mesh32k.md).
//!
//! Metal counts `round_up(max_vertices, 4) * vertex_stride + max_primitives *
//! primitive_stride` against 32768 bytes, where the vertex struct holds its
//! members in the order of the DXIL's output signature **ids**, each member
//! aligned to its own size ([`crate::mesh_size`]). A shader that declares
//! 128 vertices and a `float` between each of its `float3`s pays four bytes
//! of padding per vertex for them, 512 bytes per three outputs, and 520 of
//! the 8,765 translated mesh shaders of the title are refused only or mostly
//! for that. [`repack_outputs`] renumbers the output elements so that the
//! struct has the least padding; the declared maxima and every value
//! written stay as they are, and the pixel shader finds its inputs by
//! semantic, so what is drawn does not change.
//!
//! What is edited, in the bitcode of the `DXIL` part (everything else of
//! the container stays; the container hash does too, which the converter
//! does not check):
//!
//! * the element nodes of `!dx.entryPoints`' output signature: the `i32`
//!   id is the first operand, and the list's order follows the new ids;
//! * the second operand of every `dx.op.storeVertexOutput` call (opcode
//!   171), which names the element: a constant, replaced by the constant
//!   of the new id. Operands are relative value numbers, so the numbering of
//!   the function's values is followed ([`Numbering`]);
//! * the function offsets the module's symbol table keeps, which move with
//!   the bytes.
//!
//! Anything the code does not understand (an instruction it cannot number,
//! a signature of matrices, 64-bit outputs, a node used twice, a store whose
//! element is not a constant) is an error and the shader is left alone.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::bitcode::{Bitcode, Block, Item};
use crate::device::Device;
use crate::mesh_size::{self, Output};
use crate::msc::MscError;
use crate::render::{GraphicsDesc, GraphicsPipeline};

/// Why a shader was not repacked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FitError {
    /// The container or its bitcode is malformed.
    Malformed(String),
    /// The shader uses something this code does not edit.
    Unsupported(String),
}

impl fmt::Display for FitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FitError::Malformed(m) => write!(f, "malformed DXIL: {m}"),
            FitError::Unsupported(m) => write!(f, "mesh outputs not repacked: {m}"),
        }
    }
}

impl std::error::Error for FitError {}

impl From<crate::bitcode::Error> for FitError {
    fn from(e: crate::bitcode::Error) -> FitError {
        FitError::Malformed(e.to_string())
    }
}

fn bad<T>(m: impl Into<String>) -> Result<T, FitError> {
    Err(FitError::Malformed(m.into()))
}

fn unsupported<T>(m: impl Into<String>) -> Result<T, FitError> {
    Err(FitError::Unsupported(m.into()))
}

/// A repacked shader.
#[derive(Clone, Debug)]
pub struct Fit {
    /// The new container.
    pub dxil: Vec<u8>,
    /// Bytes of one vertex before and after (Metal's layout).
    pub stride_before: u32,
    pub stride_after: u32,
    /// Output elements, of which `dropped` were taken out; `storeVertexOutput`
    /// calls edited (the stores of dropped ones are removed besides).
    pub elements: usize,
    pub dropped: usize,
    pub stores: usize,
}

/// Pipelines made from a repacked mesh shader since the start (for the
/// counters and the tests).
static REPACKED: AtomicU64 = AtomicU64::new(0);

pub fn repacked() -> u64 {
    REPACKED.load(Ordering::Relaxed)
}

/// Whether Metal refused a mesh pipeline for the size of its output.
pub fn is_size_refusal(e: &MscError) -> bool {
    matches!(e, MscError::Limit(m) if m.contains("Total mesh size"))
}

/// `desc` with another mesh stage.
fn with_ms<'b>(d: &GraphicsDesc<'b>, ms: &'b [u8]) -> GraphicsDesc<'b> {
    GraphicsDesc {
        vs: None,
        ps: d.ps,
        amp: d.amp,
        ms: Some(ms),
        gs: None,
        root: d.root,
        inputs: d.inputs.clone(),
        targets: d.targets.clone(),
        topology: d.topology,
        sample_count: d.sample_count,
        sample_mask: d.sample_mask,
        alpha_to_coverage: d.alpha_to_coverage,
        has_depth_target: d.has_depth_target,
        depth_stencil: d.depth_stencil,
        raster: d.raster,
        compat: d.compat,
    }
}

/// The second try of a mesh pipeline Metal refused for its output size
/// (`first` is that refusal): the same pipeline with the vertex outputs
/// repacked. `None` when the refusal is of something else or nothing can
/// be gained; the caller keeps `first` then.
pub fn retry(
    device: &Device,
    desc: &GraphicsDesc<'_>,
    first: &MscError,
) -> Option<Result<GraphicsPipeline, MscError>> {
    if !is_size_refusal(first) {
        return None;
    }
    let ms = desc.ms?;
    // Packed tighter first (nothing changes but the order); when that is not
    // enough, without the outputs the pixel shader has no input for.
    let mut last = None;
    let pixel = desc.ps.map_or(Consumer::NoPixelShader, Consumer::Pixel);
    for consumer in [Consumer::Unknown, pixel] {
        let Some(fit) = repack_outputs(ms, consumer).ok().flatten() else {
            continue;
        };
        let made = device.graphics_pipeline_from_dxil(&with_ms(desc, &fit.dxil));
        match &made {
            Ok(_) => {
                REPACKED.fetch_add(1, Ordering::Relaxed);
                return Some(made);
            }
            Err(e) if is_size_refusal(e) => last = Some(made),
            Err(_) => return Some(made),
        }
    }
    last
}

// ---- the container -------------------------------------------------------

const DXIL: [u8; 4] = *b"DXIL";

struct Parts<'a> {
    head: &'a [u8],
    parts: Vec<([u8; 4], &'a [u8])>,
}

fn word(b: &[u8], at: usize) -> Result<u32, FitError> {
    b.get(at..at + 4)
        .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
        .ok_or_else(|| FitError::Malformed("a container cut short".into()))
}

fn split(c: &[u8]) -> Result<Parts<'_>, FitError> {
    if c.get(..4) != Some(b"DXBC") {
        return bad("not a DXBC container");
    }
    let count = word(c, 28)? as usize;
    let mut parts = Vec::new();
    for i in 0..count {
        let at = word(c, 32 + 4 * i)? as usize;
        let size = word(c, at + 4)? as usize;
        let fourcc: [u8; 4] = c
            .get(at..at + 4)
            .and_then(|f| f.try_into().ok())
            .ok_or_else(|| FitError::Malformed("a part cut short".into()))?;
        let data = c
            .get(at + 8..at + 8 + size)
            .ok_or_else(|| FitError::Malformed("a part past the end".into()))?;
        parts.push((fourcc, data));
    }
    Ok(Parts {
        head: &c[..24],
        parts,
    })
}

fn join(p: &Parts<'_>, dxil: &[u8]) -> Vec<u8> {
    let n = p.parts.len();
    let mut offsets = Vec::new();
    let mut body = Vec::new();
    let base = 32 + 4 * n;
    for (cc, data) in &p.parts {
        let data = if *cc == DXIL { dxil } else { data };
        offsets.push((base + body.len()) as u32);
        body.extend_from_slice(cc);
        body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        body.extend_from_slice(data);
    }
    let mut out = p.head.to_vec();
    out.extend_from_slice(&((base + body.len()) as u32).to_le_bytes());
    out.extend_from_slice(&(n as u32).to_le_bytes());
    for o in offsets {
        out.extend_from_slice(&o.to_le_bytes());
    }
    out.extend_from_slice(&body);
    out
}

/// The program header (24 bytes: version, size in words, `DXIL`, DXIL
/// version, bitcode offset from the `DXIL` word, bitcode size) and the
/// bitcode.
fn bitcode_of(part: &[u8]) -> Result<(&[u8], &[u8]), FitError> {
    if part.get(8..12) != Some(b"DXIL") {
        return bad("a DXIL part without the DXIL header");
    }
    let off = word(part, 16)? as usize;
    let size = word(part, 20)? as usize;
    let at = 8 + off;
    let bc = part
        .get(at..at + size)
        .ok_or_else(|| FitError::Malformed("bitcode past its part".into()))?;
    Ok((&part[..at], bc))
}

fn new_part(head: &[u8], bitcode: &[u8]) -> Vec<u8> {
    let mut part = head.to_vec();
    part.extend_from_slice(bitcode);
    while !part.len().is_multiple_of(4) {
        part.push(0);
    }
    let words = (part.len() / 4) as u32;
    part[4..8].copy_from_slice(&words.to_le_bytes());
    part[20..24].copy_from_slice(&(bitcode.len() as u32).to_le_bytes());
    part
}

// ---- the module ----------------------------------------------------------

const MODULE: u32 = 8;
const CONSTANTS: u32 = 11;
const FUNCTION_BLOCK: u32 = 12;
const VST: u32 = 14;
const METADATA: u32 = 15;
const TYPES: u32 = 17;

/// The `storeVertexOutput` opcode of DXIL.
const STORE_VERTEX_OUTPUT: i64 = 171;

fn records(b: &Block) -> impl Iterator<Item = (usize, &crate::bitcode::Record)> {
    b.items.iter().enumerate().filter_map(|(i, it)| match it {
        Item::Record(r) => Some((i, r)),
        _ => None,
    })
}

fn blocks(b: &Block, id: u32) -> impl Iterator<Item = (usize, &Block)> {
    b.items
        .iter()
        .enumerate()
        .filter_map(move |(i, it)| match it {
            Item::Block(x) if x.id == id => Some((i, x)),
            _ => None,
        })
}

/// The type table: code and operands of each type.
fn types(module: &Block) -> Vec<(u64, Vec<u64>)> {
    let mut out = Vec::new();
    for (_, t) in blocks(module, TYPES) {
        for (_, r) in records(t) {
            // NUMENTRY and STRUCT_NAME define no type.
            if r.code != 1 && r.code != 19 {
                out.push((r.code, r.ops.clone()));
            }
        }
    }
    out
}

/// Integer constants by value number: type and value.
type Ints = HashMap<u64, (u64, i64)>;

/// Reads a constants block whose first value is number `first`; returns the
/// number of values it defines.
fn constants(b: &Block, first: u64, ints: &mut Ints) -> u64 {
    let mut ty = 0;
    let mut n = 0;
    for (_, r) in records(b) {
        if r.code == 1 {
            ty = r.ops.first().copied().unwrap_or(0);
            continue;
        }
        let id = first + n;
        n += 1;
        match r.code {
            2 => {
                ints.insert(id, (ty, 0));
            }
            4 => {
                let v = r.ops.first().copied().unwrap_or(0);
                let value = if v & 1 == 0 {
                    (v >> 1) as i64
                } else if v == 1 {
                    i64::MIN
                } else {
                    -((v >> 1) as i64)
                };
                ints.insert(id, (ty, value));
            }
            _ => {}
        }
    }
    n
}

enum Md {
    Str(Vec<u64>),
    Value { value: u64 },
    Node { item: usize, ops: Vec<u64> },
    Other,
}

/// The module's metadata: what `!dx.entryPoints` leads to.
struct Meta<'a> {
    mds: Vec<Md>,
    ints: &'a Ints,
    /// The first operand of the entry point list (metadata id).
    entry: u64,
    /// Index of the metadata block among the module's items.
    block: usize,
}

/// One element of an entry point's signature.
struct Element {
    /// The node's reference in the list (metadata id + 1).
    node: u64,
    id: i64,
    /// `DXIL::SemanticKind`: 0 arbitrary, 3 position, 6 clip, 7 cull distance.
    kind: i64,
    size: u32,
    cols: u32,
    name: String,
    index: i64,
}

impl<'a> Meta<'a> {
    /// The module's metadata block that names the entry point, if any.
    fn read(module: &Block, ints: &'a Ints) -> Option<Meta<'a>> {
        for (block, md) in blocks(module, METADATA) {
            let mut mds: Vec<Md> = Vec::new();
            let mut entry = None;
            let mut name: Vec<u64> = Vec::new();
            for (i, r) in records(md) {
                match r.code {
                    1 => mds.push(Md::Str(r.ops.clone())),
                    35 => {
                        for _ in 0..r.ops.first().copied().unwrap_or(0) {
                            mds.push(Md::Str(Vec::new()));
                        }
                    }
                    2 => mds.push(Md::Value {
                        value: *r.ops.get(1).unwrap_or(&u64::MAX),
                    }),
                    3 | 5 => mds.push(Md::Node {
                        item: i,
                        ops: r.ops.clone(),
                    }),
                    4 => name = r.ops.clone(),
                    10 => {
                        let n: String = name.iter().map(|&c| c as u8 as char).collect();
                        if n == "dx.entryPoints" {
                            entry = r.ops.first().copied();
                        }
                    }
                    6 | 36 | 38 | 39 => {}
                    _ => mds.push(Md::Other),
                }
            }
            if let Some(entry) = entry {
                return Some(Meta {
                    mds,
                    ints,
                    entry,
                    block,
                });
            }
        }
        None
    }

    fn node(&self, r: u64) -> Result<&Vec<u64>, FitError> {
        match r.checked_sub(1).and_then(|i| self.mds.get(i as usize)) {
            Some(Md::Node { ops, .. }) => Ok(ops),
            _ => bad("a metadata reference to something that is no node"),
        }
    }

    /// The record of a node: its item index in the metadata block.
    fn item(&self, r: u64) -> Result<usize, FitError> {
        match r.checked_sub(1).and_then(|i| self.mds.get(i as usize)) {
            Some(Md::Node { item, .. }) => Ok(*item),
            _ => bad("a metadata reference to something that is no node"),
        }
    }

    fn int(&self, r: u64) -> Result<i64, FitError> {
        match r.checked_sub(1).and_then(|i| self.mds.get(i as usize)) {
            Some(Md::Value { value }) => self.ints.get(value).map(|v| v.1).ok_or_else(|| {
                FitError::Unsupported("a metadata number that is no module constant".into())
            }),
            _ => bad("a metadata reference to something that is no number"),
        }
    }

    fn string(&self, r: u64) -> String {
        match r.checked_sub(1).and_then(|i| self.mds.get(i as usize)) {
            Some(Md::Str(c)) => c.iter().map(|&c| c as u8 as char).collect(),
            _ => String::new(),
        }
    }

    /// The input (`which` 0) or output (1) signature of the entry point:
    /// the list's reference and its elements.
    fn signature(&self, which: usize) -> Result<Option<(u64, Vec<Element>)>, FitError> {
        let entry = self.node(self.entry + 1)?;
        let Some(&sig) = entry.get(2).filter(|&&s| s != 0) else {
            return Ok(None);
        };
        let Some(&list) = self.node(sig)?.get(which).filter(|&&l| l != 0) else {
            return Ok(None);
        };
        let mut els = Vec::new();
        for &r in self.node(list)? {
            let ops = self.node(r)?;
            if ops.len() < 8 {
                return unsupported("an element node with fewer than eight operands");
            }
            let bytes = match self.int(ops[2])? {
                2 | 3 | 8 | 11 | 12 => 2,
                4 | 5 | 9 | 13 | 14 => 4,
                c => return unsupported(format!("an element of component type {c}")),
            };
            if self.int(ops[6])? != 1 {
                return unsupported("an element of several rows");
            }
            let cols = self.int(ops[7])?;
            if !(1..=4).contains(&cols) {
                return unsupported("an element of other than one to four columns");
            }
            let index = match ops.get(4) {
                Some(&n) if n != 0 => self.node(n)?.first().map_or(Ok(0), |&v| self.int(v))?,
                _ => 0,
            };
            els.push(Element {
                node: r,
                id: self.int(ops[0])?,
                kind: self.int(ops[3])?,
                size: cols as u32 * bytes,
                cols: cols as u32,
                name: self.string(ops[1]),
                index,
            });
        }
        Ok(Some((list, els)))
    }
}

/// The module level of a bitcode: types, constants, value numbers.
struct Module {
    index: usize,
    types: Vec<(u64, Vec<u64>)>,
    ints: Ints,
    values: u64,
    /// The function type of each function with a body, in order.
    defined: Vec<u64>,
}

fn module_of(bc: &Bitcode) -> Result<Module, FitError> {
    let Some(index) = bc.blocks.iter().position(|b| b.id == MODULE) else {
        return bad("no module block");
    };
    let module = &bc.blocks[index];
    let tys = types(module);
    let globals = records(module)
        .filter(|(_, r)| matches!(r.code, 7 | 8 | 9 | 14))
        .count() as u64;
    let mut ints = Ints::new();
    let mut values = globals;
    for (_, c) in blocks(module, CONSTANTS) {
        values += constants(c, values, &mut ints);
    }
    // (`FUNCTION` records name the function type, or a pointer to it.)
    let defined: Vec<u64> = records(module)
        .filter(|(_, r)| r.code == 8 && r.ops.get(2) == Some(&0))
        .map(|(_, r)| match tys.get(r.ops[0] as usize) {
            Some(t) if t.0 == 8 => t.1.first().copied().unwrap_or(0),
            _ => r.ops[0],
        })
        .collect();
    Ok(Module {
        index,
        types: tys,
        ints,
        values,
        defined,
    })
}

/// The `(name, index)` of the inputs of a pixel shader's container (lower
/// case names), or `None` when it cannot be read.
fn inputs_of(container: &[u8]) -> Option<std::collections::HashSet<(String, i64)>> {
    let parts = split(container).ok()?;
    let part = parts.parts.iter().find(|p| p.0 == DXIL)?.1;
    let (_, bitcode) = bitcode_of(part).ok()?;
    let bc = Bitcode::parse(bitcode).ok()?;
    let m = module_of(&bc).ok()?;
    let meta = Meta::read(&bc.blocks[m.index], &m.ints)?;
    let (_, els) = meta.signature(0).ok()??;
    Some(
        els.into_iter()
            .map(|e| (e.name.to_ascii_lowercase(), e.index))
            .collect(),
    )
}

/// Value numbers of one function block, followed instruction by instruction.
///
/// The numbers run: global variables, functions, aliases, the module's
/// constants, then per function its arguments, its constants, and each
/// instruction that makes a value. Operands of instructions are relative:
/// the number of the next value minus the operand's.
struct Numbering<'a> {
    types: &'a [(u64, Vec<u64>)],
}

impl Numbering<'_> {
    fn void(&self, ty: u64) -> bool {
        self.types.get(ty as usize).is_some_and(|t| t.0 == 2)
    }

    /// Whether the record makes a value; `index` of the callee for calls.
    fn produces(&self, code: u64, ops: &[u64]) -> Result<bool, FitError> {
        Ok(match code {
            2..=9 | 16 | 19 | 20 | 23 | 26..=30 | 37 | 38 | 40 | 41 | 43 | 46 | 47 => true,
            1 | 10 | 11 | 12 | 15 | 24 | 31 | 33 | 35 | 36 | 39 | 42 | 44 | 45 => false,
            34 => {
                let (fnty, _) = call_shape(ops)?;
                let t = self
                    .types
                    .get(fnty as usize)
                    .filter(|t| t.0 == 21)
                    .ok_or_else(|| {
                        FitError::Malformed("a call of a type that is no function".into())
                    })?;
                !self.void(*t.1.get(1).unwrap_or(&u64::MAX))
            }
            c => return unsupported(format!("instruction record {c}")),
        })
    }
}

/// Function type id and index of the callee operand of a `CALL` record
/// `[paramattrs, cc, (fmf), fnty, callee, args...]`.
fn call_shape(ops: &[u64]) -> Result<(u64, usize), FitError> {
    let cc = *ops
        .get(1)
        .ok_or_else(|| FitError::Malformed("a short call".into()))?;
    let mut at = 2;
    if cc >> 17 & 1 == 1 {
        at += 1;
    }
    if cc >> 15 & 1 == 0 {
        return unsupported("a call without its function type");
    }
    let fnty = *ops
        .get(at)
        .ok_or_else(|| FitError::Malformed("a short call".into()))?;
    Ok((fnty, at + 1))
}

/// Who reads a mesh shader's vertex outputs.
#[derive(Clone, Copy, Debug)]
pub enum Consumer<'a> {
    /// Not known: every output stays.
    Unknown,
    /// The pipeline has no pixel shader: only the position and the clip and
    /// cull distances are read.
    NoPixelShader,
    /// This pixel shader's container: what it has no input for is not read.
    Pixel(&'a [u8]),
}

/// Repacks the vertex outputs of a mesh shader's DXIL container: the
/// elements renumbered so that Metal's struct has the least padding.
///
/// With a known `consumer` the outputs nothing reads are dropped too (their
/// stores removed): Metal does not drop them (docs/metal/mesh32k.md).
///
/// `Ok(None)`: nothing to gain (no mesh outputs, or the order is already
/// the best this finds). `Err`: left alone, see [`FitError`].
pub fn repack_outputs(container: &[u8], consumer: Consumer<'_>) -> Result<Option<Fit>, FitError> {
    let parts = split(container)?;
    let part = parts
        .parts
        .iter()
        .find(|p| p.0 == DXIL)
        .ok_or_else(|| FitError::Malformed("no DXIL part".into()))?
        .1;
    let (head, bitcode) = bitcode_of(part)?;
    let mut bc = Bitcode::parse(bitcode)?;
    let before_layout = bc.write_placed()?;
    if before_layout.0 != bitcode {
        return unsupported("a stream this writer does not reproduce byte for byte");
    }
    let old_places = before_layout.1;
    let m = module_of(&bc)?;
    let mi = m.index;
    let read_input = match consumer {
        Consumer::Unknown => None,
        Consumer::NoPixelShader => Some(std::collections::HashSet::new()),
        Consumer::Pixel(ps) => inputs_of(ps),
    };

    let Some(plan) = plan(&bc, &m, read_input.as_ref())? else {
        return Ok(None);
    };
    let (stores, elements, dropped, before, after) = (
        plan.stores.len(),
        plan.elements,
        plan.dropped,
        plan.stride_before,
        plan.stride_after,
    );
    // Apply.
    let module = &mut bc.blocks[mi];
    for (bi, ri, at, rel) in plan.stores {
        if let Item::Block(f) = &mut module.items[bi]
            && let Item::Record(r) = &mut f.items[ri]
        {
            r.ops[at] = rel;
        }
    }
    let mut removed = plan.removals;
    removed.sort_unstable_by(|a, b| b.cmp(a));
    for (bi, ri) in removed {
        if let Item::Block(f) = &mut module.items[bi] {
            f.items.remove(ri);
        }
    }
    if let Item::Block(md) = &mut module.items[plan.md_block] {
        for (item, id_ref) in plan.node_edits {
            if let Item::Record(r) = &mut md.items[item] {
                r.ops[0] = id_ref;
            }
        }
        if let Item::Record(r) = &mut md.items[plan.list_item] {
            r.ops = plan.new_list;
        }
    }
    fix_offsets(&mut bc, mi, &old_places)?;
    let new_bitcode = bc.write()?;
    let dxil = new_part(head, &new_bitcode);
    Ok(Some(Fit {
        dxil: join(&parts, &dxil),
        stride_before: before,
        stride_after: after,
        elements,
        dropped,
        stores,
    }))
}

/// What to change, worked out without touching the tree.
struct Plan {
    stride_before: u32,
    stride_after: u32,
    elements: usize,
    dropped: usize,
    /// Metadata block (module item), the output list's record in it and its
    /// new operands, and the id operand of each element node's record.
    md_block: usize,
    list_item: usize,
    new_list: Vec<u64>,
    node_edits: Vec<(usize, u64)>,
    /// Function block item, record, operand index, new operand.
    stores: Vec<(usize, usize, usize, u64)>,
    /// Function block item and record of the stores to take out.
    removals: Vec<(usize, usize)>,
}

fn plan(
    bc: &Bitcode,
    m: &Module,
    readers: Option<&std::collections::HashSet<(String, i64)>>,
) -> Result<Option<Plan>, FitError> {
    let module = &bc.blocks[m.index];
    let Some(meta) = Meta::read(module, &m.ints) else {
        return Ok(None);
    };
    let Some((list, els)) = meta.signature(1)? else {
        return Ok(None);
    };
    let n = els.len();
    let mut seen = vec![false; n];
    for e in &els {
        match usize::try_from(e.id).ok().filter(|&i| i < n && !seen[i]) {
            Some(i) => seen[i] = true,
            None => return unsupported("output ids that are not 0..n each once"),
        }
    }
    // Each element node belongs to this list only.
    for (i, md) in meta.mds.iter().enumerate() {
        if let Md::Node { ops, .. } = md
            && i as u64 + 1 != list
            && els.iter().any(|e| ops.contains(&e.node))
        {
            return unsupported("an element node used twice");
        }
    }
    let pos: Vec<usize> = (0..n).filter(|&i| els[i].kind == 3).collect();
    if pos.len() != 1 {
        return Ok(None);
    }
    // Outputs the pixel shader has no input for (ordinary ones only).
    let dead: Vec<bool> = els
        .iter()
        .map(|e| {
            e.kind == 0
                && readers.is_some_and(|r| !r.contains(&(e.name.to_ascii_lowercase(), e.index)))
        })
        .collect();
    let rest: Vec<usize> = (0..n).filter(|&i| els[i].kind != 3 && !dead[i]).collect();
    let distances: u32 = els
        .iter()
        .zip(&dead)
        .filter(|(e, d)| !**d && (e.kind == 6 || e.kind == 7))
        .map(|(e, _)| e.cols)
        .sum();
    let sizes: Vec<u32> = rest.iter().map(|&i| els[i].size).collect();
    let order: Vec<usize> = pos
        .iter()
        .copied()
        .chain(
            mesh_size::packed_order(distances, &sizes)
                .into_iter()
                .map(|k| rest[k]),
        )
        .collect();
    let outputs = |order: &[usize]| -> Vec<Output> {
        order
            .iter()
            .map(|&i| Output {
                name: String::new(),
                system_value: match els[i].kind {
                    3 => 1,
                    6 => 2,
                    7 => 3,
                    _ => 0,
                },
                components: els[i].cols,
                bytes_per_component: els[i].size / els[i].cols,
                allocated: true,
            })
            .collect()
    };
    // Metal orders the members by id: the current order is the id order.
    let mut current: Vec<usize> = (0..n).collect();
    current.sort_by_key(|&i| els[i].id);
    let dropped = dead.iter().filter(|&&d| d).count();
    let stride_before = mesh_size::stride(&outputs(&current), true);
    let stride_after = mesh_size::stride(&outputs(&order), true);
    if stride_after >= stride_before {
        return Ok(None);
    }

    // old id -> new id of what stays
    let mut new_id: HashMap<i64, i64> = HashMap::new();
    for (k, &i) in order.iter().enumerate() {
        new_id.insert(els[i].id, k as i64);
    }
    let dead_ids: std::collections::HashSet<i64> =
        (0..n).filter(|&i| dead[i]).map(|i| els[i].id).collect();
    // The module constant `i32 k`, and its metadata reference.
    let i32_ty = m
        .types
        .iter()
        .position(|t| t.0 == 7 && t.1.first() == Some(&32))
        .map(|t| t as u64)
        .ok_or_else(|| FitError::Unsupported("no i32 type".into()))?;
    let constant_of = |k: i64| -> Option<u64> {
        m.ints
            .iter()
            .filter(|(_, v)| **v == (i32_ty, k))
            .map(|(id, _)| *id)
            .min()
    };
    let mut node_edits: Vec<(usize, u64)> = Vec::new();
    for (k, &i) in order.iter().enumerate() {
        let c = constant_of(k as i64)
            .ok_or_else(|| FitError::Unsupported(format!("no constant {k}")))?;
        let r = meta
            .mds
            .iter()
            .position(|x| matches!(x, Md::Value { value } if *value == c))
            .map(|x| x as u64 + 1)
            .ok_or_else(|| FitError::Unsupported(format!("no metadata number {k}")))?;
        node_edits.push((meta.item(els[i].node)?, r));
    }
    let new_list: Vec<u64> = order.iter().map(|&i| els[i].node).collect();
    let const_of_new: HashMap<i64, u64> = (0..order.len() as i64)
        .map(|k| {
            constant_of(k)
                .map(|c| (k, c))
                .ok_or_else(|| FitError::Unsupported(format!("no constant {k}")))
        })
        .collect::<Result<_, _>>()?;

    // Function blocks: the stores.
    let numbering = Numbering { types: &m.types };
    let function_blocks: Vec<usize> = blocks(module, FUNCTION_BLOCK).map(|(i, _)| i).collect();
    if function_blocks.len() != m.defined.len() {
        return bad("function blocks and defined functions differ in number");
    }
    let mut stores = Vec::new();
    let mut removals = Vec::new();
    for (&bi, &fty) in function_blocks.iter().zip(&m.defined) {
        let Item::Block(f) = &module.items[bi] else {
            unreachable!()
        };
        let args = m
            .types
            .get(fty as usize)
            .filter(|t| t.0 == 21)
            .map(|t| t.1.len().saturating_sub(2) as u64)
            .ok_or_else(|| {
                FitError::Malformed("a function of a type that is no function type".into())
            })?;
        let mut local = Ints::new();
        let mut next = m.values + args;
        for (_, c) in blocks(f, CONSTANTS) {
            next += constants(c, next, &mut local);
        }
        // The opcode constant is the function's own when only it uses it.
        let c_op = local
            .iter()
            .chain(m.ints.iter())
            .find(|(_, v)| **v == (i32_ty, STORE_VERTEX_OUTPUT))
            .map(|(id, _)| *id);
        for (ri, r) in records(f) {
            if r.code == 34 {
                let (_, callee) = call_shape(&r.ops)?;
                // A constant is numbered before the instruction using it.
                let arg = |k: usize| -> Option<u64> {
                    r.ops
                        .get(callee + 1 + k)
                        .and_then(|&rel| next.checked_sub(rel))
                };
                if let (Some(c), Some(a0)) = (c_op, arg(0))
                    && a0 == c
                {
                    let Some(a1) = arg(1) else {
                        return bad("a storeVertexOutput without its element");
                    };
                    let old = m.ints.get(&a1).map(|v| v.1).ok_or_else(|| {
                        FitError::Unsupported("a store of a computed element".into())
                    })?;
                    if dead_ids.contains(&old) {
                        removals.push((bi, ri));
                    } else {
                        let new = *new_id.get(&old).ok_or_else(|| {
                            FitError::Unsupported("a store of an unknown element".into())
                        })?;
                        stores.push((bi, ri, callee + 2, next - const_of_new[&new]));
                    }
                }
            }
            if numbering.produces(r.code, &r.ops)? {
                next += 1;
            }
        }
    }
    Ok(Some(Plan {
        stride_before,
        stride_after,
        elements: n,
        dropped,
        md_block: meta.block,
        list_item: meta.item(list)?,
        new_list,
        node_edits,
        stores,
        removals,
    }))
}

/// The module's symbol table names function blocks by their offset in
/// words (`VST_CODE_FNENTRY`), and `MODULE_CODE_VSTOFFSET` the table's:
/// both follow the bytes that moved. `old` is where the blocks were.
fn fix_offsets(
    bc: &mut Bitcode,
    mi: usize,
    old: &[crate::bitcode::Placed],
) -> Result<(), FitError> {
    let at = |places: &[crate::bitcode::Placed], id: u32| -> Vec<usize> {
        places
            .iter()
            .filter(|p| p.id == id && p.depth == 2)
            .filter_map(|p| p.word)
            .collect()
    };
    let old_functions = at(old, FUNCTION_BLOCK);
    let old_vst = at(old, VST);
    // What names what, from the stream as it was read: the module's
    // VSTOFFSET (item, value) and each function entry (symbol table item,
    // record, value, function block number).
    let mut vstoffset: Option<(usize, u64)> = None;
    let mut entries: Vec<(usize, usize, u64, usize)> = Vec::new();
    for (i, it) in bc.blocks[mi].items.iter().enumerate() {
        match it {
            Item::Record(r) if r.code == 13 && !old_vst.is_empty() => {
                vstoffset = r.ops.first().map(|&v| (i, v));
            }
            Item::Block(b) if b.id == VST => {
                for (ri, r) in records(b) {
                    if r.code == 3 && r.ops.len() >= 2 {
                        let off = r.ops[1];
                        let f = old_functions
                            .iter()
                            .position(|&w| w as u64 == off || w as u64 + 1 == off)
                            .ok_or_else(|| {
                                FitError::Malformed(
                                    "a function entry that names no function block".into(),
                                )
                            })?;
                        entries.push((i, ri, off, f));
                    }
                }
            }
            _ => {}
        }
    }
    let mut last = Vec::new();
    for _ in 0..4 {
        let (_, now) = bc.write_placed()?;
        let functions = at(&now, FUNCTION_BLOCK);
        let vst = at(&now, VST);
        if functions.len() != old_functions.len() || vst.len() != old_vst.len() {
            return bad("blocks went missing");
        }
        let mut values = Vec::new();
        let module = &mut bc.blocks[mi];
        if let Some((i, v)) = vstoffset {
            let new = shift(v, old_vst[0], vst[0])?;
            if let Item::Record(r) = &mut module.items[i] {
                r.ops[0] = new;
            }
            values.push(new);
        }
        for &(i, ri, off, f) in &entries {
            let new = shift(off, old_functions[f], functions[f])?;
            if let Item::Block(b) = &mut module.items[i]
                && let Item::Record(r) = &mut b.items[ri]
            {
                r.ops[1] = new;
            }
            values.push(new);
        }
        if values == last {
            return Ok(());
        }
        last = values;
    }
    bad("the symbol table's offsets do not settle")
}

/// `value` named the word `old`, give or take the convention's one: now it
/// names `new`.
fn shift(value: u64, old: usize, new: usize) -> Result<u64, FitError> {
    let extra = value
        .checked_sub(old as u64)
        .filter(|&e| e <= 1)
        .ok_or_else(|| FitError::Malformed("an offset that is not a block's".into()))?;
    Ok(new as u64 + extra)
}
