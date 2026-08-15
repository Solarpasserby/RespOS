# RespOS Linux/POSIX 语义与内核模型重构方案

## 目标与适用范围

本方案从已跑通 BuildStorm 的提交 `7cdae1e` 出发，后续目标从“解除单个比赛阻断”调整为：

1. 保持 RV64 BuildStorm、双架构构建和现有专项回归不倒退；
2. 以 Linux 可观察行为、POSIX 接口语义和失败原子性为判定基准；
3. 把临时 override、弱生命周期补丁和全局保守失效逐步收敛为清晰的 VFS/inode/PageCache 模型；
4. 在证据支持下继续优化，但不以吞掉错误、伪造成功或放宽并发保护换取成绩。

第一轮重点是文件属性、时间、inode/dentry 和写回；随后扩展到 namei/权限、MM、task/signal、网络与
双架构。本文是推进顺序和验收契约，不表示列出的缺陷均已复现；未核实项必须标记 `待验证`。

## 基线与证据规则

### 当前基线

- 代码基线：`1788fa2`（包含 `0c21575` 与自动比赛镜像识别）；Phase 0--3 已闭合，Phase 4 主体已完成，
  当前进入 Phase 5。
- 最近已完成本地结果：RV64 16 GiB/8 核无 feature BuildStorm `ok=true`；LA64 12 GiB/12 hart
  完整 BuildStorm `ok=true`，并已完成同步 shootdown、ASID、FP/LSX first-use、residency、按地址空间
  frame 退役与 range 传播；op=5 完整 final 失败后已回退为 op=4。
- 外部边界：课程评测平台当前暂不可用；当前基线的复评、正式镜像和平台 wall time 均为
  `待验证`，不得用本地结果补写平台通过。
- 现有门禁：RV64/LA64 顺序 release 构建，Linux 对照 probe，RV64 Phase4/socket/signal/task/MM、
  file/private-map/shared-MM/frame-reclaim 专项，以及 `cargo fmt`、`git diff --check`。每项实际执行
  结果仍应记录 commit、镜像、QEMU 参数和日期，不能把此清单当作自动通过。

### 证据优先级

1. 当前源码、Git blame/history、可复现的 syscall probe 和测试日志；
2. POSIX/Open Group 规范、Linux 主线源码和 kernel.org 文档；
3. glibc/musl 行为及 LTP/xfstests 等测试的明确预期；
4. 其他教学内核只作为结构参考，不作为语义证明。

“Linux 这样实现”不自动要求复制其内部结构；只要 ABI 可观察结果、并发结果和错误语义一致，RespOS
可以使用更简单的机制。反过来，测试暂未覆盖也不代表偏差可以保留。

## 全程不变量

后续所有阶段共同遵守：

- syscall 失败不得先发布部分新状态，除非规范明确允许部分完成；
- path、dentry、inode、open-file description 和 fd slot 不混为同一生命周期；
- 同一 inode 的 hardlink alias 必须观察同一 size/mode/uid/gid/nlink/times 与 PageCache；
- 普通文件 buffered I/O 与 `MAP_SHARED` 不维护两份互相覆盖的数据真相；
- dirty 数据只有在写回成功后才能变 clean，写回错误必须可由同步接口观察；
- truncate、rename、unlink、chmod/chown、时间更新和缓存失效必须有明确线性化点；
- lwext4 线程安全边界未被证明前，所有 C 入口继续共用唯一 `EXT4_OP_LOCK`；
- 所有用户指针在产生不可回滚副作用前完成范围、权限、lazy/COW 准备；
- task/signal/futex/socket 等阻塞路径保持 single-winner 和无 lost-wakeup；
- 诊断 feature 与正式配置分离，性能结论必须同时报告完成进度和语义门禁。

## 推进方法

每个主题按同一闭环执行：

1. 写出 Linux/POSIX 可观察契约和当前调用链；
2. 用最小 probe 复现当前差异，不能只凭代码猜测；
3. 明确状态所有者、锁序、提交点、失败点和缓存失效点；
4. 先修模型和失败原子性，再删除旧 override/兼容分支；
5. 运行专项、双架构和 SMP 门禁；
6. 使用固定 120 秒窗口检查性能回退；跨阶段里程碑再跑完整 BuildStorm；
7. 将已验证不变量写入 `architecture.md`，决策写入 `decisions.md`，结果写入 `current-status.md`。

