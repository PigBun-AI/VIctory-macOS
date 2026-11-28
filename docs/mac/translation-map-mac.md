# 转译地图 + 覆盖矩阵（Mac/Wine 段 第 1 版）

日期 2026-11-27。分支 mac/translation-map（基于 mac/fix-wine-tests）。矩阵：`docs/mac/translation-matrix-mac.csv.txt`（88 行，2026-11-28 重算；列见文末）。
范围：Rosetta 指令语义、加载期补丁、故障跟踪与异常、HLE（内存/线程/文件/DirectStorage），加上共享层（victory-gpu，接缝 A 之前）的骨架行（owner=shared）。Metal 后端、着色器翻译不在本文件。

## 0. 一眼看结论

- 覆盖率（第 5 节；不含「仅参照（D3DMetal）」行）：按单元数，已用的 72 个单元里 proven 36（50.0%）、unproven 24（33.3%）、gap 12（16.7%）（2026-11-28 下午 mac/map-close1 两个 int3 节点后重算，已在 mac/livetr 48ee9251 之上；口径同 5.1）。按频次加权（仅指令类，静态普查，freq 列求和 3,840,791）proven 14.5%、unproven 85.5%、gap 0%。
- **Wine 下实测（本机 macOS 27.2.1 / M5 Max / CrossOver Wine 26.3，2026-11-27，tools/mac/rosetta-probe-wine.sh，一次 692 个 case 进程 + 数值批次）**：
  - 普通 store（mov、xmm/ymm 移动、vextract、非临时 store、rep movs/stos）在 fault/只读/跨页/16 KiB 宿主页内 4 KiB 保护下都恢复，VEH 看到的地址精确、异常地址 = 指令地址、类型 write，结果对。
  - **masked store 与 vcvtps2ph [mem] 在 Wine 下一样致命（EmulateForward 中止进程，VEH 不被调用）**：masked store 的规律是「**fault 的 lane 不是 lane 0 就致命**」，跟是否跨页无关（hi、last 掩码打到不可访问页也致命；full、lane 0、交替掩码的第一个 lane 在故障页上能恢复）；vcvtps2ph 写内存只要 fault 就致命（60/60）。16 KiB 宿主页内的 4 KiB 保护、16 KiB 边缘保护同样。对游戏：只有加载期补丁（mac.loader.masked_patch，把这些改成逐 lane 普通 store）或不让这些 store 碰被保护的页才安全；默认跟踪关时没有页被保护。
  - **lock cmpxchg 16 位在地址 = 15 mod 16 且成功时内存不写但 ZF=1（丢更新，新登记 mac.isa.lock_cmpxchg16_split）**；标题有 2,036 条 16 位 lock cmpxchg，是否有未对齐的未知。32 位 cmpxchg 成功时 RAX 高 32 位被清零（SDM 说不动），登记为 mac.isa.cmpxchg32_rax_hit。
  - **RMW 指令（cmpxchg、xadd、xchg [mem]、lock or/bts）在 PAGE_NOACCESS 页 fault 时 VEH 看到的类型是 read（x86 是 write）；跨页的 RMW 报 read + 页基址**，只读页则正确。victory-gpu 的 watch.rs 只处理 WRITE，所以保护用 PAGE_READONLY 不要用 NOACCESS（mac.isa.rmw_fault_access_type）。
  - BMI/LZCNT/POPCNT 46 种形式各 60,000 组、FMA 60 种形式（随机 + 边界 + 4 种舍入 + FTZ/DAZ + NaN 规则）、vcvtph2ps 全部 65,536 个半精度、vcvtps2ph imm 0-4 × 4 种舍入各 320 万个，全部与软件模型逐位一致（0 个不符）；cpuid/xgetbv 和 4 KiB 页粒度也实测。失败的 cmpxchg 的 CF/OF/SF/AF/PF 在 8 种形式、各 2 万组随机下 100% 是操作数反了的结果（ZF 对），xadd 的标志全对。
