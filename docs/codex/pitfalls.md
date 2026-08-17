# RespOS 已确认易错点

## x86 的 packed epoll_event 不能外推到 RV64/LA64

- 状态：已确认并修复
- 适用范围：`epoll_ctl`/`epoll_wait` 原始用户 ABI、Rust probe、libuv/其他 C runtime
- 最后验证：2026-08-16
- 证据：RV64/LA64 Linux UAPI headers；`os/src/syscall/special_fd.rs`；双架构 CMake 自举与
  `socket_phase5/tcp_half_close/udp_shutdown/signal_phase5` 日志
- 内容：旧内核和 Rust probe 都按 12-byte、data offset 4 互相通信，因此专项会自洽通过，却与目标
  架构 libc 使用的自然对齐 16-byte、offset 8 不兼容。真实 CMake/libuv 随后把 padding/data 误读为 fd，
  触发 watcher 数组断言。修正内核时必须同步修正 probe，否则旧 probe 反而会制造假回归。
- 后续影响：手写 syscall struct 时先核对目标 UAPI 的 `sizeof`/`offsetof`，并用真实 libc workload 与
  非零 64-bit sentinel 验证 round-trip；测试与实现共享同一个错误常量不是独立 oracle。

## pathname 的 4096-byte 上限不能复用于 exec argv/env

- 状态：已确认并修复
- 适用范围：`execve`、shell `-c`、CMake/Ninja/Make 生成命令、ARG_MAX
- 最后验证：2026-08-16
- 证据：guest 内 CMake 约 11.6 KiB `/bin/sh -c` 参数；`os/src/mm/mod.rs`；双架构 self-build 日志
- 内容：路径通常受 page-sized name buffer 约束，但 Linux 单个 argv/env string 的边界是 32 pages，
  过长应为 `E2BIG`。旧 `extract_cstrings_from_user()` 复用 pathname helper，导致合法长参数先以
  `ENAMETOOLONG` 失败，表面上只是 make/CMake 子命令异常退出。
- 后续影响：pathname、exec 单字符串、argv/env 个数和总字节必须作为四类独立预算维护；扩大单项上限
  不能绕过整体 ARG_MAX，也不能把 path 的错误码一起改掉。

## 对 detached pthread 调用 join 不能作为可移植内核 oracle

- 状态：已确认；测试夹具已修正
- 适用范围：pthread detach/resource-reclaim probe、musl/glibc 差异归因
- 最后验证：2026-08-16
- 证据：`respos-software/libc-combination.c`、宿主 Linux 与 RV64 Alpine 首轮对照
- 内容：对 detached thread 再调用 `pthread_join()` 属于不可依赖的误用，libc 不保证统一返回
  `EINVAL`；RV64 Alpine 首版夹具在此前四线程组通过后因此段错误，但移除该断言、保持线程明确存活并
  用 condition 验证其完成后，双架构完整矩阵及各 8 轮压力均通过。
- 后续影响：线程回收测试应验证 detached worker 的可观察完成、同步对象不再被访问以及后续大量线程
  create/join 能继续成功；不得把 join detached 的返回值或崩溃直接归因为 clone/futex 内核回归。

## chroot 先检查 privilege 会把 pathname 与目录权限错误遮蔽为 EPERM

- 状态：已确认并修复
- 适用范围：`sys_chroot`、用户指针复制、namei、目录 search permission
- 最后验证：2026-08-14
- 证据：`scripts/chroot_permission_probe_linux.c`；修复前后双架构 musl/glibc
  `chroot01`--`chroot04` 聚焦日志
- 内容：把 `euid != 0` 放在 syscall 开头看似能快速拒绝非特权调用，却会让 invalid pointer、missing
  path、non-directory 和不可搜索目录都提前返回 `EPERM`。Linux 对这些调用先观察 pathname 与目录
  search error；只有目标可访问时才返回 privilege error。
- 后续影响：涉及 pathname 和 privilege 的 syscall 不能凭“权限失败更安全”统一提前判权；应先用
  Linux probe 固定错误优先级，并确保状态修改仍放在所有可能失败的检查之后。

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
- 适用范围：libc 组合接口、镜像内 musl/glibc 差异、LTP
  `pathconf02/readlink03/readlinkat02/epoll_create02/recvmmsg01`
- 最后验证：2026-08-14
- 证据：当前 RV64/LA64 初赛镜像 `/musl/lib/libc.so` 的
  `pathconf/fpathconf/readlink/readlinkat/epoll_create/recvmmsg` 符号反汇编；
  对应 musl/glibc LTP 日志
- 内容：当前镜像的 musl `pathconf(path, name)` 不访问 `path`，而是传
  `fpathconf(-1, name)` 后返回常量表。因此 `ENOTDIR/ENOENT/ENAMETOOLONG/EACCES/ELOOP`
  不可能由内核 namei 返回；即使修改 `statfs` 或路径解析也不会影响这条调用链。
- 内容：LA64 musl 1.2.5 的 `readlink/readlinkat` 又展示了另一种形态：wrapper 把零长度
  转换成内部 1-byte 缓冲区调用，使内核只能看到合法的截断读。内核既不能恢复原始
  `bufsiz==0`，也不能把所有 size 1 请求改为 `EINVAL`。RV64 musl 1.2.0 直接传参，所以
  同一内核上只有 LA64 musl 失败。
- 内容：RV64 musl 1.2.0 的 `epoll_create(size)` 无条件清零参数后调用合法的
  `epoll_create1(0)`；LA64 musl 1.2.5 已在 wrapper 中拒绝 `size <= 0`。现代 RV64/LA64
  没有 legacy `epoll_create` syscall，内核若为修 LTP 而拒绝 flags 0，会同时破坏所有合法
  `epoll_create1(0)` 调用。
- 内容：两架构 musl 的 `recvmmsg` wrapper 会在 syscall 前遍历 `msgvec`，清零每项
  `msg_iovlen/msg_controllen` 的高 32 位以适配 kernel ABI；对 LTP 的 guard/bad vector，
  这些用户态 store 先产生 SIGSEGV，内核没有机会返回 `EFAULT`。
  同一内核上的 glibc 两种 time ABI 错误矩阵全过，不能把 wrapper fault 归到 kernel uaccess。
- 后续影响：遇到只在一种 libc 复现的接口差异时，先从实际测试镜像导出 libc，
  确认参数是否到达 syscall，再决定内核所有者。不得特判 LTP runner；替换整套 libc
  必须单独协商并跑完整相关 workload。

## LA64 secondary 不得在 boot timer-service 就绪前进入 scheduler

- 状态：已确认并修复
- 适用范围：LA64 SMP 冷启动期间的 socket/nanosleep/futex/poll timeout
- 最后验证：2026-08-15
- 证据：修复前 2 hart 的 50 ms recv timeout 为 983 ms；临时诊断所见 boot/hart1 raw counter
  仅差 35738 ticks（约 0.357 ms）。修复后 release 初赛 snapshot 的 1/2/12 hart
  `socket_timeout_probe` 全过，12 hart online mask 为 `0xfff`。
- 内容：boot hart 为兼容小于 `MAX_HARTS` 的 `-smp` 最多等待 1 秒收集 online mask。secondary 若发布
  online 后立即进入 scheduler，可以在唯一 timer-service hart 尚未启用中断和编程首个 compare 时注册
  deadline，首组 timeout 因而延迟到启动轮询结束。此前由 affinity A/B 推断的“每 hart `rdtime.d`
  约 1 秒偏移”已被 raw counter 和 QEMU 全局 virtual clock 实现否定。
- 后续影响：secondary 可以先发布 online，但必须等待 boot hart 完成 bounded discovery、启用 timer
  interrupt 并编程首个 compare 后的 `BOOT_RELEASED`。不得用 affinity、扩大容差或未经证据的 per-hart
  时钟归一化掩盖启动顺序问题；修改 SMP boot/timer 顺序后重跑 1/2/12 hart timeout 与 futex 专项。

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

## recvmmsg timeout 不能按理想 deadline 自行唤醒

- 状态：已确认并按 Linux ABI 实现
- 适用范围：`recvmmsg` 非空 timeout、SA_RESTART、partial count、MSG_WAITFORONE
- 最后验证：2026-08-15；Linux oracle 与 RV64/LA64 socket probe 各 8 轮
- 证据：Linux 完全无消息的 200 ms timeout 运行 35 秒仍未返回；改为 300 ms 后发送 message 时返回 `1`
  且 remainder 为零。最终日志为 `/tmp/respos-{rv,la}-socket-recvmmsg-timeout-stress8.log`。
