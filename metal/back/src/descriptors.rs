//! Bindings: descriptor heaps, the constant ring and the argument table.
//!
//! The shader side is what Metal Shader Converter emits: a *top-level
//! argument buffer* (TLAB) at bind point 2 holding root constants, root
//! descriptors (64-bit GPU addresses) and pointers to descriptor tables;
//! descriptor tables are arrays of [`IrDescriptorTableEntry`] in a heap
//! buffer at bind point 0 (resources) or 1 (samplers).
//!
//! Metal 4 has no inline byte binding. Root constants and root descriptors are
//! written into the per-submission [`ConstantRing`] at the offsets the
//! caller lays out (the layout comes later from MSC reflection:
//! `IRRootSignatureGetResourceLocations`; MSC inserts alignment padding, so
//! never compute it as "index times size"), and the ring address is bound
//! with `MTL4ArgumentTable setAddress:atIndex:` ([`ArgumentTable`]).
//!
//! # Where the entry encoding comes from
//!
//! [`IrDescriptorTableEntry`] and the encoders below follow
//! `metal_irconverter_runtime.h` (`IRDescriptorTableEntry`,
//! `IRDescriptorTableSetBuffer`, `IRDescriptorTableGetBufferMetadata`,
//! `IRDescriptorTableSetTexture`, `IRDescriptorTableSetSampler`, the
//! `kIR*BindPoint` and `kIRBufSize*`/`kIRTexView*`/`kIRTypedBuffer*`
//! constants). Checked on 2026-11-27 against the header installed with
//! Metal Shader Converter 4.0.3 (`/usr/local/include/metal_irconverter_runtime/
//! metal_irconverter_runtime.h`, `IR_VERSION` 4.0.3, "4.0 beta 3" in
//! saxaboom's naming): the entry layout, the metadata masks and shifts and
//! the bind points below match it; it differs from the copy first used
//! here (saxaboom `efd8085`, `bindings_generator/vendor/`, PR #40) only in
//! `extern` declarations of the `kIR*` constants and where the
//! `IR_PRIVATE_IMPLEMENTATION` block sits, not in any value. Details and
//! the comparison are in docs/metal/msc.md. The prose of Apple's
//! `apple/game-porting-toolkit` `docs/metal-shader-converter/
//! binding-model.md` (`4f42344`, 2026-06-07) agrees except for the point
//! below.
//!
//! One place where the prose and the header disagree: the prose says the
//! texture-buffer-view offset in a buffer entry's metadata is "stored in 2
//! bytes"; the header masks it with `kIRTexViewMask = 0xff` (8 bits) at
//! bit 32. This crate follows the header (the code is what MSC's shaders
//! decode). Buffer length is masked to 32 bits by the header too, so a view
//! of 4 GiB or more cannot be expressed; [`buffer_metadata`] refuses it.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{MTL4ArgumentTable, MTL4ArgumentTableDescriptor, MTLDevice};

use crate::device::Device;
use crate::error::{Error, Result};
use crate::memory::{Buffer, BufferKind};
use crate::residency::SetKind;
use crate::typed::{TypedFormat, ViewAccess};

/// Bind points of the MSC runtime (`kIR*BindPoint`, runtime header).
pub mod bind_point {
    /// `kIRDescriptorHeapBindPoint`: the resource descriptor heap.
    pub const DESCRIPTOR_HEAP: u64 = 0;
    /// `kIRSamplerHeapBindPoint`: the sampler heap.
    pub const SAMPLER_HEAP: u64 = 1;
    /// `kIRArgumentBufferBindPoint`: the top-level argument buffer.
    pub const ARGUMENT_BUFFER: u64 = 2;
    /// `kIRArgumentBufferHullDomainBindPoint`.
    pub const ARGUMENT_BUFFER_HULL_DOMAIN: u64 = 3;
    /// `kIRArgumentBufferDrawArgumentsBindPoint`.
    pub const DRAW_ARGUMENTS: u64 = 4;
    /// `kIRArgumentBufferUniformsBindPoint`.
    pub const UNIFORMS: u64 = 5;
    /// `kIRVertexBufferBindPoint`.
    pub const VERTEX_BUFFER: u64 = 6;
    /// `kIRStageInAttributeStartIndex`.
    pub const STAGE_IN_ATTRIBUTE_START: u64 = 11;
}

