# Metal backend front: the D3D12 / DXGI surface the game side uses

Written 2026-11-27 for milestone M1, from a scan of `crates/victory-gpu/src` and
`crates/victory-shader/src` (every `.Method(` that is a D3D12 / DXGI method name,
cross-checked against the `windows` 0.62.2 interface tables). Call sites are
`victory-gpu` as `gpu/…` and `victory-shader` as `shader/…`, line numbers of the
scanned tree (worktree `m1-front`).

> **New base (mac/livetr):** this survey is of the old base. The calls the new
> base adds, and what the front answers, are in [m4-newbase.md](m4-newbase.md).

## 1. Milestone tags

* **M1** compute: buffers, descriptor heaps and tables, root signatures, compute
  pipelines, fences and queues, copies between buffers, barriers, feature queries.
  Needs no texture and no present.
* **M2** textures and the rest of GPU work: texture copies, placed/reserved resources
  and heaps, indirect dispatch (`ExecuteIndirect`), UAV clears, query heaps,
  timestamps, pipeline libraries.
* **M3** graphics: graphics pipelines, RTV/DSV, draws, swap chains and present.
* **diag**: debug layer and DRED. Our `D3D12GetDebugInterface` answers
  `DXGI_ERROR_SDK_COMPONENT_MISSING`, so applications never get these objects.

118 distinct method names are called: **M1 53** (all implemented),
M2 24, M3 31, diag 10.

**Status** `done` = implemented in the shell (`metal/front/src/shell`), calling the
core; `no-op` = accepted and ignored (RTV/DSV creation, markers); `E_NOTIMPL` = the
generated stub answers `E_NOTIMPL` (void methods: the command list is marked
failed and `Close` reports it); `stub` = diag.

Methods found nowhere in the game side are also implemented when D3D12 needs them
to be sane (`GetNodeCount`, `MakeResident`, `Evict`, `GetNodeCount`, `SetStablePowerState`,
`GetCopyableFootprints`, `GetHeapProperties`, `GetResourceAllocationInfo`, the
`GetDesc*` of adapters, `QueryVideoMemoryInfo`, …) — 76 `x_` methods in all.

## 2. Exports

| dll | export | where it is used | status |
|---|---|---|---|
| d3d12.dll | `D3D12CreateDevice` | gpu/host.rs:343,346,1088; shader/emit/host.rs:152 | done (FL 12_0 required; foreign adapters refused) |
| d3d12.dll | `D3D12SerializeRootSignature` | gpu/root.rs:106 via `GetProcAddress` | done, byte-identical to D3DMetal's output |
| d3d12.dll | `D3D12SerializeVersionedRootSignature` | gpu/root.rs:132 | done |
| d3d12.dll | `D3D12CreateRootSignatureDeserializer` | gpu/root.rs:164 | done |
| d3d12.dll | `D3D12CreateVersionedRootSignatureDeserializer` | (windows crate import) | done |
| d3d12.dll | `D3D12GetDebugInterface` | gpu/host.rs:315,318 (Debug1/2 casts) | `DXGI_ERROR_SDK_COMPONENT_MISSING` |
| d3d12.dll | `D3D12EnableExperimentalFeatures`, `D3D12GetInterface` | — | `S_OK` for 0 features / `E_NOINTERFACE` |
| dxgi.dll | `CreateDXGIFactory`, `CreateDXGIFactory1`, `CreateDXGIFactory2` | gpu/host.rs:462,477,550,1030,5647; shader/emit/host.rs:144 | done (flags ignored) |
| dxgi.dll | `DXGIGetDebugInterface1` | — | `DXGI_ERROR_SDK_COMPONENT_MISSING` |
| dxgi.dll | `DXGIDeclareAdapterRemovalSupport` | — | `S_OK` |

**How `root.rs` loads them**: `host_export` (gpu/root.rs:53-78) calls
`LoadLibraryExW(L"d3d12.dll", NULL, LOAD_LIBRARY_SEARCH_SYSTEM32)` once and then
`GetProcAddress` by name (`D3D12SerializeRootSignature`, `…VersionedRootSignature`,
`D3D12CreateRootSignatureDeserializer`). So the file in `system32` must export those
names undecorated (x64 `extern "system"`) — ours do. The M0 test
(`tools/metal/test-override-order.sh`) shows that with `WINEDLLOVERRIDES=…d3d12,dxgi=n,b`
this loads our dll.

## 3. Interfaces and QueryInterface

The shell lists only the highest interface per object; `#[implement]` makes
`QueryInterface` answer for the whole chain below it (`IUnknown` included).

