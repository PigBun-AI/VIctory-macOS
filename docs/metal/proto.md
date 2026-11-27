# victory-metal-proto: the contract between front and back (seam B)

Crate `metal/proto` (`victory-metal-proto`), protocol **version 2** (version 1 was M1: buffers and compute; version 2 adds
textures, placement heaps and texture copies, see sections 3 and 4), written
2026-11-27 (Asia/Shanghai). `no_std` + `alloc`, no dependencies, builds for
`x86_64-pc-windows-gnullvm` and for both Apple targets. It names no D3D12,
DXGI or Metal type: layering rule 3 (`tools/metal/check-layers.sh` enforces it).

Who uses what:

| Layer | Uses |
|---|---|
| front (`metal/front`) | `Encoder` to build blocks, `Link::submit` / `Link::call` (through `LinkExt::call_args`), `RootSigDesc::encode`, the POD types, handles |
| bridge (`metal/bridge`) | implements `Link` (Wine unixlib, in-process) and `SurfaceProvider`; moves bytes, does not look inside |
| back (`metal/back`) | `Decoder` on `submit` blocks, `args_from_raw` on `call`, `RootSigDesc::decode`, answers with `Status` |
| replay / tests | `FileReader` / `FileWriter`, the same `Decoder` |

Everything below is also in the rustdoc of the crate; the tables at the end are
generated from the code (`tests/docs.rs` fails when they go stale).

## 1. Conventions

* **Little-endian only**, `repr(C)`, no implicit padding (explicit `_pad` fields; checked at compile time). Every block, payload and command is a multiple of 8 bytes.
* **No pointers in POD.** An address is a `u64` (`gpu_va`, `cpu_addr`, `data_addr`). The two sides share one address space (same process), so an address the back returns is usable by the front as is.
* **Handles** are `u32` newtypes (`ResId`, `HeapId`, `RootSigId`, `PsoId`, `FenceId`, `QueueId`, `ListId`, `SurfaceId`, `MemHeapId`). `ResId` names a buffer or a texture (one number space; an operation that needs the other kind answers `INVALID_ARG`). **0 is null** and never a live object. The **back allocates** every handle and returns it from a sync call; the front only repeats them in the stream. `ListId` is the one exception: the front chooses it, it only means something inside one block.
* **GPU addresses**: `CreateBuffer` / `WrapMemory` return `gpu_va` (the buffer's GPU address) and, when the contents are CPU-visible, `cpu_addr`. `GetGPUVirtualAddress`, root descriptors (`SET_ROOT_DESCRIPTOR`) and CBV views all pass this `u64` through unchanged. `CreateDescHeap` returns `gpu_va` of slot 0 and the slot size `stride`; the front does handle arithmetic itself and sends `(heap, slot index)` in `SET_ROOT_TABLE`.
* **Formats** are DXGI format numbers carried as `u32` (`types::format::*` names the ones the M1 front uses). What a number means, and which are supported, is the back's business.
* **Enums are open**: a transparent integer with named constants (`BufferKind::UPLOAD`). Any number can be carried; `is_known()` says whether this version defines it. The decoder refuses unknown values where a command needs a known one (`Command::check`).
* **Barrier masks are ours** (not D3D12 states): stage `u16`: bit0 Vertex, bit1 Fragment, bit2 Tile, bit3 Object, bit4 Mesh, bit5 Dispatch, bit6 Blit, bit7 ResourceState, `ALL = 0x00FF`; visibility `u8`: bit0 Device, bit1 ResourceAlias. The front derives them from D3D12 resource states; the back maps each bit to the matching Metal stage.
* **`Status`** (`i32`, 0 = OK): `INVALID_ARG -1`, `BAD_VERSION -2`, `UNKNOWN_OP -3`, `UNSUPPORTED -4`, `BAD_STREAM -5`, `NOT_FOUND -6`, `OUT_OF_MEMORY -7`, `DEVICE_LOST -8`, `DEVICE_HUNG -9`, `TIMEOUT -10`, `NOT_INITIALIZED -11`, `INTERNAL -12`, `CONFLICT -13` (rule 6: a page already reaches the GPU through another buffer). The bridge maps these to its transport's codes.

## 2. `Link` and `SurfaceProvider`

```rust
pub trait Link: Send + Sync {
    fn submit(&self, block: &[u8]) -> Status;                       // one ExecuteCommandLists
    fn call(&self, op: SyncOp, args: *mut u8, len: usize) -> Status; // one sync call
}
pub trait LinkExt: Link { fn call_args<A: SyncArgs>(&self, args: &mut A) -> Status; } // blanket impl; sets head.version
pub trait SurfaceProvider { fn layer_for(&self, window: WindowRef) -> *mut c_void; }
```

`SurfaceProvider::layer_for` is `metal_layer_for` in the architecture note
(section 2.4); the name changed because proto does not name Metal. Install it
in the back through the bridge; it is only used by `GET_SURFACE_LAYER` (M3).

## 3. Sync calls (`Link::call`)

Each `SyncOp` has one `repr(C)` argument block (`XxxArgs`). The caller fills the
input fields, calls, and reads the outputs from the same block. The first field
of every block is `head: CallHead { version: u16, flags: u16, reserved: u32 }`:
the front sets `head.version` to the op's version (`A::VERSION`; `new()` and
`call_args` do it). The back checks, with `args_from_raw::<A>(ptr, len)`, that
the pointer is non-null and aligned, that `len == size_of::<A>()`, and that the
version matches: otherwise it answers `INVALID_ARG` / `BAD_VERSION`. An op
number it does not know gets `UNKNOWN_OP`.

