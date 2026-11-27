//! Mac patch S1 (docs/mac/PATCHES.md): per-vertex values as flat inputs.
//!
//! `dx.op.attributeAtVertex` (HLSL `GetAttributeAtVertex`) has no form on the Metal chain: MSC refuses it
//! (error 7) and D3DMetal too. On a host without it (`host_attribute_at_vertex` false) the pixel stage
//! reads the three vertices of an attribute as flat inputs instead: vertex 0 is the attribute's own
//! (constant) input, which holds the provoking vertex; vertices 1 and 2 come from added per-primitive
//! inputs, packed four scalars to an element (`TEXCOORD64`, `TEXCOORD65`, ...). The mesh stage fills them
//! from a group-shared exchange (`Lower::vtx_*`): each lane puts the values its vertex outputs hold into
//! group memory, the lanes meet, and each lane writes the values of its primitive's second and third
//! vertex as primitive outputs.
//!
//! The two stages agree on the layout by the list of [`VertexRead`]s alone: the pixel stage reports the
//! list it needs ([`super::HostShader::vertex_reads`]) and the host translates the mesh stage with it
//! ([`super::translate_with`]).

use std::cell::RefCell;

use super::container::SigElement;
use crate::io::{Interp, Io};

/// The semantic of the added elements; the index counts up from [`FLAT_BASE`].
pub const FLAT_NAME: &str = "TEXCOORD";
/// The first semantic index of the added elements (the title's own stay far below).
pub const FLAT_BASE: u32 = 64;
/// The first register row of the added elements: past any row the console's signatures use.
const FIRST_ROW: u32 = 32;

/// A channel of an input element that the pixel program reads from vertex 1 and/or vertex 2.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct VertexRead {
    /// The element's semantic, as the pixel stage's input and the mesh stage's output name it.
    pub name: String,
    pub index: u32,
    /// The channel within the register (0..4), not relative to the element's first column.
    pub channel: u8,
    /// Bit 0: vertex 1 is read; bit 1: vertex 2 is read.
    pub vertices: u8,
}

/// Sorted, one entry per channel (vertex bits merged), none without a vertex.
pub fn canonical(mut reads: Vec<VertexRead>) -> Vec<VertexRead> {
    reads.sort();
    let mut out: Vec<VertexRead> = Vec::new();
    for r in reads {
        match out.last_mut() {
            Some(l) if (&l.name, l.index, l.channel) == (&r.name, r.index, r.channel) => {
                l.vertices |= r.vertices;
            }
            _ => out.push(r),
        }
    }
    out.retain(|r| r.vertices & 3 != 0);
    out
}

/// The scalars of the added elements in order: (read, vertex 1 or 2).
fn scalars(reads: &[VertexRead]) -> impl Iterator<Item = (usize, u8)> + '_ {
    reads.iter().enumerate().flat_map(|(n, r)| {
        (1..=2u8)
            .filter(move |k| r.vertices >> (k - 1) & 1 != 0)
            .map(move |k| (n, k))
    })
}

/// The added element and its component that carry vertex `k` of the read `read`.
pub fn slot(reads: &[VertexRead], read: usize, k: u8) -> Option<(usize, u8)> {
    let at = scalars(reads).position(|s| s == (read, k))?;
    Some((at / 4, (at % 4) as u8))
}

/// The index of the read of this channel, if there is one.
pub fn find(reads: &[VertexRead], name: &str, index: u32, channel: u8) -> Option<usize> {
    reads
        .iter()
        .position(|r| r.name == name && r.index == index && r.channel == channel)
}

/// The elements the reads add: unsigned flat elements of four scalars (the last may be shorter), from
/// register `row` on (`None`: the first row past the console's).
pub(crate) fn elements(reads: &[VertexRead], row: Option<u32>) -> Vec<SigElement> {
    let total = scalars(reads).count() as u32;
    let row = row.unwrap_or(FIRST_ROW);
    (0..total.div_ceil(4))
        .map(|n| SigElement {
            name: FLAT_NAME.into(),
            index: FLAT_BASE + n,
            kind: 0,
            // DXIL component type 5: u32, so the bits of any value pass untouched.
            comp: 5,
            // Constant.
            interp: 1,
            row: Some(row + n),
            col: 0,
            cols: (total - 4 * n).min(4) as u8,
            usage: 0,
            half: false,
        })
        .collect()
}

