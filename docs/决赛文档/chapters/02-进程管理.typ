= 2. 进程管理
<2-进程管理>
#quote(block: true)[
本章回答：RespOS 如何统一表示进程与线程，并在创建、调度、上下文切换、退出和多核并发之间维持一致的任务生命周期？
]

== 2.1 概述与设计目标
<21-概述与设计目标>
RespOS 采用与 Linux task 模型相近的统一任务设计：内核只维护一种可调度对象 `TaskControlBlock`（TCB），进程与线程的差异由身份和资源共享边界表达，而不是拆成两套控制块。每个任务都有独立的 tid、内核栈、寄存器现场和调度状态；线程组组长满足 `tid == tgid`，同组线程拥有不同 tid，但共享 tgid。因此，`gettid()` 标识具体执行线程，`getpid()` 返回线程组身份，调度器始终按 tid 调度，进程级退出、信号和等待则以线程组或 tgid 为边界。

这一模型要同时解决五类问题：任务对象如何持有资源，`clone` 如何组合共享语义，阻塞与唤醒如何避免丢失事件，任务切换如何保证旧上下文已经保存，以及退出任务如何在不释放当前内核栈的前提下完成回收。表 2-1 给出了这些问题与 RespOS 设计的对应关系。

#strong[表 2-1 进程管理的设计目标与实现抓手]

#figure(
  align(center)[#table(
    columns: 3,
    align: (auto,auto,auto,),
    table.header([设计目标], [RespOS 的实现], [产生的效果],),
    table.hline(),
    [统一进程与线程], [TCB + tid/tgid + `ThreadGroup`], [创建、调度、阻塞、信号和退出复用同一套任务接口],
    [明确资源共享边界], [`Arc<MemorySet>`、`Arc<FdTable>`、共享 handler 表], [`clone` 可按 flag 选择共享或复制，fork/线程语义不混用],
    [保证阻塞唤醒正确], [ready/blocked 分离、事件单赢家、发布后复查], [覆盖 wait、futex、pipe 和 signal 的 lost-wakeup 窗口],
    [保证多核切换安全], [per-CPU idle context、handoff slot、`cpu_owner`], [其他 CPU 不会恢复尚未保存完成的任务上下文],
    [分离退出与析构], [Exited 状态、父进程强引用、`DEAD_TASKS`], [wait status 可保留，TCB 与内核栈在安全栈上延迟释放],
  )]
  , kind: table
  )

== 2.2 统一任务对象与资源所有权
<22-统一任务对象与资源所有权>
=== 2.2.1 TaskControlBlock
<221-taskcontrolblock>
TCB 不是简单的"寄存器集合"，而是一个任务从创建到析构的资源所有权中心。代码片段 2-1 摘录了当前实现中最能说明身份、调度、进程关系、资源共享和信号状态的字段；未列出的 UID/GID、调度属性、资源限制和计时字段遵循相同的所有权原则。

#strong[代码片段 2-1 `TaskControlBlock` 的代表性字段]

```rust
// os/src/task/task.rs（节选）
pub struct TaskControlBlock {
    kernel_stack: KernelStack,
    tid: RwLock<TidHandle>,
    tgid: AtomicUsize,

    thread_group: Arc<SpinLock<ThreadGroup>>,
    group_exiting: Arc<AtomicBool>,
    terminate_requested: AtomicBool,
    task_status: SpinLock<TaskStatus>,
    cpu_owner: AtomicUsize,

    parent: Arc<SpinLock<Option<Weak<TaskControlBlock>>>>,
    children: Arc<SpinLock<BTreeMap<usize, Arc<TaskControlBlock>>>>,
    exited_children: Arc<SpinLock<BTreeSet<usize>>>,

    memory_set: Arc<RwLock<MemorySet>>,
    fd_table: SpinLock<Arc<FdTable>>,

    sig_pending: SpinLock<SigPending>,
    sig_stack: SpinLock<SignalStack>,
    sig_handler: Arc<SpinLock<SigHandler>>,

    interruptible: AtomicBool,
    waiting_for_child: AtomicBool,
    interrupted: AtomicBool,
}
```

