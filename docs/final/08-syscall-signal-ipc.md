# 8. System Call、Signal 与 IPC

## 8.1 syscall 分发与 Linux ABI 兼容边界

【本节目的】说明用户态请求如何进入领域模块、参数/用户指针/errno 如何处理。

【建议写什么】按 process/mm/fs/net/time/signal/special_fd/system 分类给接口地图；重点写参数校验、失败原子性、未实现状态诚实失败和架构 syscall ABI 差异。

【建议检查的 RespOS 代码】`os/src/syscall/{mod.rs,errno.rs,process.rs,mm.rs,fs.rs,net.rs,time.rs,signal.rs,ipc.rs,special_fd.rs}`；`user/src/syscall.rs`。

【建议查看的 Git 历史】`00c6822`、`15fe1a5`、`3aa1fb5`、`cba8e24`。

【建议准备的图 / 表】syscall 分类—实现者—失败 errno—验证 case 表；用户指针访问流程图。

【建议准备的测试 / 数据】LTP focused filter、basic、libc/CAgent 命令；记录首个真实失败而非汇总数量。

【容易出现的问题】不要把 Linux 习惯行为当成 RespOS 已实现事实；纯 no-op 与有长期状态影响的 syscall 必须区分。

## 8.2 signal 状态、投递与用户态返回

【本节目的】解释 signal pending、handler、siginfo、alt stack、signal frame 和 sigreturn 的完整链路。

【建议写什么】写 signal 由谁拥有、谁修改、同步/异步来源、阻塞/唤醒竞争、用户栈布局、恢复上下文和失败处理；标明 RV/LA trap context 差异。

【建议检查的 RespOS 代码】`os/src/signal/{mod.rs,sig_handler.rs,sig_info.rs,sig_stack.rs,sig_struct.rs}`；`os/src/syscall/signal.rs`；`os/src/arch/*/trap/`。

【建议查看的 Git 历史】`git log --all -- docs/signal-merge-review.md os/src/signal`；`signal-merge-review.md`；`b785262`（trap return 证据）。

【建议准备的图 / 表】signal 产生→挂起→投递→handler→sigreturn 流程；signal frame 字段表。

【建议准备的测试 / 数据】`sig_simple`、signal/futex/exit 竞争、双架构 trap return；保留非法地址和嵌套 handler 结果。

【容易出现的问题】不能只测 handler 被调用；trap return 的 SIE/stvec 顺序和 signal frame 提供的状态都要验证。

## 8.3 pipe/futex 之外的 IPC 与共享内存接口

【本节目的】界定 `syscall/ipc.rs` 实际提供的 IPC 能力，并避免把未实现的 System V IPC 写入正文。

【建议写什么】逐项盘点 `ipc.rs`、共享内存/mmap、futex、pipe 的真实接口；对初赛文档中出现但当前代码未确认的 System V IPC 标“待人工确认”。

【建议检查的 RespOS 代码】`os/src/syscall/ipc.rs`；`os/src/mm/memory_set.rs`；`os/src/task/futex/`；`os/src/fs/pipe.rs`；`user/src/bin/pipetest.rs`。

【建议查看的 Git 历史】`git log --all -- os/src/syscall/ipc.rs`；`15fe1a5`、`3aa1fb5`；与初赛 `main.typ` IPC 章节逐项对照。

【建议准备的图 / 表】已实现 IPC—共享对象—同步方式—测例表；实现/未实现边界表。

【建议准备的测试 / 数据】futex probes、shared-MM probe、pipe test、LTP IPC 子集（以实际清单为准）。

【容易出现的问题】初赛文档中的“System V IPC 机制”不自动等于当前实现；不要为了目录完整虚构功能。

## 8.4 时间、睡眠、timer 与中断唤醒

【本节目的】说明用户可见时间接口如何与 timer trap、调度阻塞、POSIX timer 和 signal 交互。

【建议写什么】覆盖时钟源、tick/重编程、nanosleep、interval/POSIX timer、timerfd（若当前实现确有）、超时与中断；说明 kernel timer safe point。

【建议检查的 RespOS 代码】`os/src/syscall/time.rs`；`os/src/arch/*/timer.rs`；`os/src/arch/*/trap/mod.rs`；`os/src/task/task.rs`。

【建议查看的 Git 历史】`163cba1`、`f35ff2d`、`57046f1`、`269a94a`。

【建议准备的图 / 表】用户 sleep→blocked→timer→wakeup 时序；timer source/precision/observable API 表。

【建议准备的测试 / 数据】`task_a_clock_probe`、sleep timeout、signal interrupt、SMP idle timer；记录误差和重复次数。

【容易出现的问题】中断中不能重入持有的 heap/高层锁；不要以墙钟耗时作为受 SCHED_IDLE 环境下的性能结论。