/// `kIRBufSizeOffset`.
pub const BUF_SIZE_OFFSET: u64 = 0;
/// `kIRBufSizeMask`.
pub const BUF_SIZE_MASK: u64 = 0xffff_ffff;
/// `kIRTexViewOffset`.
pub const TEX_VIEW_OFFSET: u64 = 32;
/// `kIRTexViewMask`.
pub const TEX_VIEW_MASK: u64 = 0xff;
/// `kIRTypedBufferOffset`.
pub const TYPED_BUFFER_OFFSET: u64 = 63;

/// `IRDescriptorTableEntry`: three 64-bit words.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IrDescriptorTableEntry {
    /// Buffer GPU address (buffers); sampler `gpuResourceID` (samplers); 0
    /// (textures).
    pub gpu_va: u64,
    /// Texture `gpuResourceID` (textures, and the texture view of a typed
    /// buffer); 0 otherwise.
    pub texture_view_id: u64,
    /// Buffer metadata / texture min-LOD clamp / sampler LOD bias, see the
    /// constructors.
    pub metadata: u64,
}

const _: () = assert!(size_of::<IrDescriptorTableEntry>() == 24);
const _: () = assert!(align_of::<IrDescriptorTableEntry>() == 8);

/// Size of one entry in bytes.
pub const ENTRY_SIZE: u64 = size_of::<IrDescriptorTableEntry>() as u64;

/// `IRDescriptorTableGetBufferMetadata` for a view of `size` bytes with no
/// texture-buffer view (`texture_view_offset_elements` is the 16-byte
/// alignment padding of a texture buffer view; 0 without one).
pub fn buffer_metadata(size: u64, texture_view_offset_elements: u32, typed: bool) -> Result<u64> {
    if size > BUF_SIZE_MASK {
        return Err(Error::BadArgument(format!(
            "a buffer view of {size} bytes does not fit the 32-bit length of the MSC descriptor"
        )));
    }
    Ok(((size & BUF_SIZE_MASK) << BUF_SIZE_OFFSET)
        | ((u64::from(texture_view_offset_elements) & TEX_VIEW_MASK) << TEX_VIEW_OFFSET)
        | (u64::from(typed) << TYPED_BUFFER_OFFSET))
}

impl IrDescriptorTableEntry {
    /// `IRDescriptorTableSetBuffer(entry, gpu_va, metadata)`.
    pub fn buffer(gpu_va: u64, metadata: u64) -> Self {
        IrDescriptorTableEntry {
            gpu_va,
            texture_view_id: 0,
            metadata,
        }
    }

    /// A raw (untyped) buffer view: `buffer(gpu_va, buffer_metadata(size, 0, false))`.
    pub fn raw_buffer(gpu_va: u64, size: u64) -> Result<Self> {
        Ok(Self::buffer(gpu_va, buffer_metadata(size, 0, false)?))
    }

    /// `IRDescriptorTableSetTexture(entry, texture, min_lod_clamp, metadata)`
    /// given the texture's `gpuResourceID`. The header says `metadata` must
    /// be zero.
    pub fn texture(resource_id: u64, min_lod_clamp: f32, metadata: u32) -> Self {
        IrDescriptorTableEntry {
            gpu_va: 0,
            texture_view_id: resource_id,
            metadata: u64::from(min_lod_clamp.to_bits()) | (u64::from(metadata) << 32),
        }
    }

    /// `IRDescriptorTableSetSampler(entry, sampler, lod_bias)` given the
    /// sampler's `gpuResourceID`.
    pub fn sampler(resource_id: u64, lod_bias: f32) -> Self {
        IrDescriptorTableEntry {
            gpu_va: resource_id,
            texture_view_id: 0,
            metadata: u64::from(lod_bias.to_bits()),
        }
    }

