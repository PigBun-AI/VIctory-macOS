//! Buffers: the four kinds, `wrap`, GPU addresses, and the rule that a
//! physical page is behind at most one of them.
//!
//! Every buffer made here is put into a residency set at once (see
//! [`crate::residency`]) and registered in the [`AddressMap`], which lets
//! the descriptor writers check a raw GPU address the front end hands over
//! (a buffer location of the original API) against live, resident buffers.
//!
//! Destruction is automatic and deferred: dropping the last [`Buffer`]
//! handle retires the resource instead of freeing it; the GPU may still be
//! reading it (see [`Residency::retire`]).

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{MTLAllocation, MTLBuffer, MTLDevice, MTLResource, MTLResourceOptions};

use crate::device::{Device, DeviceInner};
use crate::error::{Error, Result};
use crate::pages::Claim;
use crate::residency::{Retired, SetKind};

/// What a buffer is for; decides storage and CPU cache mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferKind {
    /// GPU-only (`MTLStorageModePrivate`). No CPU pointer.
    Default,
    /// CPU writes, GPU reads (shared, write-combined).
    Upload,
    /// GPU writes, CPU reads (shared, default cache).
    Readback,
    /// Both sides read and write (shared, default cache).
    Shared,
    /// Caller memory behind a no-copy buffer ([`Device::wrap`]).
    Wrapped,
}

/// One live buffer in the [`AddressMap`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressEntry {
    /// Length in bytes.
    pub len: u64,
    /// Whether the buffer is in a residency set. Always true for buffers
    /// made by [`Device`]; `false` only for buffers made with
    /// [`Device::create_buffer_unmanaged`] (tests of the failure mode).
    pub resident: bool,
    /// Which kind it is.
    pub kind: BufferKind,
}

/// GPU address ranges of the live buffers.
#[derive(Debug, Default)]
pub struct AddressMap {
    map: BTreeMap<u64, AddressEntry>,
}

impl AddressMap {
    fn insert(&mut self, va: u64, entry: AddressEntry) {
        self.map.insert(va, entry);
    }

    fn remove(&mut self, va: u64) {
        self.map.remove(&va);
    }

    /// The buffer that contains `[va, va + len)`, with the offset of `va`
    /// inside it.
    pub fn find(&self, va: u64, len: u64) -> Option<(u64, AddressEntry)> {
        let (&base, &entry) = self.map.range(..=va).next_back()?;
        let end = va.checked_add(len)?;
        (end <= base + entry.len).then_some((base, entry))
    }

    /// Number of live buffers.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// True when there are none.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Releases a page claim when dropped.
struct PageGuard {
    pages: Arc<Mutex<crate::pages::PageRegistry>>,
    claim: Claim,
}

impl Drop for PageGuard {
    fn drop(&mut self) {
        let mut pages = self.pages.lock().unwrap_or_else(|e| e.into_inner());
        pages.release(&self.claim);
    }
}

pub(crate) struct BufferInner {
    pub(crate) raw: Retained<ProtocolObject<dyn MTLBuffer>>,
    device: Arc<DeviceInner>,
    kind: BufferKind,
    set: SetKind,
    len: u64,
    gpu_va: u64,
    cpu: Option<NonNull<u8>>,
    pages: Option<PageGuard>,
    managed: bool,
}

// The Metal buffer is thread-safe; `cpu` is a pointer into its memory that
// the callers synchronise like any shared memory.
unsafe impl Send for BufferInner {}
unsafe impl Sync for BufferInner {}

impl Drop for BufferInner {
    fn drop(&mut self) {
        self.device
            .addresses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(self.gpu_va);
        if !self.managed {
            return;
        }
        let object: Retained<ProtocolObject<dyn MTLAllocation>> =
            ProtocolObject::from_retained(self.raw.clone());
        let keep = self
            .pages
            .take()
            .map(|g| Box::new(g) as Box<dyn std::any::Any + Send>);
        self.device.residency.retire(Retired {
            object,
            kind: self.set,
            waits: self.device.in_flight_waits(),
            keep,
        });
    }
}

/// A buffer. Cloning shares it; the last drop retires it (deferred).
#[derive(Clone)]
pub struct Buffer {
    pub(crate) inner: Arc<BufferInner>,
}

impl std::fmt::Debug for Buffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Buffer")
            .field("kind", &self.inner.kind)
            .field("len", &self.inner.len)
            .field("gpu_va", &format_args!("{:#x}", self.inner.gpu_va))
            .finish()
    }
}

