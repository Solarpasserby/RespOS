# LTP 性能优化方向审查

本文基于当前 RespOS 代码，并对照 `examples/rocketos` 的实现，整理跑 LTP 时更可能有效、且改动成本相对可控的优化方向。这里的“慢”主要指：

- 大量短进程/短线程测试整体耗时长；
- futex、pipe、wait、select/poll 等同步类测试吞吐低；
- cyclictest/hackbench 这类压力场景下调度响应差；
- QEMU 串口输出、路径查找、用户态拷贝等小开销叠加。

结论先行：RocketOS 里最值得当前 RespOS 借鉴的是两类基础设施：

- 调度器 ready queue 的 `task_index`，用来避免按 tid 移除任务时扫描所有优先级队列；
- `TimeManager` 用 `BTreeMap<deadline, timers>` 管理超时事件，避免 timer tick 全局扫任务/扫等待队列。

相反，RocketOS 的完整多核调度、CFS、部分 ppoll 轮询实现、日志较多的热路径都不适合作为当前低成本优化的直接模板。

## RocketOS 对照结论

| RocketOS 设计 | 当前 RespOS 是否适合借鉴 | 建议 |
| --- | --- | --- |
| `FIFOScheduler` 中的 `task_index: BTreeMap<Tid, (QueueType, usize)>` | 适合，且和 RespOS 现有 RT/normal/idle 队列很贴合 | 优先加入 ready queue 索引，降低 `remove/requeue` 成本 |
| `TimeManager` 中的 `BTreeMap<TimeSpec, Vec<AlarmEntry>>` | 适合，比每 tick 扫 Vec/扫 task 更自然 | 用 deadline bucket 统一 nanosleep、timed futex，后续接 timerfd/itimer |
| `FileOp::add_wait_queue/r_ready/w_ready` | 接口思想适合，但实现要分阶段 | 先让 pipe/eventfd/timerfd 支持 poll waiter，再考虑 socket |
| 每 hart 一个调度器、CPU 选择、负载迁移 | 当前不适合低成本引入 | RespOS 现在主要按 `-smp 1` 验证，先做好单核；以后 SMP 再参考 |
| CFS/vruntime/权重表 | 不建议现在引入 | 普通任务公平性可长期借鉴 nice weight，但不应阻塞眼前 LTP 优化 |
| `WaitQueue` 仍是 `VecDeque` 线性 remove | 不值得照搬 | RespOS 应直接做 tid map，比 RocketOS 这部分更进一步 |
| hot path 中较多 `log::info!/warn!` | 不适合 eval 性能场景 | 保留关键错误，release/eval 关掉普通路径日志 |

## 当前代码中的主要性能瓶颈和可行优化

### 1. ready queue 按 tid 移除仍会扫描所有队列

当前 RespOS 调度器已经从单 FIFO 队列演进成 RT/normal/idle 分层队列，这个方向是对的。但 `Scheduler::remove(tid)`、`requeue_ready_task()` 这类路径仍然需要扫描多个队列。LTP 的 `sched_*`、`futex`、`clone/fork/exit/wait` 很密集，任务数上来后 O(n) 移除会被放大。

RocketOS 的 `examples/rocketos/os/src/sched/fifo.rs` 对这里有一个很贴合的实现：每次 ready 入队时维护 `task_index`，记录 tid 当前所在的队列类型和队列下标；remove 时先查索引，只扫描一个具体 `VecDeque`。

对 RespOS 的建议实现：

```rust
enum ReadyQueueKind {
    Rt(usize),
    Normal(usize),
    Idle,
}

task_index: BTreeMap<usize, ReadyQueueKind>
```

需要注意几点：