/// The first register row for the added elements of a signature: past the rows `existing` uses.
pub(crate) fn first_row(existing: &[SigElement]) -> u32 {
    existing
        .iter()
        .filter_map(|e| e.row)
        .map(|r| r + 1)
        .max()
        .unwrap_or(0)
        .max(FIRST_ROW)
}

/// The reads in bytes, to key a translation by them.
pub fn encode(reads: &[VertexRead]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend((reads.len() as u32).to_le_bytes());
    for r in reads {
        out.extend((r.name.len() as u32).to_le_bytes());
        out.extend(r.name.as_bytes());
        out.extend(r.index.to_le_bytes());
        out.push(r.channel);
        out.push(r.vertices);
    }
    out
}

/// The reads of [`encode`]; `None` for bytes that are not.
pub fn decode(mut bytes: &[u8]) -> Option<Vec<VertexRead>> {
    fn take<'a>(b: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
        let (head, rest) = b.split_at_checked(n)?;
        *b = rest;
        Some(head)
    }
    let word = |b: &mut &[u8]| Some(u32::from_le_bytes(take(b, 4)?.try_into().ok()?));
    let n = word(&mut bytes)? as usize;
    let mut out = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        let len = word(&mut bytes)? as usize;
        let name = String::from_utf8(take(&mut bytes, len)?.to_vec()).ok()?;
        let index = word(&mut bytes)?;
        let [channel, vertices] = take(&mut bytes, 2)? else { return None };
        out.push(VertexRead { name, index, channel: *channel, vertices: *vertices });
    }
    bytes.is_empty().then_some(out)
}

/// What a pixel program reads of vertices 1 and 2, by the semantics of its input signature `inputs`.
/// Conservative where the interpolation instruction does not say which weight it applies (both).
pub(crate) fn pixel_reads(io: &Io, inputs: &[SigElement]) -> Vec<VertexRead> {
    // The rows read per vertex (`Lower::per_vertex`).
    let interpolated = |row: u8| {
        io.interps.iter().any(|(_, i)| match *i {
            Interp::Interpolated { attribute, .. } | Interp::Weighted { attribute } => attribute.row == row,
            Interp::Vertex { .. } => false,
        })
    };
    let per_vertex = |row: u8| {
        io.interps.iter().any(|(_, i)| match *i {
            Interp::Weighted { attribute } => attribute.row == row,
            Interp::Vertex { attribute, vertex } => attribute.row == row && vertex != 2 && interpolated(row),
            Interp::Interpolated { .. } => false,
        })
    };
    let mut reads = Vec::new();
    for (_, i) in &io.interps {
        let (attribute, vertices) = match *i {
            Interp::Interpolated { attribute, .. } | Interp::Weighted { attribute } => (attribute, 3),
            Interp::Vertex { attribute, vertex: 0 } => (attribute, 1),
            Interp::Vertex { attribute, vertex: 1 } => (attribute, 2),
            Interp::Vertex { .. } => continue,
        };
        if !per_vertex(attribute.row) {
            continue;
        }
        let Some(e) = inputs.iter().find(|e| {
            e.row == Some(u32::from(attribute.row))
                && attribute.channel >= e.col
                && attribute.channel < e.col + e.cols
        }) else {
            continue;
        };
        reads.push(VertexRead {
            name: e.name.clone(),
            index: e.index,
            channel: attribute.channel,
            vertices,
        });
    }
    canonical(reads)
}

thread_local! {
    /// What the mesh stage being translated must hand on ([`with_reads`]).
    static READS: RefCell<Vec<VertexRead>> = const { RefCell::new(Vec::new()) };
    /// Reads the lowering of the pixel program in progress found that [`pixel_reads`] did not list (16-bit
    /// inputs read other channels than the instruction names): asked for, and the lowering run again.
    static ASKED: RefCell<Vec<VertexRead>> = const { RefCell::new(Vec::new()) };
}

