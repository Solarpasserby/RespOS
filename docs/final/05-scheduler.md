# 5. 调度器与任务执行

## 5.1 调度模型与 ready/blocked 状态

【本节目的】说明任务如何从可运行、阻塞、运行到退出，并由谁提交状态变更。

【建议写什么】填写 RT/normal/idle 多级队列、`task_index`、blocked 集合、优先级/时间片、单一提交者和重复入队规则；不要只列 API。

【建议检查的 RespOS 代码】`os/src/task/scheduler.rs`；`processor.rs`；`task.rs`；`manager.rs`。

【建议查看的 Git 历史】`3aa1fb5`、`dc793c4`、`17dcd4e`；对照 context handoff 前后的队列提交顺序。

【建议准备的图 / 表】Task 状态转换图；队列/索引/blocked 一致性表。

【建议准备的测试 / 数据】sleep/yield、优先级、futex、wait4、并发进程压力；列出每次状态转换的日志证据。

【容易出现的问题】不能从多个路径直接修改 Running/Ready；不要用“队列里有任务”推断任务一定可被当前 CPU claim。

## 5.2 context switch 与内核栈生命周期

【本节目的】解释 `__switch` 保存/恢复什么、页表切换与内核栈为什么安全、任务何时可再次被其他 CPU claim。

【建议写什么】描述 `TaskContext`、内核栈、per-CPU processor、handoff slot、owner fence 和架构汇编；把“保存完成后才发布 Ready”作为关键不变量。

【建议检查的 RespOS 代码】`os/src/task/context.rs`；`kstack.rs`；`processor.rs`；`os/src/arch/{rv64,loongarch64}/task/switch.S`。

【建议查看的 Git 历史】`dc793c4`、`17dcd4e`；查看修复 `sepc=0`/双 Running 窗口的提交差异。

【建议准备的图 / 表】context switch 栈和寄存器示意；旧 CPU—handoff—新 CPU 时序图。

【建议准备的测试 / 数据】SMP=1/2/4/8 的 sleep、CPU-bound、后台进程和 procfs 回归；保留 GDB 栈快照。

【容易出现的问题】先入 ready queue 再保存 context 会让另一 CPU 恢复未完成的 TCB；不能以单核测试覆盖该窗口。

## 5.3 idle、wakeup、affinity 与 IPI

【本节目的】说明无任务时 CPU 如何 idle，以及新任务如何被正确 CPU 唤醒。

【建议写什么】覆盖 per-CPU idle、WFI/timer、affinity-aware dequeue、owner 释放后的补 kick、RV64 software interrupt/IPI；区分“发布 ready”和“发出 kick”。

【建议检查的 RespOS 代码】`os/src/task/processor.rs`；`scheduler.rs`；`task.rs`；`os/src/arch/rv64/{smp.rs,trap/mod.rs,sbi.rs}`。

【建议查看的 Git 历史】`dc793c4`、`17dcd4e`、`f326ac8`；核对 affinity 与 owner handoff 的演进。

【建议准备的图 / 表】idle/wakeup/IPI 流程；CPU affinity 与 ready task 选择矩阵。

【建议准备的测试 / 数据】SMP=2/4/8、`sched_set/getaffinity`、后台 sleep、CPU 负载和 IPI 计数（若有）。

【容易出现的问题】只在 dequeue 过滤 affinity 会造成任务无人唤醒；IPI 过早于 context save 也会重现 owner 窗口。

## 5.4 futex、阻塞与超时的调度交互

【本节目的】说明 futex wait/wake/requeue、超时和信号中断如何与 blocked 状态竞争。

【建议写什么】写清用户页预检查在锁外、Pending/Woken/TimedOut/Interrupted 状态、single-winner、超时 timer 和唤醒路径；futex ABI 参数详见第 8 章。

【建议检查的 RespOS 代码】`os/src/task/futex/{mod.rs,queue.rs,wait.rs}`；`os/src/task/task.rs`；`os/src/arch/*/timer.rs`。

【建议查看的 Git 历史】`f35ff2d`、`57046f1`、`3aa1fb5`；查看 cmp-requeue 竞态和时钟语义收敛。

【建议准备的图 / 表】futex waiter 状态机；waiter 与 timer/wake/exit 的竞争时序。

【建议准备的测试 / 数据】`task_a_futex_exit_probe`、`race_probe`、`cmp_requeue_probe`；专项 yield build 与默认 build 分开记录。

【容易出现的问题】cmp-requeue 默认 probe 不是有效竞态门禁；不能在 futex 全局锁内触发用户缺页或分配。