- 记录的是“当前实际入队位置”，不是临时读取 `task.sched_policy()` 得到的位置。
- `add/add_front/fetch/remove/requeue/remove_thread_group` 都必须维护索引一致性。
- `fetch()` 从队列弹出任务后，也要从 `task_index` 删除。
- `requeue_ready_task()` 应按旧索引移除，再按新调度属性重新入队。
- RespOS 有独立 idle queue，不能完全照抄 RocketOS 把 idle 放进 RT 0 的做法。

收益：

- `remove(tid)` 从“扫描所有 RT + normal + idle 队列”变成“查索引 + 扫一个队列”；
- `requeue_ready_task()` 和调度属性变化路径收益明显；
- 比引入 CFS 或多核调度风险小得多。

风险：

- 索引一致性是安全重点，重复入队或 fetch 后索引残留都会造成偶发难查问题。
- 建议加 debug-only 校验函数：统计所有 ready queue 中 tid，和 `task_index` 双向比对。

### 2. blocked queue 按 tid 唤醒也应改成 map

RocketOS 的普通 `WaitQueue` 仍然是 `VecDeque`，按 tid remove 还是线性扫描；这一点不值得照搬。RespOS 这里可以直接做得更好。

当前 `wakeup_task(tid)` 需要在 `blocked_queue: VecDeque<Arc<TaskControlBlock>>` 中线性查找。futex、pipe、nanosleep、wait、signal 都会走按 tid 唤醒，LTP 同步类测试会频繁触发。

低成本优化建议：

- 把 `blocked_queue` 改成 `BTreeMap<Tid, Arc<TaskControlBlock>>` 或 `HashMap<Tid, Arc<TaskControlBlock>>`。
- `block_current_task()` 插入 tid。
- `wakeup_task(tid)` 直接 remove 对应 task。
- 如果需要保持调试输出顺序，优先用 `BTreeMap`；如果更看重平均性能，用 `HashMap`。

这个优化语义最简单，改动集中，建议和 ready queue `task_index` 分开提交，便于定位回归。

### 3. timer tick 不应每次扫描所有等待项/所有任务

当前 `check_all_task_timers()` 的主要问题是每个 timer interrupt 都可能执行：

- `check_futex_timeouts()` 扫 timed futex wait；
- `check_nanosleep_timeouts()` 扫 nanosleep wait；
- `TASK_MANAGER.for_each(...)` 扫所有 task 检查 real timer / posix timer。

这会让 tick 路径复杂度接近：

```text
O(timed futex waits) + O(nanosleep waits) + O(all tasks)
```

RocketOS 的 `TimeManager` 更适合当前 RespOS 借鉴：用 `BTreeMap<deadline, Vec<entry>>` 按 deadline 分桶，tick 时只取 `deadline <= now` 的 bucket，并且先从 map 中移除，再在锁外执行回调。

对 RespOS 更合适的实现方式：

```rust
enum TimerAction {
    WakeTask { tid: Tid },
    FutexTimeout { tid: Tid, uaddr: usize },
    TimerFdExpire { fd_ref: ... },
    ItimerSignal { tid: Tid, which: usize },
    PosixTimerSignal { tid: Tid, timer_id: usize },
}

struct TimerEntry {
    tid: Tid,
    seq: u64,
    action: TimerAction,
}

struct DeadlineTimerManager {
    timers: BTreeMap<TimeSpec, Vec<TimerEntry>>,
    by_task: BTreeMap<(Tid, TimerKind), (TimeSpec, u64)>,
}
```

为什么建议用 enum action，而不是直接照抄 RocketOS 的 boxed callback：

- RespOS 是内核代码，enum 更容易审计、调试和避免闭包捕获生命周期问题；
- 取消/更新 timer 时可以用 `(Tid, TimerKind) -> (deadline, seq)` 做惰性删除；
- tick 路径只处理已到期 bucket，不再扫描全部等待项。

落地顺序建议：

1. 第一阶段只迁移 nanosleep 和 timed futex。
   - 这两个是 LTP 高频路径，收益直接。
   - timerfd/itimer/posix timer 先保留原实现，避免一次改太大。