    /// The buffer length encoded in `metadata` (bytes).
    pub fn buffer_size(&self) -> u64 {
        (self.metadata >> BUF_SIZE_OFFSET) & BUF_SIZE_MASK
    }

    /// Whether `metadata` marks a typed buffer.
    pub fn is_typed_buffer(&self) -> bool {
        self.metadata >> TYPED_BUFFER_OFFSET & 1 == 1
    }

    fn to_bytes(self) -> [u8; 24] {
        let mut out = [0u8; 24];
        out[0..8].copy_from_slice(&self.gpu_va.to_le_bytes());
        out[8..16].copy_from_slice(&self.texture_view_id.to_le_bytes());
        out[16..24].copy_from_slice(&self.metadata.to_le_bytes());
        out
    }

    fn from_bytes(b: &[u8; 24]) -> Self {
        let u = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().expect("8 bytes"));
        IrDescriptorTableEntry {
            gpu_va: u(0),
            texture_view_id: u(8),
            metadata: u(16),
        }
    }
}

/// A descriptor heap: one `MTLBuffer` of [`IrDescriptorTableEntry`]s in the
/// heap residency set. Used for the resource heap (bind point 0) and, with
/// sampler entries, for the sampler heap (bind point 1).
#[derive(Clone)]
pub struct DescriptorHeap {
    device: Device,
    buffer: Buffer,
    capacity: u32,
}

impl DescriptorHeap {
    /// A heap of `capacity` entries, all zero.
    pub fn new(device: &Device, capacity: u32) -> Result<DescriptorHeap> {
        if capacity == 0 {
            return Err(Error::BadArgument("empty descriptor heap".into()));
        }
        let buffer = device.create_buffer_in(
            SetKind::Heap,
            BufferKind::Shared,
            u64::from(capacity) * ENTRY_SIZE,
        )?;
        buffer.write(0, &vec![0u8; (u64::from(capacity) * ENTRY_SIZE) as usize])?;
        Ok(DescriptorHeap {
            device: device.clone(),
            buffer,
            capacity,
        })
    }

    /// Number of entries.
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Size of one slot in bytes (`CreateDescHeap` returns this as `stride`).
    pub fn stride(&self) -> u32 {
        ENTRY_SIZE as u32
    }

    /// CPU address of slot 0 (`CreateDescHeap` returns it as `cpu_addr`).
    pub fn cpu_addr(&self) -> u64 {
        self.buffer.cpu_ptr().map_or(0, |p| p as u64)
    }

    /// GPU address of entry 0: what is bound at bind point 0 / 1.
    pub fn gpu_va(&self) -> u64 {
        self.buffer.gpu_va()
    }

    /// GPU address of entry `index`: what a root descriptor table holds
    /// (the table starts at the heap entry the front end's handle names).
    pub fn entry_va(&self, index: u32) -> u64 {
        self.buffer.gpu_va() + u64::from(index) * ENTRY_SIZE
    }

    /// The heap's buffer.
    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// Writes an entry as it is.
    pub fn write(&self, index: u32, entry: IrDescriptorTableEntry) -> Result<()> {
        if index >= self.capacity {
            return Err(Error::BadArgument(format!(
                "descriptor {index} in a heap of {}",
                self.capacity
            )));
        }
        self.buffer
            .write(u64::from(index) * ENTRY_SIZE, &entry.to_bytes())
    }

    /// Reads an entry back.
    pub fn read(&self, index: u32) -> Result<IrDescriptorTableEntry> {
        if index >= self.capacity {
            return Err(Error::BadArgument(format!(
                "descriptor {index} in a heap of {}",
                self.capacity
            )));
        }
        let mut b = [0u8; 24];
        self.buffer.read(u64::from(index) * ENTRY_SIZE, &mut b)?;
        Ok(IrDescriptorTableEntry::from_bytes(&b))
    }