- 内容：Linux 只在成功接收每条 message 后检查并写回 timeout。普通 handler 的零进展 `EINTR` 保持
  timespec 原值；SA_RESTART 从该原值重新等待；partial count 保留上一条成功时写回的 remainder。
  这是已记录的历史行为，不等价于 poll/nanosleep 的独立 timer。
- 后续影响：不要因为接口有 timespec 参数就注册 task timeout waiter；那会让无消息调用按时返回，反而
  破坏目标 ABI。探针必须用有界 sender/signal 确保 Linux oracle 退出，不能直接等待 timeout 并让 CI
  永久挂住。

## signal 入队后的晚到 interrupted 写会伪造下一次 syscall 的 EINTR

- 状态：已确认并修复
- 适用范围：SMP signal enqueue、interruptible wait、连续两个阻塞 syscall
- 最后验证：2026-08-15；RV64/LA64 signal probe 各连续 8 轮
- 证据：扩展 epoll timeout 向量后，RV64 第 6 轮出现 `epoll_pwait` 正确返回 `EINTR`、handler 只执行
  一次，但紧随其后的 `wait4` 也返回 `EINTR`；`os/src/task/task.rs::mark_signal_interrupted()` 修复后
  signal/socket/wait4/task 双架构回归通过。
- 内容：发送核先入 pending queue，等待核可在发送核写 wake hint 前观察并消费 signal。若发送核随后仍
  无条件写 `interrupted=true`，该 bool 已不对应任何 pending signal，会污染下一次阻塞 syscall。只在
  syscall 返回点清一次 hint 不能闭合该先后关系，因为晚写可能发生在 clear 之后。
- 后续影响：发布 hint 后重新验证权威 pending 条件；若目标已非 interruptible 或 signal 已不可递送，
  立即撤销。不要靠用户态重试 cleanup wait 隐藏，因为同一缺陷会影响任意相邻阻塞调用。

## stop 先通知 parent、后写 Stopped 会丢并发 SIGCONT

- 状态：已确认并修复
- 适用范围：SIGSTOP/SIGTSTP 等默认 Stop、SIGCONT、WUNTRACED、SMP
- 最后验证：2026-08-15；双架构 signal 8 轮与 job-control 专项
- 证据：修复前无调试输出的完整 signal probe 多次停在 `sigchld_autoreap PASS` 之后；GDB 显示两 hart
  都在 `run_tasks` idle，而添加阶段输出会改变时序并通过。修复后日志为
  `/tmp/respos-{rv,la}-signal-clock-nanosleep-stress8.log`。
- 内容：旧 Stop action 先写 wait event/唤醒父进程，随后 `stop_current_and_run_next()` 才写 `Stopped`。
  父进程若立即 SIGCONT，会因仍看到 `Running` 而不 enqueue child；child 随后停住，父进程等待回收，
  系统最终无 ready task。即使提前写 `Stopped`，handoff 再写一次也会覆盖并发 SIGCONT 的 `Ready`。
- 后续影响：正确顺序是 `Stopped -> wait/SIGCHLD publication -> state-preserving handoff`。遇到仅加打印就
  消失的 job-control hang，应优先审计状态发布顺序，不能把它记录为偶发调度抖动。

## wait 只用 exited-child 集合兜底会丢 stop/continue 唤醒

- 状态：已确认并修复
- 适用范围：wait4/waitid、WUNTRACED/WCONTINUED、SA_NOCLDSTOP、SMP
- 最后验证：2026-08-15；RV64/LA64 signal probe 各连续 8 轮
- 证据：加入 pipe read restart 后改变调度时序，probe 两次稳定停在 `SA_NOCLDSTOP` 尾段；临时输出改变
  时序后通过。ProcessState child-event generation 修复后，无时序输出双架构 8 轮通过，日志为
  `/tmp/respos-{rv,la}-signal-read-restart-stress8.log`。
- 内容：child 可在父进程完成 children scan、但尚未设置 waiting flag 时发布 stop。通知看不到 waiter；
  父进程随后发布 Blocked，若只检查 `exited_child_ids` 就永远睡眠，因为 stop/continue 不是 exit。
- 后续影响：所有“先检查条件、再登记 waiter”的接口都要有同一锁或 generation/sequence handshake；
  调试打印、额外 yield 或仅检查退出队列都不是 lost-wakeup 修复。

## pid-specific wait 不能因无关旧 zombie 持续撤销睡眠

- 状态：已确认并修复
- 适用范围：wait4/waitid、多个子进程、LA64 SMP synchronous TLB shootdown
- 最后验证：2026-08-15；RV64/LA64 futex race 各 20 轮
- 证据：`os/src/syscall/process.rs::wait_block_current()`；
  `/tmp/respos-{rv,la}-futex-timed-norestart-waitfix.log`
- 内容：等待 child A 时，`exited_child_ids` 中可能已有尚未回收的 child B。若 Blocked 后仅因该集合非空
  就撤销睡眠，父进程会在权威 children scan 与登记之间永久忙循环。LA64 内核态 IE=0 时还会阻止该 hart
  响应 child A 退出回收所需的 TLB shootdown，形成 wait↔exit 闭环。GDB 证据显示一核停在
  `remote_tlb_shootdown`，另一核在 `sys_wait4` 重复扫描/分配。
- 后续影响：scan→register 竞态复查只比较扫描前保存的 child-event generation，并保留 signal/ready 条件；
  旧 exit 集合不是“本轮扫描后发生事件”的证据。具体 child 是否匹配仍由下一轮 pid/pgrp/options 扫描决定。

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

## `UTIME_NOW` 要按文件系统精度比较，双 `UTIME_OMIT` 是特殊 no-op

- 状态：Linux/RV64/LA64 专项已验证
- 适用范围：`utimensat/futimens`、`UTIME_NOW/UTIME_OMIT`、timestamp validation、path lookup
- 最后验证：2026-08-15
- 证据：`utimens_special_probe`，日志 `/tmp/respos-{rv,la}-utimens-special.log`
- 内容：文件系统可以把指定时间向下量化到自身支持的精度，所以 `UTIME_NOW` 不能与调用前纳秒级
  clock 值做严格不小于比较；应使用声明的精度窗口。Linux 的双 OMIT 更特殊：它不改 atime/mtime/ctime，
  并且 pathname 不存在也返回成功；这要求在 lookup 前识别 no-op。普通非法 nsec 则必须先完整校验，
  不能先发布另一个 NOW/显式字段。
- 后续影响：不得把双 OMIT 的早返回推广到单 OMIT 或其他向量，也不得用秒级 probe 宣称纳秒已落盘。
  时间专项必须区分“特殊值选择状态机”“权限分类”“当前 realtime 来源”和“文件系统持久化精度”四层
  证据。尤其双 NOW/times NULL 属于 current-time：非 owner 可凭 inode 写权限成功、无写权限返回
  `EACCES`；显式时间和 `NOW+OMIT` 属于 arbitrary，普通非 owner 应返回 `EPERM`，不能只复用 write bit。

## RTC 接入后，所有 wall-clock metadata 必须同步退出 monotonic 假设

- 状态：已确认并修复当前 filesystem/IPC 调用点；RTC ioctl 后续已拆分为独立硬件域
- 适用范围：`CLOCK_REALTIME` 初始化、`UTIME_NOW`、自动 inode 时间、SysV IPC timestamp、`RTC_RD_TIME`
- 最后验证：2026-08-16
- 证据：首次 LA64 `utimens_special_probe` 在 NOW window 断言失败；修复后双架构即时、跨重启和 LTP 通过
- 内容：仅让 `clock_gettime(CLOCK_REALTIME)` 读取 RTC 会把原来同为“开机时间”的两套时钟拆开：
  filesystem 若继续用 `get_time_ms()`，`UTIME_NOW` 会落在 1970，而用户对照已经是日历 epoch。wall-clock
  metadata 必须统一走 realtime helper；timeout、uptime 和 CPU accounting 则必须保留 monotonic。
  `RTC_RD_TIME/RTC_SET_TIME` 是硬件时钟接口，不属于 system realtime metadata，不能复用 realtime offset。
  另外，设备映射列表不等于设备枚举列表：RV64 的普通 RTC MMIO 若加入 `VIRTIO_MMIO`，会改变 block
  transport index 并使根盘初始化失败。
