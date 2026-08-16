# RespOS POSIX 语义专项推进方案

## 文档定位

本方案把 [linux-posix-refactor-plan.md](./linux-posix-refactor-plan.md) 中“跨 Phase 的 POSIX 语义覆盖任务”
细化为可以逐项实现、验证和交付的执行路线。

需要先澄清阶段名称：现有路线的 **Phase 6 是细粒度并发与性能模型**，不是 POSIX 语义阶段。POSIX
语义当前属于 Phase 5 主线及跨 Phase 持续任务；只有相关状态所有权和语义门槛闭合后，才允许进入
Phase 6 的调度器、allocator、异步 I/O 和细粒度锁重构。

本方案采用一条明确路线，不另起与 Phase 5 重复的实现分支：

```text
覆盖矩阵与基线
  -> socket 阻塞/错误语义
  -> task/session/job-control/signal
  -> mmap EOF/truncate/SIGBUS
  -> 文件与时间遗留语义
  -> libc 组合接口
  -> 有证据触发的可选扩展
  -> Phase 6 性能重构
```

### 2026-08-16 近期执行入口调整

现场赛近期任务改由
[software-compatibility-network-plan.md](./software-compatibility-network-plan.md) 统筹：当前主线先运行
Git/Vim/GCC/rustc，并按第一个稳定失败从本文件矩阵选择语义工作；队友负责 virtio-net 与 Git
HTTP(S)/SSH。本文件继续维护已知 ABI 边界、证据和 backlog，但不再意味着必须脱离 workload 顺序逐项
扫完整张 POSIX 清单。简单 HTTP server 不是当前交付目标，网络通过也不得由 loopback 结果代替。

## 基线、目标与非目标

### 计划基线

- 制定日期：2026-08-14；代码基线：`5d9adea`。
- Linux/POSIX Phase 0--4 主体已完成；Phase 5 的 AF_UNIX 基本关闭/poll、signal ABI 首轮、精确
  timeout、`wait4` 的窄化 `SA_RESTART` 和 CPU clock 已有专项证据。
- 制定时 task leader exit/non-leader exec、mmap EOF/truncate/SIGBUS、完整 signal restart/process-pending、
  termios/job control 和 inet socket 等价语义均未闭合；2026-08-15 的当前进度已关闭稳定 identity/de-thread
  核心并完成 process-pending 与 job-control 首轮，具体边界以 `current-status.md` 为准。
- 2026-08-15 当前工作树已闭合 socket timeout、稳定 process identity、leader exit/non-leader exec、
  mmap EOF/truncate/SIGBUS、process-pending/job-control 首轮及主要零进展 restart class；剩余边界以
  `current-status.md` 顶部和各里程碑的下一交付物为准。
- 快速变化的实际结果继续写入 [current-status.md](./current-status.md)，本文件只维护推进顺序、任务边界
  和退出门槛。

### 目标口径

目标是覆盖比赛用户态实际使用的 POSIX.1-2024 源级接口，并使 musl/glibc 最终观察到的返回值、
`errno`、阻塞/唤醒、signal、失败原子性和资源生命周期与目标契约一致。Linux 是当前 ABI 对照平台，
但不要求照搬 Linux 内部数据结构。

覆盖分为三层：

| 层级 | 范围 | 优先级 |
| --- | --- | --- |
| P0 基础语义 | 进程/线程、session、signal、时间、文件、MM、pipe/socket 以及 libc 依赖的底层原语 | 必须先闭合 |
| P1 libc 组合接口 | `pthread_*`、`sem_open()`、`shm_open()`、`aio_*`、`posix_spawn()` | P0 稳定后逐簇验证 |
| P2 可选扩展 | POSIX message queue、`mlockall()`/`munlockall()`、XSI SysV message/semaphore | 由规范目标、LTP 或 workload 触发 |

本方案不宣称通过 POSIX 官方一致性认证，也不以 Linux syscall 数量、LTP 总通过数或一个比赛 workload
替代语义证明。Linux 专有接口可以服务比赛需求，但不计入 POSIX 覆盖率；未实现的状态型 ABI 必须诚实
失败。

## 覆盖矩阵与状态规则

每个条目必须记录以下字段：

| 字段 | 内容 |
| --- | --- |
| 接口簇 | POSIX 源级接口及实际 libc 调用链 |
| 契约 | 成功结果、错误优先级、阻塞/EINTR/restart、partial result、生命周期 |
| 规范选项 | mandatory、option group、XSI 或 Linux extension |
| 内核所有者 | task/signal/MM/VFS/net 等领域对象，不只写 syscall 编号 |
| 当前状态 | 使用下列统一状态值 |
| 证据 | Linux probe、RespOS probe、LTP、双架构与 SMP 日志 |
| 最后验证 | commit、日期、架构、libc、镜像和命令 |

统一状态只使用：`未盘点`、`待验证`、`已知差异`、`实现中`、`RV64 已验证`、`双架构已验证`、
`受限支持`、`诚实不支持`、`可选扩展`。没有测试证据时不得从 `实现中` 直接改为 `双架构已验证`。

当前第一批 backlog 如下；它是任务入口，不是通过声明：