    /// Writes a buffer view the front end named by GPU address (`SRV`/`UAV`/
    /// `CBV` of a buffer). The range must lie inside a live buffer that is in
    /// a residency set: a descriptor to anything else would make the GPU read
    /// memory Metal 4 does not know it needs to keep resident, so it is
    /// **rejected** ([`Error::NotResident`]) instead of written.
    pub fn write_buffer_view(&self, index: u32, gpu_va: u64, size: u64, typed: bool) -> Result<()> {
        match self.device.resolve_va(gpu_va, size.max(1)) {
            None => {
                return Err(Error::NotResident(format!(
                    "no live buffer contains {gpu_va:#x}..+{size:#x}"
                )));
            }
            Some((_, entry)) if !entry.resident => {
                return Err(Error::NotResident(format!(
                    "the buffer behind {gpu_va:#x} is in no residency set"
                )));
            }
            Some(_) => {}
        }
        self.write(
            index,
            IrDescriptorTableEntry::buffer(gpu_va, buffer_metadata(size, 0, typed)?),
        )
    }

    /// Writes a typed buffer view (`Buffer<T>` / `RWBuffer<T>`): `num_elements`
    /// elements of `format` starting at element `first_element` of `buffer`,
    /// as MSC reads it: the buffer address, a texture buffer over the same
    /// bytes ([`Buffer::texture_view`]), the length in bytes and the typed flag.
    ///
    /// Errors: not resident, outside the buffer, or a start that is not a
    /// multiple of the texture-buffer alignment ([`Error::Unsupported`], see
    /// [`crate::typed`]); nothing is written then.
    pub fn write_typed_buffer(
        &self,
        index: u32,
        buffer: &Buffer,
        format: &TypedFormat,
        first_element: u64,
        num_elements: u64,
        access: ViewAccess,
    ) -> Result<()> {
        let bytes = u64::from(format.bytes);
        let (Some(first), Some(size)) = (
            first_element.checked_mul(bytes),
            // The descriptor's length is the texture's: an expanded mirror's elements are bigger.
            num_elements.checked_mul(u64::from(format.mirror_bytes())),
        ) else {
            return Err(Error::BadArgument("typed view range overflows".into()));
        };
        crate::typed::check_access(format, access)?;
        self.check_resident(buffer.gpu_va(), 1)?;
        let alignment = self.device.typed_view_alignment(format);
        if access == ViewAccess::Read && (format.three_channel || !first.is_multiple_of(alignment))
        {
            // Not a texture-buffer start: read an aligned copy of the range
            // (see `crate::mirror`).
            let m = buffer.typed_mirror(format, first, num_elements)?;
            return self.write(
                index,
                IrDescriptorTableEntry {
                    gpu_va: m.gpu_va,
                    texture_view_id: m.resource_id,
                    metadata: buffer_metadata(size, 0, true)?,
                },
            );
        }
        let view = buffer.texture_view(format, first, num_elements, access)?;
        self.write(
            index,
            IrDescriptorTableEntry {
                gpu_va: buffer.gpu_va() + first,
                texture_view_id: view.resource_id,
                metadata: buffer_metadata(size, 0, true)?,
            },
        )
    }

    fn check_resident(&self, gpu_va: u64, len: u64) -> Result<()> {
        match self.device.resolve_va(gpu_va, len) {
            None => Err(Error::NotResident(format!(
                "no live buffer contains {gpu_va:#x}"
            ))),
            Some((_, entry)) if !entry.resident => Err(Error::NotResident(format!(
                "the buffer behind {gpu_va:#x} is in no residency set"
            ))),
            Some(_) => Ok(()),
        }
    }

    /// [`write_buffer_view`](Self::write_buffer_view) for a whole [`Buffer`].
    pub fn write_buffer(&self, index: u32, buffer: &Buffer, typed: bool) -> Result<()> {
        self.write_buffer_view(index, buffer.gpu_va(), buffer.len(), typed)
    }
}