一次提交只解决一个可描述的语义主题。不得把时间模型、PageCache、namei 和 ext4 拆锁塞进同一提交。

## Phase 0：建立语义回归框架

### 完成状态（2026-08-10）

Phase 0 已完成，后续修改从 Phase 1 开始。仓库内新增 RespOS 用户态探针
`user/src/bin/fs_metadata_probe.rs` 与 Linux 对照程序 `scripts/fs_metadata_probe_linux.c`。RespOS 探针把
当前偏差打印为 `FS_METADATA_EXPECTED_FAIL` 并以零退出码完成其余检查；Linux 对照遇到同类偏差则
直接失败，避免把内核缺陷伪装成共同预期。

Phase 1 复核发现最初的 hardlink 与目录 `ENOENT` 来自用户库 `stat()` 的 syscall 79 参数布局错误，
不是内核缺陷；修正为 `newfstatat(AT_FDCWD,...)` 后两项正常。unlink 后 fd 的 `fchmod` 路径依赖是真实
缺陷，并由 Phase 1 修复。该校正说明 probe 本身也必须按 Linux ABI 审查，expected failure 不是免审标签。

最小 LTP 清单采用仓库现有用例：`chmod01/03/05/07`、`chown01/02/03/05`、
`fchmod01..05`、`fchmodat01/02`、`fchown01/02/03/05`、`fstat02/03`、`link02/04`、
`linkat01/02`、`rename01/03..14`、`utime01..05/07`、`utimensat01`。当时完整 LTP 曾受 writable
`MAP_SHARED` 框架阻断；该阻断现已解除，但清单仍只是目标门禁，当前结果必须由新日志确认。

### 工作内容

- 建立可独立运行的 `fs_metadata_probe`，覆盖 mode/owner/nlink/times、hardlink alias、目录与普通文件；
- 建立失败注入点或可控只读/无空间场景，验证失败原子性和 errno；
- 为测试结果记录 syscall 返回值、操作前后 stat、close/reopen 后 stat，必要时 remount 后再检查；
- 整理 LTP 中与 stat/chmod/chown/utimens/link/rename/fsync 直接相关的最小清单；
- 给性能门禁固定镜像 hash、QEMU 参数、feature、guest 命令和阶段进度。

### 退出门槛

- probe 能稳定暴露至少目录 chmod 持久化和属性失败原子性当前行为；
- 测试自身在 Linux 上有可对照结果，且不依赖 RespOS 私有输出；
- 尚未修复的差异明确标为 expected failure，不用假成功隐藏。

## Phase 1：文件属性与时间模型

这是第一项实施工作。

### 2026-08-10 阶段结果

mode/owner/显式时间的提交协议已完成第一轮收敛：ext4 使用单 inode transaction，Rust 缓存只在底层
成功后发布，目录 chmod 可跨启动保持。后续 inode-number 下沉已让 unlink 后打开 fd 的
fchmod/fchown/futimens 直接作用于后端 inode，不再使用 orphan storage path。Linux/RV64 对照、
双架构构建、五项 SMP probe 与当时完整 BuildStorm 均通过。

本阶段同时校正了 Phase 0 用户库把 syscall 79 错当二参数 stat 的测试缺陷；此前 hardlink/目录
`ENOENT` 结论作废。ext4 纳秒/负时间持久化和平台 `CLOCK_REALTIME` 初始化仍未完成，作为明确边界保留，
不得用当前秒级通过结果宣称完整 POSIX 时间模型；进入相关时间语义修改前必须先补对应跨重启 probe。

### 已知或待验证问题

- 已修复：ext4 目录 `set_mode()` 只更新内存缓存、不持久化底层 inode；
- 已修复：mode/owner override 在底层操作前发布，底层失败时可能破坏失败原子性；
- 已确认：ext4 时间持久化主要使用 32-bit 秒，纳秒与真实 `CLOCK_REALTIME` 模型不完整；
- 已验证（当前运行时范围）：`UTIME_NOW/UTIME_OMIT` 的字段选择、双 OMIT no-op/不存在路径与非法 nsec
  无副作用；负时间、纳秒持久化和溢出边界仍待验证；
