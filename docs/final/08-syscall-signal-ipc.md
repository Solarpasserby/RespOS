# 8. System Call、Signal 与 IPC

## 8.1 syscall 分发与 Linux ABI 兼容边界

系统调用是 RespOS 面向应用程序的统一入口。RV64 使用 `a7` 传入调用号、`a0～a5` 传入参数；LA64 使用对应的 `r11` 和 `r4～r9`。架构层只负责取出寄存器、推进用户 PC，并把失败转换为负 errno；进入 `syscall()` 后，两种架构共用同一套实现。

RespOS 没有把所有逻辑堆在分发表中，而是按资源归属拆到不同模块：

| 系统调用类别 | 代表接口 | 主要实现者 |
| --- | --- | --- |
| 进程与调度 | clone、exec、wait、affinity、rlimit | `syscall/process.rs` 与 task 模块 |
| 内存 | brk、mmap、mprotect、mremap、msync | `syscall/mm.rs` 与 `MemorySet` |
| 文件与网络 | open/read/write、poll/epoll、socket | `syscall/fs.rs`、`net.rs` 与对应对象 |
| signal 与时间 | sigaction、sigreturn、clock、timer | `syscall/signal.rs`、`time.rs` 与 signal/task 模块 |
| IPC 与特殊 fd | System V shm、eventfd、timerfd、memfd | `syscall/ipc.rs`、`special_fd.rs` |

这个分层的重点是让 syscall 层保持“薄”：它检查参数、读取用户结构并找到目标对象，真正的 VMA 修改、任务状态变化和文件生命周期由所属模块完成。用户指针也不会直接转成 Rust slice 解引用，而是统一通过 `copy_from_user()`、`copy_to_user()` 逐页检查权限，并处理跨页、lazy 和 COW 情况。

开发过程中，我们把失败时的行为也当作 ABI 的一部分。以 `timer_create` 为例，用户态只有拿到 timer id 后才能管理该对象，所以内核先把 id 写回用户地址，再把 timer 放入全局表；如果写回失败，系统中不会留下一个用户无法删除的 timer。wait4、shmat、timer_settime 和 prlimit 等路径也采用类似的“先准备、成功写回后再提交”顺序。

另一项原则是不过度宣称兼容性。完全未知的调用号返回 `ENOSYS`；接口存在但当前模式不支持时返回 `EOPNOTSUPP` 或 `EINVAL`。例如 signalfd、io_uring、userfaultfd 和 pidfd 当前都明确失败，不会用返回 0 冒充实现。这样短期内少通过一些只检查返回值的用例，却能避免应用在后续步骤依赖一个实际上不存在的内核状态。

任务 A/B/C 重构已用双架构 debug/release 构建和代表性专项覆盖 wait、timer、MM、IPC 与 fd 的失败路径；但当前完整 LTP 仍受镜像和版本边界影响，不能把历史通过数量直接作为最新结果。

> **TODO（数据）**：在最终提交和固定镜像上重跑 syscall 分类回归，按“完整支持 / 受限支持 / 明确拒绝”整理清单，并记录每类首个真实失败。

本节核心实现位于 `os/src/syscall/{mod.rs,errno.rs}`、各领域 syscall 文件和 `os/src/mm/mod.rs::{copy_from_user,copy_to_user}`，主要设计收敛见 `3aa1fb5`、`15fe1a5`、`cba8e24`。

## 8.2 signal 状态、投递与用户态返回

RespOS 将 signal 状态按线程和进程两层管理。每个线程拥有自己的待处理集合（pending）、屏蔽集（mask）和备用信号栈，因此不同线程可以独立屏蔽或等待信号；同一线程组共享 handler 表，使任意线程调用 `sigaction` 后，整个进程看到一致的处理规则。

signal 既可能来自 `kill/tkill/tgkill`，也可能来自用户缺页、非法访问、timer、pipe 或子进程状态变化。线程级 signal 直接进入目标 tid；进程级 signal 会在线程组中选择一个当前能够接收它的线程。若目标正在 futex、sleep 或 wait 等可中断等待中，投递路径会同时争取 `Interrupted` 状态并唤醒任务，避免 signal 已经 pending、线程却一直睡着。

完整处理流程如下：