Order of use: `HELLO` first (it creates the device; fails with `UNSUPPORTED`
when the device has no Metal 4 family or lacks a capability in `required_caps`),
then anything; `SHUTDOWN` ends it.

Notes the table cannot hold:

* `CREATE_BUFFER`: `kind` is a `BufferKind` (`DEFAULT` private, `UPLOAD`, `READBACK`, `SHARED`); `WRAPPED` is refused. `cpu_addr == 0` for `DEFAULT`.
* `WRAP_MEMORY`: `[addr, addr+len)` page aligned, used in place (no copy). `CONFLICT` if any page already belongs to another buffer.
* `DESTROY_*`: the back waits until the GPU is done with the object (residency rule 11); the front need not.
* `CREATE_DESC_HEAP`: `kind` is a `HeapKind` (`CBV_SRV_UAV`, `SAMPLER`; `RTV`/`DSV` reserved for M3), `flags` bit 0 = shader visible.
* `WRITE_DESCRIPTOR`: `view.kind == NONE` writes a null descriptor; `SAMPLER` reads `sampler` instead of `view`. Buffer SRV/UAV: `raw` counts 32-bit words; `typed` counts elements of `format`; `structured` counts elements of `structure_stride`. A CBV is `gpu_va` + `size_bytes`.
* Textures (version 2). `CREATE_TEXTURE`: `dimension` is a `TexDim` (1D, 2D, 3D), `format` a DXGI number (typeless ones are views-of-any-format-of-their-family), `depth_or_array` is the depth of a 3D texture and the array size of the others (a cube map is a 2D array of 6 times n slices and a cube *view*), `usage` the `tex_usage` bits (`SRV`, `UAV`, `RENDER_TARGET`, `DEPTH_STENCIL`); `heap` null makes a committed texture, else it is placed at `heap_offset`. `alloc_size` is what Metal allocated. Memory: `CREATE_HEAP` (`MemKind` DEFAULT private, UPLOAD, READBACK shared) makes a placement heap, `CREATE_PLACED_BUFFER` a buffer in it, `GET_RESOURCE_ALLOCATION_INFO` answers `size`/`alignment` from Metal's `heapTextureSizeAndAlign` / `heapBufferSizeAndAlign` (the front rounds to the original API's 64 KiB). Resources placed on the same bytes are ordered by `BARRIER{ALIASING}`; the heap is resident while it lives. `DESTROY_TEXTURE` / `DESTROY_HEAP` wait for the GPU like every destroy.
* `WRITE_DESCRIPTOR` (block version 2) texture views: `kind` `TEXTURE_SRV` / `TEXTURE_UAV`, `resource` a texture, `format` the view format (`UNKNOWN` = the texture's own; a different one needs the texture to be typeless or of the same family), `dimension` (`TexViewDim`), `plane` (depth-stencil: 0 depth, 1 stencil), `swizzle` (component sources, `[R,G,B,A]` = none), `mip_first`/`mip_count` and `slice_first`/`slice_count` (0 = all that remain), `min_lod` (SRV `ResourceMinLODClamp`). RTV/DSV have no descriptors in the back: the front keeps `(res, mip, slice, format)` records itself.
* `CREATE_COMPUTE_PIPELINE`: `dxil_addr/dxil_len` is the DXIL container, valid for the call. The conversion (Metal Shader Converter) is not part of this milestone's proto work: until the back has it the call may answer `UNSUPPORTED`.
* `FENCE_WAIT`: `timeout_ms == u64::MAX` waits forever; `TIMEOUT` when it runs out. `FENCE_VALUE` reads the completed value. The queue-side signal/wait are stream commands.
* `DEVICE_STATUS.state`: 0 fine, 1 hung, 2 removed (front's `GetDeviceRemovedReason`).

## 4. The command stream (`Link::submit`)

```text
block   = StreamHeader (32 bytes) command*
header  = magic "VMPSTRM\0" | proto_version u32 | header_len u16 (=32) | flags u16 (=0)
          | caps u64 | total_len u32 | count u32
command = opcode u16 | version u8 | flags u8 (=0) | len u32 | payload[len]      len % 8 == 0
```

* One block is one `ExecuteCommandLists`: lists run in order. A list is `BEGIN_LIST{list, queue}` ... `END_LIST{list}`; each list becomes one Metal 4 command buffer on `queue`. `QUEUE_SIGNAL` / `QUEUE_WAIT` between lists are queue operations ordered with them. `ListId`s are chosen by the front, unique within the block.
* Binding state (`SET_PIPELINE`, `SET_ROOT_SIGNATURE`, `SET_DESCRIPTOR_HEAPS`, root parameters) is per list and starts empty at `BEGIN_LIST`. Root parameters are indexed as in the root signature (`root_index`); `SET_ROOT_CONSTANTS` writes `count` 32-bit values at `dest_offset` (in 32-bit values) of the parameter; `SET_ROOT_TABLE` names a heap slot, the table's first descriptor.
* `BARRIER`: `MEMORY` = writes of `before` stages become visible to `after` stages (UAV barriers and state transitions; the front derives the stages); `ALIASING` = the two resources share memory (`res_before`/`res_after`, 0 = any); `LAYOUT` is reserved. Between two submits the back inserts its own conservative queue barrier; do not send one for that. Split barriers: send the barrier at the "end" point only.
* Offsets from the start of a block are multiples of 8, so a reader with an 8-aligned buffer may also view payloads in place; the decoder does not need that.
* **Readers refuse, never panic**: unknown opcode (`UnknownOpcode`), reserved range (`ReservedOpcode`, answer `UNSUPPORTED`), opcode version they do not know (`UnknownOpcodeVersion`), a length that runs past the block or is not what the opcode needs (`BadPayloadLength`), a count or total that is not honest (`CommandCountMismatch`, `BadTotalLength`, `TrailingBytes`), unknown enum values or stray mask bits (`BadValue`). `Error::status()` gives the `Status` to answer.
* Reserved opcode ranges (listed at the end) are for M2/M3; the encoder cannot write them and the decoder refuses them.
* Texture commands (version 2, opcodes 0x0400-0x0402). `COPY_TEXTURE_REGION` moves a region between a texture subresource (`TextureLocation{kind: SUBRESOURCE, res, plane, mip, slice}`) and a buffer footprint (`kind: FOOTPRINT, res, offset, format, row_pitch, width, height, depth`), or between two texture subresources; without `HAS_BOX` the source is the whole subresource or footprint. The front does the subresource index arithmetic (`D3D12CalcSubresource`), the back never sees a flat index. A row pitch or offset Metal cannot use is refused with `INVALID_ARG`, never copied wrong; block compressed formats are counted in blocks. `COPY_RESOURCE` copies every subresource of a resource to one of the same description. `CLEAR_UAV_TEXTURE` clears mip `mip`, slices `first_slice`.., optionally one rectangle, with four `f32` bit patterns (`float_values` = 1) or `u32` values.
* Placeholders (`DISPATCH_INDIRECT`, `FILL_BUFFER`, `CLEAR_UAV`) have complete payloads and codecs; a back may answer `UNSUPPORTED` until M2.

## 5. Root signature description

`RootSigDesc { flags, params: Vec<RootParam>, ranges: Vec<DescriptorRange>, static_samplers: Vec<StaticSampler> }`,
`encode()` / `decode()`. Bytes: 16-byte header (`flags`, `param_count`,
`range_count`, `sampler_count`, u32 each), then the records (`RootParam` 32 bytes,
`DescriptorRange` 24, `StaticSampler` 56). A `TABLE` parameter names a run of the
flat `ranges` array (`range_start`, `range_count`). `kind`: `CONSTANTS` (uses
`num_32bit_values`), `CBV`, `SRV`, `UAV` (root descriptors), `TABLE`. Range kinds:
`SRV`, `UAV`, `CBV`, `SAMPLER`; `num_descriptors = UNBOUNDED` (`u32::MAX`) and
`offset_in_table = APPEND` (`u32::MAX`) are the open-ended / follow-the-previous values.
Flags: `CBV_SRV_UAV_HEAP_INDEXED`, `SAMPLER_HEAP_INDEXED`. Limits: 64 parameters,
4096 ranges, 2032 static samplers; `decode` checks counts against the bytes
before allocating, known enums, and that tables stay inside the range array.

## 6. Dump / replay file

```text
file = FileHeader (64 bytes) block*
header = magic "VMPFILE\0" | file_version u32 | proto_version u32 | header_len u16 (=64) | flags u16 (=0)
         | block_count u32 (0 = read to the end) | caps u64 | creator [u8;16] | reserved [u32;4]
```

The blocks are exactly the blocks `submit` takes, so a bare block is also a valid
header-less dump (`FileReader::looks_like_file` tells them apart by magic). The
file is for capture and replay with no Wine and no game (`metal/back`'s `replay`
comes with M2): a recorder writes what the front sends, plus two replay-only
commands that make the file self-contained:

* `SYNC_RECORD{op, status, args, blob}`: a sync call as it happened. `args` is the whole argument block after the call (inputs and outputs); `blob` holds the memory the block pointed at (`CREATE_ROOT_SIGNATURE` bytes, `CREATE_COMPUTE_PIPELINE` DXIL). The replayer re-issues the call with `args` (the address fields re-pointed at the blob inside the file buffer) and maps the recorded handles to the live ones (ids are deterministic for a deterministic back, but a replayer must not rely on it).
* `BUFFER_DATA{res, offset, data}`: bytes the CPU wrote into a mapped buffer, to be applied before the next block that uses it.

A live back never sees these two (`Support::ReplayOnly`). Dumps made from a game
run are derived from game data and never go into the repository (rule 9); the
files in `metal/proto/tests/golden/` are synthetic. Replay itself (the executable,
handle remapping) is **not implemented yet**; only the format is.

## 7. Changing the protocol (rule 4)

In the same commit: raise `PROTO_VERSION`; raise the opcode's version byte
(`cmd::opcode_version`) or the sync block's `VERSION` when an existing one
changes shape (new ones start at 1); update the round-trip tests and the golden
files (`UPDATE_GOLDEN=1 cargo test -p victory-metal-proto --test golden`); add the
new `layout_fingerprint()` to `tests/version.rs` (keep the old arms); regenerate
the tables below (`cargo run -p victory-metal-proto --example api_table -- --write`).
`tests/version.rs` fails if the format changed but the version did not. No
pointer fields, no D3D12- or Metal-shaped fields.

## 8. API tables (generated)

`fields` lists each field as `name: type @byte offset` (padding omitted). `size`
is the block size in bytes; for commands it is the fixed head, the payload is
padded to a multiple of 8 (`+ data` = a variable part follows the head; its
length is the head's count/len field). Support: `Implemented` = the back has it
in M1, `Placeholder` = codec done, back may answer `UNSUPPORTED`, `ReplayOnly` =
only in dumps.

<!-- BEGIN GENERATED: api_table -->

### Sync calls (`Link::call`)

| num | name | ver | size | support | fields (name: type @offset) | what |
|---|---|---|---|---|---|---|
| 0 | `NOP` (`NopArgs`) | 1 | 8 | Implemented | head: CallHead @0 | does nothing; measures the call |
| 1 | `HELLO` (`HelloArgs`) | 1 | 136 | Implemented | head: CallHead @0, client_proto_version: u32 @8, required_caps: Caps @16, proto_version: u32 @24, caps: Caps @32, device_name: [u8; 96] @40 | first call: creates the device. in: client_proto_version, required_caps; out: proto_version, caps, device_name |
| 2 | `QUERY_CAPS` (`QueryCapsArgs`) | 1 | 56 | Implemented | head: CallHead @0, caps: Caps @8, max_buffer_len: u64 @16, working_set: u64 @24, page_size: u32 @32, descriptor_stride: u32 @36, sampler_stride: u32 @40, max_threads_per_group: u32 @44, simd_width: u32 @48, max_queues: u32 @52 | out: everything but head (limits, slot sizes for handle arithmetic) |
| 3 | `SHUTDOWN` (`ShutdownArgs`) | 1 | 8 | Implemented | head: CallHead @0 | destroys everything; HELLO again to restart |
| 4 | `CREATE_QUEUE` (`CreateQueueArgs`) | 1 | 24 | Implemented | head: CallHead @0, kind: u32 @8, priority: u32 @12, queue: QueueId @16 | in: kind (QueueKind), priority; out: queue |
| 5 | `DESTROY_QUEUE` (`DestroyQueueArgs`) | 1 | 16 | Implemented | head: CallHead @0, queue: QueueId @8 | in: queue (drained first) |
| 6 | `CREATE_BUFFER` (`CreateBufferArgs`) | 1 | 56 | Implemented | head: CallHead @0, size: u64 @8, kind: u32 @16, flags: u32 @20, res: ResId @24, gpu_va: u64 @32, cpu_addr: u64 @40, alloc_size: u64 @48 | in: size, kind (BufferKind, not WRAPPED), flags=0; out: res, gpu_va, cpu_addr (0 for DEFAULT), alloc_size |
| 7 | `DESTROY_BUFFER` (`DestroyBufferArgs`) | 1 | 16 | Implemented | head: CallHead @0, res: ResId @8 | in: res (freed after the GPU is done) |
| 8 | `WRAP_MEMORY` (`WrapMemoryArgs`) | 1 | 40 | Implemented | head: CallHead @0, addr: u64 @8, len: u64 @16, flags: u32 @24, res: ResId @28, gpu_va: u64 @32 | in: addr, len (page aligned), flags=0; out: res, gpu_va; CONFLICT if a page is already in another buffer |
| 9 | `CREATE_DESC_HEAP` (`CreateDescHeapArgs`) | 1 | 48 | Implemented | head: CallHead @0, kind: u32 @8, capacity: u32 @12, flags: u32 @16, heap: HeapId @20, stride: u32 @24, gpu_va: u64 @32, cpu_addr: u64 @40 | in: kind (HeapKind), capacity, flags (bit0 shader visible); out: heap, stride, gpu_va, cpu_addr |
| 10 | `DESTROY_DESC_HEAP` (`DestroyDescHeapArgs`) | 1 | 16 | Implemented | head: CallHead @0, heap: HeapId @8 | in: heap |
| 11 | `WRITE_DESCRIPTOR` (`WriteDescriptorArgs`) | 2 | 144 | Implemented | head: CallHead @0, heap: HeapId @8, index: u32 @12, view: ViewDesc @16, sampler: SamplerDesc @104 | in: heap, index, view (or sampler when view.kind = SAMPLER) |
| 12 | `COPY_DESCRIPTORS` (`CopyDescriptorsArgs`) | 1 | 32 | Implemented | head: CallHead @0, dst_heap: HeapId @8, dst_index: u32 @12, src_heap: HeapId @16, src_index: u32 @20, count: u32 @24 | in: dst_heap, dst_index, src_heap, src_index, count (same heap kind) |
| 13 | `CREATE_ROOT_SIGNATURE` (`CreateRootSignatureArgs`) | 1 | 32 | Implemented | head: CallHead @0, data_addr: u64 @8, data_len: u64 @16, root_sig: RootSigId @24 | in: data_addr, data_len (RootSigDesc::encode bytes); out: root_sig |
| 14 | `DESTROY_ROOT_SIGNATURE` (`DestroyRootSignatureArgs`) | 1 | 16 | Implemented | head: CallHead @0, root_sig: RootSigId @8 | in: root_sig |
| 15 | `CREATE_COMPUTE_PIPELINE` (`CreateComputePipelineArgs`) | 1 | 56 | Implemented | head: CallHead @0, root_sig: RootSigId @8, flags: u32 @12, dxil_addr: u64 @16, dxil_len: u64 @24, reserved: [u32; 4] @32, pso: PsoId @48 | in: root_sig, flags=0, dxil_addr, dxil_len, reserved=0; out: pso (may be UNSUPPORTED until the shader converter is wired) |
| 16 | `DESTROY_PIPELINE` (`DestroyPipelineArgs`) | 1 | 16 | Implemented | head: CallHead @0, pso: PsoId @8 | in: pso |
| 17 | `CREATE_FENCE` (`CreateFenceArgs`) | 1 | 24 | Implemented | head: CallHead @0, initial: u64 @8, flags: u32 @16, fence: FenceId @20 | in: initial, flags=0; out: fence |
| 18 | `DESTROY_FENCE` (`DestroyFenceArgs`) | 1 | 16 | Implemented | head: CallHead @0, fence: FenceId @8 | in: fence |
| 19 | `FENCE_VALUE` (`FenceValueArgs`) | 1 | 24 | Implemented | head: CallHead @0, fence: FenceId @8, value: u64 @16 | in: fence; out: value (highest completed) |
| 20 | `FENCE_WAIT` (`FenceWaitArgs`) | 1 | 32 | Implemented | head: CallHead @0, fence: FenceId @8, value: u64 @16, timeout_ms: u64 @24 | in: fence, value, timeout_ms (u64::MAX = forever); TIMEOUT when it runs out |
| 21 | `FENCE_SIGNAL` (`FenceSignalArgs`) | 1 | 24 | Implemented | head: CallHead @0, fence: FenceId @8, value: u64 @16 | in: fence, value (never lowers it) |
| 22 | `DEVICE_STATUS` (`DeviceStatusArgs`) | 1 | 16 | Implemented | head: CallHead @0, state: u32 @8 | out: state (0 fine, 1 hung, 2 removed) |
| 23 | `GET_SURFACE_LAYER` (`GetSurfaceLayerArgs`) | 1 | 32 | Placeholder | head: CallHead @0, window: u64 @8, surface: SurfaceId @16, layer_addr: u64 @24 | in: window (WindowRef); out: surface, layer_addr (M3; back may answer UNSUPPORTED) |
| 64 | `CREATE_TEXTURE` (`CreateTextureArgs`) | 1 | 72 | Implemented | head: CallHead @0, dimension: TexDim @8, format: u32 @12, width: u32 @16, height: u32 @20, depth_or_array: u32 @24, mips: u32 @28, samples: u32 @32, usage: u32 @36, flags: u32 @40, heap: MemHeapId @44, heap_offset: u64 @48, res: ResId @56, alloc_size: u64 @64 | in: dimension (TexDim), format (DXGI), width, height, depth_or_array, mips, samples, usage (tex_usage bits), flags=0, heap (null = committed), heap_offset; out: res, alloc_size |
| 65 | `DESTROY_TEXTURE` (`DestroyTextureArgs`) | 1 | 16 | Implemented | head: CallHead @0, res: ResId @8 | in: res (freed after the GPU is done) |
| 66 | `CREATE_HEAP` (`CreateHeapArgs`) | 1 | 40 | Implemented | head: CallHead @0, kind: MemKind @8, flags: u32 @12, size: u64 @16, heap: MemHeapId @24, alloc_size: u64 @32 | in: kind (MemKind), flags=0, size; out: heap, alloc_size (placement memory for buffers and textures) |
| 67 | `DESTROY_HEAP` (`DestroyHeapArgs`) | 1 | 16 | Implemented | head: CallHead @0, heap: MemHeapId @8 | in: heap |
| 68 | `CREATE_PLACED_BUFFER` (`CreatePlacedBufferArgs`) | 1 | 56 | Implemented | head: CallHead @0, heap: MemHeapId @8, offset: u64 @16, size: u64 @24, res: ResId @32, gpu_va: u64 @40, cpu_addr: u64 @48 | in: heap, offset, size; out: res, gpu_va, cpu_addr (0 in a DEFAULT heap) |
| 69 | `GET_RESOURCE_ALLOCATION_INFO` (`GetResourceAllocationInfoArgs`) | 1 | 72 | Implemented | head: CallHead @0, kind: ResKind @8, dimension: TexDim @12, format: u32 @16, width: u32 @20, height: u32 @24, depth_or_array: u32 @28, mips: u32 @32, samples: u32 @36, usage: u32 @40, heap_kind: MemKind @44, buffer_size: u64 @48, size: u64 @56, alignment: u64 @64 | in: kind (ResKind), the texture fields or buffer_size, heap_kind; out: size, alignment (what Metal's heap size-and-align says) |

### Stream commands (`Link::submit`)

| opcode | name | ver | head size | support | fields (name: type @offset) | what |
|---|---|---|---|---|---|---|
| 0x0001 | `BEGIN_LIST` | 1 | 16 | Implemented | list: ListId @0, queue: QueueId @4, flags: u32 @8 | start a command list on a queue |
| 0x0002 | `END_LIST` | 1 | 8 | Implemented | list: ListId @0 | end the list |
| 0x0003 | `QUEUE_SIGNAL` | 1 | 16 | Implemented | queue: QueueId @0, fence: FenceId @4, value: u64 @8 | queue signals a fence value |
| 0x0004 | `QUEUE_WAIT` | 1 | 16 | Implemented | queue: QueueId @0, fence: FenceId @4, value: u64 @8 | queue waits for a fence value |
| 0x0005 | `DEBUG_LABEL` | 1 | 8 + data | Implemented | len: u32 @0 | text marker (len bytes follow) |
| 0x0100 | `SET_PIPELINE` | 1 | 8 | Implemented | pso: PsoId @0 | bind a pipeline |
| 0x0101 | `SET_ROOT_SIGNATURE` | 1 | 8 | Implemented | bind_point: BindPoint @0, root_sig: RootSigId @4 | bind a root signature |
| 0x0102 | `SET_DESCRIPTOR_HEAPS` | 1 | 8 | Implemented | cbv_srv_uav: HeapId @0, sampler: HeapId @4 | choose the two heaps |
| 0x0103 | `SET_ROOT_CONSTANTS` | 1 | 16 + data | Implemented | bind_point: BindPoint @0, root_index: u32 @4, dest_offset: u32 @8, count: u32 @12 | 32-bit root constants (count u32 follow) |
| 0x0104 | `SET_ROOT_DESCRIPTOR` | 1 | 24 | Implemented | bind_point: BindPoint @0, root_index: u32 @4, kind: RootParamKind @8, gpu_va: u64 @16 | root CBV/SRV/UAV by GPU address |
| 0x0105 | `SET_ROOT_TABLE` | 1 | 16 | Implemented | bind_point: BindPoint @0, root_index: u32 @4, heap: HeapId @8, index: u32 @12 | root table = heap slot |
| 0x0200 | `DISPATCH` | 1 | 16 | Implemented | x: u32 @0, y: u32 @4, z: u32 @8 | dispatch thread groups |
| 0x0201 | `DISPATCH_INDIRECT` | 1 | 8 | Placeholder | args_va: u64 @0 | dispatch from GPU arguments (M2) |
| 0x0210 | `COPY_BUFFER_REGION` | 1 | 32 | Implemented | dst: ResId @0, src: ResId @4, dst_offset: u64 @8, src_offset: u64 @16, size: u64 @24 | buffer to buffer copy |
| 0x0211 | `FILL_BUFFER` | 1 | 24 | Placeholder | dst: ResId @0, value: u32 @4, offset: u64 @8, size: u64 @16 | fill with a 32-bit value (M2) |
| 0x0212 | `CLEAR_UAV` | 1 | 40 | Placeholder | res: ResId @0, format: u32 @4, values: [u32; 4] @8, offset: u64 @24, size: u64 @32 | clear a buffer UAV (M2) |
| 0x0300 | `BARRIER` | 1 | 16 | Implemented | class: BarrierClass @0, visibility: u8 @1, before: u16 @2, after: u16 @4, res_before: ResId @8, res_after: ResId @12 | stage-to-stage / aliasing barrier |
| 0x0400 | `COPY_TEXTURE_REGION` | 1 | 120 | Implemented | dst: TextureLocation @0, src: TextureLocation @40, dst_x: u32 @80, dst_y: u32 @84, dst_z: u32 @88, flags: u32 @92, box_left: u32 @96, box_top: u32 @100, box_front: u32 @104, box_right: u32 @108, box_bottom: u32 @112, box_back: u32 @116 | buffer footprint to texture, texture to buffer footprint, or texture to texture (optional source box) |
| 0x0401 | `COPY_RESOURCE` | 1 | 8 | Implemented | dst: ResId @0, src: ResId @4 | whole resource to a resource of the same description (buffer or texture) |
| 0x0402 | `CLEAR_UAV_TEXTURE` | 1 | 64 | Implemented | res: ResId @0, format: u32 @4, dimension: TexViewDim @8, float_values: u8 @9, flags: u16 @10, mip: u32 @12, first_slice: u32 @16, slice_count: u32 @20, values: [u32; 4] @24, rect_left: u32 @40, rect_top: u32 @44, rect_right: u32 @48, rect_bottom: u32 @52 | clear a texture UAV (mip, slices, optional rectangle) with float or uint values |
| 0x0F00 | `SYNC_RECORD` | 1 | 16 + data | ReplayOnly | op: SyncOp @0, status: Status @4, args_len: u32 @8, blob_len: u32 @12 | recorded sync call (args, blob follow) |
| 0x0F01 | `BUFFER_DATA` | 1 | 24 + data | ReplayOnly | res: ResId @0, offset: u64 @8, len: u64 @16 | recorded CPU write to a buffer (len bytes follow) |

### Reserved opcode ranges

- 0x0403-0x04FF: textures: numbers not yet used
- 0x0500-0x05FF: mesh shading (M3)
- 0x0600-0x06FF: graphics state and draws (M3)
- 0x0700-0x07FF: present (M3)
- 0x0800-0x08FF: queries and timestamps (M2)
- 0x0900-0x09FF: ExecuteIndirect (M2)
<!-- END GENERATED: api_table -->