| object | highest interface | QI answers | used above the base |
|---|---|---|---|
| factory | `IDXGIFactory7` | `IDXGIObject`, `IDXGIFactory`…`7` | Factory2 (gpu/host.rs:5647), Factory4 everywhere |
| adapter | `IDXGIAdapter4` | `IDXGIObject`, `IDXGIAdapter`…`4` | Adapter1 (gpu/host.rs:479,1039) |
| device | `ID3D12Device14` | `ID3D12Object`, `ID3D12Device`…`14` (not `ID3D12DeviceChild`: `E_NOINTERFACE`, as D3DMetal) | Device1 (gpu/pipeline/library.rs:1292…), 2 (gpu/pipeline/host.rs:1039), 3 (gpu/inplace_probe.rs:101), 10 (gpu/resource.rs:1571), 13 (gpu/inplace_probe.rs:104) |
| command list | `ID3D12GraphicsCommandList6` | `ID3D12CommandList`, `…GraphicsCommandList`…`6` | List1 (gpu/list.rs:1150), List6 (gpu/draw.rs:1805) |
| queue | `ID3D12CommandQueue` | `ID3D12Pageable` chain | |
| fence | `ID3D12Fence1` | `ID3D12Fence`, `Fence1` | |
| resource | `ID3D12Resource2` | `ID3D12Resource`, `1`, `2` | |
| descriptor heap, root signature, pipeline state, allocator | their own | | |
| deserializers | `ID3D12RootSignatureDeserializer`, `ID3D12VersionedRootSignatureDeserializer` | | |

The smoke test (`metal/front/smoke`) checks 176 `GOLD` lines of exactly these QI
chains and their `HRESULT`s against D3DMetal's.

**EnumWarpAdapter** is called once, in `shader/emit/host.rs:146`, by the `warp = true`
branch of the test helper `create_device`. We answer `DXGI_ERROR_UNSUPPORTED`
(0x887A0004), the same as D3DMetal; those tests then fail with a message,
nothing else breaks. `EnumAdapters1` end-of-list is `DXGI_ERROR_NOT_FOUND`
(0x887A0002). One adapter; `EnumAdapterByLuid` matches its LUID; the adapter has no outputs.

## 4. Methods used, by milestone