impl Buffer {
    /// `MTLBuffer.gpuAddress`. The front end's `GetGPUVirtualAddress`,
    /// root descriptors and descriptors carry this value unchanged.
    pub fn gpu_va(&self) -> u64 {
        self.inner.gpu_va
    }

    /// Length in bytes.
    pub fn len(&self) -> u64 {
        self.inner.len
    }

    /// `MTLBuffer.allocatedSize`: what the allocation really takes
    /// (`CreateBuffer` returns it as `alloc_size`).
    pub fn alloc_size(&self) -> u64 {
        MTLResource::allocatedSize(&*self.inner.raw) as u64
    }

    /// `cpu_ptr` as an address, 0 when there is none (`cpu_addr` of the
    /// wire calls).
    pub fn cpu_addr(&self) -> u64 {
        self.cpu_ptr().map_or(0, |p| p as u64)
    }

    /// True for a zero-length buffer (never made; kept for clippy).
    pub fn is_empty(&self) -> bool {
        self.inner.len == 0
    }

    /// The kind.
    pub fn kind(&self) -> BufferKind {
        self.inner.kind
    }

    /// The set it was put in.
    pub fn set(&self) -> SetKind {
        self.inner.set
    }

    /// CPU pointer to byte 0; `None` for [`BufferKind::Default`].
    pub fn cpu_ptr(&self) -> Option<*mut u8> {
        self.inner.cpu.map(NonNull::as_ptr)
    }

    /// Copies `bytes` to `offset`. Fails for GPU-only buffers and ranges
    /// outside the buffer. The caller makes sure the GPU is not using it.
    pub fn write(&self, offset: u64, bytes: &[u8]) -> Result<()> {
        let ptr = self.checked_ptr(offset, bytes.len() as u64)?;
        // SAFETY: the range is inside the buffer's CPU mapping.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len()) };
        Ok(())
    }

    /// Copies `out.len()` bytes from `offset`.
    pub fn read(&self, offset: u64, out: &mut [u8]) -> Result<()> {
        let ptr = self.checked_ptr(offset, out.len() as u64)?;
        // SAFETY: as in `write`.
        unsafe { std::ptr::copy_nonoverlapping(ptr, out.as_mut_ptr(), out.len()) };
        Ok(())
    }

    /// Reads `count` `u32`s starting at byte `offset`.
    pub fn read_u32s(&self, offset: u64, count: usize) -> Result<Vec<u32>> {
        let mut out = vec![0u32; count];
        let ptr = self.checked_ptr(offset, 4 * count as u64)?;
        // SAFETY: as in `write`; u32 reads of bytes via copy are alignment-free.
        unsafe { std::ptr::copy_nonoverlapping(ptr, out.as_mut_ptr().cast::<u8>(), 4 * count) };
        Ok(out)
    }

    /// Writes `values` as little-endian `u32`s at byte `offset`.
    pub fn write_u32s(&self, offset: u64, values: &[u32]) -> Result<()> {
        let ptr = self.checked_ptr(offset, 4 * values.len() as u64)?;
        // SAFETY: as in `write`.
        unsafe {
            std::ptr::copy_nonoverlapping(values.as_ptr().cast::<u8>(), ptr, 4 * values.len())
        };
        Ok(())
    }

