# RespOS 四天内核重构：A 组任务书（进程、同步、调度与时钟）

> 负责人：A  
> 工作分支建议：`refactor/task-runtime`  
> 总体方案：[四天内核重构总控与验收方案](./四天内核重构总控与验收方案.md)

## 1. 任务目标

A 负责进程生命周期、调度器、阻塞/唤醒、futex 和时钟语义，同时担任本轮集成负责人。

四天内不追求完整 Linux 线程组、CFS、优先级继承 futex 或完整 POSIX clock。目标是：

1. 消除退出、等待、阻塞和唤醒路径中的状态不一致；
2. 让普通退出与信号退出共用同一套资源清理逻辑；
3. 保证任务不会同时存在于 ready 和 blocked 队列；
4. 收紧 futex 的 wake、timeout、signal 竞争语义；
5. 让 monotonic、realtime 和 CPU time 的支持范围保持诚实；
6. 负责三条开发分支的集成，不让跨模块修改失控。

## 2. 文件边界

### 主要负责

- `os/src/task/task.rs`
- `os/src/task/scheduler.rs`
- `os/src/task/processor.rs`
- `os/src/task/futex/`
- `os/src/syscall/process.rs`
- `os/src/syscall/time.rs`
- 与上述功能直接相关的 signal/timer 小范围代码
- `os/src/syscall/mod.rs`，仅用于最终集成

### 不主动修改

- `os/src/mm/memory_set.rs` 的 VMA/COW 主体；
- `os/src/fs/` 的 VFS、page cache 和 ext4 主体；
- 网络协议栈；
- 架构页表实现；
- 大规模 syscall 编号或模块目录调整。

若任务退出需要新的 MM 或 FS 接口，应先写清接口和生命周期要求，请对应负责人实现。

## 3. 必须保持的不变量

### 3.1 进程生命周期

```text
Running
   ├─ thread exit ─────────────→ Detached/Released
   └─ process exit ────────────→ Zombie
                                     │
                                     └─ parent wait → Reaped
```

- exit 停止执行并释放运行资源，wait 才回收 Zombie 记录；
- 一个进程只能完成一次进程级资源清理；
- 普通退出和信号退出的资源清理范围必须一致；
- robust futex 和 `clear_child_tid` 必须在用户地址空间不可访问之前处理；
- `parent.children` 与 `child.parent` 必须双向一致；
- wait copyout 失败不能丢失 child；
- 线程不能被普通 wait 当成独立子进程；
- 任务退出后不能残留在 ready、blocked、futex 或 timer 队列。

### 3.2 调度器

- 一个 tid 不能同时出现在 ready 和 blocked 集合；
- ready queue、bitmap 和 `task_index` 必须一致；
- blocked task 被唤醒时至多入队一次；
- exited task 不能重新进入 ready queue；
- 调度属性变化后，ready task 必须重新进入正确队列；
- `SCHED_FIFO`、`SCHED_RR` 和普通任务的实际行为必须与文档一致。

### 3.3 futex

```text
检查用户值
  → 登记 waiter
  → 任务进入 blocked
  → wake / timeout / signal 三者竞争
  → 只能有一个完成原因
  → 清理 waiter、timer 和 blocked 状态
```

- wait 的“检查值并入队”之间不能丢失唤醒；
- 值不相等返回 `EAGAIN`；
- bitset 为零返回 `EINVAL`；
- wake 数量不能超过请求值；
- timeout、signal、wake 不能重复唤醒同一任务；
- requeue 后 waiter 的 key 和队列位置一致；
- 退出任务必须从 futex 队列删除。

### 3.4 时钟

```text
monotonic = 硬件启动时间
realtime  = monotonic + realtime_offset
CPU time  = 任务实际运行累计时间
```

- 设置 realtime 不能改变 monotonic；
- `clock_getres` 必须报告真实分辨率；
- 未实现的 clock 不用墙上时间冒充；
- relative sleep 和 absolute sleep 必须明确使用的时钟；
- timeout 到期只完成一次。

## 4. 第一天：进程、同步与时钟 ABI 审计

### 4.1 负责范围

A 对下面的 syscall 家族建立分级表和状态影响审计：

- clone、exec、exit、wait；
- signal；
- futex 和 robust list；
- scheduler、priority、affinity；
- rlimit 和 credential；
- POSIX timer、interval timer 和 clock；
- system 类全局状态接口。

