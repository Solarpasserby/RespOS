# 4. 进程与线程管理

## 4.1 Task、Process、Thread 与线程组身份

RespOS 沿用了 Linux 的 task 思路：内核只维护一种可调度对象 `TaskControlBlock`，不再为进程和线程建立两套控制块。两者的差异不在数据结构名称，而在身份和资源的共享范围。这样，创建、调度、阻塞、信号与退出都可以复用同一套任务接口，`clone` 只需决定新任务加入哪个线程组、共享哪些对象。

每个 TCB 都有独立的 `tid`、内核栈、trap context 和调度状态。线程组组长满足 `tid == tgid`，同组其他线程拥有不同的 `tid`，但共享组长的 `tgid`。因此 `gettid()` 标识具体执行线程，`getpid()` 返回线程组身份；调度器按 tid 工作，进程级操作则以 tgid 为边界。

下列字段足以概括这一模型，完整定义见 `os/src/task/task.rs::TaskControlBlock`：

```rust
kernel_stack: KernelStack,
tid: RwLock<TidHandle>,
tgid: AtomicUsize,
thread_group: Arc<SpinLock<ThreadGroup>>,
memory_set: Arc<RwLock<MemorySet>>,
fd_table: SpinLock<Arc<FdTable>>,
sig_pending: SpinLock<SigPending>,
sig_handler: Arc<SpinLock<SigHandler>>,
```

我们没有简单地把所有状态都放进一个共享结构，而是按生命周期划分所有权：

| 归属类型 | 主要状态 | 设计目的 |
| --- | --- | --- |
| 线程私有 | 内核栈、trap context、调度状态、signal mask/pending、备用信号栈、robust list、clear-child-tid | 允许线程独立运行、阻塞和退出 |
| 线程组共享 | `ThreadGroup`、`group_exiting`、父子关系、signal handler、cwd/root、进程 timer | 保持进程级状态一致，并让组级退出只有一个清理者 |
| 创建时可选共享 | `MemorySet`、`FdTable` | 根据 `CLONE_VM`、`CLONE_FILES` 决定共享原对象还是复制；普通 fork 复制，线程通常共享 |
| 全局索引 | `TASK_MANAGER: tid -> Weak<TCB>` | 支持信号、调度和回收路径按 tid 查找任务，又不额外延长生命周期 |

`ThreadGroup` 同样只保存 `tid -> Weak<TCB>`。如果是强引用的话，线程组持有线程TCB导致其无法释放资源；父进程的 `children` 则有意保存子进程 leader 的强引用，使子进程退出后仍能保留 wait status，直到 `wait4` 完成回收。这一强弱引用组合比单纯依赖 `Arc` 计数更贴合进程语义。

这一模型在单核阶段已经能够支撑基本 fork/线程功能，但 SMP 压力暴露了两个更深的边界。其一，真正执行 `wait4` 的可能是父进程中的任意线程，退出通知不能只唤醒 leader；我们为 TCB 增加 child-wait 状态，并按线程组唤醒实际 waiter。其二，退出 TCB 会因内核栈安全而延迟释放(原因是退出线程还在内核栈上跑退出代码，这时候TCB被暂时寄存在DEAD_TASKS里面)，从而TCB的强引用不减到零。因此后续清理不再仅凭 `Arc::strong_count` 判断所有权，而是通过 TASK_MANAGER 检查线程是否还活着。这些修正分别解决了 BuildStorm 中的 wait 卡死和 pipe 写端迟迟不释放问题。

统一任务模型已经通过 `task_a_futex_exit_probe` 的真实线程组退出测试；RV64 的 `smp_phase3_probe` 在 2/4/8 核各完成 30 轮 fork/exec/wait、pipe 与网络并发回归。当前 tid 仍采用单调递增策略，没有在退出后立即复用：这是为了避开弱索引和 futex 路径仍引用旧 tid 的竞态，也是我们在进一步完善代际 ID 或统一回收前保留的安全边界。

本节核心实现集中在 `os/src/task/task.rs::{TaskControlBlock, ThreadGroup, clone_, exit_process_group}`、`os/src/task/manager.rs`、`os/src/task/tid.rs` 与 `os/src/task/kstack.rs`。