### M1
| method | interface(s) | sites | first call sites (file:line) | status |
|---|---|--:|---|---|
| `CheckFeatureSupport` | ID3D12Device, IDXGIFactory5 | 31 | gpu/resource.rs:279, gpu/resource.rs:2003, gpu/host.rs:353 | done |
| `Close` | ID3D12GraphicsCommandList | 12 | gpu/host.rs:148, gpu/host.rs:200, gpu/host.rs:1933 | done |
| `CopyBufferRegion` | ID3D12GraphicsCommandList | 15 | gpu/host.rs:147, gpu/list.rs:2356, gpu/list.rs:2388 | done |
| `CopyDescriptors` | ID3D12Device | 1 | gpu/descriptor/host.rs:1835 | done |
| `CopyDescriptorsSimple` | ID3D12Device | 2 | gpu/descriptor/host.rs:735, gpu/descriptor/host.rs:742 | done |
| `CopyResource` | ID3D12GraphicsCommandList | 5 | gpu/host.rs:5579, gpu/list.rs:832, gpu/descriptor/host.rs:4558 | done |
| `CreateCommandAllocator` | ID3D12Device | 11 | gpu/host.rs:140, gpu/host.rs:192, gpu/host.rs:1095 | done |
| `CreateCommandList` | ID3D12Device | 11 | gpu/host.rs:141, gpu/host.rs:193, gpu/host.rs:1098 | done |
| `CreateCommandQueue` | ID3D12Device | 8 | gpu/host.rs:367, gpu/host.rs:2661, gpu/alias.rs:655 | done |
| `CreateCommittedResource` | ID3D12Device | 34 | gpu/resource.rs:1641, gpu/resource.rs:3799, gpu/resource.rs:4212 | done |
| `CreateComputePipelineState` | ID3D12Device | 11 | gpu/memory.rs:908, gpu/compute.rs:98, gpu/hdr_probe.rs:1048 | done |
| `CreateConstantBufferView` | ID3D12Device | 1 | shader/emit/host.rs:1924 | done |
| `CreateDescriptorHeap` | ID3D12Device | 27 | gpu/ambient.rs:323, gpu/host.rs:374, gpu/host.rs:381 | done |
| `CreateFence` | ID3D12Device | 8 | gpu/host.rs:371, gpu/device.rs:568, gpu/alias.rs:670 | done |
| `CreateRootSignature` | ID3D12Device | 11 | gpu/memory.rs:899, gpu/compute.rs:199, gpu/indirect.rs:283 | done |
| `CreateSampler` | ID3D12Device | 6 | gpu/sampler.rs:200, gpu/descriptor/host.rs:769, gpu/descriptor/host.rs:3232 | done |
| `CreateShaderResourceView` | ID3D12Device | 20 | gpu/ambient.rs:516, gpu/ambient.rs:528, gpu/ambient_estimate.rs:417 | done |
| `CreateUnorderedAccessView` | ID3D12Device | 25 | gpu/ambient.rs:506, gpu/twin.rs:306, gpu/hdr_probe.rs:1084 | done |
| `Dispatch` | ID3D12GraphicsCommandList | 44 | gpu/ambient.rs:539, gpu/memory.rs:999, gpu/memory.rs:1124 | done |
| `EnumAdapterByLuid` | IDXGIFactory4 | 6 | gpu/host.rs:463, gpu/host.rs:551, gpu/pipeline/depth_bounds_test.rs:115 | done |
| `EnumAdapters1` | IDXGIFactory1 | 2 | shader/emit/host.rs:203, shader/emit/host.rs:274 | done |
| `EnumWarpAdapter` | IDXGIFactory4 | 3 | gpu/host.rs:478, gpu/host.rs:1034, shader/emit/host.rs:146 | done |
| `ExecuteCommandLists` | ID3D12CommandQueue | 9 | gpu/host.rs:2237, gpu/host.rs:2242, gpu/host.rs:2644 | done |
| `GetAdapterLuid` | ID3D12Device | 6 | gpu/host.rs:461, gpu/host.rs:549, gpu/pipeline/depth_bounds_test.rs:115 | done |
| `GetCPUDescriptorHandleForHeapStart` | ID3D12DescriptorHeap | 31 | gpu/ambient.rs:495, gpu/host.rs:2851, gpu/host.rs:2863 | done |
| `GetCompletedValue` | ID3D12Fence | 6 | gpu/host.rs:2548, gpu/alias.rs:673, shader/emit/host.rs:863 | done |
| `GetCopyableFootprints` | ID3D12Device | 12 | gpu/texback.rs:275, gpu/texback.rs:518, gpu/twin.rs:147 | done |
| `GetDesc` | ID3D12CommandQueue, ID3D12DescriptorHeap, ID3D12DeviceConfiguration, ID3D12FunctionParameterReflection, ID3D12FunctionReflection, ID3D12Heap, ID3D12LibraryReflection, ID3D12ProtectedResourceSession, ID3D12Resource, ID3D12ShaderCacheSession, ID3D12ShaderReflection, ID3D12ShaderReflectionConstantBuffer, ID3D12ShaderReflectionType, ID3D12ShaderReflectionVariable, IDXGIAdapter, IDXGIOutput, IDXGIOutputDuplication, IDXGISurface, IDXGISwapChain | 76 | gpu/resource.rs:364, gpu/resource.rs:965, gpu/resource.rs:1467 | done |
| `GetDesc1` | ID3D12ProtectedResourceSession1, ID3D12Resource2, IDXGIAdapter1, IDXGIOutput6, IDXGISwapChain1 | 10 | gpu/host.rs:464, gpu/host.rs:479, gpu/host.rs:634 | done |
| `GetDescriptorHandleIncrementSize` | ID3D12Device | 16 | gpu/ambient.rs:333, gpu/host.rs:379, gpu/host.rs:386 | done |
| `GetDevice` | ID3D12DeviceChild, IDXGIDeviceSubObject | 1 | gpu/device.rs:570 | done |
| `GetDeviceRemovedReason` | ID3D12Device | 30 | gpu/host.rs:264, gpu/host.rs:450, gpu/host.rs:637 | done |
| `GetGPUDescriptorHandleForHeapStart` | ID3D12DescriptorHeap | 11 | gpu/ambient.rs:496, gpu/draw.rs:4625, gpu/hdr_probe.rs:1142 | done |
| `GetGPUVirtualAddress` | ID3D12Resource | 73 | gpu/host.rs:1000, gpu/host.rs:3750, gpu/host.rs:3773 | done |
| `GetResourceAllocationInfo` | ID3D12Device | 5 | gpu/resource.rs:968, gpu/resource.rs:1467, gpu/placement.rs:165 | done |
| `GetRootSignatureDesc` | ID3D12RootSignatureDeserializer | 1 | gpu/root.rs:509 | done |
| `Map` | ID3D12Resource, IDXGISurface | 46 | gpu/resource.rs:3848, gpu/resource.rs:4264, gpu/host.rs:2960 | done |
| `QueryVideoMemoryInfo` | IDXGIAdapter3 | 1 | gpu/host.rs:558 | done |
| `Reset` | ID3D12CommandAllocator, ID3D12GraphicsCommandList | 6 | gpu/host.rs:3050, gpu/host.rs:3051, gpu/host.rs:3486 | done |
| `ResourceBarrier` | ID3D12GraphicsCommandList | 87 | gpu/resource.rs:3828, gpu/ambient.rs:362, gpu/ambient.rs:461 | done |
| `SetComputeRoot32BitConstants` | ID3D12GraphicsCommandList | 25 | gpu/ambient.rs:532, gpu/memory.rs:997, gpu/memory.rs:1122 | done |
| `SetComputeRootConstantBufferView` | ID3D12GraphicsCommandList | 2 | gpu/draw.rs:1234, shader/emit/host.rs:2102 | done |
| `SetComputeRootDescriptorTable` | ID3D12GraphicsCommandList | 42 | gpu/ambient.rs:533, gpu/memory.rs:994, gpu/memory.rs:1119 | done |
| `SetComputeRootShaderResourceView` | ID3D12GraphicsCommandList | 20 | gpu/memory.rs:995, gpu/memory.rs:1120, gpu/memory.rs:1192 | done |
| `SetComputeRootSignature` | ID3D12GraphicsCommandList | 48 | gpu/ambient.rs:530, gpu/memory.rs:993, gpu/memory.rs:1118 | done |
| `SetComputeRootUnorderedAccessView` | ID3D12GraphicsCommandList | 30 | gpu/memory.rs:996, gpu/memory.rs:1121, gpu/memory.rs:1193 | done |
| `SetDescriptorHeaps` | ID3D12GraphicsCommandList | 33 | gpu/ambient.rs:529, gpu/memory.rs:992, gpu/memory.rs:1117 | done |
| `SetEventOnCompletion` | ID3D12Fence | 4 | gpu/host.rs:2245, gpu/host.rs:2647, gpu/alias.rs:674 | done |
| `SetMarker` | ID3D12CommandQueue, ID3D12GraphicsCommandList | 1 | gpu/host.rs:3564 | done |
| `SetName` | ID3D12Object | 1 | gpu/host.rs:1513 | done |
| `SetPipelineState` | ID3D12GraphicsCommandList | 50 | gpu/ambient.rs:531, gpu/host.rs:6794, gpu/memory.rs:998 | done |
| `Signal` | ID3D12CommandQueue, ID3D12Fence | 8 | gpu/host.rs:2244, gpu/host.rs:2646, gpu/alias.rs:672 | done |
| `Unmap` | ID3D12Resource, IDXGISurface | 38 | gpu/resource.rs:4266, gpu/host.rs:2964, gpu/texback.rs:379 | done |