| 接口簇 | 当前证据 | 初始状态 | 下一交付物 |
| --- | --- | --- | --- |
| `SO_RCVTIMEO`/`SO_SNDTIMEO`、`MSG_DONTWAIT` | Linux/RespOS probe 已有；LA64 SMP 50 ms 晚醒约 1 秒 | 阻断 | 归一化 LA64 per-hart 时间域后重跑 |
| nonblocking `connect`、`poll`、`SO_ERROR` | loopback success/refused、error consumption 与同 fd 失败后重连双架构 2 hart 通过；LA64 2 hart iperf 在 UDP→TCP 顺序停滞 | 阻断 | 定位 LA64 SMP iperf；补 unreachable/timeout/reset |
| stream `shutdown(SHUT_WR)`/half-close | Linux/RV64/LA64 probe 覆盖 TCP 排队数据先于 EOF、反向流、dup 后 `EPIPE`，TCP/AF_UNIX buffered peer shutdown 的 RDHUP，以及 AF_UNIX RDHUP-only blocking/edge/oneshot/rearm | 双架构已验证（loopback TCP 与 AF_UNIX stream 当前范围） | 补 TCP 事件式 waiter、并发 close/shutdown、reset/linger 与非 loopback |
| `MSG_PEEK/WAITALL/NOSIGNAL`、partial I/O | Linux/RespOS probe 含 timeout/EOF/signal 短读，双架构 2 hart 通过 | 已闭合（当前范围） | 完整初赛与网络回归 |
| `getsockname/getpeername/accept` 地址写回 | Linux/RespOS probe 覆盖错误优先级、AF_UNIX unnamed/pathname/非 UTF-8 abstract 双向地址与截断；musl/glibc `getpeername01,getsockname01` 双架构 2 hart 通过 | 双架构已验证（stream 当前范围） | 补 inet 关闭/半关闭、datagram/seqpacket disconnected/autobind 及 pathname alias/rename identity |
| AF_UNIX `SO_PEERCRED` | socketpair 与 pathname 双向凭据快照、musl/glibc `getsockopt02` 双架构 2 hart 通过 | 双架构已验证 | 补 credential change；`SCM_CREDENTIALS/SO_PASSCRED` 另立子项 |
| AF_UNIX→pipe `splice` | Linux/RespOS 错误矩阵与 connected transfer probe、musl/glibc `splice01`--`splice07` 双架构 2 hart 通过 | 双架构已验证 | datagram/seqpacket 与 socket 输出方向按需求扩展；不宣称 zero-copy |
| `getsid()` | Linux/RespOS session probe 与 PID 1 基础身份双架构 2 hart 通过 | 双架构已验证（无 controlling tty） | 与完整 job control 一起扩展跨 exec/session 关系 |
| termios/job control | Linux/RV64/LA64 probe 已覆盖 controlling tty/前台组、stop/continue wait event、后台读 EIO、TOSTOP ignored-write 与 detach | 双架构已验证（首轮状态机） | canonical/echo/control chars、hangup I/O、orphan transition、forced steal 与 PTY |
| leader `exit`、non-leader `exec` | 八项 `task_phase5_probe` 在 RV64/LA64 2/8 hart 通过，含稳定 identity、失败原子性与并发 exec | 双架构已验证（M2.1 核心） | exec-vs-exit_group 扩大竞态与资源基线 |
| `CLONE_VFORK|CLONE_VM` | `clone05` 双架构双 libc 通过；额外 vfork01/02 与 RV64 CAgent/minibuild 回归通过 | 双架构已验证（当前范围） | 完整 BuildStorm、posix_spawn file-actions/失败回报 |
| process-pending、`SA_NOCLDWAIT`、通用 restart | process-pending、标准/实时队列、进程内 `RLIMIT_SIGPENDING`、SIGCHLD auto-reap/NOCLDSTOP 已通过三方专项；restart 覆盖 wait4、pipe/vector/socket/mmsg（含 recvmmsg timeout）、null-timeout futex，poll/sleep timeout 类已验证非重启 | 受限支持 | real-UID 全局 pending 配额、time64/compat 与未盘点 timeout 类 |
| futex absolute timeout / precise wake | RV64 bitset/wake 与 LA64 wake 双 libc 通过；LA64 首组 musl monotonic wait 在 secondary 上线窗口稳定延迟约 0.87 s，后续 realtime/glibc 正常 | 已知差异 | 待确认：审计 LA64 secondary 启动与跨 hart 时间/调度，不放宽 timeout 阈值 |
| mmap EOF/truncate/punch/ENOSPC SIGBUS | Linux/RV64/LA64 probe 的 EOF/truncate shared/private/private-COW、跨进程失效及 16 轮 fault/store 竞态通过；ext4 punch、真实块释放、cross-process provenance、16 轮 punch-vs-store/msync 通过；16 MiB auxiliary ext4 满盘 shared store SIGBUS 与释放空间后恢复双架构通过；sub-page filesystem block grow/refault、`/dev/zero` 零填充映射与 `MAP_GROWSDOWN` 已双架构双 libc 通过；26 项扩展簇为 25/1，唯一 `mremap06` 由缺失 fallocate 预分配导致 TBROK | 双架构已验证（M3 当前范围） | ext4 default/`KEEP_SIZE` unwritten extent、page-mkwrite 性能与更宽 mmap LTP |
| `mmap/mprotect(PROT_NONE)` | LA64 software-present/hardware-invalid PTE；`mmap05` 与临时过滤的 `mprotect04` 双 libc 通过，RV64 回归通过；独立 Linux/RV64/LA64 probe 已覆盖未知 prot/未对齐 `EINVAL` 权限不变、只读 shared backing `EACCES` 不获写权限及 unmapped-hole `ENOMEM`，并校正 POSIX 允许非 `EINVAL` 失败部分修改 | 双架构已验证（当前 PROT_NONE 与参数/权限/映射缺口失败范围） | 补内存压力/VMA 上限失败与并发 user-copy；旧 LA 模拟器上的 NR/NX 其他组合单列验证 |
| `posix_fadvise` / PageCache advice | 六种 advice、错误矩阵、open-description 状态、1/16/32 页窗口、WILLNEED、完整页/EOF DONTNEED、dirty writeback 与 NOREUSE reset 已由 Linux oracle 和 RV64/LA64 8 轮专项验证；双 libc 八项 LTP、mmap 与 rusage 相邻回归通过 | 双架构已验证（当前同步 writeback 范围） | async writeback/error 时序、极大范围节流与完整 BuildStorm soak |
| realtime/纳秒/atime、CPU/rusage | CPU process/thread user/system、`RUSAGE_THREAD`、fault/RSS/actual context-switch、block I/O、cold-file major fault、全部 Linux-zero legacy 字段及 wait4/raw-waitid children 聚合已通过专项；RTC init/read/set 与同设备 reset persistence、ext4 扩展及 128-byte 旧 inode 已通过双架构；regular/directory default/24h relatime、strictatime、mount/open noatime、nodiratime、lazytime 显式同步、background/eviction、无显式 sync crash-image 与 ctime 关系双架构通过；timestamp LTP 双 libc双架构通过 | 双架构已验证（CPU accounting 与 rusage 当前字段、RTC 当前 QEMU 范围、ext4 timestamp layout 与 atime 当前范围）；其余待验证 | CPU hotplug accounting 与更宽 LTP；真实断电/volatile device cache、lazytime I/O failure/registry soak、真实电池 RTC 掉电由硬件平台另验 |
| musl `pathconf()` pathname 错误 | 当前 RV64/LA64 镜像的 musl 反汇编证实 `pathconf` 丢弃 path；musl `pathconf02` 五项失败而 glibc 全通过 | 已知差异 | 待确认：可复现 musl 构建/镜像替换与完整 musl 回归 |
| LA64 musl `readlink*()` 零长度 | musl 1.2.5 wrapper 把 size 0 转成内部 size 1 调用；内核已对真实 size 0 返回 `EINVAL`，RV64 musl 1.2.0 与两架构 glibc 通过 | 已知差异 | 待确认：是否修改 musl runtime；不在内核特判 size 1 |
| RV64 musl `epoll_create()` invalid size | musl 1.2.0 丢弃 size 后调用合法 `epoll_create1(0)`；LA64 musl 1.2.5 与两架构 glibc 的 `epoll_create02` 通过 | 已知差异 | 待确认：与其他 musl 差异统一更新 runtime；不得拒绝合法 `epoll_create1(0)` |
| musl `recvmmsg()` bad message vector | 两架构 musl 在 syscall 前规范化 mmsghdr 两个 size 字段并对 guard vector SIGSEGV；两架构 glibc 两种 time ABI 的 10 项错误矩阵通过 | 已知差异 | 待确认：纳入 musl runtime 处理；内核不拦截 syscall 前 store |
| `pwrite()` + `O_APPEND` | Linux 32 轮整 syscall 原子性 oracle 与双架构 musl/glibc 16-case 小写簇通过；128 KiB 并发 guest 在默认 RV64 自然交错 15/16，强制 chunk 让出时双架构交错 16/16 | 已知差异（选位正确、整 syscall 非原子） | 待协商：可睡眠 syscall 级序列化或可回滚 append-range reservation；覆盖不同 open description、EFAULT/short-write/truncate |
| 已删除目录 fd 的 `getdents64()` | Linux probe 覆盖未读/已缓存目录流；双架构 musl/glibc `getdents01/02` 通过 | 双架构已验证 | 自定义内存目录若支持 unlink，下沉通用 detached 状态 |
| `chroot()` pathname/permission/privilege 错误优先级 | Linux probe 固定 `EACCES/ENOENT` 先于 `EPERM`；双架构 musl/glibc `chroot01`--`chroot04` 通过 | 双架构已验证（当前权限模型） | capability/user/mount namespace 按需求另立状态模型 |
| ext4 特殊 inode、`mknod` device payload 与 xattr 限制 | 双架构 probe 验证四类 inode mode、12-bit major/20-bit minor 的 stat/statx 回报与 xattr 限制；musl/glibc 13-case mknod/xattr 及 4-case statx 簇通过 | 双架构已验证（当前范围） | 设备驱动 open/read/write 语义按需求另立子项；不扩展 kernel 32-bit device encoding |
| ext4 `fallocate()` default/`KEEP_SIZE` | Linux 物理预留 probe 通过；双架构 musl/glibc `fallocate03` 八项均返回 `EOPNOTSUPP` | 已知差异 | 待确认：为 lwext4 unwritten extent 增加事务化预分配入口；禁止稀疏扩容伪装 |
| SysV SHM `shmat/shmdt` | 跨 attach 数据/futex、`IPC_RMID` lifecycle、`shm_nattch` MM identity，以及 `shmat` 与最后 detach/RMID 的发布竞态和失败回滚 probe 均在 Linux/RV64/LA64 通过；竞态门禁覆盖 32 轮双 attacher（64 次 attach）和 128 轮顺序回收循环；`SHMMIN/SHMMAX`、existing-key size/flag errno、默认 `SHMMNI=4096`、clean-table `SHMALL=2`、已有对象时动态下调 `SHMALL/SHMMNI`、固定配额双创建者线性化、核心 metadata、flat credential 非 owner 权限，以及当前无-swap模型的 `SHM_LOCKED` flag/ownership 已通过；非空地址已按精确映射处理；`shmctl01` metadata 断言正确，但 LA64 卡在 20-child signal/reap teardown；LA64 glibc 2.38 仍有旧 64 KiB SHMLBA 冲突 | 双架构已验证（当前共享/生命周期/计数/双 attacher 线性化、顺序回收、size、SHMMNI、SHMALL 静态/动态配额、固定配额并发创建、核心 metadata、基础权限与 lock 控制面范围）；LA teardown 与 runtime 已知差异 | 待协商：LA 多子进程 signal/exit/wait 活性；另补更宽 N 路并发、`SHM_REMAP` 并发、并发 sysctl/create、IPC namespace、物理内存/ID 溢出、namespace capability、lock 的 `RLIMIT_MEMLOCK`/真实 pinning 与绝对 timestamp；runtime 更新 |
| `pthread_*`/named sem/shm/AIO/`posix_spawn` | 尚无完整 libc 组合矩阵 | 待验证 | musl/glibc 同源 probe 簇 |
| message queue、`mlockall`、其余 XSI IPC | 需求尚无证据 | 可选扩展 | 需求触发记录；默认不实现 |

