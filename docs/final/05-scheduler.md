# 5. 调度器与任务执行

## 5.1 调度模型与 ready/blocked 状态

RespOS 的调度器从早期的单一队列逐步扩展为分层队列。我们的目标不是照搬 Linux CFS，而是先用一套容易检查、能够覆盖实时任务、普通任务和低优先级后台任务的模型，支撑双架构测试和 RV64 多核运行。

| 任务类型 | 队列划分 | 选择顺序 | 当前边界 |
| --- | --- | --- | --- |
| `SCHED_FIFO` / `SCHED_RR` | 1～99 共 99 个 RT 队列 | 数值越大越先运行，同级先进先出 | 周期时钟目前会让两者都重新排队，尚未完整区分 Linux FIFO 与 RR |
| 普通任务 | nice -20～19 共 40 个队列 | nice 越小越先运行，同级轮转 | 使用固定优先级，不是 CFS 的虚拟运行时间模型 |
| `SCHED_IDLE` | 单独的 idle 队列 | RT 和普通队列都为空时运行 | 适合低优先级后台工作 |

RT 和普通队列各有一张位图，调度器可以跳过空队列；`task_index` 记录 tid 所在的队列，用来阻止重复入队并检查索引一致性；真正睡眠的任务则放在独立的 `blocked_tasks` 中。任务不会同时出现在 ready 和 blocked 集合里，这是整个调度状态机最基本的约束。

```text
创建 / 唤醒 ──→ Ready ──取出并认领──→ Running
                    ↑                    │
                    │       yield / 时钟 │
                    └────保存完成后发布───┘

Running ──sleep / futex / wait──→ Blocked ──事件到达──→ Ready
Running ──exit──────────────────→ Exited
```

进入多核后，我们发现“队列操作加锁”还不够。取任务时必须在同一段调度器临界区内完成出队和 CPU 认领，否则 signal 或 wakeup 可能看到一个既不在 ready 队列、状态又仍是 Ready 的任务。当前 `fetch_task()` 先选择符合优先级和 CPU 亲和性（affinity）的任务，再通过 `try_claim_running_on_cpu()` 将它交给唯一 CPU；认领失败时任务仍留在 ready 路径，等待原 CPU 完成上下文保存。

调度器的 debug 检查也经历过一次优化。早期实现会为检查队列临时分配集合，并反复扫描 140 个优先级队列；多核退出压力下，这些调试分配会放大全局堆竞争。现在检查只线性遍历一次队列，同时核对状态、索引、位图和 ready/blocked 互斥，保留了错误发现能力，也降低了 debug 内核的额外干扰。

本节核心实现位于 `os/src/task/scheduler.rs::{Scheduler, add_task, fetch_task, wakeup_task}` 和 `os/src/task/task.rs::{TaskStatus, try_claim_running_on_cpu}`，主要演进见 `57046f1`、`dc793c4`、`17dcd4e`。

## 5.2 context switch 与内核栈生命周期

RespOS 采用同步有栈的任务切换方式。每个任务拥有独立内核栈：用户态发生 trap 时，完整用户寄存器保存在栈上的 `TrapContext`；真正切换内核任务时，`__switch` 只保存函数调用约定要求保留的寄存器、线程指针和页表 token。这样既保留了清晰的内核调用栈，也避免每次调度复制整份用户上下文。

内核栈按 slot 分配，相邻栈之间留有 guard page。任务退出后只回收 slot，已有内核页表映射继续保留并供后续任务复用，减少频繁修改全局内核页表带来的开销。TCB 和栈的最终释放要等 CPU 切回 idle 栈之后进行，具体退出生命周期见 4.4 节。

多核调度中最关键的一次修正发生在 context 保存顺序上。最初的实现先把当前任务放回全局 ready 队列，再执行 `__switch`。另一颗 CPU 可能立刻取到它，并恢复一份尚未保存完成的 context；8 核测试中，这个窗口最终表现为 `sepc=0` 和同一任务在两颗 CPU 上运行。

我们为每颗 CPU 增加了一个任务交接槽（handoff slot），并把顺序改成：

```text
Running task 移入本 CPU handoff
→ __switch 保存 task context，切到本 CPU idle 栈
→ idle loop 将 task 发布为 Ready
→ 释放 cpu_owner，允许其他 CPU 认领
```

`cpu_owner` 是这条协议的第二道保险。任务从 ready 队列取出时通过原子操作认领 owner；旧 CPU 只有在已经离开旧任务内核栈后才能释放它。即使 wakeup 在 context 保存期间提前到达，新 CPU 也不能越过 owner 恢复半成品 context。

该修正后，RV64 8 核已通过 `nproc`、完整 `/proc/cpuinfo` 和四路后台 sleep/wait；随后 `smp_phase3_probe` 在 2/4/8 核各完成 30 轮 fork/exec/wait、pipe 和网络组合回归。LA64 使用相同的 `TaskContext` 和切换接口，但当前仍走单核 processor，不能把 RV64 的 SMP 结果直接外推到 LA64。

> **TODO（图 5-1）**：补充“旧 CPU 保存 context—handoff 发布—新 CPU 认领”的正确时序，并与早期错误顺序对照。

