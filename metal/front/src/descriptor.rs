//! Descriptor heaps: handle arithmetic.
//!
//! D3D12 hands out `CPU_DESCRIPTOR_HANDLE` / `GPU_DESCRIPTOR_HANDLE` as plain
//! integers and applications do their own arithmetic on them
//! (`start + index * increment`). The front gives every heap a disjoint
//! address range for each kind of handle and resolves any handle back to
//! `(heap, index)`, so a handle can be checked, copied from, or turned into a
//! table reference in a command. The handle values are synthetic: nothing
//! dereferences them.

use std::collections::BTreeMap;

use crate::hresult::{Hr, Result};

pub const TYPE_CBV_SRV_UAV: u32 = 0;
pub const TYPE_SAMPLER: u32 = 1;
pub const TYPE_RTV: u32 = 2;
pub const TYPE_DSV: u32 = 3;

pub const FLAG_SHADER_VISIBLE: u32 = 1;

/// `D3D12_MAX_...`: bounds D3D12 documents for shader-visible heaps.
/// 2^21: the shader-visible limit of tier 3 is "at least 1,000,000"; D3DMetal takes
/// 2^21 (victory-shader's tests use a heap that size for `ResourceDescriptorHeap[]`).
pub const MAX_SHADER_VISIBLE_CBV_SRV_UAV: u32 = 1 << 21;
pub const MAX_SHADER_VISIBLE_SAMPLERS: u32 = 2048;

/// A key into the space: the front's own number for a heap (not the back's `SlotHeap`).
pub type SlotHeap = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeapDesc {
    pub ty: u32,
    pub count: u32,
    pub flags: u32,
    pub node_mask: u32,
}

impl HeapDesc {
    pub fn shader_visible(&self) -> bool {
        self.flags & FLAG_SHADER_VISIBLE != 0
    }
}

/// `CreateDescriptorHeap` argument rules (D3DMetal agrees: RTV/DSV heaps
/// cannot be shader visible).
pub fn validate_heap_desc(d: &HeapDesc) -> Result<()> {
    if d.ty > TYPE_DSV || d.count == 0 || d.flags & !FLAG_SHADER_VISIBLE != 0 || d.node_mask > 1 {
        return Err(Hr::E_INVALIDARG);
    }
    if d.shader_visible() {
        let max = match d.ty {
            TYPE_CBV_SRV_UAV => MAX_SHADER_VISIBLE_CBV_SRV_UAV,
            TYPE_SAMPLER => MAX_SHADER_VISIBLE_SAMPLERS,
            _ => return Err(Hr::E_INVALIDARG),
        };
        if d.count > max {
            return Err(Hr::E_INVALIDARG);
        }
    }
    Ok(())
}

/// Where a heap lives in handle space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeapRange {
    pub id: SlotHeap,
    pub cpu_base: u64,
    /// 0 for a heap that is not shader visible.
    pub gpu_base: u64,
    pub count: u32,
    pub stride: u32,
}

impl HeapRange {
    pub fn cpu_handle(&self, index: u32) -> u64 {
        self.cpu_base + index as u64 * self.stride as u64
    }
    pub fn gpu_handle(&self, index: u32) -> u64 {
        self.gpu_base + index as u64 * self.stride as u64
    }
}

const CPU_START: u64 = 0x0000_1000_0000_0000;
const GPU_START: u64 = 0x0000_2000_0000_0000;
const GAP: u64 = 4096;

#[derive(Default)]
pub struct DescriptorSpace {
    next_cpu: u64,
    next_gpu: u64,
    next_id: SlotHeap,
    cpu: BTreeMap<u64, HeapRange>,
    gpu: BTreeMap<u64, HeapRange>,
}

fn round_up(v: u64, a: u64) -> u64 {
    v.div_ceil(a) * a
}

impl DescriptorSpace {
    pub fn new() -> Self {
        DescriptorSpace {
            next_cpu: CPU_START,
            next_gpu: GPU_START,
            next_id: 1,
            ..Default::default()
        }
    }

    pub fn alloc(&mut self, count: u32, stride: u32, shader_visible: bool) -> HeapRange {
        let span = round_up(count as u64 * stride as u64, GAP) + GAP;
        let id = self.next_id;
        self.next_id += 1;
        let cpu_base = self.next_cpu;
        self.next_cpu += span;
        let gpu_base = if shader_visible {
            let b = self.next_gpu;
            self.next_gpu += span;
            b
        } else {
            0
        };
        let r = HeapRange {
            id,
            cpu_base,
            gpu_base,
            count,
            stride,
        };
        self.cpu.insert(cpu_base, r);
        if shader_visible {
            self.gpu.insert(gpu_base, r);
        }
        r
    }