每个 syscall 先标记 A/B/C/D，再按总控文档的风险评分选出 5～10 个深审。第一天不追求修完，而是确认支持边界和状态污染风险。

### 4.2 首批红名单

- [ ] `timer_create` 在 timer id copyout 前插入全局表，检查 EFAULT 泄漏；
- [ ] POSIX timer 是否在 owner exit 时清理；
- [ ] timer owner 只保存数字 tgid 时是否受 pid 复用影响；
- [ ] `timer_settime` 修改状态与 old value copyout 的顺序；
- [ ] `prlimit64` 修改 limit 与 old limit copyout 的顺序；
- [ ] `getppid` 等 ABI 路径中的可达 `unwrap/expect`；
- [ ] CPU clock、TAI、alarm clock 是否使用错误时间源冒充；
- [ ] `clock_getres` 是否与真实精度一致；
- [ ] `SCHED_FIFO` 与 `SCHED_RR` 是否有真实行为区别；
- [ ] affinity 是否只存储而不参与调度；
- [ ] `getrandom` 的安全承诺与当前随机源不一致；
- [ ] 普通退出和信号退出的共享资源清理不对称。

### 4.3 失败原子性测试

至少设计：

- [ ] `timer_create` 的 timerid 指针非法，进程 timer 数不增加；
- [ ] `timer_settime` 的 old value 指针非法，timer 状态符合明确约定；
- [ ] `prlimit64` 的 old limit 指针非法，limit 状态符合明确约定；
- [ ] wait status/rusage copyout 失败后 child 仍可 wait；
- [ ] signal info copyout 失败不丢 signal；
- [ ] owner exit 后全局 timer/waiter 不残留；
- [ ] pid/tid 复用后旧异步对象不作用于新任务。

### 4.4 对象身份规则

异步对象优先保存稳定对象身份：

```text
Weak<TaskControlBlock> / Arc-backed process object
```

不能只保存数字 pid/tgid 后在事件到期时重新查询，因为数字可能复用。若本轮无法调整对象模型，至少在退出路径显式清理并增加 pid 复用测试。

### 4.5 第一天交付

- [ ] A 负责范围的 syscall 分级表；
- [ ] 5～10 份高风险状态影响审计；
- [ ] 红名单的修复/受限/拒绝结论；
- [ ] timer、wait、rlimit 的失败注入计划；
- [ ] 与 B/C 确认跨模块接口；
- [ ] 不修改 syscall 行为来追逐单个 LTP 结果。

## 5. 第二天：生命周期和调度器底座

### 5.1 开工前

- [ ] 记录进程、pthread、futex、nanosleep 的当前结果；
- [ ] 画出 `clone → run → exit → wait` 调用链；
- [ ] 画出 `waiter enqueue → block → wake` 调用链；
- [ ] 搜索所有进程退出入口和资源回收入口；
- [ ] 确认 B、C 两组不会同时修改 `task.rs`。

### 5.2 统一进程组退出

当前应重点检查：

- `task_exit`
- `task_exit_by_signal`
- `task_group_exit`
- `task_group_exit_by_signal`
- `exit_thread`
- `exit_robust_list`
- `reparent_children_to_init`
- `notify_parent_exit`

已知高风险点：正常退出会避免回收仍被 `CLONE_VM` 共享的地址空间，而信号退出路径存在直接回收地址空间的非对称行为。

建议收敛为：

```rust
enum ExitCause {
    Code(i32),
    Signal(i32),
}

fn exit_process_group(task: Arc<TaskControlBlock>, cause: ExitCause)
```

统一函数至少按下面顺序组织：

1. 确定 leader 和线程组成员；
2. 阻止重复进程组退出；
3. 从调度器移除其他线程；
4. 处理线程私有退出动作；
5. 托管 children；
6. 处理 robust list 与 `clear_child_tid`；
7. 按共享关系清理地址空间和 fd；
8. 设置退出状态；
9. 通知 parent；
10. 清理残余 signal、timer、waiter 和任务索引。

限制：

- [ ] 本轮不改变“leader 退出会结束整个线程组”的项目策略；
- [ ] 不顺手实现完整 Linux de_thread；
- [ ] 不引入新的进程对象大重构；
- [ ] 不使用额外延时或 `yield` 掩盖退出竞态。

### 5.3 wait 回收顺序