代码片段 2-1 体现了三种并发手段的分工。原子字段用于频繁读取且状态较小的身份或协议位，例如 tgid、终止请求和 CPU owner；`SpinLock` 保护需要整体一致更新的任务状态、父子集合和信号状态；`RwLock<MemorySet>` 允许地址空间查询并发进行，同时把页表或 VMA 修改收敛到写侧。trap context 并不是 TCB 中的独立字段，而是固定放在该任务内核栈顶端；`TaskContext` 位于其下方，由 `__switch` 保存和恢复。

=== 2.2.2 强弱引用与生命周期
<222-强弱引用与生命周期>
RespOS 根据对象是否应延长任务生命周期选择 `Arc` 或 `Weak`。如表 2-2 所示，线程组和全局任务索引只保存 TCB 的弱引用，避免"索引持有对象、对象又持有索引关系"造成无法析构；父任务的 `children` 则有意持有子进程 leader 的强引用，使子进程退出后仍能保留 wait status，直到 `wait4` 提交回收。

#strong[表 2-2 任务相关对象的所有权与共享范围]

#figure(
  align(center)[#table(
    columns: 4,
    align: (auto,auto,auto,auto,),
    table.header([对象或状态], [创建者与共享范围], [更新/释放方式], [用户可见语义],),
    table.hline(),
    [`KernelStack`、trap context、`TaskContext`], [每个 TCB 独占], [CPU 切到 idle 栈后随 TCB 延迟析构；slot 可复用], [每个线程可独立陷入内核和被调度],
    [`ThreadGroup`], [同组线程共享，成员表保存 `Weak<TCB>`], [创建线程时加入，线程退出或 exec 清理时移除], [tid 不同但 tgid 相同，组级操作作用于全部线程],
    [`MemorySet`], [fork 默认建立 COW 子空间；`CLONE_VM` 可共享], [exec 替换，最后一个所有者退出时回收], [决定进程间内存隔离或线程间地址空间共享],
    [`FdTable`], [fork 复制描述符表；`CLONE_FILES` 或线程创建时共享], [close/exec/exit 更新；exec 先私有化再处理 CLOEXEC], [决定一方 close 是否影响另一方的 fd 表],
    [`SigPending`、mask、备用信号栈], [每线程独占], [投递、sigprocmask、sigreturn 和 exec 更新], [每个线程可独立屏蔽、等待和处理信号],
    [`SigHandler`], [同一线程组共享], [`sigaction` 更新，exec 重置用户 handler], [任一线程修改 handler 后全进程可见],
    [`TASK_MANAGER`], [全局 `tid -> Weak<TCB>`], [创建时加入，逻辑退出时移除], [kill/tkill/tgkill、调度和清理路径可按 tid 定位任务],
    [`children`], [线程组共享，保存子进程 leader 的 `Arc<TCB>`], [子进程创建时加入，`wait4` 成功后删除], [zombie 状态在父进程读取前不会丢失],
  )]
  , kind: table
  )

图 2-1 展示了这些对象之间的关系。一个"进程"由线程组身份和一组共享资源共同构成；TCB 才是真正进入调度器的执行单元。

#strong[图 2-1 进程、线程与资源对象关系]

```text
进程（tgid）
├─ ThreadGroup ──Weak──> TCB(tid = tgid, leader)
│                    ├─ TCB(tid = n1)
│                    └─ TCB(tid = n2)
├─ Arc<MemorySet>  <──── 同组线程通常共享
├─ Arc<FdTable>    <──── 同组线程共享；fork 默认复制表
├─ Arc<SigHandler> <──── sigaction 进程级可见
└─ children: Arc<TCB> ──> 保留子进程 wait status

每个 TCB 独占：KernelStack + TrapContext + TaskContext + SigPending + SignalStack
```

