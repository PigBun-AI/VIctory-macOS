//! Our own POD types: enums, descriptions, masks. They look like what D3D12
//! needs and are neither D3D12's nor Metal's.
//!
//! Enums are "open": a transparent integer with named constants, so an unknown
//! number can be carried, checked (`is_known`) and refused without UB.

use crate::ids::ResId;
use crate::wire::{Wire, open_enum, wire_struct};
use core::ops::{BitAnd, BitOr};

open_enum! {
    /// What a buffer is for; decides where its memory lives.
    pub struct BufferKind(u32) {
        /// GPU-only (private) storage.
        DEFAULT = 0,
        /// CPU-writable, GPU-readable (write-combined is fine).
        UPLOAD = 1,
        /// GPU-writable, CPU-readable.
        READBACK = 2,
        /// CPU and GPU both read and write (shared storage, unified memory).
        SHARED = 3,
        /// Caller-owned memory used in place (`WrapMemory`). Never a valid
        /// `CreateBuffer` kind; reported by queries.
        WRAPPED = 4,
    }
}

open_enum! {
    /// What a descriptor heap holds.
    pub struct HeapKind(u32) {
        CBV_SRV_UAV = 0,
        SAMPLER = 1,
        /// Render-target views. Reserved for M3.
        RTV = 2,
        /// Depth-stencil views. Reserved for M3.
        DSV = 3,
    }
}

open_enum! {
    pub struct QueueKind(u32) {
        DIRECT = 0,
        COMPUTE = 1,
        COPY = 2,
    }
}

open_enum! {
    /// What a `ViewDesc` describes.
    pub struct ViewKind(u8) {
        /// A null descriptor: reads as zero, writes are dropped.
        NONE = 0,
        BUFFER_SRV = 1,
        BUFFER_UAV = 2,
        CBV = 3,
        /// The sampler is in `WriteDescriptorArgs::sampler`, not in the view.
        SAMPLER = 4,
        /// A texture shader resource view (`resource` names a texture; the
        /// texture fields of [`ViewDesc`] say which part and how).
        TEXTURE_SRV = 5,
        /// A texture unordered access view.
        TEXTURE_UAV = 6,
    }
}

open_enum! {
    /// Dimension of a texture resource.
    pub struct TexDim(u32) {
        TEX1D = 1,
        TEX2D = 2,
        TEX3D = 3,
    }
}

open_enum! {
    /// How a texture view sees its texture (the shader's texture type).
    pub struct TexViewDim(u8) {
        /// Not a texture view.
        NONE = 0,
        TEX1D = 1,
        TEX1D_ARRAY = 2,
        TEX2D = 3,
        TEX2D_ARRAY = 4,
        TEX2D_MS = 5,
        TEX2D_MS_ARRAY = 6,
        TEX3D = 7,
        /// Six slices of a 2D array texture, from `slice_first`.
        CUBE = 8,
        /// `slice_count` / 6 cubes, from `slice_first`.
        CUBE_ARRAY = 9,
    }
}

open_enum! {
    /// What a memory heap is for; decides where its memory lives.
    pub struct MemKind(u32) {
        /// GPU-only (private storage): buffers and textures.
        DEFAULT = 0,
        /// CPU-writable, GPU-readable (shared, write-combined): buffers.
        UPLOAD = 1,
        /// GPU-writable, CPU-readable (shared): buffers.
        READBACK = 2,
        /// CPU and GPU both read and write (shared, cached): buffers.
        SHARED = 3,
    }
}

open_enum! {
    /// What `GET_RESOURCE_ALLOCATION_INFO` is asked about.
    pub struct ResKind(u32) {
        BUFFER = 0,
        TEXTURE = 1,
    }
}

open_enum! {
    /// What a [`TextureLocation`] names.
    pub struct LocKind(u8) {
        /// One subresource of a texture: `res`, `plane`, `mip`, `slice`.
        SUBRESOURCE = 0,
        /// Bytes of a buffer laid out as a texture region: `res` (a buffer),
        /// `offset`, `format`, `row_pitch`, `width`, `height`, `depth`.
        FOOTPRINT = 1,
    }
}

/// `CREATE_TEXTURE` usage bits (what the texture will be used for; the back
/// turns them into Metal usage flags).
pub mod tex_usage {
    /// Sampled or read by shaders.
    pub const SRV: u32 = 1 << 0;
    /// Written (and read) by shaders as a UAV.
    pub const UAV: u32 = 1 << 1;
    /// A colour render target.
    pub const RENDER_TARGET: u32 = 1 << 2;
    /// A depth-stencil target.
    pub const DEPTH_STENCIL: u32 = 1 << 3;
    pub const ALL_BITS: u32 = 0xF;
}

