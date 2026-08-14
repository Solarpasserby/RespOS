# RespOS 已确认易错点

## open-file 目录项缓存不能证明目录仍在 namespace

- 状态：已确认并对 ext4 修复
- 适用范围：`getdents64`、open directory fd、`rmdir`、deferred inode reclaim
- 最后验证：2026-08-14
- 证据：`scripts/getdents_unlinked_probe_linux.c`、双架构 musl/glibc `getdents01/getdents02`
- 内容：为保持一次目录遍历的 d_off 一致性，`File` 会缓存 readdir 快照；但目录
  被 `rmdir` 后，open fd 的 inode 只因 Arc 而继续存活，已不属于 namespace。若先返回
  缓存或从 deferred lower inode 重建，Linux 要求的 `ENOENT` 会被伪装成成功的 `.`/`..`。
- 后续影响：在读任何 open-file namespace cache 前先检查所有者的 detached/unlinked 状态；
  不得用 pathname lookup 代替 inode identity。添加新的可 unlink 内存目录后端时，必须同步
  实现该状态，不能只修 ext4 特判后宣称通用 VFS 已闭环。

## libc 在用户态丢弃接口参数时，内核无法补回 POSIX 错误语义

- 状态：已确认；当前 musl runtime 替换待协商
- 适用范围：libc 组合接口、镜像内 musl/glibc 差异、LTP `pathconf02/readlink03/readlinkat02`
- 最后验证：2026-08-14
- 证据：当前 RV64/LA64 初赛镜像 `/musl/lib/libc.so` 的
  `pathconf/fpathconf/readlink/readlinkat` 符号反汇编；
  `rv-output.txt`/`la-output.txt` 中 musl/glibc `pathconf02/readlink03/readlinkat02` 对照
- 内容：当前镜像的 musl `pathconf(path, name)` 不访问 `path`，而是传
  `fpathconf(-1, name)` 后返回常量表。因此 `ENOTDIR/ENOENT/ENAMETOOLONG/EACCES/ELOOP`
  不可能由内核 namei 返回；即使修改 `statfs` 或路径解析也不会影响这条调用链。
- 内容：LA64 musl 1.2.5 的 `readlink/readlinkat` 又展示了另一种形态：wrapper 把零长度
  转换成内部 1-byte 缓冲区调用，使内核只能看到合法的截断读。内核既不能恢复原始
  `bufsiz==0`，也不能把所有 size 1 请求改为 `EINVAL`。RV64 musl 1.2.0 直接传参，所以
  同一内核上只有 LA64 musl 失败。
- 后续影响：遇到只在一种 libc 复现的接口差异时，先从实际测试镜像导出 libc，
  确认参数是否到达 syscall，再决定内核所有者。不得特判 LTP runner；替换整套 libc
  必须单独协商并跑完整相关 workload。

## LA64 SMP 不能直接跨 hart 比较未归一化的 `rdtime.d` deadline

- 状态：可观察差异与 affinity A/B 已确认；归一化实现待协商
- 适用范围：LA64 SMP socket/nanosleep/futex/poll timeout、用户 monotonic time、CPU accounting
- 最后验证：2026-08-14
- 证据：50 ms `socket_timeout_probe` 在 LA64 单核和固定 hart0 通过，2 hart/固定 hart1 稳定约
  979--981 ms；`/tmp/respos-la-socket-timeout-{smp1,smp2-r2,hart0,hart1}.log`
- 内容：当前 task 以本 hart 的 `rdtime.d` 换算绝对微秒 deadline，timer-service hart 直接读取同一数值
  编程和扫描。QEMU LA64 各 hart 的可观察时间域存在约 1 秒偏移时，从非服务 hart 发布的 50 ms
  deadline 会在服务 hart 看来位于约 1 秒以后。单核通过、放宽超时上界或把任务固定到 hart0 都不能
  证明 SMP timeout 正确。
- 后续影响：应在 secondary boot 建立 per-hart offset 并统一到一个全局单调时间域，同时让本地硬件
  timer 按相对 tick 编程。修复必须重跑跨 hart timeout、迁移 clock、CPU accounting 和双架构门禁；
  不得用 affinity、1 秒轮询或扩大容差作为正式修复。精确 offset 校准协议完成前，根因实现细节保持
  `待验证`。

## 100 Hz 扫描会把亚 10 ms timeout 量化到下一 tick

- 状态：已确认并对 task/futex deadline 修复
- 适用范围：nanosleep、poll/select/epoll、futex；RV64/LA64 QEMU
- 最后验证：2026-08-13
- 证据：八个 LTP case 修复前四环境共同失败、1 ms 请求约 10.2 ms；修复后四环境全通过
- 内容：用户可见时钟达到微秒分辨率，不代表阻塞唤醒也有同样精度。若 timeout 只被 100 Hz timer
  scan 消费，1/2/5 ms 都会等到约 10 ms。单纯把 deadline 从毫秒改成微秒不能消除扫描量化；单纯设置
  精确 QEMU compare 也可能因注入延迟超出 LTP 容差。
- 后续影响：同时检查 deadline 表示、硬件 rearm、SMP 服务 hart 通知和 QEMU 注入延迟。提前 compare
  必须配合有界等待到权威软件 deadline，不能以提前唤醒换取通过；不要先全局提高 tick 频率掩盖问题。

## 用户 root 创建后新增内核根 PTE 会在映射本身成功后继续缺页

- 状态：已确认并修复
- 适用范围：共享高半区、动态 kernel stack、RV64/LA64 三级页表
- 最后验证：2026-08-13
- 证据：RV64 完整初赛 slot 16384 的 StorePageFault；强制 slot 16383→16384 专项
- 内容：用户 root 只复制内核 root PTE 的当时值。若动态映射跨入此前为空的 1 GiB 根分支，
  `KERNEL_SPACE` 可以成功建表并映射 leaf，但旧用户 root 的对应根项仍无效；普通 sfence 不能把一个
  不存在的 PTE 复制过去。坏地址可能精确落在新内核栈的 trap context，看起来像 clone/memcpy 崩溃。
- 后续影响：先在 boot 阶段固定高半区 root 拓扑，再允许运行期按需修改共享下级表。排查高编号栈或
  动态内核 VA fault 时，同时计算 root/PMD 边界，不能只检查物理 frame 是否已经分配。

## devfs 自定义 inode 缺少 owner setter 会让 create 被 namei 原子回滚

- 状态：已确认并修复 `/dev/shm`
- 适用范围：namei create、devfs/tmpfs 风格 inode、LTP newlib、libctest 临时文件
- 最后验证：2026-08-13
- 证据：`os/src/fs/namei.rs::install_created_inode()`、`os/src/fs/dev/shm.rs`；双架构
  `confstr01/openat02` 与 RV64 libctest-only 日志
- 内容：create 的 lower inode 分配成功不等于 syscall 成功；namei 还要提交 mode/uid/gid。trait 默认
  `set_owner()` 返回 `EINVAL`，因此只实现 create/read/write/set_mode 的 shm inode 会被回滚，并使
  `open(O_CREAT)`、`mkstemp()`、`tmpfile()` 和 LTP harness 同时失败。初赛 runner 又把 `/tmp` 链到
  `/dev/shm`，会让问题伪装成两个文件系统同时故障。
- 后续影响：任何可创建 inode 的内存/devfs 后端都要实现并由 stat 回报 create 协议要求的元数据；
  遇到大量临时文件 `EINVAL` 时先解析 mount/symlink 后的真实后端，再检查首个 metadata commit。

## 初赛镜像的 `/tmp` 默认指向轻量 `/dev/shm`，创建修复不等于完整 tmpfs 语义

- 状态：已确认并通过 runner 的 ext4 `/tmp` 隔离
- 适用范围：初赛镜像、libcbench、libctest、LTP 文件系统测例
- 最后验证：2026-08-13
- 证据：当前 RV64/LA64 初赛根镜像 `debugfs stat /tmp`；双架构 7-case musl/glibc 聚焦日志
- 内容：官方初赛镜像本身就把 `/tmp` 链到 `/dev/shm`，不能只根据 testrunner 的
  `prepare_libcbench_tmp()` 判断链接来源。shm inode 实现 owner/mode 后能创建普通文件，但仍缺少完整
  timestamp、symlink、hardlink、FIFO 和 xattr 语义，因而 LTP 会从“全部创建 EINVAL”变成多类分散失败。
  当前 runner 在 libcbench 后把 `/tmp` 建成根 ext4 上的 `01777` 目录，LTP-only 也执行该准备。
- 后续影响：完整 tmpfs 是独立实现任务，不能把 ext4 runner 隔离宣称为 `/dev/shm` 已完整兼容。若日志
  再出现 `/dev/shm/LTP_*`，先确认实际 `/tmp` 后端和 runner 入口，再分析具体 syscall。

## requested 特殊 `InodeType` 不等于 ext4 已持久化特殊 inode

- 状态：已确认并修复 FIFO/character/block/socket
- 适用范围：ext4 create、mknod/mkfifo、特殊节点的 stat/xattr 与命名 FIFO errno
- 最后验证：2026-08-14
- 证据：修复前 FIFO 五项 LTP 及 `setxattr02` character/block/socket 共同失败；修复后双架构
  musl/glibc 的 FIFO 簇和 13-case mknod/xattr 簇全通过
- 内容：旧 create 根据 requested type 构造 Rust inode，却用普通文件 `O_CREAT` 写 lower inode；后续
  lookup 以磁盘 mode 为准，又把对象恢复成 regular。结果不是单个 syscall 分支错误，而是 stat type
  与 rdev 错误、character/block/socket 接受 `user.*` xattr，以及 FIFO 的 nonblock/lseek/fsync/pipe
  capacity 语义一起偏离。
- 后续影响：遇到多个特殊文件 syscall 同时失败，先检查 lower inode mode 与 reopen 后 node_type，
  再检查 device payload 是否真正传入 lower inode；不要在 xattr 或各 syscall 层按 pathname/type 打补丁。
  setattr 只保留/更新权限位时尤其不能用它补救错误的 type。

## `statx` 的 device minor 不能只取 legacy 低 8 位

- 状态：已确认并修复
- 适用范围：`statx.stx_{rdev,dev}_{major,minor}`、ext4/device fs 的 device number
- 最后验证：2026-08-14
- 证据：`scripts/mknod_dev_t_probe_linux.c`、`mknod_xattr_probe`；双架构 2 hart 专项与
  `mknod01,setxattr02,statx02,statx03` 四环境回归
- 内容：Linux kernel/ext4 的 32-bit device encoding 为 12-bit major、20-bit minor；低 8 位兼容旧编码，
  minor 的其余 12 位位于 device value 的高段。直接使用 `rdev & 0xff` 会让 `stat()` 看似正确，
  `statx()` 却静默截断高 minor，因此普通低 minor 设备和既有 LTP 无法暴露问题。