    fn checked_ptr(&self, offset: u64, len: u64) -> Result<*mut u8> {
        let cpu = self
            .inner
            .cpu
            .ok_or_else(|| Error::BadArgument("buffer has no CPU mapping".into()))?;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| Error::BadArgument("range overflows".into()))?;
        if end > self.inner.len {
            return Err(Error::BadArgument(format!(
                "range {offset}..{end} is outside a buffer of {} bytes",
                self.inner.len
            )));
        }
        // SAFETY: offset < len of the mapping.
        Ok(unsafe { cpu.as_ptr().add(offset as usize) })
    }

    /// Whether this buffer is in a residency set.
    pub fn is_resident(&self) -> bool {
        self.inner.managed
            && self
                .inner
                .device
                .residency
                .contains(self.inner.set, &self.allocation())
    }

    pub(crate) fn raw(&self) -> &ProtocolObject<dyn MTLBuffer> {
        &self.inner.raw
    }

    fn allocation(&self) -> Retained<ProtocolObject<dyn MTLAllocation>> {
        ProtocolObject::from_retained(self.inner.raw.clone())
    }

    pub(crate) fn same(&self, other: &Buffer) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub(crate) fn belongs_to(&self, device: &Device) -> bool {
        Arc::ptr_eq(&self.inner.device, &device.inner)
    }
}

impl Device {
    /// A buffer of `len` bytes in the resource set.
    pub fn create_buffer(&self, kind: BufferKind, len: u64) -> Result<Buffer> {
        self.create_buffer_in(SetKind::Resource, kind, len)
    }

    /// A buffer in the set `set` (heaps, rings and indirect arguments use
    /// this).
    pub fn create_buffer_in(&self, set: SetKind, kind: BufferKind, len: u64) -> Result<Buffer> {
        self.make_buffer(set, kind, len, true)
    }

    /// A buffer that is **not** in any residency set. Only for tests of the
    /// failure mode "GPU reads something that was never made resident"; a
    /// real front end never needs it.
    pub fn create_buffer_unmanaged(&self, kind: BufferKind, len: u64) -> Result<Buffer> {
        self.make_buffer(SetKind::Resource, kind, len, false)
    }

    fn make_buffer(
        &self,
        set: SetKind,
        kind: BufferKind,
        len: u64,
        managed: bool,
    ) -> Result<Buffer> {
        if kind == BufferKind::Wrapped {
            return Err(Error::BadArgument(
                "use Device::wrap for wrapped buffers".into(),
            ));
        }
        if len == 0 {
            return Err(Error::BadArgument("zero-length buffer".into()));
        }
        if len > self.inner.caps.max_buffer_len {
            return Err(Error::BadArgument(format!(
                "{len} bytes exceed maxBufferLength {}",
                self.inner.caps.max_buffer_len
            )));
        }
        let options = match kind {
            BufferKind::Default => MTLResourceOptions::StorageModePrivate,
            BufferKind::Upload => {
                MTLResourceOptions::StorageModeShared
                    | MTLResourceOptions::CPUCacheModeWriteCombined
            }
            BufferKind::Readback | BufferKind::Shared => MTLResourceOptions::StorageModeShared,
            BufferKind::Wrapped => unreachable!("rejected above"),
        } | MTLResourceOptions::HazardTrackingModeUntracked;
        let raw = self
            .inner
            .raw
            .newBufferWithLength_options(len as usize, options)
            .ok_or_else(|| Error::Metal(format!("newBuffer({len}) returned nil")))?;
        let cpu = (kind != BufferKind::Default)
            .then(|| NonNull::new(raw.contents().as_ptr().cast::<u8>()))
            .flatten();
        self.finish_buffer(raw, kind, set, len, cpu, None, managed)
    }