/// Component sources of a texture view swizzle (`ViewDesc::swizzle`).
pub mod swizzle {
    pub const R: u8 = 0;
    pub const G: u8 = 1;
    pub const B: u8 = 2;
    pub const A: u8 = 3;
    pub const ZERO: u8 = 4;
    pub const ONE: u8 = 5;
    /// R, G, B, A: the view is not swizzled.
    pub const IDENTITY: [u8; 4] = [R, G, B, A];
}

open_enum! {
    /// How a buffer SRV/UAV addresses its elements.
    pub struct BufferForm(u8) {
        /// Byte-address buffer; `first_element`/`num_elements` count 32-bit words.
        RAW = 0,
        /// Elements of `format`.
        TYPED = 1,
        /// Elements of `structure_stride` bytes.
        STRUCTURED = 2,
    }
}

open_enum! {
    pub struct Filter(u8) {
        NEAREST = 0,
        LINEAR = 1,
    }
}

open_enum! {
    pub struct Reduction(u8) {
        STANDARD = 0,
        COMPARISON = 1,
        MIN = 2,
        MAX = 3,
    }
}

open_enum! {
    pub struct AddressMode(u8) {
        WRAP = 0,
        MIRROR = 1,
        CLAMP = 2,
        BORDER = 3,
        MIRROR_ONCE = 4,
    }
}

open_enum! {
    pub struct CompareFunc(u8) {
        NEVER = 0,
        LESS = 1,
        EQUAL = 2,
        LESS_EQUAL = 3,
        GREATER = 4,
        NOT_EQUAL = 5,
        GREATER_EQUAL = 6,
        ALWAYS = 7,
    }
}

open_enum! {
    /// Which pipeline stage family a root binding or a root signature belongs to.
    pub struct BindPoint(u32) {
        COMPUTE = 0,
        /// Reserved for M3.
        GRAPHICS = 1,
    }
}

open_enum! {
    /// Kind of one root parameter (and of the root descriptor `SetRootDescriptor` sets).
    pub struct RootParamKind(u32) {
        /// 32-bit constants stored in the root.
        CONSTANTS = 0,
        CBV = 1,
        SRV = 2,
        UAV = 3,
        /// A descriptor table: a list of ranges.
        TABLE = 4,
    }
}

open_enum! {
    pub struct RangeKind(u32) {
        SRV = 0,
        UAV = 1,
        CBV = 2,
        SAMPLER = 3,
    }
}

open_enum! {
    /// Which shader stages see a root parameter or static sampler.
    pub struct ShaderVisibility(u32) {
        ALL = 0,
        VERTEX = 1,
        FRAGMENT = 2,
        OBJECT = 3,
        MESH = 4,
    }
}

open_enum! {
    /// What a barrier is.
    pub struct BarrierClass(u8) {
        /// Make writes of `before` stages visible to `after` stages: covers
        /// UAV barriers and state transitions (the front derives the stages).
        MEMORY = 0,
        /// Two resources share memory; the first may not be used after the second starts.
        ALIASING = 1,
        /// Layout transition. Reserved: Metal buffers need none, textures may (M3).
        LAYOUT = 2,
    }
}

/// Our own barrier stage mask (u16). The back maps each bit to the matching
/// Metal stage; D3D12 states never reach proto.
pub mod stage {
    pub const VERTEX: u16 = 1 << 0;
    pub const FRAGMENT: u16 = 1 << 1;
    pub const TILE: u16 = 1 << 2;
    pub const OBJECT: u16 = 1 << 3;
    pub const MESH: u16 = 1 << 4;
    pub const DISPATCH: u16 = 1 << 5;
    pub const BLIT: u16 = 1 << 6;
    pub const RESOURCE_STATE: u16 = 1 << 7;
    pub const ALL: u16 = 0x00FF;
}

/// Barrier visibility bits (u8).
pub mod visibility {
    pub const DEVICE: u8 = 1 << 0;
    pub const RESOURCE_ALIAS: u8 = 1 << 1;
    pub const ALL_BITS: u8 = 0b11;
}