检查 `sys_wait4` 和 `sys_waitid`：

```text
找到可等待 child
  → 保存 pid/status/rusage
  → copy_to_user
  → copy 全部成功
  → 从 parent 和全局结构回收
```

- [ ] `WNOHANG` 与 `ECHILD` 不混淆；
- [ ] copyout 失败时 child 仍可再次 wait；
- [ ] 重复 wait 返回正确错误；
- [ ] 任意 child 搜索不会被第一个未退出 child 阻断；
- [ ] stopped/continued 事件不会误删 Zombie。

### 5.4 调度器 invariant

增加仅在 debug 构建启用的检查函数，至少覆盖：

- [ ] bitmap 与实际非空队列一致；
- [ ] `task_index` 中每项能在对应 ready queue 找到；
- [ ] ready queue 中每个 tid 在 `task_index` 中恰好出现一次；
- [ ] `blocked_tasks` 与 `task_index` 没有交集；
- [ ] ready task 状态为 Ready；
- [ ] blocked task 状态为 Blocked；
- [ ] 不存在重复 tid。

在 `add/fetch/block/wake/remove/requeue` 后调用。若运行开销明显，只保留在 debug 模式。

### 5.5 第二天测试

- [ ] child 退出，parent 延迟 wait；
- [ ] 两个 child 反序退出并分别 wait；
- [ ] wait copyout 使用非法地址后，再用合法地址 wait；
- [ ] 线程退出不被 parent 当作 child；
- [ ] 进程收到终止信号后留下正确 wait status；
- [ ] 父进程先退出，child 被 init 接管；
- [ ] 反复 pthread create/join；
- [ ] 调度器 invariant 全程不触发。

### 5.6 第二天交付

- [ ] 普通退出和信号退出共用核心清理路径；
- [ ] wait 回收顺序得到测试保护；
- [ ] 调度器 invariant 可在 debug 模式运行；
- [ ] 提交按“退出统一”“wait 修复”“scheduler invariant”拆分。

## 6. 第三天：futex、阻塞和时钟

### 6.1 先记录锁顺序

重点对象：

- `FUTEX_QUEUES`
- `TIMED_FUTEX_WAITS`
- `SCHEDULER`
- task 内部状态锁
- `MemorySet` 锁

输出一张锁依赖表：

| 已持有 | 尝试获取 | 原因 | 是否允许 |
| --- | --- | --- | --- |
| FUTEX queue | MemorySet | 读取 futex 用户值 | 待审查 |
| FUTEX queue | Scheduler | block task | 待审查 |
| timer waits | Scheduler | timeout wake | 应避免嵌套 |

不得在没有说明竞态窗口的情况下简单缩小锁范围。futex 的值检查和 waiter 入队需要保持原子语义。

### 6.2 收敛 futex wait

普通 wait 和 timed wait 应尽量共享以下 helper：

- 参数与地址校验；
- key 生成；
- 在队列锁内重新读值；
- waiter 登记；
- blocked 状态准备；
- wake 后完成原因判断；
- waiter/timer 清理。

建议引入显式完成原因：

```rust
enum WaitCompletion {
    Woken,
    TimedOut,
    Interrupted,
}
```

不要求把所有等待设施泛化为公共 `WaitQueue<T>`，但不能继续让普通 wait、timed wait、bitset wait 各自维护近似而不同的清理流程。

### 6.3 futex 竞争测试

- [ ] 值不匹配立即 `EAGAIN`；
- [ ] 一个 waiter、一个 waker；
- [ ] 多 waiter，只 wake 指定数量；
- [ ] timeout 先于 wake；
- [ ] wake 先于 timeout；
- [ ] signal 打断 wait；
- [ ] signal 与 wake 相邻发生；
- [ ] bitset 只唤醒匹配 waiter；
- [ ] requeue 后从新 key 唤醒；
- [ ] waiter 线程退出后队列无残留；
- [ ] 连续运行 100 次无随机 hang。

### 6.4 时钟诚实化

第一优先级：

- [ ] `CLOCK_MONOTONIC` 使用不可回退的启动时间；
- [ ] `CLOCK_REALTIME` 使用 monotonic 加 offset；
- [ ] `settimeofday/clock_settime` 只修改 realtime；
- [ ] `clock_getres` 报告实际分辨率；
- [ ] nanosleep 的 relative timeout 使用 monotonic timeout clock。