- 默认启动（Rosetta + 跟踪关）时 **1234 处 masked store 补丁一个都不打**（masked::choice(None,false)=None）。运行日志里看到“1234 patched”是因为 tools/mac/run.sh 和集成 runlt.sh 都 export 了 VICTORY_PATCH_MASKED=all。直接 `wine victory.exe` 或 tools/macos/victory.sh 启动则不打补丁。
- **0x140456fe2 不是标题地址**：标题映像加载在 0x6fffecfc0000（首选 0x140000000），这个 rip 在 victory.exe 自己里。metal-livetr 的 release exe（582a646f，不含 G6）里它是 `vpmaskmovd ymmword ptr [rcx], ymm4, ymm3`，位于 `victory_safe_merge_at`（crates/victory-gpu/src/safecopy.rs 的 global_asm）。错误字节 c4e25d8e19 eb04 c5fe7f19 f30fb8c0 与之吻合。标题自己的 RVA 0x456fe2 只是一条 `jne`，且不在补丁范围。G6（b7943221）把这段改成 Rosetta 下逐 dword 的普通 mov，本分支已含 G6；metal-livetr 那个 exe 是否含 G6 没有核对。日志出处：game-runs 20261127-d3dmetal-livetr-ab/stderr.log:3651、metal-livetr-r4:5140、r5:4156（num_insts=34 inst_index=9）。不改代码。
- 本机原生（非 Wine）Rosetta 探针的旧结论仍成立：clwb、AVX-512、clzero、monitorx → SIGILL（Wine 下 clwb 同样是非法指令异常）；普通 256 位 store 跨页可恢复（旧版 Rosetta 不行，Wine 下已验证可以）。
- 新登记（上级消息 2026-11-28）：mac.hle.audio_timer_leak（audio_client_timer 线程每分钟 +20~40，约几小时撞 macOS 单进程 16,384 线程上限）、mac.title.int3_31f50（标题 image+0x31f50 的 int3，只见过 1 次，used=unknown）。
- **后端取舍**：用户决定以后只走自己的后端，D3DMetal 不再投入。凡是只和 D3DMetal 有关的行（no-op PSO、MakeResident、timestamp、depth bounds、R32G32B32、SetRenderVertexBuffers 崩溃）标「仅参照（D3DMetal）」，不进覆盖率和偏差排序，单列在第 10 节。reference=d3dmetal 的单元只有开场、菜单场景可当像素参照，世界场景不算已证明（见第 9 节）。

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
| mac.isa.maskmov_ymm_store_fault_crossing | AVX2 masked 256-bit store that starts in accessible memory and faults in the next page (the VirtualProtect-gu… | 1,130 | static-census | proven | emulated |
| mac.isa.vcvtps2ph_mem_fault | F16C vcvtps2ph store to memory when the destination page faults (any page, crossing or not) | 70 | static-census | proven | emulated |
| mac.isa.vextract128_mem | vextractf128/vextracti128 store to memory (xmm half of ymm) | 33 | static-census | proven | translated |
| mac.isa.store256_plain | plain 256-bit vector stores (vmovups/vmovdqu/vmovaps ymm to memory), crossing into a protected page | 119,255 | static-census | proven | translated |
| mac.isa.clwb | clwb (cache-line write-back) executes | 8 | static-census | proven | stubbed |
| mac.isa.nontemporal_store | non-temporal stores (vmovntps/vmovntdq ymm, movnti, movntdq) | 53 | static-census | proven | translated |
| mac.isa.lock_cmpxchg_semantics | lock cmpxchg / cmpxchg16b: memory result and ZF | 28,361 | static-census | proven | translated |
| mac.isa.lock_cmpxchg_flags | flags after a FAILED lock cmpxchg (CF, AF, SF, OF, PF) | 28,305 | static-census | proven | emulated |
| mac.isa.lock_other_xchg | other locked RMW (xadd, or, and, inc, dec, bts, btr, add, sub, xor) and xchg with memory | 28,435 | static-census | proven | translated |
| mac.isa.cpuid_xgetbv | cpuid and xgetbv as the title sees them | 11 | static-census | proven | emulated |
| mac.isa.rdtsc | rdtsc / rdtscp | 0 | static-census | unproven | emulated |
| mac.isa.bmi_lzcnt_popcnt | BMI1/BMI2, LZCNT, POPCNT | 76,588 | static-census | proven | translated |
| mac.isa.fma_f16c | FMA3 (vfmadd/vfmsub/vfnmadd/vfnmsub) and F16C conversions (register forms) | 207,582 | static-census | proven | translated |
| mac.isa.avx_general | all other VEX AVX/AVX2 instructions (ymm arithmetic, shuffles, gathers) | 3,278,635 | static-census | unproven | translated |
| mac.isa.amd_only | AMD-only instructions: clzero, monitorx, mwaitx, SSE4a insertq (extrq 0) | 33 | static-census | unproven | emulated |
| mac.isa.tso_ordering | x86 total-store-order memory model for lock-free code (pause 2,601, mfence 36, sfence 20, lfence 8) | 2,665 | static-census | unproven | translated |
| mac.isa.fault_resume_ordinary | synchronous page fault resume of ordinary stores, locked RMW, rep movs, movnt (all but the masked/vcvtps2ph c… | 未知 | unknown | proven | translated |
| mac.isa.page_size | page granularity seen by the title (4 KiB guest pages on 16 KiB host pages) | 未知 | unknown | proven | translated |
| mac.isa.lock_cmpxchg16_split | 16-bit lock cmpxchg whose operand straddles a 16-byte boundary (address = 15 mod 16) | 2,036 | static-census | unproven | emulated |
| mac.isa.cmpxchg32_rax_hit | 32-bit cmpxchg success path: upper 32 bits of RAX | 9,665 | static-census | proven | faithful |
| mac.isa.rmw_fault_access_type | access type and address in the exception record of a faulting locked/RMW instruction (cmpxchg, xadd, xchg [me… | 56,796 | static-census | proven | emulated |
| mac.title.int3_31f50 | the title's own fatal OOM of frame scratch arena 1 at image+0x31f50 (Game Init, once in ~29 runs; not injected by us, cause of the empty arena undecided) | 1 of ~29 runs | log | unproven | deviation |
| mac.title.int3_self_exit_r6 | the title's own assertion at image+0x5149aec: BUILDING entity pool (22500) empty (4 of 4 long world runs, 357-449 s) | 4 of 4 | log | gap | deviation |
| mac.title.srl_prestream_stall | intro SRL Prestream never completes, entities grow until the pool is full | 3 of 3 crash.logs | log | gap | deviation |

### 加载期补丁

| id | 单元 | 频次 | 来源 | status | disposition |
|---|---|---|---|---|---|
| mac.loader.masked_patch | masked-store rewrite to plain stores (masked.rs): scans c4-VEX 0F38 8e/2e/2f stores + vcvtps2ph + vextract128… | 1,234 | log | proven | translated |
| mac.loader.masked_default_off | masked-store patch NOT applied by default: needs tracking ON under Rosetta (stores_need_patching) or VICTORY_… | 1,234 | static-census | proven | faithful |
| mac.loader.wide_patch | 256-bit store split into two 128-bit stores (wide.rs), sites from `cargo xtask wide-stores` | 117,413 | static-census | proven | stubbed |
| mac.loader.clwb_nop | clwb write-back loops replaced by NOPs (only if exactly 8 loops are found) | 8 | log | proven | stubbed |
| mac.loader.resolution | title output size site patches (2 sites) and VICTORY_OUTPUT / RENDER_SCALE / UPSCALER | 2 | log | unproven | stubbed |

### 故障跟踪与异常

| id | 单元 | 频次 | 来源 | status | disposition |
|---|---|---|---|---|---|
| mac.fault.tracking_decision | fault tracking of title writes OFF by default under Rosetta (watch::writes returns None; callers copy memory … | 未知 | unknown | proven | stubbed |
| mac.fault.ntquery_class7 | NtQueryVirtualMemory class 7 (MemoryRegionInformationEx) used by watch::placeholder() | 1 | static-code | gap | stubbed |
| mac.fault.xmem_no_write_watch | MEM_WRITE_WATCH left off the pools under Rosetta | 16 | log | proven | emulated |
| mac.fault.files_overlapped_wait | overlapped ReadFile is waited for before the destination is guarded again (tracking on under Rosetta) | 2 | static-census | unproven | emulated |
| mac.fault.titlewrite | decoder that emulates faulting title stores (what the store does to registers/flags/memory) | 0 | log | proven | emulated |
| mac.fault.shadow_inplace | shadow copies of title memory for the GPU (shadow.rs) and in-place views (inplace.rs) | 9 | log | unproven | emulated |
| mac.fault.safecopy_asm | victory_safe_put / victory_safe_merge stores (vpmaskmovd) and their scalar twins | 3 | static-code | proven | translated |

### HLE 内存/线程/文件/DirectStorage

| id | 单元 | 频次 | 来源 | status | disposition |
|---|---|---|---|---|---|
| mac.hle.xmem_virtual_alloc | XMemVirtualAlloc -> VirtualAlloc2 with address requirements (4 GiB..2^40 or 2^47-n, low 4 GiB for 32-bit flag… | 16 | log | proven | translated |
| mac.hle.console_model | console model presented to the title: Series X (device type 0x64) vs Series S (0x60) (map id C-R-005) | 1 | log | proven | translated |
| mac.hle.low_address_commit | allocation returning NULL to the title (title writes 0 at image+0x2215c35 after __XMemAllocationHooks__) | 3 | log | unproven | unsupported |
| mac.hle.commit_quota | commit-limit assumption: title budget (title_limit) from console::title_memory vs host commit | 5 | static-census | unproven | stubbed |
| mac.hle.working_set_statistics | XMemGetWorkingSetStatistics returns limit/used for the title | 1 | static-census | unproven | stubbed |
| mac.hle.tools_memory | retail answers: no tools memory, transfers bounded | 2 | static-census | proven | mapped |
| mac.hle.globalmemorystatus | GlobalMemoryStatusEx answered by Wine from the Mac host | 1 | static-census | unproven | emulated |
| mac.hle.cool_fox_residency | [仅参照 D3DMetal] 32 GB residency budgets, fallback heaps, GPU pool back-off and deferred uploads under low comm… | 2,800 | log | unproven | stubbed |
| mac.hle.zero_copy_alias | [仅参照 D3DMetal] zero-copy title buffers via mach_vm_remap (copy=FALSE) onto D3DMetal buffer pages | 未知 | unknown | gap | stubbed |
| mac.hle.suspendthread | SuspendThread only for the title's own threads (others return success without suspending) | 1 | static-census | unproven | translated |
| mac.hle.files | virtual drives G: and T: map title file paths onto host folders (rewrites CreateFileW/ReadFile/...) | 13 | static-census | proven | translated |
| mac.hle.dstorage | DirectStorage queues/requests, zlib and BCPack decompression on the CPU | 79 | log | proven | emulated |
| mac.hle.display_audio_misc | HLE stubs: HDR off (SDR present), screenshots fail, DSP convolution/transform outputs zeros, speech silent, e… | 4 | log | unproven | stubbed |
| mac.hle.audio_timer_leak | audio_client_timer threads grow without bound: +20..40 per minute in a long run | 20 | log | gap | emulated |

### 分辨率链（占位，细节待另一只手交回）

| id | 单元 | 频次 | 来源 | status | disposition |
|---|---|---|---|---|---|
| mac.res.output | title output size (swap chain / output resolution) | 1 | log | proven | faithful |
| mac.res.internal | title internal (render) resolution | 未知 | unknown | proven | translated |
| mac.res.drs | dynamic resolution scaling driven by GPU frame time | 未知 | unknown | gap | unsupported |
| mac.res.fsr | in-game FSR upscaler (amd_fidelityfx_upscaler_xbox.dll + loader dll) | 未知 | unknown | unproven | translated |

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
| shared.pipeline.unknown_creation_methods | title pipeline-creation methods at vtable slots 0x1c8 and 0x218 (title+0x2477578, title+0x24800d5) return E_N… | 18,432 | log | gap | stubbed |
| shared.pipeline.untranslated_shader | a shader that does not translate: its pipelines' draws are dropped | 1 | log | gap | unsupported |
| shared.draw.no_host_pipeline | draws/dispatches dropped because no host pipeline exists (being made, refused, or untranslated) | 1 | log | gap | stubbed |
| shared.draw.descriptor_drops | draws dropped for: no free descriptor slots, root UAV/SRV with no bound view heap, address-reading shader wit… | 0 | log | unproven | stubbed |
| shared.indirect.clamp_dropped | [仅参照 D3DMetal] indirect execution dropped when the argument-clamp shader fails to build (HLSL needs SM 6.6 / … | 2 | log | proven | stubbed |
| shared.query.timestamps | [仅参照 D3DMetal] timestamp queries (EndQuery/ResolveQueryData) used by the title for DRS and profiling | 未知 | unknown | gap | unsupported |
| shared.query.depth_bounds | [仅参照 D3DMetal] depth bounds test reported supported (OPTIONS2 DepthBoundsTestSupported=true on D3DMetal) | 未知 | unknown | unproven | stubbed |
| shared.resource.typed_buffer_r32g32b32 | [仅参照 D3DMetal] R32G32B32 typed buffers (D3DMetal: "R32G32B32 typed buffers not supported, using R32G32") | 未知 | unknown | gap | stubbed |
| shared.residency.make_resident | [仅参照 D3DMetal] MakeResident/Evict ("Unsupported: MakeResident" once per process on D3DMetal) | 1 | log | gap | stubbed |
| shared.pipeline.d3dmetal_noop_pso | [仅参照 D3DMetal] about 750 pipeline state objects marked no-op by D3DMetal in the world: world frames come out … | 750 | log/report | gap | unsupported |
| shared.draw.set_render_vertex_buffers_crash | [仅参照 D3DMetal] SetRenderVertexBuffers crash on D3DMetal (the G7 kind of defect) | 未知 | unknown | gap | unsupported |

### Metal 后端与标题看门狗（2026-11-28 新增；metal.* 归属 Metal 后端负责人）

| id | 单元 | 频次 | 来源 | status | disposition |
|---|---|---|---|---|---|
| metal.post.auto_exposure | 自动曝光统计算错：tonemap 曝光缩放 = 0.75/最小亮度，应为平均亮度；曝光逐帧累积，进入可玩世界后约 100–150 帧开始发白（场景 HDR 数值与 D3DMetal 重放一致；正在修） | 未知 | unknown | gap | deviation |
| metal.gpu.fidelity_slow_frame | fidelity（内部 2560x1440）GPU 每帧 6.7–17 s（perf 0.2 s）；约 31 s、约 115 s 的停顿；w37 后卡死、CPU 0%（怀疑 GPU 超时后没有 signal） | 未知 | unknown | gap | deviation |
| mac.title.hang_watchdog | 标题 30 s 挂起看门狗（+0x2215c35 → +0x23cf683 写 0）；我们后端长帧停顿时被它杀掉；L5 --no-watchdog 是非保真绕法，晨测使用 | 未知 | log | unproven | stubbed |

## 3. 用到但未证明（按频次）

used=yes 且 status≠proven，去掉「仅参照（D3DMetal）」行（第 10 节）。频次是静态普查次数或日志次数，两类不能直接相加。

| 频次 | 来源 | id | status | 缺什么 |
|---|---|---|---|---|
| 3,278,635 | static-census | mac.isa.avx_general | unproven | differential test vs a scalar model for the 50 most frequent VEX mnemonics (census isa-functions.tsv) |
| 56,796 | static-census | mac.isa.rmw_fault_access_type | gap | handler test: a read fault at an RMW instruction counts as a write guard hit; keep guards PAGE_READONLY |
| 28,305 | static-census | mac.isa.lock_cmpxchg_flags | gap | decoder test compares ZF only (flags measured: 100% reversed on a miss, see matrix) |
| 18,432 | log | shared.pipeline.unknown_creation_methods | gap | a decoder test over the logged descriptions (16 words) after the two methods are reverse engineered |
| 9,665 | static-census | mac.isa.cmpxchg32_rax_hit | gap | spot-check consumers of RAX after the 9,665 dword lock cmpxchg sites; Apple report |
| 2,665 | static-census | mac.isa.tso_ordering | unproven | litmus tests (MP, SB, IRIW) 1e8 iterations under Rosetta+Wine |
| 2,036 | static-census | mac.isa.lock_cmpxchg16_split | gap | check whether any of the 2,036 word-sized lock cmpxchg sites can be misaligned (address log of first hit per site); else rewrite |
| 1,234 | log | mac.loader.masked_patch | unproven | run the 4 Windows-only patch tests under Wine + a writer/reader race test |
| 1,234 | static-census | mac.loader.masked_default_off | gap | plan() unit test for the 4 combinations (Rosetta x tracking x VICTORY_PATCH_MASKED) in apps/victory mac.rs; log a line when nothing is patched |
| 79 | log | mac.hle.dstorage | unproven | run the 19 dstorage tests under Wine; bytes-equal test per request kind |
| 70 | static-census | mac.isa.vcvtps2ph_mem_fault | gap | rewrite-vs-original test for vcvtps2ph store, all rounding modes |
| 33 | static-census | mac.isa.amd_only | unproven | trap-count log (VICTORY diagnostics) per frame + a unit test of the Insertq/mwaitx emulation |
| 20 | log | mac.hle.audio_timer_leak | gap | find the thread that is not retired; 1,000 create/cancel test with a thread-count assert |
| 16 | log | mac.fault.xmem_no_write_watch | unproven | test pool flags per mode |
| 16 | log | mac.hle.xmem_virtual_alloc | unproven | run xmem tests under Wine |
| 13 | static-census | mac.hle.files | unproven | run files tests under Wine |
| 9 | log | mac.fault.shadow_inplace | unproven | run the warp-reference tests against a real second adapter |
| 8 | static-census | mac.isa.clwb | gap | See mac.loader.clwb_nop. No Arm equivalent is reachable from translated code; clflushopt/clflush do execute (probe) and could replace clwb loops if a flush wer… |
| 8 | log | mac.loader.clwb_nop | unproven | counter "title pages the GPU reads without a CPU copy" must stay 0; run once with clflush replacing the NOPs and compare frames |
| 5 | static-census | mac.hle.commit_quota | unproven | test: title-like allocation sequence past the 13.5 GB budget must be refused as on Xbox |
| 4 | log | mac.hle.display_audio_misc | unproven | one test per stub: HDR query, screenshot, DSP command zeros, speech, package list |
| 3 | log | mac.hle.low_address_commit | gap | log every allocator failure (hook result) + a test that forces XMemVirtualAlloc failure and checks the title-visible NULL path |
| 2 | static-census | mac.fault.files_overlapped_wait | unproven | overlapped read into guarded memory under Wine |
| 2 | static-census | mac.hle.tools_memory | unproven | Risk: None (faithful to retail). |
| 1 | log | mac.res.output | unproven | Fidelity phase: run the Xbox mechanism as is (internal resolution, DRS and the game's own FSR, amd_fidelityfx_upscaler_xbox.dll, all through translated shaders… |
| 1 | static-code | mac.fault.ntquery_class7 | gap | Wine test: NtQueryVirtualMemory class 7 on a placeholder (then the 3 failing watch tests become the regression) |
| 1 | static-census | mac.hle.working_set_statistics | unproven | test with a fake title that allocates then reads the statistics: expects its own heaps only |
| 1 | static-census | mac.hle.globalmemorystatus | unproven | test the override returns the console budget fields |
| 1 | static-census | mac.hle.suspendthread | unproven | Windows exe test: suspend title-image thread vs foreign thread |
| 1 | log | shared.object.unknown_method | gap | identify and implement queue slot 0xb8; test every vtable slot answers or logs |
| 1 | log | shared.pipeline.untranslated_shader | gap | translate-all test of the title container with wave64->wave32 lowering (shader crate) |
| 1 | log | shared.draw.no_host_pipeline | gap | assert "dropped 0" in the 5 s summary for a scripted opening run |
| 未知 | unknown | shared.root.signature | unproven | Wine 1 failure: D3DMetal accepts a blob the test expects to be rejected (PATCHES.md). / Risk: Console-only root flags are dropped. |
| 未知 | unknown | shared.descriptor.heaps | unproven | fix the 2 failing descriptor::host tests under D3DMetal |
| 未知 | unknown | shared.resource.heaps_at_addresses | unproven | See mac.hle.zero_copy_alias. |
| 0 | log | mac.fault.titlewrite | gap | fix the decoder test to compare ZF only for failed cmpxchg under Rosetta |
| 0 | log | shared.fence.wait | unproven | unit test: blocking wait without event completes |
| 0 | log | shared.device.feature_query | unproven | unit test over every D3D12_FEATURE id |
| 0 | log | shared.draw.descriptor_drops | unproven | stress test of descriptor slots |

## 4. 偏差清单（emulated + stubbed + unsupported，按 频次×严重度 排序）

严重度 1 无可见后果、2 性能或罕见路径、3 逻辑或画面可能错、4 内容缺失或明显错、5 进程崩溃。排序分 = 频次（未知记 1）× 严重度。可行性 高/中/低/待核实。外部事实的来源和查证日期在 CSV 的 reference_detail 和 notes（日期均 2026-11-27）。

| # | id | 处置 | 频次 | 严重度 | 偏差 | 后果 | 还原方案 | 代价 | 可行性 |
|---|---|---|---|---|---|---|---|---|---|
| 1 | mac.isa.rmw_fault_access_type | emulated | 56,796 | 3 | Page fault of a locked RMW is a write at the faulting byte. | NOACCESS: read; split RMW: read at the page base. | Guard with PAGE_READONLY; handler test for RMW reads. | 小 | 高 |
| 2 | mac.loader.wide_patch | stubbed | 117,413 | 1 | n/a | Off unless VICTORY_PATCH_WIDE=<list> is set (no script sets it, no list is generated by the repo): effectively not applied. Probe: plain 256-bit stores resume on macOS 27.2.1, so it is not needed there. | Keep disabled; if a later macOS regresses, generate the list at first start (xtask logic as a loader pass) and apply. Cost: 117k trampolines (~2 MB code). | 小（数十行/一个测试） | 高 |
| 3 | shared.pipeline.unknown_creation_methods | unsupported | 18,432 | 4 | Console D3D12 has two pipeline-creation methods the PC API lacks. | Title goes without those pipelines silently (what they draw is unknown): whole draws and dispatches missing, maybe visible as absent geometry, or the engine uses a fallback (not proven). | Decode the logged descriptions (first words point at stream tokens: the dump in the log) and implement the two methods through CreatePipelineState from a stream; feasibility medium. This is the largest known unsupported call count in the shared layer. | 中（模块级改动，需验证） | 中 |
| 4 | mac.isa.lock_cmpxchg_flags | emulated | 28,305 | 1 | x86 sets all arithmetic flags as CMP accumulator,destination. | Operands reversed in the flag computation on a miss: CF/OF/SF/AF/PF wrong, ZF right. Consumer code reading only ZF is unaffected. | Exposure is nil for the title: of 28,305 sites, 19,802 are followed directly by a ZF-only reader (je/jne/sete/cmov), 0 by a reader of CF/SF/OF, 8,503 by a non-reader. Fix the decoder test (titlewrite) to expect Rosetta flags or compare ZF only; if a site ever… | 小（数十行/一个测试） | 高 |
| 5 | mac.isa.cmpxchg32_rax_hit | emulated | 9,665 | 1 | RAX bits 63:32 unchanged on a hit. | Zeroed on a hit. | Spot-check consumers; report to Apple. | 小 | 待核实 |
| 6 | mac.isa.lock_cmpxchg16_split | emulated | 2,036 | 3 | Zen 2 performs the lock cmpxchg atomically across a 16-byte boundary. | Hit at offset 15 mod 16: memory not written but ZF=1 (lost update). | Check alignment of the 2,036 word sites, else rewrite as dword. | 小（一个日志计数器） | 中 |
| 7 | mac.isa.maskmov_ymm_store_fault_crossing | emulated（2026-11-28 L1 已覆盖全部站点，status=proven；以下为未打补丁时的偏差） | 1,130 | 5 | Zen 2 / x86 resumes the instruction after the fault handler opens the page (what Windows VirtualProtect guard… | Fatal under Wine too (probe-wine 2026-11-27): Rosetta aborts when a lane >= 1 faults (any page, any crossing), no resume; lane-0 faults resume. Same fatal text as the 0x140456fe2 hang. | Highest fidelity = never let a masked store fault: (1) loader rewrite to plain stores (L1, 1234 sites) whenever tracking is on; (2) our own asm (safecopy, fixed by G6); (3) keep fault tracking off by default. A true fix needs Apple (Feedback Assistant report;… | 中（模块级改动，需验证） | 中 |
| 8 | mac.loader.masked_default_off | stubbed | 1,234 | 4 | n/a | CONFIRMS the reported suspicion for the product code: with Rosetta and fault tracking off (the default) choice(None, needed=false) is None, so 0 of 1234 masked stores are patched; the log line "patched" in the runs appears only b… | Make the decision explicit and logged: patch whenever Rosetta (cost: 1234 jumps, no runtime cost when no fault); apply the 8 extra RVAs together with "all"; add a unit test for plan() covering the 4 combinations. Wide patch L2 is likewise never on by default … | 小（数十行/一个测试） | 高 |
| 9 | mac.isa.maskmov_ymm_store_nofault | emulated | 1,130 | 1 | Zen 2 executes VEX masked stores natively; page fault of a masked-off lane is suppressed by the CPU. | None seen: data stored equals x86 for full, lane-0 and zero masks; zero mask into an inaccessible page raises no fault (as x86). | Nothing to restore; keep tools/mac/rosetta-probe.sh as the regression after every macOS update. | 中（模块级改动，需验证） | 中 |
| 10 | mac.isa.vcvtps2ph_mem_fault | unsupported | 70 | 5 | Zen 2 resumes after the fault. | Fatal under Wine on ANY write fault of the store (60 of 60 runs), non-faulting runs correct. | Same as masked stores: load-time rewrite (L1 includes vcvtps2ph, 70 sites) when tracking is on. For fidelity of the converted half-floats the rewrite must keep rounding control imm8 and MXCSR behaviour: add a test comparing rewrite vs original over random flo… | 中（模块级改动，需验证） | 中 |
| 11 | mac.hle.dstorage | emulated | 79 | 1 | Xbox hardware decompression block (zlib/BCPack) feeding memory directly. | Software decompression on CPU threads (perf), requests into guarded memory retried; behaviour identical in bytes (tests). | Metal fast-resource-loading (MTLIOCommandQueue) offers zlib/LZFSE/LZ4/LZMA/LZBitmap (SDK MTLDevice.h, checked 2026-11-27) but not BCPack: keep CPU BCPack, optionally GPU/IO queue for zlib. | 中（模块级改动，需验证） | 中 |
| 12 | mac.isa.amd_only | emulated | 33 | 2 | Zen 2 executes them (clzero zeroes a cache line; monitorx/mwaitx wait on an address). | clzero loop -> plain zero stores (patched into a jump after first fault, equal result); monitorx -> 3-byte no-op, mwaitx -> pause (the wait loop re-reads its value: busy-wait, more CPU, same logic); insertq -> emulated per fault … | Real mwaitx wake-up semantics could be given by a futex-style park on the monitored address in the emulator (Apple os_sync_wait_on_address): lower power, same logic. Insertq: patch to the equivalent pinsr/shift sequence at load. | 小（数十行/一个测试） | 高 |
| 13 | mac.hle.audio_timer_leak | emulated | 20 | 3 | Console OS retires audio timers. | 20-40 threads/min leak; macOS limit 16,384 per process, hit after hours. | Reuse one timer thread; 1,000 create/cancel test. | 小（数十行） | 中 |
| 14 | mac.isa.cpuid_xgetbv | emulated | 11 | 3 | Zen 2: AuthenticAMD, family 17h, AVX2/BMI/FMA/F16C/CLWB/CLFLUSHOPT/CLZERO/MONITORX advertised. | Vendor Intel, no CLWB/CLFLUSHOPT/CLZERO/MONITORX bits, AVX only with the env var. A title branch on vendor or family would take a different path than on Xbox. | Patch the 9 cpuid sites (3 functions) to present Zen 2 values, or confirm by disassembly that they only test feature bits. Both runs scripts (tools/mac/run.sh, tools/macos/victory.sh) set ROSETTA_ADVERTISE_AVX=1; a bare launch does not. | 小（数十行/一个测试） | 高 |
| 15 | mac.fault.xmem_no_write_watch | stubbed | 16 | 2 | GPU reads memory directly. | No kernel write-watch: written pages unknown, so whole copies (same cost as tracking off). Wine emulates write watch with page protection, which hits Rosetta unresumable faults. | With tracking restored, turn on again; test: pool alloc has/hasn't MEM_WRITE_WATCH per mode. | 中（模块级改动，需验证） | 中 |
| 16 | mac.fault.shadow_inplace | emulated | 9 | 2 | GPU reads title memory directly. | Copy-in/copy-out through shadows; D3DMetal refuses OpenExistingHeapFromAddress (alias.rs doc), so no in-place on the Mac host path. | See mac.hle.zero_copy_alias. | 大 | 低 |
| 17 | mac.isa.clwb | unsupported | 8 | 2 | Zen 2 CLWB writes a dirty cache line back to memory without evicting it; used so the GPU (not cache-coherent … | Instruction does not exist under Rosetta: SIGILL. Windows line emulates by exception (one per loop, ~300 us through Wine). | See mac.loader.clwb_nop. No Arm equivalent is reachable from translated code; clflushopt/clflush do execute (probe) and could replace clwb loops if a flush were ever needed (evicts the line, so slower but correct). | 中（模块级改动，需验证） | 中 |
| 18 | mac.loader.clwb_nop | stubbed | 8 | 2 | CLWB pushes dirty lines to memory so non-coherent consumers (GPU, DMA) see CPU writes. | Write-back dropped. Safe on this path because all title memory is read by our own CPU code (copies for the GPU: unified memory, CPU-coherent). Would break if the GPU read title memory without a CPU copy (zero-copy) and CPU caches… | Today: keep NOP. If zero-copy is ever built (mach_vm_remap / MTLBuffer no-copy on the title pages), replace by a real flush: clflush is available under Rosetta (probe) or rely on Apple silicon CPU/GPU coherency for shared memory (Apple docs: unified memory; 待… | 小（数十行/一个测试） | 高 |
| 19 | mac.hle.low_address_commit | unsupported | 3 | 5 | Fixed Xbox address map with enough commit. | Title gets a null from one of its own allocators (not through XMemVirtualAlloc) and dies on purpose (mov [0],0; int3). | Instrument first (log every allocator failure site incl. hooks result); then port #398 low-address guards if the failing request is a 32-bit-flag one, or raise Wine commit/reservation. Feasibility: medium (cause unknown). | 中（模块级改动，需验证） | 中 |
| 20 | mac.hle.commit_quota | stubbed | 5 | 3 | Console OS enforces a 13.5 GB title budget and fails commits past it. | Our budget is a number we give the title; the host does not enforce it, so over-commit is not refused (and swap/compression may stall) instead of failing as on Xbox. | Account title commits in the HLE (sum of committed MEM_COMMIT) and refuse past the console budget, as the Xbox OS does; test with a title-like allocation sequence. | 小（数十行/一个测试） | 高 |
| 21 | mac.hle.display_audio_misc | stubbed | 4 | 3 | Console OS/APU/DSP hardware does these. | HDR never output; DSP effects silent (zeros); no TTS; package queries optimistic; no haptic triggers. | Per item: HDR via CAMetalLayer EDR (Metal backend); DSP convolution/FFT in software (the code says it needs the coefficient format); speech via AVSpeechSynthesizer; gameinput haptics via GCController haptics (CoreHaptics, 待核实 trigger-motor support). | 中（模块级改动，需验证） | 中 |
| 22 | mac.loader.resolution | stubbed | 2 | 3 | Title picks output size, internal resolution, DRS and in-game FSR itself. | Output forced to 1280x720 on a Mac (see mac.res.*). | See mac.res.output. | 小（数十行/一个测试） | 高 |
| 23 | mac.res.output | stubbed | 1 | 4 | Title renders internal resolution then outputs at the console output size (1080p/1440p/2160p per user setting… | Output is changed to 1280x720, inconsistent with the Xbox internal resolution + DRS + in-game FSR mechanism; effect to be determined. | Fidelity phase: run the Xbox mechanism as is (internal resolution, DRS and the game's own FSR, amd_fidelityfx_upscaler_xbox.dll, all through translated shaders, no replacement). MetalFX is a performance-phase candidate only. | 待核实 | 待核实 |
| 24 | mac.fault.files_overlapped_wait | emulated | 2 | 2 | Async read completes into memory the GPU may read. | The call blocks until the read ends (only with tracking on): async IO serialised, slower loads; avoids a kernel write into a guarded page which hangs the process under Wine/Rosetta (#50). | Restore async by reading into a bounce buffer and copying after completion (no kernel write to guarded pages); test with guarded destination under Wine. | 中（模块级改动，需验证） | 中 |
| 25 | mac.res.internal | stubbed | 未知 | 3 | Title chooses internal resolution (and render scale) per mode. | VICTORY_RENDER_SCALE default off: internal resolution follows the title; effect of the 720p output on it is to be determined. | Same as mac.res.output. | 待核实 | 待核实 |
| 26 | mac.res.drs | emulated | 未知 | 3 | Title adapts internal resolution from GPU timing (Xbox GPU timestamps). | GPU timestamps are 0 on D3DMetal (EndQuery/ResolveQueryData unsupported), so the title's DRS input differs from Xbox; effect to be determined. | Same as mac.res.output; needs working timestamp queries on the Metal backend (see shared.query.timestamps). | 待核实 | 待核实 |
| 27 | mac.res.fsr | emulated | 未知 | 3 | Title calls the Xbox FidelityFX upscaler DLL on the GPU. | VICTORY_UPSCALER default off; behaviour of the shipped DLL under the loader is to be determined. | Same as mac.res.output (shaders translated, no replacement). | 待核实 | 待核实 |
| 28 | mac.fault.tracking_decision | stubbed | 未知 | 3 | Title memory is unified memory the GPU reads directly; no tracking exists. | Whole-block copies for every submission instead of only written pages: 129 GB/frame estimated once the world streams (masked.rs header, #50); in logs the opening scene copies 0.1-3.1 MB/frame with tracking off. | Make tracking safe so it can be on: needs every faulting instruction class resumable (masked stores L1 + vcvtps2ph L1, our asm G6, SuspendThread H1, overlapped ReadFile A7); then default on. Alternative without faults: GPU reads title pages in place (no copy)… | 中（模块级改动，需验证） | 中 |
| 29 | mac.hle.working_set_statistics | stubbed | 1 | 3 | Console OS reports real title use, per bandwidth class. | "used" is the whole process PagefileUsage (our layer and the card included, 27 GB vs a 13.5 GB limit in run 60 per code comment); GPU-optimal use reads 0. | Count the title's own heaps/mapped pages and per class in the XMem hooks; return those. | 小（数十行/一个测试） | 高 |
| 30 | mac.hle.globalmemorystatus | emulated | 1 | 3 | Console OS reports its fixed memory (title budget). | Reports the Mac host RAM/commit (e.g. >100 GB on this machine) so any pool sizing from it differs from Xbox; the 32 GB-RAM optimisation line (cool_fox) keys on host RAM too. | Override in victory-hle kernel overrides (host_lookups_by_name_get_our_overrides) with the console budget; test the structure fields. | 小（数十行/一个测试） | 高 |
| 31 | shared.descriptor.heaps | emulated | 未知 | 3 | Descriptors are raw 32-byte words the GPU reads from memory. | Host views made from the title's raw descriptors and kept in sync: raw words are not visible to the host driver. | Restore through the Metal backend argument-buffer model (descriptor words decoded to MTLResourceID); see Metal rows. | 中（模块级改动，需验证） | 中 |
| 32 | shared.object.unknown_method | stubbed | 1 | 3 | All methods exist. | E_NOTIMPL, which the title reads as a lost device in some paths (device.rs comment, VIctory 13185f5). r5 log: command queue slot 0xb8 called from title+0x24900a5, once (queue method). | Implement slot 0xb8 of the queue (identify via D3D12 vtable: likely a queue method added in a newer interface). | 小（数十行/一个测试） | 高 |
| 33 | shared.pipeline.untranslated_shader | unsupported | 1 | 3 | GPU runs any shader. | Draws or dispatches of that pipeline vanish (1 compute shader in the opening scene). | Lower wave64 ops to wave32 (shader owner; 18 permlane/quad failing tests listed in PATCHES.md). | 中（模块级改动，需验证） | 中 |
| 34 | shared.draw.no_host_pipeline | stubbed | 1 | 3 | n/a | Draws lost until the pipeline is made; refused ones are lost for good. | Metal side: implement attributeAtVertex (MSC), raise mesh limit by splitting meshlets; shared side: wait for the pipeline instead of dropping the first use (decision 0007 note). | 中（模块级改动，需验证） | 中 |
| 35 | shared.draw.descriptor_drops | stubbed | 0 | 3 | n/a | Draw dropped silently after one log line. | Raise slots dynamically; feasibility high; no occurrence yet. | 小（数十行/一个测试） | 高 |
| 36 | mac.fault.ntquery_class7 | stubbed | 1 | 2 | Windows kernel answers class 7 (region size/placeholder flags). | Wine returns an invalid-info-class status: placeholder() is never true, so memory that is a placeholder is not recognised (only matters with VICTORY_FAULT_TRACKING=1). | High fidelity and cheap: our own registry knows every placeholder we create (xmem.rs placeholder branch), answer from it instead of asking the kernel; or carry a 20-line Wine patch (tools/macos/patches/) adding class 7 to ntdll/unix/virtual.c. | 小（数十行/一个测试） | 高 |
| 37 | shared.resource.heaps_at_addresses | emulated | 未知 | 2 | Resources live at fixed GPU virtual addresses in title memory. | Copies and shadows instead of aliasing; see mac.hle.zero_copy_alias. | See mac.hle.zero_copy_alias. | 大 | 低 |
| 38 | shared.fence.wait | stubbed | 0 | 2 | Fence wait is a GPU hardware semaphore wait. | Blocking CPU wait with no event returns E_NOTIMPL (log "blocking fence wait not implemented": 0 in all runs). | Implement with an internal event (small): feasibility high. | 小（数十行/一个测试） | 高 |
| 39 | shared.device.feature_query | stubbed | 0 | 2 | Driver answers every feature. | E_NOTIMPL for the rest; log "device feature query not implemented": 0 in ab/r5. | Answer additional features from the host device (options1..21, shader model, GPU VA); feasibility high. | 小（数十行/一个测试） | 高 |
| 40 | mac.fault.titlewrite | emulated | 0 | 1 | No emulation: stores simply run. | Tracking-dependent; with tracking off nothing runs through it ("no fault or hang seen" in runs). | Fix test: compare ZF/hit path only for lock cmpxchg under Rosetta (see mac.isa.lock_cmpxchg_flags); run the rest. | 小（数十行/一个测试） | 高 |

### 4.1 stub 清单（disposition=stubbed，按频次；每项后果与要补的测试）

Metal 侧 matrix.sh 合并时也按 freq 排序这些行。

| 频次 | 来源 | id | 可能后果 | 打通要补的测试 |
|---|---|---|---|---|
| 117,413 | static-census | mac.loader.wide_patch | Off unless VICTORY_PATCH_WIDE=<list> is set (no script sets it, no list is generated by the repo): effectively not applied. Probe: plain 256-bit stores resume on macOS 27.2.1, so it is not needed there. | Windows exe probe of plain 256-bit store crossing a guarded page under Wine (VEH resume) |
| 1,234 | static-census | mac.loader.masked_default_off | CONFIRMS the reported suspicion for the product code: with Rosetta and fault tracking off (the default) choice(None, needed=false) is None, so 0 of 1234 masked stores are patched; the log line "patched" in the runs appears only b… | plan() unit test for the 4 combinations (Rosetta x tracking x VICTORY_PATCH_MASKED) in apps/victory mac.rs; log a line when nothing is patched |
| 16 | log | mac.fault.xmem_no_write_watch | No kernel write-watch: written pages unknown, so whole copies (same cost as tracking off). Wine emulates write watch with page protection, which hits Rosetta unresumable faults. | test pool flags per mode |
| 8 | log | mac.loader.clwb_nop | Write-back dropped. Safe on this path because all title memory is read by our own CPU code (copies for the GPU: unified memory, CPU-coherent). Would break if the GPU read title memory without a CPU copy (zero-copy) and CPU caches… | counter "title pages the GPU reads without a CPU copy" must stay 0; run once with clflush replacing the NOPs and compare frames |
| 5 | static-census | mac.hle.commit_quota | Our budget is a number we give the title; the host does not enforce it, so over-commit is not refused (and swap/compression may stall) instead of failing as on Xbox. | test: title-like allocation sequence past the 13.5 GB budget must be refused as on Xbox |
| 4 | log | mac.hle.display_audio_misc | HDR never output; DSP effects silent (zeros); no TTS; package queries optimistic; no haptic triggers. | one test per stub: HDR query, screenshot, DSP command zeros, speech, package list |
| 2 | log | mac.loader.resolution | Output forced to 1280x720 on a Mac (see mac.res.*). | 见 restore_plan |
| 1 | log | mac.res.output | Output is changed to 1280x720, inconsistent with the Xbox internal resolution + DRS + in-game FSR mechanism; effect to be determined. | frame-hash and timing compare with VICTORY_OUTPUT unset (title default) once the resolution investigation reports |
| 1 | static-code | mac.fault.ntquery_class7 | Wine returns an invalid-info-class status: placeholder() is never true, so memory that is a placeholder is not recognised (only matters with VICTORY_FAULT_TRACKING=1). | Wine test: NtQueryVirtualMemory class 7 on a placeholder (then the 3 failing watch tests become the regression) |
| 1 | static-census | mac.hle.working_set_statistics | "used" is the whole process PagefileUsage (our layer and the card included, 27 GB vs a 13.5 GB limit in run 60 per code comment); GPU-optimal use reads 0. | test with a fake title that allocates then reads the statistics: expects its own heaps only |
| 1 | log | shared.object.unknown_method | E_NOTIMPL, which the title reads as a lost device in some paths (device.rs comment, VIctory 13185f5). r5 log: command queue slot 0xb8 called from title+0x24900a5, once (queue method). | identify and implement queue slot 0xb8; test every vtable slot answers or logs |
| 1 | log | shared.draw.no_host_pipeline | Draws lost until the pipeline is made; refused ones are lost for good. | assert "dropped 0" in the 5 s summary for a scripted opening run |
| 未知 | unknown | metal.post.auto_exposure | gap | 曝光统计改成平均亮度后逐帧曝光测试；重抓世界语料（w300 起） |
| 未知 | unknown | metal.gpu.fidelity_slow_frame | gap | 每个 pass 的 GPU 耗时；确认 GPU 超时后是否缺 signal |
| 未知 | log | mac.title.hang_watchdog | gap | 长帧停顿消除后看门狗不再触发（保真跑不得用 L5） |
| 未知 | unknown | mac.res.internal | VICTORY_RENDER_SCALE default off: internal resolution follows the title; effect of the 720p output on it is to be determined. | 见 restore_plan |
| 未知 | unknown | mac.fault.tracking_decision | Whole-block copies for every submission instead of only written pages: 129 GB/frame estimated once the world streams (masked.rs header, #50); in logs the opening scene copies 0.1-3.1 MB/frame with tracking off. | tools/mac/rosetta-probe.sh as a Windows exe under Wine (guarded-page resume for every store class) then a run with FAULT_TRACKING=1 and a frame-hash compare vs tracking … |
| 0 | log | shared.fence.wait | Blocking CPU wait with no event returns E_NOTIMPL (log "blocking fence wait not implemented": 0 in all runs). | unit test: blocking wait without event completes |
| 0 | log | shared.device.feature_query | E_NOTIMPL for the rest; log "device feature query not implemented": 0 in ab/r5. | unit test over every D3D12_FEATURE id |
| 0 | log | shared.draw.descriptor_drops | Draw dropped silently after one log line. | stress test of descriptor slots |

## 5. 覆盖率

口径：单元 = 矩阵里 used=yes 的行，**去掉标为「仅参照（D3DMetal）」的行**（第 10 节；用户 2026-11-28 决定只走自己的后端）。proven = 有硬件事实或规范依据且有测试或实测；unproven = 用到但缺证据；gap = 已知缺口。

### 5.1 按单元数

| 范围 | 单元数 | proven | unproven | gap |
|---|---|---|---|---|
| 全部 used=yes（不含仅参照） | 72 | 36 (50.0%) | 24 (33.3%) | 12 (16.7%) |
| 指令类（mac.isa.*） | 21 | 17 (81.0%) | 4 | 0 |
| 其余（loader/fault/hle/res/title/shared/shader/metal） | 51 | 19 (37.3%) | 20 | 12 |

上一版（本文件第 1 版，未跑 Wine 探针、未剔除仅参照行）：59 个单元 proven 12（20.3%）、unproven 31、gap 16。这版的变化：Wine 探针把 cpuid_xgetbv、bmi_lzcnt_popcnt、fma_f16c、page_size、wide_patch 从 unproven 变 proven；新登记 3 个指令类 gap（lock_cmpxchg16_split、cmpxchg32_rax_hit、rmw_fault_access_type）和 1 个 HLE gap（audio_timer_leak）；7 个 used=yes 的 D3DMetal 行移出统计。

2026-11-28 第二批闭环（mac/map-close2）后从矩阵重算：69 个，proven 25、unproven 32、gap 12（masked_default_off、isa.clwb、low_address_commit 三行 gap→unproven，代码与单元测试已做、待下午运行；新增 mac.hle.console_model 一行 unproven；表里 gap 数此前落后于矩阵，这次一并按矩阵校正）。

2026-11-28 重算（脚本从矩阵算，口径同上：used=yes、去掉 notes 以「仅参照（D3DMetal）」开头的 10 行，共 68 个）。和上次记录（56 个：16/24/16）的差别：矩阵从 70 行增到 87 行，v1 之后加进来的行（shared.rt.*、shader.*、mac.res.*、mac.isa 新行等）计入；本次改动：mac.isa.maskmov_ymm_store_fault_crossing 由 gap 改 proven（L1 加载期补丁改掉全部 masked store 站点，扫描 1233 + 8 个 plain = 1241，审计 0 漏）；新增 3 个 used=yes 的 gap 行：metal.post.auto_exposure、metal.gpu.fidelity_slow_frame、mac.title.hang_watchdog。68 个里含 4 个 shader.*/metal.* 行（shader.ps.attribute_at_vertex proven，shader.mesh.output_size_32k、metal.post.auto_exposure、metal.gpu.fidelity_slow_frame gap）；若只算 v1 范围（去掉 shader.*、metal.* 行）是 64 个：proven 24（37.5%）、unproven 26（40.6%）、gap 14（21.9%）。

disposition 分布（全部 92 行，含仅参照行）：emulated 23，mapped 1，stubbed 28，translated 21，unsupported 8，faithful 4，deviation 6，未定 0，工具行（无 disposition）1。

2026-11-28 下午（分支 mac/map-close1）：新增 mac.runtime.hle_counters（工具行，used=no，不进上面的 68 个；status=proven：机制在 Wine 下实测，见 PATCHES.md H3）。第 5.1 节三个表的数字不变（68：25/26/17）。

2026-11-28 下午（分支 mac/map-close1，rebase 到 mac/livetr 48ee9251 即 close2 之后，计数全部从合并后的矩阵重算）：本分支的变化：新增 mac.runtime.hle_counters（工具行，不进计数）、mac.hle.xmem_map_partial_view（gap，flaky 测试）、mac.title.int3_self_exit_r6（gap）、mac.title.srl_prestream_stall（gap）；mac.title.int3_31f50 由 used=unknown 改 used=yes（静态分析：标题自己的帧临时堆 arena 1 分配失败断言，不是我们注入的，仍 unproven）；在 Wine 下跑后 unproven→proven：xmem_virtual_alloc、tools_memory、files、dstorage、titlewrite、lock_cmpxchg_flags、xmem_no_write_watch、masked_default_off、clwb_nop、isa.clwb、hle.console_model。mac.loader.resolution（除零 clamp 没有运行证据）和 mac.hle.low_address_commit（守卫默认关、没在 Wine 下跑过）保持 unproven。

### 5.2 按频次加权（指令类，静态普查，每条指令只算一次）

为避免重复计数，排除了和别的行同一批位点的 mac.isa.maskmov_ymm_store_nofault、mac.isa.lock_cmpxchg_flags、mac.isa.lock_cmpxchg16_split、mac.isa.cmpxchg32_rax_hit、mac.isa.rmw_fault_access_type、mac.isa.rdtsc（0 次）；mac.isa.avx_general 的权重取 VEX 总数扣掉 FMA/F16C、普通 256 位 store、masked store、vextract、非临时 store 后的剩余（2,950,582）。频次未知的单元（mac.isa.fault_resume_ordinary、mac.isa.page_size）不进分母

| 状态 | 加权频次 | 占比 |
|---|---|---|
| proven | 461,518 | 13.5% |
| unproven | 2,953,280 | 86.5% |
| gap | 8 | 0.0% |
| 合计 | 3,414,806 | 100% |

说明：2026-11-28 重算（mac.isa.maskmov_ymm_store_fault_crossing 与 mac.isa.vcvtps2ph_mem_fault 由 gap 变 proven，gap 只剩 clwb 的 8 次）；v1 是 proven 5.2%、unproven 94.8%、gap 0.0%。加权结果仍被 unproven 的 avx_general（2,950,582，Rosetta 对 AVX/AVX2 的转译没有逐条差分测试）主导；把它拿掉，其余指令类的 proven 占 99.4%（v1 是 37.9%）。HLE 和加载补丁的频次是调用数或位点数，单位不同，不与指令混合加权，只在各行的 freq 里给出。

## 6. 运行时频次缺口与要加的插桩（没有启动游戏）

- 没有执行剖面：指令频次是静态位点数，不是热路径次数。要加：Linux perf 式采样不可用，建议在 victory-loader 里对 masked/clwb/console_insn 位点加命中计数器（每 5 秒一行日志），并给 cmpxchg 失败路径抽样。
- HLE 调用次数：日志只有汇总（13 条“title memory asked for”分组、79 次 dstorage 文件打开等），没有逐次调用计数。**已加（2026-11-28，PATCHES.md H3）**：`VICTORY_MAC_HLE_COUNTS=<文件>`，每个 HLE 入口一个计数（x86-64 桩，对所有签名通用），按模块和函数每 5 秒写文件、进程退出时再写；默认关。机制已在 Wine 下测过；还没有跑游戏取数（需要 game-lock）。
- console_insn 陷入次数（ClzeroLoop、Monitorx、Mwaitx、Insertq）每帧多少：日志只有“logged once”。要加计数。
- D3DMetal 的 EndQuery/ResolveQueryData/MakeResident 次数：只有“每进程一次”的提示。要加计数。
- Wine 下的 guarded-page 恢复：已做（tools/mac/rosetta-probe-wine.sh，见第 7 节）。还缺：lock cmpxchg 16 位在标题里是否真有未对齐访问（要给 2,036 个位点加地址日志）、audio_client_timer 线程数进 5 秒日志。

## 7. 工具与数据

- tools/mac/census-title-isa.sh/.py：对标题 .text 反汇编做指令类普查（约 2 分钟），输出在 research/census/title-isa-<date>/（游戏派生，不进仓库）。
- tools/mac/census-title-imports.sh/.py：每个导入函数的静态调用点数。
- tools/mac/rosetta-probe.sh + rosetta-probe/probe.c：本机 Rosetta 行为探针（native x86_64 进程，非 Wine）；输出 research/census/rosetta-probe-<date>.txt。
- tools/mac/rosetta-probe-wine.sh + rosetta-probe/probe-wine.c（+ softref.h、summarize-wine.py、tests/validate-softref.c）：把探针交叉编成 Windows exe，在 CrossOver Wine 里用 VEH 实测；一个 case 一个进程（致命错误只杀这个进程，驱动脚本记 FATAL/HANG/CRASH）。CPU 专用，不启动游戏；前缀 target/wineprefix-probe，只 `wineserver -k` 自己的前缀。输出 research/census/rosetta-probe-wine-<date>.txt（+ -summary.md），不进仓库。用法：`tools/mac/wine-tests.sh --rosetta-probe [quick|full]`，单个 case：`tools/mac/rosetta-probe-wine.sh case <case> <mode> [mask=..] [back=N] [b=0x..] [gran=N]`。选了挂进 wine-tests.sh 的选项，不做成 cargo test（探针会让进程被 Rosetta 杀死，且要重新编 C）。数值批次（`case batch bmi|f16c|fma|fma_nan|fma_ftz|atomic_flags|atomic_split`）的参照是纯整数软件模型 softref.h，先用 arm64 硬件校验（tests/validate-softref.c：3563 万例 0 不符）。
- 外部资料（查证日期 2026-11-27）：Apple《About the Rosetta translation environment》（AVX/AVX2 支持、不含 AVX-512）、MacOSX27.2 SDK 头文件 mach/mach_vm.h 与 Metal MTLDevice.h、Wine master dlls/ntdll/unix/virtual.c（VERSION 11.22；无 class 7）。Metal 4 的能力、macOS 28 对 Rosetta 的改动：待核实。

## 8. 矩阵列

`id,chain,hop,unit,owner,used,freq,freq_source,tests,reference,reference_detail,status,disposition,xbox_hw,deviation,restore_plan,notes`（deviation、restore_plan 只对 emulated/stubbed/unsupported 行填写；translated 行的风险与后续写在 notes 里以“Risk:”“Next:”标出）。


## 9. 方法与验收

- 证据等级：hw（实机/实测）、spec（规范）、d3dmetal（D3DMetal 参照）、none。status=proven 要有硬件事实或规范依据，并且有测试或实测。
- reference=d3dmetal 的单元，只有**开场、菜单场景**可以当像素参照；**世界场景下不算已证明**（D3DMetal 在世界里约 750 条管线被标成 no-op，世界帧画出来是纯色，不能当参照）。
- **世界画面的最终验收由用户肉眼判断**；本表里的 proven 不代表世界画面通过。
- 我们自己的 Metal 后端已经画得出 3D 世界帧：research/captures/INDEX.md 里 Metal 抓的第一批 15 份语料，7 份是 3D 世界帧（w2、w48、w98、w298、w300、w600、w1000），w300 是监狱室内第一个可控场景（带 HUD 提示；INDEX.md 仍把这 7 份都归在开场过场计时下）。上面「世界画不出」只指 D3DMetal 的参照。
- 数值和指令语义用软件模型逐位比（模型本身先对 arm64 硬件校验）；Wine 路径下的故障恢复用 VEH 记录异常地址、访问类型、是否恢复。

## 10. 仅参照（D3DMetal）

用户 2026-11-28 决定：以后只走自己的后端，D3DMetal 不再投入。下面这些行只和 D3DMetal 有关，在矩阵 notes 里标「仅参照（D3DMetal）」，不计入第 5 节的覆盖率，也不进第 4 节的偏差排序和第 3 节的缺口表。

| id | 单元 | status | 说明 |
|---|---|---|---|
| mac.hle.cool_fox_residency | 32 GB residency budgets, fallback heaps, GPU pool back-off and deferred uploads under low commit | unproven | D3DMetal does not implement MakeResident (stub), so residency control is a no-op on that host; budgets assume a PC GPU … |
| mac.hle.zero_copy_alias | zero-copy title buffers via mach_vm_remap (copy=FALSE) onto D3DMetal buffer pages | gap | In mac/livetr there is NO zero-copy: every buffer the GPU needs is shadowed and copied (shadow.rs), copy-back after wri… |
| shared.indirect.clamp_dropped | indirect execution dropped when the argument-clamp shader fails to build (HLSL needs SM 6.6 / Inter… | proven | Indirect executions dropped = the world load never ends (hlsl.rs comment). |
| shared.query.timestamps | timestamp queries (EndQuery/ResolveQueryData) used by the title for DRS and profiling | gap | Title sees time 0: DRS/profiling input wrong (see mac.res.drs). |
| shared.query.depth_bounds | depth bounds test reported supported (OPTIONS2 DepthBoundsTestSupported=true on D3DMetal) | unproven | Report says supported but Metal has no depth-bounds test: the draw state is ignored (RDNA2-specific; Metal owner). |
| shared.resource.typed_buffer_r32g32b32 | R32G32B32 typed buffers (D3DMetal: "R32G32B32 typed buffers not supported, using R32G32") | gap | Third component dropped: wrong vertex/texel data for that format. |
| shared.residency.make_resident | MakeResident/Evict ("Unsupported: MakeResident" once per process on D3DMetal) | gap | No-op: budgets enforce nothing (see mac.hle.cool_fox_residency). |
| shared.pipeline.d3dmetal_noop_pso | about 750 pipeline state objects marked no-op by D3DMetal in the world: world frames come out as a … | gap | D3DMetal draws nothing for those pipelines, so the D3DMetal world frame is not a reference for the world scene. |
| shared.draw.set_render_vertex_buffers_crash | SetRenderVertexBuffers crash on D3DMetal (the G7 kind of defect) | gap | 仅参照（D3DMetal）。不计入覆盖率与偏差排序。Placeholder, evidence not examined. |
