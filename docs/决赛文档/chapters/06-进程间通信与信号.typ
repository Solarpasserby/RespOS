= 6. 进程间通信与信号
<6-进程间通信与信号>
#quote(block: true)[
本章介绍信号、管道、futex 和 System V 共享内存四种机制，以及它们与任务阻塞和唤醒的关系。
]

== 6.1 概述与机制分工
<61-概述与机制分工>
RespOS 没有把进程间通信实现为单一总线，而是按通信语义选择不同机制：信号传递离散异步事件，管道传递有序字节流，futex 只在竞争发生时进入内核，System V 共享内存让多个地址空间直接映射同一组物理页。四者最终都与任务调度器协作，因此"如何登记等待者、由谁完成事件、何时唤醒任务"是本章共同的正确性主线。

表 6-1 对比了四种机制的对象身份、阻塞条件和典型用途。它们并非相互替代，而是分别覆盖通知、传输、同步和大块共享四类需求。

#strong[表 6-1 RespOS IPC 与信号机制分工]

#figure(
  align(center)[#table(
    columns: 5,
    align: (auto,auto,auto,auto,auto,),
    table.header([机制], [内核中的核心对象], [是否传输数据], [何时进入阻塞], [典型用途],),
    table.hline(),
    [信号], [每线程 `SigPending`、线程组共享 `SigHandler`], [`SigInfo` 与信号编号], [`sigsuspend`、`sigtimedwait` 或打断其他可中断等待], [异步通知、timer、异常、子进程状态变化],
    [管道], [共享 `PipeRingBuffer` 与读写端 `FileOp`], [有序字节流], [读空且仍有写端；写满且仍有读端], [shell 重定向、父子进程输出捕获],
    [futex], [`FutexKey`、哈希等待队列、完成状态], [用户数据留在共享内存，内核只管理等待关系], [用户值等于期望值且发生竞争], [pthread mutex/condvar、线程 join],
    [System V shm], [`ShmSegment`、共享 frame、每次 attach 记录], [进程直接读写同一物理页], [映射本身不阻塞], [大块共享数据、跨进程共享状态],
  )]
  , kind: table
  )

== 6.2 信号机制
<62-信号机制>
=== 6.2.1 线程状态与进程共享状态
<621-线程状态与进程共享状态>
RespOS 将信号状态分为线程私有和线程组共享两层。每个线程独立持有 pending 位图、mask、每个信号号对应的一份 `SigInfo` 以及备用信号栈；同一线程组共享 handler 表。这一划分使 `sigprocmask`、`sigsuspend` 和 `sigaltstack` 保持线程语义，同时保证任一线程调用 `sigaction` 后，整个进程看到一致的处理规则。

代码片段 6-1 给出了 pending 状态的当前表示。位图负责快速查找最小的可投递信号，`BTreeMap` 保存与该信号号关联的信息；同一个信号号再次到达会覆盖 info 并保持一个 pending 位，因此当前语义不是 Linux 实时信号的逐项排队模型。

#strong[代码片段 6-1 每线程信号挂起状态]

```rust
// os/src/signal/sig_struct.rs
pub struct SigPending {
    pub pending: SigSet,
    pub mask: SigSet,
    pub info: BTreeMap<i32, SigInfo>,
}
```

表 6-2 进一步说明了各对象的创建、共享和释放边界。exec 会清空 pending、重置备用栈，并将用户自定义 handler 恢复为默认动作；显式设为忽略的 handler 保持忽略，这与"新映像不继承旧用户函数地址，但保留忽略选择"的语义相符。

#strong[表 6-2 信号对象的生命周期与可见范围]

#figure(
  align(center)[#table(
    columns: 4,
    align: (auto,auto,auto,auto,),
    table.header([状态], [共享范围], [主要更新者], [生命周期边界],),
    table.hline(),
    [pending + `SigInfo`], [每线程], [kill/tkill/tgkill、异常、timer、pipe、子进程事件], [投递或 `sigtimedwait` 消费；exec 清空],
    [signal mask], [每线程], [`sigprocmask`、`sigsuspend`、handler 进入与 sigreturn], [clone 继承当前 mask；sigreturn 恢复旧 mask],
    [alt stack], [每线程], [`sigaltstack`], [新线程使用默认值；exec 重置],
    [handler 表], [线程组共享], [`sigaction`、`SA_RESETHAND`、exec], [同组线程共同可见；exec 重置用户 handler],
  )]
  , kind: table
  )