- 待验证：read EOF、零长度 read、readdir、`O_NOATIME`、relatime/strictatime 的完整行为；
- 已验证：unlink 后打开 fd 的 futimens/fchmod/fchown 通过 inode-number API 作用于同一 inode；
- 已验证（单线程及 reopen）：同 inode 不同 hardlink path 的属性缓存一致；并发与缓存回收仍待 Phase 2。

### 目标模型

- inode 是属性的内存所有者；path 只用于解析，不能成为已打开 inode 后续属性更新的唯一身份；
- setattr 使用“校验与准备 → 底层 transaction → 成功后发布缓存”的顺序；
- 自动 atime、数据写入 mtime/ctime、显式 utimens、chmod/chown 分成明确事件，不共享错误状态机；
- `TimeSpec` 在 VFS 内保留纳秒；底层格式不支持时按文件系统精度规则截断，而不是静默改变其他字段；
- ctime 由状态变化事件生成，用户不能直接设定；自动 atime 不更新 ctime；
- directory 与 regular/symlink 走同一持久化原则，不以类型绕过底层更新。

### 实施顺序

1. 先补 metadata probe；
2. 修目录 chmod 持久化及 chmod/chown 失败原子性；
3. 将 mode/owner/times/nlink override 收敛为 inode 级、按成功提交的状态；
4. 设计 path-independent inode setattr，优先复用 inode number/ref，解决 unlink 后 fd；
5. 补齐纳秒和 realtime 基准；
6. 覆盖 utimens/futimens/fchmod/fchown、hardlink、reopen/remount。

### 退出门槛

- metadata probe 全通过，失败后 stat 与持久化状态均未变化；
- hardlink alias 在并发及缓存回收后观察一致；
- RV64/LA64 release、五项 probe 和目标 LTP 子集通过；
- 120 秒 BuildStorm 窗口无显著回退；若回退超过 5%，先归因再进入下一阶段。

## Phase 2：inode/dentry 与 namespace 一致性

### 2026-08-10 阶段结果

Phase 2 已完成第二轮收敛：ext4 使用真实后端 inode identity，同 inode hardlink/rename/open fd 共享
对象与 PageCache；数据与属性操作已下沉为 inode-number API，不再维护 alias/orphan path。目录 raw
metadata 使用 per-inode generation，mutation 在底层成功后失效实际父目录。nlink=0 inode 按最后一个
VFS Arc（包括 File、cwd、Path/Dentry）释放后入队，并在安全点回收，不再以 open-file 计数猜测。

lookup 与 mutation 的完整 seqlock/RCU 可见性协议、lwext4 orphan-list 崩溃恢复仍保留为后续工作；
不在本阶段以拆除全局 lwext4 锁或提前 free 仍存活 inode 换取性能/即时回收。

### 目标

- 以稳定 inode identity 统一同 inode 对象，减少 path-keyed 状态；
- 明确 positive/negative dentry 的有效期、引用和 rename/unlink 行为；
- 将全局 directory metadata generation 逐步收敛为 per-directory/per-inode generation；
- create/link/unlink/rename/orphan cleanup 的缓存更新与底层修改使用同一个提交点；
- rename 跨目录时同时更新源父、目标父、被替换目标和移动 inode 的状态。

### 必测竞态

- stat 与 chmod/chown/utimens 并发；
- lookup/readdir 与 create/unlink/rename 并发；
- hardlink 后通过两个路径交替修改和读取属性；
- 打开文件 unlink 后继续 read/write/stat，最后一个引用关闭后回收；
- rename 覆盖、跨目录 rename、目录 `nlink`、`..` 与 mount boundary。

### 边界

这一阶段优化 Rust VFS 缓存一致性，不拆 lwext4 全局锁。只有 C 库共享状态、锁序和 reentrant API
得到独立证明后，才另立设计讨论细粒度 ext4 并发。

## Phase 3：PageCache、写回与持久化语义（已完成，2026-08-11）

### 目标模型

- PageCache page 明确区分 clean、dirty、writeback、error 和 pinned；
- 数据写入与 inode size/mtime/ctime 更新具有可解释的顺序；
- `close`、`fsync`、`fdatasync`、`sync`、`msync`、unmount 的保证分别定义；
- 写回失败由后续同步接口或对应 open-file error cursor 观察，不能静默清 dirty；
- 最后一个 `File::drop()` 不再是防止数据丢失的唯一机制；
- truncate/hole、mmap、buffered I/O 与回收共享同一 page identity 和 invalidate 协议。