/// Format ids. The number is the DXGI format number, passed through untouched;
/// only the back knows what a number means (and which it supports). These are
/// the ones the M1 front names; any other number travels as it is.
pub mod format {
    pub const UNKNOWN: u32 = 0;
    pub const R32G32B32A32_TYPELESS: u32 = 1;
    pub const R32G32B32A32_FLOAT: u32 = 2;
    pub const R32G32B32A32_UINT: u32 = 3;
    pub const R32G32B32A32_SINT: u32 = 4;
    pub const R32G32B32_FLOAT: u32 = 6;
    pub const R32G32B32_UINT: u32 = 7;
    pub const R16G16B16A16_FLOAT: u32 = 10;
    pub const R16G16B16A16_UNORM: u32 = 11;
    pub const R16G16B16A16_UINT: u32 = 12;
    pub const R32G32_TYPELESS: u32 = 15;
    pub const R32G32_FLOAT: u32 = 16;
    pub const R32G32_UINT: u32 = 17;
    pub const R32G32_SINT: u32 = 18;
    pub const R10G10B10A2_UNORM: u32 = 24;
    pub const R11G11B10_FLOAT: u32 = 26;
    pub const R8G8B8A8_TYPELESS: u32 = 27;
    pub const R8G8B8A8_UNORM: u32 = 28;
    pub const R8G8B8A8_UNORM_SRGB: u32 = 29;
    pub const R8G8B8A8_UINT: u32 = 30;
    pub const R16G16_FLOAT: u32 = 34;
    pub const R16G16_UINT: u32 = 36;
    pub const R32_TYPELESS: u32 = 39;
    pub const D32_FLOAT: u32 = 40;
    pub const R32_FLOAT: u32 = 41;
    pub const R32_UINT: u32 = 42;
    pub const R32_SINT: u32 = 43;
    pub const R8G8_UNORM: u32 = 49;
    pub const R8G8_UINT: u32 = 50;
    pub const R16_TYPELESS: u32 = 53;
    pub const R16_FLOAT: u32 = 54;
    pub const R16_UNORM: u32 = 56;
    pub const R16_UINT: u32 = 57;
    pub const R16_SINT: u32 = 59;
    pub const R8_TYPELESS: u32 = 60;
    pub const R8_UNORM: u32 = 61;
    pub const R8_UINT: u32 = 62;
    pub const R8_SINT: u32 = 64;
    pub const B8G8R8A8_UNORM: u32 = 87;
    pub const R32G8X24_TYPELESS: u32 = 19;
    pub const D32_FLOAT_S8X24_UINT: u32 = 20;
    pub const R32_FLOAT_X8X24_TYPELESS: u32 = 21;
    pub const X32_TYPELESS_G8X24_UINT: u32 = 22;
    pub const R24G8_TYPELESS: u32 = 44;
    pub const D24_UNORM_S8_UINT: u32 = 45;
    pub const R24_UNORM_X8_TYPELESS: u32 = 46;
    pub const X24_TYPELESS_G8_UINT: u32 = 47;
    pub const D16_UNORM: u32 = 55;
    pub const BC1_UNORM: u32 = 71;
    pub const BC3_UNORM: u32 = 78;
    pub const BC7_UNORM: u32 = 98;
}

/// Capability bits of a back (`QueryCaps`), also written into stream and file
/// headers to say what the stream needs.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct Caps(pub u64);

impl Caps {
    pub const NONE: Caps = Caps(0);
    /// The device supports the Metal 4 family (the back refuses to start without it).
    pub const METAL4: Caps = Caps(1 << 0);
    pub const UNIFIED_MEMORY: Caps = Caps(1 << 1);
    pub const MESH_SHADERS: Caps = Caps(1 << 2);
    /// 64-bit atomics on heap memory.
    pub const ATOMICS64_HEAP: Caps = Caps(1 << 3);
    pub const ARGUMENT_BUFFERS_TIER2: Caps = Caps(1 << 4);
    /// The back runs translated (x86_64 under Rosetta).
    pub const TRANSLATED: Caps = Caps(1 << 5);
    /// `WrapMemory` (no-copy buffers over caller memory) works.
    pub const WRAP_MEMORY: Caps = Caps(1 << 6);
    /// Every bit this version of proto defines.
    pub const KNOWN: Caps = Caps(0x7F);

