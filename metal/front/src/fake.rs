//! A backend with no GPU: buffers are host memory, `CopyBuffer` really
//! copies, everything else is recorded and counted. Used by the core's tests
//! and by the Wine ABI smoke test (so a copy round trip works end to end
//! before the Metal back exists).

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use crate::adapter::AdapterInfo;
use crate::backend::*;
use crate::hresult::{Hr, Result};

const VA_START: u64 = 0x1_0000_0000;
const VA_ALIGN: u64 = 64 * 1024;
const MAX_FAKE_BUFFER: u64 = 1 << 30;

struct Buffer {
    ptr: *mut u8,
    size: usize,
    gpu_va: u64,
}

// SAFETY: the pointer is a private heap allocation owned by this record.
unsafe impl Send for Buffer {}

impl Drop for Buffer {
    fn drop(&mut self) {
        // SAFETY: allocated in `create_buffer` as a boxed slice of this length.
        unsafe { drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(self.ptr, self.size))) };
    }
}

#[derive(Default)]
struct Inner {
    next_id: u32,
    next_va: u64,
    buffers: HashMap<ResourceId, Buffer>,
    by_va: BTreeMap<u64, ResourceId>,
    next_pipeline: u32,
    pipelines: HashMap<PipelineId, usize>,
    /// Everything `submit(Execute)` was given, in order.
    executed: Vec<CommandStream>,
    /// Commands that could not be carried out (bad ids, out of range).
    rejected: Vec<String>,
    signals: Vec<u64>,
}

pub struct FakeBackend {
    adapter: AdapterInfo,
    inner: Mutex<Inner>,
}

impl FakeBackend {
    pub fn new() -> FakeBackend {
        Self::with_adapter(AdapterInfo::placeholder())
    }

    pub fn with_adapter(adapter: AdapterInfo) -> FakeBackend {
        FakeBackend { adapter, inner: Mutex::new(Inner { next_id: 1, next_va: VA_START, next_pipeline: 1, ..Default::default() }) }
    }

    pub fn executed(&self) -> Vec<CommandStream> {
        self.inner.lock().unwrap().executed.clone()
    }

    pub fn rejected(&self) -> Vec<String> {
        self.inner.lock().unwrap().rejected.clone()
    }

    pub fn live_buffers(&self) -> usize {
        self.inner.lock().unwrap().buffers.len()
    }

    pub fn live_pipelines(&self) -> usize {
        self.inner.lock().unwrap().pipelines.len()
    }