/// The refusal that makes the lowering start again with the reads asked for so far.
pub(crate) const MORE: &str = "a per-vertex value the layout lacks";

/// Forgets what was asked for (a new translation).
pub(crate) fn clear_asked() {
    ASKED.with(|a| a.borrow_mut().clear());
}

/// Asks for a read the layout lacks.
pub(crate) fn ask(read: VertexRead) {
    ASKED.with(|a| {
        let mut a = a.borrow_mut();
        a.push(read);
        *a = canonical(std::mem::take(&mut *a));
    });
}

/// What was asked for.
pub(crate) fn asked() -> Vec<VertexRead> {
    ASKED.with(|a| a.borrow().clone())
}

/// Whether `reads` hold everything that was asked for.
pub(crate) fn covers(reads: &[VertexRead]) -> bool {
    asked().iter().all(|a| {
        reads
            .iter()
            .any(|r| (&r.name, r.index, r.channel) == (&a.name, a.index, a.channel) && r.vertices & a.vertices == a.vertices)
    })
}

/// Runs `f` (a translation) with `reads` as what the mesh stage hands to the pixel stage.
pub fn with_reads<R>(reads: &[VertexRead], f: impl FnOnce() -> R) -> R {
    let before = READS.replace(canonical(reads.to_vec()));
    let r = f();
    READS.set(before);
    r
}

/// The reads the translation in progress on this thread was given.
pub fn current() -> Vec<VertexRead> {
    READS.with(|r| r.borrow().clone())
}

#[cfg(test)]
thread_local! {
    /// The `dx.op` names a test's translation called (`Shader::op`).
    static OPS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) fn note_op(name: &str) {
    OPS.with(|o| o.borrow_mut().push(name.to_owned()));
}

/// The ops called since the last call, by name.
#[cfg(test)]
pub(crate) fn take_ops() -> Vec<String> {
    OPS.with(|o| std::mem::take(&mut *o.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(name: &str, index: u32, channel: u8, vertices: u8) -> VertexRead {
        VertexRead { name: name.into(), index, channel, vertices }
    }

    #[test]
    fn the_layout_packs_four_scalars_to_an_element_in_order() {
        let reads = canonical(vec![
            read("TEXCOORD", 1, 2, 3),
            read("COLOR", 0, 0, 1),
            read("TEXCOORD", 1, 0, 2),
            read("TEXCOORD", 1, 0, 1),
        ]);
        // COLOR0.x: vertex 1; TEXCOORD1.x: vertices 1 and 2; TEXCOORD1.z: vertices 1 and 2.
        assert_eq!(reads.len(), 3);
        assert_eq!(slot(&reads, 0, 1), Some((0, 0)));
        assert_eq!(slot(&reads, 0, 2), None);
        assert_eq!(slot(&reads, 1, 1), Some((0, 1)));
        assert_eq!(slot(&reads, 1, 2), Some((0, 2)));
        assert_eq!(slot(&reads, 2, 1), Some((0, 3)));
        assert_eq!(slot(&reads, 2, 2), Some((1, 0)));
        let els = elements(&reads, None);
        assert_eq!(els.len(), 2);
        assert_eq!((els[0].index, els[0].cols, els[0].row), (64, 4, Some(32)));
        assert_eq!((els[1].index, els[1].cols, els[1].row), (65, 1, Some(33)));
    }

    #[test]
    fn the_reads_survive_their_bytes() {
        let reads = canonical(vec![read("TEXCOORD", 7, 3, 3), read("COLOR", 1, 0, 2)]);
        assert_eq!(decode(&encode(&reads)), Some(reads));
        assert_eq!(decode(&[1, 0, 0, 0]), None);
    }

    #[test]
    fn reads_translate_scoped() {
        assert!(current().is_empty());
        with_reads(&[read("TEXCOORD", 0, 0, 3)], || assert_eq!(current().len(), 1));
        assert!(current().is_empty());
    }
}
