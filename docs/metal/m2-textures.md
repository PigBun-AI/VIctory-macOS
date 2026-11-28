# M2: textures, heaps, custom heaps (front and back)

Written 2026-11-27 on branch `m3-tex`. Frameworks: Metal 4 on macOS 27 (checked
2026-11-27; `MTLHeap` sizes from `heapTextureSizeAndAlign` /
`heapBufferSizeAndAlign`, views from `newTextureViewWithPixelFormat:textureType:levels:slices:`),
`objc2-metal` 0.3.2. Wire numbers: [proto.md](proto.md) (version 2; SyncOp
0x40–0x45, opcodes 0x0400–0x0402; tables are generated from code).

## What is there

| Piece | Where |
|---|---|
| DXGI → `MTLPixelFormat` table, one place, with tests | `metal/back/src/format.rs` |
| Textures (private storage, usage from usage bits, `PixelFormatView` when a view reinterprets), cached views | `back/src/texture.rs` |
| Heaps, placed buffers/textures, size and alignment | `back/src/heap.rs` |
| Buffer ↔ texture and texture ↔ texture copies | `back/src/copy.rs` |
| Texture UAV clears (compute kernels, argument slots 29/30) | `back/src/clear.rs` |
| Sampler objects and sampler heap entries | `back/src/exec/sync.rs` |
| Front: resource descriptions, custom heaps, subresource arithmetic, SRV/UAV/RTV/DSV descriptions | `front/src/{resource,device,view,footprint}.rs`, shell in `front/src/shell/` |

## Format coverage (back table)

Covered: RGBA8/BGRA8 (UNORM, sRGB, SNORM, UINT, SINT, typeless), R8/RG8,
R16/RG16/RGBA16 (FLOAT, UNORM, SNORM, UINT, SINT), R32/RG32/RGBA32
(FLOAT, UINT, SINT), R11G11B10_FLOAT, R10G10B10A2 (UNORM, UINT), R9G9B9E5,
B5G6R5, B5G5R5A1, BC1–BC7 (UNORM, sRGB, BC4/5 SNORM, BC6H UF/SF), depth (D16, D32, D24S8, D32S8X24) and their
plane views. Typeless formats are only views: the texture keeps the family
and views reinterpret (`PixelFormatView`); a view of another family is `INVALID_ARG`.

`B8G8R8X8_UNORM`, `_TYPELESS` and `_UNORM_SRGB` are stored as BGRA8 (see
"B8G8R8X8" below).

Not covered (refused with `UNSUPPORTED`, nothing created): `R32G32B32_*` (no
96-bit Metal format), `B4G4R4A4_UNORM`, `R1_UNORM`,
`R10G10B10_XR_BIAS_A2_UNORM`, video/planar and palette formats, `R8G8_B8G8`,
`G8R8_G8B8`.

Semantic differences, all written in the module docs:

* `D24_UNORM_S8_UINT` and `R24G8` → `Depth32Float_Stencil8`: depth is a float,
  not a 24-bit integer; a comparison against a constant sees the exact value
  rather than one rounded to 1/16777215; texels are 8 bytes (4 depth, 1
  stencil, padding), so `GET_RESOURCE_ALLOCATION_INFO` gives the real size;
  copies between buffer and depth/stencil texture move floats for depth
  (`R32_FLOAT` footprint) and bytes for stencil (`R8_UINT`); an
  `R24_UNORM_X8_TYPELESS` footprint of a depth plane is converted
  (`round(d * 16777215)` in the low 24 bits, top byte 0: `copy.rs` pack and
  unpack kernels; `tests/copy_d24.rs`).
* The front counts D24S8, D32S8X24 and the R24G8/R32G8X24 typeless/view
  families as two planes (depth, stencil), as D3D12 and D3DMetal do.
* Block-compressed formats are counted in blocks (4×4). The front computes
  footprints with D3D12's rules (pitch aligned to 256, subresources at 512);
  the back refuses a row pitch smaller than a row, not a multiple of the
  texel (block) size, an offset not a multiple of it, a region off the mip or
  past the buffer, with `INVALID_ARG`: never copied wrong. A compressed mip is stored in whole
  blocks, so the edge of a region is the mip size **rounded up to the block**: D3D12's footprint of a
  2×2 or 1×1 BC mip is one 4×4 block and `CopyTextureRegion` takes it (the game's BC streaming does
  this; the back used to refuse it with `INVALID_ARG`).
* A copy between a BC texture and an uncompressed texture whose texel is the BC block
  (`R32G32B32A32_UINT` ↔ BC2/3/5/6H/7, `R32G32_UINT` ↔ BC1/4) is allowed by D3D12 and refused by
  Metal's texture-to-texture blit (formats differ). The back does it through a private staging
  buffer (texture → buffer in the source's format, buffer → texture in the destination's); the region is
  counted in the source's texels. Any other mix of formats is still refused.