    /// Resource owning `gpu_va` and the offset inside it.
    pub fn resolve_va(&self, gpu_va: u64) -> Option<(ResourceId, u64)> {
        let g = self.inner.lock().unwrap();
        let (base, id) = g.by_va.range(..=gpu_va).next_back()?;
        let b = g.buffers.get(id)?;
        (gpu_va < base + b.size as u64).then_some((*id, gpu_va - base))
    }
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for FakeBackend {
    fn adapter(&self) -> AdapterInfo {
        self.adapter.clone()
    }

    fn create_buffer(&self, size: u64, heap: HeapKind) -> Result<BufferAlloc> {
        if size == 0 || size > MAX_FAKE_BUFFER {
            return Err(Hr::E_OUTOFMEMORY);
        }
        let mut g = self.inner.lock().unwrap();
        let id = g.next_id;
        g.next_id += 1;
        let gpu_va = g.next_va;
        g.next_va += size.div_ceil(VA_ALIGN) * VA_ALIGN;
        let boxed = vec![0u8; size as usize].into_boxed_slice();
        let ptr = Box::into_raw(boxed).cast::<u8>();
        g.buffers.insert(id, Buffer { ptr, size: size as usize, gpu_va });
        g.by_va.insert(gpu_va, id);
        let cpu_ptr = if heap == HeapKind::Default { 0 } else { ptr as usize };
        Ok(BufferAlloc { id, gpu_va, cpu_ptr })
    }

    fn destroy_buffer(&self, id: ResourceId) {
        let mut g = self.inner.lock().unwrap();
        if let Some(b) = g.buffers.remove(&id) {
            g.by_va.remove(&b.gpu_va);
        }
    }

    fn create_compute_pipeline(&self, root_signature: &[u8], shader: &[u8]) -> Result<PipelineId> {
        // The container magic is all the placeholder can check.
        if root_signature.is_empty() || shader.len() < 4 || &shader[..4] != b"DXBC" {
            return Err(Hr::E_INVALIDARG);
        }
        let mut g = self.inner.lock().unwrap();
        let id = g.next_pipeline;
        g.next_pipeline += 1;
        g.pipelines.insert(id, shader.len());
        Ok(id)
    }

    fn destroy_pipeline(&self, id: PipelineId) {
        self.inner.lock().unwrap().pipelines.remove(&id);
    }

    fn submit(&self, _queue: QueueId, work: Submission) -> Result<()> {
        match work {
            Submission::Execute(streams) => {
                let mut g = self.inner.lock().unwrap();
                for s in streams {
                    for c in &s.commands {
                        if let Command::CopyBuffer { dst, dst_offset, src, src_offset, size } = *c {
                            let ok = match (g.buffers.get(&dst), g.buffers.get(&src)) {
                                (Some(d), Some(s)) => {
                                    let in_range = |off: u64, len: usize| off.checked_add(size).is_some_and(|e| e <= len as u64);
                                    if in_range(dst_offset, d.size) && in_range(src_offset, s.size) {
                                        // SAFETY: both ranges are inside live allocations (checked above);
                                        // `copy` handles dst and src being the same buffer.
                                        unsafe {
                                            std::ptr::copy(
                                                s.ptr.add(src_offset as usize),
                                                d.ptr.add(dst_offset as usize),
                                                size as usize,
                                            )
                                        };
                                        true
                                    } else {
                                        false
                                    }
                                }
                                _ => false,
                            };
                            if !ok {
                                g.rejected.push(format!("{c:?}"));
                            }
                        }
                    }
                    g.executed.push(s);
                }
                Ok(())
            }
            Submission::Signal { fence, value } => {
                self.inner.lock().unwrap().signals.push(value);
                // Everything before it ran synchronously above.
                fence.signal(value);
                Ok(())
            }
            Submission::Wait { fence, value } => {
                if fence.completed() >= value { Ok(()) } else { Err(Hr::E_FAIL) }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_get_distinct_aligned_addresses() {
        let b = FakeBackend::new();
        let a = b.create_buffer(100, HeapKind::Upload).unwrap();
        let c = b.create_buffer(70_000, HeapKind::Default).unwrap();
        assert_eq!(a.gpu_va % VA_ALIGN, 0);
        assert!(c.gpu_va >= a.gpu_va + VA_ALIGN);
        assert_ne!(a.cpu_ptr, 0);
        assert_eq!(c.cpu_ptr, 0);
        assert_eq!(b.resolve_va(c.gpu_va + 69_999), Some((c.id, 69_999)));
        assert_eq!(b.resolve_va(c.gpu_va + 70_000), None);
        b.destroy_buffer(a.id);
        assert_eq!(b.live_buffers(), 1);
        assert_eq!(b.resolve_va(a.gpu_va), None);
    }

    #[test]
    fn copy_commands_copy_and_bad_ones_are_rejected() {
        let b = FakeBackend::new();
        let up = b.create_buffer(16, HeapKind::Upload).unwrap();
        let def = b.create_buffer(16, HeapKind::Default).unwrap();
        // SAFETY: the allocation is 16 bytes and lives until destroyed.
        unsafe { std::slice::from_raw_parts_mut(up.cpu_ptr as *mut u8, 16) }.copy_from_slice(&[7; 16]);
        let ok = Command::CopyBuffer { dst: def.id, dst_offset: 4, src: up.id, src_offset: 0, size: 8 };
        let bad = Command::CopyBuffer { dst: def.id, dst_offset: 12, src: up.id, src_offset: 0, size: 8 };
        b.submit(0, Submission::Execute(vec![CommandStream { commands: vec![ok, bad] }])).unwrap();
        assert_eq!(b.rejected().len(), 1);
        assert_eq!(b.executed().len(), 1);
    }

    #[test]
    fn pipelines_need_a_container() {
        let b = FakeBackend::new();
        assert_eq!(b.create_compute_pipeline(&[1], &[0; 64]), Err(Hr::E_INVALIDARG));
        let id = b.create_compute_pipeline(&[1], b"DXBCxxxx").unwrap();
        assert_eq!(b.live_pipelines(), 1);
        b.destroy_pipeline(id);
        assert_eq!(b.live_pipelines(), 0);
    }
}