- 后续影响：device 编解码必须共用 Linux 布局，probe 至少选择一个 minor 大于 255 的节点并同时比较
  raw `st_rdev` 和 statx 拆分字段；不得用现有 `/dev/null` 等低 minor 节点宣称完整验证。

## 直接从 `wait4` 返回 EINTR 会绕过 SA_RESTART 并放大成 LTP 任务泄漏

- 状态：已确认并对 `wait4` 修复
- 适用范围：signal handler、`wait4/waitpid`、LTP newlib harness
- 最后验证：2026-08-13
- 证据：完整初赛日志中 RV64/LA64 的 259/211 次 `waitpid(...)=EINTR` 与 task health 增长；双架构专项
- 内容：用户 handler 由 `signal()` 安装时通常带 `SA_RESTART`。若内核仍把 interruptible wait 的
  `EINTR` 直接返回用户态，LTP 的 `SAFE_WAITPID` 会 TBROK，清理父进程提前退出，后代继续运行并把输出
  串入后续 case；这会表现为大量不相关失败、ready/blocked task 单调增长甚至晚期假死。
- 后续影响：不能在 wait 内核循环里简单忽略信号，因为 handler 必须先执行；应在 signal frame 中保存
  重执行上下文。分析大量 LTP TBROK 时同时看 `SA_RESTART`、task health 和跨 case 输出串台。

## per-hart allocator cache 不能假设原 hart 释放，也不能反转 cache/buddy 锁序

- 状态：已确认并由 A1 实现/测试约束
- 适用范围：`heap_magazine`、raw-order allocator API、跨 hart free、OOM recovery、heap proc 统计
- 最后验证：2026-08-13
- 证据：allocator 全小类/跨 owner/随机/OOM/coalesce host 单测；RV64 8-hart shared-MM、frame reclaim、
  显式 drain 后再次 shared-MM；LA64 30 秒 BuildStorm magazine 计数
- 内容：任务可以迁移，分配与释放不保证发生在同一 hart；cache 中 block 仍由 allocator 独占，不能
  同时出现在 buddy free bitmap。正常 miss/overflow 持本地 magazine 后才持 buddy，因此全 hart OOM
  drain 若先持 buddy 再等待某个 magazine 会形成 ABBA 死锁。当前所有路径统一为 `magazine -> buddy`，
  跨 hart free 进入执行释放的 hart；live requested、buddy reserved 和 cached bytes 分开记账。为避免
  命中路径更新共享 peak，magazine 模式显式输出 `heap_peak_exact=0`。
- 后续影响：不得把 owner hart 编入对象头、关闭 coalesce、让 cache 无界增长，或用伪造 Layout 调用普通
  dealloc。修改容量/refill/order 后必须重跑所有权随机测试、OOM drain/retry、最终大块 coalesce、双架构
  构建与跨核 probe；正式收益只看关闭 `perf_counters` 的 A/B。

## BuildStorm 前段 allocator 短窗口不能代表主 release 阶段

- 状态：已确认；当前 A1 magazine 完整收益仍为 No-Go
- 适用范围：allocator/锁/TLB/FS 等改变不同编译阶段成本的 BuildStorm A/B
- 最后验证：2026-08-13，8 GiB/12 hart 相邻完整 A/B 与记账降本完整复测
- 证据：LA 70 秒两对、120 秒一对无 perf A/B；8 GiB/12 hart 完整 off/旧 A1/降本 A1 时间线；一轮
  12 GiB 2366 秒未完成异常样本
- 内容：旧 A1 在 70/120 秒前段把 dev 阶段改善约 5--11%，但同配置完整 8 GiB A/B 中 axbuild 为
  `1335.45s` 对 baseline `1281.89s`，反而慢 `4.18%`。把每 hit/free 的 live/cached 两次原子并入本地
  magazine 锁后，中窗口到 34/35 marker 比 baseline 提前约 267/87 秒，但完整 axbuild `1318.37s`
  仍比 `1281.89s` 慢 `2.85%`。编译依赖图在不同阶段只提供约 1、3、8 或 12 个 runnable rustc，marker
  会长时间不变；短窗口或某个 checkpoint 的方向不能替代完整结果。
- 后续影响：短窗口只作候选筛选；影响高频基础设施的修改在完整测试前必须建立覆盖主 release 稳态的
  中窗口或阶段 checkpoint。完整轮已经显著落后且进度不足时可终止并标为反证，但不得把未完成轮写成
  最终墙钟成绩。没有相同内存、镜像、feature、宿主换页状态的完整相邻 A/B 时，不得因前段改善或历史
  不同配置数字默认启用 feature。

## per-hart 不等于无同步：远端回收要求保留 owner 协议

- 状态：已确认当前锁边界；无锁 owner-hart drain 方案待实现
- 适用范围：per-hart heap magazine、OOM recovery、proc heap 统计、跨 hart free
- 最后验证：2026-08-13
- 证据：LA 无 perf 热路径反汇编；8 GiB 完整 off/旧 A1/记账降本 A/B；RV64 8-hart drain 回归
- 内容：旧 A1 的本地 cache hit 仍执行 magazine mutex、cached bytes 和 live bytes 共三次 LA 原子；
  将后两者并入同一锁后，完整退化由 `4.18%` 收窄到 `2.85%`，但唯一的 mutex 原子仍在。不能据此把
  state 直接改成 `UnsafeCell`：OOM drain 和统计读取可能从另一 hart 访问，和 owner hart 的 push/pop
  并发会产生数据竞争、丢块或重复释放。仅关闭远端 drain 也会让被 cache 保留的可用内存造成伪 OOM。
- 后续影响：若做 A3，无原子普通路径必须让每个 owner hart 在 IPI 或明确安全点自行转移其 cache，并
  有同步完成/离线 hart/中断嵌套协议；失败后仍需 drain 后重试，且继续覆盖跨 hart free、最终 coalesce、
  双架构和完整 A/B。在该协议闭合前保留当前 mutex 和默认关闭 feature。

## 高频性能计数器本身会把共享 cache line 变成伪热点

- 状态：已确认并修复 heap 计数结构；LA 重校准待验证
- 适用范围：`perf_counters`、多 hart allocator/scheduler/锁热路径、BuildStorm A/B
- 最后验证：2026-08-13
- 证据：LA 相邻 130 秒 perf/no-feature 窗口；`os/src/perf.rs` 的 heap per-hart shards；RV64 8-hart
  `smp_shared_mm_probe` 与闭合输出
- 内容：Relaxed atomic 只放宽内存顺序，不消除多个 hart 对同一 cache line 的所有权争抢。第一版
  heap size-class 全局原子使 dev/core 里程碑慢约 17--22%，已经大于要测的优化收益。当前 heap 高频
  总量和分桶按 hart 分片，峰值在已有 allocator 临界区中维护；仅分片后的相邻样本仍有 5--13% 的
  dev 阶段扰动，因此硬件时钟计时进一步改为每 hart 1/64 抽样估算；完整 feature 的 sampled/no-feature
  单轮仍相差约 11--16%，说明其他逐事件计数和宿主波动也不可忽略。旧完整日志中的全局计数绝对时间
  不能与新版本直接 A/B，完整 `perf_counters` 也不能用于判断小幅墙钟收益。
- 后续影响：为每次 syscall、alloc、fault、lock 或 timer 增加计数前，先决定是否按 hart/CPU 分片；
  不得用“仅 Relaxed”声称低开销。观测 feature 必须与 no-feature 做同阶段校准；超过 3% 时计数只作
  结构/数量级诊断，优化收益改由关闭观测 feature 的生产路径 before/after A/B 决定。

## 辅助盘 profile 不会随平台根镜像变化，线上阶段不能固定为 final

- 状态：已确认；通过 `mode=auto` 与根盘标志检测修复
- 适用范围：`respos/profile`、`disk*.img`、`contest_launcher`、初赛复测与决赛评分
- 最后验证：2026-08-13
- 证据：x0/x1 挂载关系；RV64/LA64 四份官方镜像的脚本检查；同一 auto 辅助盘的双架构四镜像启动；
  RV64 决赛不挂载 x1 的启动日志
- 内容：比赛方替换作为 x0 的官方根镜像不会改写我们作为 x1 提交的辅助盘。若 x1 固定
  `mode=final`，平台在初赛复测中仍挂载 x1 时会错误启动决赛脚本。当前线上 profile 使用
  `mode=auto`：优先检查 CAgent/BuildStorm 决赛脚本，再检查 musl/glibc basic 初赛脚本；profile
  缺失、空白或无效也走自动检测，未知根盘告警后回退 preliminary。本地显式 profile 仍可强制阶段。
- 后续影响：不得把 `make all` 的 profile 改回固定 preliminary/final，也不能根据辅助盘名或 QEMU
  资源猜阶段。官方镜像脚本路径发生变化时，必须先取得新镜像/公告证据，再更新检测标志并完成
  双架构两阶段启动矩阵。

## Rust 版本、LTO 和诊断 feature 会通过栈布局暴露或掩盖 VirtIO 跨页 DMA 错误

- 状态：根因已确认并由 selective bounce 修复；RV64 完整 final 已通过
- 适用范围：`os/cargo/config-{riscv64,loongarch64}.toml`、线上 `make all`、决赛动态 glibc workload
- 最后验证：2026-08-14
- 证据：同源码/镜像/QEMU 的 rustc 1.86 thin-LTO、rustc 1.86 no-LTO、rustc 1.89 thin-LTO A/B；
  RV64 CAgent/BuildStorm 前置门禁；QEMU/GDB 的 16 B `BlkReq` 页尾地址和物理内存检查；
  `/tmp/respos-rv-{kstack-shootdown,virtio-bounce,virtio-bounce-diag}.log`
- 内容：早期 A/B 中 Rust 1.86 thin-LTO 失败、1.86 no-LTO 或 Rust 1.89 成功，曾被误判为工具链代码生成
  问题。新复现显示 no-LTO 加 `fault_trace` 也会失败：实际取决于 16 B `BlkReq` 是否落在页尾
  `...dff8`。旧 HAL 只翻译首虚拟地址，sector 字段落入非连续的下一物理页后，设备读成 sector 0；
  脚本、动态库或编译器因此收到错误磁盘内容。关闭 LTO 和增加 feature 只是改变栈布局，不能作为修复。
  当前 HAL 对整个范围验证物理连续性，仅对非连续范围建立并按方向回收 bounce buffer。
- 后续影响：线上兼容门禁不能止于“Rust 1.86 编译成功”，至少要用该工具链产物启动动态 glibc 程序。
  `Hal::share()` 不能假设任意 Rust slice 物理连续，也不能只给 `BlkReq` 加对齐来掩盖数据 buffer 的同类
  风险。当前 `lto=false` 继续作为保守提交配置；若为性能恢复 LTO，必须在 selective bounce 下重新完成
  平台同版 RV/LA CAgent、BuildStorm toolchain/minibuild 和完整构建，而不能只用较新本机 rustc。