- 后续影响：增加新的绝对 timestamp 时必须明确 clock domain；测试既要断言 calendar epoch，也要断言
  调整 realtime 不影响 monotonic。平台 RTC 默认 enable 状态不能凭固件经验假设，LA64 TOY 需显式启用。

## ioctl command 必须在 syscall ABI 边界截断为 32-bit unsigned int

- 状态：已确认并修复 RV64 musl `hwclock`，双架构双 libc 实测
- 适用范围：所有 ioctl、尤其 direction bit 置位的 `_IOR/_IOW` command
- 最后验证：2026-08-16
- 证据：RV64 musl 实际传入 `0xffffffff80247009`；修复后
  `/tmp/respos-{rv,la}-rtc-set-phase5-current.log` 中两套 BusyBox `hwclock -r` 与专项均通过
- 内容：Linux `ioctl(int, unsigned int, ...)` 在 syscall 入口只观察低 32 bit；RespOS 若直接拿 64-bit
  `usize` 比较，libc 从有符号立即数路径装载 `_IOR` 常量时会因高位符号扩展而误报 `ENOTTY`。应统一在
  `sys_ioctl` 入口 `as u32`，不能给每个设备重复增加 sign-extended 常量。
- 后续影响：新增 ioctl dispatcher 复用规范化后的 command；探针需包含至少一个 direction bit 置位命令
  和真实 libc 工具，纯 Rust 零扩展常量不足以覆盖该 ABI。

## QEMU RTC reset persistence 不等于跨进程电池后备

- 状态：已确认并双架构 reset 验证
- 适用范围：goldfish `tick_offset`、LS7A `offset_toy`、reboot、RTC 测试结论
- 最后验证：2026-08-16
- 证据：QEMU `hw/rtc/{goldfish_rtc,ls7a_rtc}.c` reset/realize 状态机；
  `/tmp/respos-{rv,la}-rtc-reset-persist-phase5.log`
- 内容：两个设备在 system reset 时清 alarm/control，但保留已写的 time offset；新 QEMU 进程 realize 时
  又从 `-rtc base` 初始化 offset。专项必须在同一 QEMU 内经历真正第二次内核启动，不能用 set 后立即读回
  代替；也不能把该结果描述成跨 QEMU 进程掉电保持。
- 后续影响：若平台提供 RTC state/NVRAM 后端，再建立跨进程门禁；当前本地证据只申报 device-reset
  persistence。

## ext4 extra timestamp 必须把 signed low seconds 与 epoch 位一起处理

- 状态：已实现并在扩展/128-byte 旧 inode 上双架构跨冷启动验证
- 适用范围：ext4 atime/mtime/ctime raw inode 编解码、组合 setattr、旧 inode layout
- 最后验证：2026-08-15
- 证据：Linux oracle、`ext4_timestamp_phase5_probe`，日志
  `/tmp/respos-{rv,la}-ext4-persist-{prepare,verify}.log`
- 内容：`i_*time` 低 word 不是简单 u32 epoch seconds；解码必须先 sign-extend 为 i32，再加
  `(extra & 3) << 32`，纳秒为 `extra >> 2`。写入也必须由目标 signed 秒反推出 epoch，且在修改 mode/
  owner/任一时间字段前验证全部 requested timestamp。只保存 u32 秒或只在 Rust cache 保存 nsec 会在
  reboot/reopen 后丢语义。
- 后续影响：不能用当前可见 cache 作为持久化证据；至少一次测试必须对同一磁盘重新启动并从 raw inode
  读取。Linux VFS 对超范围值做 clamp，并在命中 min/max 时把 nsec 清零；不能误实现成 `ERANGE` 或
  保留端点 nsec。无 extra field 时上界改为 `INT32_MAX` 且所有 nsec 清零；Rust cache 必须在 lower commit
  后发布这个实际可表示值，不能等 reopen/reboot 才显现退化。当前两种 layout 均已跨重启验证。

## 自动 atime 更新不能复用会刷新 ctime 的显式 utimens 路径

- 状态：已确认并修复 BuildStorm 写放大；regular-file/directory、24-hour 与 lazytime 已双架构专项验证
- 适用范围：relatime/lazytime、read/readdir、ctime、ext4 metadata writeback
- 最后验证：2026-08-16
- 证据：RV64 8 GiB/8 核固定 120 秒 attributes 计数 A/B；Linux oracle 与
  `/tmp/respos-{rv,la}-atime-lazytime-phase5-final.log`
- 内容：旧实现每次自动 atime 都同时把 ctime 更新为 now，使下一次 relatime 判断继续满足
  `atime <= ctime`。58,692 次 set-times 中有 58,682 次来自自动 atime，形成约 29 万次块写请求。
  自动访问更新改为不碰 ctime 后，atime 落盘降至 1,185 次，块写请求降至 9,663；显式 `touch -a`
  仍同时更新 ctime。
- 后续影响：自动访问时间和显式 `utimensat/futimens` 必须走语义不同的入口。不能为复用代码让普通
  read 改 ctime，也不能反向让显式时间修改漏掉 ctime；relatime 回归需检查第二次读取不重复落盘。
  directory `readdir` 同样适用，并且 `MS_NODIRATIME` 只能抑制目录，不能扩散到普通文件。

## 普通 inode 的 atime 不能归 open-file description 私有缓存所有

- 状态：已由 lazytime 首轮 RV64 门禁定位并修复，双架构 release/perf 专项通过
- 适用范围：同 inode 多 fd、pathname utimens、relatime 判断、lazytime stat 可见性
- 最后验证：2026-08-16
- 证据：修复前 pathname `utimensat` 已把 atime 设为 100，但旧 fd 仍以先前 override 判断并抑制 read；
  修复后 `/tmp/respos-{rv,la}-atime-lazytime-{,perf-}phase5-final.log` 通过
- 内容：ext4 inode 已有按 `(fs_id, ino)` 共享的 metadata cache。再把自动 atime 写入 `FileInner`
  会产生第二份、只对一个 open-file description 可见的所有权；其他 fd/pathname 更新 inode 后无法使它
  失效。普通 ext4 的自动/显式时间必须只发布到 inode cache；只有尚无 lower inode 的 tmpfile 可以使用
  open-file override。
- 后续影响：遇到跨 fd 可见性失败时先检查状态所有权，不要靠 reopen 掩盖。lazytime 延迟的是持久化，
  不是 inode 内存可见性；`fsync`、filesystem sync 和 remount-off 必须在 lower barrier 前提交 pending。

## lazytime registry 保活会隐藏真实 eviction，aging 也不能使用 realtime

- 状态：已由 background/eviction 专项确认并修复，双架构 release/perf 与 crash-image 通过
- 适用范围：lazytime dirtytime aging、inode/dentry reclaim、`clock_settime`、ext4 lower metadata I/O
- 最后验证：2026-08-16
- 证据：`/tmp/respos-{rv,la}-atime-dirtytime-eviction-phase5{,-perf}.log`、
  `/tmp/respos-{rv,la}-lazytime-crash-{prepare,verify}.log`
- 内容：pending registry 为防 inode 在 durability boundary 前消失而持有强引用；若通用 eviction 只看
  `Arc::strong_count`，这个保活引用会让最后 dentry 逐出永远看似“仍有 owner”。逐出路径必须识别
  registry + 当前调用这两个内部引用，先释放 dentry/cache 锁，再调用可能进入 lwext4 的 flush。dirtytime
  年龄记录首次 dirty 的 monotonic 时刻；若使用 realtime，`clock_settime` 会制造提前到期或无限延迟。
- 后续影响：新增 inode cache/reclaim 路径必须接入同一 eviction hook，且不得持有 dentry、registry 或
  scheduler 锁进入 lower I/O。失败不能丢 pending/owner；重复 atime 更新不能刷新首次 dirty 时刻。

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

## 用 `BTreeMap<signo, SigInfo>` 表示 pending 会同时破坏标准与实时信号语义

- 状态：已确认并修复当前队列范围
- 适用范围：`SigPending`、`rt_sigqueueinfo`、`rt_sigtimedwait`、`RLIMIT_SIGPENDING`
- 最后验证：2026-08-15；Linux/RV64/LA64 oracle/probe 与双架构 LTP `tgkill02`
- 证据：旧 `add_signal()` 对同号执行 `insert`，会覆盖首条 info；修复后日志
  `/tmp/respos-{rv,la}-signal-rtqueue-{quota,tgkill02}.log`