## 4.2 创建与地址空间/文件描述符共享

RespOS 将 fork、线程创建和 vfork 统一收敛到 `sys_clone()`。系统调用层负责校验 flag、设置 TLS/TID 和返回值，`TaskControlBlock::clone_()` 负责构造任务及资源关系，调度器最后发布新任务。这个分层使创建语义集中在一个入口中，也便于我们在后续 SMP 调试中审查“资源准备完成”和“任务对其他 CPU 可见”之间的边界。

各 flag 的规则是可组合的，下面直接说明 0/1 对资源的影响：

| 条件 | 为 0 时 | 为 1 时 | RespOS 当前边界 |
| --- | --- | --- | --- |
| `CLONE_THREAD` | 新 tid 同时成为新 tgid，建立新线程组和父子关系 | 新 tid 加入调用者线程组，沿用 tgid | 要求同时设置 `CLONE_SIGHAND` 和 `CLONE_VM` |
| `CLONE_VM` | 通过 COW 构造子地址空间 | 共享同一个 `MemorySet` | 非线程 `vfork` 是当前例外：即使该位为 1，仍创建 COW 子地址空间，因此共享语义不完整 |
| `CLONE_FILES` | 非线程任务复制 fd table | 共享同一个 `FdTable` | 同组线程共享 fd table，不同进程独立 |
| `CLONE_SIGHAND` | 复制 handler 内容 | 共享 handler | 当前仅在线程路径共享，非线程路径复制 |
| `CLONE_VFORK` | 父任务在 child 发布后立即返回 | 父任务阻塞到 child exec 或 exit | 目前妥协实现，直接创建COW子地址空间|

表中的两处说明对应同一个 vfork 特例。Linux vfork 通常同时设置 `CLONE_VM | CLONE_VFORK`：父任务停止运行，child 在 exec/exit 前直接使用父任务的地址空间，因此此时并不存在父子并发写 MM 的问题。RespOS 没有这样做，并不是 vfork 语义要求创建新空间，而是当前 TCB 将地址空间保存为 `Arc<RwLock<MemorySet>>`，exec 又通过替换锁内的 `MemorySet` 来安装新映像。如果 vfork 父子共享这个 `Arc`，child exec 会把父任务看到的地址空间也一并替换，父任务恢复后将无法返回原程序。

因此，RespOS 当前的 `vfork` 语义并不完整：已经实现“父任务阻塞，child exec/exit 后唤醒”的同步语义，但没有实现 exec/exit 前父子共享用户地址空间的内存语义。当前代码调用 `MemorySet::from_existed_user()` 为 child 建立 COW 子地址空间，只是为了规避上述 exec 替换问题，不能将其表述为完整支持 `vfork`。后续需要让 child 在 vfork 阶段借用父任务的 MM，并在 exec 提交时只替换 child 自己的 MM 引用，而不是修改父子共同指向的 `MemorySet` 对象。

资源选择的核心只有两处，完整代码位于 `os/src/task/task.rs::TaskControlBlock::clone_`：

```rust
let memory_set = if flags.share_user_vm() {
    self.memory_set.clone()
} else {
    Arc::new(RwLock::new(MemorySet::from_existed_user(&mut self.memory_set.write())?))
};
let fd_table = if is_thread || flags.contains(CloneFlags::CLONE_FILES) {
    self.fd_table.lock().clone()
} else {
    FdTable::from_existed_user(&self.fd_table.lock())
};
```

普通 fork 不复制全部物理内存。`MemorySet::from_existed_user()` 只复制 VMA 元数据；lazy 页继续保持未分配，readonly 和 shared 页复用原 frame，private writable 页在父子两侧标记为 COW。为了保持失败安全，代码先建立子页表项，成功后才修改父页表项。这样既降低进程创建的时间和内存峰值，也避免 fork 失败在父地址空间留下映射空洞。

文件描述符采用“两层共享”设计。普通 fork 复制 descriptor 表，因此父子可以独立 close、dup 和设置 CLOEXEC；表内的 `FdEntry` 仍持有相同 `Arc<FileOp>`，从而共享文件偏移和 pipe endpoint。`CLONE_FILES` 则直接共享整张表。exec 前，RespOS 会先解除 `CLONE_FILES` 共享，再在新表上执行 `close_on_exec()`，避免 child exec 错误关闭父进程的描述符。