### M2
(texture, heap and custom-heap work: see [m2-textures.md](m2-textures.md); `GetCustomHeapProperties` is done too.)

| method | interface(s) | sites | first call sites (file:line) | status |
|---|---|--:|---|---|
| `ClearUnorderedAccessViewFloat` | ID3D12GraphicsCommandList | 1 | gpu/draw.rs:1498 | done |
| `ClearUnorderedAccessViewUint` | ID3D12GraphicsCommandList | 2 | gpu/draw.rs:1501, gpu/draw.rs:4990 | done |
| `CopyTextureRegion` | ID3D12GraphicsCommandList | 41 | gpu/resource.rs:3833, gpu/resource.rs:4182, gpu/resource.rs:4256 | done |
| `CreateCommandSignature` | ID3D12Device | 3 | gpu/draw.rs:2777, gpu/indirect.rs:818, gpu/indirect.rs:1263 | E_NOTIMPL/no-op |
| `CreateCommittedResource3` | ID3D12Device10 | 1 | gpu/resource.rs:1614 | E_NOTIMPL/no-op |
| `CreateHeap` | ID3D12Device | 2 | gpu/alias.rs:389, gpu/placement.rs:141 | done |
| `CreatePlacedResource` | ID3D12Device | 4 | gpu/resource.rs:1584, gpu/alias.rs:391, gpu/placement.rs:252 | done |
| `CreatePlacedResource2` | ID3D12Device10 | 1 | gpu/resource.rs:1577 | E_NOTIMPL/no-op |
| `CreateQueryHeap` | ID3D12Device | 1 | gpu/host.rs:2918 | done (timestamp heaps, approximate ticks: m4-newbase.md) |
| `CreateReservedResource` | ID3D12Device | 1 | gpu/inplace_probe.rs:790 | E_NOTIMPL/no-op |
| `EndQuery` | ID3D12GraphicsCommandList | 2 | gpu/host.rs:3013, gpu/host.rs:3555 | done (timestamp only) |
| `ExecuteIndirect` | ID3D12GraphicsCommandList | 5 | gpu/draw.rs:2711, gpu/indirect.rs:837, gpu/indirect.rs:1291 | E_NOTIMPL/no-op |
| `GetSerializedSize` | ID3D12PipelineLibrary | 5 | gpu/pipeline/library.rs:727, gpu/pipeline/library.rs:1299, gpu/pipeline/library.rs:1438 | E_NOTIMPL/no-op |
| `GetTimestampFrequency` | ID3D12CommandQueue | 2 | gpu/host.rs:2273, gpu/host.rs:2279 | done |
| `LoadComputePipeline` | ID3D12PipelineLibrary | 5 | gpu/pipeline/host.rs:757, gpu/pipeline/library.rs:1432, gpu/pipeline/library.rs:1563 | E_NOTIMPL/no-op |
| `LoadGraphicsPipeline` | ID3D12PipelineLibrary | 1 | gpu/pipeline/host.rs:925 | E_NOTIMPL/no-op |
| `LoadPipeline` | ID3D12PipelineLibrary1 | 1 | gpu/pipeline/host.rs:1043 | E_NOTIMPL/no-op |
| `OpenExistingHeapFromAddress` | ID3D12Device3 | 1 | gpu/inplace_probe.rs:102 | E_NOTIMPL/no-op |
| `OpenExistingHeapFromAddress1` | ID3D12Device13 | 1 | gpu/inplace_probe.rs:105 | E_NOTIMPL/no-op |
| `OpenExistingHeapFromFileMapping` | ID3D12Device3 | 1 | gpu/inplace_probe.rs:108 | E_NOTIMPL/no-op |
| `ResolveQueryData` | ID3D12GraphicsCommandList | 1 | gpu/host.rs:3015 | done (timestamp only, `WRITE_IMMEDIATE`) |
| `Serialize` | ID3D12PipelineLibrary | 4 | gpu/pipeline/library.rs:830, gpu/pipeline/library.rs:843, gpu/pipeline/library.rs:1439 | E_NOTIMPL/no-op |
| `StorePipeline` | ID3D12PipelineLibrary | 5 | gpu/pipeline/library.rs:635, gpu/pipeline/library.rs:667, gpu/pipeline/library.rs:680 | E_NOTIMPL/no-op |
| `UpdateTileMappings` | ID3D12CommandQueue | 2 | gpu/inplace_probe.rs:815, gpu/inplace_probe.rs:862 | E_NOTIMPL/no-op |