## 单项闭环方法

每个接口簇严格按以下顺序推进：

1. **写契约卡片**：列出正常路径、错误优先级、零长度/空指针/边界值、阻塞与 nonblock、signal
   interruption、partial result、dup/fork/exec/close 后的状态归属。
2. **建立 Linux baseline**：优先使用一份能在宿主 Linux 和 musl/glibc guest 编译的 C probe；无法同源
   时，两份 probe 必须使用同一输入向量和通过标志。宿主以 `-Wall -Wextra -Werror` 编译。
3. **固定 RespOS 差异**：未修复版打印精确的 `*_EXPECTED_FAIL`，退出非零；不得把 expected failure
   当成通过，也不得先改实现再凭印象补测试。
4. **确定状态所有者**：写清领域对象、锁序、waiter 注册点、single-winner、提交点、失败点和回收点。
   syscall 层只做 ABI 解析、用户拷贝和 errno 映射。
5. **一次只修一个语义主题**：先修模型及失败原子性，再删除旧兼容分支；不把 scheduler 或 allocator
   性能重构混入 ABI 修复。
6. **分层验证**：先 probe，再聚焦 LTP，再做 SMP/资源闭环和双架构；高风险里程碑才跑完整初赛或
   BuildStorm。
7. **记录证据**：结果写入 `current-status.md`；稳定不变量写入 `architecture.md`；被接受的取舍写入
   `decisions.md`；确认的失败模式写入 `pitfalls.md`。