第二优先级：

- [ ] 若能在上下文切换处低风险累计运行时间，则实现 thread/process CPU time；
- [ ] 若四天内无法可靠实现，CPU clock 返回明确错误，不使用墙上时间冒充；
- [ ] 没有明确语义的 TAI/alarm clock 降级为不支持。

限制：

- 不实现 NTP 调频；
- 不实现完整 suspend/boottime 区分；
- 不实现高精度 timer；
- 不为了测例声明 1ns 精度。

### 6.5 第三天交付

- [ ] futex 三方竞争路径有统一清理；
- [ ] 锁顺序文档完成；
- [ ] monotonic/realtime 行为明确；
- [ ] 不再返回虚假的 CPU time 或时钟精度；
- [ ] 与 B 负责人联合验证 futex user pointer；
- [ ] 与 C 负责人联合验证 pipe/poll 唤醒。

## 7. 第四天：集成、交叉审查与冻结

### 7.1 集成顺序

1. 合并 B 的内存分支；
2. 完成双架构 check 和内存 smoke；
3. 合并 C 的文件系统分支；
4. 完成双架构 check 和文件 smoke；
5. 合并 A 的 task-runtime 分支；
6. 统一 `syscall/mod.rs` import/dispatch；
7. 运行完整固定回归集合。

不得用一次“大冲突解决提交”混入新的语义修改。

### 7.2 交叉审查

A 负责审查 C 的：

- fd 生命周期；
- pipe/poll 阻塞；
- 文件 I/O 是否在持有不可睡眠锁时执行；
- page cache wake 是否可能重复。

B 将审查 A 的：

- futex 用户地址读取；
- task exit 与共享地址空间；
- copyout 失败路径。

对发现的问题只做 P0/P1 修复。第四天下午不再进行结构性重写。

### 7.3 调度与同步性能

记录至少 5 次中位数：

- getpid/syscall latency；
- yield/context switch；
- pthread create/join；
- futex uncontended/contended；
- sleep/wakeup。

若新 invariant 或诊断造成 release 性能退化，应确保它们只在 debug 构建启用。

## 8. Codex 任务拆分建议

不要向 Codex 提交“重构 task 模块”这种开放任务。拆成：

1. 生成 process/signal/time/scheduler syscall 分级表和高风险状态影响表；
2. 审查 POSIX timer 的 copyout 失败回滚、owner exit 清理和 PID 复用；
3. 审查 prlimit/timer/wait 的 prepare/commit 边界；
4. 审查并统一普通/信号进程组退出；
5. 为 Scheduler 增加 debug invariant；
6. 检查 wait4/waitid 的 copyout/回收顺序；
7. 画出 futex 锁依赖和竞态窗口，不先修改；
8. 收敛 futex waiter 清理；
9. 校正 clock_gettime/getres 的支持范围；
10. 添加对应用户态回归。

每个任务提示必须包含允许修改文件、禁止修改文件、不变量和测试。

## 9. 止损条件

出现以下情况立即停止当前改造并回退到最近可用提交：

- pthread/futex 出现无法稳定复现的随机 hang；
- 必须增加延时才能通过；
- 一个 tid 能重复进入 ready queue；
- 信号退出导致无关进程页错误；
- timer timeout 后任务被重复唤醒；
- 双架构行为明显分叉；
- 为统一代码而改变了未测试的 clone flags；
- 第四天下午仍需要重写核心状态机。

## 10. 最终验收清单

- [ ] 普通退出与信号退出资源语义一致；
- [ ] process/signal/time/scheduler syscall 已完成分级；
- [ ] POSIX timer 在 copyout 失败和 owner exit 时不泄漏；
- [ ] 异步对象不会因 pid/tgid 复用作用于错误任务；
- [ ] prlimit/timer/wait 的提交与 copyout 顺序有测试；
- [ ] wait copyout 失败不会丢 child；
- [ ] 调度器 invariant 在固定回归中不触发；
- [ ] futex wake/timeout/signal 不重复完成；
- [ ] 退出任务无 waiter/timer 残留；
- [ ] monotonic 不受 realtime 调整影响；
- [ ] 时钟精度和支持范围真实；
- [ ] RV/LA 双架构通过；
- [ ] 相关测试连续运行无随机 hang；
- [ ] A 的提交可按语义独立回退。