2. 第二阶段把 timerfd 接到统一 deadline manager。
   - timerfd 到期时更新 counter，并唤醒读/poll waiter。

3. 第三阶段再接 itimer/posix timer active list。
   - 目标是去掉 `TASK_MANAGER.for_each` 每 tick 扫所有 task。

如果暂时不做完整 `TimeManager`，也可以先加一个最小优化：为 nanosleep/futex 各维护 earliest deadline，`now < earliest` 时直接跳过 Vec 扫描。但这只是过渡方案，长期还是 deadline bucket 更干净。

### 4. poll/select 要逐步从轮询 yield 变成事件唤醒

当前 `ppoll/pselect6` 的常见模式是：

1. 扫 fd readiness；
2. 没有 ready 就 `yield_current_task()`；
3. 再扫描。

这会制造很多 runnable task，调度器负担变重，同步类 LTP 也容易慢。

RocketOS 的 `FileOp` trait 有 `add_wait_queue/r_ready/w_ready/hang_up` 这类接口。它的活跃 ppoll 实现并不完全理想，仍有轮询和日志问题；但“文件对象自己暴露 readiness 与 wait hook”这个接口方向值得借。

对 RespOS 的阶段性设计：

```rust
trait FileOp {
    fn read_ready(&self) -> bool;
    fn write_ready(&self) -> bool;

    // 可以先不做成通用复杂 poll table，低成本版本足够覆盖 pipe/eventfd/timerfd。
    fn add_poll_waiter(&self, tid: Tid, events: PollEvents) -> Result<(), Errno>;
    fn remove_poll_waiter(&self, tid: Tid);
}
```

优先级：

1. pipe
   - pipe-heavy 的 select/poll 测试很多。
   - pipe 内部已有 read/write syscall waiter，可额外维护 read-poll/write-poll waiter。
   - write 后唤醒 read poller；read/drop 后唤醒 write poller 和 hangup poller。

2. eventfd
   - counter 从 0 变非 0 时唤醒读 waiter；
   - counter 从满变可写时唤醒写 waiter。

3. timerfd
   - 到期时 counter 增加并唤醒读 waiter。

4. socket
   - 后置。网络栈状态更多，先不要把 poll 改动和 socket 正确性绑在一起。

超时处理可以直接复用统一 `TimeManager`：ppoll/pselect 没有 fd ready 且 timeout 非 0 时，把当前任务挂到相关 file wait list，同时注册一个 poll timeout timer；任一事件发生后唤醒并清理 wait 注册。

### 5. wakeup boost 可以保留，但要比 child-runs-first 更克制

当前 `wakeup_task(tid)` 将阻塞任务设为 ready 后直接放回同优先级队列尾部。对纯吞吐任务这没问题，但同步密集场景里，被 futex/pipe/wait 唤醒的任务通常在临界路径上，长期放队尾会拖慢 handshake。

建议保留一个克制版 wakeup boost：

- 只对 `Blocked -> Ready` 的任务同优先级队首插入；
- 不对所有 fork/clone 做 child-runs-first；
- RT 队列是否 boost 要谨慎，可以先只对 normal queue 做；
- 若担心饥饿，可给 task 加一个短期 boost 次数限制。

这个方向比“为了 cyclictest 调整 testrunner 参数”或“无条件让 child 抢跑”更正当，也更适合解释成内核调度策略优化。

### 6. 用户态拷贝有重复页表遍历空间

`copy_from_user/copy_to_user` 当前为了稳妥，通常会先检查用户区间，再逐页翻译复制。LTP 中 `stat/fstat/gettimeofday/clock_gettime/futex/ioctl/read/write/getdents` 都会频繁小对象 copy，重复遍历页表会累积。

低成本优化：

- 对完全落在单个用户页内的小对象 copy 做 fast path；
- 在逐页 copy 时合并权限检查和复制，避免先 `check_user_*` 再翻译一次；
- `pollfd/fd_set/iovec` 这类数组尽量一次 copy 到内核缓冲区处理。