单项只有同时满足“契约明确、Linux baseline 通过、RespOS 不再出现对应 expected-fail、相关资源可
回收、双架构顺序构建通过”才能关闭。涉及 trap context、signal、task、MM、futex 或 socket 阻塞时，
还必须有至少 2 hart 的 lost-wakeup/并发专项。

## 里程碑 M0：建立可维护的覆盖基线

### 工作内容

1. 从 POSIX 接口簇而非 syscall 表出发建立矩阵；为 libc 组合接口记录其实际底层 syscall。
2. 将当前 695 项 LTP 失败集合按 FS/MM/task/signal/time/IPC/net/libc 分类，首个框架错误与后续 TBROK
   分开统计；被注释用例继续明确记录，不能从分母静默消失。
3. 复用现有 `fs_*`、`mmap_phase5`、`signal_phase5`、`task_phase5`、`socket_phase5` probe，缺少的主题
   才新增 probe。
4. 固定每轮证据元数据：commit、dirty patch 摘要、镜像 hash、QEMU、架构、hart、libc、命令和日期。

### 退出门槛

- P0 每个接口簇都有所有者和状态，不存在只写“syscall 已实现”的条目；
- 已知差异均能由 probe 或当前源码直接举证；
- 历史结果、当前结果与 `待验证` 不混写。

## 里程碑 M1：socket 阻塞与错误语义

该里程碑先完成，因为当前工作区已开始 socket timeout，且它与架构线耦合较低。

### M1.1 timeout 与 per-call nonblock

采用以下唯一模型：timeout 属于共享 socket/open-file description；每次阻塞 syscall 进入时生成一次
monotonic deadline；waiter 先登记并发布 Blocked，再由数据、空间、close、signal 或 deadline 竞争
single-winner 唤醒。`MSG_DONTWAIT` 只影响本次调用，不修改 `O_NONBLOCK`。

probe 至少覆盖：

- `SO_RCVTIMEO/SO_SNDTIMEO` 的 `struct timeval` ABI、非法 `tv_usec`、短 `optlen` 与 get/set round-trip；
- 零 timeout 表示无限等待，而不是立即 `EAGAIN`；
- recv 空队列和 send 满缓冲超时返回 `EAGAIN/EWOULDBLOCK`，且不会明显早醒；
- partial send/recv 优先返回字节数，不在已有进展后丢成 timeout/EINTR；
- `MSG_DONTWAIT` 立即返回但后续普通调用仍按 socket timeout 阻塞；
- peer close、shutdown、dup 后另一线程阻塞以及 signal 到达与 timeout 同时发生。

