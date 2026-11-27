//! Placement heaps: memory buffers and textures are placed in at an offset,
//! so that resources can share bytes (the original API's placed resources
//! and aliasing).
//!
//! A [`Heap`] is one `MTLHeap` of type placement, untracked, in the
//! `Resource` residency set. The resources placed in it are **not** added to
//! a set themselves (the heap is: an allocation placed in a resident heap is
//! resident with it; checked by the GPU tests in `tests/texture_gpu.rs`, which
//! read placed buffers and textures through bindless descriptors). They hold
//! the heap alive: the heap is retired after the last resource placed in it,
//! after the GPU work that may touch it (see [`crate::residency`]).
//!
//! # Aliasing
//!
//! Two resources placed on the same bytes are two views of one memory. Metal
//! does not order work on one against work on the other: the front sends
//! `BARRIER{ALIASING}` between uses, which the backend turns into a barrier
//! over every stage with resource-alias visibility
//! ([`CommandList::aliasing_barrier`](crate::record::CommandList::aliasing_barrier)).
//! After the barrier the new resource's contents are undefined unless the
//! program wrote them (as in the original API); a texture needs a clear or a
//! full copy first. Nothing here makes a placed resource `aliasable`
//! (`makeAliasable`): the heap's memory is simply reused by the program.
//!
//! # Size and alignment
//!
//! [`Device::heap_buffer_size_align`] and [`Device::heap_texture_size_align`]
//! return Metal's `heapBufferSizeAndAlign` / `heapTextureSizeAndAlign`: the
//! size a resource takes from a heap and the alignment of its offset. A
//! placement at an offset that is not a multiple of the alignment, or that
//! reaches past the heap's end, is refused ([`Error::BadArgument`]).

use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTLAllocation, MTLBuffer, MTLDevice, MTLHeap, MTLHeapDescriptor, MTLHeapType,
    MTLResourceOptions,
};

use crate::device::{Device, DeviceInner};
use crate::error::{Error, Result};
use crate::memory::{Buffer, BufferKind, resource_options};
use crate::residency::{Retired, SetKind};
use crate::texture::{Texture, TextureDesc};

pub(crate) struct HeapInner {
    raw: Retained<ProtocolObject<dyn MTLHeap>>,
    device: Arc<DeviceInner>,
    kind: BufferKind,
    size: u64,
}

// `MTLHeap` is thread-safe.
unsafe impl Send for HeapInner {}
unsafe impl Sync for HeapInner {}

impl Drop for HeapInner {
    fn drop(&mut self) {
        let object: Retained<ProtocolObject<dyn MTLAllocation>> =
            ProtocolObject::from_retained(self.raw.clone());
        self.device.residency.retire(Retired {
            object,
            kind: SetKind::Resource,
            waits: self.device.in_flight_waits(),
            keep: None,
        });
    }
}

/// A placement heap. Cloning shares it; the last drop retires it (deferred).
#[derive(Clone)]
pub struct Heap {
    pub(crate) inner: Arc<HeapInner>,
}

impl std::fmt::Debug for Heap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Heap")
            .field("kind", &self.inner.kind)
            .field("size", &self.inner.size)
            .finish()
    }
}

/// Size and alignment a resource needs from a heap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SizeAlign {
    /// Bytes the resource takes in the heap.
    pub size: u64,
    /// Required alignment of its offset.
    pub align: u64,
}

impl Device {
    /// A placement heap of at least `size` bytes. `kind` decides where the
    /// memory lives: [`BufferKind::Default`] private (buffers and textures),
    /// the others CPU-visible (buffers only).
    pub fn create_heap(&self, kind: BufferKind, size: u64) -> Result<Heap> {
        if kind == BufferKind::Wrapped {
            return Err(Error::BadArgument("a heap cannot wrap memory".into()));
        }
        if size == 0 || size > self.inner.caps.max_buffer_len {
            return Err(Error::BadArgument(format!(
                "heap of {size} bytes (maxBufferLength {})",
                self.inner.caps.max_buffer_len
            )));
        }
        let descriptor = MTLHeapDescriptor::new();
        descriptor.setType(MTLHeapType::Placement);
        descriptor.setSize(size as usize);
        descriptor.setResourceOptions(resource_options(kind));
        let raw = self
            .inner
            .raw
            .newHeapWithDescriptor(&descriptor)
            .ok_or_else(|| Error::Metal(format!("newHeap({size} bytes, {kind:?}) returned nil")))?;
        raw.setLabel(Some(&NSString::from_str("victory placement heap")));
        let size = MTLHeap::size(&*raw) as u64;
        let allocation: &ProtocolObject<dyn MTLAllocation> = ProtocolObject::from_ref(&*raw);
        self.inner.residency.add(SetKind::Resource, allocation);
        Ok(Heap {
            inner: Arc::new(HeapInner {
                raw,
                device: self.inner.clone(),
                kind,
                size,
            }),
        })
    }