### 实施顺序

1. 补 writeback 状态机和错误注入 probe；
2. 明确 dirty owner 与 inode/PageCache 强引用生命周期；
3. 修 fsync/fdatasync/msync 范围和错误传播；
4. 建立受控后台或批量 writeback 后，再评估取消 close 数据提交；
5. 覆盖 truncate 与 fault/read/write/writeback 并发。

### 退出门槛

- fsync 成功后重开/重新挂载数据一致；注入 I/O 错误时不会报告假成功；
- mmap+pwrite+truncate probe、稀疏文件与 frame reclaim 通过；
- dirty/page/LRU/inode registry 不随累计工作量无界增长。

## Phase 4：namei、权限与文件系统 ABI（主体已完成，2026-08-11）

### 主题

- open/create/link/unlink/rename 的 final-component、symlink、trailing slash 和 errno；
- descriptor flags 与 open-file status flags 的 Linux 分层；
- fsuid/fsgid、supplementary groups、umask、setgid directory、sticky bit；
- readonly mount、`O_NOATIME`、`AT_EMPTY_PATH`、`AT_SYMLINK_NOFOLLOW`；
- capability、POSIX ACL、immutable/append-only 作为后续扩展，不用 uid=0 特判掩盖。

### 原则

syscall 层只解析 ABI 参数；权限、path walk 和 mutation transaction 分别由稳定领域层负责。每项差异先
用 Linux 对照 probe 定义预期 errno，再修改实现。

## Phase 5：MM、task/signal、IPC 与网络语义复核

### 2026-08-13 当前状态与并行顺序

已通过首轮专项的部分包括 Phase 4 文件系统 ABI、AF_UNIX 基本 pathname/close/poll、signal 查询与
exec 保留语义；明确仍失败或未闭合的部分包括 mmap EOF/truncate/SIGBUS、task leader exit/non-leader
exec、完整 signal restart/process-pending/job control、TCP socket 等价语义和 iperf daemon 后的
iozone/wait 停滞。平台不可用只推迟正式镜像验收，不改变本地 Linux 对照和专项 probe 的退出门槛。

与架构线并行时按以下顺序推进：

1. 先做 IPC/network：socket timeout、nonblocking connect/`SO_ERROR`、MSG flags、half-close 和
   poll/epoll；同步保留 daemon + iozone 跨子系统回归。
2. 再独立处理 task/signal：leader 单独 exit、最后线程 zombie、non-leader exec/de-thread，随后才接
   `SA_NOCLDWAIT`、process-pending 和 `SA_RESTART`，一次修改只覆盖一个状态所有权主题。
3. mmap EOF/truncate/SIGBUS 先固定 Linux 契约、probe、VMA/inode identity 和失效范围；公共
   shootdown/frame completion 已稳定，但改 `MemorySet` 前仍须与架构线约定接口和验证责任。
4. Phase 1 保留的纳秒/realtime、atime 模式和 Phase 2 并发 namespace 边界不混入 Phase 5 主线，
   除非新 probe 证明它们成为当前阻断。

共享文件及接口规则以 [current-status.md](./current-status.md) 顶部双线分工为准。

### 跨 Phase 的 POSIX 语义覆盖任务

POSIX 覆盖作为 Phase 线的独立任务持续维护，不以 Linux syscall 编号数量作为完成标准。先建立接口
覆盖矩阵，逐项记录规范选项、libc 实现路径、内核入口、当前语义状态、Linux 对照 probe、RespOS
结果和双架构验证日期。Phase 5 已包含的 socket、task/signal 和 mmap 项直接引用本节状态，不重复
设计或重复修复。具体的任务拆分、唯一推进顺序和单项退出门槛见
[posix-semantics-execution-plan.md](./posix-semantics-execution-plan.md)。

当前任务包按以下优先级推进：

1. 基础 POSIX：补 `getsid()`；闭合 termios/job control；把线程组 exit/exec、`SA_RESTART`、
   process-pending、mmap EOF/truncate/SIGBUS 和 socket timeout/flags/`SO_ERROR` 纳入覆盖矩阵。