- 内容：标准信号 pending 时应合并并保留首个实例信息，实时信号则必须同号 FIFO 多实例排队。仅有
  bitmap 加单值 map 会让标准信号错误返回最后 value，也让实时信号静默丢实例。反过来，无界 Vec 队列
  又会令 `tgkill02` 的 limit=0 错误成功；必须在 enqueue 前 reserve 配额，最后实例 pop 后才清 bitmap。
- 后续影响：队列测试必须同时断言 info value 顺序、耗尽后的 pending 位、配额 `EAGAIN` 和消费后恢复；
  只测试 handler“至少执行一次”无法发现覆盖和额度泄漏。

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

## chunk 级 append 锁不等于整次 pwrite syscall 原子

- 状态：已确认，待修复
- 适用范围：大于 `IO_CHUNK_SIZE` 的 `pwrite/pwrite64`、`O_APPEND`、并发 writer
- 最后验证：2026-08-14；release、4 GiB/2 hart
- 证据：`os/src/syscall/fs.rs::sys_pwrite64()`、`File::pwrite_at_offset()`；Linux/RV64/LA64
  `pwrite_append_atomic_probe`
- 内容：每个 64 KiB chunk 都能在 open-file inner lock 下原子选择 EOF 并写入，但 syscall 循环会在
  chunk 之间释放锁。另一个 writer 因而可把自己的 chunk 插入同一 128 KiB record；默认 RV64 已自然
  复现，强制让出时双架构稳定 16/16 复现。小写 LTP 或最终长度正确都无法发现 record 内交错。
- 后续影响：不得直接把 spin lock 扩到 `copy_from_user`、page fault 或 yield 周围；单核竞争会失去
  进展。修复必须显式定义 syscall 级可睡眠 ownership 或 range reservation 的失败回滚，并测试不同
  open description、EFAULT、short write、truncate 和文件长度观察者。

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

## extent 中段 split 后必须释放物理块，且 `st_blocks` 不能由 size 推算

- 状态：已确认并修复 ext4 punch-hole
- 适用范围：lwext4 extent remove、`PUNCH_HOLE`、stat、delayed inode metadata
- 最后验证：2026-08-16；Linux/RV64/LA64 mmap punch 专项
- 证据：`vendor/lwext4_rust/c/lwext4/src/ext4_extent.c`、`os/src/fs/ext4/inode.rs`、
  `/tmp/respos-{rv,la}-mmap-punch-phase5-errors.log`
- 内容：旧的 extent 中段删除分支能拆出左右 extent，却漏掉 physical block release；读取表面已呈现 hole，
  但块仍泄漏。若 VFS 又用逻辑 size 计算 `st_blocks`，专项甚至无法发现释放是否真实发生。另一个覆盖
  陷阱是 punch 后再提交旧 delayed timestamp：stale inode 快照会恢复旧 `i_blocks`。
- 后续影响：打洞门禁必须同时检查 lower 重读为零、size/offset 不变和 `st_blocks` 下降；破坏 extent 前
  先 flush 旧 metadata，inode ref 必须在 write-back cache 模式退出前提交。不要把普通 `fallocate04`
  的预分配 skip 误报成 punch-hole 结果。

## `ext4_fwrite` 不能用 inode-ref 提交结果覆盖原始写错误

- 状态：已确认并修复 ENOSPC 错误保真与 cache-mode 配对
- 适用范围：lwext4 partial write、ENOSPC/EIO、PageCache writeback、shared mmap page-mkwrite
- 最后验证：2026-08-16；双架构 16 MiB auxiliary ext4 满盘专项
- 证据：`vendor/lwext4_rust/c/lwext4/src/ext4.c`、
  `/tmp/respos-{rv,la}-mmap-enospc-phase5-final.log`
- 内容：写循环已记录 `ENOSPC` 且可能完成部分块后，旧 Finish 路径把 `r` 覆盖成
  `ext4_fs_put_inode_ref()` 的成功，Rust 只能从短计数合成 `EIO`；通过 `goto out_fsize` 的错误出口还会
  跳过 block-cache write-back disable。结果既丢失错误类别，又可能让后续 I/O 留在错误的 cache nesting。
- 后续影响：operation result、cleanup result 和 transaction commit 必须分开保存；inode-ref 成功时提交
  已完成的 partial write，但返回原始 operation error。所有启用 cache write-back 的路径都必须在统一
  Finish 中配对关闭，新增 C API 也应专项检查错误出口。

## synthetic mount 丢失 mkfs geometry 会把填盘测试伪装成 writeback 故障

- 状态：已确认并修复
- 适用范围：lightweight block mount、no-op mkfs、filesystem capacity/block size、LTP `mmap16`
- 最后验证：2026-08-16；RV64/LA64 musl/glibc `mmap16` 各 10/10
- 证据：`/tmp/respos-rv-mmap16-synthetic-fs-capacity.log`、
  `/tmp/respos-{rv,la}-mmap16-pagecache-isize-extended-v2.log`
- 内容：`mmap16` 的首个 checkpoint 正常，parent 也进入填盘循环；逐次 writeback 全部成功且隐藏 backing
  文件增长到数百 MiB，说明不是后台 `ENOSPC` 被吞掉。根因是 `/dev/vda2` 只提供设备 admission size，
  mount backing 却没有继承 no-op mkfs 请求的 10 MiB capacity/1 KiB block size。因而 parent 永远填不满
  实际根文件系统，child 才在第二个 checkpoint 超时。
- 后续影响：synthetic device size 与 formatted filesystem geometry 必须分开；no-op formatter 也要把
  capacity/block size 传给 mount emulation。遇到填盘不结束时先记录 backing 的实际增长与 lower write
  结果，不能仅从“buffered write 一直成功”推断 writeback-error 生命周期有缺陷。

## LoongArch 写保护必须同时清 PTE W 与 D

- 状态：已确认并修复
- 适用范围：page-mkwrite、COW、任何要求下一次 store 重新 fault 的 PTE 降权
- 最后验证：2026-08-16；双架构 `mmap16` 与 Phase 5 mmap probe
- 证据：清 W 后 RV64 `mmap16` 10/10、LA64 0/10；清 W/D 后 LA64 musl/glibc 均 10/10
- 内容：公共 `PTEFlags::WRITE` 映射到 LoongArch PTE W，但硬件 PageModifyFault 还由 D 位决定。保留旧
  DIRTY 只清 WRITE 时，LA 可继续 store 而不进入 page_mkwrite；RV64 因权限模型不同不会暴露该错误。
- 后续影响：建立 read-only/COW/page-mkwrite 边界时沿用 `WRITE | DIRTY` 成对清除；页表修改后仍须完成
  本地及远端 TLB shootdown，不能用架构单测结果外推另一架构。

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

## AF_UNIX abstract 名称不是 UTF-8 pathname，unnamed 也不是 named peer 的占位值

- 状态：已确认并修复 stream 地址回报
- 适用范围：AF_UNIX bind/connect/accept、`getsockname/getpeername`、地址截断
- 最后验证：2026-08-14
- 证据：`scripts/getpeername_probe_linux.c`、`user/src/bin/getpeername_probe.rs`；双架构 2 hart 专项
- 内容：abstract namespace 用前导 NUL 加任意字节标识，允许非 UTF-8；pathname identity 不含 ABI
  输出时的结尾 NUL。若只保存 `String` 或 connected bool，socketpair 会通过，但 named client 的 peer、
  accepted endpoint 的 local/peer 和 accept 输出都会错误退化为 2-byte unnamed 地址。
- 后续影响：地址 probe 必须同时绑定 listener/client，检查 client 与 accepted endpoint 双向查询，并用
  小 buffer 验证“copy 截断但 actual addrlen 不截断”；不能只检查 `sa_family`。

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