这里不能牺牲权限校验。建议先加单页 fast path，保持错误码和 fault 行为一致。

### 7. VFS、procfs、路径解析：优化热路径，不要过早大改

当前已经有全局 dentry cache，整体方向可用。RocketOS 的 dentry/文件系统实现也有 child map 和全局 cache，但并不意味着要照搬整个 FS 层。

更适合 RespOS 的低成本方向：

- 对 `/dev/null`、`/dev/zero`、`/dev/cpu_dma_latency`、`/proc/self/*` 这类 LTP 高频路径做更短路径；
- `Nameidata` 解析时减少每个 segment 的 `String` 分配，能用 `&str` slice 就不要急着分配；
- `/proc/<pid>` lookup 继续走 task map，避免为了 lookup 触发 `/proc` 根目录枚举；
- 给 dentry cache 加命中/容量统计，再决定是否调大容量或优化淘汰。

不建议现在大规模重写 VFS，因为 LTP 失败/变慢更多集中在同步、调度、timer，而不是单纯 path lookup。

### 8. 日志输出会放大 QEMU 性能问题

QEMU `-nographic` 下串口输出非常慢。RocketOS 里一些 hot path 有较多 `log::info!/warn!`，这点不能照搬。RespOS eval/release 下也应保持热路径安静。

建议：

- 启动阶段的 app 列表、timer 频率、superblock 信息只在 debug 或 verbose feature 下输出；
- syscall 热路径禁止普通 `println!/info!/warn!`；
- futex、signal、poll、timer 这类高频路径保留 feature-gated trace；
- panic、严重错误、不可恢复异常仍正常输出。

这项成本很低，且对 QEMU 场景收益经常很明显。

### 9. wait/waitpid 可以长期做 wait-ready queue

`sys_wait4` 需要扫描 children 判断 pid/pgid、exit/stop/continue 事件。语义复杂，完整优化要小心。

低成本版本可以先只做 exited child：

- 子进程 exit 时把 tid 挂到 parent 的 exited-ready 队列；
- `wait4` 先消费 ready 队列；
- 涉及 `WUNTRACED/WCONTINUED/WNOWAIT` 的复杂语义仍保留扫描兜底。

这不是第一优先级，但对 shell/LTP 大量 fork/wait 有潜在收益。

## 不建议当前照搬 RocketOS 的部分

1. 不建议现在引入 per-hart scheduler。
   - 当前主要验证还是单核 QEMU，单核调度器热点更直接。
   - 后续如果切 SMP，再参考 RocketOS 的每 hart ready queue、`cpu_id`、负载迁移。

2. 不建议现在引入完整 CFS。
   - CFS 会牵涉 vruntime、权重、时间片、唤醒公平性，验证成本高。
   - 当前 RespOS 已有 RT/normal/idle 分层，先把 remove、wakeup、timeout 做快更划算。

3. 不建议照搬 RocketOS 的 WaitQueue。
   - 它仍是线性队列，不能解决 RespOS 当前按 tid 唤醒的核心问题。

4. 不建议照搬日志密集实现。
   - LTP/QEMU 性能场景下，串口输出本身就是瓶颈。

5. 不建议在 testrunner 或单个测例里降压力参数。
   - 这会掩盖内核问题，不利于后续 LTP 回归。

## 推荐落地顺序

### 第一批：低风险、性价比高

1. ready queue 增加 `task_index`。
   - 目标：`remove/requeue` 不再扫描所有优先级队列。
   - 参考：RocketOS `FIFOScheduler::task_index`。
   - 验证：`sched_*`、futex、cyclictest、hackbench。

2. `blocked_queue` 改成 tid map。
   - 目标：`wakeup_task(tid)` 从线性查找变成按 key 删除。
   - 验证：futex、pipe、wait、nanosleep。