这条 fd 生命周期经历过一次真实的压力优化。早期实现只要发现 `FdTable` 有多个 `Arc` 引用就复制，但多线程 exec 摘除的旧 TCB 会因内核栈延迟回收继续持表，导致 pipe 写端无法降到零，cargo 一直等不到 EOF。我们随后引入 `unshare_fd_table_for_exec()`：先为执行者建立私有表，再通过 `TASK_MANAGER` 区分“仍存活的共享者”和“只剩延迟 TCB 的引用”；没有 live sharer 时主动清理旧表。该修正使 BuildStorm 越过了此前的输出收集阻塞。

`CLONE_VFORK` 同步路径的完善过程更能体现 SMP 下发布顺序的重要性。第一阶段我们增加一次性的 `vfork_parent: Weak<TCB>`，让 child 在 exec 成功或 exit 时恢复父任务，修复了父任务错误等待到子命令结束的问题（`cf30f64`）。进入 8 核测试后又发现，若先 `add_task(child)`、再登记 blocked parent，child 可能在另一 CPU 上先完成 exec，一次性 wake 随即丢失。最终顺序调整为：

```text
登记 vfork_parent → parent 进入 blocked 表 → 发布 child
→ child exec/exit → 一次性唤醒 parent
```

该顺序在 `17dcd4e` 中收敛，并由 RV64 8 核 BuildStorm 受控 trace 确认：vfork parent 能在 child exec 后恢复。这一证据只验证了同步语义，不能证明 vfork 的地址空间共享语义。`smp_shared_mm_probe` 在 RV64 2 核完成 100 轮、8 核连续完成 1000 轮跨 CPU 固定地址 remap/read，验证的是一般 `CLONE_VM` 共享路径，同样不能替代 vfork 专项验证；创建与回收综合路径则由 2/4/8 核 `smp_phase3_probe` 覆盖。

我们仍保留两项明确边界：跨进程 `CLONE_FILES + exit` 尚缺独立专项测试；`clone_()` 当前先登记 TCB、后执行部分 ptid/ctid copyout，后置 copyout 失败尚未形成完整 rollback。相关 SMP 运行证据目前来自 RV64，LA64 已通过源码构建门禁，但不能据此宣称双架构并发行为均已验证。

本节实现与演进可从 `os/src/syscall/process.rs::sys_clone`、`os/src/task/task.rs::{clone_, unshare_fd_table_for_exec, release_vfork_parent}`、`os/src/mm/memory_set.rs::from_existed_user`、`os/src/fs/fdtable.rs` 以及提交 `cf30f64`、`17dcd4e`、`f326ac8` 中定位。

## 4.3 exec 的映像替换与 sibling 线程处理

`exec` 不是在原地址空间上继续加载程序，而是用一个完整的新映像替换当前进程。RespOS 将这条路径收敛到 `install_exec_image()`，并采用“先准备、后提交”的顺序：在新 ELF、用户栈和参数尚未全部构造成功前，不修改旧 `MemorySet`，从而使常见的格式错误、参数越界和内存不足仍能安全返回原程序。

| 阶段 | 主要工作 | 保证 |
| --- | --- | --- |
| 准备新映像 | 校验 ELF，建立新 `MemorySet`，布置 argv/envp/auxv | 失败时旧程序仍可继续运行 |
| 停止 sibling | 标记其他线程不可调度，等待远端 CPU 交还 owner，在旧 MM 中清 robust list 和 clear-child-tid | 旧用户地址不会写入新映像 |
| 提交映像 | 替换 `MemorySet`，激活新页表，重建 trap context | 返回用户态时 PC、SP 与地址空间一致 |
| 收尾 | 私有化 fd table、应用 CLOEXEC、重置信号状态、释放 vfork parent | 父进程只会看到完整提交后的 exec |

