# 转译地图 + 覆盖矩阵（Mac/Wine 段 第 1 版）

日期 2026-11-27。分支 mac/translation-map（基于 mac/fix-wine-tests）。矩阵：`docs/mac/translation-matrix-mac.csv`（63 行，列见文末）。
范围：Rosetta 指令语义、加载期补丁、故障跟踪与异常、HLE（内存/线程/文件/DirectStorage），加上共享层（victory-gpu，接缝 A 之前）的骨架行（owner=shared）。Metal 后端、着色器翻译不在本文件。

## 0. 一眼看结论

- 覆盖率（见第 5 节）：按单元数，已用的 59 个单元里 proven 12（20.3%）、unproven 31、gap 16；按频次加权（仅指令类，静态普查，互不重叠口径）proven 5.2%、unproven 94.8%、gap 0.0%。
- 默认启动（Rosetta + 跟踪关）时 **1234 处 masked store 补丁一个都不打**（masked::choice(None,false)=None）。运行日志里看到“1234 patched”是因为 tools/mac/run.sh 和集成 runlt.sh 都 export 了 VICTORY_PATCH_MASKED=all。直接 `wine victory.exe` 或 tools/macos/victory.sh 启动则不打补丁。
- **0x140456fe2 不是标题地址**：标题映像加载在 0x6fffecfc0000（首选 0x140000000），这个 rip 在 victory.exe 自己里。metal-livetr 的 release exe（582a646f，不含 G6）里它是 `vpmaskmovd ymmword ptr [rcx], ymm4, ymm3`，位于 `victory_safe_merge_at`（crates/victory-gpu/src/safecopy.rs 的 global_asm）。错误字节 c4e25d8e19 eb04 c5fe7f19 f30fb8c0 与之吻合。标题自己的 RVA 0x456fe2 只是一条 `jne`，且不在补丁范围。G6（b7943221）把这段改成 Rosetta 下逐 dword 的普通 mov，本分支已含 G6；metal-livetr 那个 exe 是否含 G6 没有核对。日志出处：game-runs 20261127-d3dmetal-livetr-ab/stderr.log:3651、metal-livetr-r4:5140、r5:4156（num_insts=34 inst_index=9）。不改代码。
- Rosetta 实测（本机 macOS 27.2.1 / M5 Max，2026-11-27）：masked store ymm 跨页故障 → 进程中止（EmulateForward）；vcvtps2ph 写内存任何写故障都中止；clwb、AVX-512、clzero、monitorx → SIGILL；普通 256 位 store 跨页可恢复（与旧结论不同，Wine 下未验证）；失败的 lock cmpxchg 的 CF/OF/SF/AF/PF 与 x86 不同（ZF 正确）。

## 1. 转译链（每一跳）

| 跳 | 输入表示 | 输出表示 | 规则所在代码 | Xbox 上谁做 |
|---|---|---|---|---|
| H0 标题映像加载 | PE 映像 victory-image-b99f876b…（build 1.0.2.1），首选基址 0x140000000 | Wine 里映射在 0x6fffecfc0000，重定位后的 x86-64 代码 | crates/victory-loader（prepare.rs、tls.rs、threads.rs）；apps/victory/src/mac.rs 定下补丁计划 | 系统加载器，固定地址 |
| H1 加载期补丁 L1/L2/L3/L4 | x86-64 字节（masked store、256 位 store、clwb 循环、分辨率位点） | 改写后的 x86-64（5 字节 jmp 到每处的 pad） | crates/victory-loader/src/mac/{masked 1618 行, wide 580, clwb 152, resolution 815}.rs；xtask wide-stores | 无（Xbox 硬件直接执行） |
| H2 Rosetta 指令转译 | x86-64 机器码（含 AVX/AVX2/BMI/FMA/F16C，不含 AVX-512） | arm64 代码，TSO 由硬件辅助 | 系统组件（不可改）；行为用 tools/mac/rosetta-probe 实测 | Zen 2 CPU 原生执行 |
| H3 故障与异常 | SIGSEGV/Windows 异常 → 标题写故障、非法指令（clwb、AMD 专有指令） | 恢复执行、或模拟后继续 | crates/victory-loader/src/{crash.rs, console_insn.rs, write_sites.rs}；crates/victory-gpu/src/{watch.rs, titlewrite*, faultbook.rs, mac/rosetta.rs} | CPU/内存系统，无需跟踪 |
| H4 HLE：内存/线程/文件/存储 | 标题对 xmem.dll、kernel32、DStorage 的调用（587 个导入，68 个 DLL） | Wine/macOS 的虚拟内存、线程、文件；CPU 解压 | crates/victory-hle/src/modules/{xmem 2552 行, kernel, files, dstorage, mac}.rs | Xbox OS 与硬件解压块 |
| H5 共享层：D3D12.X → 宿主 D3D12 | 标题的 D3D12.X 调用（命令列表、根签名、描述符、栅栏、管线创建） | 宿主 D3D12 调用（D3DMetal），或 Metal 后端的接缝 A 入口 | crates/victory-gpu/src（device、root、descriptor、resource、draw、pipeline、indirect…），mac/{rosetta, safecopy, hlsl, world, adapter}.rs | 驱动 + GPU 命令处理器 |

接缝 A 之后（着色器翻译、Metal 后端）由 Metal 负责人填写。链类型：本文件所有行 chain=command。

## 2. 单元说明

每个单元对应矩阵一行：id、used（标题里用到没有）、freq（频次与来源）、tests（具体测试名；“无”即没有）、reference（hw 硬件事实、spec 文档、d3dmetal、none）、status（proven/unproven/gap）、disposition。下面只列每块的要点，细节在 CSV 的 notes。

### Rosetta 指令语义