```text
signal 产生 → 写入目标线程 pending → 必要时打断 Blocked
→ trap 返回用户态前取出一个 signal
→ 在普通栈或 alt stack 构造 signal frame
→ 跳到用户 handler → trampoline 调用 sigreturn
→ 恢复寄存器、PC 和 signal mask
```

普通 handler 的 frame 保存寄存器现场和旧 mask；设置 `SA_SIGINFO` 时，还会传入 `siginfo_t` 与 `ucontext`。`SA_ONSTACK`、`SA_NODEFER`、`SA_RESETHAND` 也已接入这条路径。没有用户 handler 时，内核按默认规则执行忽略、终止、停止或继续；当前 Core 类信号只执行终止，尚不会生成 core 文件。

signal 返回曾经暴露过一个更底层的问题：保存的用户上下文可能带有不安全的中断状态，而 trap restore 又过早切换到用户 trap vector，导致 timer 在寄存器尚未恢复完成时重入。RV64 后来把“清 SIE、恢复寄存器、最后切换 user vector”的顺序固定下来；8 核 BuildStorm 的连续 GDB 快照不再出现此前的递归 fault 特征。这个修正不仅保护普通 syscall 返回，也保护 signal handler 和 sigreturn 的返回路径。

当前 signal 仍有两个明确边界：`SA_RESTART` 尚未实现，被 signal 打断的 syscall 通常返回 `EINTR`；pending 使用位图并为每个信号号保存一份 `SigInfo`，同一种实时信号连续到达时仍会合并，尚不是完整的 Linux 实时信号排队模型。

现有双架构 futex race probe 已验证 signal 能打断等待且不会与 timeout/wake 重复完成；RV64 2/4/8 核退出压力也验证了 SIGTERM、退出和 wait 的组合路径。不过这些证据不能替代 signal frame、alt stack 和嵌套 handler 的独立回归。

> **TODO（图 8-1）**：补充“pending—用户 signal frame—handler—sigreturn”流程图，分别标出普通 frame 与 `SA_SIGINFO` frame。
>
> **TODO（数据）**：在最终 RV64/LA64 镜像上增加普通 handler、`SA_SIGINFO`、alt stack、非法 frame 和嵌套 signal 专项记录。

本节核心实现位于 `os/src/signal/`、`os/src/syscall/signal.rs`、`os/src/task/task.rs::receive_siginfo` 和两架构 `trap/`，主要演进见 `ac2ee01`、`3c19c5b`、`57046f1`、`b785262`。

## 8.3 pipe/futex 之外的 IPC 与共享内存接口

RespOS 的进程间通信不是一个孤立模块，而是由文件、内存和任务同步共同组成：

| IPC 方式 | 共享对象 | 适用场景 | 当前实现 |
| --- | --- | --- | --- |
| pipe | 内核环形缓冲区与读写端点 | 字节流、父子进程重定向 | 支持阻塞、poll/epoll 和 EOF 生命周期，详见第 7 章 |
| futex | 用户共享字 + 内核等待队列 | pthread 锁、条件变量 | 支持 wait/wake/requeue/bitset，竞争处理见 5.4 节 |
| 共享 mmap | 匿名共享页或共享文件页 | 自定义共享数据结构 | 与 `MemorySet`、共享 frame/page cache 结合，详见第 6 章 |
| System V 共享内存 | `ShmSegment` 与一组共享 frame | 通过 key/id 建立跨进程共享段 | 已实现 shmget、shmat、shmctl、shmdt |

System V 共享内存由全局 `SHM_TABLE` 管理。`shmget` 根据 key 查找或创建共享段，并为段分配物理页；`shmat` 把同一组 frame 映射进调用者地址空间；`shmdt` 只移除当前 attach；`IPC_RMID` 在仍有进程使用时先标记删除，最后一个 attach 消失后才真正释放共享段。

这条路径在重构前存在一个典型的半提交问题：shmat 还没有成功建图，就可能提前修改 attach 记录和时间。当前实现先准备地址、权限、frame 和 attach id，再调用 `MemorySet::mmap_area()`；映射失败会撤销 attach owner，只有成功后才更新 `atime/lpid`。这一顺序让错误地址、冲突映射和权限失败不会污染全局共享段状态。