3. eval/release quiet log。
   - 目标：降低 QEMU 串口输出瓶颈。
   - 验证：启动日志减少，panic/关键错误仍可见。

### 第二批：中等改动、收益稳定

1. deadline bucket `TimeManager`。
   - 先接 nanosleep + timed futex。
   - 后续接 timerfd/itimer/posix timer。

2. poll/select 事件化等待。
   - 先支持 pipe/eventfd/timerfd。
   - socket 后置。

3. wakeup boost 策略收敛。
   - 只对 Blocked -> Ready 做同优先级队首插入。
   - 必须观察是否造成普通 CPU task 饥饿。

4. copy_from_user/copy_to_user 单页 fast path。
   - 高频小结构体 syscall 会受益。

### 第三批：长期完善

1. wait-ready queue，减少 wait4/waitid 扫描。
2. VFS path segment 零分配解析。
3. `/proc/self`、`/dev/*`、常见 sysctl 文件快速路径。
4. 普通任务更完整的公平性策略。
   - 可借鉴 RocketOS 的 nice weight/CFS 思路，但建议等前两批稳定后再做。
5. SMP 后再考虑 per-hart scheduler。
   - 每 hart ready queue、task cpu affinity、轻量负载均衡都属于后续阶段。

## 建议的验证指标

每做一项优化，建议固定跑以下几组：

1. 编译与静态检查：
   - `cargo fmt --check`
   - `cargo check`
   - `git diff --check`

2. 基础启动：
   - `make rv`
   - `make la`

3. LTP 子集：
   - futex：`futex_wait01`、`futex_wait_bitset01`、`futex_wake01`
   - pipe/select/poll：`pipe*`、`select*`、`poll*`、`pselect*`
   - process：`fork*`、`clone*`、`wait*`
   - sched：`sched_yield01`、`sched_setscheduler*`、`sched_rr_get_interval*`
   - timer：`nanosleep*`、`clock_nanosleep*`、`timerfd*`

4. 观察指标：
   - LTP 子集总耗时；
   - QEMU 输出量；
   - cyclictest P8 中每个线程的 `C` 是否非 0；
   - 是否出现长时间无输出但未退出；
   - 是否出现 `EINTR`、timeout、wait/futex 偶发失败。

## 最值得马上尝试的具体 patch

如果只选两个最小优化，我建议优先做：

```text
Scheduler ready queue: 增加 tid -> queue kind/index 的 task_index
Scheduler blocked_queue: VecDeque -> BTreeMap/HashMap<Tid, Arc<TaskControlBlock>>
```

原因：

- 都集中在调度/阻塞基础设施；
- 不需要改 syscall 语义；
- 影响面覆盖 futex、pipe、nanosleep、wait、sched；
- 和 RocketOS 中已经验证过的 ready queue 索引思路一致，但 blocked queue 可以比 RocketOS 做得更好。

第三个建议是：

```text
新增 DeadlineTimerManager，用 BTreeMap deadline bucket 承接 nanosleep + timed futex
```

这个对压力场景下的调度延迟很关键，但涉及 timeout 取消、提前唤醒、EINTR、futex wake 竞争，建议单独提交并重点回归。

## 小结

RespOS 跑 LTP 偏慢，主要不是单个测例参数问题，而是同步等待和调度基础设施还比较朴素：

- ready/remove 和 blocked/wakeup 存在线性扫描；
- timeout/timer 每 tick 扫描范围过大；
- poll/select 仍偏轮询；
- 用户拷贝和路径解析有重复小开销；
- QEMU 串口日志容易放大测试耗时。

参考 RocketOS 后，当前最适合 RespOS 的路线不是引入大而全的新调度器，而是先补三个基础件：ready queue 索引、blocked tid map、deadline timer manager。它们改动相对可控，收益覆盖 LTP 高频路径，也更容易做成干净、可解释的内核优化。
