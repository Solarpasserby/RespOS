# Phase 5 稳定进程身份与 de-thread 方案

## 状态与适用范围

- 状态：`待确认`，尚未进入内核实现
- 适用范围：线程组 leader 原始 `exit`、最后线程 zombie、non-leader `execve`、process-directed
  signal、wait/session/pgrp 与进程级资源回收
- 当前证据 commit：`75216ffc287908492e6daa8bfe455dc5aac3444a` 加 2026-08-14 当前工作树
- Linux 对照：`scripts/task_phase5_probe_linux.c` 三项全通过
- RespOS 反证：RV64/LA64、4 GiB/2 hart 的 `TASK_A_TASK_PHASE5_PROBE=1` 都稳定出现四个
  `TASK_PHASE5_EXPECTED_FAIL`；日志为 `/tmp/respos-{rv,la}-task-phase5-identity-gate.log`

本方案只定义 Phase 5 正确性状态所有权，不进入 Phase 6 的 scheduler/per-CPU 性能重构。

## 已确认问题

当前 `TaskControlBlock` 同时承担 thread 与 process 两种身份：

- `tid == tgid` 的 TCB 被当作 process leader 和进程查询入口；
- parent/children、waitable exit status、pgid/sid 与部分 signal lookup 直接保存或查找 leader TCB；
- `TASK_MANAGER` 只有 TID 索引，process-directed 路径常用 `TASK_MANAGER.get(tgid)`；
- `task_exit()` 把 leader 的原始 `SYS_exit` 直接升级为 group exit；
- `execve()` 显式拒绝 non-leader，以避免旧 leader TID/父子关系失效。

因此，单独删除上述两个分支会造成无法查询进程、父进程提前 wait、最后 worker 无法发布 zombie、
signal 找错目标以及 de-thread 后 `getpid() != gettid()`。保留一个已退出的 leader TCB 作为 tombstone
虽然能让现有 probe 更容易通过，但仍把进程身份绑定在线程对象上，不满足 M2 退出门槛，本方案不采用。

## 唯一目标模型

新增稳定的 `ProcessState`，每个进程一个，由同线程组所有 TCB 和父进程 children 表共同持有：

```text
ProcessTable[tgid] ── Arc<ProcessState>
                         ├── immutable tgid
                         ├── pgid / sid / credentials / process flags
                         ├── parent: Weak<ProcessState>
                         ├── children: Map<tgid, Arc<ProcessState>>
                         ├── members: Map<tid, Weak<TaskControlBlock>>
                         ├── lifecycle: Running | Exiting | Zombie | Reaped
                         ├── exit cause / wait event / child accounting
                         ├── process-directed pending signals
                         └── process-shared handler/timer/resource handles

TaskManager[tid] ── Arc<TaskControlBlock>
                         ├── current tid
                         ├── Arc<ProcessState>
                         ├── kernel stack / trap context / scheduler state
                         ├── thread signal mask + thread-directed pending
                         └── robust list / clear_child_tid / alt stack
```

`ProcessTable` 与 `TaskManager` 分工固定：前者按 PID/TGID 查进程，后者按 TID 查线程。进程对象在最后
线程退出后变为 Zombie 并继续存在，直到父进程成功 copyout wait 结果后提交 Reaped；不依赖任何已退出
TCB 存活。

第一阶段应迁移所有影响生命周期正确性的 process 字段：TGID、PGID、SID、parent/children、
exited-child notification、group-exit owner、exit cause/wait event、process-directed pending 和共享
handler/timer owner。credentials、limits、cwd/root/fd/mm 等已有共享 Arc 可以由 `ProcessState` 持有或
继续作为显式 handle，但不得再靠“找到 leader TCB”决定其生命周期。

## leader 原始 `SYS_exit`

提交顺序：

1. 清理当前 leader 的 robust list、`clear_child_tid`、thread waiter 和 scheduler/TID 索引；
2. 从 `ProcessState.members` 删除该 TID，但不修改 process TGID，不通知 parent，不发布 waitable exit；
3. 若仍有 member，销毁该 TCB 后继续运行 worker；
4. 若它恰为最后线程，则由同一 `finish_last_thread()` 路径把本次 exit cause 提交给 `ProcessState`，
   释放进程级资源并发布一次 Zombie/SIGCHLD。

最后线程可以不是原 leader。原始 `SYS_exit` 的退出码只有在它是最后线程时才成为进程退出码；否则最终
状态来自最后线程的原始 exit 或任意线程发起的 `exit_group`。

## non-leader `execve` / de-thread

ELF、argv/envp、用户栈和所有可能失败的权限/格式检查必须先完成，失败时不得杀 sibling 或更改 TID。
准备成功后，在 `ProcessState` 的 exec/group-exit 串行锁下提交：

1. 将 lifecycle 暂时置为 exec transition，阻止并发 clone、第二个 exec 与 group exit 抢同一提交；
2. 向所有 sibling（包括旧 leader）发布 termination，移出 ready/blocked queue，并等待 remote CPU
   post-switch acknowledgement；
