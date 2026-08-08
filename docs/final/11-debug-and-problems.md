# 11. 关键 Bug、Debug 与问题解决过程

> 本章只选真实发生且有代码/日志证据的 3–6 个案例。每个案例按同一顺序写，未确认的根因必须保留“待验证”。

## 12.1 vfork 父任务登记晚于子任务发布导致 lost wakeup

【本节目的】展示一个只在 SMP 时序下暴露的阻塞/唤醒竞态。

【建议写什么】问题现象 → 最初判断 → 定位过程 → 真正根因 → 修复方案 → 为什么这样修复 → 测试与验证 → 工程经验；明确 parent blocked 必须先于 child ready。

【建议检查的 RespOS 代码】`os/src/syscall/process.rs::sys_clone`；`os/src/task/task.rs::execve`、`exit_process_group`。

【建议查看的 Git 历史】`cf30f64`；关联 `docs/codex/pitfalls.md` 的 vfork 记录。

【建议准备的图 / 表】错误/正确时序图；一次性 wake owner 表。

【建议准备的测试 / 数据】`clone(0x4111)` trace、CAgent/cargo/rustfmt、RV64 SMP=1 与 8 对照。

【容易出现的问题】单核通过会掩盖顺序 bug；普通 SIGCHLD 可能让错误表现看起来像正确唤醒。

## 12.2 先发布 Ready、后保存 context 导致双运行/`sepc=0`

【本节目的】展示 scheduler queue 原子化仍不足时的 context handoff 问题。

【建议写什么】按八项模板解释另一 CPU 恢复尚未保存 TCB 的路径、GDB 证据、handoff slot/owner fence 修复和回归边界。

【建议检查的 RespOS 代码】`os/src/task/processor.rs`；`scheduler.rs`；`os/src/arch/rv64/task/switch.S`。

【建议查看的 Git 历史】`dc793c4`、`17dcd4e`；`docs/codex/buildstorm-smp-plan.md` Phase 3 记录。

【建议准备的图 / 表】错误 context 保存窗口；修复前后提交顺序图。

【建议准备的测试 / 数据】GDB `sepc=0`/instruction fault、SMP 2/8、四路 sleep、procfs smoke。

【容易出现的问题】不能写成“加锁后解决”；关键是 context 保存完成和 owner 发布的顺序。

## 12.3 普通 spin lock 被同 CPU timer 中断重入导致死锁

【本节目的】展示跨 CPU 锁安全不等于中断重入安全。

【建议写什么】分别记录 heap 锁与 `ACTIVE_ITIMER_TASKS` 锁两个真实形态，说明 GDB 保留中断栈下方被打断栈、`IrqSafeHeap` 与 timer safe point 修复。

【建议检查的 RespOS 代码】`os/src/mm/heap_allocator.rs`；`os/src/arch/rv64/trap/mod.rs`；`os/src/task/task.rs`。

【建议查看的 Git 历史】`17dcd4e`；关联 `/tmp/respos-smp8-gdb-bt1.txt`、`smp2-dynamic-bt`。

【建议准备的图 / 表】syscall 临界区→timer trap→同锁重入图；锁/中断可达性表。

【建议准备的测试 / 数据】RV64 2/4/8 核退出压力、动态 GDB、修复后 3 轮结果。

【容易出现的问题】只看顶层等待栈会误判；不能把“换 allocator”作为未经所有权审计的方案。

## 12.4 `__restore` 过早开放用户 trap 导致递归 StorePageFault

【本节目的】展示架构返回路径中 SIE、stvec、sscratch 和寄存器恢复的时序 bug。

【建议写什么】记录 rustc 长时间运行时的 PC/scause 快照、最初判断、真正根因、清 SIE/延后 user vector 的修复和 postfix 证据。

【建议检查的 RespOS 代码】`os/src/arch/rv64/trap/{context.rs,trap.S,mod.rs}`；signal return 上下文构造。

【建议查看的 Git 历史】`f326ac8`、`b785262`；比较 `/tmp/respos-rustc-pc-sample*` 和 postfix。

【建议准备的图 / 表】旧/新 restore 指令顺序；CSR/sscratch 状态表。

【建议准备的测试 / 数据】8 核 BuildStorm 快照、非法指令 trace、256M/8 核 smoke、cargo rustc 推进证据。

【容易出现的问题】不能只修改 `TrapContext::init`；signal/exec 等路径也可能提供带 SIE 的上下文。

## 12.5 exec/exit 时远端 sibling 仍使用旧地址空间

【本节目的】展示共享 MemorySet、用户地址清理和远端 CPU owner 的生命周期问题。

【建议写什么】记录旧用户地址被新映像覆盖的风险、terminate mark/remove/spin-wait/ack/cleanup 协议、为何采用协作式而非主动 IPI、尚未完成的运行验证。

【建议检查的 RespOS 代码】`os/src/task/task.rs`；`processor.rs`；`syscall/process.rs`；`MemorySet` active mask。

【建议查看的 Git 历史】`b785262`；`current-status.md` 2026-08-07 条目。

【建议准备的图 / 表】错误 exec 顺序与四步 quiescence 对照；owner ack 时序。

【建议准备的测试 / 数据】quiescetrace、exec/exit 多线程、BuildStorm verbose；当前仅构建门禁通过的部分必须标待验证。

【容易出现的问题】不能把协议实现等同于协议已通过压力验证；不要把 trace 作为最终产物功能。

## 12.6 pipe/fd/wait 生命周期导致 BuildStorm 静默阻塞（根因待验证）

【本节目的】诚实记录当前最重要的未闭环问题，并展示如何避免过早下结论。

【建议写什么】问题现象（越过 toolchain 但无 minibuild 标记）→候选根因（pipe 引用、waiter、退出回收、sleep/wakeup）→已排除窗口→下一步实验；明确不能宣称某一候选为最终根因。

【建议检查的 RespOS 代码】`os/src/fs/{pipe.rs,poll.rs,fdtable.rs}`；`os/src/task/{task.rs,processor.rs}`；`syscall/process.rs`。

【建议查看的 Git 历史】`17dcd4e`、`f326ac8`、`b785262`；`current-status.md` 2026-08-06/07 条目。

【建议准备的图 / 表】cargo parent/child/rustfmt/pipe/waiter 关系图；已确认/未确认假设表。

【建议准备的测试 / 数据】保留 stdout/stderr 的 minibuild、`proctrace`/`pipelifetrace`/`quiescetrace`/`ldtrace`、单项低量复现；记录 QEMU 是否正常退出。

【容易出现的问题】不能把 pipe `Arc` 计数变化写成完整根因；不能以 QEMU 宿主终止后的日志作为通过。
