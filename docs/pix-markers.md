# GPU 时间线：先按列表，再按 pass

在启动环境设 `VICTORY_PIX_MARKERS=1`；重启进程生效。未设或其他值关闭。release 可用，不需要 diagnostics 或 WinPixEventRuntime。记录的是命令列表上的 BeginEvent/EndEvent（metadata 0、以零结尾的 UTF-16、长度按字节），不是录制线程的 CPU 区间。

## 这条时间线代表什么

基线 `c280afad` 的 `crates/victory-gpu/src/list.rs:3–10` 说明：标题写包指针及限位为零，命令被导向 HLE 方法表；不是扫描原始 PM4/NOP 包。在 crates 中未找到可转发的 PIXBeginEvent/PIXEndEvent 或 push/pop marker 实现。`host.rs::note` 中的 SetMarker 是主机诊断生成的 breadcrumb，并非标题的字符串。这里用结构名称，不推断引擎 pass 意图。`timeline/hooks.rs` 包装 CPU 方法调用；没有找到可直接复用的 gpu-class/whole_timing 枚举；BVH 分类沿用 `pipeline::is_builder`。

一帧没有固定条数。标题录制和提交的边界不等于 Present；`Host::make_list` 创建 DIRECT allocator/list，池化后 reset，`execute_parts` 等按批次组织，`host.rs:5830` 在主 direct queue 提交一组列表。主绘制和普通 compute dispatch 都走该队列。按实际资源依赖可插入 direct 状态前导、CARD 上传/结果及回读列表，relay fetch 可用独立 COPY queue（否则 direct 路径）。`host/awake.rs` 的维护用 COMPUTE 列表并不是标题的计算 pass。辅助探针、replay 和一次性初始化列表不在这套标记覆盖内。

|层级/名称|解释及钩子（相对 crates/victory-gpu/src）|
|---|---|
|`Direct/Main #0`|每次主列表录制的根；`host.rs:4138,6343,6408`|
|↳ `RT/DSV #n`|`draw.rs:1194`；从实际重绑目标到下次重绑或 Close，默认绑定缓存跳过重复设置。它是绑定段，不代表区间内只有图形工作。|
|↳ `Dispatch #n` 或 `BVH/Dispatch #n`|`draw.rs:3206`；嵌在当前目标段（没有则在根下），围住成功绑定后实际发出的直接 Dispatch。n 是列表内 RT 和 dispatch 共用的序号。不拆 ExecuteIndirect 内的工作。|
|`Direct/State prelude #0`|`host.rs:6772`；状态转换及 depth twin 拷贝。|
|`CARD/Refresh #0`、`CARD/Results #0`|`host.rs:4355,4517`；CARD 前置刷新/上传和后置结果处理。|
|`Relay/Readback #0`、`CARD/Readback #0`|`host.rs:4785,5096`；拷入主机可读缓冲区，不包括 CPU 等待或映射。|

## 开销估算与验收

设一帧执行 L 个主列表、A 个上述辅助列表、R 个实际 RT 重绑、D 次直接 dispatch，则有约 **L+A+R+D** 对事件。例：10/6/40/20 对应 76 对，不是实测。复用录好的列表时每次执行都在 GPU 时间线上出现，但只在录制时付 CPU 代价。嵌套区间不可直接相加。关闭时，稳定录制路径每个主列表开关两处、每个 RT 或 dispatch 一处读取列表内缓存标志并分支；辅助列表头尾各一次检查（入口 OnceLock 快路径、出口 Option guard）。上述例子约 92 次检查，不按 draw 数放大；无格式化、分配、标记锁或 COM 事件调用。开启时有每列表状态锁和栈上 UTF-16 转码，无每事件堆分配。

第一张票先以 `nsys profile --help` 确认该 Windows 版本支持的 provider，再在既有 bench 启动链上配置 `nsys profile --trace=dx12,dx12-annotations`。在两次 Present 间展开 **GPU** direct/copy queue 及 annotation 范围，不把 CPU API duration 当 GPU 时间。Nsight Graphics 的 GPU Trace/Frame Debugger 中展开同名事件核对包围的 draw、dispatch 和 copy。若只见 CPU 事件，不能报 GPU 时长。相同热缓存、固定镜头、相同采集设置紧邻对照关/开，核对工作量、CPU 录制耗时和 GPU 帧时间；另用无 profiler 对照区分采集扰动。离线验证不代表 Nsight 已识别标记或开销已测定。
