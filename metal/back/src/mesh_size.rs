//! How Metal counts the output of a mesh threadgroup against its 32768-byte
//! limit ("Total mesh size (N) exceeds the maximum mesh size allowed
//! (32768)"), measured on an Apple M5 Max, macOS 27.2.1, SDK 27.2
//! (2026-11-27); the experiments are `tests/mesh_limit_probe.rs`, the facts
//! pinned by `tests/mesh_limit.rs`, the write-up `docs/metal/mesh32k.md`.
//!
//! ```text
//! total = round_up(max_vertices, 4) * vertex_stride + max_primitives * primitive_stride
//! ```
//!
//! * the stride is that of the output struct: members in order, each aligned
//!   to **its own size** (a `float3` is 12 bytes at a multiple of 12, a
//!   `half3` 6 at a multiple of 6) and no padding at the end;
//! * the vertex struct has the clip and cull distances first (4 bytes each),
//!   then the position (16), then every other output in element order; the
//!   distances are passed on as ordinary values too, so they count twice;
//! * the declared maxima count, not what the shader writes; indices, the
//!   topology, the payload (own 16 KB limit) and threadgroup memory (own 32 KB
//!   limit) are not part of it;
//! * Metal does not drop outputs the fragment shader does not read.
//!
//! Metal says no at pipeline creation (`MTL4Compiler`, error domain
//! `AGXMetalG17G` on this GPU), not Metal Shader Converter.

/// The most a mesh threadgroup's output may take, bytes (inclusive).
pub const LIMIT: u64 = 32768;

/// One output of a mesh shader, as its signature declares it.
#[derive(Clone, Debug)]
pub struct Output {
    pub name: String,
    /// D3D_NAME: 1 position, 2 clip distance, 3 cull distance, 0 ordinary.
    pub system_value: u32,
    pub components: u32,
    /// 4 for 32-bit values, 2 for 16-bit.
    pub bytes_per_component: u32,
    /// False for an element that takes no slot.
    pub allocated: bool,
}

const POSITION: u32 = 1;
const DISTANCES: [u32; 2] = [2, 3];

fn end_after(end: u32, size: u32) -> u32 {
    if size == 0 {
        end
    } else {
        end.next_multiple_of(size) + size
    }
}

/// Bytes of one vertex (`vertex`) or primitive of the output struct.
pub fn stride(outputs: &[Output], vertex: bool) -> u32 {
    let size = |o: &Output| o.components * o.bytes_per_component;
    let live = || outputs.iter().filter(|o| o.allocated);
    let mut end = 0;
    if vertex {
        let distances: u32 = live()
            .filter(|o| DISTANCES.contains(&o.system_value))
            .map(|o| o.components)
            .sum();
        for _ in 0..distances {
            end = end_after(end, 4);
        }
        end = end_after(end, 16);
        for o in live().filter(|o| o.system_value != POSITION) {
            end = end_after(end, size(o));
        }
    } else {
        for o in live() {
            end = end_after(end, size(o));
        }
    }
    end
}

/// The order of the non-position outputs `sizes` (bytes each, in the order
/// the signature has them) that leaves the least padding in a vertex with
/// `distances` clip and cull distance components: at each step the output
/// that needs the fewest padding bytes, the bigger one first when equal.
/// Returns indices into `sizes`.
pub fn packed_order(distances: u32, sizes: &[u32]) -> Vec<usize> {
    let mut end = 0;
    for _ in 0..distances {
        end = end_after(end, 4);
    }
    end = end_after(end, 16);
    let mut left: Vec<usize> = (0..sizes.len()).collect();
    let mut order = Vec::with_capacity(sizes.len());
    while !left.is_empty() {
        let pad = |i: usize| match sizes[i] {
            0 => 0,
            s => end.next_multiple_of(s) - end,
        };
        let (at, &best) = left
            .iter()
            .enumerate()
            .min_by_key(|&(_, &i)| (pad(i), std::cmp::Reverse(sizes[i])))
            .expect("not empty");
        order.push(best);
        left.remove(at);
        end = end_after(end, sizes[best]);
    }
    order
}

/// Metal's total for the declared maxima and the two strides.
pub fn total(
    max_vertices: u32,
    vertex_stride: u32,
    max_primitives: u32,
    primitive_stride: u32,
) -> u64 {
    u64::from(max_vertices.next_multiple_of(4)) * u64::from(vertex_stride)
        + u64::from(max_primitives) * u64::from(primitive_stride)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(name: &str, sv: u32, components: u32) -> Output {
        Output {
            name: name.into(),
            system_value: sv,
            components,
            bytes_per_component: 4,
            allocated: true,
        }
    }

    #[test]
    fn strides_follow_the_measured_layout() {
        // pos, float, float3: the float3 is at 24 (a multiple of 12), ends at 36.
        let v = [o("p", 1, 4), o("a", 0, 1), o("b", 0, 3)];
        assert_eq!(stride(&v, true), 36);
        // after the position a float3 is at 24 (8 bytes of padding): 24..36,
        // 36..48, then the float at 48.
        let v = [o("p", 1, 4), o("a", 0, 3), o("b", 0, 3), o("c", 0, 1)];
        assert_eq!(stride(&v, true), 52);
    }

    #[test]
    fn the_vertex_count_is_rounded_to_four() {
        assert_eq!(total(225, 144, 0, 0), 228 * 144);
        assert_eq!(total(256, 128, 1, 0), 32768);
        assert_eq!(total(3, 16, 5, 4), 4 * 16 + 5 * 4);
    }

    #[test]
    fn the_packed_order_fills_gaps_and_never_does_worse() {
        // float, float3 alternating: after the position (16) a float3 is at
        // 24, so floats go first and into the gaps.
        let sizes = [4, 12, 4, 12, 4, 12];
        let order = packed_order(0, &sizes);
        let mut perm = order.clone();
        perm.sort_unstable();
        assert_eq!(perm, [0, 1, 2, 3, 4, 5]);
        let at = |order: &[usize]| {
            let v: Vec<Output> = std::iter::once(o("p", 1, 4))
                .chain(order.iter().map(|&i| o("x", 0, sizes[i] / 4)))
                .collect();
            stride(&v, true)
        };
        assert_eq!(at(&[0, 1, 2, 3, 4, 5]), 84);
        assert!(at(&order) < at(&[0, 1, 2, 3, 4, 5]));
        assert_eq!(at(&order), 16 + 3 * 16);
        assert_eq!(packed_order(0, &[]), Vec::<usize>::new());
    }

    #[test]
    fn clip_distances_come_first() {
        let v = [o("p", 1, 4), o("c", 2, 1), o("a", 0, 4)];
        // 4 (distance), pad to 16, position -> 32, the distance again as a
        // value at 32 -> 36, a at 48 -> 64
        assert_eq!(stride(&v, true), 64);
    }
}