## VS Code bundled rust-analyzer 必须与镜像 Rust toolchain 匹配

- 状态：已确认；容器重建后生效
- 适用范围：`.devcontainer/Dockerfile`、VS Code Rust Analyzer extension
- 最后验证：2026-08-13
- 证据：扩展 `rust-analyzer 0.3.3008-standalone` 状态提示要求 `rustc >= 1.94`，当前镜像默认
  `nightly-2025-05-20` 实际为 `rustc 1.89.0-nightly (2025-05-19)`。
- 内容：Dockerfile 在 root 构建阶段为 `nightly-2025-05-20` 安装 `rust-analyzer` component；
  `.vscode/settings.json` 显式指定该 component 的二进制路径。不要让扩展的 standalone server
  代替工具链配套的 component，也不要仅为 IDE 提示升级默认 Rust，因为提交兼容性仍以
  `nightly-2025-01-18` 复现的比赛 Rust 1.86 为准。
- 后续影响：修改 Dockerfile 后必须执行 Dev Containers: Rebuild Container，不能只 Reload Window。

## rust-analyzer 的自动检查会与 lwext4 CMake 构建脚本竞争同一源码目录

- 状态：已确认并通过 IDE 配置规避
- 适用范围：VS Code rust-analyzer、`vendor/lwext4_rust/build.rs`、正常 `make` 构建
- 最后验证：2026-08-13
- 证据：rust-analyzer Language Server 日志中的 Flycheck 命令与
  `vendor/lwext4_rust/c/lwext4/build_musl-generic/CMakeFiles/...o.d` 缺失错误；该 build script
  每次执行都会删除并重建源码树中的 `build_musl-generic`。
- 内容：rust-analyzer 的 `cargo check` 和前台 `make` 共用该 CMake 输出路径时，任一方可删除
  另一方正在写入的依赖文件，表现为编辑器持续 `Flycheck failed`，但不是 Rust 源码错误。
  `.vscode/settings.json` 为分析器关闭 build scripts、save 时 Flycheck 和原生诊断，并使用独立
  target directory；后者规避 bundled rust-analyzer 0.3.3008 对本 `no_std` 交叉目标触发的
  `inference diagnostic in desugared expr` 内部错误。代码导航、补全和格式化仍可用，实际
  RV64/LA64 验证必须使用顶层 Makefile。
- 后续影响：不要在未把 lwext4 CMake 输出移到 Cargo `OUT_DIR` 前重新启用
  `rust-analyzer.checkOnSave`。需要即时编译诊断时，应显式顺序运行 `make build-rv` 或
  `make build-la`，不要与另一个构建并行。

## `build-rv` 与 `build-la` 不能并行共享可变 Cargo 配置

- 状态：已确认；串行重建可恢复
- 适用范围：顶层 Makefile、本地双架构验证、`make -j`
- 最后验证：2026-08-12
- 证据：并行执行两个目标后 RV 首条用户指令为 LA 编码并触发 IllegalInstruction；单独
  `make build-rv` 后相同 RV 8-hart 命令正常进入 shell 且 shared-MM 100 轮通过
- 内容：两个目标都会覆盖 `os/.cargo/config.toml` 与 `user/.cargo/config.toml`。并行构建会竞态选择
  target/linker/rustflags，并可能把另一架构用户程序嵌入内核；不要并行运行这两个目标。
- 后续影响：正式 `make all` 不加 `-j` 时按依赖顺序执行；若要支持并行，必须改为互不共享的
  `--config`/`CARGO_HOME` 或 target-specific 配置，不能仅靠清理产物掩盖竞态。

## LoongArch `csrwr` 会改写源寄存器，不能连续复用同一页表 root 临时寄存器

- 状态：已确认并修复
- 适用范围：LA context switch、PGDL/PGDH、SMP、内核高半区
- 最后验证：2026-08-11
- 证据：QEMU/GDB 在 BuildStorm 卡死现场观察到 PGDL 与 PGDH 不同；
  `os/src/arch/loongarch64/task/switch.S`
- 内容：`csrwr rd, csr` 不只是写 CSR，还把 CSR 原值回写 `rd`。旧 `__switch` 先后用同一个
  `$t0` 写 PGDL 和 PGDH，第二条实际把旧 PGDL 写进 PGDH，形成用户低半区使用新 root、内核
  高半区使用旧 root 的 split-root。单核时全局 TLB 残留可能掩盖问题；SMP/频繁切换会最终在内核
  trap 或 scheduler 中以页故障、持锁 hart 消失、其他 hart 自旋的形式暴露。
- 后续影响：同一值写多个 CSR 前必须先复制到不同通用寄存器，或每次重新加载；审查所有
  `csrwr` 序列时都要把源寄存器视为读写操作数。页表切换回归必须同时读取 PGDL/PGDH，不能只看
  用户态是否暂时继续执行。

## LoongArch `INVTLB op=3` 不是按 ASID 失效，不能接在 op=0 后重复执行