### M3
| method | interface(s) | sites | first call sites (file:line) | status |
|---|---|--:|---|---|
| `ClearDepthStencilView` | ID3D12GraphicsCommandList | 4 | gpu/list.rs:1263, gpu/draw.rs:5294, gpu/pipeline/depth_bounds_test.rs:239 | E_NOTIMPL/no-op |
| `ClearRenderTargetView` | ID3D12GraphicsCommandList | 3 | gpu/list.rs:1223, gpu/draw.rs:3746, shader/emit/memory_tests.rs:1833 | E_NOTIMPL/no-op |
| `CreateDepthStencilView` | ID3D12Device | 2 | gpu/host.rs:2880, shader/emit/memory_tests.rs:1696 | done |
| `CreateGraphicsPipelineState` | ID3D12Device | 6 | gpu/host.rs:6701, gpu/draw.rs:3709, gpu/hdr_probe.rs:1033 | E_NOTIMPL/no-op |
| `CreatePipelineState` | ID3D12Device2 | 2 | gpu/pipeline/host.rs:1045, shader/emit/host.rs:721 | E_NOTIMPL/no-op |
| `CreateRenderTargetView` | ID3D12Device | 5 | gpu/host.rs:2854, gpu/host.rs:6739, gpu/hdr_probe.rs:1078 | done |
| `CreateSwapChainForHwnd` | IDXGIFactory2 | 1 | gpu/host.rs:5663 | E_NOTIMPL/no-op |
| `DispatchMesh` | ID3D12GraphicsCommandList6 | 2 | gpu/draw.rs:1810, shader/emit/host.rs:1357 | E_NOTIMPL/no-op |
| `DrawIndexedInstanced` | ID3D12GraphicsCommandList | 1 | gpu/draw.rs:2526 | E_NOTIMPL/no-op |
| `DrawInstanced` | ID3D12GraphicsCommandList | 6 | gpu/host.rs:6812, gpu/draw.rs:2430, gpu/draw.rs:3753 | E_NOTIMPL/no-op |
| `GetBuffer` | ID3D12ShaderReflectionVariable, IDXGISwapChain | 1 | gpu/host.rs:5543 | E_NOTIMPL/no-op |
| `GetCurrentBackBufferIndex` | IDXGISwapChain3 | 1 | gpu/host.rs:5542 | E_NOTIMPL/no-op |
| `IASetIndexBuffer` | ID3D12GraphicsCommandList | 1 | gpu/draw.rs:948 | E_NOTIMPL/no-op |
| `IASetPrimitiveTopology` | ID3D12GraphicsCommandList | 6 | gpu/host.rs:6811, gpu/draw.rs:576, gpu/draw.rs:3752 | E_NOTIMPL/no-op |
| `IASetVertexBuffers` | ID3D12GraphicsCommandList | 4 | gpu/draw.rs:932, gpu/draw.rs:2498, shader/emit/host.rs:1346 | E_NOTIMPL/no-op |
| `OMSetBlendFactor` | ID3D12GraphicsCommandList | 1 | gpu/draw.rs:601 | E_NOTIMPL/no-op |
| `OMSetDepthBounds` | ID3D12GraphicsCommandList1 | 1 | gpu/draw.rs:877 | E_NOTIMPL/no-op |
| `OMSetRenderTargets` | ID3D12GraphicsCommandList | 7 | gpu/host.rs:6798, gpu/draw.rs:796, gpu/draw.rs:3747 | E_NOTIMPL/no-op |
| `OMSetStencilRef` | ID3D12GraphicsCommandList | 1 | gpu/draw.rs:607 | E_NOTIMPL/no-op |
| `Present` | ID3D12SharingContract, IDXGISwapChain | 1 | gpu/host.rs:5602 | E_NOTIMPL/no-op |
| `RSSetScissorRects` | ID3D12GraphicsCommandList | 6 | gpu/host.rs:6805, gpu/draw.rs:595, gpu/draw.rs:3749 | E_NOTIMPL/no-op |
| `RSSetViewports` | ID3D12GraphicsCommandList | 6 | gpu/host.rs:6799, gpu/draw.rs:589, gpu/draw.rs:3748 | E_NOTIMPL/no-op |
| `ResizeBuffers` | IDXGISwapChain | 1 | gpu/host.rs:5522 | E_NOTIMPL/no-op |
| `ResolveSubresource` | ID3D12GraphicsCommandList | 1 | gpu/list.rs:1149 | E_NOTIMPL/no-op |
| `ResolveSubresourceRegion` | ID3D12GraphicsCommandList1 | 1 | gpu/list.rs:1158 | E_NOTIMPL/no-op |
| `SetGraphicsRoot32BitConstants` | ID3D12GraphicsCommandList | 3 | gpu/host.rs:6797, gpu/draw.rs:1190, shader/emit/host.rs:1313 | E_NOTIMPL/no-op |
| `SetGraphicsRootConstantBufferView` | ID3D12GraphicsCommandList | 3 | gpu/draw.rs:1231, shader/emit/memory_tests.rs:1850, shader/emit/memory_tests.rs:1851 | E_NOTIMPL/no-op |
| `SetGraphicsRootDescriptorTable` | ID3D12GraphicsCommandList | 1 | shader/emit/host.rs:1327 | E_NOTIMPL/no-op |
| `SetGraphicsRootShaderResourceView` | ID3D12GraphicsCommandList | 6 | gpu/draw.rs:1042, gpu/draw.rs:1215, gpu/draw.rs:1232 | E_NOTIMPL/no-op |
| `SetGraphicsRootSignature` | ID3D12GraphicsCommandList | 6 | gpu/host.rs:6795, gpu/draw.rs:998, gpu/draw.rs:3750 | E_NOTIMPL/no-op |
| `SetGraphicsRootUnorderedAccessView` | ID3D12GraphicsCommandList | 5 | gpu/host.rs:6796, gpu/draw.rs:1043, gpu/draw.rs:1233 | E_NOTIMPL/no-op |

