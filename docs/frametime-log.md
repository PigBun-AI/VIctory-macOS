# 按游戏帧号截取 Present 耗时（Windows release）

启动前在 PowerShell 设 `$env:VICTORY_FRAMETIME_LOG = 'D:\logs\opening.csv'`，父目录须存在。不设即关闭；环境变量只在首次调用时读取，之后修改须重启。关闭后的钩子只检查缓存，不分配、不锁、不读时钟或 draw 计数。不需要 diagnostics。

CSV 以追加模式写出，空文件写 `frame,frame_us,draws` 表头。建议每次游戏运行用新路径，避免不同场次的帧号重复。

- `frame`：`queue::FRAMES.fetch_add(1) + 1`，经 `Shown.frame` 传给宿主 Present；与游戏日志 `frame presented (since the last line: ...)` 的 `frame` 完全同源。同步和异步提交都从呈现端取样；没有走到宿主 Present 的游戏帧不补行。
- `frame_us`：本次 `host.present_planes` 返回与上一次返回的单调时钟 `Instant` 间隔，整数微秒。含等待和宿主侧工作，不是 GPU 执行时间。首行没有上一帧，记 0，统计应排除。按当前行帧号截取时，区间左端第一行的耗时从其前一个 Present 算起。
- `draws`：标题栏 `fps / ms/frame / draws/frame` 所用的宿主累计 `crate::draw::drawn()` 在两次采样间的差值；首行从进程初始的 0 算起。

每 4096 行或距上次写出满 1 秒时，由呈现线程在下一次采样批量写出；正常 CRT 退出冲刷尾批。强杀可能丢掉尾批。打开或写出失败会警告一次并停止记录，不影响继续游戏。