/// Why the constant ring cannot serve an allocation right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingFull {
    /// The request is larger than a segment.
    TooBig,
    /// Every segment is in use by unsealed (not yet submitted) work.
    Open,
    /// The next segment is busy until the timeline reaches this value.
    Busy(u64),
}

/// A piece of the constant ring.
#[derive(Clone, Copy, Debug)]
pub struct RingAlloc {
    /// GPU address to bind (`setAddress:atIndex:`).
    pub gpu_va: u64,
    /// CPU pointer to the same bytes.
    ptr: *mut u8,
    /// Length in bytes.
    pub len: u64,
}

// The ring memory is shared with the GPU by design; synchronisation is by
// the segment scheme.
unsafe impl Send for RingAlloc {}
unsafe impl Sync for RingAlloc {}

impl RingAlloc {
    /// Copies `bytes` to byte `offset` of the allocation.
    pub fn write_bytes(&self, offset: u64, bytes: &[u8]) -> Result<()> {
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| Error::BadArgument("range overflows".into()))?;
        if end > self.len {
            return Err(Error::BadArgument(format!(
                "write {offset}..{end} outside a ring allocation of {} bytes",
                self.len
            )));
        }
        // SAFETY: inside the allocation, which is inside the ring's buffer.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.ptr.add(offset as usize),
                bytes.len(),
            )
        };
        Ok(())
    }

    /// Writes 32-bit root constants at byte `offset` (4-byte aligned).
    pub fn write_u32s(&self, offset: u64, values: &[u32]) -> Result<()> {
        if !offset.is_multiple_of(4) {
            return Err(Error::BadArgument(
                "32-bit constants need 4-byte alignment".into(),
            ));
        }
        let mut bytes = Vec::with_capacity(values.len() * 4);
        for v in values {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        self.write_bytes(offset, &bytes)
    }

    /// Writes a 64-bit value (a root descriptor's GPU address, or the GPU
    /// address of a descriptor table) at byte `offset` (8-byte aligned).
    pub fn write_u64(&self, offset: u64, value: u64) -> Result<()> {
        if !offset.is_multiple_of(8) {
            return Err(Error::BadArgument(
                "64-bit values need 8-byte alignment".into(),
            ));
        }
        self.write_bytes(offset, &value.to_le_bytes())
    }

    /// Zeroes the allocation.
    pub fn zero(&self) {
        // SAFETY: the allocation's own bytes.
        unsafe { std::ptr::write_bytes(self.ptr, 0, self.len as usize) };
    }
}

/// The per-submission constant ring: one upload buffer split in segments.
/// A segment is reused only after the submission that last touched it has
/// completed on the queue's timeline.
pub struct ConstantRing {
    buffer: Buffer,
    segment: u64,
    busy: Vec<u64>,
    touched: Vec<usize>,
    cur: usize,
    off: u64,
}

/// A segment that has been handed out and not yet sealed by a submission.
const OPEN: u64 = u64::MAX;

impl ConstantRing {
    /// A ring of `segments` segments of `segment_bytes` bytes (the latter a
    /// multiple of 256).
    pub fn new(device: &Device, segment_bytes: u64, segments: u32) -> Result<ConstantRing> {
        if segment_bytes == 0 || !segment_bytes.is_multiple_of(256) || segments == 0 {
            return Err(Error::BadArgument(
                "ring segments must be a non-zero multiple of 256 bytes".into(),
            ));
        }
        let buffer = device.create_buffer_in(
            SetKind::Heap,
            BufferKind::Upload,
            segment_bytes * u64::from(segments),
        )?;
        Ok(ConstantRing {
            buffer,
            segment: segment_bytes,
            busy: vec![0; segments as usize],
            touched: Vec::new(),
            cur: 0,
            off: 0,
        })
    }

    /// GPU address of the ring's first byte.
    pub fn base_va(&self) -> u64 {
        self.buffer.gpu_va()
    }

    /// Bytes of one segment.
    pub fn segment_bytes(&self) -> u64 {
        self.segment
    }