    /// Wraps caller memory without copying (`newBufferWithBytesNoCopy`):
    /// the GPU reads and writes `ptr..ptr + len` itself.
    ///
    /// `ptr` and `len` must be multiples of the page size ([`Caps::page_size`](crate::device::Caps)):
    /// 4 KiB under Rosetta, 16 KiB natively. A range that shares a page with
    /// a live wrapped buffer is **refused** with [`Error::PageConflict`]:
    /// one physical page is behind at most one GPU buffer (layering rule 6).
    /// The buffer goes into the `Title` residency set.
    ///
    /// # Safety
    /// `ptr..ptr + len` must stay mapped for as long as the returned buffer
    /// (and, after its last drop, the GPU work that used it) lives.
    pub unsafe fn wrap(&self, ptr: *mut u8, len: usize) -> Result<Buffer> {
        let page = self.inner.caps.page_size as usize;
        if ptr.is_null()
            || !(ptr as usize).is_multiple_of(page)
            || len == 0
            || !len.is_multiple_of(page)
        {
            return Err(Error::BadArgument(format!(
                "wrap({ptr:p}, {len:#x}) is not aligned to the {page}-byte page"
            )));
        }
        if len as u64 > self.inner.caps.max_buffer_len {
            return Err(Error::BadArgument(format!(
                "{len} bytes exceed maxBufferLength {}",
                self.inner.caps.max_buffer_len
            )));
        }
        let claim = self
            .inner
            .pages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .claim(ptr as usize, len)?;
        let guard = PageGuard {
            pages: self.inner.pages.clone(),
            claim,
        };
        let nn = NonNull::new(ptr.cast::<c_void>()).expect("checked non-null");
        // SAFETY: forwarded from the caller; the claim is released by `guard`
        // on every error path below.
        let raw = unsafe {
            self.inner
                .raw
                .newBufferWithBytesNoCopy_length_options_deallocator(
                    nn,
                    len,
                    MTLResourceOptions::StorageModeShared
                        | MTLResourceOptions::HazardTrackingModeUntracked,
                    None,
                )
        }
        .ok_or_else(|| {
            Error::Metal(format!(
                "newBufferWithBytesNoCopy({ptr:p}, {len:#x}) returned nil"
            ))
        })?;
        self.finish_buffer(
            raw,
            BufferKind::Wrapped,
            SetKind::Title,
            len as u64,
            NonNull::new(ptr),
            Some(guard),
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_buffer(
        &self,
        raw: Retained<ProtocolObject<dyn MTLBuffer>>,
        kind: BufferKind,
        set: SetKind,
        len: u64,
        cpu: Option<NonNull<u8>>,
        pages: Option<PageGuard>,
        managed: bool,
    ) -> Result<Buffer> {
        raw.setLabel(Some(&NSString::from_str(match kind {
            BufferKind::Default => "victory default",
            BufferKind::Upload => "victory upload",
            BufferKind::Readback => "victory readback",
            BufferKind::Shared => "victory shared",
            BufferKind::Wrapped => "victory title memory",
        })));
        let gpu_va = raw.gpuAddress();
        if managed {
            let allocation: &ProtocolObject<dyn MTLAllocation> = ProtocolObject::from_ref(&*raw);
            self.inner.residency.add(set, allocation);
        }
        self.inner
            .addresses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                gpu_va,
                AddressEntry {
                    len,
                    resident: managed,
                    kind,
                },
            );
        Ok(Buffer {
            inner: Arc::new(BufferInner {
                raw,
                device: self.inner.clone(),
                kind,
                set,
                len,
                gpu_va,
                cpu,
                pages,
                managed,
            }),
        })
    }

    /// The live buffer behind a raw GPU address range, if any: the check a
    /// descriptor write or a root descriptor makes on what the front end
    /// sent. Returns the buffer's base address, and its entry.
    pub fn resolve_va(&self, va: u64, len: u64) -> Option<(u64, AddressEntry)> {
        self.inner
            .addresses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .find(va, len)
    }

    /// Number of live buffers (for the tests and stats).
    pub fn live_buffers(&self) -> usize {
        self.inner
            .addresses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_map_finds_ranges() {
        let mut m = AddressMap::default();
        let e = |len| AddressEntry {
            len,
            resident: true,
            kind: BufferKind::Default,
        };
        m.insert(0x1000, e(0x100));
        m.insert(0x2000, e(0x10));
        assert_eq!(m.find(0x1000, 0x100).map(|x| x.0), Some(0x1000));
        assert_eq!(m.find(0x1080, 0x10).map(|x| x.0), Some(0x1000));
        assert!(m.find(0x10f0, 0x20).is_none(), "crosses the end");
        assert!(m.find(0x1100, 1).is_none(), "one past the end");
        assert!(m.find(0xfff, 1).is_none());
        assert!(m.find(0x2005, 4).is_some());
        assert!(m.find(u64::MAX - 2, 8).is_none(), "overflow");
        m.remove(0x1000);
        assert!(m.find(0x1000, 1).is_none());
        assert_eq!(m.len(), 1);
    }
}