=== 6.2.2 从产生到 sigreturn 的完整链路
<622-从产生到-sigreturn-的完整链路>
信号来源包括 `kill/tkill/tgkill`、用户缺页或非法访问、timer 到期、向无读端管道写入以及子进程停止/继续/退出。线程级信号直接进入指定 tid；进程级信号在目标线程组中选择一个当前未屏蔽该信号的线程，若暂时没有成员可立即接收，则回退到 leader 或组内成员保存 pending。`kill` 还按 uid/euid/suid 检查发送权限，`tgkill` 同时校验 tid 是否属于指定 tgid。

如图 6-1 所示，内核并不在任意指令位置直接跳入用户 handler，而是在 trap 返回用户态前统一调用 `handle_signal()`。这时完整寄存器现场已经位于任务内核栈，内核可以在用户普通栈或备用栈上构造 signal frame，再通过修改 trap context 改写返回 PC、SP 和参数寄存器。

#strong[图 6-1 信号产生、挂起、投递与返回流程]

```text
信号源
  └─> 选择目标 tid
      └─> pending 位图 + SigInfo
          ├─> 目标处于可中断等待：争取 Interrupted 并唤醒
          └─> 目标继续运行
                 └─> trap 返回用户态前 handle_signal()
                     ├─> 默认动作：ignore / stop / continue / terminate
                     └─> 用户 handler：构造 SigFrame 或 SigRTFrame
                         └─> handler 返回 trampoline
                             └─> sigreturn 恢复寄存器、PC 与旧 mask
```

普通 handler 使用 `SigFrame` 保存 `SigContext` 和旧 mask；设置 `SA_SIGINFO` 时使用 `SigRTFrame`，并向 handler 传入 `siginfo_t` 与 `ucontext`。`SA_ONSTACK` 选择备用栈，`SA_NODEFER` 决定 handler 执行期间是否自动屏蔽当前信号，`SA_RESETHAND` 在首次投递后恢复默认动作。所有 frame 都通过受检用户拷贝写入；构造失败时不会带着半成品现场返回用户态，而是按致命信号终止当前任务。

默认动作按表 6-3 分为四类。Core 类当前与 Term 一样终止线程组，但不会生成 core 文件；Stop 和 Continue 会记录 wait event 并向父进程发送 SIGCHLD，使 `wait4(WUNTRACED/WCONTINUED)` 能观察到状态变化。

#strong[表 6-3 默认信号动作与当前行为]

#figure(
  align(center)[#table(
    columns: 3,
    align: (auto,auto,auto,),
    table.header([动作类别], [代表信号], [RespOS 行为],),
    table.hline(),
    [Ignore], [`SIGCHLD`、`SIGURG`、`SIGWINCH`], [消费后不改变任务状态],
    [Stop], [`SIGSTOP`、`SIGTSTP`、`SIGTTIN`、`SIGTTOU`], [任务进入 `Stopped`，父进程可等待该事件],
    [Continue], [`SIGCONT`], [将 stopped 任务重新发布为 `Ready`，并记录 continued 事件],
    [Term / Core], [`SIGTERM`、`SIGKILL` / `SIGSEGV`、`SIGABRT` 等], [进入线程组退出；Core 类暂不写 core 文件],
  )]
  , kind: table
  )

=== 6.2.3 可中断等待与架构安全约束
<623-可中断等待与架构安全约束>
信号投递与调度器之间采用"pending + 可中断标志 + 具体等待队列清理"的协作方式。目标线程进入 futex、pipe、wait4 或 signal wait 前先设置 `interruptible`；投递者发现存在未屏蔽且非忽略信号后，将完成状态改为 `Interrupted`，从 futex 等专用队列移除 waiter，并尝试从 scheduler 的 blocked 集合唤醒 tid。等待者恢复后返回 `EINTR`，而不是继续无期限睡眠。