== 2.3 任务状态与生命周期
<23-任务状态与生命周期>
=== 2.3.1 状态机与调度不变量
<231-状态机与调度不变量>
RespOS 使用 `Ready`、`Running`、`Blocked`、`Stopped` 和 `Exited` 五种任务状态。图 2-2 中最重要的不变量是：同一个 tid 不能同时出现在 ready queue 与 `blocked_tasks` 中；多核取任务时，出队与 CPU owner 认领必须处于同一调度临界过程，避免其他 CPU 或唤醒者看到"已出队但仍可再次发布"的中间状态。

#strong[图 2-2 任务状态转换]

```text
创建/唤醒 ──> Ready ──出队并认领 CPU──> Running
                 ^                         │
                 │ yield / 时钟抢占         │ sleep / futex / pipe / wait
                 └────保存完成后发布────────┘
                                           v
                                        Blocked
                                           │ 事件、超时或信号
                                           └──────────────> Ready

Running ──停止类信号──> Stopped ──SIGCONT──> Ready
Running ──exit / 致命信号──────────────────> Exited
```

队列锁只能保证容器本身不被并发破坏，不能自动保证状态机正确。RespOS 因此还通过 `task_index` 阻止重复入队，通过 `blocked_tasks` 保存真正睡眠的任务，并在 debug 构建中检查队列、索引和状态三者是否一致。

=== 2.3.2 创建：fork、clone 与共享边界
<232-创建forkclone-与共享边界>
`fork`、线程创建和 vfork 最终都收敛到 `sys_clone()`。系统调用层校验 flag，设置父/子 tid、TLS、用户栈和子任务返回值；`TaskControlBlock::clone_()` 构造身份及资源关系；最后由调度器发布新任务。表 2-3 只列出会改变对象身份或共享关系的关键 flag，常规 ABI 标志不在此展开。

#strong[表 2-3 clone 关键标志与当前共享语义]

#figure(
  align(center)[#table(
    columns: 4,
    align: (auto,auto,auto,auto,),
    table.header([标志], [未设置], [设置后], [当前实现边界],),
    table.hline(),
    [`CLONE_THREAD`], [新 tid 同时成为新 tgid，创建新线程组], [新 tid 加入调用者线程组并沿用 tgid], [要求 `CLONE_SIGHAND`，后者又要求 `CLONE_VM`],
    [`CLONE_VM`], [通过 COW 建立子 `MemorySet`], [共享同一个 `MemorySet`], [非线程 vfork 为保护父映像，当前仍建立 COW 子空间],
    [`CLONE_FILES`], [新进程复制 descriptor 表], [共享同一个 `FdTable`], [同组线程无条件共享 fd table],
    [`CLONE_VFORK`], [父任务可在 child 发布后继续], [父任务阻塞到 child 成功 exec 或 exit], [已实现同步边；未直接共享父地址空间],
  )]
  , kind: table
  )

代码片段 2-2 是资源选择的核心。这里的"复制 fd table"并不复制每个 open-file description：新表中的 `FdEntry` 仍持有相同的 `Arc<FileOp>`，因此 fork 后父子可以独立 close 或设置 CLOEXEC，但共享文件偏移和管道端点；`CLONE_FILES` 则连 descriptor 表本身也共享。

#strong[代码片段 2-2 clone 的地址空间与 fd table 选择]

```rust
// os/src/task/task.rs
let memory_set = if flags.share_user_vm() {
    self.memory_set.clone()
} else {
    Arc::new(RwLock::new(MemorySet::from_existed_user(
        &mut self.memory_set.write(),
    )?))
};

let fd_table = if is_thread || flags.contains(CloneFlags::CLONE_FILES) {
    self.fd_table.lock().clone()
} else {
    FdTable::from_existed_user(&self.fd_table.lock())
};
```