    /// `size` bytes aligned to `align` (a power of two up to 256), or the
    /// reason there is no room. `completed` is the timeline's completed
    /// value.
    pub fn try_alloc(
        &mut self,
        size: u64,
        align: u64,
        completed: u64,
    ) -> std::result::Result<RingAlloc, RingFull> {
        assert!(align.is_power_of_two() && align <= 256, "align {align}");
        if size > self.segment {
            return Err(RingFull::TooBig);
        }
        let mut off = (self.off + align - 1) & !(align - 1);
        if off + size > self.segment {
            let next = (self.cur + 1) % self.busy.len();
            if next == self.cur {
                // A single-segment ring has nowhere to move to.
                return Err(RingFull::Open);
            }
            match self.busy[next] {
                OPEN => return Err(RingFull::Open),
                v if v > completed => return Err(RingFull::Busy(v)),
                _ => {}
            }
            self.cur = next;
            off = 0;
        }
        self.busy[self.cur] = OPEN;
        if !self.touched.contains(&self.cur) {
            self.touched.push(self.cur);
        }
        self.off = off + size;
        let byte = self.cur as u64 * self.segment + off;
        Ok(RingAlloc {
            gpu_va: self.buffer.gpu_va() + byte,
            // SAFETY: `byte + size` is inside the ring's buffer.
            ptr: unsafe {
                self.buffer
                    .cpu_ptr()
                    .expect("upload buffer")
                    .add(byte as usize)
            },
            len: size,
        })
    }

    /// A submission carrying everything allocated since the last seal has
    /// been handed to the queue with timeline value `value`.
    pub fn seal(&mut self, value: u64) {
        for s in self.touched.drain(..) {
            self.busy[s] = value;
        }
    }
}

/// An `MTL4ArgumentTable` with the buffer slots the MSC bind points use.
#[derive(Clone)]
pub struct ArgumentTable {
    raw: Retained<ProtocolObject<dyn MTL4ArgumentTable>>,
    max_buffers: u32,
}

// The table is only mutated by the one recorder that owns it.
unsafe impl Send for ArgumentTable {}
unsafe impl Sync for ArgumentTable {}

impl ArgumentTable {
    /// A table with `max_buffers` buffer slots (31 is Metal's limit; the MSC
    /// bind points use 0..=2 for compute).
    pub fn new(device: &Device, max_buffers: u32) -> Result<ArgumentTable> {
        let descriptor = MTL4ArgumentTableDescriptor::new();
        descriptor.setMaxBufferBindCount(max_buffers as usize);
        // Vertex buffers of render encoders carry their stride here
        // (`MTLBufferLayoutStrideDynamic`, see `render.rs`).
        descriptor.setSupportAttributeStrides(true);
        descriptor.setLabel(Some(&NSString::from_str("victory argument table")));
        let raw = device
            .raw()
            .newArgumentTableWithDescriptor_error(&descriptor)
            .map_err(|e| Error::Metal(format!("argument table: {}", e.localizedDescription())))?;
        Ok(ArgumentTable { raw, max_buffers })
    }

    /// `setAddress:atIndex:`.
    pub fn set_address(&self, index: u32, gpu_va: u64) -> Result<()> {
        if index >= self.max_buffers {
            return Err(Error::BadArgument(format!(
                "argument table slot {index} of {}",
                self.max_buffers
            )));
        }
        // SAFETY: the index was checked against the table's size.
        unsafe { self.raw.setAddress_atIndex(gpu_va, index as usize) };
        Ok(())
    }

    /// `setAddress:attributeStride:atIndex:`: a vertex buffer with its stride.
    pub fn set_address_stride(&self, index: u32, gpu_va: u64, stride: u32) -> Result<()> {
        if index >= self.max_buffers {
            return Err(Error::BadArgument(format!(
                "argument table slot {index} of {}",
                self.max_buffers
            )));
        }
        // SAFETY: the index was checked against the table's size; the table
        // was made with `supportAttributeStrides`.
        unsafe {
            self.raw
                .setAddress_attributeStride_atIndex(gpu_va, stride as usize, index as usize)
        };
        Ok(())
    }