这里的关键不是多调用一次 `wakeup_task()`，而是让 wake、timeout、signal 和 exit 只能有一个完成者。否则同一 tid 可能重复进入 ready queue，或者一条路径删除了另一条路径仍要使用的 waiter。futex 将这一规则显式编码为完成状态；pipe 和 wait 则通过"锁内登记 + 发布 blocked 后复查 + 恢复时清理残留"关闭竞态窗口。

RV64 的 signal return 还遵守统一的 trap 安全顺序：写回 `sstatus` 前清除 SIE，保持 kernel trap vector 直到通用寄存器和 per-CPU `sscratch` 恢复完成，最后才切换 user vector 并执行 `sret`。signal frame 来自用户地址空间，不能信任其中保存的中断使能位；这一约束同时保护普通 syscall 返回、handler 首次进入和 sigreturn。

当前 `SA_RESTART` 只完成 flag 解析，尚未实现系统调用自动重启，被信号打断的可中断系统调用通常返回 `EINTR`。pending 又按信号号合并，因此同一种实时信号连续到达时只保留一个 pending 位和一份 `SigInfo`。这两个边界明确限定了当前信号兼容层的范围。

== 6.3 管道：字节流、阻塞与端点生命周期
<63-管道字节流阻塞与端点生命周期>
=== 6.3.1 核心对象
<631-核心对象>
匿名管道由两个 `Pipe` 端点共享一个 `PipeRingBuffer`。每个端点实现 `FileOp`，读端只允许 read，写端只允许 write；`pipe2` 将两个端点分别安装进 fd table，并在第二个 fd 分配或最终 copyout 失败时回滚已经分配的描述符。代码片段 6-2 展示了缓冲区中与同步直接相关的状态。

#strong[代码片段 6-2 管道环形缓冲区与等待队列]

```rust
// os/src/fs/pipe.rs
struct PipeRingBuffer {
    buffer: VecDeque<u8>,
    capacity: usize,
    read_closed: bool,
    write_closed: bool,
    read_waiters: VecDeque<usize>,
    write_waiters: VecDeque<usize>,
    poll_waiters: Arc<PollWaiters>,
}
```

缓冲区默认容量在 RV64 与 LA64 上均为 64 KiB，并可通过受限接口按页调整。read/write waiter 保存 tid，poll waiter 则独立保存关注的事件掩码：前者负责把真正阻塞的任务移回 ready queue，后者负责通知 poll/epoll 重新检查文件状态。

=== 6.3.2 读写阻塞协议
<632-读写阻塞协议>
表 6-4 给出了读写端在不同缓冲区和端点状态下的行为。阻塞 read/write 都是可中断等待，信号到达后返回 `EINTR`；`O_NONBLOCK` 则在无法立即推进且尚未传输数据时返回 `EAGAIN`。

#strong[表 6-4 管道读写状态机]

#figure(
  align(center)[#table(
    columns: 3,
    align: (auto,auto,auto,),
    table.header([操作与条件], [返回或状态变化], [唤醒对象],),
    table.hline(),
    [read 且缓冲区非空], [取出可用字节并返回], [唤醒一个 writer，通知 `POLL_WRITE`],
    [read、缓冲区空、仍有写端], [reader 登记并进入 `Blocked`], [等待 write、写端关闭或 signal],
    [read、缓冲区空、所有写端关闭], [返回 0（EOF）], [poll 可观察 HUP/可读结束状态],
    [write 且有空间、仍有读端], [写入可容纳字节并返回], [唤醒一个 reader，通知 `POLL_READ`],
    [write、缓冲区满、仍有读端], [writer 登记并进入 `Blocked`], [等待读取、读端关闭或 signal],
    [write 且所有读端关闭], [返回 `EPIPE`，`sys_write` 产生 `SIGPIPE`], [当前任务按信号规则处理],
  )]
  , kind: table
  )

管道把"检查条件、登记 waiter、发布 blocked"放在同一把 buffer 锁保护的区间内，如图 6-2 所示。写端必须取得同一把锁才能写入并取出 reader tid，因此不会在 reader 检查完空缓冲区、尚未登记时丢掉唤醒。若 waker 在 reader 真正 `switch_to_next_task()` 前已经把它改为 `Ready`，reader 会撤销 ready 入队并继续当前上下文，避免不必要的切换。