| id | 单元 | 频次 | 来源 | status | disposition |
|---|---|---|---|---|---|
| mac.isa.maskmov_ymm_store_nofault | AVX2 masked store to memory (vpmaskmovd/q, vmaskmovps/pd; ymm 1124, xmm 6) when no fault occurs | 1,130 | static-census | proven | emulated |
| mac.isa.maskmov_ymm_store_fault_crossing | AVX2 masked 256-bit store that starts in accessible memory and faults in the next page (the VirtualProtect-gu… | 1,130 | static-census | gap | unsupported |
| mac.isa.vcvtps2ph_mem_fault | F16C vcvtps2ph store to memory when the destination page faults (any page, crossing or not) | 70 | static-census | gap | unsupported |
| mac.isa.vextract128_mem | vextractf128/vextracti128 store to memory (xmm half of ymm) | 33 | static-census | proven | translated |
| mac.isa.store256_plain | plain 256-bit vector stores (vmovups/vmovdqu/vmovaps ymm to memory), crossing into a protected page | 119,255 | static-census | proven | translated |
| mac.isa.clwb | clwb (cache-line write-back) executes | 8 | static-census | gap | unsupported |
| mac.isa.nontemporal_store | non-temporal stores (vmovntps/vmovntdq ymm, movnti, movntdq) | 53 | static-census | proven | translated |
| mac.isa.lock_cmpxchg_semantics | lock cmpxchg / cmpxchg16b: memory result and ZF | 28,361 | static-census | proven | translated |
| mac.isa.lock_cmpxchg_flags | flags after a FAILED lock cmpxchg (CF, AF, SF, OF, PF) | 28,305 | static-census | gap | emulated |
| mac.isa.lock_other_xchg | other locked RMW (xadd, or, and, inc, dec, bts, btr, add, sub, xor) and xchg with memory | 28,435 | static-census | proven | translated |
| mac.isa.cpuid_xgetbv | cpuid and xgetbv as the title sees them | 11 | static-census | unproven | emulated |
| mac.isa.rdtsc | rdtsc / rdtscp | 0 | static-census | unproven | emulated |
| mac.isa.bmi_lzcnt_popcnt | BMI1/BMI2, LZCNT, POPCNT | 76,588 | static-census | unproven | translated |
| mac.isa.fma_f16c | FMA3 (vfmadd/vfmsub/vfnmadd/vfnmsub) and F16C conversions (register forms) | 207,582 | static-census | unproven | translated |
| mac.isa.avx_general | all other VEX AVX/AVX2 instructions (ymm arithmetic, shuffles, gathers) | 3,278,635 | static-census | unproven | translated |
| mac.isa.amd_only | AMD-only instructions: clzero, monitorx, mwaitx, SSE4a insertq (extrq 0) | 33 | static-census | unproven | emulated |
| mac.isa.tso_ordering | x86 total-store-order memory model for lock-free code (pause 2,601, mfence 36, sfence 20, lfence 8) | 2,665 | static-census | unproven | translated |
| mac.isa.fault_resume_ordinary | synchronous page fault resume of ordinary stores, locked RMW, rep movs, movnt (all but the masked/vcvtps2ph c… | 未知 | unknown | proven | translated |
| mac.isa.page_size | page granularity seen by the title (4 KiB guest pages on 16 KiB host pages) | 未知 | unknown | unproven | translated |

### 加载期补丁

| id | 单元 | 频次 | 来源 | status | disposition |
|---|---|---|---|---|---|
| mac.loader.masked_patch | masked-store rewrite to plain stores (masked.rs): scans c4-VEX 0F38 8e/2e/2f stores + vcvtps2ph + vextract128… | 1,234 | log | unproven | translated |
| mac.loader.masked_default_off | masked-store patch NOT applied by default: needs tracking ON under Rosetta (stores_need_patching) or VICTORY_… | 1,234 | static-census | gap | stubbed |
| mac.loader.wide_patch | 256-bit store split into two 128-bit stores (wide.rs), sites from `cargo xtask wide-stores` | 117,413 | static-census | unproven | stubbed |
| mac.loader.clwb_nop | clwb write-back loops replaced by NOPs (only if exactly 8 loops are found) | 8 | log | unproven | stubbed |
| mac.loader.resolution | title output size site patches (2 sites) and VICTORY_OUTPUT / RENDER_SCALE / UPSCALER | 2 | log | proven | stubbed |

### 故障跟踪与异常

| id | 单元 | 频次 | 来源 | status | disposition |
|---|---|---|---|---|---|
| mac.fault.tracking_decision | fault tracking of title writes OFF by default under Rosetta (watch::writes returns None; callers copy memory … | 未知 | unknown | proven | stubbed |
| mac.fault.ntquery_class7 | NtQueryVirtualMemory class 7 (MemoryRegionInformationEx) used by watch::placeholder() | 未知 | unknown | gap | stubbed |
| mac.fault.xmem_no_write_watch | MEM_WRITE_WATCH left off the pools under Rosetta | 未知 | unknown | unproven | stubbed |
| mac.fault.files_overlapped_wait | overlapped ReadFile is waited for before the destination is guarded again (tracking on under Rosetta) | 未知 | unknown | unproven | emulated |
| mac.fault.titlewrite | decoder that emulates faulting title stores (what the store does to registers/flags/memory) | 未知 | unknown | gap | emulated |
| mac.fault.shadow_inplace | shadow copies of title memory for the GPU (shadow.rs) and in-place views (inplace.rs) | 未知 | unknown | unproven | emulated |
| mac.fault.safecopy_asm | victory_safe_put / victory_safe_merge stores (vpmaskmovd) and their scalar twins | 未知 | unknown | proven | translated |

### HLE 内存/线程/文件/DirectStorage

| id | 单元 | 频次 | 来源 | status | disposition |
|---|---|---|---|---|---|
| mac.hle.xmem_virtual_alloc | XMemVirtualAlloc -> VirtualAlloc2 with address requirements (4 GiB..2^40 or 2^47-n, low 4 GiB for 32-bit flag… | 16 | log | unproven | translated |
| mac.hle.low_address_commit | allocation returning NULL to the title (title writes 0 at image+0x2215c35 after __XMemAllocationHooks__) | 3 | log | gap | unsupported |
| mac.hle.commit_quota | commit-limit assumption: title budget (title_limit) from console::title_memory vs host commit | 未知 | unknown | unproven | stubbed |
| mac.hle.working_set_statistics | XMemGetWorkingSetStatistics returns limit/used for the title | 1 | static-census | unproven | stubbed |
| mac.hle.tools_memory | retail answers: no tools memory, transfers bounded | 2 | static-census | unproven | mapped |
| mac.hle.globalmemorystatus | GlobalMemoryStatusEx answered by Wine from the Mac host | 1 | static-census | unproven | emulated |
| mac.hle.cool_fox_residency | 32 GB residency budgets, fallback heaps, GPU pool back-off and deferred uploads under low commit | 未知 | unknown | unproven | stubbed |
| mac.hle.zero_copy_alias | zero-copy title buffers via mach_vm_remap (copy=FALSE) onto D3DMetal buffer pages | 未知 | unknown | gap | stubbed |
| mac.hle.suspendthread | SuspendThread only for the title's own threads (others return success without suspending) | 1 | static-census | unproven | translated |
| mac.hle.files | virtual drives G: and T: map title file paths onto host folders (rewrites CreateFileW/ReadFile/...) | 13 | static-census | unproven | translated |
| mac.hle.dstorage | DirectStorage queues/requests, zlib and BCPack decompression on the CPU | 79 | log | unproven | emulated |
| mac.hle.display_audio_misc | HLE stubs: HDR off (SDR present), screenshots fail, DSP convolution/transform outputs zeros, speech silent, e… | 4 | log | unproven | stubbed |

### 分辨率链（占位，细节待另一只手交回）

| id | 单元 | 频次 | 来源 | status | disposition |
|---|---|---|---|---|---|
| mac.res.output | title output size (swap chain / output resolution) | 1 | log | unproven | stubbed |
| mac.res.internal | title internal (render) resolution | 未知 | unknown | unproven | stubbed |
| mac.res.drs | dynamic resolution scaling driven by GPU frame time | 未知 | unknown | unproven | emulated |
| mac.res.fsr | in-game FSR upscaler (amd_fidelityfx_upscaler_xbox.dll + loader dll) | 未知 | unknown | unproven | emulated |

### 共享层骨架（owner=shared）

| id | 单元 | 频次 | 来源 | status | disposition |
|---|---|---|---|---|---|
| shared.device.create | D3D12XboxCreateDevice / device object, feature queries, queue creation | 1 | static-census | proven | translated |
| shared.root.signature | root signature / root description parse (console flags dropped) | 未知 | unknown | unproven | translated |
| shared.descriptor.heaps | descriptor heaps/views copied beside the title heaps (descriptor/host.rs) | 未知 | unknown | unproven | emulated |
| shared.resource.heaps_at_addresses | resources placed at title addresses (resource.rs, tiles.rs, texture.rs, shadow/inplace) | 未知 | unknown | unproven | emulated |
| shared.fence.wait | fence wait: blocking wait via event; blocking wait without event NOTIMPL (fence.rs 144-149) | 0 | log | unproven | stubbed |
| shared.device.feature_query | CheckFeatureSupport for feature ids not in {options, architecture, levels, format, multisample} | 0 | log | unproven | stubbed |
| shared.object.unknown_method | any vtable slot without an implementation (object.rs generic: "method not implemented", returns E_NOTIMPL) | 1 | log | gap | stubbed |
| shared.pipeline.unknown_creation_methods | title pipeline-creation methods at vtable slots 0x1c8 and 0x218 (title+0x2477578, title+0x24800d5) return E_N… | 18,432 | log | gap | unsupported |
| shared.pipeline.untranslated_shader | a shader that does not translate: its pipelines' draws are dropped | 1 | log | gap | unsupported |
| shared.draw.no_host_pipeline | draws/dispatches dropped because no host pipeline exists (being made, refused, or untranslated) | 1 | log | gap | stubbed |
| shared.draw.descriptor_drops | draws dropped for: no free descriptor slots, root UAV/SRV with no bound view heap, address-reading shader wit… | 0 | log | unproven | stubbed |
| shared.indirect.clamp_dropped | indirect execution dropped when the argument-clamp shader fails to build (HLSL needs SM 6.6 / InterlockedOr o… | 2 | log | proven | stubbed |
| shared.query.timestamps | timestamp queries (EndQuery/ResolveQueryData) used by the title for DRS and profiling | 未知 | unknown | gap | unsupported |
| shared.query.depth_bounds | depth bounds test reported supported (OPTIONS2 DepthBoundsTestSupported=true on D3DMetal) | 未知 | unknown | unproven | stubbed |
| shared.resource.typed_buffer_r32g32b32 | R32G32B32 typed buffers (D3DMetal: "R32G32B32 typed buffers not supported, using R32G32") | 未知 | unknown | gap | stubbed |
| shared.residency.make_resident | MakeResident/Evict ("Unsupported: MakeResident" once per process on D3DMetal) | 1 | log | gap | stubbed |

## 3. 用到但未证明（按频次）

used=yes 且 status≠proven。频次是静态普查次数或日志次数，两类不能直接相加。

| 频次 | 来源 | id | status | 缺什么 |
|---|---|---|---|---|
| 3,278,635 | static-census | mac.isa.avx_general | unproven | differential test vs a scalar model for the 50 most frequent VEX mnemonics (census isa-functions.tsv) |
| 207,582 | static-census | mac.isa.fma_f16c | unproven | bit-exact test of vfmadd*/vcvtps2ph/vcvtph2ps vs software model, random + edge values, 4 rounding modes |
| 117,413 | static-census | mac.loader.wide_patch | unproven | Windows exe probe of plain 256-bit store crossing a guarded page under Wine (VEH resume) |
| 76,588 | static-census | mac.isa.bmi_lzcnt_popcnt | unproven | random compare of each BMI op vs software model |
| 28,305 | static-census | mac.isa.lock_cmpxchg_flags | gap | probe test with the architectural flags; census of the next 3 instructions |
| 18,432 | log | shared.pipeline.unknown_creation_methods | gap | a decoder test over the logged descriptions (16 words) after the two methods are reverse engineered |
| 2,665 | static-census | mac.isa.tso_ordering | unproven | litmus tests (MP, SB, IRIW) 1e8 iterations under Rosetta+Wine |
| 1,234 | log | mac.loader.masked_patch | unproven | run the 4 Windows-only patch tests under Wine + a writer/reader race test |
| 1,234 | static-census | mac.loader.masked_default_off | gap | plan() unit test for the 4 combinations (Rosetta x tracking x VICTORY_PATCH_MASKED) in apps/victory mac.rs; log a line when nothing is patched |
| 1,130 | static-census | mac.isa.maskmov_ymm_store_fault_crossing | gap | Windows exe probe of masked-store crossing under Wine; after patching, a guarded-page store test with tracking on |
| 79 | log | mac.hle.dstorage | unproven | run the 19 dstorage tests under Wine; bytes-equal test per request kind |
| 70 | static-census | mac.isa.vcvtps2ph_mem_fault | gap | rewrite-vs-original test for vcvtps2ph store, all rounding modes |
| 33 | static-census | mac.isa.amd_only | unproven | trap-count log (VICTORY diagnostics) per frame + a unit test of the Insertq/mwaitx emulation |
| 16 | log | mac.hle.xmem_virtual_alloc | unproven | run xmem tests under Wine |
| 13 | static-census | mac.hle.files | unproven | run files tests under Wine |
| 11 | static-census | mac.isa.cpuid_xgetbv | unproven | disassemble the 9 cpuid sites; test run without ROSETTA_ADVERTISE_AVX |
| 8 | static-census | mac.isa.clwb | gap | See mac.loader.clwb_nop. No Arm equivalent is reachable from translated code; clflushopt/clflush do execute (probe) and could replace clwb loops if a flush were ever nee… |
| 8 | log | mac.loader.clwb_nop | unproven | counter "title pages the GPU reads without a CPU copy" must stay 0; run once with clflush replacing the NOPs and compare frames |
| 4 | log | mac.hle.display_audio_misc | unproven | one test per stub: HDR query, screenshot, DSP command zeros, speech, package list |
| 3 | log | mac.hle.low_address_commit | gap | log every allocator failure (hook result) + a test that forces XMemVirtualAlloc failure and checks the title-visible NULL path |
| 2 | static-census | mac.hle.tools_memory | unproven | Risk: None (faithful to retail). |
| 1 | static-census | mac.hle.working_set_statistics | unproven | test with a fake title that allocates then reads the statistics: expects its own heaps only |
| 1 | static-census | mac.hle.globalmemorystatus | unproven | test the override returns the console budget fields |
| 1 | static-census | mac.hle.suspendthread | unproven | Windows exe test: suspend title-image thread vs foreign thread |
| 1 | log | shared.object.unknown_method | gap | identify and implement queue slot 0xb8; test every vtable slot answers or logs |
| 1 | log | shared.pipeline.untranslated_shader | gap | translate-all test of the title container with wave64->wave32 lowering (shader crate) |
| 1 | log | shared.draw.no_host_pipeline | gap | assert "dropped 0" in the 5 s summary for a scripted opening run |
| 1 | log | shared.residency.make_resident | gap | residency test on the Metal backend |
| 未知 | unknown | mac.isa.page_size | unproven | 4 KiB mprotect probe under Rosetta and Wine |
| 未知 | unknown | mac.fault.ntquery_class7 | gap | Wine test: NtQueryVirtualMemory class 7 on a placeholder (then the 3 failing watch tests become the regression) |
| 未知 | unknown | mac.fault.xmem_no_write_watch | unproven | test pool flags per mode |
| 未知 | unknown | mac.fault.files_overlapped_wait | unproven | overlapped read into guarded memory under Wine |
| 未知 | unknown | mac.fault.titlewrite | gap | fix the decoder test to compare ZF only for failed cmpxchg under Rosetta |
| 未知 | unknown | mac.fault.shadow_inplace | unproven | run the warp-reference tests against a real second adapter |
| 未知 | unknown | mac.hle.commit_quota | unproven | test: title-like allocation sequence past the 13.5 GB budget must be refused as on Xbox |
| 未知 | unknown | mac.hle.cool_fox_residency | unproven | force-low-commit env in a unit test so the back-off path runs |
| 未知 | unknown | mac.hle.zero_copy_alias | gap | probe: mach_vm_remap(copy=FALSE) of a title VirtualAlloc range onto an MTLBuffer under Rosetta+Wine; write on one side, read on the other |
| 未知 | unknown | shared.root.signature | unproven | Wine 1 failure: D3DMetal accepts a blob the test expects to be rejected (PATCHES.md). / Risk: Console-only root flags are dropped. |
| 未知 | unknown | shared.descriptor.heaps | unproven | fix the 2 failing descriptor::host tests under D3DMetal |
| 未知 | unknown | shared.resource.heaps_at_addresses | unproven | See mac.hle.zero_copy_alias. |
| 0 | log | shared.fence.wait | unproven | unit test: blocking wait without event completes |
| 0 | log | shared.device.feature_query | unproven | unit test over every D3D12_FEATURE id |
| 0 | log | shared.draw.descriptor_drops | unproven | stress test of descriptor slots |
| 未知 | unknown | shared.query.timestamps | gap | query test: EndQuery/ResolveQueryData returns non-zero increasing time |
| 未知 | unknown | shared.query.depth_bounds | unproven | draw with depth bounds vs reference image |
| 未知 | unknown | shared.resource.typed_buffer_r32g32b32 | gap | typed buffer R32G32B32 readback test |

## 4. 偏差清单（emulated + stubbed + unsupported，按 频次×严重度 排序）

严重度 1 无可见后果、2 性能或罕见路径、3 逻辑或画面可能错、4 内容缺失或明显错、5 进程崩溃。排序分 = 频次（未知记 1）× 严重度。可行性 高/中/低/待核实。外部事实的来源和查证日期在 CSV 的 reference_detail 和 notes（日期均 2026-11-27）。

| # | id | 处置 | 频次 | 严重度 | 偏差 | 后果 | 还原方案 | 代价 | 可行性 |
|---|---|---|---|---|---|---|---|---|---|
| 1 | mac.loader.wide_patch | stubbed | 117,413 | 1 | n/a | Off unless VICTORY_PATCH_WIDE=<list> is set (no script sets it, no list is generated by the repo): effectively not applied. Probe: plain 256-bit stores resume on macOS 27.2.1, so it is not needed there. | Keep disabled; if a later macOS regresses, generate the list at first start (xtask logic as a loader pass) and apply. Cost: 117k trampolines (~2 MB code). | 小（数十行/一个测试） | 高 |
| 2 | shared.pipeline.unknown_creation_methods | unsupported | 18,432 | 4 | Console D3D12 has two pipeline-creation methods the PC API lacks. | Title goes without those pipelines silently (what they draw is unknown): whole draws and dispatches missing, maybe visible as absent geometry, or the engine uses a fallback (not proven). | Decode the logged descriptions (first words point at stream tokens: the dump in the log) and implement the two methods through CreatePipelineState from a stream; feasibility medium. This is the largest known unsupported call count in the shared layer. | 中（模块级改动，需验证） | 中 |
| 3 | mac.isa.lock_cmpxchg_flags | emulated | 28,305 | 1 | x86 sets all arithmetic flags as CMP accumulator,destination. | Operands reversed in the flag computation on a miss: CF/OF/SF/AF/PF wrong, ZF right. Consumer code reading only ZF is unaffected. | Exposure is nil for the title: of 28,305 sites, 19,802 are followed directly by a ZF-only reader (je/jne/sete/cmov), 0 by a reader of CF/SF/OF, 8,503 by a non-reader. Fix the decoder test (titlewrite) to expect Rosetta flags or compare ZF only; if a site ever… | 小（数十行/一个测试） | 高 |
| 4 | mac.isa.maskmov_ymm_store_fault_crossing | unsupported | 1,130 | 5 | Zen 2 / x86 resumes the instruction after the fault handler opens the page (what Windows VirtualProtect guard… | Fatal: Rosetta aborts the process (no resume). Same fatal text as the 0x140456fe2 hang. | Highest fidelity = never let a masked store fault: (1) loader rewrite to plain stores (L1, 1234 sites) whenever tracking is on; (2) our own asm (safecopy, fixed by G6); (3) keep fault tracking off by default. A true fix needs Apple (Feedback Assistant report;… | 中（模块级改动，需验证） | 中 |
| 5 | mac.loader.masked_default_off | stubbed | 1,234 | 4 | n/a | CONFIRMS the reported suspicion for the product code: with Rosetta and fault tracking off (the default) choice(None, needed=false) is None, so 0 of 1234 masked stores are patched; the log line "patched" in the runs appears only b… | Make the decision explicit and logged: patch whenever Rosetta (cost: 1234 jumps, no runtime cost when no fault); apply the 8 extra RVAs together with "all"; add a unit test for plan() covering the 4 combinations. Wide patch L2 is likewise never on by default … | 小（数十行/一个测试） | 高 |
| 6 | mac.isa.maskmov_ymm_store_nofault | emulated | 1,130 | 1 | Zen 2 executes VEX masked stores natively; page fault of a masked-off lane is suppressed by the CPU. | None seen: data stored equals x86 for full, lane-0 and zero masks; zero mask into an inaccessible page raises no fault (as x86). | Nothing to restore; keep tools/mac/rosetta-probe.sh as the regression after every macOS update. | 中（模块级改动，需验证） | 中 |
| 7 | mac.isa.vcvtps2ph_mem_fault | unsupported | 70 | 5 | Zen 2 resumes after the fault. | Fatal on ANY write fault of the store (stronger than the masked stores, which only fail when crossing). | Same as masked stores: load-time rewrite (L1 includes vcvtps2ph, 70 sites) when tracking is on. For fidelity of the converted half-floats the rewrite must keep rounding control imm8 and MXCSR behaviour: add a test comparing rewrite vs original over random flo… | 中（模块级改动，需验证） | 中 |
| 8 | mac.hle.dstorage | emulated | 79 | 1 | Xbox hardware decompression block (zlib/BCPack) feeding memory directly. | Software decompression on CPU threads (perf), requests into guarded memory retried; behaviour identical in bytes (tests). | Metal fast-resource-loading (MTLIOCommandQueue) offers zlib/LZFSE/LZ4/LZMA/LZBitmap (SDK MTLDevice.h, checked 2026-11-27) but not BCPack: keep CPU BCPack, optionally GPU/IO queue for zlib. | 中（模块级改动，需验证） | 中 |
| 9 | mac.isa.amd_only | emulated | 33 | 2 | Zen 2 executes them (clzero zeroes a cache line; monitorx/mwaitx wait on an address). | clzero loop -> plain zero stores (patched into a jump after first fault, equal result); monitorx -> 3-byte no-op, mwaitx -> pause (the wait loop re-reads its value: busy-wait, more CPU, same logic); insertq -> emulated per fault … | Real mwaitx wake-up semantics could be given by a futex-style park on the monitored address in the emulator (Apple os_sync_wait_on_address): lower power, same logic. Insertq: patch to the equivalent pinsr/shift sequence at load. | 小（数十行/一个测试） | 高 |
| 10 | mac.isa.cpuid_xgetbv | emulated | 11 | 3 | Zen 2: AuthenticAMD, family 17h, AVX2/BMI/FMA/F16C/CLWB/CLFLUSHOPT/CLZERO/MONITORX advertised. | Vendor Intel, no CLWB/CLFLUSHOPT/CLZERO/MONITORX bits, AVX only with the env var. A title branch on vendor or family would take a different path than on Xbox. | Patch the 9 cpuid sites (3 functions) to present Zen 2 values, or confirm by disassembly that they only test feature bits. Both runs scripts (tools/mac/run.sh, tools/macos/victory.sh) set ROSETTA_ADVERTISE_AVX=1; a bare launch does not. | 小（数十行/一个测试） | 高 |
| 11 | mac.isa.clwb | unsupported | 8 | 2 | Zen 2 CLWB writes a dirty cache line back to memory without evicting it; used so the GPU (not cache-coherent … | Instruction does not exist under Rosetta: SIGILL. Windows line emulates by exception (one per loop, ~300 us through Wine). | See mac.loader.clwb_nop. No Arm equivalent is reachable from translated code; clflushopt/clflush do execute (probe) and could replace clwb loops if a flush were ever needed (evicts the line, so slower but correct). | 中（模块级改动，需验证） | 中 |
| 12 | mac.loader.clwb_nop | stubbed | 8 | 2 | CLWB pushes dirty lines to memory so non-coherent consumers (GPU, DMA) see CPU writes. | Write-back dropped. Safe on this path because all title memory is read by our own CPU code (copies for the GPU: unified memory, CPU-coherent). Would break if the GPU read title memory without a CPU copy (zero-copy) and CPU caches… | Today: keep NOP. If zero-copy is ever built (mach_vm_remap / MTLBuffer no-copy on the title pages), replace by a real flush: clflush is available under Rosetta (probe) or rely on Apple silicon CPU/GPU coherency for shared memory (Apple docs: unified memory; 待… | 小（数十行/一个测试） | 高 |
| 13 | mac.hle.low_address_commit | unsupported | 3 | 5 | Fixed Xbox address map with enough commit. | Title gets a null from one of its own allocators (not through XMemVirtualAlloc) and dies on purpose (mov [0],0; int3). | Instrument first (log every allocator failure site incl. hooks result); then port #398 low-address guards if the failing request is a 32-bit-flag one, or raise Wine commit/reservation. Feasibility: medium (cause unknown). | 中（模块级改动，需验证） | 中 |
| 14 | mac.hle.display_audio_misc | stubbed | 4 | 3 | Console OS/APU/DSP hardware does these. | HDR never output; DSP effects silent (zeros); no TTS; package queries optimistic; no haptic triggers. | Per item: HDR via CAMetalLayer EDR (Metal backend); DSP convolution/FFT in software (the code says it needs the coefficient format); speech via AVSpeechSynthesizer; gameinput haptics via GCController haptics (CoreHaptics, 待核实 trigger-motor support). | 中（模块级改动，需验证） | 中 |
| 15 | shared.indirect.clamp_dropped | stubbed | 2 | 4 | Hardware ExecuteIndirect clamps in the command processor. | Indirect executions dropped = the world load never ends (hlsl.rs comment). | G4 (in this branch) raises the shader model; confirm with a log that the line is absent after G4. | 小（数十行/一个测试） | 高 |
| 16 | mac.loader.resolution | stubbed | 2 | 3 | Title picks output size, internal resolution, DRS and in-game FSR itself. | Output forced to 1280x720 on a Mac (see mac.res.*). | See mac.res.output. | 小（数十行/一个测试） | 高 |
| 17 | mac.res.output | stubbed | 1 | 4 | Title renders internal resolution then outputs at the console output size (1080p/1440p/2160p per user setting… | Output is changed to 1280x720, inconsistent with the Xbox internal resolution + DRS + in-game FSR mechanism; effect to be determined. | Fidelity phase: run the Xbox mechanism as is (internal resolution, DRS and the game's own FSR, amd_fidelityfx_upscaler_xbox.dll, all through translated shaders, no replacement). MetalFX is a performance-phase candidate only. | 待核实 | 待核实 |
| 18 | mac.res.internal | stubbed | 未知 | 3 | Title chooses internal resolution (and render scale) per mode. | VICTORY_RENDER_SCALE default off: internal resolution follows the title; effect of the 720p output on it is to be determined. | Same as mac.res.output. | 待核实 | 待核实 |
| 19 | mac.res.drs | emulated | 未知 | 3 | Title adapts internal resolution from GPU timing (Xbox GPU timestamps). | GPU timestamps are 0 on D3DMetal (EndQuery/ResolveQueryData unsupported), so the title's DRS input differs from Xbox; effect to be determined. | Same as mac.res.output; needs working timestamp queries on the Metal backend (see shared.query.timestamps). | 待核实 | 待核实 |
| 20 | mac.res.fsr | emulated | 未知 | 3 | Title calls the Xbox FidelityFX upscaler DLL on the GPU. | VICTORY_UPSCALER default off; behaviour of the shipped DLL under the loader is to be determined. | Same as mac.res.output (shaders translated, no replacement). | 待核实 | 待核实 |
| 21 | mac.fault.tracking_decision | stubbed | 未知 | 3 | Title memory is unified memory the GPU reads directly; no tracking exists. | Whole-block copies for every submission instead of only written pages: 129 GB/frame estimated once the world streams (masked.rs header, #50); in logs the opening scene copies 0.1-3.1 MB/frame with tracking off. | Make tracking safe so it can be on: needs every faulting instruction class resumable (masked stores L1 + vcvtps2ph L1, our asm G6, SuspendThread H1, overlapped ReadFile A7); then default on. Alternative without faults: GPU reads title pages in place (no copy)… | 中（模块级改动，需验证） | 中 |
| 22 | mac.hle.commit_quota | stubbed | 未知 | 3 | Console OS enforces a 13.5 GB title budget and fails commits past it. | Our budget is a number we give the title; the host does not enforce it, so over-commit is not refused (and swap/compression may stall) instead of failing as on Xbox. | Account title commits in the HLE (sum of committed MEM_COMMIT) and refuse past the console budget, as the Xbox OS does; test with a title-like allocation sequence. | 小（数十行/一个测试） | 高 |
| 23 | mac.hle.working_set_statistics | stubbed | 1 | 3 | Console OS reports real title use, per bandwidth class. | "used" is the whole process PagefileUsage (our layer and the card included, 27 GB vs a 13.5 GB limit in run 60 per code comment); GPU-optimal use reads 0. | Count the title's own heaps/mapped pages and per class in the XMem hooks; return those. | 小（数十行/一个测试） | 高 |
| 24 | mac.hle.globalmemorystatus | emulated | 1 | 3 | Console OS reports its fixed memory (title budget). | Reports the Mac host RAM/commit (e.g. >100 GB on this machine) so any pool sizing from it differs from Xbox; the 32 GB-RAM optimisation line (cool_fox) keys on host RAM too. | Override in victory-hle kernel overrides (host_lookups_by_name_get_our_overrides) with the console budget; test the structure fields. | 小（数十行/一个测试） | 高 |
| 25 | shared.descriptor.heaps | emulated | 未知 | 3 | Descriptors are raw 32-byte words the GPU reads from memory. | Host views made from the title's raw descriptors and kept in sync: raw words are not visible to the host driver. | Restore through the Metal backend argument-buffer model (descriptor words decoded to MTLResourceID); see Metal rows. | 中（模块级改动，需验证） | 中 |
| 26 | shared.object.unknown_method | stubbed | 1 | 3 | All methods exist. | E_NOTIMPL, which the title reads as a lost device in some paths (device.rs comment, VIctory 13185f5). r5 log: command queue slot 0xb8 called from title+0x24900a5, once (queue method). | Implement slot 0xb8 of the queue (identify via D3D12 vtable: likely a queue method added in a newer interface). | 小（数十行/一个测试） | 高 |
| 27 | shared.pipeline.untranslated_shader | unsupported | 1 | 3 | GPU runs any shader. | Draws or dispatches of that pipeline vanish (1 compute shader in the opening scene). | Lower wave64 ops to wave32 (shader owner; 18 permlane/quad failing tests listed in PATCHES.md). | 中（模块级改动，需验证） | 中 |
| 28 | shared.draw.no_host_pipeline | stubbed | 1 | 3 | n/a | Draws lost until the pipeline is made; refused ones are lost for good. | Metal side: implement attributeAtVertex (MSC), raise mesh limit by splitting meshlets; shared side: wait for the pipeline instead of dropping the first use (decision 0007 note). | 中（模块级改动，需验证） | 中 |
| 29 | shared.query.timestamps | unsupported | 未知 | 3 | GPU timestamps in nanoseconds. | Title sees time 0: DRS/profiling input wrong (see mac.res.drs). | Metal backend: MTLCounterSampleBuffer (timestamp sampling at stage boundary, Apple GPU limits 待核实); or synthesise timestamps from the frame timeline. | 中（模块级改动，需验证） | 中 |
| 30 | shared.resource.typed_buffer_r32g32b32 | stubbed | 未知 | 3 | Native 96-bit typed buffer. | Third component dropped: wrong vertex/texel data for that format. | Metal: three R32 views or a structured buffer; owner Metal. | 中（模块级改动，需验证） | 中 |
| 31 | mac.fault.ntquery_class7 | stubbed | 未知 | 2 | Windows kernel answers class 7 (region size/placeholder flags). | Wine returns an invalid-info-class status: placeholder() is never true, so memory that is a placeholder is not recognised (only matters with VICTORY_FAULT_TRACKING=1). | High fidelity and cheap: our own registry knows every placeholder we create (xmem.rs placeholder branch), answer from it instead of asking the kernel; or carry a 20-line Wine patch (tools/macos/patches/) adding class 7 to ntdll/unix/virtual.c. | 小（数十行/一个测试） | 高 |
| 32 | mac.fault.xmem_no_write_watch | stubbed | 未知 | 2 | GPU reads memory directly. | No kernel write-watch: written pages unknown, so whole copies (same cost as tracking off). Wine emulates write watch with page protection, which hits Rosetta unresumable faults. | With tracking restored, turn on again; test: pool alloc has/hasn't MEM_WRITE_WATCH per mode. | 中（模块级改动，需验证） | 中 |
| 33 | mac.fault.files_overlapped_wait | emulated | 未知 | 2 | Async read completes into memory the GPU may read. | The call blocks until the read ends (only with tracking on): async IO serialised, slower loads; avoids a kernel write into a guarded page which hangs the process under Wine/Rosetta (#50). | Restore async by reading into a bounce buffer and copying after completion (no kernel write to guarded pages); test with guarded destination under Wine. | 中（模块级改动，需验证） | 中 |
| 34 | mac.fault.shadow_inplace | emulated | 未知 | 2 | GPU reads title memory directly. | Copy-in/copy-out through shadows; D3DMetal refuses OpenExistingHeapFromAddress (alias.rs doc), so no in-place on the Mac host path. | See mac.hle.zero_copy_alias. | 大 | 低 |
| 35 | mac.hle.cool_fox_residency | stubbed | 未知 | 2 | Xbox has unified GDDR6 with no residency management. | D3DMetal does not implement MakeResident (stub), so residency control is a no-op on that host; budgets assume a PC GPU with separate VRAM. Back-off branch unexercised. | On the Metal backend use MTLResidencySet (Metal 3+/4; 待核实 for macOS 27) to implement the budget for real; on D3DMetal nothing can be done. Add a forced-commit-limit test (VICTORY env) to exercise the back-off. | 中（模块级改动，需验证） | 中 |
| 36 | mac.hle.zero_copy_alias | stubbed | 未知 | 2 | Title memory IS the GPU memory. | In mac/livetr there is NO zero-copy: every buffer the GPU needs is shadowed and copied (shadow.rs), copy-back after writes. The title's GPU writes into its memory are landed by copying (land.rs), not shared. | Port alias.rs (x86_64 raw mach_msg2_trap path, D3DMetal only) as a Mac-gated module; or on the native Metal backend map title pages with newBufferWithBytesNoCopy (page-aligned, 待核实 constraint with Wine mappings). Cost: mach MIG traps from Rosetta, EXC_GUARD o… | 中（模块级改动，需验证） | 中 |
| 37 | shared.resource.heaps_at_addresses | emulated | 未知 | 2 | Resources live at fixed GPU virtual addresses in title memory. | Copies and shadows instead of aliasing; see mac.hle.zero_copy_alias. | See mac.hle.zero_copy_alias. | 大 | 低 |
| 38 | shared.query.depth_bounds | stubbed | 未知 | 2 | Hardware depth bounds. | Report says supported but Metal has no depth-bounds test: the draw state is ignored (RDNA2-specific; Metal owner). | Metal: emulate in the fragment shader (discard) or via pixel shader depth read; cost medium. | 中（模块级改动，需验证） | 中 |
| 39 | shared.residency.make_resident | stubbed | 1 | 2 | No residency on console. | No-op: budgets enforce nothing (see mac.hle.cool_fox_residency). | MTLResidencySet in the Metal backend. | 中（模块级改动，需验证） | 中 |
| 40 | mac.fault.titlewrite | emulated | 未知 | 1 | No emulation: stores simply run. | Tracking-dependent; with tracking off nothing runs through it ("no fault or hang seen" in runs). | Fix test: compare ZF/hit path only for lock cmpxchg under Rosetta (see mac.isa.lock_cmpxchg_flags); run the rest. | 小（数十行/一个测试） | 高 |
| 41 | mac.isa.rdtsc | emulated | 0 | 1 | Zen 2 TSC. | Not used by the title (0 sites); timing is QueryPerformanceCounter (263 call sites + 22 address-taken). | Not used by the title (0 sites); nothing to restore. If a later build uses it, rdtsc would need a QPC-based emulation trap. | 小（数十行/一个测试） | 高 |
| 42 | shared.fence.wait | stubbed | 0 | 2 | Fence wait is a GPU hardware semaphore wait. | Blocking CPU wait with no event returns E_NOTIMPL (log "blocking fence wait not implemented": 0 in all runs). | Implement with an internal event (small): feasibility high. | 小（数十行/一个测试） | 高 |
| 43 | shared.device.feature_query | stubbed | 0 | 2 | Driver answers every feature. | E_NOTIMPL for the rest; log "device feature query not implemented": 0 in ab/r5. | Answer additional features from the host device (options1..21, shader model, GPU VA); feasibility high. | 小（数十行/一个测试） | 高 |
| 44 | shared.draw.descriptor_drops | stubbed | 0 | 3 | n/a | Draw dropped silently after one log line. | Raise slots dynamically; feasibility high; no occurrence yet. | 小（数十行/一个测试） | 高 |

### 4.1 stub 清单（disposition=stubbed，按频次；每项后果与要补的测试）

Metal 侧 matrix.sh 合并时也按 freq 排序这些行。

| 频次 | 来源 | id | 可能后果 | 打通要补的测试 |
|---|---|---|---|---|
| 117,413 | static-census | mac.loader.wide_patch | Off unless VICTORY_PATCH_WIDE=<list> is set (no script sets it, no list is generated by the repo): effectively not applied. Probe: plain 256-bit stores resume on macOS 27.2.1, so it is not needed there. | Windows exe probe of plain 256-bit store crossing a guarded page under Wine (VEH resume) |
| 1,234 | static-census | mac.loader.masked_default_off | CONFIRMS the reported suspicion for the product code: with Rosetta and fault tracking off (the default) choice(None, needed=false) is None, so 0 of 1234 masked stores are patched; the log line "patched" in the runs appears only b… | plan() unit test for the 4 combinations (Rosetta x tracking x VICTORY_PATCH_MASKED) in apps/victory mac.rs; log a line when nothing is patched |
| 8 | log | mac.loader.clwb_nop | Write-back dropped. Safe on this path because all title memory is read by our own CPU code (copies for the GPU: unified memory, CPU-coherent). Would break if the GPU read title memory without a CPU copy (zero-copy) and CPU caches… | counter "title pages the GPU reads without a CPU copy" must stay 0; run once with clflush replacing the NOPs and compare frames |
| 4 | log | mac.hle.display_audio_misc | HDR never output; DSP effects silent (zeros); no TTS; package queries optimistic; no haptic triggers. | one test per stub: HDR query, screenshot, DSP command zeros, speech, package list |
| 2 | log | mac.loader.resolution | Output forced to 1280x720 on a Mac (see mac.res.*). | 见 restore_plan |
| 2 | log | shared.indirect.clamp_dropped | Indirect executions dropped = the world load never ends (hlsl.rs comment). | assert after G4 the clamp shader builds under D3DMetal DXC |
| 1 | log | mac.res.output | Output is changed to 1280x720, inconsistent with the Xbox internal resolution + DRS + in-game FSR mechanism; effect to be determined. | frame-hash and timing compare with VICTORY_OUTPUT unset (title default) once the resolution investigation reports |
| 1 | static-census | mac.hle.working_set_statistics | "used" is the whole process PagefileUsage (our layer and the card included, 27 GB vs a 13.5 GB limit in run 60 per code comment); GPU-optimal use reads 0. | test with a fake title that allocates then reads the statistics: expects its own heaps only |
| 1 | log | shared.object.unknown_method | E_NOTIMPL, which the title reads as a lost device in some paths (device.rs comment, VIctory 13185f5). r5 log: command queue slot 0xb8 called from title+0x24900a5, once (queue method). | identify and implement queue slot 0xb8; test every vtable slot answers or logs |
| 1 | log | shared.draw.no_host_pipeline | Draws lost until the pipeline is made; refused ones are lost for good. | assert "dropped 0" in the 5 s summary for a scripted opening run |
| 1 | log | shared.residency.make_resident | No-op: budgets enforce nothing (see mac.hle.cool_fox_residency). | residency test on the Metal backend |
| 未知 | unknown | mac.res.internal | VICTORY_RENDER_SCALE default off: internal resolution follows the title; effect of the 720p output on it is to be determined. | 见 restore_plan |
| 未知 | unknown | mac.fault.tracking_decision | Whole-block copies for every submission instead of only written pages: 129 GB/frame estimated once the world streams (masked.rs header, #50); in logs the opening scene copies 0.1-3.1 MB/frame with tracking off. | tools/mac/rosetta-probe.sh as a Windows exe under Wine (guarded-page resume for every store class) then a run with FAULT_TRACKING=1 and a frame-hash compare vs tracking … |
| 未知 | unknown | mac.fault.ntquery_class7 | Wine returns an invalid-info-class status: placeholder() is never true, so memory that is a placeholder is not recognised (only matters with VICTORY_FAULT_TRACKING=1). | Wine test: NtQueryVirtualMemory class 7 on a placeholder (then the 3 failing watch tests become the regression) |
| 未知 | unknown | mac.fault.xmem_no_write_watch | No kernel write-watch: written pages unknown, so whole copies (same cost as tracking off). Wine emulates write watch with page protection, which hits Rosetta unresumable faults. | test pool flags per mode |
| 未知 | unknown | mac.hle.commit_quota | Our budget is a number we give the title; the host does not enforce it, so over-commit is not refused (and swap/compression may stall) instead of failing as on Xbox. | test: title-like allocation sequence past the 13.5 GB budget must be refused as on Xbox |
| 未知 | unknown | mac.hle.cool_fox_residency | D3DMetal does not implement MakeResident (stub), so residency control is a no-op on that host; budgets assume a PC GPU with separate VRAM. Back-off branch unexercised. | force-low-commit env in a unit test so the back-off path runs |
| 未知 | unknown | mac.hle.zero_copy_alias | In mac/livetr there is NO zero-copy: every buffer the GPU needs is shadowed and copied (shadow.rs), copy-back after writes. The title's GPU writes into its memory are landed by copying (land.rs), not shared. | probe: mach_vm_remap(copy=FALSE) of a title VirtualAlloc range onto an MTLBuffer under Rosetta+Wine; write on one side, read on the other |
| 0 | log | shared.fence.wait | Blocking CPU wait with no event returns E_NOTIMPL (log "blocking fence wait not implemented": 0 in all runs). | unit test: blocking wait without event completes |
| 0 | log | shared.device.feature_query | E_NOTIMPL for the rest; log "device feature query not implemented": 0 in ab/r5. | unit test over every D3D12_FEATURE id |
| 0 | log | shared.draw.descriptor_drops | Draw dropped silently after one log line. | stress test of descriptor slots |
| 未知 | unknown | shared.query.depth_bounds | Report says supported but Metal has no depth-bounds test: the draw state is ignored (RDNA2-specific; Metal owner). | draw with depth bounds vs reference image |
| 未知 | unknown | shared.resource.typed_buffer_r32g32b32 | Third component dropped: wrong vertex/texel data for that format. | typed buffer R32G32B32 readback test |

## 5. 覆盖率

口径：单元 = 矩阵里 used=yes 的行。proven = 有硬件事实或规范依据且有测试或实测；unproven = 用到但缺证据；gap = 已知缺口。

### 5.1 按单元数

| 范围 | 单元数 | proven | unproven | gap |
|---|---|---|---|---|
| 全部 used=yes | 59 | 12 (20.3%) | 31 (52.5%) | 16 (27.1%) |
| 指令类（mac.isa.*） | 18 | 7 (38.9%) | 7 | 4 |
| 其余（loader/fault/hle/res/shared） | 41 | 5 (12.2%) | 24 | 12 |

disposition 分布（全部 63 行）：emulated 14，mapped 1，stubbed 23，translated 18，unsupported 7。

### 5.2 按频次加权（指令类，静态普查，每条指令只算一次）

为避免重复计数，排除了和别的行同一批位点的 mac.isa.maskmov_ymm_store_nofault、mac.isa.lock_cmpxchg_flags、mac.isa.rdtsc（0 次）；mac.isa.avx_general 的权重取 VEX 总数扣掉 FMA/F16C、普通 256 位 store、masked store、vextract、非临时 store 后的剩余（2,950,582）。频次未知的单元（mac.isa.fault_resume_ordinary、mac.isa.page_size）不进分母。

| 状态 | 加权频次 | 占比 |
|---|---|---|
| proven | 176,137 | 5.2% |
| unproven | 3,237,461 | 94.8% |
| gap | 1,208 | 0.0% |
| 合计 | 3,414,806 | 100% |

说明：加权结果被 unproven 的 avx_general（2,950,582，Rosetta 对 AVX/AVX2 的转译没有逐条差分测试）主导。把它拿掉，其余指令类的 proven 占 37.9%。HLE 和加载补丁的频次是调用数或位点数，单位不同，不与指令混合加权，只在各行的 freq 里给出。

## 6. 运行时频次缺口与要加的插桩（没有启动游戏）

- 没有执行剖面：指令频次是静态位点数，不是热路径次数。要加：Linux perf 式采样不可用，建议在 victory-loader 里对 masked/clwb/console_insn 位点加命中计数器（每 5 秒一行日志），并给 cmpxchg 失败路径抽样。
- HLE 调用次数：日志只有汇总（13 条“title memory asked for”分组、79 次 dstorage 文件打开等），没有逐次调用计数。要加：每个 xmem/kernel/dstorage 入口一个原子计数器，每 5 秒打印。
- console_insn 陷入次数（ClzeroLoop、Monitorx、Mwaitx、Insertq）每帧多少：日志只有“logged once”。要加计数。
- D3DMetal 的 EndQuery/ResolveQueryData/MakeResident 次数：只有“每进程一次”的提示。要加计数。
- Wine 下的 guarded-page 恢复：需把 tools/mac/rosetta-probe/probe.c 交叉编成 Windows exe 在 Wine 里跑（不启动游戏、不占 GPU）。这是最便宜、价值最高的缺失测试。

## 7. 工具与数据

- tools/mac/census-title-isa.sh/.py：对标题 .text 反汇编做指令类普查（约 2 分钟），输出在 research/census/title-isa-<date>/（游戏派生，不进仓库）。
- tools/mac/census-title-imports.sh/.py：每个导入函数的静态调用点数。
- tools/mac/rosetta-probe.sh + rosetta-probe/probe.c：本机 Rosetta 行为探针（native x86_64 进程，非 Wine）；输出 research/census/rosetta-probe-<date>.txt。
- 外部资料（查证日期 2026-11-27）：Apple《About the Rosetta translation environment》（AVX/AVX2 支持、不含 AVX-512）、MacOSX27.2 SDK 头文件 mach/mach_vm.h 与 Metal MTLDevice.h、Wine master dlls/ntdll/unix/virtual.c（VERSION 11.22；无 class 7）。Metal 4 的能力、macOS 28 对 Rosetta 的改动：待核实。

## 8. 矩阵列

`id,chain,hop,unit,owner,used,freq,freq_source,tests,reference,reference_detail,status,disposition,xbox_hw,deviation,restore_plan,notes`（deviation、restore_plan 只对 emulated/stubbed/unsupported 行填写；translated 行的风险与后续写在 notes 里以“Risk:”“Next:”标出）。
