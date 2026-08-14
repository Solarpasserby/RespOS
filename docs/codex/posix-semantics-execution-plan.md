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

## 基线、目标与非目标

### 计划基线

- 制定日期：2026-08-14；代码基线：`5d9adea`。
- Linux/POSIX Phase 0--4 主体已完成；Phase 5 的 AF_UNIX 基本关闭/poll、signal ABI 首轮、精确
  timeout、`wait4` 的窄化 `SA_RESTART` 和 CPU clock 已有专项证据。
- task leader exit/non-leader exec、mmap EOF/truncate/SIGBUS、完整 signal restart/process-pending、
  termios/job control 和 inet socket 等价语义仍未闭合。
- 当前工作区的 `os/src/net/socket.rs` 与 `scripts/socket_timeout_probe_linux.c` 正在推进 socket timeout；
  在 Linux baseline、RespOS probe、双架构构建和专项运行完成前，本方案统一将其视为“实现中”，不记为
  已支持。
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
| `MSG_PEEK/WAITALL/NOSIGNAL`、partial I/O | Linux/RespOS probe 含 timeout/EOF/signal 短读，双架构 2 hart 通过 | 已闭合（当前范围） | 完整初赛与网络回归 |
| `getpeername()` 错误优先级与地址写回 | Linux/RespOS probe 与 musl/glibc `getpeername01` 双架构 2 hart 通过；未命名 socketpair 已回报 | 双架构已验证 | 补 named/abstract AF_UNIX peer、截断长度与关闭态路径 |
| AF_UNIX `SO_PEERCRED` | socketpair 与 pathname 双向凭据快照、musl/glibc `getsockopt02` 双架构 2 hart 通过 | 双架构已验证 | 补 credential change；`SCM_CREDENTIALS/SO_PASSCRED` 另立子项 |
| AF_UNIX→pipe `splice` | Linux/RespOS 错误矩阵与 connected transfer probe、musl/glibc `splice01`--`splice07` 双架构 2 hart 通过 | 双架构已验证 | datagram/seqpacket 与 socket 输出方向按需求扩展；不宣称 zero-copy |
| `getsid()` | syscall dispatch 缺项，已有 `setsid/getpgid/setpgid` | 已知差异 | session probe 与最小实现 |
| termios/job control | 当前 tty ioctl 主要只有窗口查询，源码明确未建模 controlling tty | 已知差异 | tty/session/pgrp 状态设计和 probe |
| leader `exit`、non-leader `exec` | `task_phase5_probe` 有三项 expected failure | 已知差异 | leader identity 与 de-thread 实现 |
| process-pending、`SA_NOCLDWAIT`、通用 restart | signal 首轮只闭合查询/exec；restart 仅覆盖 `wait4` | 已知差异 | 分主题 signal probe，不做全局一刀切 restart |
| mmap EOF/truncate/SIGBUS | `mmap_phase5_probe` 有七项 expected failure | 已知差异 | resident provenance、truncate invalidation、fault 分类 |
| realtime/纳秒/atime、user/system CPU time | Phase 1 与 CPU clock 文档保留明确边界 | 待验证 | 跨重启时间 probe 与 clock/accounting 子项 |
| musl `pathconf()` pathname 错误 | 当前 RV64/LA64 镜像的 musl 反汇编证实 `pathconf` 丢弃 path；musl `pathconf02` 五项失败而 glibc 全通过 | 已知差异 | 待确认：可复现 musl 构建/镜像替换与完整 musl 回归 |
| LA64 musl `readlink*()` 零长度 | musl 1.2.5 wrapper 把 size 0 转成内部 size 1 调用；内核已对真实 size 0 返回 `EINVAL`，RV64 musl 1.2.0 与两架构 glibc 通过 | 已知差异 | 待确认：是否修改 musl runtime；不在内核特判 size 1 |
| `pwrite()` + `O_APPEND` | Linux baseline 与双架构 musl/glibc 16-case pwrite/pwritev 簇通过；显式记录为 Linux 偏离 POSIX 的兼容选择 | 双架构已验证 | 补大写/并发 append syscall 原子性 probe |
| `pthread_*`/named sem/shm/AIO/`posix_spawn` | 尚无完整 libc 组合矩阵 | 待验证 | musl/glibc 同源 probe 簇 |
| message queue、`mlockall`、XSI IPC | 需求尚无证据 | 可选扩展 | 需求触发记录；默认不实现 |

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

当前拟定的唯一所有权模型、de-thread 提交顺序、锁序与验证拆分见
[process-identity-phase5-design.md](./process-identity-phase5-design.md)。该设计当前为 `待确认`；不得以
保留退出 leader TCB 的 tombstone 作为最终实现。