#strong[图 6-2 管道读空时的无丢失唤醒协议]

```text
reader（持 buffer 锁）
  检查为空且 write_closed = false
    └─> read_waiters.push(tid)
        └─> prepare_current_task_blocked()
            └─> 释放 buffer 锁

writer（取得同一 buffer 锁）
  写入数据 ──> pop_read_waiter() ──> wakeup_task(tid)

reader 切换前复查：
  已经 Ready ──> 撤销队列项并继续
  仍为 Blocked ──> switch_to_next_task()
```

=== 6.3.3 EOF、EPIPE 与 poll 可见性
<633-eofepipe-与-poll-可见性>
匿名管道没有单独维护"fd 数量计数器"，其端点生命周期由 `Arc<dyn FileOp>` 管理。dup 或 fork 让多个 `FdEntry` 共享同一个端点对象，只有最后一个写端引用释放时，`Pipe::drop()` 才设置 `write_closed` 并唤醒全部 reader；最后一个读端释放时同理设置 `read_closed` 并唤醒 writer。因此 EOF 的条件是"缓冲区已空且最后一个写端对象已释放"，而不是"某个子进程已经 exit"或"当前暂时没有数据"。

这一语义与 fd table 生命周期直接相关。exec 必须先解除 `CLONE_FILES` 共享再执行 CLOEXEC，退出任务又要在安全时机释放遗留 fd 引用，否则捕获 stdout/stderr 的父进程会一直等不到 EOF。RespOS 的 `unshare_fd_table_for_exec()` 与 idle loop 的 `cleanup_dead_tasks()` 正是为这一跨模块引用链提供收敛点。

`read_ready()` 在"缓冲区非空或写端已关闭"时成立；`write_ready()` 当前要求至少有一页（或容量本身，若更小）的可写空间，而不是仅检查"未满"。端点关闭会通知 `POLL_READ | POLL_WRITE | POLL_HUP`，poll/epoll 随后重新读取实际状态。这使管道的阻塞、非阻塞和事件监控共享同一组状态来源。

== 6.4 Futex：用户态快路径与内核竞争路径
<64-futex用户态快路径与内核竞争路径>
=== 6.4.1 等待对象与 key
<641-等待对象与-key>
futex 的常见无竞争路径完全在用户态完成，只有用户值表明需要等待时才进入内核。RespOS 使用 256 个哈希桶保存 `FutexQ`，每个条目包含 `FutexKey`、等待 tid 和 bitset。key 必须表达"哪些地址实际上代表同一个同步字"，而不能只比较进程内虚拟地址。

表 6-5 给出了当前 key 生成规则。私有 futex 以 tgid 隔离同一虚拟地址；能识别的共享映射则转化为 backing 身份、页索引和页内偏移，使不同进程中的不同虚拟地址仍可落到同一等待队列。

#strong[表 6-5 FutexKey 的身份规则]

#figure(
  align(center)[#table(
    columns: 4,
    align: (auto,auto,auto,auto,),
    table.header([映射类型], [`scope` 来源], [key 中的地址部分], [目的或边界],),
    table.hline(),
    [`FUTEX_PRIVATE_FLAG`], [tgid], [原用户虚拟地址], [仅同一线程组内匹配],
    [共享文件映射], [文件 dev/ino 派生身份], [文件页索引 + 页内偏移], [不同进程映射同一文件位置可互相唤醒],
    [已有共享匿名 frame], [物理 frame 身份], [VMA 页索引 + 页内偏移], [fork 继承或共享 frame 映射可匹配],
    [System V shm], [当前 attach id], [attach 内页索引 + 页内偏移], [同一次 attach 内稳定；不同进程独立 shmat 尚不能保证相同],
    [无法解析为共享 backing], [tgid], [原用户虚拟地址], [安全回退为进程内匹配],
  )]
  , kind: table
  )

=== 6.4.2 single-winner 完成协议
<642-single-winner-完成协议>
一次 futex wait 可能被 wake、deadline、signal 或任务退出结束。代码片段 6-3 中的完成状态把竞争结果显式记录在每个 tid 的 `FutexWait` 中；状态只能从 `Pending` 成功改变一次，后到事件发现已经完成后不再重复唤醒。