### diag
| method | interface(s) | sites | first call sites (file:line) | status |
|---|---|--:|---|---|
| `ClearStoredMessages` | ID3D12InfoQueue, IDXGIInfoQueue | 1 | gpu/host.rs:2837 | stub |
| `EnableDebugLayer` | ID3D12Debug, ID3D12Debug1 | 1 | gpu/host.rs:305 | stub |
| `GetAutoBreadcrumbsOutput` | ID3D12DeviceRemovedExtendedData | 1 | gpu/host.rs:2721 | stub |
| `GetMessage` | ID3D12InfoQueue, IDXGIInfoQueue | 2 | gpu/host.rs:2821, gpu/host.rs:2826 | stub |
| `GetNumStoredMessages` | ID3D12InfoQueue, IDXGIInfoQueue | 1 | gpu/host.rs:2819 | stub |
| `GetPageFaultAllocationOutput1` | ID3D12DeviceRemovedExtendedData1 | 1 | gpu/host.rs:2685 | stub |
| `SetAutoBreadcrumbsEnablement` | ID3D12DeviceRemovedExtendedDataSettings | 1 | gpu/host.rs:333 | stub |
| `SetEnableGPUBasedValidation` | ID3D12Debug1, ID3D12Debug3 | 1 | gpu/host.rs:316 | stub |
| `SetGPUBasedValidationFlags` | ID3D12Debug2, ID3D12Debug3 | 1 | gpu/host.rs:319 | stub |
| `SetPageFaultEnablement` | ID3D12DeviceRemovedExtendedDataSettings | 1 | gpu/host.rs:334 | stub |