退出门槛是现有 `task_phase5_probe` 从三个 expected failure 变为 `TASK_PHASE5 ALL PASS`，并追加
2/8 hart 的 exit/exec/wait/资源回收压力。

### M2.2 session 与终端状态

在稳定 process identity 上补 `getsid()`，随后把 controlling terminal 状态放入 tty/terminal 对象，
不继续在 `sys_ioctl` 中堆硬编码。最小状态包括 session owner、foreground pgrp、termios attributes 和
hangup；`setsid/setpgid/getsid/tcgetsid/tcgetpgrp/tcsetpgrp` 与 `TCGETS/TCSETS*` 共享该状态。

probe 覆盖 session leader 限制、父进程对子进程 `setpgid` 的 exec 边界、前后台组切换、孤儿组、
`SIGTTOU/SIGTTIN/SIGHUP/SIGCONT` 和 controlling tty 释放。没有完整 tty 语义前，不以固定成功或固定
窗口值宣称 job control 支持。

### M2.3 process-pending 与 restart class

将 process-directed pending queue 与 per-thread pending queue 分离，递送时才按 mask/目标线程选择；
exec、fork、线程退出和 group exit 分别定义保留/清理规则。随后独立实现 `SA_NOCLDWAIT` 和 job-control
signals。

syscall restart 不做“所有 EINTR 自动重启”。按 Linux restart class 分批扩展：

1. 无 partial side effect 的 wait 类；
2. read/write/pipe/socket，已有字节数优先；
3. 带 timeout 的 poll/futex/sleep/socket，重启时保留剩余时间或按接口契约返回；
4. 明确不可重启的调用保持 `EINTR`。

每一类都必须覆盖无 handler、无 `SA_RESTART`、有 `SA_RESTART`、默认忽略和 signal/完成竞态。

### M2 退出门槛

- `getsid`、session/pgrp、tty foreground、task lifecycle、process-pending 和 restart 分簇 probe 通过；
- 不再依赖 leader TCB 存活维持进程身份；task/frame/fd/futex 资源压力后回到基线；
- RV64/LA64 signal frame、单核和 SMP 专项均通过。

## 里程碑 M3：mmap EOF、truncate 与 SIGBUS

采用当前审计已经收敛的方案：

1. file-backed VMA 保存稳定 `(dev, ino)` identity、文件偏移和每个 resident page 的来源
   `FileShared/FilePrivateUncopied/PrivateCOW`；
2. fault 时读取当前 inode size，不使用 mmap-time EOF；完整越界页返回可区分的 bus fault，trap 层投递
   `SIGBUS`；EOF 所在部分页按契约补零；
3. truncate 成功并释放 File/PageCache 锁后，以 inode identity 去重扫描 live `MemorySet`；第一版接受
   `O(live tasks x VMAs)`，不提前增加 inode-to-VMA 反向索引；
4. shrink 撤销完整越界 PTE/resident frame，并通过现有 shootdown/residency completion 后回收；部分页
   对 shared 和未 COW private 清尾，已 COW private 按 Linux probe 的匿名页规则处理；
5. grow 后未 fault 页按当前 EOF 重新从文件读取；旧 truncate 失效不得被 writeback 反向扩容；
6. user copy 遇到同类 file fault 时返回正确的短拷贝/errno，不在内核中直接触发用户 signal handler。

退出门槛是 `mmap_phase5_probe` 的七项 expected failure 全部消失，并通过 shared/private、已 COW/未
COW、grow/shrink、并发 fault/truncate、RV64/LA64 SMP shootdown 与 frame reclaim 专项。

## 里程碑 M4：文件与时间遗留语义

该里程碑不重做 Phase 1--4，而只关闭已有文档明确保留的边界：

1. ext4 纳秒、负时间和溢出规则，以及真实 `CLOCK_REALTIME` 初始化/设置的持久化边界；
2. `UTIME_NOW/UTIME_OMIT`、atime/relatime/strictatime/`O_NOATIME` 的事件和 ctime 关系；
3. `times/getrusage` 的 user/system 拆分和子进程累计，不把 total 同填两字段当完整实现；
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
5. 重构稳定 process identity，关闭 leader exit/non-leader exec；
6. 在稳定身份上实现 controlling tty/job control，再推进 process-pending 和 restart classes；
7. 按 M3 的 resident provenance 方案关闭 mmap 七项差异；
8. 根据覆盖矩阵和 LTP 证据推进 M4、M5；没有触发证据时不进入 M6。

每个补丁说明必须包含：契约、修复前 probe、状态所有者/提交点、修复后结果、双架构状态和仍保留的
边界。这样 POSIX 工作可以持续推进，又不会提前引入 Phase 6 的大规模性能重构。