    pub const fn contains(self, other: Caps) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Caps {
    type Output = Caps;
    fn bitor(self, rhs: Caps) -> Caps {
        Caps(self.0 | rhs.0)
    }
}

impl BitAnd for Caps {
    type Output = Caps;
    fn bitand(self, rhs: Caps) -> Caps {
        Caps(self.0 & rhs.0)
    }
}

impl crate::wire::Wire for Caps {
    const SIZE: usize = 8;
    const ZERO: Self = Caps(0);
    fn put(&self, out: &mut alloc::vec::Vec<u8>) {
        self.0.put(out);
    }
    fn get(r: &mut crate::wire::Reader<'_>) -> Result<Self, crate::status::Error> {
        Ok(Caps(u64::get(r)?))
    }
}

wire_struct! {
    /// A sampler. Plain numbers; the back builds whatever its API wants.
    pub struct SamplerDesc {
        min_filter: Filter,
        mag_filter: Filter,
        mip_filter: Filter,
        reduction: Reduction,
        address_u: AddressMode,
        address_v: AddressMode,
        address_w: AddressMode,
        compare: CompareFunc,
        mip_lod_bias: f32,
        min_lod: f32,
        max_lod: f32,
        /// 0 or 1 = no anisotropic filtering.
        max_anisotropy: u32,
        border_color: [f32; 4],
    }
}

wire_struct! {
    /// One descriptor to write into a heap slot (`WriteDescriptor`).
    ///
    /// Buffer SRV/UAV: `resource`, `form`, `first_element`, `num_elements`,
    /// plus `format` (typed) or `structure_stride` (structured). CBV:
    /// `gpu_va` and `size_bytes` (a CBV is a GPU address, not a resource).
    /// Texture SRV/UAV: `resource` (a texture), `format` (the view's format,
    /// `UNKNOWN` = the texture's own), then the fields from `dimension` on.
    /// Fields a kind does not use are zero.
    pub struct ViewDesc {
        kind: ViewKind,
        form: BufferForm,
        /// Bit 0: the UAV has a counter (`counter_resource`, `counter_offset`).
        flags: u16,
        /// Format id (see [`format`]); `format::UNKNOWN` for raw/structured.
        format: u32,
        resource: ResId,
        structure_stride: u32,
        first_element: u64,
        num_elements: u32,
        size_bytes: u32,
        gpu_va: u64,
        counter_resource: ResId,
        _pad: u32,
        counter_offset: u64,
        /// Texture views: how the shader sees the texture ([`TexViewDim`]).
        dimension: TexViewDim,
        /// Texture views: which plane of a depth-stencil texture (0 depth or
        /// colour, 1 stencil).
        plane: u8,
        /// Texture views: component sources (see [`swizzle`]), `[R, G, B, A]`
        /// for no swizzle. A zero field is `[R, R, R, R]`, not "unset".
        swizzle: [u8; 4],
        _pad2: u16,
        /// Texture views: first mip level.
        mip_first: u32,
        /// Texture views: number of mip levels; 0 = all from `mip_first`.
        mip_count: u32,
        /// Texture views: first array slice (cube: first face; 3D UAV: first W slice).
        slice_first: u32,
        /// Texture views: number of array slices (cube: faces); 0 = all from
        /// `slice_first`.
        slice_count: u32,
        /// Texture SRV: lowest LOD a sample may reach (`ResourceMinLODClamp`).
        min_lod: f32,
        _pad3: u32,
    }
}

impl ViewDesc {
    pub const HAS_COUNTER: u16 = 1;

    pub const fn null() -> Self {
        <Self as Wire>::ZERO
    }

    pub const fn cbv(gpu_va: u64, size_bytes: u32) -> Self {
        let mut v = <Self as Wire>::ZERO;
        v.kind = ViewKind::CBV;
        v.gpu_va = gpu_va;
        v.size_bytes = size_bytes;
        v
    }

    /// A raw (byte-address) buffer view; offsets and counts in 32-bit words.
    pub const fn raw(kind: ViewKind, resource: ResId, first_word: u64, num_words: u32) -> Self {
        let mut v = <Self as Wire>::ZERO;
        v.kind = kind;
        v.form = BufferForm::RAW;
        v.resource = resource;
        v.first_element = first_word;
        v.num_elements = num_words;
        v
    }

    pub const fn typed(
        kind: ViewKind,
        resource: ResId,
        format: u32,
        first_element: u64,
        num_elements: u32,
    ) -> Self {
        let mut v = <Self as Wire>::ZERO;
        v.kind = kind;
        v.form = BufferForm::TYPED;
        v.format = format;
        v.resource = resource;
        v.first_element = first_element;
        v.num_elements = num_elements;
        v
    }

    pub const fn structured(
        kind: ViewKind,
        resource: ResId,
        stride: u32,
        first_element: u64,
        num_elements: u32,
    ) -> Self {
        let mut v = <Self as Wire>::ZERO;
        v.kind = kind;
        v.form = BufferForm::STRUCTURED;
        v.structure_stride = stride;
        v.resource = resource;
        v.first_element = first_element;
        v.num_elements = num_elements;
        v
    }

    /// A texture SRV or UAV (`kind` is `TEXTURE_SRV` or `TEXTURE_UAV`) over
    /// all mips and slices, not swizzled. Narrow it by setting the range fields.
    pub const fn texture(
        kind: ViewKind,
        resource: ResId,
        format: u32,
        dimension: TexViewDim,
    ) -> Self {
        let mut v = <Self as Wire>::ZERO;
        v.kind = kind;
        v.resource = resource;
        v.format = format;
        v.dimension = dimension;
        v.swizzle = swizzle::IDENTITY;
        v
    }

    /// `true` when every enum field holds a number this version defines.
    pub const fn is_valid(&self) -> bool {
        self.kind.is_known()
            && self.form.is_known()
            && self.dimension.is_known()
            && self.flags & !Self::HAS_COUNTER == 0
    }
}
