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

Not covered (refused with `UNSUPPORTED`, nothing created): `R32G32B32_*` (no
96-bit Metal format), `B8G8R8X8_*` (no format with an unused channel; mapping
to BGRA8 would change what alpha reads as), `B4G4R4A4_UNORM`, `R1_UNORM`,
`R10G10B10_XR_BIAS_A2_UNORM`, video/planar and palette formats, `R8G8_B8G8`,
`G8R8_G8B8`.

Semantic differences, all written in the module docs:

* `D24_UNORM_S8_UINT` and `R24G8` → `Depth32Float_Stencil8`: depth is a float,
  not a 24-bit integer; a comparison against a constant sees the exact value
  rather than one rounded to 1/16777215; texels are 8 bytes (4 depth, 1
  stencil, padding), so `GET_RESOURCE_ALLOCATION_INFO` gives the real size;
  copies between buffer and depth/stencil texture move floats for depth
  (`R32_FLOAT` footprint) and bytes for stencil (`R8_UINT`); an
  `R24_UNORM_X8_TYPELESS` footprint is refused, not converted.
* The front counts D24S8, D32S8X24 and the R24G8/R32G8X24 typeless/view
  families as two planes (depth, stencil), as D3D12 and D3DMetal do.
* Block-compressed formats are counted in blocks (4×4). The front computes
  footprints with D3D12's rules (pitch aligned to 256, subresources at 512);
  the back refuses a row pitch smaller than a row, not a multiple of the
  texel (block) size, an offset not a multiple of it, a region off the mip or
  past the buffer, with `INVALID_ARG`: never copied wrong.
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

* `CreateCommittedResource3`, `CreatePlacedResource2` (Device10, enhanced barriers,
  castable formats): not implemented; `victory-gpu` falls back to the older calls.
* `B8G8R8X8_*`: refused (see above); `victory-gpu`'s host UAV test for it fails there.
* Reserved resources and tile mappings, `OpenExistingHeapFromAddress`: not done.
* Buffer UAV clears through `ClearUnorderedAccessView*` stay `E_NOTIMPL` in the shell
  (texture ones are done).
* Layout barriers (`BARRIER{LAYOUT}`) are `UNSUPPORTED`: not needed (private textures).