3. 在旧地址空间仍有效时执行 sibling robust-list、`clear_child_tid` 和 futex waiter 清理；
4. 从 `TaskManager` 和 members 移除调用者旧 TID 及旧 leader TID；调用者接管数值 TGID，更新
   `TidHandle`、members 和 `TaskManager[tgid]`。内核栈 slot 不依赖 TID，可保持调用者原 slot；
5. 只保留调用线程的 thread-directed pending 和 signal mask；清除其他线程私有 pending/alt stack，
   process-directed pending 仍归 `ProcessState`；
6. 原子安装新 mm/trap context，unshare `CLONE_FILES` 后处理 CLOEXEC，重置用户 handler，最后发布
   Running 并唤醒 vfork parent。

任何可能失败的步骤必须放在第 2 步前；进入 sibling quiescence 后的路径只能成功提交或以明确 fatal
group exit 收口，不能返回到已经部分拆除的旧映像。

## `exit_group`、wait 与 signal

- `exit_group` 以 `ProcessState.lifecycle` CAS 取得唯一 teardown owner，终止并确认所有 member 后只提交
  一次进程级 exit cause；非 owner 等待 lifecycle 到 Zombie，不再等待某个 TCB 的 Exited 标志。
- `wait4/waitid` 扫描 parent `ProcessState.children`，从 child process 读取 PGID、wait event、exit cause
  和 rusage；用户 copyout 成功后才从 children/ProcessTable 移除，保持现有失败原子性。
- `kill(pid)`、process timer 与 process-directed queue 先查 `ProcessTable`，再从 members 中选择一个
  可递送线程；`tgkill` 仍同时校验 ProcessTable TGID 与 TaskManager TID。
- `setsid/setpgid/getsid/getpgid` 直接读写 `ProcessState`，不遍历所有 member 复制 PGID/SID。
- children reparent、SIGCHLD 和 vfork completion 都指向稳定 process identity；thread exit 不产生
  SIGCHLD，Zombie 只发布一次。

## 锁序与提交规则

建议固定锁序：

```text
ProcessTable -> parent ProcessState -> child ProcessState -> members snapshot
             -> scheduler/task removal -> per-thread cleanup -> mm/fd teardown
```

- 不在持有 members/children 锁时等待 remote CPU、fault 用户页或执行文件写回；先快照 Arc 后释放锁。
- lifecycle CAS 决定 group exit/exec 的 single winner；scheduler `terminate_requested + cpu_owner ack`
  继续作为 TCB quiescence 协议。
- wait 的用户 copyout 必须发生在 Reaped 提交前；exec 的所有可失败准备必须发生在 sibling teardown 前。
- ProcessTable 只在新进程身份发布、Zombie reaped 和失败回滚三个明确提交点修改。

## 实现拆分

1. 引入 `ProcessState/ProcessTable`，让 fork/clone、PID/session 查询、process signal lookup 双写并通过
   现有回归；此步不改变 exit/exec 行为。
2. 把 parent/children/wait/zombie 和 process exit cause 切到 ProcessState，完成 leader 原始 exit 与
   最后线程退出。
3. 实现 exec transition 与 non-leader TID 接管；删除 `execve` 的 leader-only 限制。
4. 迁移 process-directed pending、POSIX timer owner 和剩余 leader lookup；删除兼容双写和
   `process_leader()` fallback。
5. 在稳定身份上继续 `SA_NOCLDWAIT`、job control 和完整 session/tty，而不是再给 TCB 增加特殊情况。

每一步都应保持双架构可构建并有独立回滚边界，不能把全部字段一次性机械搬迁后才首次运行。

## 验证门禁

最低门禁：

- Linux/RespOS `task_phase5_probe` 四项全部 PASS，不得出现 expected-fail marker；
- leader 原始退出后父进程 `WNOHANG` 仍返回 0，TGID `kill(pid)` 可达，process-directed signal 由存活
  worker 接收，且 worker 观察 `getpid()==TGID`、`gettid()!=TGID`；
- RV64/LA64 2 hart 与至少一轮 8 hart leader-exit/nonleader-exec 压力；
- `TASK_A_WAIT4_PROBE`、`TASK_A_SIGNAL_PHASE5_PROBE`、session probe、futex exit/robust/clear-child-tid；
- exec 失败原子性、两个线程并发 exec、exec-vs-exit_group、leader-exit-vs-kill/wait 的 single-winner；
- 循环结束后 task、ready/blocked、fd、frame、futex waiter 和 POSIX timer owner 回到基线；
- Rust 1.86 RV64/LA64 顺序 release 构建。

完整初赛/LTP 和 job-control 仍是后续 M2 门禁，不能用三项 probe 代替。

## 需要确认的变更授权

该方案会改变 task/process 核心所有权、PID 查询、wait/signal 与 exec 提交协议，属于 Phase 5 必要但
高风险的架构修改。确认后按上述五个可回滚步骤实施；未确认前只保留 probe、设计和修复前证据，不用
leader tombstone 或 affinity 等兼容性特判绕过门禁。