    pub(crate) fn raw(&self) -> &ProtocolObject<dyn MTL4ArgumentTable> {
        &self.raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_is_three_words() {
        assert_eq!(size_of::<IrDescriptorTableEntry>(), 24);
        assert_eq!(ENTRY_SIZE, 24);
    }

    #[test]
    fn buffer_encoding_follows_the_runtime_header() {
        // IRDescriptorTableGetBufferMetadata: size in the low 32 bits,
        // texture-view offset (8 bits) at bit 32, typed flag at bit 63.
        assert_eq!(buffer_metadata(0x1234, 0, false).unwrap(), 0x1234);
        assert_eq!(buffer_metadata(0x1234, 5, false).unwrap(), 0x5_0000_1234);
        assert_eq!(
            buffer_metadata(0x1234, 0, true).unwrap(),
            (1 << 63) | 0x1234
        );
        // The offset is masked to 8 bits, as kIRTexViewMask says.
        assert_eq!(buffer_metadata(1, 0x1ff, false).unwrap(), 0xff_0000_0001);
        // Lengths that do not fit 32 bits are refused rather than truncated.
        assert!(buffer_metadata(1 << 32, 0, false).is_err());
        assert!(buffer_metadata(u64::from(u32::MAX), 0, false).is_ok());
    }

    #[test]
    fn entry_constructors_follow_the_runtime_header() {
        let e = IrDescriptorTableEntry::buffer(0xdead_0000, 77);
        assert_eq!(
            (e.gpu_va, e.texture_view_id, e.metadata),
            (0xdead_0000, 0, 77)
        );
        let e = IrDescriptorTableEntry::raw_buffer(0x4000, 256).unwrap();
        assert_eq!((e.gpu_va, e.texture_view_id, e.metadata), (0x4000, 0, 256));
        assert_eq!(e.buffer_size(), 256);
        assert!(!e.is_typed_buffer());
        // Texture: gpuVA 0, view id, low 32 bits min-LOD clamp (float bits),
        // high 32 bits the metadata argument.
        let e = IrDescriptorTableEntry::texture(0xabcd, 1.5, 0);
        assert_eq!(e.gpu_va, 0);
        assert_eq!(e.texture_view_id, 0xabcd);
        assert_eq!(e.metadata, u64::from(1.5f32.to_bits()));
        let e = IrDescriptorTableEntry::texture(1, 0.0, 7);
        assert_eq!(e.metadata, 7 << 32);
        // Sampler: resource id in the first word, LOD bias bits in metadata.
        let e = IrDescriptorTableEntry::sampler(0x99, -0.25);
        assert_eq!((e.gpu_va, e.texture_view_id), (0x99, 0));
        assert_eq!(e.metadata, u64::from((-0.25f32).to_bits()));
    }

    #[test]
    fn bytes_round_trip_in_memory_order() {
        let e = IrDescriptorTableEntry {
            gpu_va: 1,
            texture_view_id: 2,
            metadata: 3,
        };
        let b = e.to_bytes();
        assert_eq!(&b[0..8], &1u64.to_le_bytes());
        assert_eq!(IrDescriptorTableEntry::from_bytes(&b), e);
    }

    #[test]
    fn bind_points_match_the_header() {
        assert_eq!(bind_point::DESCRIPTOR_HEAP, 0);
        assert_eq!(bind_point::SAMPLER_HEAP, 1);
        assert_eq!(bind_point::ARGUMENT_BUFFER, 2);
        assert_eq!(bind_point::ARGUMENT_BUFFER_HULL_DOMAIN, 3);
        assert_eq!(bind_point::DRAW_ARGUMENTS, 4);
        assert_eq!(bind_point::UNIFORMS, 5);
        assert_eq!(bind_point::VERTEX_BUFFER, 6);
        assert_eq!(bind_point::STAGE_IN_ATTRIBUTE_START, 11);
    }
}