普通 fork 的内存复制采用 COW。`MemorySet::from_existed_user()` 复制 VMA 元数据；未驻留的 lazy 页继续保持未分配，只读页和共享页复用原 frame，私有可写驻留页在父子两侧转为 COW。实现先完成子页表构造，再修改父页表权限，使中途 `ENOMEM` 不会在父地址空间留下映射空洞。这一机制把 fork 成本从"复制整个地址空间"降为"复制元数据并按首次写入付费"。

vfork 的难点不只是"让父任务睡眠"，而是建立不会丢失的一次性同步边。若先发布 child 再登记 parent，child 可能在另一 CPU 上立即 exec 并发送唤醒，而 parent 尚未进入 blocked 集合。RespOS 按图 2-3 固定发布顺序，并用 `vfork_parent: Option<Weak<TCB>>` 保证 exec/exit 只释放一次。

#strong[图 2-3 vfork 的一次性同步顺序]

```text
parent: new_task.set_vfork_parent(parent)
        ──> prepare_current_task_blocked(parent)
        ──> add_task(child)
        ──> switch_to_next_task()

child : exec 完整提交 或 exit
        ──> release_vfork_parent()
        ──> wakeup_task(parent.tid)
```

Linux vfork 还要求 child 在 exec/exit 前直接使用父地址空间。RespOS 当前将地址空间保存在 `Arc<RwLock<MemorySet>>` 中，而 exec 通过替换锁内的 `MemorySet` 安装新映像；若非线程 vfork 直接共享该对象，child exec 会同时替换父任务可见的映像。因此当前实现保留了父子同步语义，但用 COW 子空间隔离映像替换。这是明确的数据结构边界，而不是将当前行为误写成完整 Linux vfork。

=== 2.3.3 exec：先准备、再静止、后提交
<233-exec先准备再静止后提交>
exec 的目标是一次性替换整个进程映像，而不是在旧地址空间上逐段修补。RespOS 将文件 ELF 加载收敛到 `MemorySet::try_from_elf_file()`，将安装阶段收敛到 `install_exec_image()`，按表 2-4 的四个阶段推进。

#strong[表 2-4 exec 映像替换阶段与正确性保证]

#figure(
  align(center)[#table(
    columns: 3,
    align: (auto,auto,auto,),
    table.header([阶段], [主要工作], [正确性保证],),
    table.hline(),
    [准备新映像], [校验 ELF，创建新 `MemorySet`，布置 argv/envp/auxv], [失败发生在提交前，旧程序仍保持完整],
    [静止 sibling], [设置 `terminate_requested`，从调度器摘除并等待 `cpu_owner` 释放], [其他 CPU 不再执行旧用户映像],
    [提交映像], [替换 `MemorySet`、激活页表、重建 trap context], [PC、SP、寄存器与新地址空间同时生效],
    [提交外围状态], [私有化 fd table、执行 CLOEXEC、重置信号状态、释放 vfork parent], [父任务只会在 exec 可见状态完整后恢复],
  )]
  , kind: table
  )

图 2-4 展示了多线程 exec 中的 stop/ack 协议。sibling 的 robust list 和 clear-child-tid 保存的是旧映像用户地址，因此必须在地址空间替换前处理；仅从 ready queue 删除任务也不够，因为它可能仍在远端 CPU 上执行。远端 CPU 只有在 `__switch` 保存完上下文并回到 per-CPU idle 栈后才释放 `cpu_owner`，`publish_saved_handoff()` 对已经请求终止的任务不再重新发布。

#strong[图 2-4 多线程 exec 的静止与提交顺序]

```text
准备完整新映像
   └─> request_termination(siblings)
       └─> remove_task(siblings)
           └─> 等待所有 cpu_owner 释放
               └─> 在旧 MM 中清理 robust list / clear-child-tid
                   └─> 替换 MemorySet 与 TrapContext
                       └─> CLOEXEC + signal reset + 唤醒 vfork parent
```

