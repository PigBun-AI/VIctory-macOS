//! mac S2: the vertex and primitive outputs of a mesh shader numbered so
//! that Metal's output struct has the least padding.
//!
//! Metal counts `max_vertices * vertex_stride + max_primitives *
//! primitive_stride` against 32768 bytes (`MTL4Compiler`, measured on an
//! Apple M5 Max, macOS 27.2.1, 2026-11-27: docs/metal/mesh32k.md). The
//! vertex struct is made from the DXIL output elements in the order of their
//! element **ids** (the position and the clip and cull distances are placed
//! first, whatever their ids), each member aligned to its own size and
//! nothing added at the end. The translator numbers the elements as the
//! console's signature has them, so a `float` between two `float3`s costs 4
//! bytes of padding on every vertex: 520 of the 8,765 mesh shaders of the
//! title were over the limit, 377 of them only by that padding.
//!
//! The ids are the position in the signature lists (`outputs`, `prims`), so
//! sorting the lists before lowering renumbers the metadata and every
//! `storeVertexOutput` / `storePrimitiveOutput` together. D3D12 links stages
//! by semantic and a signature's order is of no meaning there.

use super::container::SigElement;

/// Bytes an element takes in Metal's struct (every output is 32-bit here:
/// 16-bit ones are declared 32-bit by the translator).
fn size(e: &SigElement) -> u32 {
    4 * u32::from(e.cols.max(1))
}

/// End of a member of `size` bytes put at `end` (aligned to its own size).
fn after(end: u32, size: u32) -> u32 {
    end.next_multiple_of(size.max(1)) + size
}

/// The order of `sizes` (bytes of each non-position member, in their
/// present order) that leaves the least padding after `start` bytes: at each
/// step the member that needs the fewest padding bytes, the bigger one first
/// when equal, the earlier one first when still equal. Indices into `sizes`.
fn packed(start: u32, sizes: &[u32]) -> Vec<usize> {
    let mut end = start;
    let mut left: Vec<usize> = (0..sizes.len()).collect();
    let mut order = Vec::with_capacity(sizes.len());
    while !left.is_empty() {
        let pad = |i: usize| end.next_multiple_of(sizes[i].max(1)) - end;
        let (at, &best) = left
            .iter()
            .enumerate()
            .min_by_key(|&(_, &i)| (pad(i), std::cmp::Reverse(sizes[i])))
            .expect("not empty");
        order.push(best);
        left.remove(at);
        end = after(end, sizes[best]);
    }
    order
}

/// Sorts a vertex output list: position first, then the least padding.
/// Clip and cull distance components lead Metal's struct (4 bytes each),
/// then the position (16), then every member including the distances.
pub(super) fn vertex(outputs: &mut Vec<SigElement>) {
    let mut start = 0;
    for e in outputs.iter().filter(|e| matches!(e.kind, 6 | 7)) {
        for _ in 0..e.cols {
            start = after(start, 4);
        }
    }
    start = after(start, 16);
    let (position, rest): (Vec<SigElement>, Vec<SigElement>) = std::mem::take(outputs)
        .into_iter()
        .partition(|e| e.kind == 3);
    let sizes: Vec<u32> = rest.iter().map(size).collect();
    let order = packed(start, &sizes);
    let mut rest: Vec<Option<SigElement>> = rest.into_iter().map(Some).collect();
    outputs.extend(position);
    outputs.extend(order.into_iter().filter_map(|i| rest[i].take()));
}

/// Sorts a primitive output list (no position, no distances).
pub(super) fn primitive(prims: &mut Vec<SigElement>) {
    let sizes: Vec<u32> = prims.iter().map(size).collect();
    let order = packed(0, &sizes);
    let mut all: Vec<Option<SigElement>> = std::mem::take(prims).into_iter().map(Some).collect();
    prims.extend(order.into_iter().filter_map(|i| all[i].take()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(name: &str, index: u32, kind: u8, cols: u8) -> SigElement {
        SigElement {
            name: name.into(),
            index,
            kind,
            comp: 9,
            interp: 2,
            row: Some(index),
            col: 0,
            cols,
            usage: (1 << cols) - 1,
            half: false,
        }
    }

    /// Bytes of a vertex in Metal's layout, by the rule above.
    fn stride(list: &[SigElement]) -> u32 {
        let mut end = 0;
        for e in list.iter().filter(|e| matches!(e.kind, 6 | 7)) {
            for _ in 0..e.cols {
                end = after(end, 4);
            }
        }
        end = after(end, 16);
        for e in list.iter().filter(|e| e.kind != 3) {
            end = after(end, size(e));
        }
        end
    }

    fn names(list: &[SigElement]) -> Vec<String> {
        list.iter()
            .map(|e| format!("{}{}", e.name, e.index))
            .collect()
    }

    #[test]
    fn a_float_between_float3s_stops_costing_padding() {
        // The position, then `float` and `float3` pairs alternating.
        let mut v = vec![el("SV_Position", 0, 3, 4)];
        for i in 0..11 {
            v.push(el("A", i, 0, 1));
            v.push(el("B", i, 0, 3));
        }
        let before = stride(&v);
        vertex(&mut v);
        let after = stride(&v);
        assert_eq!(before, 276);
        assert_eq!(after, 16 + 11 * 16);
        assert!(after < before);
        assert_eq!(v[0].kind, 3, "the position stays first");
        assert_eq!(v.len(), 23);
    }

    #[test]
    fn nothing_is_lost_and_an_ordered_list_stays() {
        let mut v = vec![
            el("SV_Position", 0, 3, 4),
            el("TEXCOORD", 0, 0, 4),
            el("TEXCOORD", 1, 0, 4),
            el("COLOR", 0, 0, 4),
        ];
        let before = names(&v);
        vertex(&mut v);
        assert_eq!(names(&v), before);
        let mut w = vec![
            el("X", 0, 0, 2),
            el("Y", 0, 0, 1),
            el("Z", 0, 0, 3),
            el("W", 0, 0, 1),
        ];
        let mut sorted = names(&w);
        sorted.sort();
        vertex(&mut w);
        let mut got = names(&w);
        got.sort();
        assert_eq!(got, sorted, "the same elements");
    }

    #[test]
    fn distances_lead_the_struct_and_are_counted_with_the_members() {
        // Two clip distance components: 8 bytes, the position at 16..32.
        let mut v = vec![
            el("SV_Position", 0, 3, 4),
            el("A", 0, 0, 3),
            el("SV_ClipDistance", 0, 6, 2),
            el("B", 0, 0, 1),
            el("C", 0, 0, 3),
        ];
        let before = stride(&v);
        vertex(&mut v);
        assert!(stride(&v) <= before);
        assert_eq!(v[0].kind, 3);
        assert!(v.iter().any(|e| e.kind == 6));
    }

    #[test]
    fn primitive_outputs_are_ordered_the_same_way() {
        let mut p = vec![
            el("P", 0, 0, 3),
            el("Q", 0, 0, 1),
            el("R", 0, 0, 3),
            el("S", 0, 0, 1),
        ];
        let pstride = |l: &[SigElement]| l.iter().fold(0, |end, e| after(end, size(e)));
        let before = pstride(&p);
        primitive(&mut p);
        assert_eq!((before, pstride(&p)), (40, 32));
        assert_eq!(p.len(), 4);
    }
}