2026-08-15 增量：pathname connect 在 accept queue 满时也必须走同一 pending-lock waiter 协议；阻塞
socket 直接返回 `EAGAIN` 是可见 ABI 错误。accept 取走 endpoint 后唤醒一个 connector，listener close
同时唤醒 accept/connect 两类 waiter；restart 分类还必须检查 `SO_SNDTIMEO`，不能只看 syscall number。

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
  另一个独立陷阱是为了避免 child exec 覆盖 parent 而让非线程 vfork 忽略 `CLONE_VM`：这会保留等待
  顺序，却使 child 在 exit 前的用户内存写入对 parent 不可见，LTP `clone05` 因此稳定读回 0。当前
  per-task 可替换 MM handle 已解决 exec 隔离，不能恢复该过期例外。
  cargo/rustfmt 曾表现为捕获输出不返回，但最新 trace 已确认顺序修复后 parent 正常恢复且 rustfmt
  完成退出，仍有独立的 pipe 引用未释放问题；两者不能混作同一个根因。
- 后续影响：vfork 同步必须是一次性且只限该 clone 关系；exec 仅在新映像状态完整后释放，退出路径
  也必须释放以覆盖 exec 失败。父 waiter 必须先登记，child 才能进入 ready queue；不要以普通
  SIGCHLD、yield 或单核通过代替该协议。共享可见性与 parent wakeup 是两条独立门禁，分别以
  `clone05` 和 vfork/exec command workload 验证。

## 不要为旧 LA64 glibc 的 64 KiB `SHMLBA` 改写当前内核 ABI

- 状态：已确认
- 适用范围：LA64 `shmat(SHM_RND)`、glibc 2.38/LTP 与 musl/current Linux 混用
- 最后验证：2026-08-14；release、4 GiB/2 hart
- 证据：`os/src/syscall/ipc.rs::sys_shmat()`、镜像 `/glibc/lib/libc.so.6` 与两套 LTP `shmat01`；
  Linux `d23b77953f5a`、glibc `cae3c9e3a117`
- 内容：旧 LA64 glibc header 把 `SHMLBA` 固定为 64 KiB，musl 和当前 Linux/glibc 使用
  `PAGE_SIZE`。LTP 用各自编译期常量构造输入和期望，因此同一 4 KiB 内核会表现为 musl 通过、旧
  glibc 期望向下多舍入 60 KiB。syscall flags 不携带 libc 的 header 版本，内核无法无歧义兼容两种
  rounding。
- 后续影响：不得按进程名、ELF/libc 或地址尾数猜测 `SHMLBA`，也不得全局改成 64 KiB 让当前 musl
  回归。应更新 runtime/test image；SysV SHM 的跨 attach 共享身份由独立内核语义和 probe 验证。

## 映射实例 identity 不能作为 shared futex backing identity

- 状态：已确认并修复
- 适用范围：SysV SHM 重复/跨进程 `shmat`、不同虚拟地址上的 shared futex
- 最后验证：2026-08-14；release、4 GiB/2 hart
- 证据：`os/src/mm/memory_set.rs::shared_futex_key()`；Linux、RV64、LA64
  `sysv_shm_futex_probe`
- 内容：每次 attach 唯一的 id 适合把 VMA 分片归入一次 `shmdt`，却不代表共享 backing。用它构造
  futex owner 时，同一 segment 的两个地址即使共享数据 frame 也会进入不同队列，表现为 sentinel
  可见但 wake 返回 0。同步 identity 必须来自两侧共同持有的 frame/backing，并另用页内 offset 区分
  futex 字。
- 后续影响：设计共享映射元数据时应分别列出“映射生命周期 identity”和“共享内容 identity”；至少用
  不同虚拟地址的跨进程 wait/wake 验证，不能以数据读写可见性替代 futex 证明。

## 回收 MemorySet 不会自动清理独立的 SysV SHM table

- 状态：已确认并修复 exit/exec 基本路径
- 适用范围：已 `IPC_RMID` segment、进程不调用 `shmdt` 而 exit/exec、fork/`CLONE_VM`
- 最后验证：2026-08-14；release、4 GiB/2 hart
- 证据：`os/src/task/task.rs`、`os/src/syscall/ipc.rs::release_shm_attachments()`；修复前后双架构
  `sysv_shm_lifecycle_probe`
- 内容：旧实现只在 `sys_shmdt()` 扫描 marked-removed segment。exit/exec 虽回收 PTE，`SHM_TABLE`
  仍持有 frames 和旧 shmid，故旧 id 可再次 attach。不能把 cleanup 塞进 `Drop<MemorySet>`：table 是
  外部 owner，且 fork/CLONE_VM peer 可能仍持有同一 attach identity。
- 后续影响：地址空间 teardown 应先让 mapping 对 task 不可达，再显式提交 detach，并以 live MM 复核
  最后 owner。测试必须同时覆盖“最后 owner 退出应删除”和“child 退出但 parent 仍持有时不得删除”。

## task 数量不是 SysV SHM attachment 数量

- 状态：已确认并修复
- 适用范围：`shm_nattch`、pthread/`CLONE_VM`、fork、`IPC_RMID` 最后 owner 判断
- 最后验证：2026-08-14；release、4 GiB/2 hart
- 证据：`os/src/syscall/ipc.rs::shm_attach_count()`；Linux、RV64、LA64
  `sysv_shm_nattch_probe`
- 内容：一个线程组的多个 TCB 可以共享同一 `MemorySet`，逐 task 扫描会把其中每个 attach 重复 N 次。
  相反，fork 复制出独立 MM 后，继承的每次 attach 确实应在 parent/child 分别计数。正确的两级身份是
  先去重 MM，再在 MM 内去重 attach id。
- 后续影响：任何按 live task 推导资源 owner 的统计都应先明确资源属于 thread、process 还是 MM；
  不能因为 thread probe 最终退出后数值恢复，就忽略其存活窗口内的 ABI 错误。

## table owner 不是已提交的 SysV SHM attachment

- 状态：已确认并修复
- 适用范围：并发 `shmat` 与最后 detach/`IPC_RMID`、attach 失败回滚、非空 `shmaddr`
- 最后验证：2026-08-15；release、4 GiB/2 hart；双 attacher、128 轮顺序回收循环
- 证据：`os/src/syscall/ipc.rs::sys_shmat()`；修复前后 RV64/LA64
  `sysv_shm_attach_race_probe`
- 内容：`attach_owners` 提前插入只说明一次 attach 正在建立，按 live MM 扫描时还看不到对应 VMA。
  若回收只检查已安装 attachment，最后 detach 会删除 segment，随后 attach 仍可安装共享 frames 并返回
  成功，形成无法 `IPC_STAT` 的孤儿映射。另一个容易掩盖回滚缺陷的错误是把非空 `shmaddr` 当 mmap
  hint：地址冲突会被静默搬家，使预期失败路径根本没有发生。
- 后续影响：跨不同 owner/锁域的资源发布必须区分 reservation 与 committed state，并让删除条件同时
  观察两者；失败测试应断言精确 errno 和最终回收，不能只检查返回地址为正。当前双 attacher 与顺序
  回收循环结果不能外推到任意 N 路并发或资源上限耗尽；`SHM_REMAP` 的覆盖语义也需要单独验证。

## `shmget(key, size, 0)` 没有隐含 read 请求

- 状态：已由 Linux/RV64/LA64 probe 确认
- 适用范围：existing-key `shmget`、SysV permission mode、`IPC_STAT/SHM_STAT/SHM_STAT_ANY`
- 最后验证：2026-08-15；release、4 GiB/2 hart
- 证据：Linux/guest `sysv_shm_metadata_probe`
- 内容：`shmflg` 低 9 位表达调用者本次请求检查的权限；全部为 0 时只查询 id，即使 segment mode 为
  `0000` 也可成功。只有明确请求 `0400/0200` 且对应 owner/group/other bit 不满足时才返回 `EACCES`。
  这不同于 `IPC_STAT/SHM_STAT` 固定要求 read permission；`SHM_STAT_ANY` 又显式忽略 read mode。
- 后续影响：不得把“mode 为 0000”实现为禁止所有 `shmget` lookup，也不得让 `SHM_STAT_ANY` 复用普通
  stat 的权限拒绝；权限 probe 必须分别覆盖零请求、read 请求和 ownership-only 控制操作。

## `mprotect()` 非 `EINVAL` 失败不保证整段权限回滚