### M1.2 connect、pending error 与 readiness

为每个 inet socket 保存显式 `Idle/Connecting/Connected/Failed/Closed` 状态和 pending error。非阻塞
`connect` 首次返回 `EINPROGRESS`；完成后由 poll 写就绪/错误就绪唤醒。`SO_ERROR` 返回并消费 pending
error，不能继续固定返回 0。refused、reset、timeout、unreachable 和成功分别建立 Linux 对照。

### M1.3 message flags 与 poll/epoll

逐个实现或诚实拒绝 `MSG_PEEK`、`MSG_WAITALL`、`MSG_NOSIGNAL`；未知 flag 返回契约规定的错误，不能
静默忽略。统一校验 blocking/nonblock、EOF、half-close、`EPIPE/SIGPIPE`、`POLLIN/POLLOUT/HUP/ERR`
和 epoll 观察到的结果。AF_UNIX 与 inet 可以有不同内部机制，但对相同契约不得出现两个 errno 模型。
socket 地址查询也沿用同一方法：先用 Linux 对照固定 fd、用户输出参数和各连接态组合的错误优先级，
再提交成功写回；例如未连接 inet 的 `ENOTCONN` 先于非法长度，而 connected socketpair 才进入
`EINVAL/EFAULT` 输出校验，不能用一条全局优先级替代状态矩阵。
数据报 shutdown 也必须独立取 Linux oracle：当前已固定 connected UDP 的 `SHUT_WR`
`EPIPE`、`SHUT_RD` 空队列 EOF 以及已排队/未来数据报仍可读；不得从 stream 半关闭直接外推。

### M1 退出门槛

- socket timeout/connect/flags 三组 Linux 与 RespOS probe 全通过；
- RV64/LA64 顺序构建，至少 2 hart 的 timeout/close/signal 竞争通过；
- `socket_phase5_probe`、`unix_socket_block_probe`、`net_timer_progress_probe` 回归通过；
- iperf BASIC/PARALLEL/REVERSE UDP/TCP 以及“daemon 后 iozone”固定顺序不回退。

## 里程碑 M2：task、session、job control 与 signal

### M2.1 先修线程组生命周期

先闭合 leader 原始 `exit`、最后线程发布 zombie、non-leader `exec`/de-thread/TID 接管，再扩展 job
control。进程身份必须从“某个永远存活的 leader TCB”中解耦为稳定的 process/thread-group state；
父子关系、wait status、pgid/sid、process-directed signal 和共享资源回收都引用该身份。非 leader exec
只保留调用线程的 thread-directed pending，按既有协议清理 sibling 的 robust futex 和
`clear_child_tid`，最后原子安装新映像。

已采用的唯一所有权模型、de-thread 提交顺序、锁序与验证拆分见
[process-identity-phase5-design.md](./process-identity-phase5-design.md)。稳定身份、leader exit、最后线程
Zombie 与 non-leader exec 核心路径已实现；不得以保留退出 leader TCB 的 tombstone 作为实现。

现有 `task_phase5_probe` 已从修复前 expected failure 变为八项 `TASK_PHASE5 ALL PASS`，RV64/LA64
2/8 hart 均通过。M2.1 剩余门槛是 exec-vs-exit_group 扩大竞态与 task/frame/fd/futex/timer 资源基线；
process-pending 和 job control 属于 M2.2/M2.3，不由该结果替代。

### M2.2 session 与终端状态

在稳定 process identity 上补 `getsid()`，随后把 controlling terminal 状态放入 tty/terminal 对象，
不继续在 `sys_ioctl` 中堆硬编码。最小状态包括 session owner、foreground pgrp、termios attributes 和
hangup；`setsid/setpgid/getsid/tcgetsid/tcgetpgrp/tcsetpgrp` 与 `TCGETS/TCSETS*` 共享该状态。

probe 覆盖 session leader 限制、父进程对子进程 `setpgid` 的 exec 边界、前后台组切换、孤儿组、
`SIGTTOU/SIGTTIN/SIGHUP/SIGCONT` 和 controlling tty 释放。没有完整 tty 语义前，不以固定成功或固定
窗口值宣称 job control 支持。

2026-08-15 已把状态放入共享 console terminal，并以 Linux PTY oracle 与双架构 2 hart probe闭合
controlling tty/foreground pgrp、默认 `SIGTTIN` stop/`SIGCONT` continue 的 wait 事件、ignored/blocked
后台读、`TOSTOP` 写和 detach；随后补齐停止进程组因 parent exit/reparent 形成孤儿时的
`SIGHUP`→`SIGCONT`，双架构各连续 8 轮通过。PTY、line discipline、hangup I/O、并发关系变更线性化与
forced steal 仍是退出门槛，因此 M2.2 保持进行中。

### M2.3 process-pending 与 restart class

process-directed pending queue 已与 per-thread pending queue 分离，递送时才按 mask/目标线程选择；
exec、fork、线程退出和 group exit 的保留/清理规则已有专项覆盖。标准信号合并保留首条 info、实时信号
同号多实例 FIFO 及进程内 `RLIMIT_SIGPENDING` 填满/恢复已通过 Linux oracle、双架构 probe 和
`tgkill02`；Linux 的 real-UID 跨进程统一计数仍待凭据 owner 模型。`SA_NOCLDWAIT` 与显式 SIGCHLD
ignore 的自动回收及 `SA_NOCLDSTOP` 已通过三方专项；下一步优先推进 restart classes，并继续补剩余
job-control signals。