文件式 exec 还采用 private file-backed VMA 按需加载主程序的 `PT_LOAD`。内核只读取 ELF header、program header 和 `PT_INTERP` 元数据，元数据前缀上限为 1 MiB；程序页面在首次访问时按页读入。与早期 `read_all()` 整文件载入相比，这一设计避免约 45 MiB 的 cargo 可执行文件在内核堆与用户页中重复占用。argv 和 envp 每组允许最多 4096 项，同时以每组 1 MiB 累计字节限制约束内核分配，既支持真实工具链，又避免无界输入。

当前 exec 只允许线程组 leader 执行，以避免非 leader 替换映像后重新组织 tgid 与父子关系；动态链接器仍采用整文件读取，尚未复用主程序的按需路径。这两个边界不影响上述提交顺序，但限定了当前兼容范围。

=== 2.3.4 exit、wait 与延迟析构
<234-exitwait-与延迟析构>
RespOS 将"停止执行""向父进程保留退出状态"和"释放 TCB/内核栈"分成三个不同时间点。表 2-5 说明了每个时间点的资源处理职责；这种分离是 wait 语义和内核栈安全共同要求的结果。

#strong[表 2-5 退出与回收的分阶段职责]

#figure(
  align(center)[#table(
    columns: 3,
    align: (auto,auto,auto,),
    table.header([时间点], [状态变化], [主要处理],),
    table.hline(),
    [线程逻辑退出], [当前 TCB 变为 `Exited`], [清理 robust list、clear-child-tid、futex waiter，并从调度器/线程组/任务索引摘除],
    [线程组退出], [原子选出唯一 teardown owner], [静止 sibling，处理共享 MM/fd/timer，托管孤儿，leader 向父进程发布 SIGCHLD 与 wait status],
    [`wait4` 成功], [父任务删除 `children[child_tid]`], [先 copyout status/rusage，成功后再删除 zombie 并累计子进程时间],
    [安全栈上析构], [idle loop 清空 `DEAD_TASKS`], [最终 drop TCB，回收内核栈 slot；已有内核页表映射保留供复用],
  )]
  , kind: table
  )

`exit_group()` 和致命信号共用组级退出路径。共享的 `group_exiting` 通过 compare-exchange 选出唯一清理者，防止多个 CPU 重复清空 fd table、回收地址空间或通知父进程。组级退出沿用 exec 的 stop/ack 协议：先阻止 sibling 再次被 claim，再等待远端 owner 释放，最后处理共享资源。

`wait4` 采用 prepare/copyout/commit。内核先将 status 和 rusage 写回用户空间，所有 copyout 成功后才从 `children` 删除 child 并提交子进程时间；用户指针无效时，父任务可修正参数后再次 wait，不会丢失退出状态。SMP 下实际 waiter 可能是父线程组中的任意线程，因此子进程退出会唤醒所有设置了 `waiting_for_child` 的成员；waiter 在发布 blocked 后还会复查 `exited_children` 和中断标志，覆盖"扫描完成、正式睡眠之前 child 恰好退出"的窗口。

图 2-5 展示了父子回收与本任务析构两条并行生命周期。父进程回收的是用户可见的 zombie 状态，idle loop 回收的是已经不再执行的内核对象，两者不能互相替代。

#strong[图 2-5 退出通知、wait 与最终析构]

```text
child 逻辑退出 ──> Exited + SIGCHLD ──> parent wait4 copyout ──> children 删除
       │
       └─> 当前 CPU 切到 idle 栈 ──> DEAD_TASKS ──> cleanup_dead_tasks ──> drop TCB/栈 slot
```

== 2.4 分层优先级调度
<24-分层优先级调度>
RespOS 的调度器没有照搬 Linux CFS，而是采用便于检查的分层固定优先级队列，在同一框架下承载实时任务、普通任务和低优先级后台任务。表 2-6 描述了当前选择顺序和兼容边界。

#strong[表 2-6 调度策略与就绪队列]