## 5. What is hard in M1

1. **`CreateComputePipelineState`**: DXIL goes to the back as is
   (`CREATE_COMPUTE_PIPELINE`); the conversion needs the Metal Shader Converter,
   so until the back has it the call answers `UNSUPPORTED` → `E_NOTIMPL`.
2. **The descriptor model**: D3D12 CPU/GPU handles are plain integers the
   application does arithmetic on; the front allocates handle space
   (`descriptor.rs`) and the back keeps the descriptor contents. CPU→GPU
   copy semantics (`CopyDescriptors*`) become proto `COPY_DESCRIPTORS` calls.
3. **Root signature bytes**: applications (and `victory-shader` tests) compare
   serialized blobs and DXBC digests; the front reproduces D3DMetal's bytes and its
   MD5 variant (`rootsig.rs`), verified byte-for-byte against the recorded golden.
4. **Barriers**: D3D12 resource states have no Metal counterpart; see section 6.
5. **`Map` of upload/readback buffers**: the back owns the memory; `Map` returns an
   address valid in the process (same address space as the bridge).
6. **`CheckFeatureSupport`** has ~40 structures; the answers are a table copied from
   the golden (`caps.rs`), Apple-GPU values on top.

## 6. Resource state → stage table

Stage bits are the cross-layer convention (proto `types::stage`): bit0 Vertex,
bit1 Fragment, bit2 Tile, bit3 Object, bit4 Mesh, bit5 Dispatch, bit6 Blit,
bit7 ResourceState; `SHADERS` = Vertex|Fragment|Object|Mesh|Dispatch.

| D3D12 state | stages |
|---|---|
| `VERTEX_AND_CONSTANT_BUFFER` | SHADERS |
| `INDEX_BUFFER` | Vertex |
| `RENDER_TARGET`, `DEPTH_WRITE`, `DEPTH_READ`, `PIXEL_SHADER_RESOURCE`, `RESOLVE_*`, `SHADING_RATE_SOURCE` | Fragment |
| `UNORDERED_ACCESS` | SHADERS |
| `NON_PIXEL_SHADER_RESOURCE` | Vertex|Object|Mesh|Dispatch |
| `STREAM_OUT` | Vertex |
| `INDIRECT_ARGUMENT` | Dispatch|Vertex|Object|Mesh |
| `COPY_DEST`, `COPY_SOURCE` | Blit |
| `RAYTRACING_ACCELERATION_STRUCTURE` | Dispatch|Blit |
| `COMMON` / `PRESENT` | none (implicit promotion and decay: no barrier) |

Rules (`state.rs`, `transition_barrier`): same state or either side COMMON → no
barrier; two read states in a row → no barrier; otherwise
`BARRIER{class: memory, visibility: Device, before: stages(old), after: stages(new)}`.
A UAV barrier is SHADERS→SHADERS; an aliasing barrier is ALL→ALL with the
`ResourceAlias` visibility bit. A split barrier (`BEGIN_ONLY`/`END_ONLY`) is
sent once, at END. Barriers are whole-resource; the tracker keeps the state per
subresource so that mismatched `StateBefore` is counted.

## 7. The recorder: the front writes proto