syscall restart 不做“所有 EINTR 自动重启”。按 Linux restart class 分批扩展：

1. 无 partial side effect 的 wait 类；
2. read/write/pipe/socket，已有字节数优先；
3. 带 timeout 的 poll/futex/sleep/socket，重启时保留剩余时间或按接口契约返回；
4. 明确不可重启的调用保持 `EINTR`。

2026-08-15 已将零进展阻塞 `read/write/readv/writev(pipe)` 与 `accept/accept4(AF_UNIX)` 纳入显式
restart-class 表；随后以满/空 AF_UNIX socketpair 闭合
`sendto/recvfrom/sendmsg/recvmsg/sendmmsg/recvmmsg(null timeout)`。普通 handler、`SA_RESTART`、默认忽略
三种路径均通过 Linux/RV64/LA64 对照，向量 I/O、`MSG_WAITALL` 和 mmsg 还验证 partial-result/count
优先级。参数感知分类器随后纳入 null-timeout `FUTEX_WAIT/FUTEX_WAIT_BITSET`；普通 handler、
`SA_RESTART`、默认忽略三态及双架构各 8 轮通过，带 timeout 的 futex 仍保持 `EINTR/ETIMEDOUT` 并以
双架构各 20 轮 signal/wake/timeout 竞态门禁证明。无 `SO_SNDTIMEO` 的阻塞 connect 也已纳入；AF_UNIX
满 accept queue 的普通 handler/`SA_RESTART`/默认忽略三态在 Linux/RV64/LA64 通过；同一 probe 还验证
accept/recvfrom/sendto/connect 设置方向对应 socket timeout 后，即使 `SA_RESTART` 也返回 `EINTR`。
分类器已把方向 timeout 检查推广到 socket fd 上的 read/write/readv/writev，双架构各连续 8 轮。
`nanosleep/ppoll/pselect6/epoll_pwait/clock_nanosleep` 的 timeout 非重启契约也已三方闭合：普通与
`SA_RESTART` handler 均返回 `EINTR`，默认 ignored signal 等到 timeout；relative nanosleep/clock_nanosleep
校验 remaining time，absolute clock_nanosleep 保持 remainder 不变。`recvmmsg` 非空 timeout 也已闭合，
但契约不同：timeout 只在成功 message 后检查/写回，不独立唤醒；零进展 EINTR 不改原值并允许
SA_RESTART，partial count 保留最后一次成功写回，`MSG_WAITFORONE` 首条后转 nonblock。后续 time64/compat
和其他 timeout 类仍须分别建立证据。
同轮暴露并修复 wait 的 scan→register lost-wakeup：父进程级 child-event generation 使登记后的代际变化
强制重扫；登记复查不再因无关旧 zombie 忙循环，双架构完整 signal probe 各连续 8 轮通过。
signal enqueue 的 `interrupted` 仅作 wake hint，发布后现重新验证 pending/interruptible 条件，防止
consumer 已消费 signal 后的晚写让下一阻塞 syscall 假 `EINTR`。
stop/continue 同样按发布顺序闭合：先写 `Stopped`，再通知 parent，最后 handoff 不覆盖并发 SIGCONT 的
`Ready`；双架构 signal 8 轮和 job-control 专项通过。

每一类都必须覆盖无 handler、无 `SA_RESTART`、有 `SA_RESTART`、默认忽略和 signal/完成竞态。

### M2 退出门槛

- `getsid`、session/pgrp、tty foreground、task lifecycle、process-pending 和 restart 分簇 probe 通过；
- 不再依赖 leader TCB 存活维持进程身份；task/frame/fd/futex 资源压力后回到基线；
- RV64/LA64 signal frame、单核和 SMP 专项均通过。

## 里程碑 M3：mmap EOF、truncate 与 SIGBUS

2026-08-15 已闭合核心七项：普通 mmap 使用 live EOF、truncate 跨唯一 MemorySet 失效 resident whole
page、clean private file page 以 PageCache+COW 保存 provenance，partial tail 在 clean/dirty private
情况下分别清零/保留。随后补齐显式跨进程竞态：独立 truncator 对已驻留和未 fault mapper 的访问均
收敛为 SIGBUS，另以 16 轮同时 fault/store 与 shrink 验证 truncate-done 后不残留可访问 PTE。
Linux/RV64/LA64 均输出 `MMAP_PHASE5 ALL PASS`；双架构双 libc `mmap05`、双架构 BuildStorm file 与
RV64 private-map 相邻回归通过。2026-08-16 又闭合 ext4 punch-hole：partial boundary 清零、完整物理块
释放、`st_blocks` 真实下降、eviction 后 lower 零读，以及跨进程 shared/clean-private 失效和 private-COW
保留均由 Linux/RV64/LA64 对照通过；错误矩阵与 mmap/BuildStorm/fadvise 相邻门禁也已通过。剩余边界是
shared write 存储耗尽也已由 write-protected PTE + fault-time backing 协议关闭，并在双架构 16 MiB ext4
验证 SIGBUS 和释放空间后恢复；16 轮 punch-vs-store/msync 三方门禁也已通过。剩余边界是 page-mkwrite
性能和更宽 mmap LTP。`mmap16` 的旧 TBROK 已确认是 synthetic mount 丢失 mkfs capacity/block size，
并非 buffered writeback 吞错；补齐 geometry 与 `pagecache_isize_extended()` 式 grow 写保护后，双架构
musl/glibc 各 10/10 TPASS。扩展到 26 项后，`/dev/zero` 的 `mmap10` 与带 SP/256-page
guard-gap 校验的 `MAP_GROWSDOWN` `mmap18` 已闭合；双架构双 libc 均为 25/1，唯一
`mremap06` 在前置 `fallocate` 阶段 TBROK。下一个可实现边界因此是 default/`KEEP_SIZE`
unwritten extent，不是修改 mremap 以绕过 fixture。

