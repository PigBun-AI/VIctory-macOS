//! Synchronous calls: `Link::call(op, args, len)`.
//!
//! Each `SyncOp` has one `repr(C)` argument block. The caller fills the input
//! fields, calls, and reads the output fields from the same block. Blocks are
//! plain integers (addresses are `u64`, never pointers), start with a
//! [`CallHead`], and are a multiple of 8 bytes. `len` must equal the block's
//! size and the head's `version` must equal the version this proto defines for
//! the op; anything else is refused with `Status::INVALID_ARG` /
//! `Status::BAD_VERSION`.
//!
//! Ids and addresses in output fields are chosen by the back.

use crate::ids::{FenceId, HeapId, MemHeapId, PsoId, QueueId, ResId, RootSigId, SurfaceId};
use crate::pso::GraphicsPsoDesc;
use crate::status::Status;
use crate::types::{Caps, MemKind, ResKind, SamplerDesc, TexDim, ViewDesc};
use crate::wire::{Layout, Wire, open_enum, wire_struct};

open_enum! {
    /// Number of a sync call. 0x40 and up are reserved for M2/M3: 0x40-0x4F
    /// textures, heaps, formats, samplers, copies (m3-tex); 0x50-0x5F
    /// graphics pipelines and rendering (m3-gfx); the rest (queries, swap
    /// chains) later. 0x8000 and up are never assigned.
    pub struct SyncOp(u32) {
        NOP = 0,
        HELLO = 1,
        QUERY_CAPS = 2,
        SHUTDOWN = 3,
        CREATE_QUEUE = 4,
        DESTROY_QUEUE = 5,
        CREATE_BUFFER = 6,
        DESTROY_BUFFER = 7,
        WRAP_MEMORY = 8,
        CREATE_DESC_HEAP = 9,
        DESTROY_DESC_HEAP = 10,
        WRITE_DESCRIPTOR = 11,
        COPY_DESCRIPTORS = 12,
        CREATE_ROOT_SIGNATURE = 13,
        DESTROY_ROOT_SIGNATURE = 14,
        CREATE_COMPUTE_PIPELINE = 15,
        DESTROY_PIPELINE = 16,
        CREATE_FENCE = 17,
        DESTROY_FENCE = 18,
        FENCE_VALUE = 19,
        FENCE_WAIT = 20,
        FENCE_SIGNAL = 21,
        DEVICE_STATUS = 22,
        GET_SURFACE_LAYER = 23,

        CREATE_TEXTURE = 0x40,
        DESTROY_TEXTURE = 0x41,
        CREATE_HEAP = 0x42,
        DESTROY_HEAP = 0x43,
        CREATE_PLACED_BUFFER = 0x44,
        GET_RESOURCE_ALLOCATION_INFO = 0x45,

        CREATE_GRAPHICS_PIPELINE = 0x50,

        CREATE_SWAPCHAIN = 0x60,
        RESIZE_SWAPCHAIN = 0x61,
        DESTROY_SWAPCHAIN = 0x62,
        PRESENT = 0x63,
        SWAPCHAIN_INFO = 0x64,
        SET_SWAPCHAIN_LATENCY = 0x65,
    }
}

/// First number of the sync-op range reserved for later milestones.
pub const SYNC_RESERVED_START: u32 = 0x40;
pub const SYNC_RESERVED_END: u32 = 0x7FFF;

wire_struct! {
    /// First field of every argument block.
    pub struct CallHead {
        /// Version of this op's block (`SyncArgs::VERSION`).
        version: u16,
        flags: u16,
        _reserved: u32,
    }
}

/// An argument block of one `SyncOp`.
pub trait SyncArgs: Layout {
    const OP: SyncOp;
    const VERSION: u16;
    fn head_mut(&mut self) -> &mut CallHead;
    fn head(&self) -> &CallHead;
}