- 状态：已确认并修复重复失效
- 适用范围：LA task switch、`sfence()`、ASID/global TLB 设计
- 最后验证：2026-08-12
- 证据：[LoongArch Reference Manual Volume 1](https://loongson.github.io/LoongArch-Documentation/LoongArch-Vol1-EN.html#invtlb)、
  `os/src/arch/loongarch64/{task/switch.S,register/mod.rs}`；公开 LA 决赛镜像 12 hart 限时回归
- 内容：架构定义中 op=0 清除所有 TLB 项，op=3 清除所有 `G=0` 项，按指定 ASID 清除非 global
  项的是 op=4。旧代码连续执行 op=0、op=3，第二条只重复清理已经为空的子集。当前保留 op=0，
  因而删除 op=3 不改变“全量本地失效”语义。
- 后续影响：不能依据错误 opcode 注释设计 ASID 优化。当前按 ASID/range 请求安全使用 op=4；
  Global kernel mapping 仍须验证 software refill、成对 G 位、ASID 生命周期和跨核 shootdown。
  op=5 已在完整 BuildStorm 中造成 allocator corruption，不能仅凭短专项重新启用。

## LoongArch 4 KiB Global pair 规则不能外推为 huge-leaf bit 位置

- 状态：已确认；当前显式拒绝 huge Global mapping
- 适用范围：LA 2 MiB direct map、`PageTable::map_huge_2m()`、Global kernel mapping
- 最后验证：2026-08-14
- 证据：`os/src/arch/loongarch64/mm/page_table.rs`；`/tmp/respos-la-global-probe.log`
- 内容：把未经真实访问验证的 `PMD_HGLOBAL=bit 12` 写入 2 MiB 高端 RAM leaf 后，系统在启动后的
  首次高端内存写立即触发 `PageInvalidStore`，badaddr `0xffffffc090400e70`。4 KiB TLB pair 要求
  TLBELO0/1 的 G 位同时成立，这不能证明目录 huge leaf 使用同一位或可由软件页表直接编码。4 KiB
  paired-leaf 实验虽通过正确性门禁，首组 on--off--on 完整性能为
  `1560.36/1410.58/1634.28s`；更稳定宿主单对复测又为 `1530.37/1418.31s`，但换页量不一致且没有
  R3。当前只保留默认关闭的 4 KiB 候选；`map_huge_2m()` 收到 `PTEFlags::GLOBAL` 返回 `EINVAL`。
- 后续影响：不得凭空恢复 bit 12 或把普通 `PTE_G` 搬到任意“空闲”物理地址位。若以后重启 huge
  Global，必须先以架构手册/硬件页表格式确定编码，再单独覆盖低/高 RAM 读写、DMA、跨 ASID 和完整
  final；短启动或只访问低内存不足以验收。

## 启用 ASID 后当前 active mask 不覆盖全部潜在旧 TLB

- 状态：已确认并通过 residency mask 修复
- 适用范围：LA ASID、task migration、PTE update、远端 shootdown
- 最后验证：2026-08-12
- 证据：`os/src/mm/memory_set.rs`；LA 12-hart 双 shared-MM、Phase3、1200 短进程与 perf 窗口
- 内容：hart 切到 idle 后会清除 active bit，但普通 ASID 切换不会清除该 hart 的旧 TLB。只向当前
  active mask 发送 PTE shootdown 会让任务以后迁回该 hart 时重新命中旧映射。必须记录自上次同步失效
  后加载过该 ASID 的 residency，或在每次重新激活时按 generation 补做本地失效。
- 后续影响：该故障已通过 per-PageTable retired frame 批次和同步 completion 修复；不得恢复跨
  MemorySet 的全局 retired-frame 队列，也不能绕过 residency ack 提前释放旧 frame。

## RV64 最大支持 RAM 必须同时覆盖 early FDT 和正式 direct map

## LoongArch refill 的“无效 TLB 项”不得带 V/D 有效位

- 状态：已确认并修复
- 适用范围：LA TLB refill、惰性 VMA、filesystem ELF exec
- 最后验证：2026-08-11
- 证据：LA pub `/glibc/busybox` 的 GOT 应为 `0x12018e064`，故障值
  `0x4c0000c128f300c6` 实际来自 ELF 首页偏移 `0xee0`；预取可写 PT_LOAD 后 BusyBox 正常输出；
  修复 refill 后在 `-m 12G -smp 12` 下进入 `cagent-glibc` group。
- 内容：原 `construct_invalid` 对 `TLBREHI` 做位旋转并 OR `3`，生成了带 `V/D` 的表项。目录缺失
  时硬件因此不会进入普通 page-fault handler，而会把未映射虚拟页别名到错误物理页。无效项应向
  `TLBRELO0/1` 明确写零；`TLBREHI` 已由硬件保存 fault VPPN/PS，不要用普通 `TLBEHI` 替代。
- 后续影响：验证 LA 惰性映射时不能只检查软件页表；必须确认缺页确实到达 Rust handler。预取所有
  ELF 可写段只能掩盖 refill 缺陷，会增加启动 I/O 和常驻内存，不能作为正式修复。

## 开启 LoongArch FPU/LSX 后必须保存任务扩展上下文

- 状态：已确认并修复 user trap/task 隔离
- 适用范围：LA `EUEN.FPE/SXE`、Rust/glibc 大型动态程序、timer/syscall/task switch
- 最后验证：2026-08-12
- 证据：修复前直接 `rustc --version`、`cargo` 与 BuildStorm 稳定 SIGSEGV；新增 eager
  FP/LSX/FCSR/FCC 保存恢复后 CAgent 10/10、toolchain 和 minibuild 通过；首次使用门控后 12-hart
  CAgent 10/10、shared-MM 100 轮和 Phase3 30 轮继续通过。
- 内容：只打开 EUEN 而不保存寄存器会让用户扩展状态跨异步 trap 和任务切换串扰；崩溃可能表现为
  普通 load 的坏指针，而不是 illegal instruction 或浮点异常。关闭 timer 抢占不能完整规避 syscall/阻塞
  调度，也不能代替架构上下文实现。首次使用优化必须在 user trap entry 判断 EUEN 后再执行向量指令；
  进入 Rust 内核前则要重新启用内核扩展，避免在保存路径内形成嵌套 unavailable trap。
- 后续影响：新增架构扩展时同时审计 enable、trap save/restore、fork/exec、signal frame 和 ptrace。当前
  signal mcontext 尚缺 FP/LSX 扩展；当前只是 per-task first-use gating，不是跨 hart lazy-owner。

## 内核堆存储不能作为巨型静态 BSS

- 状态：已确认并修复无串口启动失败
- 适用范围：两架构 `KERNEL_HEAP_SIZE`、链接布局、`clear_bss()`、early direct map
- 最后验证：2026-08-11
- 证据：`kernel-la` program headers；QEMU 10.0.2 `-d in_asm,guest_errors`；
  `os/src/arch/loongarch64/{config/mm.rs,mod.rs}`
- 内容：256 MiB 静态内核堆使 BSS 结束于约 `0x10aa7000`，而启动页表只覆盖前 128 MiB，
  `clear_bss()` 尚未建立最终页表就访问越界并最终执行地址 0，表现为完全没有串口输出。
  临时降为 64 MiB 后 LA 恢复启动；当前进一步把 buddy bitmap/heap storage 移到 `ekernel` 后的
  启动期物理预留区，RV64/LA64 ELF 均不再携带巨型 BSS。
- 后续影响：frame allocator 的下界必须是完整 heap 预留末端而不是 `ekernel`，否则会把仍在使用的
  heap 页重复分配。预留区本身必须落在实际 RAM 和 early direct map 内；LoongArch QEMU 4 GiB
  RAM 含 `0x10000000..0x80000000` PCI/MMIO 空洞，frame allocator 和正式 direct map 都必须按
  low/high 两段处理，不能把 `MEMORY_END` 简单改成连续 4 GiB。

## RV64 `-m 16G` 的 FDT 在当前 early map 外

- 状态：已确认并修复到 16 GiB
- 适用范围：RV64 QEMU 内存配置、early page table、FDT 解析、direct map
- 最后验证：2026-08-11
- 证据：QEMU 10.0.2/OpenSBI 1.5.1、`-m 16G -smp 8`、FDT `0x47fe00000`；
  `os/src/arch/rv64/{entry/entry.asm,config/board.rs}`；完整 BuildStorm `ok=true`。
- 内容：原 8 GiB early root 只映射 `0x80000000..0x280000000`，内核在解析真实内存范围前
  就无法访问 16 GiB RAM 顶部的 FDT。修复后 identity/高半区各用 16 个 1 GiB leaf 覆盖
  `0x80000000..0x480000000`，FDT 实际末址再限制 frame allocator 和正式 direct map。
- 后续影响：不能只改 frame allocator 常量。任何新内存上限都必须同时覆盖 early identity、
  early 高半区、FDT 位置、正式 direct map 和实际 FDT 分配上限，并保持小内存配置不会
  因扩大可达窗口而分配未安装 RAM。

## RV64 巨型静态 heap 曾阻止 QEMU 装载 kernel ELF

- 状态：已确认并通过移出 BSS 修复
- 适用范围：RV64 小内存启动、kernel heap、QEMU DTB 放置
- 最后验证：2026-08-11
- 证据：`os/src/arch/rv64/config/mm.rs` 的 256 MiB `KERNEL_HEAP_SIZE`；`ekernel` 物理末址
  约 `0x90bce000`；QEMU `-m 256M` 报 `No enough memory to place DTB after kernel/initrd`。
- 内容：这个失败发生在 QEMU 进入 OpenSBI/内核之前，与 FDT 动态内存解析及 16 GiB
  early leaf 扩展无关。当前 heap 移出 BSS 后 release kernel-rv 约 8.0 MiB，512 MiB guest 已
  运行到 libcbench；256 MiB guest 现在能进入内核，并明确报告 heap
  `reserved_end=0x90c0b000` 超过 `memory_end=0x90000000`，因而不是有效运行配置。
- 后续影响：区分“ELF 无法装载”和“启动后没有足够 RAM 预留目标 heap”。若要支持 256 MiB guest，
  必须实现按实际 RAM 缩减或动态扩容 heap，不能只缩小 ELF。

## 自动 atime 更新不能复用会刷新 ctime 的显式 utimens 路径

- 状态：已确认并修复 BuildStorm 写放大
- 适用范围：relatime、read/readdir、ctime、ext4 metadata writeback
- 最后验证：2026-08-10
- 证据：RV64 8 GiB/8 核固定 120 秒 attributes 计数 A/B；临时 ext4 文件 stat/cat/touch 专项
- 内容：旧实现每次自动 atime 都同时把 ctime 更新为 now，使下一次 relatime 判断继续满足
  `atime <= ctime`。58,692 次 set-times 中有 58,682 次来自自动 atime，形成约 29 万次块写请求。
  自动访问更新改为不碰 ctime 后，atime 落盘降至 1,185 次，块写请求降至 9,663；显式 `touch -a`
  仍同时更新 ctime。
- 后续影响：自动访问时间和显式 `utimensat/futimens` 必须走语义不同的入口。不能为复用代码让普通
  read 改 ctime，也不能反向让显式时间修改漏掉 ctime；relatime 回归需检查第二次读取不重复落盘。

## 多个 lwext4 时间 setter 会为同一 inode 重复完整路径遍历

- 状态：已确认并由组合 vendor API 修复当前热点
- 适用范围：atime/mtime/ctime、文件写回、Cargo/rustc 高频 metadata 更新
- 最后验证：2026-08-10
- 证据：RV64 8 GiB/8 核固定 120 秒 class A/B；`vendor/lwext4_rust/c/lwext4/src/ext4.c`
- 内容：`ext4_atime_set`、`ext4_mtime_set`、`ext4_ctime_set` 各自获取 path 对应 inode ref 并提交。
  即便 Rust 已持全局 ext4 锁，连续调用仍重复 pathname walk 和 metadata transaction。组合更新把多个
  字段放入一次 inode ref 生命周期，attributes hold 从 54.60 降至 30.46 CPU 秒。
- 后续影响：多字段 metadata 更新应优先审查能否在一个底层 transaction 中完成；不能仅因外层已有锁
  就假设多个 C API 调用成本很低。合并必须保持字段选择、范围过滤、只读检查、错误和持久化语义，不能
  用跳过时间戳更新换取速度。

## 过小 dentry cache 会同时丢掉 inode metadata 和 PageCache identity

- 状态：已确认并由 16K cache 修复当前窗口
- 适用范围：Cargo 深目录树、VFS dentry/inode cache、ext4 lookup/stat、kernel heap 权衡
- 最后验证：2026-08-10
- 证据：1024/8192/16384 项 RV64 8 GiB/8 核 120 秒窗口；lookup 30,616 -> 6,007 ->
  4,019，8192 项 eviction 5,426，16K 为 0
- 内容：全局 dentry cache 满后每次 insert 扫描并任意移除一个只有 cache 引用的叶节点。
  叶 dentry 消失后 inode 的 weak cache 也无法升级，连带丢失 raw metadata 和 inode PageCache；
  后续访问不只多一次 dentry lookup，还会重新读文件页。
- 后续影响：调容量时同时看 lookup calls/ticks、stat miss、PageCache fill/registry、eviction 和
  heap peak。短窗口没有 eviction 不代表完整构建永不达容量，也不应为零 eviction 无界扩容。

## buddy allocator 的 free-list 线性 buddy 查找会在编译负载下吞掉大量 CPU

- 状态：已确认并由 vendor allocator 修复短窗口热点
- 适用范围：kernel heap 高频小对象释放、8 核 Cargo/rustc、allocator 归因
- 最后验证：2026-08-10；RV64 8 核固定 120 秒 A/B
- 证据：旧 allocator dealloc total/core 29.82/26.26 CPU 秒，vendor bitmap+doubly-list 后为
  4.42/2.88 秒
- 内容：总 heap ticks 同时包含 spin-lock wait 和 allocator core，不能只凭总数归因锁竞争。拆分后旧
  `buddy_system_allocator 0.10.0` 的 core 占 dealloc 约 88%，源码对应逐项扫描 free list 寻找 buddy。
- 后续影响：先拆 lock/core 再选择方案；禁止简单关闭 coalesce（长期会碎片/OOM），也不能只在本机
  registry 打补丁。可交付实现必须位于 `vendor/` 并保持完整 split/coalesce 与 Layout 语义。

## bounce buffer 看起来是双拷贝，但不能在无占比和无 fault 协议时直接删除

- 状态：已确认当前不是主要热点
- 适用范围：用户 I/O buffer、copy_to/from_user、read/write/socket 零拷贝设想
- 最后验证：2026-08-10；RV64 8 核固定 120 秒 Cargo 窗口
- 证据：约 106 MB user copy 的 calls/bytes/ticks 合计只有约 0.424 CPU 秒
- 内容：copy helper 本身只执行一次 kernel↔user copy；“第二次”来自 FileOp/Socket 与 kernel bounce
  buffer。helper 同时承担 VMA permission、lazy/COW resolution 和 PTE translation，直接传 user pointer
  会在 kernel page fault、并发 munmap 和部分 I/O side effect 上改变语义。
- 后续影响：性能优化按计数占比排序。若未来做 prepared pages，必须先固定/验证所有 user span，再允许
  file offset 或外部设备产生副作用，并覆盖 EFAULT、跨页 COW、short I/O 与并发地址空间修改。

## stat 和已知类型的 dirent 不能为每个结果重新遍历完整路径

- 状态：已确认并修复专项性能窗口
- 适用范围：lwext4 `stat` 字段读取、directory lookup/readdir、Cargo 深路径元数据负载
- 最后验证：2026-08-14；RV64 同工作量目录遍历 A/B 与 RV64/LA64 120 秒窗口
- 证据：优化前 stat/lookup 占约 84% ext4 lock hold；raw-inode/dirent 优化后 tg-xtask 约
  `2m15--2m19s -> 1m34s`
- 内容：即使 metadata block 已缓存，`ext4_mode_get`、owner/time getters 每次仍执行 pathname walk，
  同一 stat 连续调用六次会把瓶颈从 I/O 变为锁内 CPU。Rust 通过 FFI 逐项扫描父目录同样昂贵，即便
  不再二次查 mode，仍显著慢于让 lwext4 内部按 child path/目录索引完成查找。一次 raw inode 快照既
  减少遍历，也保证各字段来自同一时刻。readdir 也不能忽略 lwext4 已从 on-disk dirent 解析的 file
  type，再对每个结果调用 `ext4_mode_get`：同工作量 RV A/B 中该做法多出 15622 次 lower call，删除
  后 readdir hold ticks 下降约 81%。但未启用 ext4 FILETYPE 的合法文件系统会返回 UNKNOWN，必须只在
  该情形回退原 child pathname 查询。
- 后续影响：看到 block I/O 已很低但 ext4 hold 仍高时，应按操作和调用数归一，不能继续只调 cache。
  优化不得直接读取未对齐 packed 字段引用；需按值读取并做 endian/high-bit 处理，同时保留 UNKNOWN
  dirent fallback。跨 syscall 缓存 raw inode 时，read/atime、write、truncate、chmod/chown、link/unlink、
  rename 和 orphan 都是必须审计的失效点；父目录还会被 namespace 操作间接修改，失效不完整时
  不应缓存目录。成功 unlink 还必须递减 `nlink_override`。锁序保持
  “释放 ext4 锁后再失效 inode 快照”。

## lwext4 的 16 项 block cache 会把路径元数据放大成海量 4 KiB I/O

- 状态：已确认并修复专项性能窗口
- 适用范围：Cargo 深目录树、lwext4 path lookup/inode/extent、PageCache 与 block I/O 归因
- 最后验证：2026-08-10；RV64 8 核固定 180 秒 Cargo 窗口
- 证据：16 项时 172.6 MB inode data read 对应 4.97 GB block read；4096 项时 221.7 MB data read只对应
  295.8 MB block read
- 内容：`block_read_bytes` 不等于应用文件内容。lwext4 的 file data 使用 direct path，但目录块、inode
  table 和 extent lookup 使用内部 bcache；只有 16 个 4 KiB entry 时，每次重新 open 深路径都会淘汰
  元数据，形成近 29 倍读取放大。只扩大上层 PageCache 或继续合并连续 data blocks无法命中这一层。
- 后续影响：分析文件系统热点时同时记录 PageCache fill bytes、Ext4Inode requested/completed bytes、
  block size buckets 和 ext4 lock hold。cache 容量 A/B 必须比较同窗口完成进度与 heap current/peak；
  单看 block bytes 可能偏好容量不足但进度更慢的配置。

## sigtimedwait 的目标信号通常已被 mask，不能只套普通 interruptible sleep

- 状态：已确认并修复 signal/time 调度风暴
- 适用范围：`rt_sigtimedwait`、`rt_sigsuspend`、busybox/coreutils timeout、进程级 signal 选路
- 最后验证：2026-08-10；RV64 8 核 timeout 专项与 Cargo 固定窗口
- 证据：修复前 600 秒窗口约 1962 万次 `signal_time` yield；修复后 busybox 3 秒 timeout 和干净
  300 秒 Cargo 窗口均为 `signal_time=0`
- 内容：简单把轮询替换为普通可中断阻塞仍会死锁：调用者按 POSIX 习惯先 block wanted signal，而普通
  `check_signal_interrupt()` 会跳过 masked signal。必须另外登记 sigwait wanted set，让投递端选择并
  唤醒 waiter，同时保留该信号 pending 供 sigtimedwait 消费，不能把它转换成 `EINTR`。
- 后续影响：验证必须同时覆盖目标信号唤醒、有限 timeout、非目标信号 EINTR 和信号到达/发布 Blocked
  的 lost-wakeup 窗口；只测“超时最终返回”不足以证明 signal selection 正确。

## 交互式 probe 返回后的 stdin 轮询会污染调度计数

- 状态：已确认
- 适用范围：`user_shell` 下的 `/proc/respos_perf` 专项测量、stdin、scheduler yield/IPI
- 最后验证：2026-08-10；RV64 1 GiB/8 核 `buildstorm_private_map_probe`
- 证据：`os/src/fs/stdio.rs::Stdin::read()` 与 yield 子系统分桶；分开发送命令时约 46--113 万次
  `stdio_yields`，预排 probe/read/quit 后同一工作窗口为 0
- 内容：probe 返回提示符后，若宿主过一段时间才发送读取计数的命令，shell 会在 SBI console 无字符时
  反复 `yield_current_task()`。这些切换发生在被测 workload 之后，却仍落在尚未再次 reset 的窗口内。
- 后续影响：交互式性能 probe 必须把工作命令、读取 `/proc/respos_perf` 和 `quit` 一次性写入串口输入；
  已出现的高 scheduler yield 必须先按调用点分桶，不能直接归因于刚结束的 workload。

## lwext4 读取稀疏洞时不能把未分配块号当物理块 0

- 状态：已确认并修复 tg-xtask 链接产物损坏
- 适用范围：稀疏文件、共享文件 mmap、linker 输出、`ext4_fread()`
- 最后验证：2026-08-08；RV64 8 核专项 probe 与手工 ArceOS release build
- 证据：`vendor/lwext4_rust/c/lwext4/src/ext4.c::ext4_fread()`、
  `os/src/fs/ext4/inode.rs::read_at()`；修复前 lld 新文件的共享映射在任何用户写入前就含稳定的磁盘
  块 0 字节，最终 `.symtab[0]` 非零；修复后该 entry 全零且 `llvm-objcopy` 返回 0
- 内容：extent lookup 对洞返回 `fblock == 0`。旧批量读取路径仍把完整洞块传给 direct block read，
  尾部不足一块的洞也读取物理块 0。两种洞必须显式填零并推进位置，只有非零且物理连续的数据块才能
  合并读取。Rust `read_at()` 预清 buffer 可作为防御，但不能替代后端正确处理洞。
- 后续影响：稀疏回归不仅要 close/reopen 后读洞，还要覆盖新建文件 ftruncate 后立即 MAP_SHARED，
  并在用户首次写入前检查映射内容全零。

## mmap 与 pwrite 共存时不能维护两份普通文件缓存页

- 状态：已确认并修复专项回归
- 适用范围：同一 inode 的 `MAP_SHARED`、read/write/pwrite、truncate
- 最后验证：2026-08-09；RV64 8 核 `buildstorm_file_probe` 与完整 BuildStorm OOM 边界
- 证据：`os/src/fs/page_cache.rs`、`os/src/fs/file.rs`、`os/src/mm/memory_set.rs`
- 内容：只在全局表中共享各个 VMA 的 frame 仍会与 PageCache 形成两份普通文件页，需要 read overlay、
  write mirror 才能维持一致性，并会为累计 mmap 页留下大量弱引用树节点。当前普通文件 mmap 直接克隆
  PageCache 的 `FrameTracker`；read/write 天然访问同页，truncate shrink 清零仍被映射持有的 frame。
  全局弱表及 overlay/update 只用于没有 PageCache 的兼容后备文件，并会清理失效弱引用。
- 后续影响：文件一致性 probe 应包含同页 mmap+pwrite 双向观察与 truncate/regrow 零填充，不能只在
  munmap/close 后比较磁盘内容；PageCache 回收也必须检查 frame 是否仍被 VMA pin。

## 用户栈溢出可能伪装成动态解释器权限故障

- 状态：已确认并修复 tg-xtask 启动 SIGSEGV
- 适用范围：大型动态 ELF、exec 初始栈、相邻 VMA fault
- 最后验证：2026-08-08；旧 BuildStorm 镜像 RV64 8 核
- 证据：fault `sepc=0x9bbbe0`、目标 `0x3000017298` 与当时栈/解释器 VMA 布局；
  `os/src/arch/{rv64,loongarch64}/config/mm.rs`
- 内容：原 512 KiB 栈向下越过 guard 后命中解释器 RX VMA，表象是 loader 区域 store permission fault。
  将栈窗口扩大为 8 MiB 且保持 lazy 后，tg-xtask `--help` 正常退出。
- 后续影响：遇到解释器附近 fault 不应只审查 PT_LOAD flags；先根据 SP 和写地址检查是否为栈越界。

## lwext4 的 truncate 扩容不能只返回成功

- 状态：已确认并修复 BuildStorm minibuild ELF 截断
- 适用范围：稀疏文件、PageCache 离散脏区写回、linker 输出
- 最后验证：2026-08-08；RV64 8 核 `buildstorm_file_probe` 与官方 BuildStorm minibuild
- 证据：`vendor/lwext4_rust/c/lwext4/src/ext4.c::ext4_ftruncate_no_lock()`、
  `os/src/fs/ext4/inode.rs::write_at()`；修复前产物实际结束于 `0x395d30`，而 ELF 节表要求到
  `0x40cd30`，差值 `0x77000` 正是被错误压缩的稀疏区；修复后 `BUILDSTORM_MINIBUILD ok`
- 内容：旧实现对 `old_size < new_size` 返回 `EOK` 却不更新 inode 或 open-file size，随后
  `file_seek()` 又把目标偏移夹到旧 EOF，导致远端脏区被追加到错误位置。增长 truncate 应只更新
  inode size 并保留洞，不需要为洞分配数据块。
- 后续影响：稀疏写回回归必须检查最终长度、洞区为零以及尾部模式数据，不能只检查 write 返回值。

## LTP `fallocate03` 通过不证明物理空间已经预留

- 状态：已确认
- 适用范围：普通 ext4 `fallocate()` default/`FALLOC_FL_KEEP_SIZE`、LTP 20240524
- 最后验证：2026-08-14；基于 `80e8c5a` 的 Linux probe 与双架构聚焦 LTP
- 证据：`scripts/fallocate_prealloc_probe_linux.c`、官方 LTP
  `testcases/kernel/syscalls/fallocate/fallocate03.c`、
  `/tmp/respos-{rv,la}-fallocate-phase5-baseline.log`
- 内容：该 LTP case 在稀疏文件的八个位置调用 fallocate，但只断言返回成功；它不检查 `st_blocks`、
  logical size、零读或数据保留。仅做 sparse truncate、写零后缩回或让 `KEEP_SIZE` 无操作成功，都可能
  让 case 变绿却没有提供 Linux `fallocate()` 所需的空间预留保证。
- 后续影响：实现必须由文件系统真实分配 extent，并用独立 probe 检查块数增长和 size/data/offset；
  底层没有独立预分配状态时应继续诚实返回 `EOPNOTSUPP`，不能在 syscall 层模拟成功。

## 可写共享文件映射不能永久冻结 mmap 时的 EOF

- 状态：已确认并修复专项 mmap 扩容回归
- 适用范围：`MAP_SHARED | PROT_WRITE`、mmap 后 ftruncate 扩容、munmap/msync 写回
- 最后验证：2026-08-08；RV64 8 核 `buildstorm_file_probe`
- 证据：`os/src/mm/memory_set.rs::mmap_file_backing()` 与
  `MemorySet::prepare_file_writeback()`
- 内容：映射建立后文件可能扩容，原先超出旧 EOF、但仍位于映射窗口内的页随之变为有效页。VMA
  若把 mmap 时的 file length 永久作为写回上限，会静默丢弃这些页。共享 VMA 应保存完整映射窗口，
  写回时再以当前 EOF 裁剪，避免重扩展后来被截短的文件。
- 后续影响：mmap 文件回归需包含“先映射短文件、后扩容、再写新增尾页”，仅测试固定长度映射不足。

## lwext4 的 superblock 与 inode 操作必须使用同一把全局锁

- 状态：已确认并修复当前 BuildStorm linker 阻塞
- 适用范围：SMP 下所有 lwext4 C API，包括文件/目录操作、sync、statfs 和 shutdown
- 最后验证：2026-08-08；RV64 8 核官方 BuildStorm
- 证据：`os/src/fs/ext4/{inode.rs,super_block.rs}`；修复前 PC 长期位于
  `ext4_dir_write_entry` 的断言路径，修复后 `ld.bfd` 完成且 collect2/gcc/rustc/cargo 正常退出
- 内容：lwext4 的 mount、block cache 与目录遍历状态位于共享 C 对象中。只串行 inode 操作而让
  superblock flush/statfs/shutdown 并发进入，会破坏其内部状态；增加另一把锁也不能建立正确互斥。
- 后续影响：任何新增 lwext4 入口必须复用 `EXT4_OP_LOCK`，不能按 Rust 模块各建一把锁。

## UNIX stream socket 必须显式传播 peer close

- 状态：已确认并修复当前 cargo/rustc 退出阻塞
- 适用范围：`socketpair(AF_UNIX, SOCK_STREAM)`、connect/accept、read/write/poll
- 最后验证：2026-08-08；RV64 8 核官方 BuildStorm
- 证据：`os/src/net/socket.rs`；修复前 cargo 永久等 `recvfrom`，修复后 linker 及完整编译子进程退出
- 内容：仅靠接收队列是否为空无法区分“暂时无数据”和“对端已经关闭”。端点必须共享 close 状态；
  peer close 后 read 先排空已有数据再返回 0，write 返回 `EPIPE`，poll 同时报告相应 readiness。
- 后续影响：修改 UNIX socket clone/drop/connect/pair 时需一起审查端点引用和 close 状态，不能只修
  阻塞 read 分支，否则 poll 或 write 会继续产生不一致行为。

## UNIX socket 的空读/满写/accept 不能用 yield 轮询

- 状态：已确认并修复专项与真实 Cargo 活性窗口
- 适用范围：AF_UNIX socketpair、pathname listener、Cargo/rustc IPC、多核调度
- 最后验证：2026-08-10
- 证据：旧 600 秒窗口最多 69,194 次 unix yield；event waiter 专项为 unix/scheduler yield 0、
  blocking switch 5，真实 Cargo timeout 窗口 net/unix 仍为 0
- 内容：只把 yield 换成 sleep 会增加延迟且仍需轮询；只在条件检查后再登记 waiter 会丢失恰好发生的
  write/read/connect/close。条件状态、waiter 登记和 blocked 发布必须与同一 buffer/pending lock 配合，
  producer 在修改条件后取出 waiter，并在释放数据锁后唤醒。
- 后续影响：验证不能只看“没有 yield”，还要验证 data integrity、buffer-full backpressure、peer EOF/
  EPIPE、nonblock EAGAIN、accept/connect、signal EINTR 和 producer-before-switch 竞态。

## RV64 `sret` 前不能提前恢复 SIE 或 user trap vector

- 状态：已确认并修复
- 适用范围：RV64 trap return、exec 初始上下文、signal frame 恢复、SMP timer interrupt
- 最后验证：2026-08-07；8 核 BuildStorm 运行中 GDB CSR 快照
- 证据：`os/src/arch/rv64/trap/{context.rs,trap.S}`；修复前
  `/tmp/respos-rustc-pc-sample{1,2,3,4,5}.txt`，修复后
  `/tmp/respos-rustc-pc-postfix-sample{1,2,3,4,5}.txt`
- 内容：若 `__restore` 先把 `stvec` 指向 `__trap_from_user`，再原样写回带 `SIE=1` 的
  `sstatus`，timer 可在 `sret` 前进入 user trap 汇编。此时 `sscratch` 和 GPR 尚未形成
  user-entry 契约，`csrrw` 后的保存指令会递归 StorePageFault；表面上多个 vCPU 仍活跃，
  实际任务已不能前进。
- 后续影响：恢复汇编必须在写 `sstatus` 前无条件清 `SIE`，保持 kernel `stvec` 直到所有
  可能 fault 的恢复完成，并由 `sret`/`SPIE` 完成最终中断状态转换。不能只依赖某一种
  TrapContext 构造路径清位，因为 signal return 等路径也可能提供保存上下文。

## exec 必须在旧地址空间中清理 sibling thread 的用户地址

- 状态：顺序已修正，远端 sibling 的协作式终止协议已实现
- 适用范围：多线程 exec、robust futex、`clear_child_tid`、共享 `MemorySet`
- 最后验证：2026-08-07；RV64 8 核 BuildStorm 受控 trace
- 证据：`os/src/task/task.rs::close_other_threads_for_exec()` 与 `exit_process_group()`；
  `os/src/task/processor.rs::publish_saved_handoff()`；`/tmp/respos-buildstorm-user-fault-trace.log`
- 内容：线程保存的 robust-list 和 clear-child-tid 是旧用户映像中的地址。若先替换
  共享 `MemorySet` 的内容，再清理 sibling，内核会把旧地址当作新程序地址写零或写
  `FUTEX_OWNER_DIED`，可破坏刚装载的映像。
- 后续影响：必须在替换 MM 前处理依赖旧用户地址的线程私有状态。当前实现采用四步
  quiescence 协议：`request_termination()` 标记 sibling 不可再被 claim → `remove_task()`
  从调度器摘除 → spin-wait `has_cpu_owner()` 等待远端 CPU 切回 idle → 安全清理。
  `publish_saved_handoff()` 对已标记 `terminate_requested` 的 task 不再重新发布到 ready
  queue，而是静默丢弃。`can_be_claimed_on_cpu()` 和 `try_claim_running_on_cpu()` 均拒绝
  已标记终止的 task。

## exec argv/envp 不能用很小的固定项数上限

- 状态：已确认并修复当前 BuildStorm E2BIG
- 适用范围：`execve`/`execveat`、动态工具链、大环境表
- 最后验证：2026-08-07；RV64 8 核 cargo/rustc
- 证据：`os/src/mm/mod.rs::extract_cstrings_from_user()`；
  `/tmp/respos-minibuild-pipe-output.log`
- 内容：argv 和 envp 共用的 32 项限制使 cargo 启动 rustc 时返回 `E2BIG`，cargo 只能报
  child `never executed`。项数放宽必须同时限制累计字节，否则会把用户指针数转化为无界
  kernel heap allocation。
- 后续影响：调试工具链 exec 必须保留 cargo 捕获的 child stderr；仅看顶层
  `BUILDSTORM_MINIBUILD fail` 会丢失具体 errno。

## 跨 CPU 自旋锁不能防止同 CPU 中断重入

- 状态：已确认并修复当前 heap/timer 路径
- 适用范围：全局 heap、kernel timer trap、任何同时被 syscall 和中断访问的状态
- 最后验证：2026-08-06；RV64 debug、2/4/8 核
- 证据：`/tmp/respos-smp8-gdb-bt1.txt`、`/tmp/respos-smp2-dynamic-bt.txt`；
  `os/src/mm/heap_allocator.rs`、`os/src/arch/rv64/trap/mod.rs`
- 内容：一个 CPU 持有普通 spin lock 时若仍可被使用同一把锁的中断打断，中断会永久等待
  被它自己暂停的临界区。实测的两种形态是 heap dealloc 被 timer 中断后再 alloc，以及
  `ACTIVE_ITIMER_TASKS.remove()` 被 timer 中断后再 lock。其他 CPU 的锁等待只是级联现象。
- 后续影响：GDB 审查要保留“中断栈下方的被打断栈”，不能只看顶层等锁 CPU。修复时要么
  让底层锁关本地中断，要么把中断工作延迟到无锁安全点；不能仅增加跨 CPU 锁或换 allocator。

## `CLONE_VFORK` 必须先登记父进程 blocked 再发布子进程

- 状态：已确认并在当前工作树修复
- 适用范围：glibc `vfork`/`posix_spawn`/`popen`，以及所有带 `CLONE_VFORK` 的 clone 调用
- 最后验证：2026-08-06；RV64、SMP=1 的原语义回归与 SMP=8 BuildStorm 受控 trace
- 证据：`os/src/syscall/process.rs::sys_clone()`、`os/src/task/task.rs::execve()` 与
  `exit_process_group()`；修复后的 4 路 CAgent pass 日志
  `/tmp/cagent_debug_vfork_fix_4_busybox/`
- 内容：Linux 语义要求 vfork 父任务一直阻塞到子任务成功 exec 或 exit。仅调用
  `blocking_and_run_next()` 而不建立子到父的唤醒边，会使父任务错误地等到子命令退出；普通 exit
  的 SIGCHLD 唤醒会掩盖这个错误，造成 `popen` 很慢而 `pclose` 几乎立即返回。SMP 下还有更窄的
  窗口：若先发布 child、后把 parent 加入 blocked 表，child 可先 exec 并把一次性 wake 丢掉。
  cargo/rustfmt 曾表现为捕获输出不返回，但最新 trace 已确认顺序修复后 parent 正常恢复且 rustfmt
  完成退出，仍有独立的 pipe 引用未释放问题；两者不能混作同一个根因。
- 后续影响：vfork 同步必须是一次性且只限该 clone 关系；exec 仅在新映像状态完整后释放，退出路径
  也必须释放以覆盖 exec 失败。父 waiter 必须先登记，child 才能进入 ready queue；不要以普通
  SIGCHLD、yield 或单核通过代替该协议。

## lazy VMA 销毁不能按虚拟跨度逐页扫描

- 状态：回收复杂度已修复，完整 BuildStorm 仍待验证
- 适用范围：进程退出、munmap 辅助路径、稀疏匿名/文件 VMA
- 最后验证：2026-08-06；RV64 8 核 cargo/rustfmt exit trace
- 证据：`os/src/mm/memory_set.rs::MapArea::unmap()`；
  `/tmp/respos-buildstorm-exittrace.log`
- 内容：lazy VMA 可能保留很大的虚拟区间但只有少量 resident frame。销毁时遍历整个 VPNRange 会让
  exit 成本与地址预留大小成正比。Framed area 应只遍历 `data_frames`；Direct area 才按完整
  区间解除既有恒等映射。最新 rustfmt trace 已越过进程退出，但 pipe open-file 仍有额外引用，
  因此 sparse unmap 优化不能作为 EOF 问题已解决的证据。
- 后续影响：对 lazy 结构做回收、fork、mprotect 或统计时优先按 resident metadata 工作；需要改变
  整个 VMA 属性时操作区间元数据，不能隐式扫描每个潜在页。

## 共享进程资源的退出归属不能用 `Arc::strong_count` 猜测

- 状态：已确认并修复当前 BuildStorm 并行 rustc ENOMEM/SIGSEGV 根因
- 适用范围：多线程进程组退出、延迟 TCB drop、zombie、`CLONE_VM`/`CLONE_FILES`
- 最后验证：2026-08-09；RV64 1 GiB/8 核 `frame_reclaim_probe` A/B 与 8 GiB fault-only BuildStorm
- 证据：`os/src/task/task.rs::exit_process_group()`、`user/src/bin/frame_reclaim_probe.rs`；guest
  `/proc/respos_health` 前后值记录于 `current-status.md`
- 内容：worker 可以已离开 thread group，却仍因 context-switch handoff 或 `DEAD_TASKS` 持有 TCB；
  此时它对共享资源的临时强引用不是另一个存活进程。用强引用总数与当前成员数比较会误判外部 owner，
  跳过地址空间清空；zombie leader 再把整个 resident set 固定到父进程 wait/reparent 之后。归属判断
  应扫描 live task identity：只有不同 tgid、仍存活且指向同一资源的任务才阻止本组 teardown。
- 后续影响：不要以扩大 guest 内存或 heap 掩盖线性 frame 泄漏。新增共享 TCB 资源时必须明确区分
  live owner、zombie owner 与退出路径临时引用，并用“分配/触碰—多线程退出—wait—空闲量恢复”短测
  建立门禁。

## pub 镜像不能在 QEMU 运行时由宿主修改

- 状态：已确认
- 适用范围：`img/sdcard-*-pub.img`、QEMU、`debugfs`、`e2fsck`
- 最后验证：2026-08-03
- 证据：RV64 pub 镜像的正常 guest `quit` 后 `e2fsck -fn`；lwext4 卸载路径和
  `/tmp/ext4-flush-pre-e2fsck.log`
- 内容：guest 正常 reboot 会依次停止 journal、卸载 ext4 并等待 virtio-blk FLUSH；只有看到
  QEMU 退出后，宿主才可运行 `e2fsck` 或 `debugfs -w`。强制终止 QEMU、在其持有 raw image
  时修改镜像、或在 gzip 解压尚未完成时运行 fsck，都会造成 journal recovery、短读或元数据
  不一致，不能归因于单个 CAgent syscall。
- 后续影响：每轮写入诊断 runner 前先恢复完整镜像并 `e2fsck -pf`；测试后先正常 guest
  `quit`、确认 QEMU 退出，再离线提取或修改文件。不可恢复的强制停止后应从压缩基线恢复镜像。

## 多核诊断应使用 QEMU `-snapshot`

- 状态：已确认
- 适用范围：RV64 SMP 压力、超时、GDB 暂停和所有不需要持久保存 guest 写入的诊断
- 最后验证：2026-08-05；RV64 `-smp 8 -m 256M` timeout/exit 压力
- 内容：SMP 压力常需超时后暂停 QEMU 或接入 GDB，不能保证 guest 有机会正常卸载 ext4。直接对
  raw pub 镜像运行会把临时 `/tmp`、journal 及非正常退出混入后续结果。
- 后续影响：诊断启动命令在 raw drive 之外加入 `-snapshot`，例如
  `qemu-system-riscv64 ... -snapshot -smp 8 -m 256M`。只有确实需要保留 guest 文件时才退出 snapshot
  模式，并遵守上一节的正常 guest `quit`/离线 fsck 流程。

## `make rv/la` 返回 0 不等于测试通过

- 状态：已确认
- 适用范围：完整 QEMU 测试
- 最后验证：2026-08-01
- 证据：2026-08-01 的 `make rv`/`make la` 均退出 0，但当轮 LTP 各有 600 余项失败
- 内容：顶层命令通过 pipefail 运行 QEMU 并 tee 日志；testrunner 即使记录用例失败仍会跑完并主动
  关机，因此外层成功主要表示 QEMU 正常结束。
- 后续影响：验收必须解析 summary、TBROK、segfault、wrapper exit code 和分组 fail。

## writable file `MAP_SHARED` 拒绝曾造成全局级联

- 状态：历史故障，当前拒绝策略已修复
- 适用范围：basic、lmbench、LTP 以及依赖共享控制页的用户程序
- 最后验证：故障 2026-08-01；修复状态收口 2026-08-13
- 证据：历史 `os/src/fs/file.rs::mmap_allowed`、2026-08-01 双架构日志；当前实现与验证见
  `architecture.md` 的 writable `MAP_SHARED` 协议和 `current-status.md` 的 Phase 3 结果
- 内容：旧策略返回 `EOPNOTSUPP`，使使用 fd-backed writable shared page 的 LTP 框架初始化失败，
  看似无关的 getpid/fcntl/fs 用例随之 TBROK。当前已采用 PageCache 统一 frame、锁外快照写回和
  写回错误协议，不再无条件拒绝 writable file `MAP_SHARED`。
- 后续影响：遇到大范围 LTP TBROK 时仍应先定位首个框架错误，不能逐个修后续 case；当前 mmap
  主线问题已经转为 EOF/truncate 后的精确 SIGBUS、COW 尾页和跨 hart 失效，不得恢复旧拒绝策略。

## MM 锁内后端 I/O 是长期锁序风险

- 状态：历史风险；核心 file mmap/writeback 路径已重构，新增路径仍须审计
- 适用范围：file fault、shared unmap/writeback
- 最后验证：故障 2026-08-01；Phase 3 收口 2026-08-11
- 证据：历史 `os/src/mm/memory_set.rs`、`docs/四天内核重构-ABC-整合审查.md`；当前协议见
  `architecture.md` 与 `current-status.md` 的 PageCache/写回章节
- 内容：旧 file backing fault/writeback 曾在 `MemorySet` 写锁内访问后端，munmap 错误传播也不完整。
  当前核心路径采用锁外准备/快照、重新校验后提交，并由 PageCache 写回状态和 error cursor 传播错误。
- 后续影响：该锁序约束仍然有效。实现 mmap EOF/SIGBUS、truncate invalidation 或新 file backing 时，
  不得重新在 MM 锁内执行 ext4/PageCache 后端 I/O；锁外阶段返回后必须校验 VMA/version/identity。

## LTP 的首个框架错误会污染后续结论

- 状态：已确认
- 适用范围：LTP harness 调试
- 最后验证：2026-08-01
- 证据：2026-08-01 writable mmap 初始化级联；`user/src/bin/testrunner.rs`；历史 `pipe2_02` helper
  级联记录
- 内容：测试可能在 `tst_test.c` 资源准备、mount、helper exec 或共享控制页阶段失败，尚未进入目标
  syscall assertion。后续大量相同错误不是大量独立内核缺陷。
- 后续影响：先定位第一个不同的失败点，并从 testsuit/image 脚本确认它属于 setup 还是测试主体。

## 未跟踪用户程序仍会进入本地构建

- 状态：已确认
- 适用范围：`user/src/bin/` 与内核内嵌应用
- 最后验证：2026-08-01
- 证据：`user/Makefile` 的 `$(wildcard src/bin/*.rs)`；当前构建日志编译了未跟踪 FS probe
- 内容：Git 未跟踪不等于构建不可见。任何放在 `user/src/bin` 的 `.rs` 都会被构建并可能嵌入内核，
  影响大小、编译时间或应用清单。
- 后续影响：报告可复现结果时附 `git status --short`；干净 CI 与本地 dirty tree 可能产物不同。

## futex cmp-requeue probe 默认构建不是有效竞态测试

- 状态：已确认
- 适用范围：`task_a_futex_cmp_requeue_probe`
- 最后验证：2026-08-06
- 证据：`os/src/task/futex/wait.rs` 的 `TASK_A_FUTEX_CMP_REQUEUE_TEST_YIELD`；
  `docs/四天内核重构-ABC-整合审查.md`
- 内容：probe 期待 changer 必定在线性化点前改值；只有用
  `TASK_A_FUTEX_CMP_REQUEUE_TEST_YIELD=1` 编译内核才会强制形成该窗口。默认内核直接运行时，
  cmp-requeue 合法地先完成并返回 affected count，probe 会产生伪失败。
- 后续影响：专项测试后必须恢复默认 build。即使专项 build 一次通过，也要记录重复轮数和任何
  timeout；不能删除非收敛样本。

## RV/LA 构建共享活动 Cargo config

- 状态：已确认
- 适用范围：顶层和子目录构建
- 最后验证：2026-08-01
- 证据：`Makefile`、`os/Makefile`、`user/Makefile`
- 内容：构建目标把架构模板复制到同一个 `.cargo/config.toml`。并行运行两个架构构建可能在
  config 被覆盖时使用错误 target/linker 设置。
- 后续影响：双架构顺序构建；若未来需要 `make -j`，先改成互不共享的配置/target dir 协议。

## 比赛镜像是可变测试状态

- 状态：已确认
- 适用范围：mount、文件、LTP、benchmark
- 最后验证：2026-08-01
- 证据：顶层 QEMU 以 raw 可写方式挂载 `img/sdcard-*.img`；`scripts/get_img.sh`
- 内容：测试会创建 `/tmp`、`/etc`、symlink、benchmark 文件和 mount backing。前一轮异常退出可能
  改变后一轮失败形态。
- 后续影响：难以解释的非确定性首先用保留的 `.xz` 恢复镜像，再比较；记录镜像版本和 hash。

## glibc 工具会暴露未覆盖的合法 open flag

- 状态：已确认
- 适用范围：glibc pub 镜像中的 coreutils/CAgent 文件命令
- 最后验证：2026-08-02
- 证据：`os/src/fs/file.rs::OpenFlags`、`os/src/syscall/fs.rs::validate_open_flags()`、
  glibc `/usr/bin/touch` 的 `EINVAL` 复现
- 内容：BusyBox 和 glibc 工具的等价操作不一定使用相同的 open flag。此次 glibc `touch`
  携带 Linux ABI 规定的 `O_NOCTTY`，旧实现因未声明该位在进入 namei 前返回 `EINVAL`。
- 后续影响：遇到用户态工具特定失败时，先保存实际 syscall flags 并对照 Linux ABI；对纯兼容、
  不改变普通文件状态的合法 flag 可接受为 no-op，不能按测试名添加特判。

## 网络 `Connection refused` 可能是 ready 时序而非 ABI

- 状态：已确认并修复（当前工作树）
- 适用范围：多个 client 同时连接同一个 smoltcp TCP listener
- 最后验证：2026-08-03
- 证据：CAgent debug 的 agent exit 255 与 `Connection failed to 127.0.0.1:8080`；
  `os/src/net/listen.rs`；修复后 RV64 三轮官方 CAgent 日志
- 内容：服务端已经 listen 不代表单个 smoltcp listener 能承接同一轮 poll 中的全部 SYN。旧实现等
  userspace `accept` 时才补 listener，第一个握手占用 listener 后，其余并发 SYN 会被 reset。该错误
  会随机落到任意 agent，因而可能伪装成 `kernel`、`cpu` 或 FS 命令失败。
- 后续影响：先保存 agent 原始日志和退出码；若是 connect 失败，不要修改 `uname`、`nproc` 等固定
  命令。监听实现必须按 backlog 提前提供容量，而不是在客户端硬编码重试。

## CAgent 全量 reject 可能来自单线程 LLM server 排队

- 状态：已初步定位，仍需并行/串行对照确认
- 适用范围：题一 `/glibc/cagent_testcode.sh`，`SMP=1` 的 10 个并行 agent
- 最后验证：2026-08-03
- 证据：`testsuit/cagent-test/simple_llm_server.c` 的 `listen(server_fd, 10)`、主循环中的
  `accept` 后直接 `handle_client(client_fd)`；`/tmp/respos-integrated-rv-cagent.log`；当前整合
  提交的单 agent `kernel` 复现
- 内容：官方 runner 同时 fork 10 个 `agent_lite`，但固定 server 不为每个连接创建线程或子进程，
  请求处理是串行的。当前全量运行虽收到请求，却让各项耗时达到约 43–60 秒并超过官方
  20–35 秒 timeout；单 agent `kernel` 能正常获得 `6.10.0-dev` 并以 exit 0 完成。
- 后续影响：不能把这轮 10/10 reject 直接归因于 `uname`、文件系统或 TCP ABI。先用保留日志的
  单项/串行 runner 复现，再单独测 server 并发；A 的 listener backlog 修复仍需保留，因为它
  解决的是连接承载能力，不能消除 server 应用层串行排队。

## 在待恢复 journal 的镜像上写入 debug 文件会丢失

- 状态：已确认
- 适用范围：从 `.gz` 恢复 pub ext4 镜像后使用 `debugfs -w`
- 最后验证：2026-08-03
- 证据：RV64 镜像注入的 `/glibc/cagent_debug.sh` host/image SHA-256 一致，但首次启动 journal
  recovery 后 guest 中变为 0 字节；先运行 `e2fsck -pf` 后重新注入可正常执行
- 内容：压缩包中的 ext4 可能带 `needs journal recovery`。在回放旧 journal 前直接写新 inode，启动
  时的恢复会用旧元数据覆盖该写入。
- 后续影响：只读查看不受影响；临时写镜像前先恢复 journal 并再次校验内容。正式干净回归不要
  注入 debug 文件。

## lwext4 的 CMake 生成目录也跨架构共享

- 状态：已确认
- 适用范围：RV64/LA64 快速切换构建
- 最后验证：2026-08-03
- 证据：`vendor/lwext4_rust/build.rs`、`c/lwext4/Makefile`；失败日志中 LA C compiler 配合 RV
  `ar/ranlib`，手动顺序执行目标架构 `make musl-generic ARCH=...` 后重试成功
- 内容：除了活动 Cargo config，lwext4 两架构还共用 `c/lwext4/build_musl-generic`。切换架构时
  可能出现 C compiler 已切换但 archive 工具仍来自上一架构的混合配置。
- 后续影响：不要并行构建两架构。若顶层构建在 archive 阶段出现交叉工具混用，先顺序执行
  `make musl-generic ARCH=riscv64|loongarch64 -C vendor/lwext4_rust/c/lwext4` 再重试；长期应把
  CMake build dir 按架构拆分。

## libctest wrapper 的 256 需要解码

- 状态：待验证
- 适用范围：musl libctest static/dynamic
- 最后验证：2026-08-01
- 证据：RV/LA 日志均显示 `run-static.sh`、`run-dynamic.sh exited with code 256`
- 内容：可见子用例大量显示 `Pass!`，但 runner 打印的是 wait status/封装值，尚未确认是脚本真实
  exit 1、状态解码问题还是隐藏子用例失败。
- 后续影响：检查 waitpid ABI 和脚本最终命令，不能把“屏幕上都是 Pass”当成 wrapper 成功。

## 高半区模型与 LoongArch 2 MiB 边界历史问题

- 状态：待验证
- 适用范围：LoongArch mmap、TLS、pthread、页表边界
- 最后验证：2026-08-01
- 证据：当前高半区源码已确认；2 MiB TLS 故障仅来自 2026-06 历史 Codex 记忆，当前源码未重现
- 内容：历史调试曾观察到小匿名 mmap 跨 2 MiB 页表边界与 pthread/TLS 故障相关，并采用 LA
  特定 mmap placement 缓解。当前是否仍需要该缓解尚未专项复验。
- 后续影响：若 LA-only pthread fault 重新出现，优先记录 fault VA、TP/TLS、PMD 边界和 mmap
  placement；在复现前不要把旧解释写成当前缺陷。

## open-file 计数不能代表 inode 的全部存活引用

- 状态：已确认并修复正常运行时提前回收
- 适用范围：unlink/rmdir、rename 覆盖、cwd、目录 fd、Path/Dentry、inode number 复用
- 最后验证：2026-08-10
- 证据：`os/src/fs/ext4/inode.rs`、`os/src/fs/namei.rs`；Linux/RV64
  `FS_NAMESPACE_CWD_UNLINK_PASS`
- 内容：File 数为零不表示 inode 无引用。进程 cwd 和临时 Path/Dentry 都能在没有打开 File 的情况下
  保持 inode 可观察；按最后 File close 回收会提前 free lower inode，并可能让仍存活的旧对象与复用的
  inode number 形成 ABA。当前改为最后 Ext4Inode Arc Drop 入队、安全点回收。
- 后续影响：遇到 VFS 生命周期问题时先列出所有引用所有者，不用局部计数代替系统模型。若 Drop 中不能
  安全取得后端锁，应延迟到明确安全点；这种妥协必须同时记录回收时机、失败重试和崩溃恢复边界。

## weak inode cache 不能承担脏数据所有权

- 状态：已确认并修复
- 适用范围：普通文件 close、truncate、unlink、PageCache writeback、inode cache
- 最后验证：2026-08-11
- 证据：`os/src/fs/page_cache.rs`、`fs_writeback_probe phase3`、`/proc/respos_perf`
- 内容：inode registry 只保存 Weak 时，最后一个 File drop 会连同仍脏的 PageCache 一起消失；把同步 I/O
  塞回 Drop 虽能暂时防丢，却让 close 成为性能和锁序热点。当前由外部 dirty-owner 表强持有完整写回
  对象，并在 syscall safe point 做有界批量提交。反向边界同样重要：truncate 可直接移除最后脏页并提交
  时间，如果不主动释放已经 clean 的 owner，低于阈值的单文件会永久留下强引用。
- 后续影响：新增写路径遵循“mutation/note 成功后、返回用户态前 register”；新增清理路径遵循
  “data 和 pending metadata 同时为空才 release”。同步 I/O 期间不得持有 owner registry 锁。

## 历史测试成绩会快速过期

- 状态：已确认
- 适用范围：README、汇报和分支比较
- 最后验证：2026-08-01
- 证据：README 历史“600 余 LTP”与当前 LTP 初始化失败并存
- 内容：同一仓库的不同 commit、镜像、内存配置、libc 和 runner 清单会产生完全不同的结果。
- 后续影响：任何成绩必须携带 commit、日期、架构、镜像、命令和 summary；旧记忆只用于寻找线索。

## TCP 双端都阻塞时，仅定时重试不能代替网络事件唤醒

- 状态：已确认并修复
- 适用范围：smoltcp TCP，特别是 iperf3 的 TCP 控制通道
- 最后验证：2026-08-11
- 证据：RV64 临时干净镜像的 iperf musl BASIC/PARALLEL/REVERSE UDP/TCP
- 内容：iperf3 的 UDP 测试也先通过 TCP 交换控制 JSON。旧 `block_on` 中客户端成功入队
  147 字节 JSON 后，服务端仍阻塞在读取 4 字节长度；回退到 yield-poll 则六项均通过，
  证明根因是协议进展没有唤醒对端，不是 UDP、`execve` 或 runner。
- 后续影响：排查“UDP 卡住”时不要忽略应用协议的 TCP 控制面；更改 TCP 阻塞策略时
  必须同时验证协议事件唤醒和空闲 listener 时的定时器前进。

## 一个存活的 TCP daemon 不应破坏无关进程的 wait/信号同步

- 状态：已确认并修复
- 适用范围：inet poll fallback、TCP/UDP blocking retry 与全局 task timer progress
- 最后验证：2026-08-13
- 证据：`os/src/syscall/fs.rs::block_for_poll`、`os/src/syscall/mod.rs`、
  `user/src/bin/net_timer_progress_probe.rs`；RV64 官方 iperf→iozone 顺序和 LA64 daemon→iozone 专项
- 内容：iozone-only 可完成；`iperf-musl → iperf-glibc → iozone-glibc` 在 initial writers
  后停滞；只杀 `iperf3` 即恢复。卡点不是 writer 的 `wait4`，而是其后 `sleep(2)`：daemon 的
  inet `poll()` 因无事件式 FileOp waiter 而在 kernel fallback 中持续 yield，阻止 boot hart 到达
  user/idle timer 安全点。单纯调整 TCP 兜底 timeout 不会改变这条路径。
- 后续影响：不能用 runner 清理、测例换序或缩短清单声称修复。遇到“某 daemon 使无关 sleep/
  timeout 卡住”时，先检查 daemon 是否长期停留在一次 kernel syscall 的 polling/yield 循环，以及该
  循环是否在无锁位置服务延迟 timer work；不要先归因于 wait、SIGCHLD 或文件写回。

## RV64 首个 glibc 动态程序可能在 loader 尾页冷 fault 时收到 SIGBUS

- 状态：已确认，根因层级已定位到 file-backed loader page 的 lower `EIO`，具体 lwext4 失败点待修复
- 适用范围：RV64 release、初赛 glibc 冷启动、动态链接器 writable/BSS 尾页
- 最后验证：2026-08-14
- 证据：无 feature `clock_gettime01`/`clock_getres01` 作为首个 glibc 程序稳定以 raw wait status 135
  退出；QEMU GDB 在不改变 guest 布局时命中 `trap_handler` 的 `Errno::EIO → SIGBUS` 分支，记录
  `sepc=0x3000010d2c`、store fault `stval=0x3000022290`。该地址属于
  `/glibc/lib/ld-linux-riscv64-lp64d.so.1` 的 writable LOAD/BSS 尾页；后续 glibc 程序与 LA64 不复现。
- 内容：启用 `fault_trace` 会改变 kernel/物理页布局并使问题消失，因此“诊断内核通过且无 fault”不能
  否定正式内核故障。CPU clock 五目标在先运行一个动态程序、loader 页热后全部通过，说明这个 135
  不是 `clock_gettime` 返回值或 CPU timer 语义失败。
- 后续影响：定位此类布局敏感冷 fault 应优先使用不改 guest image 的 GDB 地址断点。测试报告必须
  单列预热项失败，不能宣称整组 6/6；修复应进入 MM/ext4/PageCache 线，并验证 loader 最后文件页
  offset `0x21000` 的 lower read/open/seek/read/close 返回链，而不是在 CPU clock 代码中加时序掩盖。