    /// What a buffer of `len` bytes takes from a heap of `kind`.
    pub fn heap_buffer_size_align(&self, kind: BufferKind, len: u64) -> SizeAlign {
        let r = self
            .inner
            .raw
            .heapBufferSizeAndAlignWithLength_options(len as usize, resource_options(kind));
        SizeAlign {
            size: r.size as u64,
            align: r.align as u64,
        }
    }

    /// What a texture of this description takes from a (private) heap.
    pub fn heap_texture_size_align(&self, desc: &TextureDesc) -> Result<SizeAlign> {
        let d = desc.metal_descriptor()?;
        let r = self.inner.raw.heapTextureSizeAndAlignWithDescriptor(&d);
        Ok(SizeAlign {
            size: r.size as u64,
            align: r.align as u64,
        })
    }
}

impl Heap {
    /// Bytes the heap really has.
    pub fn size(&self) -> u64 {
        self.inner.size
    }

    /// The kind it was made with.
    pub fn kind(&self) -> BufferKind {
        self.inner.kind
    }

    pub(crate) fn belongs_to(&self, device: &Device) -> bool {
        Arc::ptr_eq(&self.inner.device, &device.inner)
    }

    fn check_placement(&self, offset: u64, size: u64, align: u64, what: &str) -> Result<()> {
        if align > 1 && !offset.is_multiple_of(align) {
            return Err(Error::BadArgument(format!(
                "{what} at heap offset {offset}: not a multiple of the alignment {align}"
            )));
        }
        if offset
            .checked_add(size)
            .is_none_or(|end| end > self.inner.size)
        {
            return Err(Error::BadArgument(format!(
                "{what} of {size} bytes at heap offset {offset} reaches past the heap's {} bytes",
                self.inner.size
            )));
        }
        Ok(())
    }

    /// A buffer of `len` bytes at `offset` of the heap. It has the heap's
    /// kind (a CPU pointer for upload and readback heaps).
    pub fn create_buffer(&self, device: &Device, offset: u64, len: u64) -> Result<Buffer> {
        if !self.belongs_to(device) {
            return Err(Error::BadArgument("heap of another device".into()));
        }
        if len == 0 {
            return Err(Error::BadArgument("zero-length buffer".into()));
        }
        let kind = self.inner.kind;
        let sa = device.heap_buffer_size_align(kind, len);
        self.check_placement(offset, sa.size, sa.align, "buffer")?;
        let options: MTLResourceOptions = resource_options(kind);
        // SAFETY: the offset is aligned and the range inside the heap (checked
        // above).
        let raw = unsafe {
            self.inner.raw.newBufferWithLength_options_offset(
                len as usize,
                options,
                offset as usize,
            )
        }
        .ok_or_else(|| {
            Error::Metal(format!(
                "newBuffer({len}) at heap offset {offset} returned nil"
            ))
        })?;
        let cpu = (kind != BufferKind::Default)
            .then(|| std::ptr::NonNull::new(raw.contents().as_ptr().cast::<u8>()))
            .flatten();
        device.finish_buffer(
            raw,
            kind,
            SetKind::Resource,
            len,
            cpu,
            None,
            true,
            Some(self.clone()),
        )
    }

    /// A texture at `offset` of the heap (private heaps only).
    pub fn create_texture(
        &self,
        device: &Device,
        offset: u64,
        desc: &TextureDesc,
    ) -> Result<Texture> {
        if !self.belongs_to(device) {
            return Err(Error::BadArgument("heap of another device".into()));
        }
        if self.inner.kind != BufferKind::Default {
            return Err(Error::Unsupported(
                "textures can only be placed in a default (private) heap".into(),
            ));
        }
        let sa = device.heap_texture_size_align(desc)?;
        self.check_placement(offset, sa.size, sa.align, "texture")?;
        let d = desc.metal_descriptor()?;
        // SAFETY: as above.
        let raw = unsafe {
            self.inner
                .raw
                .newTextureWithDescriptor_offset(&d, offset as usize)
        }
        .ok_or_else(|| {
            Error::Metal(format!(
                "newTexture({desc:?}) at heap offset {offset} returned nil"
            ))
        })?;
        Texture::finish(device, raw, *desc, Some(self.clone()))
    }
}