#strong[代码片段 6-3 futex 等待完成状态]

```rust
// os/src/task/futex/wait.rs
enum WaitCompletion {
    Pending,
    Woken,
    TimedOut,
    Interrupted,
}

struct FutexWait {
    deadline: Option<FutexDeadline>,
    completion: WaitCompletion,
}
```

表 6-6 展示了每个完成者的提交动作。队列记录、完成记录和 deadline 索引都要同步清理；退出路径虽然不再返回用户态，仍必须删除三处状态，防止悬空 tid 被后续 wake 命中。

#strong[表 6-6 futex wait 的竞争完成者]

#figure(
  align(center)[#table(
    columns: 4,
    align: (auto,auto,auto,auto,),
    table.header([完成者], [目标状态], [队列与调度动作], [用户态结果],),
    table.hline(),
    [`FUTEX_WAKE` / `WAKE_BITSET`], [`Woken`], [从哈希桶移除并唤醒 tid], [返回 0],
    [deadline], [`TimedOut`], [删除 queue/deadline 记录并唤醒], [`ETIMEDOUT`],
    [signal], [`Interrupted`], [删除 futex waiter，由 signal 路径唤醒 scheduler], [`EINTR`],
    [task exit], [不再恢复], [清理 queue、wait、deadline 三处记录], [无返回],
  )]
  , kind: table
  )

=== 6.4.3 requeue 的线性化与当前边界
<643-requeue-的线性化与当前边界>
`FUTEX_CMP_REQUEUE` 要求"比较源 futex 值"和"唤醒/迁移 waiter"属于同一个线性化区间。如果比较发生在队列锁外，另一个线程可能在比较后修改用户值，而内核仍按旧条件迁移 waiter。RespOS 先在锁外调用 `check_user_readable()` 解析 lazy 页，再在 `FUTEX_QUEUES` 锁内用固定 4 字节的 `read_user_u32_nofault()` 复核，随后完成 wake/requeue。这样既把比较与队列修改放在同一临界区，又避免持有 no-IRQ 自旋锁时触发补页或分配。

普通和定时 `FUTEX_WAIT` 的最终值复核目前仍在持有 `FUTEX_QUEUES` 时调用通用 `copy_from_user()`，lazy/COW 页可能进入补页路径；这一点尚未完全收敛到 cmp-requeue 的"锁外预触页、锁内 no-fault 读取"模式。另一个边界来自表 6-5：两个进程分别 `shmat` 同一个 System V 段时会得到不同 attach id，因此共享段上的跨进程 futex 还缺少稳定的 segment/frame 级身份。

== 6.5 System V 共享内存
<65-system-v-共享内存>
=== 6.5.1 段对象与映射生命周期
<651-段对象与映射生命周期>
System V 共享内存由全局 `SHM_TABLE` 管理。`ShmSegment` 保存 key、权限、创建者/最后操作者、时间戳、删除/锁定状态以及一组 `Arc<FrameTracker>`；每次 `shmat` 还分配 attach id，将同一组 frame 映射进调用者的 `MemorySet`。这里需要区分两种身份：segment/frame 是共享对象，attach 则表示某个地址空间中的一次映射关系。

图 6-3 展示了段从创建到删除的完整生命周期。`IPC_RMID` 在仍有 attach 时只设置 `marked_removed`，使新查找不再获得该段；最后一个 attach 消失后才从全局表删除并释放 frames，符合"删除名字不立即破坏现有映射"的延迟删除语义。

#strong[图 6-3 System V 共享内存生命周期]

```text
shmget(key, size)
  └─> 创建 ShmSegment + 分配共享 frames
      ├─> shmat ──> 分配 attach_id ──> MemorySet 映射同一组 frames
      │                                  └─> shmdt：删除本次 attach
      └─> shmctl(IPC_RMID)
            ├─ nattch == 0：立即删除 segment
            └─ nattch > 0 ：marked_removed
                            └─> 最后一次 shmdt 后删除 segment
```