#figure(
  align(center)[#table(
    columns: 4,
    align: (auto,auto,auto,auto,),
    table.header([任务类型], [队列组织], [选择顺序], [当前语义边界],),
    table.hline(),
    [`SCHED_FIFO` / `SCHED_RR`], [优先级 1～99 的 RT 队列], [数值越大越优先，同级 FIFO], [周期时钟当前会重新排队两者，尚未完整区分 Linux FIFO 与 RR],
    [`SCHED_OTHER` / `SCHED_BATCH`], [nice -20～19 的 40 个普通队列], [nice 越小越优先，同级轮转], [固定优先级模型，不维护 CFS 虚拟运行时间],
    [`SCHED_IDLE`], [单独的 idle 队列], [RT 与普通队列均无可运行任务时选择], [用于最低优先级后台工作],
  )]
  , kind: table
  )

代码片段 2-3 展示了调度器的核心容器。RT 和普通队列各有位图，选择任务时可以跳过空优先级；`task_index` 记录 tid 当前所在的 ready queue，既支持快速删除，也用于拒绝重复入队；真正睡眠的任务保存在独立的 `blocked_tasks` 中。

#strong[代码片段 2-3 分层调度器的数据结构]

```rust
// os/src/task/scheduler.rs
pub struct Scheduler {
    rt_queues: Vec<VecDeque<Arc<TaskControlBlock>>>,
    normal_queues: Vec<VecDeque<Arc<TaskControlBlock>>>,
    idle_queue: VecDeque<Arc<TaskControlBlock>>,
    rt_bitmap: u128,
    normal_bitmap: u64,
    task_index: HashMap<usize, ReadyQueue>,
    blocked_tasks: HashMap<usize, Arc<TaskControlBlock>>,
}
```

时钟抢占和主动 yield 都会经过 per-CPU handoff，再由 idle loop 将仍为 `Running` 且未被请求终止的任务改为 `Ready` 并放回队尾。因此同优先级内形成简单 round-robin，不同优先级之间仍保持 RT → normal → idle 的严格选择顺序。

== 2.5 上下文切换与内核栈安全
<25-上下文切换与内核栈安全>
RespOS 采用同步有栈的内核任务切换。用户态 trap 保存完整的 `TrapContext`；真正的内核任务切换以函数调用为边界，只需保存调用约定中的 callee-saved 寄存器、线程指针和页表 token。代码片段 2-4 给出了跨架构共用的 `TaskContext` 表示：RISC-V 的 `s0-s11` 与 LoongArch 的对应保存寄存器统一落在 `s[12]` 中，架构汇编负责具体映射。

#strong[代码片段 2-4 内核任务上下文]

```rust
// os/src/task/context.rs
#[repr(C, align(16))]
pub struct TaskContext {
    ra: usize,
    tp: usize,
    s: [usize; 12],
    mmu_token: usize,
    _padding: usize,
}
```

内核栈按 slot 分配，相邻 slot 之间保留一个 guard page。任务析构时回收 slot，但保留全局内核页表中的栈映射；后续任务复用同一 slot 时无需频繁修改共享内核页表。这一选择把短进程压力下的页表扰动转化为有限 slot 的循环复用。

多核切换的关键约束是：正在旧 CPU 上执行的任务不能直接加入全局 ready queue。早期"先入队、再 `__switch`"的顺序允许另一 CPU 立即取到该任务，恢复一份尚未保存完成的 context。RespOS 通过 per-CPU `handoff` 和 `cpu_owner` 将顺序固定为图 2-6 所示。

#strong[图 2-6 context-save 到 ready-publication 的 handoff 协议]

```text
旧 CPU / task 栈                    旧 CPU / idle 栈                 新 CPU
Running task
  └─> 移入本 CPU handoff
      └─> __switch 保存 context
                         └─> publish_saved_handoff
                             ├─> 若仍可运行：set_ready + enqueue
                             └─> release cpu_owner
                                                            └─> claim owner
                                                                └─> __switch 恢复
```