当前能力仍然是 System V IPC 的一个子集：消息队列和 semaphore 没有实现；`SHM_HUGETLB` 明确拒绝；系统没有 swap，因此 `SHM_LOCK/SHM_UNLOCK` 目前主要记录可见状态，不能理解成完整 Linux 换页锁定机制。此外，两个进程分别 `shmat` 同一段后，普通读写可以共享 frame，但在这块内存上使用跨进程 futex 的 key 统一尚未完成。四天内存重构的双架构代表性回归包含 `shmat01`，记录为 4 项通过，但最新提交和最终镜像上的完整 IPC 子集仍需补跑。

> **TODO（数据）**：补充 shmget/shmat/fork/shmdt/RMID 生命周期专项、独立 shmat 上的跨进程 futex，以及最终镜像的 LTP shm 子集；消息队列和 semaphore 保持“未实现”标记。

本节核心实现位于 `os/src/syscall/ipc.rs`、`os/src/mm/memory_set.rs`、`os/src/fs/pipe.rs` 和 `os/src/task/futex/`，共享内存失败原子性主要在 `15fe1a5` 中收敛。

## 8.4 时间、睡眠、timer 与中断唤醒

RespOS 将“读取时间”和“等待事件”分开处理。硬件计数器提供基础时间，syscall 层再构造用户可见的 clock：

| clock / timer | 当前语义 | 主要用途 |
| --- | --- | --- |
| `CLOCK_REALTIME` | monotonic 加一个可调整 offset | 日历时间、绝对 deadline |
| monotonic/raw/boottime | 当前都来自持续增长的硬件运行时间 | timeout 和相对计时 |
| coarse clock | 将时间量化到 1 ms | 低开销粗粒度查询 |
| nanosleep | 登记 deadline 后阻塞任务 | 可被 signal 打断并返回剩余时间 |
| interval/POSIX timer | 到期后向进程投递 signal | alarm 和周期通知 |
| timerfd | 到期次数通过 fd 读取 | 与 poll/epoll 统一等待 |

早期 sleep 和 futex timeout 以毫秒记录截止时间，部分短等待会提前醒来。我们把相关记录调整到微秒，并把 realtime 的可调偏移与 monotonic 分开：修改系统时间不会让内部 timeout 跟着跳变。双架构 `task_a_clock_probe` 各完成 20/20 轮，确认 fine clock 报告 1 µs 分辨率、coarse clock 报告 1 ms 分辨率，并验证调整 realtime 不影响 monotonic。

timer 的对象生命周期也采用先准备、后提交。`timer_create` 在 id 成功写回后才发布对象；`timer_settime` 在 old value 写回成功后才修改 deadline；进程组退出会按 owner 删除全部 POSIX timer。owner-exit 专项在 RV64、LA64 各完成 100 轮，每轮创建的 3 个 timer 都被清理。timerfd 已支持 realtime、monotonic 和 boottime，alarm clock 与 `TFD_TIMER_CANCEL_ON_SET` 因语义不完整而明确拒绝。

多核带来的难点不在读计数器，而在“到期后由谁做复杂工作”。早期 RV64 会在任意内核态 timer trap 中扫描 futex、sleep、timerfd 和 signal registry，曾两次发生同 CPU 中断重入：一次重入全局 heap 锁，一次重入 interval timer 锁。当前 timer interrupt 先重新设置下一 tick；全局扫描固定由 timer service hart 负责，并且只在它从用户态进入 timer trap，或已经处于没有当前任务的 idle context 时执行。这样就不会在 syscall 持锁期间突然再次进入同一把锁。

该修正后，RV64 debug pub 镜像在 2/4/8 核各完成 3 轮并发 timeout/sleep 退出压力，9 轮均正常收敛。当前仍不支持真正的线程 CPU clock、suspend/TAI 和 wakeup alarm；`times()` 的 user/system 时间也只是近似记账，不能用于严格 profiling。

> **TODO（图 8-2）**：补充“用户 sleep—blocked—timer safe point—wakeup”的时序图。
>
> **TODO（数据）**：记录不同 sleep 时长的误差分布、timerfd/epoll 周期计数，以及 LA64 最新 timer/signal 压力；当前墙钟环境受宿主 `SCHED_IDLE` 影响，不作为性能结论。

本节核心实现位于 `os/src/syscall/time.rs`、`os/src/syscall/special_fd.rs::TimerFd`、`os/src/syscall/mod.rs::check_all_task_timers` 和两架构 `timer.rs`/`trap/mod.rs`，主要演进见 `163cba1`、`57046f1`、`17dcd4e`。