macro_rules! sync_block {
    (
        $(#[$m:meta])*
        $name:ident, $op:ident, $ver:literal, {
            $( $(#[$fm:meta])* $f:ident : $t:ty ),* $(,)?
        }
    ) => {
        wire_struct! {
            $(#[$m])*
            pub struct $name {
                head: CallHead,
                $( $(#[$fm])* $f: $t, )*
            }
        }
        const _: () = assert!(<$name as Wire>::SIZE.is_multiple_of(8), "sync block not a multiple of 8");
        impl SyncArgs for $name {
            const OP: SyncOp = SyncOp::$op;
            const VERSION: u16 = $ver;
            fn head_mut(&mut self) -> &mut CallHead {
                &mut self.head
            }
            fn head(&self) -> &CallHead {
                &self.head
            }
        }
        impl $name {
            /// A zeroed block with the head's version set.
            pub fn new() -> Self {
                let mut a = <Self as Wire>::ZERO;
                a.head.version = $ver;
                a
            }
        }
    };
}

sync_block! {
    /// `NOP`: does nothing; measures the call.
    NopArgs, NOP, 1, {}
}

sync_block! {
    /// `HELLO`: first call. Creates the device (or fails with `UNSUPPORTED`
    /// when it has no Metal 4 family) and negotiates the proto version.
    HelloArgs, HELLO, 1, {
        /// in: `PROTO_VERSION` of the caller.
        client_proto_version: u32,
        _pad: u32,
        /// in: capabilities the caller needs; the call fails with
        /// `UNSUPPORTED` if the device lacks one.
        required_caps: Caps,
        /// out: `PROTO_VERSION` of the back.
        proto_version: u32,
        _pad2: u32,
        /// out: capabilities of the device.
        caps: Caps,
        /// out: NUL-terminated device name.
        device_name: [u8; 96],
    }
}

sync_block! {
    /// `QUERY_CAPS`: limits and sizes the front needs to answer feature queries
    /// and to do descriptor-handle arithmetic.
    QueryCapsArgs, QUERY_CAPS, 1, {
        /// out
        caps: Caps,
        /// out: largest single buffer in bytes.
        max_buffer_len: u64,
        /// out: bytes the GPU can keep resident (recommended working set).
        working_set: u64,
        /// out: virtual-memory page size of the host.
        page_size: u32,
        /// out: bytes of one CBV/SRV/UAV heap slot (`HeapSlotSize`).
        descriptor_stride: u32,
        /// out: bytes of one sampler heap slot.
        sampler_stride: u32,
        /// out
        max_threads_per_group: u32,
        /// out: SIMD width (32 on Apple GPUs).
        simd_width: u32,
        /// out: most queues `CreateQueue` can make.
        max_queues: u32,
    }
}

sync_block! {
    /// `SHUTDOWN`: destroys everything; the link is unusable until `HELLO` again.
    ShutdownArgs, SHUTDOWN, 1, {}
}

sync_block! {
    CreateQueueArgs, CREATE_QUEUE, 1, {
        /// in: a `QueueKind`.
        kind: u32,
        /// in: 0 normal.
        priority: u32,
        /// out
        queue: QueueId,
        _pad: u32,
    }
}

sync_block! {
    DestroyQueueArgs, DESTROY_QUEUE, 1, {
        /// in: waits for the queue to drain first.
        queue: QueueId,
        _pad: u32,
    }
}

sync_block! {
    /// `CREATE_BUFFER`: a buffer of `size` bytes.
    CreateBufferArgs, CREATE_BUFFER, 1, {
        /// in
        size: u64,
        /// in: a `BufferKind` (not `WRAPPED`).
        kind: u32,
        /// in: reserved, 0.
        flags: u32,
        /// out
        res: ResId,
        _pad: u32,
        /// out: GPU address; what `GetGPUVirtualAddress`, root descriptors and CBVs use.
        gpu_va: u64,
        /// out: CPU address of the contents, 0 for `DEFAULT` buffers.
        cpu_addr: u64,
        /// out: bytes actually allocated (>= `size`).
        alloc_size: u64,
    }
}

sync_block! {
    DestroyBufferArgs, DESTROY_BUFFER, 1, {
        /// in: released once the GPU is done with it (the back waits).
        res: ResId,
        _pad: u32,
    }
}

sync_block! {
    /// `WRAP_MEMORY`: use `[addr, addr+len)` of the caller's address space as a
    /// buffer, in place. The back refuses (`CONFLICT`) a page range that
    /// already reaches the GPU through another buffer (rule 6).
    WrapMemoryArgs, WRAP_MEMORY, 1, {
        /// in: address in the shared address space; page aligned.
        addr: u64,
        /// in: bytes; a multiple of the page size.
        len: u64,
        /// in: reserved, 0.
        flags: u32,
        /// out
        res: ResId,
        /// out: GPU address.
        gpu_va: u64,
    }
}

sync_block! {
    CreateDescHeapArgs, CREATE_DESC_HEAP, 1, {
        /// in: a `HeapKind`.
        kind: u32,
        /// in: slots.
        capacity: u32,
        /// in: bit 0 = shader visible.
        flags: u32,
        /// out
        heap: HeapId,
        /// out: bytes per slot.
        stride: u32,
        _pad: u32,
        /// out: GPU address of slot 0 (`GetGPUDescriptorHandleForHeapStart`).
        gpu_va: u64,
        /// out: CPU address of slot 0, 0 if not host-writable.
        cpu_addr: u64,
    }
}

sync_block! {
    DestroyDescHeapArgs, DESTROY_DESC_HEAP, 1, {
        heap: HeapId,
        _pad: u32,
    }
}

sync_block! {
    /// `WRITE_DESCRIPTOR`: writes `heap[index]` from `view` (or `sampler` when
    /// `view.kind` is `SAMPLER`). Version 2: `ViewDesc` has the texture fields.
    WriteDescriptorArgs, WRITE_DESCRIPTOR, 2, {
        heap: HeapId,
        index: u32,
        view: ViewDesc,
        sampler: SamplerDesc,
    }
}

sync_block! {
    /// `COPY_DESCRIPTORS`: `count` slots from one heap to another; the two
    /// heaps must be of the same kind. Overlap inside one heap behaves like `memmove`.
    CopyDescriptorsArgs, COPY_DESCRIPTORS, 1, {
        dst_heap: HeapId,
        dst_index: u32,
        src_heap: HeapId,
        src_index: u32,
        count: u32,
        _pad: u32,
    }
}

sync_block! {
    /// `CREATE_ROOT_SIGNATURE`: `data` is `RootSigDesc::encode()`.
    CreateRootSignatureArgs, CREATE_ROOT_SIGNATURE, 1, {
        /// in: address of the bytes, valid for the call only.
        data_addr: u64,
        /// in
        data_len: u64,
        /// out
        root_sig: RootSigId,
        _pad: u32,
    }
}

sync_block! {
    DestroyRootSignatureArgs, DESTROY_ROOT_SIGNATURE, 1, {
        root_sig: RootSigId,
        _pad: u32,
    }
}

sync_block! {
    /// `CREATE_COMPUTE_PIPELINE`: a compute pipeline from DXIL. The conversion
    /// to Metal code (MSC) is the back's business and is not wired up yet in M1:
    /// the back may answer `UNSUPPORTED` until it is.
    CreateComputePipelineArgs, CREATE_COMPUTE_PIPELINE, 1, {
        /// in
        root_sig: RootSigId,
        /// in: reserved, 0.
        flags: u32,
        /// in: address of the DXIL container, valid for the call only.
        dxil_addr: u64,
        /// in
        dxil_len: u64,
        /// in: reserved for converter options, 0.
        reserved: [u32; 4],
        /// out
        pso: PsoId,
        _pad: u32,
    }
}

sync_block! {
    /// `CREATE_GRAPHICS_PIPELINE`: a vertex (`vs` + `ps`) or mesh (`amp` +
    /// `ms` + `ps`) pipeline from DXIL, for a root signature, with all the
    /// pipeline state of `desc` ([`GraphicsPsoDesc`]). The DXIL addresses in
    /// `desc` are valid for the call only. The conversion and the Metal
    /// compile take milliseconds to a second; the call returns when the
    /// pipeline exists. `DESTROY_PIPELINE` frees it. Answers `UNSUPPORTED`
    /// for what the back cannot make (a shader the converter refuses, state
    /// Metal lacks) and says why in the back's last-error text.
    CreateGraphicsPipelineArgs, CREATE_GRAPHICS_PIPELINE, 1, {
        /// in
        root_sig: RootSigId,
        /// in: reserved, 0.
        flags: u32,
        /// out
        pso: PsoId,
        _pad: u32,
        /// in
        desc: GraphicsPsoDesc,
    }
}

sync_block! {
    DestroyPipelineArgs, DESTROY_PIPELINE, 1, {
        pso: PsoId,
        _pad: u32,
    }
}

sync_block! {
    CreateFenceArgs, CREATE_FENCE, 1, {
        /// in
        initial: u64,
        /// in: reserved, 0.
        flags: u32,
        /// out
        fence: FenceId,
    }
}

sync_block! {
    DestroyFenceArgs, DESTROY_FENCE, 1, {
        fence: FenceId,
        _pad: u32,
    }
}

sync_block! {
    /// `FENCE_VALUE`: the highest value completed so far.
    FenceValueArgs, FENCE_VALUE, 1, {
        /// in
        fence: FenceId,
        _pad: u32,
        /// out
        value: u64,
    }
}

sync_block! {
    /// `FENCE_WAIT`: blocks until the fence reaches `value`;
    /// `Status::TIMEOUT` when `timeout_ms` passes first.
    FenceWaitArgs, FENCE_WAIT, 1, {
        fence: FenceId,
        _pad: u32,
        value: u64,
        /// `u64::MAX` waits forever.
        timeout_ms: u64,
    }
}

sync_block! {
    /// `FENCE_SIGNAL`: the CPU signals `value` (never lowers the value).
    FenceSignalArgs, FENCE_SIGNAL, 1, {
        fence: FenceId,
        _pad: u32,
        value: u64,
    }
}

sync_block! {
    /// `DEVICE_STATUS`: has the device been lost?
    DeviceStatusArgs, DEVICE_STATUS, 1, {
        /// out: 0 fine, 1 hung (timeout), 2 removed.
        state: u32,
        _pad: u32,
    }
}

sync_block! {
    /// `GET_SURFACE_LAYER`: the drawing layer for a window, asked of the
    /// `SurfaceProvider` the bridge installed (M3).
    GetSurfaceLayerArgs, GET_SURFACE_LAYER, 1, {
        /// in: `WindowRef` value.
        window: u64,
        /// out
        surface: SurfaceId,
        _pad: u32,
        /// out: address of the layer object, valid while the surface lives.
        layer_addr: u64,
    }
}

sync_block! {
    /// `CREATE_TEXTURE`: a texture, committed (own memory) or placed in a
    /// memory heap at `heap_offset`. `format` is a DXGI format number; a
    /// typeless one can be viewed through any format of its family. The
    /// back refuses a description Metal cannot make (`UNSUPPORTED`, the text
    /// in `Executor::last_error`).
    CreateTextureArgs, CREATE_TEXTURE, 1, {
        /// in
        dimension: TexDim,
        /// in
        format: u32,
        /// in: texels.
        width: u32,
        /// in: texels; 1 for 1D.
        height: u32,
        /// in: depth of a 3D texture, array size of the others (1 = not an array).
        depth_or_array: u32,
        /// in: mip levels, at least 1.
        mips: u32,
        /// in: samples per texel (1, 2, 4, 8).
        samples: u32,
        /// in: `tex_usage` bits.
        usage: u32,
        /// in: reserved, 0.
        flags: u32,
        /// in: the heap to place it in; null = committed.
        heap: MemHeapId,
        /// in: byte offset in `heap`, a multiple of the alignment `GET_RESOURCE_ALLOCATION_INFO` gave.
        heap_offset: u64,
        /// out
        res: ResId,
        _pad: u32,
        /// out: bytes the texture takes (the allocated size).
        alloc_size: u64,
    }
}

sync_block! {
    /// `DESTROY_TEXTURE`: released once the GPU is done with it (the back waits).
    DestroyTextureArgs, DESTROY_TEXTURE, 1, {
        res: ResId,
        _pad: u32,
    }
}

sync_block! {
    /// `CREATE_HEAP`: memory buffers and textures can be placed in
    /// (`CREATE_PLACED_BUFFER`, `CREATE_TEXTURE` with a heap). The heap is
    /// resident for as long as it lives; resources placed in it share its
    /// memory, ordered against each other by `BARRIER{ALIASING}`.
    CreateHeapArgs, CREATE_HEAP, 1, {
        /// in: a `MemKind`.
        kind: MemKind,
        /// in: reserved, 0.
        flags: u32,
        /// in: bytes (the back rounds up to what Metal allocates).
        size: u64,
        /// out
        heap: MemHeapId,
        _pad: u32,
        /// out: bytes the heap really has.
        alloc_size: u64,
    }
}

sync_block! {
    /// `DESTROY_HEAP`: the memory goes when the GPU is done with it and every
    /// resource placed in it is destroyed.
    DestroyHeapArgs, DESTROY_HEAP, 1, {
        heap: MemHeapId,
        _pad: u32,
    }
}

sync_block! {
    /// `CREATE_PLACED_BUFFER`: a buffer of `size` bytes at `offset` of a heap.
    /// `gpu_va` and `cpu_addr` are as for `CREATE_BUFFER` (a CPU address only
    /// for upload and readback heaps).
    CreatePlacedBufferArgs, CREATE_PLACED_BUFFER, 1, {
        /// in
        heap: MemHeapId,
        _pad: u32,
        /// in: a multiple of the alignment `GET_RESOURCE_ALLOCATION_INFO` gave.
        offset: u64,
        /// in
        size: u64,
        /// out
        res: ResId,
        _pad2: u32,
        /// out
        gpu_va: u64,
        /// out
        cpu_addr: u64,
    }
}

sync_block! {
    /// `GET_RESOURCE_ALLOCATION_INFO`: what a resource of this description
    /// needs from a heap of `heap_kind`. For a buffer only `buffer_size`
    /// counts; for a texture the fields of `CREATE_TEXTURE`.
    GetResourceAllocationInfoArgs, GET_RESOURCE_ALLOCATION_INFO, 1, {
        /// in
        kind: ResKind,
        /// in: a `TexDim` (texture).
        dimension: TexDim,
        /// in: DXGI format (texture).
        format: u32,
        /// in
        width: u32,
        /// in
        height: u32,
        /// in
        depth_or_array: u32,
        /// in
        mips: u32,
        /// in
        samples: u32,
        /// in: `tex_usage` bits (texture).
        usage: u32,
        /// in: a `MemKind`.
        heap_kind: MemKind,
        /// in: bytes (buffer).
        buffer_size: u64,
        /// out: bytes in a heap (Metal's `heapTextureSizeAndAlign` / `heapBufferSizeAndAlign`).
        size: u64,
        /// out: placement alignment in bytes.
        alignment: u64,
    }
}

/// Most back buffers one swap chain has (the original API's limit).
pub const MAX_SWAPCHAIN_BUFFERS: usize = 16;

/// Bits of `CreateSwapchainArgs::flags`.
pub mod swapchain_flags {
    /// After every present the back reads one texel of what it wrote to the
    /// surface and waits for the GPU to finish (tests and tools: it makes
    /// `SWAPCHAIN_INFO` report that texel; it costs the frame's overlap).
    pub const PROBE: u32 = 1 << 0;
    pub const ALL_BITS: u32 = PROBE;
}

/// Bits of `PresentArgs::flags`.
pub mod present_flags {
    /// The application allows tearing (no wait for the display).
    pub const ALLOW_TEARING: u32 = 1 << 0;
    pub const ALL_BITS: u32 = ALLOW_TEARING;
}

sync_block! {
    /// `CREATE_SWAPCHAIN`: binds a window's drawing surface (asked of the
    /// `SurfaceProvider` the bridge installed) to `buffer_count` back
    /// buffers the back makes as ordinary 2D textures (`res` numbers in
    /// `buffers`, usable like any texture; destroy them with `DESTROY_TEXTURE`
    /// like any other, the swap chain does not keep them). `format` is a DXGI
    /// number. `max_latency` is how many presents may be queued before
    /// `PRESENT` waits (0 = 3). `UNSUPPORTED` when there is no provider, no
    /// surface for the window or the format cannot be presented.
    CreateSwapchainArgs, CREATE_SWAPCHAIN, 1, {
        /// in: `WindowRef` value (the window handle).
        window: u64,
        /// in: texels.
        width: u32,
        /// in: texels.
        height: u32,
        /// in: DXGI format of the back buffers.
        format: u32,
        /// in: 1 to 16.
        buffer_count: u32,
        /// in: `tex_usage` bits the back buffers get besides what presenting needs.
        usage: u32,
        /// in: `swapchain_flags` bits.
        flags: u32,
        /// in: frames in flight; 0 = the default (3).
        max_latency: u32,
        /// out
        swapchain: SurfaceId,
        /// out: the surface's own pixel format number (informational).
        surface_format: u32,
        _pad: u32,
        /// out: `ResId` numbers of the back buffers, the first `buffer_count`.
        buffers: [u32; 16],
    }
}

sync_block! {
    /// `RESIZE_SWAPCHAIN`: new back buffers of another size (and format and
    /// count if given) and the surface set to the new size. The caller has
    /// destroyed the old back buffers (or keeps them and gives them up: the
    /// swap chain no longer uses them). Presents already queued still show
    /// what they were given.
    ResizeSwapchainArgs, RESIZE_SWAPCHAIN, 1, {
        /// in
        swapchain: SurfaceId,
        /// in: texels.
        width: u32,
        /// in: texels.
        height: u32,
        /// in: DXGI format; 0 keeps the current one.
        format: u32,
        /// in: 1 to 16; 0 keeps the current count.
        buffer_count: u32,
        /// in: `tex_usage` bits; 0 keeps the current ones.
        usage: u32,
        /// out: the new `ResId` numbers.
        buffers: [u32; 16],
    }
}

sync_block! {
    /// `DESTROY_SWAPCHAIN`: gives the window's surface back once the queued
    /// presents are done. The back buffers are the caller's to destroy.
    DestroySwapchainArgs, DESTROY_SWAPCHAIN, 1, {
        swapchain: SurfaceId,
        _pad: u32,
    }
}

sync_block! {
    /// `PRESENT`: shows back buffer `buffer` (a `ResId` of this swap chain)
    /// after everything submitted to `queue` so far. The back waits for the
    /// surface's next drawable, copies the buffer into it (converting the
    /// format and scaling if the sizes differ), signals and presents it, and
    /// blocks only while more than `max_latency` presents are in flight.
    /// `sync_interval` 0 does not wait for the display.
    PresentArgs, PRESENT, 1, {
        /// in
        queue: QueueId,
        /// in
        swapchain: SurfaceId,
        /// in: the back buffer to show.
        buffer: ResId,
        /// in: 0 (now) or the number of display refreshes to wait.
        sync_interval: u32,
        /// in: `present_flags` bits.
        flags: u32,
        _pad: u32,
    }
}

sync_block! {
    /// `SWAPCHAIN_INFO`: what the back knows about a swap chain.
    SwapchainInfoArgs, SWAPCHAIN_INFO, 1, {
        /// in
        swapchain: SurfaceId,
        _pad: u32,
        /// out: size of the surface's drawables in texels.
        width: u32,
        /// out
        height: u32,
        /// out: the surface's own pixel format number.
        surface_format: u32,
        /// out: 1 when `probe_texel` is valid (swap chain made with `PROBE`, one present done).
        probe_valid: u32,
        /// out: presents handed to the GPU.
        presents: u64,
        /// out: presents the GPU has finished.
        completed: u64,
        /// out: the texel at (`probe_x`, `probe_y`) of the last drawable, as stored (first 8 bytes of the pixel).
        probe_texel: u64,
        /// out
        probe_x: u32,
        /// out
        probe_y: u32,
    }
}

sync_block! {
    /// `SET_SWAPCHAIN_LATENCY`: frames in flight (1 to 8).
    SetSwapchainLatencyArgs, SET_SWAPCHAIN_LATENCY, 1, {
        swapchain: SurfaceId,
        max_latency: u32,
    }
}

/// Reads an argument block from the raw pointer/length of `Link::call`.
///
/// Checks the pointer (non-null, aligned), the length (exactly the block size)
/// and the head's version; returns the `Status` to answer otherwise.
///
/// # Safety
/// `ptr` must be null or valid for reads and writes of `len` bytes for the
/// lifetime `'a`, with no other reference to those bytes alive. Every bit
/// pattern is a valid `A` (blocks hold only integers, floats and arrays of them).
#[allow(unsafe_code)]
pub unsafe fn args_from_raw<'a, A: SyncArgs>(
    ptr: *mut u8,
    len: usize,
) -> Result<&'a mut A, Status> {
    if ptr.is_null() || len != A::SIZE || !(ptr as usize).is_multiple_of(core::mem::align_of::<A>())
    {
        return Err(Status::INVALID_ARG);
    }
    // SAFETY: checked above; the caller guarantees validity of the bytes.
    let a = unsafe { &mut *(ptr as *mut A) };
    if a.head().version != A::VERSION {
        return Err(Status::BAD_VERSION);
    }
    Ok(a)
}