表 6-7 总结了各系统调用的对象变化。`SHM_RDONLY`、`SHM_EXEC`、`SHM_RND` 和 `SHM_REMAP` 会影响映射权限或位置，`shmctl` 还提供 IPC\_STAT/SET、SHM\_STAT/INFO 及 LOCK/UNLOCK 的受限状态管理。

#strong[表 6-7 System V 共享内存操作与状态变化]

#figure(
  align(center)[#table(
    columns: 3,
    align: (auto,auto,auto,),
    table.header([操作], [主要检查], [成功后的状态变化],),
    table.hline(),
    [`shmget`], [key/IPC\_CREAT/EXCL、大小、权限、全局页数与段数上限], [创建或返回 `ShmSegment`，新段一次性分配所需 frames],
    [`shmat`], [shmid、访问权限、地址对齐与已知 flag], [在当前 `MemorySet` 映射共享 frames，登记 attach owner，更新 atime/lpid],
    [`shmdt`], [地址对齐且属于某个 shm attach], [删除该 attach，更新 dtime/lpid，必要时完成延迟删除],
    [`shmctl(IPC_RMID)`], [owner 或特权权限], [无 attach 时删除；否则标记 `SHM_DEST`],
    [`shmctl(SHM_LOCK/UNLOCK)`], [owner 或特权权限], [记录 locked 状态和 ctime],
  )]
  , kind: table
  )

=== 6.5.2 shmat 的失败原子性
<652-shmat-的失败原子性>
`shmat` 同时修改全局段表和进程地址空间，任一阶段失败都不能留下"统计显示已 attach、页表却没有映射"的半成品。RespOS 先在 `SHM_TABLE` 中校验权限、复制 frame 引用并预留 attach id，然后调用 `MemorySet::mmap_area()` 建图；映射失败会删除 `attach_owners[attach_id]`，只有映射成功后才更新 segment 的 atime 与 lpid。图 6-4 给出了这一 prepare/commit 顺序。

#strong[图 6-4 shmat 的 prepare/commit 协议]

```text
prepare（SHM_TABLE 锁内）
  校验 segment/权限 ──> clone frames ──> 分配 attach_id 并登记 owner
                                      │
                                      v
map（MemorySet 写锁内）
  选择地址 ──> 建立全部 PTE/VMA ──> flush TLB
        │失败                         │成功
        v                             v
回滚 attach_owner              commit atime / lpid
```

当前实现不支持 System V 消息队列和 semaphore，`SHM_HUGETLB` 明确返回错误。系统也没有 swap，因此 `SHM_LOCK/SHM_UNLOCK` 主要维护 ABI 可见状态，不能解释为完整的换页锁定机制。这些边界不会削弱共享 frame 与延迟删除的主体语义，但限定了 System V IPC 的实现子集。

== 6.6 跨模块协作与功能总结
<66-跨模块协作与功能总结>
四种机制最终通过 TCB、fd table 和 `MemorySet` 汇合。表 6-8 总结了本章的关键设计成果及其依赖关系。

#strong[表 6-8 IPC/信号机制的跨模块协作]

#figure(
  align(center)[#table(
    columns: 3,
    align: (auto,auto,auto,),
    table.header([机制], [依赖的内核边界], [已形成的能力],),
    table.hline(),
    [信号], [TCB 的线程组/可中断状态、trap context、scheduler wakeup], [异步产生、线程选择、handler frame、默认动作与 sigreturn 完整闭环],
    [管道], [`FdTable`/`FileOp` 引用生命周期、blocked/ready 调度、poll waiters], [阻塞与非阻塞字节流、EOF/EPIPE/SIGPIPE、poll/epoll 通知],
    [futex], [`MemorySet` backing 身份、哈希等待队列、timer 与 signal], [wait/wake/bitset/requeue、超时和 signal 的 single-winner 竞争处理],
    [System V shm], [frame 所有权、VMA/PTE 建图、全局段表], [shmget/shmat/shmdt/shmctl、共享物理页、失败回滚与延迟删除],
  )]
  , kind: table
  )

这些机制虽然用途不同，但都要经过任务的阻塞、唤醒和退出路径。信号处理异步事件，管道传递字节流，futex 处理低开销同步，共享内存直接提供数据共享；对象身份、waiter 注册和最后引用释放则分别由各自模块维护。