2. libc 组合接口：以 musl/glibc probe 验证 `pthread_*`、`sem_open()`、`shm_open()`、`aio_*` 和
   `posix_spawn()`；没有同名 syscall 不等于不支持，底层 futex、文件和共享内存语义仍须单独验证。
3. 可选扩展：POSIX message queue、`mlockall()`/`munlockall()`、XSI SysV message/semaphore 保持
   `待验证/按需实现`，只有规范目标、LTP 或比赛 workload 提供需求证据后才提升优先级。

单项退出门槛为：规范契约明确，Linux baseline 与 RespOS probe 的返回值、errno、阻塞/唤醒和资源
生命周期一致，相关专项及 RV64/LA64 顺序构建通过。平台不可用期间不得把本地覆盖矩阵外推为正式
POSIX conformance 或平台成绩。

文件系统模型稳定后，按风险和测试证据推进：

- MM：mmap EOF/SIGBUS、mprotect 原子性、COW、shared writeback、并发 munmap/user copy；
- task：clone/exec/exit/wait、线程组资源、robust futex、clear_child_tid；
- signal/time：选择、mask、restart/EINTR、sigwait、timer 和 clock；
- pipe/socket：阻塞与 nonblock、poll readiness、EOF/EPIPE、shutdown、SCM/ancillary 待支持范围；
- scheduler：只有计数证明是热点后才做 per-CPU runqueue，不让调度重构混入 ABI 修复。

每个子系统单独建立状态机、不变量和专项 probe，不以 BuildStorm 单一工作负载代表通用正确性。

### Phase 5 网络语义收口清单

2026-08-11 的 RV64 iperf 诊断已确认：当前 TCP 通过“登记 waiter → poll →
二次检查 → 阻塞”防止丢失唤醒，并由 `poll_interfaces()` 唤醒 waiter；但这只是
基本可用性修复，尚不是 Linux 等价的 socket 等待模型。Phase 5 必须在不修改比赛
runner 的前提下完成以下语义工作：

1. 使 `SO_RCVTIMEO`/`SO_SNDTIMEO` 真正约束阻塞 `recv`/`send`，定义超时、部分传输与
   `EAGAIN`/`EWOULDBLOCK` 的可观察结果；
2. 实现非阻塞 `connect` 的完整状态与 pending error，使 `poll` 和 `SO_ERROR` 能区分
   成功、refused、reset、timeout 和 unreachable；
3. 按 Linux 对照 probe 补齐 `MSG_DONTWAIT`、`MSG_PEEK`、`MSG_WAITALL`、`MSG_NOSIGNAL`
   的支持边界，不得继续静默忽略 flags；
4. 明确 `shutdown`/close、peer EOF/reset、发送空间释放和 accept/connect 完成各自的唤醒
   条件，包括 dup 后另一线程正在阻塞的情况；
5. 审查 signal interruption、`SA_RESTART`、部分 I/O 优先返回字节数与 `EINTR` 的边界；
6. 对 `poll`/epoll readiness、TCP half-close、EOF、`EPIPE`/`SIGPIPE` 和错误消费建立
   Linux 对照矩阵。

Phase 5 的退出门槛是：上述 probe 在 Linux 与 RespOS 上的 ABI 可观察结果一致；
iperf BASIC/PARALLEL/REVERSE UDP/TCP、空闲 listener 旁的 sleep/timeout、信号中断和
poll/epoll 回归全部通过。2026-08-13 已关闭“iperf daemon 后 sleep 不醒”的 timer-progress
边界：inet poll fallback 和 TCP/UDP blocking retry 在 no-lock 安全点推进延迟 timer work；仍需按
本清单继续收敛 socket 的 Linux ABI，不能把这一调度修复视为网络 Phase 5 全部完成。

Phase 5 还必须保留 2026-08-11 确认的跨子系统回归：先运行 musl/glibc iperf
脚本（两者均遗留 `iperf3 -s -D`），再运行 glibc iozone throughput。2026-08-13 的 RV64
release/4 GiB/1 hart 回归已完整输出 iozone group end；LA64 的 daemon→iozone throughput 专项也完成。
根因是 daemon 的 inet `poll()` fallback 长期停留在 kernel yield 循环，令 iozone 的 nanosleep
deadline 无高层 timer 安全点可消费，不是 wait/kill/process-group 契约。该顺序继续作为固定回归；
杀 daemon、调换 runner 顺序或调大 TCP poll timeout 仍不算修复。正式完整 runner 结果仍 `待验证`。