- 状态：规范边界已确认；当前参数/权限/映射缺口向量已双架构验证
- 适用范围：`mprotect()`、VMA split、PTE permission、private-page allocation、file backing mode
- 最后验证：2026-08-15
- 证据：POSIX `mprotect()`；Linux/RV64/LA64 `mprotect_failure_probe`
- 内容：POSIX 明确允许 `mprotect()` 因 `EACCES/ENOMEM` 等非 `EINVAL` 原因失败时，范围内一部分页面
  权限已经改变。未知 prot 或未对齐地址的 `EINVAL` 应在修改前返回；当前 unmapped-hole probe 只验证
  Linux errno，不把 RespOS 当前较强的整段预检查误写为标准强制事务性。
- 后续影响：设计内存压力或 VMA 上限门禁时必须同时记录返回值与每段最终权限，但判断结果要服从规范
  允许的部分修改。若项目主动选择更强的全回滚保证，应明确 prepare/commit 与资源预留机制，不能用
  一条 hole 测试外推所有中途分配失败都原子。

## QEMU 10.0.2 LoongArch `LDPTE` 会截掉 PTE 的 NR/NX 高位

- 状态：已确认并为 `PROT_NONE` 增加兼容表示
- 适用范围：QEMU 10.0.2 TCG、LA64 软件 TLB refill、PTE bit 61 NR/bit 62 NX
- 最后验证：2026-08-14；release、4 GiB/2 hart
- 证据：[QEMU v10.0.2 `helper_ldpte()`](https://github.com/qemu/qemu/blob/v10.0.2/target/loongarch/tcg/tlb_helper.c#L535-L590)；
  [当前 QEMU 主线 `loongarch_sanitize_hw_pte()`](https://github.com/qemu/qemu/blob/master/target/loongarch/tcg/tlb_helper.c#L653-L664)；
  `os/src/arch/loongarch64/mm/page_table.rs`；修复前后 LA64
  `mmap05` 日志见 `current-status.md`
- 内容：v10.0.2 从 guest 页表读出 leaf 后对整个值执行 `& TARGET_PHYS_MASK`，bit 61/62 在写入
  TLBRELO 前已消失；CPU 虽声明支持 read-inhibit，TLB 中实际没有 NR，所以 `PROT_NONE` 可被读取。
  当前 QEMU 主线已只对 PPN 部分施加物理地址宽度掩码，并保留硬件定义的 NR/NX/RPLV。RespOS 的
  位号没有错误，反复交换 NR/NX 或在 trap 层伪造信号都不能解决旧 refill 的截位。
- 后续影响：必须先区分 guest PTE 写入值与最终 TLB entry，再诊断权限失效；`PROT_NONE` 使用硬件
  `V=0` 加 software-present/PROTNONE 避开该问题。execute-only/write-only 等仍依赖 NR/NX 的组合
  不应在 QEMU 10.0.2 上未经专项测试就宣称最小权限严格生效；升级模拟器也不能替代目标比赛环境回归。

## LoongArch native QEMU 会在缺少 `HWCAP_LOONGARCH_UAL` 时拒绝启动 TCG

- 状态：已确认并修复
- 适用范围：LA64 BuildStorm runtime、ELF auxv、native `qemu-system-loongarch64`
- 最后验证：2026-08-15；平台 BuildStorm 失败日志与本地 4 GiB/2 hart 定向复验
- 内容：编译完成后出现 `TCG: unaligned access support required; exiting` 不是性能超时、
  目标 ELF 错误或块设备失败，而是 QEMU 通过 `getauxval(AT_HWCAP)` 没看到 UAL。必须像 Linux
  一样先检查 `CPUCFG1.UAL`，再暴露 HWCAP；不能无条件设 bit 2，也不应修改比赛脚本
  跳过 runtime 验证。
- 后续影响：复验先用 `LD_SHOW_AUXV=1 /bin/true` 确认 `AT_HWCAP: 4`，再按官方脚本的
  loader/library path 做数秒 `qemu-system-loongarch64 -machine none -display none` 烟测；无需重跑
  BuildStorm 编译。

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

## 动态下调 SysV SHM 配额不能驱逐已有 segment

- 状态：当前 flat/global SHM table 已双架构验证
- 适用范围：`/proc/sys/kernel/shmall`、`/proc/sys/kernel/shmmni`、`shmget`、`IPC_RMID`
- 最后验证：2026-08-15
- 证据：Linux `ipc/shm.c::newseg()`；RV64/LA64 `sysv_shm_attach_race_probe` 的
  `dynamic_limits=pass`，日志 `/tmp/respos-{rv,la}-sysv-shm-dynamic-limits.log`
- 内容：运行时限额是后续创建的准入条件，不是已有对象的回收指令。`SHMALL` 低于当前页数、或
  `SHMMNI` 低于当前 segment 数时，已有 segment 仍须可查询/使用，新建返回 `ENOSPC`；只有显式
  `IPC_RMID` 等生命周期操作降低用量后，创建才按新阈值恢复。
- 后续影响：不得在 sysctl setter 中扫描或删除已有 segment，也不得通过把当前计数截断到新上限来
  “修正”超额状态。若以后增加 IPC namespace 或并发 sysctl/create，必须保持相同准入语义，并为
  namespace 锁序和创建线性化另立门禁。固定配额下的“统计当前用量—判断额度—插入 segment”必须处于
  同一 table 临界区；双创建者一成一败已验证，但不能外推为 sysctl 写入并发也已线性化。

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

## 不要把所有非 established TCP 状态的 shutdown 错误合并成 ENOTCONN

- 状态：Linux probe 已确认
- 适用范围：TCP `shutdown()`、listener、FIN 后状态与 errno oracle
- 最后验证：2026-08-15
- 证据：`scripts/tcp_half_close_probe_linux.c` 的向量筛选过程与 Linux `shutdown(2)` 契约
- 内容：当前 Linux 对从未连接的 stream socket 执行 `SHUT_WR` 返回 `ENOTCONN`，但 listener 上同一
  调用可返回成功；双向 FIN 已完成后再次 shutdown 的结果也受实际 TCP 状态推进影响。不能只按
  “不是 established”写一条全局 errno oracle。
- 后续影响：probe 应把规范明确且状态稳定的未连接、已连接非法 how、活跃 half-close 分开；listener、
  post-FIN、reset/linger 需先独立固定 Linux 状态时序，不能为了统一代码提前改 RespOS 错误优先级。

## UDP `SHUT_RD` 不是 stream 的接收丢弃模型

- 状态：Linux/RV64/LA64 probe 已确认并修复
- 适用范围：connected UDP `shutdown()`、`recv/recvfrom`、数据报队列
- 最后验证：2026-08-15
- 证据：`scripts/udp_shutdown_probe_linux.c`、`user/src/bin/udp_shutdown_probe.rs`；日志
  `/tmp/respos-{rv,la}-udp-shutdown-fixed.log`
- 内容：Linux 上 connected UDP 执行 `SHUT_RD` 后，已排队的数据报仍可读，队列为空时
  `recv` 返回 0；此后 peer 新发送的数据报仍会入队并能被下次 `recv` 取出。因此不能把
  UDP `SHUT_RD` 实现成 close 底层 socket，也不能在 shutdown 标志置位后无条件拒绝所有数据报。
- 后续影响：接收路径的顺序必须是“有数据报则消费，空队列才返回 shutdown EOF”。
  poll/epoll 还必须把 recv shutdown 观察为 `IN|RDHUP`，双半边 shutdown 才额外观察
  HUP；这些 level 与阻塞唤醒已由后续向量固定。shutdown EOF 与零长数据报都返回
  0，但只有后者具有 source address；不能根据长度猜测，必须在内部结果中显式区分。
  ET/ONESHOT、并发阻塞 send 与 recv 的 timeout/signal/data/shutdown 竞争仍需另立 oracle。

## RDHUP 观察 peer FIN，不能等接收缓冲读空

- 状态：Linux/RV64/LA64 probe 已确认并修复
- 适用范围：TCP/AF_UNIX stream、`POLLRDHUP/EPOLLRDHUP`、buffered data、read EOF
- 最后验证：2026-08-15
- 证据：`tcp_half_close_probe`；日志 `/tmp/respos-{rv,la}-tcp-rdhup.log`
- 内容：peer 的数据和 FIN 可以在同一观察窗口到达。此时普通 read readiness 来自缓冲数据，RDHUP 来自
  TCP peer-FIN state；两者应同时报告。若用 `!may_recv()` 推导 RDHUP，smoltcp 会因缓冲非空继续返回
  true，从而把 RDHUP 错误推迟到数据读空。AF_UNIX 同理应读取既有 `peer_write_shutdown/peer_closed`，
  不能用接收队列为空推导 half-close。
- 后续影响：RDHUP 必须是独立只读状态，不得消费数据、改写 socket state 或无条件升级为 HUP。epoll
  edge/oneshot 和 AF_UNIX 只订阅 RDHUP 的阻塞唤醒已由独立向量关闭；TCP 事件式 waiter、reset/linger
  仍需各自 probe，不能从 AF_UNIX 结果外推。

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

## 普通 mmap 的动态 EOF 不能套用到 ELF PT_LOAD

- 状态：已确认并修复
- 适用范围：file-backed fault、ELF loader、partial last page、BSS、private COW
- 最后验证：2026-08-15
- 证据：首轮 M3 实现后 RV64 musl/glibc `mmap05` 均在 loader 阶段 raw status 139；分离 live mmap 与
  fixed ELF backing、裁剪 fixed partial page 后 RV64/LA64 两套 libc 全部通过
- 内容：普通 mmap 必须在 fault 时读取当前 EOF以支持映射后增长；ELF `PT_LOAD` 只能读取 `p_filesz`
  prefix。若用当前完整文件长度替换 ELF backing.len，会把文件后续字节映入 BSS；若 fixed partial page
  直接共享完整 PageCache frame，也会越过 segment 有效尾部暴露数据，表现为动态 loader 随机地址 fault。
- 后续影响：修改 mmap EOF/COW 后不能只跑自定义 mmap probe；至少运行一个双架构动态 libc signal/
  PROT_NONE 用例（当前为 `mmap05`），并保留 fixed partial-page 零填充检查。

## user/system 记账不能把 process total 复制到两个字段

- 状态：已确认并修复
- 适用范围：`times`、`getrusage`、`wait4` rusage、`/proc/*/stat`、多线程 process CPU clock
- 最后验证：2026-08-15
- 证据：修复前 `sys_times/sys_getrusage/wait4` 均构造 `(ticks, ticks)`；Linux 对照与双架构
  `task_a_wait4_probe` 修复后通过，CPU total clock 各 20 轮不回退
- 内容：scheduler 运行区间只能回答 process total；把它同时写入 utime/stime 会把总量翻倍，并让 parent
  children usage 永久累计错误。只包 syscall 函数也不完整：page fault/signal/timer trap 属于 system，
  阻塞 syscall 的 task 切走后不应继续计时，恢复后却仍应保持 system mode。正确边界是 scheduler 管
  running，user trap 管 mode，Zombie 分别冻结两类 tick。
- 后续影响：专项必须同时包含主要为 user 的计算和真实 syscall 压力，并核对 reaped child 累计；仅检查
  CPU time 单调或两个字段非零无法发现 total 双写。

## 100 Hz rusage 专项不能用亚 tick 工作量断言非零

- 状态：已在 `RUSAGE_THREAD` 首轮 RV64 门禁确认并修正 probe
- 适用范围：`getrusage` 时间字段、短线程、双架构 CPU accounting 专项
- 最后验证：2026-08-16
- 证据：首轮 main thread 已执行计算但 `main_usage_delta == 0`；改为每个线程独立运行到跨过一个导出 tick
  后，Linux 与 RV64/LA64 20/20 通过，日志 `/tmp/respos-{rv,la}-rusage-thread-phase5.log`
- 内容：内部 accounting clock 精度高于 ABI 导出的 `CLK_TCK=100`；短于 10 ms 的累计在换算时向下取整为
  0 是合法结果。只固定迭代次数会随架构/QEMU 速度变成非确定测试。
- 后续影响：测试非零 rusage delta 时应循环到观测到一个 tick并设置有界上限；比较 process 与多个 thread
  时保留一个 tick/线程的量化容差。不要通过降低内核记账精度或伪造最小 1 tick 来迁就 probe。

## RUSAGE_CHILDREN 的 maxrss 不能求和，资源也不能在 copyout 前提交

- 状态：已确认并在 wait4/raw waitid 路径修正
- 适用范围：getrusage、wait4、waitid、Zombie/reap 生命周期
- 最后验证：2026-08-16
- 内容：Linux 对 reaped children 的 fault/context-switch 等累计求和，但 maxrss 表示任一 child 的最大
  high-water。若在 wait 扫描或 user copyout 前提交，坏 status/rusage 指针重试会重复累计；若 waitid
  reap 只删除 Zombie 而不提交，随后 `RUSAGE_CHILDREN` 又会永久少账。正确顺序是快照 Zombie、完成全部
  copyout、原子移除 child，然后一次性累计 ticks/resources。
- 后续影响：测试必须同时覆盖 bad pointer retry、wait4 与 waitid 第五参数 rusage，并用至少两个 child
  区分 max 与 sum；`WNOWAIT` 只能观察，不能累计。

## rusage block I/O 不能按 read/write syscall 次数或 writeback 次数记账

- 状态：已确认并由 Linux oracle、RV64/LA64 cold-file 专项验证
- 适用范围：`ru_inblock/ru_oublock`、VirtIO block、PageCache、mmap file fault、`fadvise64`
- 最后验证：2026-08-16
- 内容：Linux input block 在真实 read I/O 提交时计数，cache hit 不计；buffered output block 在 page 首次
  dirty 时计数，同一 dirty page 的重复写和稍后的 writeback 不能再计。若在 VFS read/write syscall 入口
  记账，cache hit 会被误报；若只在设备 write completion 记账，归属会漂移到 flusher，并与 Linux 时点
  不同。RespOS 因此把 input 归属放在成功 VirtIO read submission，把 output 归属放在 disk-backed page
  clean-to-dirty transition；boot/background I/O 没有 current task 时不记入任一用户进程。
- 后续影响：冷 major-fault 测试必须先确保 dirty data 已同步，再只驱逐 clean 且 unmapped 的 cache page；
  仅 munmap/reopen 不保证冷缓存。该 fixture 只证明 DONTNEED 的 clean full-page eviction 和后续 lower
  fill；完整 advice/open-description/writeback-before-invalidate 契约必须另跑下节的 fadvise 专项。

## per-CPU idle-stack handoff 不等于 Linux task context switch

- 状态：已修正并由双架构 20 轮 actual-switch 专项验证
- 适用范围：scheduler/processor handoff、`ru_nvcsw/ru_nivcsw`、yield/block/timer/exit
- 最后验证：2026-08-16
- 内容：RespOS 为避免另一 hart 在 outgoing context 尚未保存时提前恢复它，每次 yield/preempt/block 都先
  切到 per-CPU idle stack，再发布 task。若在请求 handoff 时立即加计数，单一 runnable task 每个 timer
  tick 都会虚增 `nivcsw`，单任务 `sched_yield` 也会虚增 `nvcsw`；Linux 只在 scheduler 最终发现
  `prev != next` 时增加。现在 handoff 携带原因，idle loop 选择 next 后才提交；next 是同一 task 时不计。
- 后续影响：probe 不能再用无竞争 `sched_yield` 制造 voluntary switch；应使用 nanosleep/blocked wait 等
  确实切向 idle 的路径。验证 involuntary 时把 parent 与 gate-controlled competitor 固定在同一 CPU，避免
  “只是经过 timer handler”或跨 CPU 同时运行造成假证据。

## `posix_fadvise` 不能把部分页失效或瞬时 residency 当成稳定 ABI

- 状态：已确认并由 Linux oracle、双架构专项验证
- 适用范围：WILLNEED、DONTNEED、PageCache、mincore、mmap pin、tmpfile
- 最后验证：2026-08-16
- 内容：Linux WILLNEED 只发起 best-effort readahead，返回时 `mincore` 是否已驻留受异步完成与回收影响；
  oracle 首版据此断言立即 resident，产生了非 ABI 失败。DONTNEED 对首尾部分覆盖页通常不失效，只有范围
  到 EOF 时末尾部分页可被丢弃；dirty、mapped 或被 frame 引用的页也可能保留。tmpfile 没有 lower storage，
  把其 clean PageCache 当普通磁盘缓存驱逐会直接丢数据。
- 后续影响：Linux 对照只断言 syscall/error/data 与稳定的完整页边界；RespOS 专项可用自身同步实现的 I/O
  计数固定策略效果，但不能将完成时序外推为 POSIX 契约。修改 eviction 时必须同时跑 mmap/SIGBUS、冷
  major fault/block-I/O 与 dirty writeback 回归。

## 零长 inode 不等于 mmap 一定 SIGBUS，grow-down fault 也不能只看 VMA flag

- 状态：已确认并修正
- 适用范围：`/dev/zero`、普通零长文件、`MAP_GROWSDOWN`、page-fault/user-copy
- 最后验证：2026-08-16；RV64/LA64 musl/glibc `mmap10` 1/1、`mmap18` 4/4
- 证据：`/tmp/respos-{rv,la}-mmap10-dev-zero.log`、
  `/tmp/respos-{rv,la}-mmap18-growsdown-sp.log`
- 内容：把所有 file-backed mapping 都套用 live EOF 会让 size=0 的 `/dev/zero` 在首次写时
  SIGBUS；正确分界是由设备显式声明零填充 mmap，不能放宽普通文件 EOF。另一方面，
  只要下方有 `MAP_GROWSDOWN` VMA 就扩展，会把任意坏指针和内核 user-copy 误当成栈增长；
  必须同时校验紧邻页、用户 SP 和 guard gap。
- 后续影响：新增 special file mmap 时应以 inode/file capability 表达语义，不以 size 或
  pathname 猜测。修改 trap frame、VMA split/merge 或 copyin/copyout fault 路径时，必须复跑 `mmap18`
  的成功与 blocker 两类用例。

## 物理 console 的控制字符不能只在 read 路径处理

- 适用范围：console line discipline、VINTR/VQUIT/VSUSP、job control、阻塞 syscall
- 现象：前台程序读 tty 时 Ctrl-C 正常，但 `sleep 10` 等不读 tty 的程序直到自然结束后才显示并处理
  `^C`；看起来像 signal 或 foreground pgrp 错误。
- 根因：固件串口是轮询输入，若只有 stdin/`/dev/tty` 的 `read/read_ready` 调用 line discipline，就没有
  读者替不读 tty 的前台作业消费控制字符。
- 处理：由唯一 global timer service hart 在 timer safe point 抽取 console；已阻塞的 tty read 检查
  当前线程实际可投递的 pending signal 后返回 `EINTR`。不要用 terminal 全局 generation 中断所有 reader，
  也不要在每个 hart 或任意持锁 kernel trap 中直接轮询并投递，否则会重复消费字符或重入
  task/signal registry。
- 最后验证：2026-08-16，RV64/LA64 Alpine ash 下 `sleep 10` 的 Ctrl-C/Ctrl-Z 均立即生效。

## 固件 console 的 destructive drain 必须整体串行

- 适用范围：SMP、timer safe-point input pump、前台 tty reader、SBI/UART polling console
- 现象：长命令偶发稳定变成字母次序错误的另一字符串，例如 `software` 变成 `softwrae`；queue 内容本身
  没有丢失，容易误诊为 shell 行编辑或 QEMU PTY 注入问题。
- 根因：timer service hart 与 foreground reader 可同时执行 `console_getchar()`；该读取会消费硬件字节。
  即使随后写入同一个受锁 queue，两个 hart 仍可先后取走相邻字符、再以相反顺序取得 queue lock。
- 处理：用独立锁覆盖从开始轮询到本轮 drain 结束的完整 pump。不能只锁单次 queue mutation，也不要让
  多 hart 分别拥有输入缓存。2026-08-16 双架构扩展软件矩阵与 job-control 回归通过。

## `PT_INTERP` 的高文件偏移不是 ELF metadata 大小

- 适用范围：大型 PIE/动态 ELF、filesystem exec、lazy PT_LOAD、kernel heap
- 现象：有效 ELF 直接执行返回 `ENOEXEC`，shell 随后把二进制当脚本解释并报莫名语法错误；显式执行
  musl loader 加该 ELF 参数却能正常运行。Alpine RV64 Cargo 的 `PT_INTERP` 位于约 19.5 MiB 偏移。
- 根因：旧 loader 为限制 kernel heap，把 metadata 及解释器字符串都要求位于首 1 MiB；这是实现限制，
  不是 ELF ABI 约束。
- 处理：只读固定 header/program-header 表，再从真实文件偏移单独读取有界解释器字符串，并重定位内存中
  program header 副本的 `p_offset`。`PT_LOAD` offset 不能随之改写，因为 lazy fault 仍以它读取原文件。
  2026-08-16 RV64/LA64 Cargo offline workspace 两轮 release 构建与产物运行通过。

## FIFO 的 `O_TRUNC`、open 配对和 EOF 属于三个不同层次

- 适用范围：shell redirection、named FIFO、多 reader/writer、namei 与 pipe runtime
- 现象：`cmd > fifo` 返回 `EINVAL`，或两个 writer 中第一个退出后 reader 提前 EOF/第二个 writer EPIPE；
  单 writer 的 nonblocking LTP 仍可能全部通过。
- 根因：namei 把 `O_TRUNC` 无条件下发给特殊 inode；每个 `NamedFifoEnd` 又复用 anonymous Pipe 的
  drop 行为，把单端点关闭误当成整个方向关闭。blocking open 若只看当前 endpoint count，还会在已配对
  writer 快速退出后重新睡眠，丢失已经完成的 rendezvous。
- 处理：open-time truncate 仅作用 regular inode；pending opener 计入 reader/writer rendezvous，配对完成
  即使对端随后退出本次 open 仍成功；由 named FIFO 聚合计数在最后一个端点关闭时发布 EOF/EPIPE。

## `PIPE_BUF` 原子性必须覆盖一次 writev 的所有 iovec

- 适用范围：pipe/FIFO、并发 producer、stdio/coreutils、writev
- 现象：并行 sha256sum 的两行被拼成一行，同时文件开头出现空行；每次单独 write 加锁看似已经串行。
- 根因：空间不足时小 write 被部分提交；修复单 write 后，sys_writev 又逐 iovec 调用 write，使 libc 分开的
  正文和换行仍可被其他 producer 插入。POSIX 原子边界看一次 syscall 的总长度，而不是单个 iovec。
- 处理：不超过 `PIPE_BUF` 的 write 空间不足时零进度阻塞/EAGAIN；同样大小的 writev 先合并为一个 record
  后提交。nonblocking pipe 不能先用“至少空一整页”的通用 readiness 拒绝小 record，必须让 pipe 按实际
  请求长度裁决。2026-08-16 LA64 连续三轮、RV64 一轮并行 48 文件 sha256 pipeline 通过。

## 文件锁清理不能只挂在显式 close syscall 上

- 适用范围：flock、POSIX record lock、fd-table clear、dup/fork/process exit
- 现象：持锁 shell/子进程已经退出，新进程的 `LOCK_NB` 或 `F_SETLK` 仍永久失败；简单 unlock 测试正常。
- 根因：进程退出直接清空 fd table，不逐项经过 `sys_close()`；同时 flock 属于 open-file-description，按
  当前进程 fd table 判断“最后一个 fd”会在跨 fork 时过早释放。record lock 则属于 PID，必须在 group
  exit 清理，即使没有可枚举的最后 close。
- 处理：flock entry 保存 open-file-description weak owner 并在竞争时剔除死亡项；record lock 在进程组
  退出提交点按 TGID 全表删除。不要把两种锁合并成同一种 owner 模型。

## virtio-net 已 up 不代表用户态具备路由与 DNS

- 适用范围：QEMU user networking、smoltcp 多 interface、Alpine Git/curl/wget
- 现象：宿主通过 hostfwd 能访问内核 HTTP，用户态连接 `10.0.2.2` 却立即拒绝；或同网段访问和 DNS
  成功，但公网 TCP 一直超时；域名也可能直接报 `bad address`。
- 根因：入站 listener 不经过 TCP connect 的 interface context；若 connect 仍固定使用 loopback，真实
  NIC 只能入站。只配置 `10.0.2.15/24` 没有 `0/0 via 10.0.2.2` 时，同网段和 `10.0.2.3` DNS 可成功，
  公网仍无路由。当前两张 Alpine 软件镜像的 `/etc/resolv.conf` 为空，最后一种失败属于镜像配置。
- 处理：分别验证 guest→宿主同网段、UDP DNS、公网 HTTP 和 Git HTTPS；检查默认路由确实安装在
  Ethernet interface，而不是 loopback。launcher 只在 software/final guest 没有既有 nameserver 时安装
  `10.0.2.3` fallback；运行使用 `-snapshot`，不要修改归档镜像或把空 resolv.conf 误报成 UDP 内核故障。