    pub fn free(&mut self, id: SlotHeap) {
        self.cpu.retain(|_, r| r.id != id);
        self.gpu.retain(|_, r| r.id != id);
    }

    fn resolve(
        map: &BTreeMap<u64, HeapRange>,
        handle: u64,
        base: fn(&HeapRange) -> u64,
    ) -> Option<(SlotHeap, u32)> {
        let (_, r) = map.range(..=handle).next_back()?;
        let off = handle.checked_sub(base(r))?;
        if off % r.stride as u64 != 0 {
            return None;
        }
        let index = off / r.stride as u64;
        (index < r.count as u64).then_some((r.id, index as u32))
    }

    /// `(heap, index)` of a CPU handle; `None` for a handle that is not
    /// exactly on a descriptor of a live heap.
    pub fn resolve_cpu(&self, handle: u64) -> Option<(SlotHeap, u32)> {
        Self::resolve(&self.cpu, handle, |r| r.cpu_base)
    }

    pub fn resolve_gpu(&self, handle: u64) -> Option<(SlotHeap, u32)> {
        Self::resolve(&self.gpu, handle, |r| r.gpu_base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_resolve_to_heap_and_index() {
        let mut s = DescriptorSpace::new();
        let a = s.alloc(16, 32, true);
        let b = s.alloc(4, 64, false);
        let c = s.alloc(2048, 16, true);
        assert_ne!(a.cpu_base, 0);
        assert_ne!(a.gpu_base, 0);
        assert_eq!(b.gpu_base, 0);
        for i in 0..16 {
            assert_eq!(s.resolve_cpu(a.cpu_handle(i)), Some((a.id, i)));
            assert_eq!(s.resolve_gpu(a.gpu_handle(i)), Some((a.id, i)));
        }
        assert_eq!(s.resolve_cpu(a.cpu_handle(16)), None, "one past the end");
        assert_eq!(
            s.resolve_cpu(a.cpu_handle(3) + 1),
            None,
            "not on a descriptor"
        );
        assert_eq!(s.resolve_cpu(b.cpu_handle(3)), Some((b.id, 3)));
        assert_eq!(s.resolve_gpu(b.cpu_base), None, "not shader visible");
        assert_eq!(s.resolve_cpu(c.cpu_handle(2047)), Some((c.id, 2047)));
        assert_eq!(s.resolve_cpu(0), None);
        // Application-style arithmetic: start + n * increment.
        assert_eq!(s.resolve_cpu(a.cpu_base + 5 * 32), Some((a.id, 5)));
        assert_eq!(s.resolve_cpu(b.cpu_base + 2 * 64), Some((b.id, 2)));
        assert_ne!(a.cpu_base, a.gpu_base);
    }

    #[test]
    fn freed_heaps_stop_resolving_and_ids_are_not_reused() {
        let mut s = DescriptorSpace::new();
        let a = s.alloc(8, 32, true);
        s.free(a.id);
        assert_eq!(s.resolve_cpu(a.cpu_base), None);
        assert_eq!(s.resolve_gpu(a.gpu_base), None);
        let b = s.alloc(8, 32, true);
        assert_ne!(a.id, b.id);
        assert_ne!(a.cpu_base, b.cpu_base);
    }

    #[test]
    fn heap_description_rules() {
        let ok = |ty, count, flags| {
            validate_heap_desc(&HeapDesc {
                ty,
                count,
                flags,
                node_mask: 0,
            })
        };
        assert!(ok(TYPE_CBV_SRV_UAV, 16, FLAG_SHADER_VISIBLE).is_ok());
        assert!(ok(TYPE_SAMPLER, 2048, FLAG_SHADER_VISIBLE).is_ok());
        assert!(ok(TYPE_RTV, 2, 0).is_ok());
        assert_eq!(ok(TYPE_RTV, 2, FLAG_SHADER_VISIBLE), Err(Hr::E_INVALIDARG));
        assert_eq!(
            ok(TYPE_SAMPLER, 2049, FLAG_SHADER_VISIBLE),
            Err(Hr::E_INVALIDARG)
        );
        assert_eq!(ok(TYPE_CBV_SRV_UAV, 0, 0), Err(Hr::E_INVALIDARG));
        assert_eq!(ok(9, 1, 0), Err(Hr::E_INVALIDARG));
    }
}