## Phase 6：细粒度并发与性能模型

只有 Phase 1--5 的状态所有权清楚后才进入：

- per-inode/per-directory VFS locks 与 directory generation；
- lwext4 inode-number API、transaction 批处理，或评估替换/重构底层文件系统；
- 后台 writeback、异步 VirtIO、多队列；
- per-CPU runqueue、allocator cache、ASID 与精确 TLB shootdown；
- LA64 12 核的对等实现和缩放验证。

网络方面，Phase 6 在 Phase 5 语义门槛通过后再将全局 TCP TID waiter 收窄为按
socket 和 read/write/connect/accept 条件分类的等待队列，消除 `poll_interfaces()` 无条件
唤醒所有 TCP 任务造成的惊群。在 VirtIO 中断、smoltcp 下一 deadline 和 scheduler idle
的协作模型经专项验证后，再去掉当前 1 ms task-timeout 兜底；不能以降低 CPU
开销为由提前删除正确性唤醒源。

优化仍遵循“先计数、一次一个主变量、语义门禁先于计时”。Linux 的细粒度结构是参考目标，不在底层
库不支持时强行模仿表面锁形态。

## 每轮验证矩阵

### 必跑快速门禁

```bash
cargo fmt --manifest-path os/Cargo.toml -- --check
cargo fmt --manifest-path user/Cargo.toml -- --check
git diff --check
make build-rv RV_USER_FEATURES= RV_KERNEL_FEATURES=
make build-la LA_USER_FEATURES= LA_KERNEL_FEATURES=
```

### 高风险 FS/MM 修改

RV64 1 GiB/8 核、`-snapshot` 下至少运行：

```text
fs_metadata_probe
buildstorm_file_probe
buildstorm_private_map_probe
smp_shared_mm_probe
frame_reclaim_probe
```

阻塞/信号/网络修改另加 `unix_socket_block_probe` 和对应 EINTR/nonblock 专项。

### 性能门禁

- 固定同一 pub 镜像、RV64 16 GiB/8 核，运行 120 秒无关变量受控的 Cargo 窗口；旧镜像短窗口只作
  本地 A/B，不作平台成绩；
- 记录完成阶段、PageCache fill、ext4 lock classes、block I/O、heap、fault、scheduler idle；
- Phase 5 每个跨子系统里程碑先跑专项与固定窗口；本地资源允许时再跑无 feature 完整 BuildStorm；
- RV64 16 GiB/8 核与 LA 12 GiB/12-hart 本地功能基线已建立；平台恢复后按正式镜像 hash 和实际
  计时边界重新验证，LA64 36 GiB 由架构线补验。

## 暂停、回退与提交规则

出现以下任一情况时停止叠加修改：

- panic、SIGSEGV/SIGBUS、文件损坏、假成功或 errno 明显回退；
- 失败路径留下部分发布状态；
- 不能说明新锁序或出现无法复现的 SMP hang；
- 内存表、dirty page、inode/dentry 或 task 状态随累计工作量无界增长；
- 性能下降超过 5% 且不能用完成工作更多解释。

修复提交应包含：问题契约、根因、实现边界、专项结果和适用平台。诊断计数可以与修复同提交，但临时
串口 trace、镜像修改和宿主环境补丁不得进入交付。完整 BuildStorm 成功不能替代语义 probe，单个
POSIX probe 成功也不能替代压力和资源闭环。

## 下一步

Phase 3 已闭合，Phase 4 主体已完成。当前从 Phase 5 的 IPC/network 开始，按“Linux 对照 → RespOS
expected-fail probe → 状态所有权设计 → 实现 → 专项/双架构/固定窗口”的闭环逐项推进；随后处理
task/signal。mmap EOF/truncate/SIGBUS 先完成设计和 probe，再基于现有 TLB shootdown 与 frame
completion 协议进入实现；改共享 `MemorySet` 前先完成接口交接。平台恢复前可以闭合本地语义任务，
但所有正式镜像/成绩结论保持
`待验证`；Phase 6 性能大改不提前并入。