* Also through the same staging buffer: `CopyResource` / region copies between formats of one
  layout that Metal names differently (`R8G8B8A8_TYPELESS` ↔ `_UNORM_SRGB`: bytes move as they are), and one plane of a
  depth-stencil texture (to the same plane of another, or the depth plane into a depth-only texture;
  Metal's blit moves both planes or none). The D24 depth plane is still refused.
* Samplers: Metal has no min/max reduction filter, so a sampler with one is
  refused (`WRITE_DESCRIPTOR` → `UNSUPPORTED`; tested). Metal has three border colours
  (transparent black, opaque black, opaque white): any other D3D12 border colour
  is **approximated** as transparent black, not refused (a known difference).
  The LOD bias travels in the descriptor entry; anisotropy is passed on.
* Row-major textures (`D3D12_TEXTURE_LAYOUT_ROW_MAJOR`) → `E_NOTIMPL`.

## Heaps and placement

`CreateHeap` → `CREATE_HEAP`: size a multiple of the alignment (64 KiB or
4 MiB; 0 means 64 KiB), else `E_INVALIDARG`. A placed resource's offset must
be a multiple of the heap's alignment and below the heap size. The heap is in
the residency set while it lives. Buffers and textures placed on the same
bytes are ordered by `BARRIER{ALIASING}`; there is no other synchronisation
(as in D3D12). Textures can only be placed in `DEFAULT` (private) heaps.
`GetResourceAllocationInfo` answers from Metal's `heapTextureSizeAndAlign`,
rounded to 64 KiB; `size = u64::MAX` when the back refuses the description.

## Custom heaps (`D3D12_HEAP_TYPE_CUSTOM`)

Metal on Apple silicon has one memory; the mapping (`front/src/resource.rs`,
`HeapProps::memory`):

| `CPUPageProperty` | `MemoryPoolPreference` | Back memory |
|---|---|---|
| `NOT_AVAILABLE` | L0 or L1 or unknown | `DEFAULT` (private) |
| `WRITE_COMBINE` or `WRITE_BACK` | L0 or L1 | `SHARED` (cached, CPU-visible; `Map` returns the CPU pointer) |
| `UNKNOWN` (0) on a custom heap, or an unknown pool/page value | | `E_INVALIDARG` |

* **L1 is treated as L0** (there is no second pool). **Write-combine is not
  kept**: the memory is cached, which is never wrong for CPU reads, and the
  GPU sees the same bytes.
* A texture in CPU-visible custom memory is a private texture; `Map` on it is
  `E_INVALIDARG` (a texture has no linear layout on the back).
* `CreateHeap` and `CreatePlacedResource` accept custom heaps the same way.
* `GetCustomHeapProperties(node, type)` answers for unified memory:
  `DEFAULT` → `NOT_AVAILABLE`/L0, `UPLOAD` → `WRITE_COMBINE`/L0, `READBACK` →
  `WRITE_BACK`/L0, nodes masks 1.
* `CheckFeatureSupport(ARCHITECTURE/ARCHITECTURE1)` reports `UMA = 1` and
  `CacheCoherentUMA = 1` (from `Caps.unified_memory`, `front/src/caps.rs`),
  consistent with the answers above: no discrete pool, CPU-cached memory is coherent
  with the GPU.

## Tests (2026-11-27)

* back, native GPU and under Rosetta: `back/tests/texture_gpu.rs` 20 tests
  (upload→readback, mips/slices, BC, 1D/3D, depth planes, typeless views,
  swizzle/cube, samplers, UAV write, heaps and aliasing, clears, copy error
  paths); `back/tests/exec_textures.rs` 7 (the same through the wire);
  format table tests in `format.rs`.
* front (no GPU, mock back): `front/tests/textures.rs` 10, view description
  tests in `view.rs` 4, footprint and resource tests; the front golden bytes
  were regenerated.
* Wine, `victory-shader`: see the conformance file; the 41 texture entries
  pass and are removed from `metal/conformance/expected-fail.txt`.

## Known gaps

* Reserved resources and tile mappings, `OpenExistingHeapFromAddress`: not done.
* Buffer UAV clears through `ClearUnorderedAccessView*` stay `E_NOTIMPL` in the shell
  (texture ones are done).
* Layout barriers (`BARRIER{LAYOUT}`) are `UNSUPPORTED`: not needed (private textures).

## B8G8R8X8, CreateCommittedResource3, CreatePlacedResource2 (M4)

* **B8G8R8X8** has no Metal format. The back stores it as BGRA8 (and `_SRGB`
  as BGRA8 sRGB). The difference: D3D12 reads the unused X channel of a
  shader view as 1; here the stored byte is a real alpha byte. A shader
  resource view of such a texture (plane 0, not a UAV) therefore gets the
  swizzle `[R, G, B, One]`, so reads give alpha 1 whatever is stored. What is
  still different: a render target, a UAV write and a copy keep the byte they
  were given (D3D12 leaves it undefined), and blending into one with alpha
  factors changes the byte (nobody reads it through a view). Tests:
  `format.rs`, `texture_gpu::the_unused_channel_of_a_bgrx_texture_reads_as_one`,
  `exec_compat::a_bgrx_srv_gets_alpha_one`.
* **CreateCommittedResource3 / CreatePlacedResource2** take the layout
  (`D3D12_BARRIER_LAYOUT`) and a list of castable formats. The layout is the
  initial *state* of the texture here (`state::state_of_layout`; layouts do
  not exist on the back, textures are private). The castable list is one
  flag, `tex_usage::CAST` (proto: bit 4 of the texture usage, `ALL_BITS` =
  0x1F): the texture is made with `PixelFormatView` and views of **any
  format of the same texel size** are allowed (`format::view_pixel_format`),
  not only formats of the same family, which is what the castable list of
  D3D12 permits. The list itself is not enforced (a view of a format outside
  it succeeds where D3D12 fails it). Tests: `format.rs`,
  `texture_gpu::a_castable_texture_is_read_in_another_format_family`.
* Typed UAV support (`caps::format_support`) now also lists the 2-channel and
  packed formats (RG32, RGB10A2, R11G11B10, RG16, RG8, ...): placed views of
  a castable texture are made in them, and Apple GPUs write them all.