采用当前审计已经收敛的方案：

1. file-backed VMA 保存稳定 `(dev, ino)` identity、文件偏移和每个 resident page 的来源
   `FileShared/FilePrivateUncopied/PrivateCOW`；
2. fault 时读取当前 inode size，不使用 mmap-time EOF；完整越界页返回可区分的 bus fault，trap 层投递
   `SIGBUS`；EOF 所在部分页按契约补零；
3. truncate 成功并释放 File/PageCache 锁后，以 inode identity 去重扫描 live `MemorySet`；第一版接受
   `O(live tasks x VMAs)`，不提前增加 inode-to-VMA 反向索引；
4. shrink 撤销完整越界 PTE/resident frame，并通过现有 shootdown/residency completion 后回收；部分页
   对 shared 和未 COW private 清尾，已 COW private 按 Linux probe 的匿名页规则处理；
5. grow 后未 fault 页按当前 EOF 重新从文件读取；若 block size 小于 VM page 且 grow 暴露同页新块，
   所有 resident shared PTE 降权，使下一次 store 重新执行 page-mkwrite allocation；旧 truncate 失效不得
   被 writeback 反向扩容；
6. user copy 遇到同类 file fault 时返回正确的短拷贝/errno，不在内核中直接触发用户 signal handler。

当前 EOF/truncate/punch/ENOSPC、sub-page-block grow 及 punch-msync 并发门槛已满足。M3 后续是补
page-mkwrite 成功路径性能/回收证据和更宽双架构 mmap LTP；
DAX/huge page 按当前范围明确排除。

## 里程碑 M4：文件与时间遗留语义

该里程碑不重做 Phase 1--4，而只关闭已有文档明确保留的边界：

1. ext4 纳秒、负时间、2038 后 epoch 和新 inode realtime 已由 Linux、双架构即时及跨重启 probe
   关闭；`CLOCK_REALTIME` 已从 RV64 goldfish/LA64 LS7A RTC 初始化；Linux 式上下界 clamp 已跨重启
   验证；128-byte 无 extra inode 的 signed-32-bit 秒级 clamp 也已双架构跨重启关闭。RTC init/read/set
   与同设备 reset persistence 已双架构关闭；新 QEMU 进程不模拟电池后备 RTC，真实硬件掉电由目标平台
   另验；
2. `UTIME_NOW/UTIME_OMIT` 的当前运行时状态机及 flat credential 下非 owner 权限矩阵已由
   Linux 契约和 RV64/LA64 probe 关闭；继续补 `CAP_FOWNER`/ACL，以及
   regular/directory default/24-hour relatime、strictatime/noatime/`O_NOATIME`/`MS_NODIRATIME` 的事件和
   ctime 关系，以及 lazytime 的立即可见性、fsync/sync/remount durability boundary、monotonic
   background expiry、最后 owner eviction 与无显式 sync crash-image 已由双架构 probe 关闭；继续补真实
   断电/volatile device cache、lower I/O failure injection 与超大 registry soak；
3. `times/getrusage` 的 process/thread user/system、`RUSAGE_THREAD`、fault/RSS/actual context-switch、
   block I/O、可重复 cold-file major fault、全部 Linux-zero legacy 字段与 wait4/raw-waitid 已回收子进程
   累计已通过 Linux/RV64/LA64 专项；full `posix_fadvise` advice、writeback-before-invalidate 和 PageCache
   行为也已通过 Linux oracle、双架构专项/LTP/mmap/rusage 回归。继续补 async writeback 时序、CPU hotplug
   accounting 与更宽 LTP，不把当前字段闭合外推为完整 rusage；
4. namespace 并发可见性、POSIX record lock 的 fork/dup/close 生命周期和 FIFO 多开边界；
5. 当前 LTP 失败矩阵中有规范证据的文件接口，其 errno、失败原子性和跨重启状态。

时间修改必须有跨重启 probe；namespace/record-lock 修改必须有至少 2 hart 竞态；文件持久化继续遵守
PageCache 唯一页身份和 writeback error cursor，不为 POSIX 用例恢复 close 全盘同步。

## 里程碑 M5：验证 libc 组合接口

此阶段原则是“先测 libc 路径，再决定是否增加内核入口”。按以下顺序分簇：