There is no front-only recorder protocol. `CommandList` (`metal/front/src/device.rs`)
keeps `Item`s — `Cmd(proto::Command)` or `RootConstants{…values}` (constants are
owned until the block is encoded) — and `Queue::execute` turns the lists of one
`ExecuteCommandLists` into **one** `submit` block (`encode_block`, `ListId` chosen
by the front, valid inside the block). Synchronous calls fill proto `XxxArgs`
blocks and go through `Link::call`. The back allocates all handles.
Tests use `MockLink` (`mock.rs`): it records submitted blocks and sync calls,
answers with fake handles and `gpu_va`s, can fail the next call of an op, and the
integration tests (`tests/device.rs`) compare the produced bytes with golden
proto blocks (`tests/golden/compute-block.vmps.txt`).

## 8. COM implementation choice

**`windows` 0.62.2 + `#[implement]`** (checked on crates.io 2026-11-27: `windows`
latest stable 0.62.2, 2025-10-06; `windows-sys` 0.61.2; the `windows-core` that
`windows` 0.62.2 depends on is 0.62.2 — crates.io shows `windows-core` 0.100.0
(2026-09-03) as newest, but `windows` 0.62.2 does not accept it, so the shell
pins `windows-core = "0.62.2"` to match what `#[implement]` expands to).

Reasons:
* `#[implement(ID3D12Device14)]` supports the whole interface family: the macro
  generates the vtables and `QueryInterface` for the interface and every base
  (one `*_Impl` trait per interface in `windows-0.62.2/src/Windows/Win32/Graphics/Direct3D12/mod.rs`,
  e.g. `ID3D12Device_Impl` at line ~9622). Listing the highest version gives QI
  over the whole hierarchy.
* The cost is that a type implementing `ID3D12Device14` must write every method of
  all 15 `ID3D12Device*_Impl` traits (~90). `tools/metal/gen-front-stubs.sh`
  generates them (`shell/stubs.rs`): hand-written `x_<Method>` where it exists,
  `E_NOTIMPL` / no-op otherwise. `--check` fails if the generated file is stale.
* The same crate and types the game side already uses (`victory-gpu` links
  `windows`), so GUIDs, `HRESULT` and struct layouts are not retyped.
* Rejected: hand-written vtables (every method of every interface by hand, with no
  generator to keep them in step with the `windows` definitions).

The core (`metal/front/src`, no `windows` types) holds all D3D12 semantics and is
tested natively on macOS; the shell is `cfg(windows)` and only does ABI,
reference counting, QueryInterface and argument decoding. d3d12.dll and dxgi.dll are
two `cdylib` crates (`metal/front/d3d12`, `metal/front/dxgi`) over one library
crate, so each dll exports only its own names.

## 9. Not done / known limits (M1)

* Real `Link`: the dlls use `MockLink` until the bridge installs a provider
  (`connect::set_provider`); `VICTORY_METAL_LINK=mock` forces the mock.
* Textures, placed resources, heaps, queries, indirect, graphics: see M2 / M3.
* `CreateRenderTargetView` / `CreateDepthStencilView` write a CPU-side record `(resource, mip, slices, format)` (no proto call; the graphics work reads it with `Device::target_view`).
* `ID3D12Fence::SetEventOnCompletion` with an event handle spawns a waiting thread
  per call.
* D3D12 hull/domain/geometry visibility in root signatures → `E_NOTIMPL`.

## 10. The bridge carries the proto

`metal/bridge` has two more call codes: `OP_PROTO_SUBMIT` (args `SubmitArgs{block, len}`)
and `OP_PROTO_CALL` (`CallArgs{op, args, len}`), both by address (same process).
`pe::WineUnixlib` implements `victory_metal_proto::Link` on top of its raw call
(`abi::RawLink`, the old M0 `Link`, which the M0 probe still uses). The proto
`Status` crosses the line as an `NTSTATUS` (`abi::status_to_nt` / `nt_to_status`:
0 is 0, an error is `0xE4D5_xxxx` holding `-status`; plain NT errors the unixlib
returns itself map to the nearest status, anything else to `INTERNAL`).

On the Mach-O side the entries hand the blocks to a replaceable executor:
`unix::set_executor(Arc<dyn Link>)` — the back's interpreter implements the proto
`Link` and installs itself there. Until it does, `NOP` answers `OK` and every
other call `NOT_INITIALIZED`. `unix::InProcess` is also a proto `Link` over the
same executor (native tests, Route B).

The front gets its `Link` from `connect::link()`: `VICTORY_METAL_LINK=mock` →
`MockLink`; else on Windows targets `WineUnixlib::load("victory_metal_bridge.dll")`.
Tests: native `cargo test -p victory-metal-bridge` (executor round trip through the
table entries, status mapping), and under Wine `tools/metal/test-bridge-proto.sh`.
Not done: the `SurfaceProvider` (Wine window → layer) and the back's interpreter.