本节核心实现位于 `os/src/task/{context.rs,kstack.rs,processor.rs}`、`os/src/arch/rv64/task/switch.S` 和 `os/src/arch/loongarch64/task/switch.S`，主要演进见 `dc793c4`、`17dcd4e`。

## 5.3 idle、wakeup、affinity 与 IPI

RV64 的每颗 CPU 都有独立的 `Processor` 和 idle context。没有可运行任务时，CPU 先发布 idle 状态，再检查一次全局 ready 队列；如果仍然为空，才打开本地 timer 中断并进入 WFI 等待。第二次检查很重要：它覆盖了“第一次取队列失败”和“CPU 正式睡眠”之间恰好有新任务入队的窗口。

唤醒路径遵守一个简单顺序：

```text
事件到达 → Blocked 改为 Ready → 放入 ready 队列
        → 按 affinity 选择一颗 idle CPU → 发送 IPI
```

核间中断（IPI）在这里主要负责叫醒 WFI 中的 CPU，处理函数只确认中断，把真正的任务选择留给 idle loop。这样不会在中断中处理复杂调度逻辑。

affinity 不只在取任务时检查。`fetch_for_cpu()` 会跳过当前 CPU 无权运行的队首任务，但保留它原来的队列位置；`add_task()` 和 `wakeup_task()` 发出 kick 时也只选择掩码允许的在线 idle CPU。若第一次 IPI 到达得过早，任务仍被旧 `cpu_owner` 占用，旧 CPU 在释放 owner 后还会补发一次 kick，关闭“任务已经 Ready，但允许它运行的 CPU 又睡着了”的窗口。

RV64 8 核已用 `0x80` 到 `0x01` 的八组单核 affinity 掩码分别运行进程，全部正常退出并得到 `nproc=8`。当前调度器仍使用一把全局锁和一组全局队列，优点是状态容易审查，缺点是核数继续增加后可能形成争用；per-CPU run queue 和负载均衡属于后续性能优化，现阶段没有数据支持宣称已经完成。

> **TODO（数据）**：补充 ready 队列锁竞争、IPI 次数和不同 runnable 数量下的扩展性数据；补充 LA64 多核 affinity 验证。

本节核心实现位于 `os/src/task/{processor.rs,scheduler.rs}`、`os/src/task/task.rs::cpu_affinity_mask` 和 `os/src/arch/rv64/smp.rs::{enter_idle,kick_one_idle_hart_in}`。

## 5.4 futex、阻塞与超时的调度交互

futex 的常见路径只在用户内存中完成原子操作，真正发生竞争时才进入内核。RespOS 为等待者维护 256 个哈希桶：私有 futex 使用“tgid + 用户地址”作为 key；共享文件页和 fork 继承的共享页则根据页面身份与页内偏移生成 key，让父子进程能够进入同一等待队列。

一次 futex wait 可能由 wake、timeout、signal 或任务退出结束。早期实现只要把任务唤醒就算完成，在事件同时到达时容易重复入队或残留超时记录。现在每个 waiter 都有明确的完成状态：

| 完成者 | waiter 结果 | 调度动作 |
| --- | --- | --- |
| `FUTEX_WAKE` | `Woken` | 从 futex 队列摘除并唤醒任务 |
| deadline | `TimedOut` | 删除队列项，返回 `ETIMEDOUT` |
| signal | `Interrupted` | 删除队列项，返回 `EINTR` |
| exit | 不再返回用户态 | 清理 queue、wait 和 deadline 三处记录 |

状态只能从 `Pending` 成功改变一次，后到的事件只负责发现自己已经输掉竞争，不能再次唤醒任务。这套“只有一个事件获胜”的设计把 futex、signal 和调度器的 blocked 状态连成了一条可检查的链路。

`FUTEX_CMP_REQUEUE` 还需要保证“比较用户值”和“移动 waiter”发生在同一临界区。RespOS 先在锁外确认用户页可读，再在 futex 队列锁内进行固定 4 字节的 no-fault 读取，避免比较期间页面分配或睡眠。当前还有两个待完善点：普通 `FUTEX_WAIT` 的最终取值仍使用通用用户拷贝；两个进程分别 `shmat` 同一 System V 段时，futex key 目前使用各自的 attach id，尚不能保证进入同一等待队列。

双架构单核专项中，wake/signal/timeout 三方竞争累计 120/120 场景通过，线程组退出清理在 RV64、LA64 各完成 20/20 轮。进入 RV64 8 核后，默认构建的 race/exit probe 已通过；`CMP_REQUEUE` 强制竞态构建曾出现一次不收敛，随后连续 20/20 通过，因此仍保留为待扩大压力的边界。

这次正确性加固并非没有代价。同配置修改前后各 7 轮的中位数显示，无竞争 futex 基本不变（RV64 +0.1%、LA64 -0.5%），而竞争 futex 增加约 22%～24% 开销，主要来自 deadline、退出清理和单赢家登记。我们保留了这些检查，没有用回退竞态修复来换取更好看的数字。

本节核心实现位于 `os/src/task/futex/{mod.rs,queue.rs,wait.rs}`、`os/src/task/scheduler.rs::{prepare_current_task_blocked,wakeup_task}` 和 `os/src/mm/mod.rs::read_user_u32_nofault`，主要演进见 `57046f1`、`f35ff2d`、`3aa1fb5`。