其中最关键的是 sibling 清理必须发生在地址空间替换之前。robust list 与 clear-child-tid 保存的都是旧映像中的用户地址；如果先安装新 MM，再处理这些地址，内核可能把 `FUTEX_OWNER_DIED` 或 0 写进新程序。进入 SMP 后，仅从调度队列删除 sibling 也不够，因为它可能仍在另一 CPU 上运行。我们最终形成了 stop/ack 协议：

```text
request_termination + remove_task
→ 等待所有 sibling 释放 cpu_owner
→ 清理线程私有状态
→ 替换 MemorySet
```

远端 CPU 在保存完上下文、切回 per-CPU idle 栈后才释放 owner；`publish_saved_handoff()` 看到 termination 标记后不再把旧任务放回 ready queue。该协议在提交 `b785262` 中完成，使 exec 和 group-exit 都不再提前回收另一 CPU 仍在使用的地址空间。

exec 路径还经历了两项直接面向决赛负载的优化。早期文件执行会先 `read_all()`，把整个 ELF 放进固定内核堆；约 45.6 MiB 的 cargo 可执行文件会与程序页重复占用内存，最终触发 `ENOMEM`。现在 `try_from_elf_file()` 只读取 ELF header、program headers 和 `PT_INTERP` 元数据，并把主程序的 `PT_LOAD` 保存为 private file-backed VMA，页面在首次访问时按需读入。元数据前缀限制为 1 MiB，既降低峰值，也避免畸形 ELF 放大内核分配。

另一处问题来自工具链参数规模。原实现将 argv、envp 各限制为 32 项，cargo 启动 rustc 时稳定返回 `E2BIG`。我们将每组上限调整为 4096 项，并同时增加每组 1 MiB 的累计字节限制：既允许真实工具链运行，又没有把兼容性修复变成无界内核分配。

在 RV64 release、8 核、8 GiB 环境中，新的文件式 loader 已成功执行 45,559,552 字节的 cargo，并稳定到达 `BUILDSTORM_TOOLCHAIN ok`；ARG_MAX 修正后 rustc 已进入 `Compiling minibuild`。这些结果证明大 ELF 加载与长参数链已经越过原阻塞点，但完整 minibuild/full compile 尚未完成，不能写成 BuildStorm 全量通过。

当前还有两个实现边界：RespOS 暂时只允许线程组 leader 执行 exec，以避免非 leader 替换映像后重新组织 tgid 与父子关系；动态链接器仍由 `read_dynamic_linker()` 整文件读入，尚未复用主程序的按需加载路径。

> **TODO（图 4-1）**：补充“新旧 `MemorySet` 与 sibling cpu_owner”的四阶段时序图。
>
> **TODO（数据）**：完成 RV64 BuildStorm minibuild/full compile，并补充 LA64 大 ELF、长 argv/envp 与多线程 exec 的运行记录；当前只有双架构构建门禁，不能代替 LA64 动态验证。

本节核心实现位于 `os/src/task/task.rs::{execve_file, install_exec_image, close_other_threads_for_exec}`、`os/src/mm/memory_set.rs::try_from_elf_file` 和 `os/src/mm/mod.rs::extract_cstrings_from_user`，主要演进见 `f326ac8`、`b785262`。

## 4.4 exit、wait 与延迟回收

RespOS 将退出拆成三个不同时间点：任务停止执行、父进程回收 wait status、TCB 与内核栈最终析构。三者不能合并处理：子进程退出后必须保留足够状态供 `wait4` 读取，而当前任务又不能在仍使用自己的内核栈时直接释放 TCB。

| 时间点 | 状态变化 | 主要资源处理 |
| --- | --- | --- |
| 线程逻辑退出 | TCB 变为 Exited，并从调度器、futex 队列和线程组摘除 | 处理 robust list、clear-child-tid 和残留 futex waiter |
| 线程组退出 | 选出唯一清理者，停止 sibling，leader 向父进程发布退出状态 | 回收仅由本组持有的 MM/fd，删除 timer，将孤儿托管给 initproc |
| `wait4` 成功 | 父进程取得 status/rusage 后从 `children` 删除 child | wait status 与子进程记账只提交一次 |
| 延迟析构 | CPU 已切回 idle 栈后释放 `DEAD_TASKS` 中的强引用 | 最终回收 TCB 和内核栈 slot |