handoff 保证 ready 发布发生在上下文保存之后，`cpu_owner` 则保证新 CPU 的 claim 发生在旧 CPU 明确释放之后。即使 wakeup 在保存期间提前到达，任务也只会留在 ready queue 中等待 owner 释放，不会被恢复两次。

== 2.6 多核处理器管理
<26-多核处理器管理>
RV64 的每颗 CPU 都有独立的 `Processor`、idle task 和 handoff slot；LoongArch 当前仍以单核处理器数组入口运行。没有可运行任务时，RV64 CPU 先发布 idle 状态，再检查一次全局 ready queue，仍为空才打开本地 timer 中断并进入 WFI。第二次检查覆盖了"首次取队列失败"到"CPU 正式睡眠"之间的新任务入队窗口。

图 2-7 给出了跨核唤醒链路。IPI 只负责唤醒 WFI 中的 CPU，任务选择仍由 idle loop 完成，从而避免在中断上下文中执行复杂调度逻辑。

#strong[图 2-7 多核任务唤醒与定向 IPI]

```text
事件到达
  └─> blocked_tasks 删除 tid
      └─> task.set_ready + 加入原优先级队列
          └─> 按 CPU affinity 选择在线 idle hart
              └─> IPI 唤醒
                  └─> idle loop fetch_for_cpu + claim cpu_owner
```

affinity 必须同时约束"谁能取任务"和"唤醒谁"。`fetch_for_cpu()` 会在优先级队列内跳过当前 CPU 不允许或 owner 尚未释放的任务，同时保留其相对位置；任务入队和 owner 释放后的 kick 也只面向 affinity 允许的在线 idle CPU。后一处补发 kick 覆盖了第一次 IPI 到达过早、任务仍被旧 owner 持有的窗口。

当前调度器使用全局锁和全局 run queue。这使 ready/blocked/index 不变量集中且容易审查，也支持当前 RV64 多核正确性；当 CPU 数继续增长时，全局锁争用会成为引入 per-CPU run queue 与负载均衡的主要依据。

== 2.7 跨模块协作与功能总结
<27-跨模块协作与功能总结>
进程管理并不独立完成所有任务语义，而是为内存、文件、信号、时钟和 IPC 提供统一生命周期边界。表 2-7 总结了这些协作关系及本章所证明的设计成果。

#strong[表 2-7 进程管理的跨模块接口与设计成果]

#figure(
  align(center)[#table(
    columns: 3,
    align: (auto,auto,auto,),
    table.header([协作模块], [进程管理提供或依赖的接口], [已形成的能力],),
    table.hline(),
    [内存管理], [`MemorySet` 的 COW clone、exec 替换、active-hart 标记与回收], [fork 延迟复制、文件 ELF 按需加载、SMP 地址空间安全切换],
    [文件系统], [`FdTable` 复制/共享、CLOEXEC、退出清表], [fork 共享 open-file 状态，exec 不误关共享父表，pipe EOF 可按最后引用产生],
    [信号], [每线程 pending/mask、组共享 handler、可中断等待], [kill/tkill/tgkill、停止/继续/致命信号与 wait 唤醒接入统一 TCB],
    [futex/pipe/wait], [blocked 注册、single-winner 完成、signal interrupt], [事件、超时、signal 和退出竞争时不重复唤醒或永久睡眠],
    [时钟与 SMP], [抢占入口、per-CPU idle、affinity IPI、`cpu_owner`], [分层调度、跨核唤醒和 context handoff],
  )]
  , kind: table
  )

综上，RespOS 的进程管理不是一组孤立系统调用，而是一套围绕 TCB 生命周期建立的执行模型：tid/tgid 定义身份，`Arc`/`Weak` 定义所有权，scheduler 定义状态发布，handoff/owner 定义多核切换边界，wait 与 `DEAD_TASKS` 分别完成用户可见回收和内核对象析构。正是这些边界共同保证了 fork/exec/exit、信号和 IPC 能在同一任务模型上稳定协作。