1. `pthread_create/join/detach`、mutex/cond/rwlock/once、TLS destructor、robust/pshared；
2. `posix_spawn` 的 file actions、attributes、signal mask/default、pgroup、失败回报和 vfork 父唤醒；
3. `shm_open/shm_unlink` 和 `sem_open/sem_unlink` 的 name、mode、lifetime、跨进程共享；
4. `aio_read/aio_write/aio_error/aio_return/aio_cancel/lio_listio` 的 libc thread 实现或内核依赖；
5. cancellation points 只在目标 libc 确实要求的底层路径上实现，不把所有阻塞 syscall 都改成异步取消。

每簇同时构建 musl/glibc probe，覆盖成功、失败、fork/exec、进程异常退出和循环资源回收。若 libc 已用
futex、clone、普通文件和 `/dev/shm` 完成组合接口，则修底层契约，不新增同名 syscall。

## 里程碑 M6：按证据启用可选扩展

POSIX message queue、`mlockall/munlockall`、XSI SysV message/semaphore 默认保持 `可选扩展`。只有以下
任一证据出现才进入实现队列：

- POSIX 目标 profile 明确要求该 option group；
- 当前 LTP/比赛 workload 因该能力发生真实阻断；
- 上层 libc 组合接口无法用已有原语实现。

启用后仍按单项闭环执行；不得用空操作返回 0。内存锁定必须真实影响回收/fault，message queue 和
SysV IPC 必须有 namespace、权限、阻塞、删除后存活引用和资源上限模型。

## 代码边界与并行冲突

| 主题 | 主要所有者 | 共享集成面 |
| --- | --- | --- |
| socket | `os/src/net/**`、socket `FileOp` | `syscall/net.rs`、poll/epoll、timer waiter、signal |
| task/session | `os/src/task/**` | process syscall、scheduler、parent/children、resource teardown |
| signal | `os/src/signal/**` | 双架构 trap frame、task identity、timer、阻塞 syscall |
| mmap | `os/src/mm/MemorySet`、VMA | PageCache/inode identity、truncate、arch shootdown |
| file/time | VFS/inode/PageCache、time state | ext4 transaction、RTC/arch clock、task accounting |
| libc 组合 | musl/glibc probe 与底层领域对象 | clone/futex/fs/shm/signal；不默认新增 syscall |

同一时段对 `MemorySet`、scheduler/processor/task、trap context、PageCache/inode identity 或 socket waiter
只允许一个主题作为写入者。跨主题修改先写接口和验证责任，再合入；性能线不得在语义 probe 未通过时
替换相同状态机。

## 验证门禁

### 每个补丁的快速门禁

```bash
cargo fmt --manifest-path os/Cargo.toml -- --check
cargo fmt --manifest-path user/Cargo.toml -- --check
git diff --check
make build-rv RV_USER_FEATURES= RV_KERNEL_FEATURES=
make build-la LA_USER_FEATURES= LA_KERNEL_FEATURES=
```

RV64/LA64 构建必须顺序执行，避免共享 Cargo 配置和 lwext4 CMake 目录竞争。

### 聚焦 LTP

```bash
LTP_CASE_FILTER=case1,case2 make rv
LTP_CASE_FILTER=case1,case2 make la
```

运行后使用 `judge/ltp_report.py` 和 `judge/ltp_compare.py` 对照 Linux baseline；必须查看首个真实失败和
summary，不能以 QEMU/make 返回 0 代替测试通过。

### 风险加测

| 修改类型 | 必加门禁 |
| --- | --- |
| waiter/timeout/socket/futex | 2 hart lost-wakeup、signal-vs-timeout、close-vs-I/O、资源循环 |
| task/signal/trap | RV64/LA64 signal frame、non-leader exec、group exit/wait、8 hart 压力 |
| MM/truncate | `mmap_phase5_probe`、shared-MM、file/private-map、frame reclaim |
| 文件持久化 | metadata/writeback probe、reopen，必要时 qcow2 overlay 跨重启 |
| 跨子系统里程碑 | 完整初赛；网络另跑 iperf daemon -> iozone 固定顺序 |

课程平台不可用期间，平台镜像/成绩统一写 `待验证`。平台恢复后先复评当前 HEAD，再更新正式状态；本地
专项通过不能外推为 POSIX conformance。

## 提交拆分与近期执行顺序

近期按以下补丁序列推进，后一项不得顺手混入前一项：

1. socket timeval timeout 与 `MSG_DONTWAIT` 已完成 Linux/RespOS probe；LA64 SMP 时间域仍阻断验收；
2. `MSG_PEEK/WAITALL/NOSIGNAL` 与 partial-I/O/EINTR 当前范围已完成双架构专项；
3. nonblocking connect、poll readiness、可消费 `SO_ERROR` 以及同 fd
   `ECONNABORTED -> EINPROGRESS -> success` 重连序列已完成；真实网络 timeout/unreachable/reset 与
   iperf 回归仍待补；
4. `getsid` 与 session/pgid Linux 对照已完成，不在该补丁中实现完整 tty；
5. 稳定 process identity、leader exit/non-leader exec 核心路径已完成，继续清理兼容双写和剩余 owner；
6. 在稳定身份上推进 process-pending、controlling tty/job control 和 restart classes；
7. 按 M3 的 resident provenance 方案关闭 mmap 七项差异；
8. 根据覆盖矩阵和 LTP 证据推进 M4、M5；没有触发证据时不进入 M6。

每个补丁说明必须包含：契约、修复前 probe、状态所有者/提交点、修复后结果、双架构状态和仍保留的
边界。这样 POSIX 工作可以持续推进，又不会提前引入 Phase 6 的大规模性能重构。