`sys_exit()` 退出普通线程时只清理该线程；当前实现中 leader 调用 `exit` 会转入线程组退出，这一点比完整 Linux 语义更简化。`exit_group()` 和致命信号则直接进入组级路径。线程组共享的 `group_exiting` 使用原子 compare-exchange 选出唯一清理者，避免多个 CPU 同时关闭 fd、回收地址空间或重复通知父进程。

组级清理同样遵守 stop/ack：先标记 sibling 终止并从调度器摘除，再等待所有远端 cpu_owner 释放，最后才处理共享 MM 和 fd table。若另一个 tgid 通过 `CLONE_VM` 或 `CLONE_FILES` 仍持有资源，退出组不能主动拆毁它。尤其是 fd table 的判断不能只看 `Arc` 计数，因为 `DEAD_TASKS` 中的旧 TCB 也会贡献引用；当前实现通过 `TASK_MANAGER` 查找不同 tgid 的 live sharer，区分真实跨进程共享和延迟引用。

`wait4` 的提交顺序也经过了失败原子性收敛。内核先把 status 和 rusage 写回用户空间，全部成功后才从 `children` 删除 child 并累加子进程时间；若用户指针无效，父进程可以修正参数后再次 wait，不会丢失退出状态或重复记账。`task_a_wait4_probe` 专门覆盖了“非法 status/rusage—有效重试—再次 wait 返回 `ECHILD`”这条路径；任务 A 阶段 RV64、LA64 各完成 100 次冷启动，相关事务断言合计 400/400 通过。

进入多核后，wait 路径先后暴露了两个 lost-wakeup 窗口。第一，实际 waiter 可能是父线程组中的非 leader 线程，因此 child 退出时需要唤醒组内标记为 waiting-for-child 的具体 tid。第二，child 可能恰好在父任务扫描 children 之后、正式进入 blocked 表之前退出；当前路径在发布 blocked 后重新检查 `exited_children`，若事件已经到达便撤销睡眠并继续回收。

最终析构采用 `DEAD_TASKS` 延迟队列。退出任务先把自身强引用放入队列并切换到 per-CPU idle context，idle loop 在确认旧上下文已经保存、旧内核栈不再执行后统一 drop。早期清理只发生在其他任务恢复等少数路径，当所有 CPU 都进入 idle 时，队列可能永久保留 fd 和 pipe 引用；现在每个 CPU 的 idle loop 都会执行 `cleanup_dead_tasks()`，关闭了“系统已经空闲，但退出资源永远不释放”的窗口。

```text
逻辑退出 → 切到 idle 栈 → cleanup_dead_tasks
       ↘ 父进程收到 SIGCHLD → wait4 copyout → children 回收
```

退出路径的当前压力证据来自 RV64 debug pub 镜像：2/4/8 核各连续 3 轮并发运行 4 路 `timeout 3 sleep 60`，共 9 轮全部出现 `SMP_EXIT_STORM_DONE`，压力段耗时 3655–5159 ms；每轮 health 检查可读并正常退出 QEMU。后续 2/4/8 核 `smp_phase3_probe` 又覆盖了 fork/exec/wait 与 pipe/socket 的组合路径。

> **TODO（图 4-2）**：补充“逻辑退出—SIGCHLD—wait4—DEAD_TASKS 析构”时序图，并标出 waiter 可能位于非 leader 线程。
>
> **TODO（数据）**：增加跨进程 `CLONE_FILES + exit`、所有 CPU idle 时的 pipe EOF 专项 probe；补充 LA64 并发 exit/wait 压力。现有 LA64 证据仅覆盖任务 A 阶段专项和当前构建，不代表最新 SMP 退出路径已动态验证。

本节核心实现位于 `os/src/task/task.rs::{cleanup_exiting_thread, exit_process_group, notify_parent_exit}`、`os/src/syscall/process.rs::sys_wait4`、`os/src/task/scheduler.rs::DEAD_TASKS` 与 `os/src/task/processor.rs::run_tasks`，主要演进见 `57046f1`、`17dcd4e`、`f326ac8`、`b785262`。
