# RespOS 四天内核重构：A 组执行记录

> 分支：`refactor/task-runtime`  
> 本文只记录 A 组基线、ABI 审计、测试和遗留风险，不替代任务书。

## 1. Baseline

| 项目 | 结果 |
| --- | --- |
| Baseline commit | `3839e4e5b7fd8410b727493d48fe0646240f166d` |
| Rust | `rustc 1.89.0-nightly (60dabef95 2025-05-19)` |
| Cargo | `cargo 1.89.0-nightly (47c911e9e 2025-05-14)` |
| QEMU RISC-V | `10.0.2` |
| QEMU LoongArch | `10.0.2` |
| MEM | `256M` |
| SMP | `1` |
| build mode | `release` |

### 1.1 双架构构建

| 检查 | 结果 | 备注 |
| --- | --- | --- |
| RV `cargo check` | PASS | 依赖中有既有 target-feature warning |
| LA `cargo check` | PASS | 无新增错误 |
| `make build-rv` | PASS | 生成 `kernel-rv` |
| `make build-la` | PASS after retry | 首次冷构建时 lwext4 缺少刚生成的 `generated/ext4_config.h`，重试通过 |

LA 首次失败发生在任何 A 组源码修改之前，归类为 baseline 构建顺序/并行问题。后续验收必须从干净构建和增量构建两个角度复核。

### 1.2 运行基线

RV 和 LA 均能启动到 `[testrunner] start`，未观察到启动 panic。启动使用 QEMU
snapshot 模式，未写回测试镜像。

当前 `img/sdcard-rv.img` 和 `img/sdcard-la.img` 中缺少 testrunner 引用的
basic/LTP 程序与脚本，大量用例以 `ENOENT` 结束，因此现有镜像只能作为启动
smoke，不能作为功能 baseline。需要补充完整比赛镜像或把 A 组小回归直接嵌入
用户程序集合。

以下项目尚待固定成独立小回归，不能用“完整 LTP 是否启动”代替：

- fork/exit/wait；
- exec 成功与失败；
- pthread create/join；
- futex wait/wake；
- nanosleep；
- signal interrupt。

记录格式固定为 `PASS`、`FAIL(errno/output)`、`PANIC(location)`、`HANG(last event)` 或 `FLAKY(x/100)`。

## 2. ABI 分级台账（初审）

等级为当前代码的保守初审结论；只有完成失败路径和生命周期回归后才能提升为 A。

| syscall 家族 | 当前等级 | 当前支持边界/主要问题 | 下一步 |
| --- | --- | --- | --- |
| `clone/exec/exit/exit_group` | B | 已统一普通/信号退出并增加单次提交保护；项目仍采用 leader exit 结束线程组的简化策略 | 补 CLONE_VM/FILES 与并发 exit 回归 |
| `wait4/waitid` | A | status/rusage copyout 位于 accounting/reap 前；非法指针后重试已双架构压力验收 | 保持并发 wait 边界 |
| `kill/tkill/tgkill/rt_sig*` | B | 有基础 signal 支持；copyout 失败、阻塞竞争和退出清理未完整验证 | 深审 `sigtimedwait` 和 signal info |
| `futex/robust_list` | B | wake/timeout/signal、线程退出清理和 CMP_REQUEUE 原子窗口已动态验收；真正 SMP 待处理 | 完整镜像上连续竞争测试 |
| `sched_*` | B | 参数和属性接口存在；FIFO/RR、affinity 是否真实影响调度需验证 | 对照 scheduler 状态机 |
| `get/setpriority` | B | 基础优先级状态存在 | 验证 ready task 重排 |
| `prlimit/getrlimit/setrlimit` | B | 校验、old-limit copyout、状态提交及非法指针重试已验收；真正并发修改未测 | 保持单核受限支持 |
| credential/capability | B | uid/gid/cap 接口存在，完整权限模型不在本轮范围 | 明确受限支持边界 |
| `timer_create/delete/get/settime` | B | create/settime 失败原子性和 owner exit 已验收；Weak owner 防 PID 误投递，当前分配器不复用 PID | PID 回收启用后补动态复用回归 |
| `get/setitimer` | B | task 内存在基础状态 | 验证 owner exit 和 signal 投递 |
| `clock_gettime/getres` | B | fine/coarse 分辨率已直读验收；CPU/TAI/alarm 明确拒绝，不伪造时间 | 保持受限支持边界 |
| `clock_settime/settimeofday` | B | realtime offset 已验证不影响 monotonic | 保持 CAP_SYS_TIME 与 clock-id 边界 |
| `nanosleep/clock_nanosleep` | B | 有 timeout wait 表 | 验证 clock、signal 和唯一完成 |
| `times` | D | user/system CPU tick 使用同一近似值 | 实现真实记账或明确受限 |
| `adjtimex/clock_adjtime` | B | 仅为简化状态模型 | 明确不支持的 modes |
| `getrandom` | B | flags 有校验，但随机源安全承诺需单独审计 | 不宣称密码学安全 |
| `uname/sysinfo/syslog/reboot` | B | 基础 system 接口 | 审计全局状态和权限边界 |
| hostname/domainname/personality | B | 简化全局/进程语义 | flags、权限和持久性测试 |

## 3. 首批高风险状态影响审计

### 3.1 `timer_create`

- 当前等级：D。
- 输入：`clock_id`、可选 `sigevent`、`timerid` 输出指针。
- 读取对象：当前 task、时钟支持表。
- 修改对象：全局 `POSIX_TIMERS`、全局 ID 计数器。
- 共享边界：global。
- 第一次状态修改：分配 ID，随后插入 `POSIX_TIMERS`。
- 当前提交点：全局表插入。
- copyout：提交点之后才写 `timerid`。
- copyout 失败状态：timer 留在全局表，用户不知道 ID，形成不可删除泄漏。
- owner：只保存数字 `tgid`。
- exit：未发现 POSIX timer 清理入口。
- PID 复用：旧 timer 到期后会按数字查询 `TASK_MANAGER`，可能命中新任务。
- 结论：P0/P1，必须修复。
- 当前处理：已改为 timer ID copyout 成功后才插入全局表；进程组普通/信号
  退出均按 owner tgid 清理 POSIX timer；到期投递使用
  `Weak<TaskControlBlock>` 稳定 owner 身份，避免数字 tgid 复用误投递。
- 最小回归：非法 `timerid` 返回 `EFAULT`，随后可观测 timer 数不增加；owner exit 后 timer 数恢复；PID 复用后新任务不收旧 signal。

### 3.2 `timer_settime`

- 当前等级：B。
- 修改对象：`POSIX_TIMERS[timerid]`。
- 第一次状态修改：更新 deadline 和 interval。
- copyout：状态修改完成后才复制 old value。
- copyout 失败状态：新配置已经生效。
- 并发问题：若改成锁外先 copyout、再按数字 ID 查表，需要定义与并发 delete/settime 的竞争语义。
- 结论：P1，需明确 prepare/commit 或用稳定的 timer 对象引用。
- 当前处理：已改为快照、old-value copyout、重新确认对象身份、提交；并发
  delete 不会向失效对象提交，同一 timer 的真正 SMP 并发 set 仍需 generation。

### 3.3 `prlimit64`

- 当前等级：B。
- 修改对象：目标进程 rlimit。
- 第一次状态修改：`set_rlimit`。
- copyout：修改后才输出 old limit。
- copyout 失败状态：limit 已改变，调用者收到 `EFAULT`。
- 结论：P1，应在提交前输出 old limit，或明确并测试 Linux 失败语义。
- 当前处理：已改为 copyin/校验、old-limit copyout、状态提交；非法 old 指针
  不再改变 limit，动态失败注入待执行。

### 3.4 `wait4`

- 当前等级：B。
- 修改对象：child 记录、退出事件、父进程 child CPU ticks。
- status copyout：在回收前完成，失败不会立即删除 child。
- rusage copyout：回收前完成，但 child ticks 已先累计到 parent。
- copyout 失败状态：child 尚可再次 wait，但重试会重复累计 child ticks。
- 结论：P1，需要把所有输出准备和 copyout 放在 child ticks/回收提交之前。
- 当前处理：已把 status/rusage copyout 放在 child accounting 和 Zombie
  回收之前，并移除并发回收路径中的 `unwrap`。

### 3.5 进程组退出

- 当前等级：B。
- 普通退出：仅当 `memory_set` 强引用计数为 1 时回收数据页。
- 信号退出：无条件调用 `recycle_data_pages()`。
- 风险：`CLONE_VM` 或其他共享 mm owner 仍存活时，信号退出可能破坏共享地址空间。
- 两条路径均未发现 POSIX timer owner 清理。
- 结论：P0，普通退出与信号退出必须收敛到同一核心清理路径。
- 当前处理：已统一为 `exit_process_group(ExitCause)`，按线程组 owner 数量处理
  共享 mm/fd，加入单次提交门，并统一 timer/waiter/线程私有清理。

### 3.6 clock family

- 当前等级：D/B 混合。
- monotonic/realtime：已有启动时间和 realtime offset 的基本区分。
- CPU clocks：当前直接返回启动时间，并非实际 CPU time。
- TAI/alarm/boottime：多个 clock id 共用同一时间源，支持边界不真实。
- `clock_getres`：固定返回 1ns，而实现主要使用毫秒时间。
- 结论：P1，保留可信子集，其余明确拒绝；不可用假时间源换取测试通过。

## 4. 首批 P0/P1

| 优先级 | 问题 | 处理方向 |
| --- | --- | --- |
| P0 | 信号退出无条件回收可能共享的 mm | 已修：统一退出核心路径；待 CLONE_VM 动态回归 |
| P1 | `timer_create` copyout 失败泄漏 | 已修：copyout 成功后再提交对象；待运行回归 |
| P1 | timer owner exit 不清理且受 PID 复用影响 | 已修：exit 清理并使用 Weak 稳定 owner；待运行回归 |
| P1 | `wait4` rusage copyout 失败重复累计 child ticks | 已修：延后 accounting/reap；待失败注入 |
| P1 | `timer_settime` old value copyout 失败但新状态已生效 | 已修：prepare/copyout/commit；待失败注入 |
| P1 | `prlimit64` old value copyout 失败但 limit 已改变 | 已修：prepare/copyout/commit；待失败注入 |
| P1 | CPU/TAI/alarm clock 假实现及 1ns 假精度 | 已修：明确拒绝未实现时钟并报告实际精度 |

## 5. 跨模块接口需求

- B/MM：共同确认 futex 用户值读取、shared futex key 和退出时共享 mm 生命周期。
- C/FS：共同确认进程组最后 owner 退出时 fd table 的清理边界，以及 pipe/poll 唤醒的唯一完成语义。

## 6. 待执行测试

- `timer_create_bad_timerid`：非法输出指针不新增 timer；
- `timer_owner_exit`：owner exit 后无 timer；
- `timer_pid_reuse`：旧 timer 不向复用 PID 投递；
- `timer_settime_bad_old`：固定 copyout 失败后的状态；
- `prlimit_bad_old`：固定 copyout 失败后的 limit；
- `wait4_bad_status_retry`：失败后仍能 wait；
- `wait4_bad_rusage_retry`：失败后仍能 wait，child ticks 只累计一次；
- `clock_realtime_does_not_change_monotonic`；
- futex wake/timeout/signal 竞争连续 100 次。

## 7. 2026-07-26 提交前审查

### 7.1 对照范围

- 总控：6.3 ABI 统一规则、7.1 任务不变量、8.2/8.3 双架构与固定 smoke、
  15 Codex 工作顺序、17 最终审查；
- A 任务书：3.1～3.4 不变量、5.2～5.4 生命周期/调度器、6.1～6.3
  futex 锁序与竞争。

### 7.2 审查发现与处理

| 发现 | 方案影响 | 处理 |
| --- | --- | --- |
| timed futex 在登记前到达的 signal 可能未触发 wake | 违反 signal/wake 唯一完成与 signal interrupt | 入队后、block 前及刚 block 后复查 signal |
| blocked 后的立即中断分支持 futex queue 锁调用 scheduler | 锁依赖不够收敛 | 完成 waiter 清理后显式释放 queue 锁再 wake |
| group exit 的 leader 未走 `clear_child_tid` | 违反 mm 失效前线程私有清理 | 提取 `cleanup_exiting_thread`，leader/non-leader 共用 |
| 缺少进程组退出单次提交状态 | 可能重复资源清理和 parent 通知 | 增加线程组共享 `group_exiting` 原子门 |
| POSIX timer 到期事件仅携带数字 tgid | PID 复用窗口可误投递 | timer 保存 `Weak<TaskControlBlock>`，投递前升级并检查未退出 |

### 7.3 复核结果

| 检查 | 结果 |
| --- | --- |
| RV `cargo check` | PASS |
| LA `cargo check` | PASS |
| RV debug build | PASS |
| LA debug build | PASS |
| LA debug snapshot | 两轮完成，正常关机，无 panic/invariant |
| RV debug snapshot | 第一轮完成，第二轮运行至第 533 项，无 panic/invariant |
| RV release build + snapshot | PASS，两轮完成并正常关机 |
| LA release build + snapshot | PASS，两轮完成并正常关机 |
| release invariant 字符串检查 | PASS，不含 debug invariant 诊断 |
| `rustfmt --check` | PASS |
| `git diff --check` | PASS |

当前镜像的 basic/LTP 可执行文件不完整，绝大多数条目以 `ENOENT` 结束。以上
只能作为构建、启动、退出循环和不变量 smoke；wait/timer/futex/clock 的功能与
100 次竞态验收仍保持未完成状态。

## 8. 2026-07-28 完整初赛镜像专项回归

### 8.1 测试介质与入口

- 保留原决赛镜像 `img/sdcard-rv.img`、`img/sdcard-la.img`（各 128 MiB），
  未覆盖；
- 解压 2025 初赛官方镜像为 `img/sdcard-rv-full.img` 和
  `img/sdcard-la-full.img`（各 4 GiB），两者均为有效 ext4，包含 musl/glibc
  basic 和完整 LTP；
- testrunner 增加仅在构建时设置 `TASK_A_LTP_ONLY=1` 才启用的任务 A
  LTP-only 入口，并沿用 `LTP_CASE_FILTER` 选择固定用例；默认构建行为不变。

### 8.2 发现的问题

RV 的以下连续序列在修复前 10 次冷启动中第 5、8 次稳定暴露问题：

```text
futex_cmp_requeue02 → futex_wait01 → futex_wait02
```

失败仅出现在 RV-musl `futex_wait02`：父进程进入 `futex_wait` 时，子进程正在
短暂 nanosleep，就绪队列为空；`prepare_current_task_blocked()` 因此拒绝阻塞，
把“暂时没有 ready task”误判成不可等待并返回 `EAGAIN`。跟踪确认 futex 用户值
与 key 均正确，随后子进程正常醒来但 `futex_wake` 只能得到 0。

### 8.3 修复

- 允许当前任务在就绪队列暂时为空时进入 Blocked；只有不存在 current task
  才返回失败；
- `switch_to_next_task()` 在无 ready task 时保留可返回的调度上下文，持续检查
  timer，直到某个任务真正 ready，而不是进入架构层永不返回的 idle；
- timer 中断命中上述等待窗口时，`preempt_current_task()` 不再把 Blocked、
  Stopped 或 Exited 的 current task 伪造为 Ready，只调度真正被唤醒的任务；
- 增加 `TaskControlBlock::is_running()` 供该状态边界使用；
- 关闭定位阶段临时启用的 `FUTEX_TRACE`。

### 8.4 动态验收

| 检查 | 结果 |
| --- | --- |
| RV 修复前三用例连续 10 次 | 第 5、8 次 RV-musl `futex_wait02` 失败，可复现 |
| RV 修复后三用例连续 100 次冷启动 | PASS，100/100 正常关机 |
| 上述 100 次的 musl/glibc 选定执行 | PASS，600/600 |
| RV release 9 项 futex，musl/glibc | PASS，9/9 + 9/9 |
| LA release 9 项 futex，musl/glibc | PASS，9/9 + 9/9 |
| RV debug 三用例，musl/glibc | PASS，3/3 + 3/3，无 invariant/panic |
| LA debug 三用例，musl/glibc | PASS，3/3 + 3/3，无 invariant/panic |
| RV/LA 默认 release 构建 | PASS，已恢复非 LTP-only 内核 |
| RV/LA 原决赛镜像 smoke | 正常关机，无 panic；旧用例缺失仍以 ENOENT 失败 |
| 本次涉及文件 `rustfmt --check` | PASS |
| `git diff --check` | PASS |

9 项集合为：

```text
futex_cmp_requeue02, futex_wait01, futex_wait02, futex_wait03,
futex_wait04, futex_wait05, futex_wait_bitset01, futex_wake01,
futex_wake03
```

RV debug 的完整 9 项额外试跑中，严格计时项 `futex_wait05` 因 debug 开销产生
约 1 ms 超限，musl/glibc 均为 8/9；无 scheduler invariant、panic 或死锁。
正式计时结论采用 release 的双架构 9/9 + 9/9，不把 debug 性能超限隐瞒或误报
为功能通过。

### 8.5 当前边界

- 已完成“最后 ready task 阻塞/定时唤醒”真实复现序列的 100 次回归，以及
  EAGAIN、timeout、bitset、wake、cmp_requeue 的双架构双 libc 覆盖；
- signal/wake/timeout 三方相邻注入见第 13 节，线程退出 waiter 清理见第 14 节；
  SMP 压力尚未完成；
- `FUTEX_CMP_REQUEUE` 用户值比较到 queue lock 的窗口已在第 16 节关闭；
- 完整初赛镜像和本地修复日志均由 `.gitignore` 排除，不进入提交；
- 本轮未执行 `git commit`，由任务负责人审查后提交。

## 9. 2026-07-28 A-008 时钟诚实化

### 9.1 支持边界

| clock | 当前行为 | `clock_getres` |
| --- | --- | --- |
| `CLOCK_REALTIME` | monotonic 基准加可调整 offset | 1 µs |
| `CLOCK_MONOTONIC` | 硬件 timeout clock，不受 realtime 设置影响 | 1 µs |
| `CLOCK_MONOTONIC_RAW` | 无 NTP 调频时与硬件 monotonic 同源 | 1 µs |
| `CLOCK_REALTIME_COARSE` | realtime 向下量化到毫秒 | 1 ms |
| `CLOCK_MONOTONIC_COARSE` | monotonic 向下量化到毫秒 | 1 ms |
| `CLOCK_BOOTTIME` | 当前无 suspend 能力，连续启动期间等同 monotonic | 1 µs |
| process/thread CPU clock | 无可靠运行时间记账，返回 `EINVAL` | 不支持 |
| realtime/boottime alarm | 无 suspend/wakeup alarm，返回 `EINVAL` | 不支持 |
| `CLOCK_TAI` | 无 TAI offset 语义，返回 `EINVAL` | 不支持 |

`clock_settime` 只接受 `CLOCK_REALTIME`；timerfd 和 POSIX timer 也不再接受
未实现的 alarm clock。`clock_nanosleep(CLOCK_THREAD_CPUTIME_ID)` 保留
`EOPNOTSUPP`，其他不支持的 sleep clock 返回 `EINVAL`。

### 9.2 nanosleep 截止时间修复

初次 LTP 实测发现，旧实现按“起始毫秒向下取整 + duration 向上取整”构造相对
deadline，可能提前近 1 ms 唤醒。改为绝对截止点后虽不再 early，但毫秒等待队列
又可能 late 近 1 ms，仍超过 LTP 约 450 µs 的阈值。

最终处理：

- realtime/monotonic 统一从真实 hardware timeout counter 换算微秒；
- nanosleep wait 记录、deadline 索引和超时扫描全部使用微秒；
- `register_task_timeout(tid, deadline_ms)` 对 epoll 等既有毫秒调用方保持接口，
  进入队列时转换为微秒；
- 相对 sleep 的 remaining time 也按微秒计算；
- 不提高硬件中断频率，不宣称高精度 timer，只是不再主动丢弃计数器已有精度。

### 9.3 验证

当前承诺语义集：

```text
clock_getres01, clock_nanosleep01, clock_nanosleep02, clock_settime02,
nanosleep01, nanosleep02, nanosleep04
```

| 检查 | 结果 |
| --- | --- |
| RV release，musl/glibc | PASS，7/7 + 7/7 |
| LA release，musl/glibc | PASS，7/7 + 7/7 |
| RV 额外 `clock_nanosleep04` + `clock_settime01` | PASS，2/2 + 2/2 |
| `clock_getres01` 不支持边界 | CPU/alarm 均按 LTP `TCONF` 明确拒绝 |
| RV CPU clock 探测 (`clock_gettime01/02`) | 预期失败；LTP 要求成功，与任务书诚实拒绝要求冲突 |
| nanosleep 修改后的 futex 三用例回归 | PASS，20/20 冷启动、120/120 执行 |
| RV/LA 默认 release 构建 | PASS，最终内核已恢复非 LTP-only |
| RV/LA 原决赛镜像 smoke | 正常关机，无 panic；旧 LTP 缺失仍为 ENOENT |
| 本次涉及文件 `rustfmt --check` | PASS |
| `git diff --check` | PASS |

测试时临时把 `clock_nanosleep04`、`clock_settime01` 加入本地 LTP 列表，运行后
已恢复原列表，未留下该临时差异。本轮仍未执行 `git commit`。

### 9.4 剩余边界

- CPU clock 要完整支持，必须在上下文切换处分别累计 thread/process 实际运行
  时间；本轮按方案选择明确拒绝；
- 当前系统无 suspend，因此 `CLOCK_BOOTTIME` 与 monotonic 在可观察范围内相同；
- 未实现 NTP 调频、TAI offset、wakeup alarm 和高精度硬件 timer；
- fine/coarse 返回值和 realtime/monotonic 独立性已在第 17 节完成专项动态验收。

## 10. 2026-07-28 wait4 失败注入验收

新增仅在设置 `TASK_A_WAIT4_PROBE=1` 时由 testrunner 启动的
`task_a_wait4_probe`，默认比赛流程不运行该探针。

探针覆盖：

1. child 以状态 7 退出，首次 `wait4` 使用非法 status 指针，必须返回
   `EFAULT`；随后有效重试必须取得同一 child 和 `7 << 8`，第三次返回
   `ECHILD`；
2. 记录 `RUSAGE_CHILDREN` 基线，child 运行至少 30 ms 后以状态 9 退出；
3. 首次 `wait4` 使用有效 status、非法 rusage 指针，必须返回 `EFAULT`；
4. 失败后再次读取 `RUSAGE_CHILDREN`，user/system 两项必须与基线完全一致；
5. 使用有效 status/rusage 重试，返回的 child usage 必须大于 0；
6. 成功后的父进程 child usage 增量必须与本次返回值精确相等，第三次 wait
   必须为 `ECHILD`。

结果：

| 检查 | 结果 |
| --- | --- |
| RV release 单轮 | PASS |
| LA release 单轮 | PASS |
| RV 独立冷启动压力 | PASS，100/100 |
| LA 独立冷启动压力 | PASS，100/100 |
| status/rusage 两组核心断言 | PASS，400/400 |
| panic、超时、重复回收 | 未发现 |
| 默认 RV/LA release 恢复 | PASS，最终内核不启用 probe |

结论：`wait4` 的 status/rusage copyout 均位于 child accounting 和 Zombie
回收提交之前；copyout 失败不会丢失 child，也不会提前或重复累计 child ticks。
本轮未执行 `git commit`。

## 11. 2026-07-28 timer/prlimit 失败注入验收

新增仅在 `TASK_A_ATOMIC_PROBE=1` 时运行的 `task_a_atomic_probe`，覆盖：

- `timer_create` 非法 timerid 指针返回 `EFAULT`；通过前后有效 ID 和对中间 ID
  执行 `timer_delete == EINVAL`，证明 copyout 失败的对象未发布；
- `timer_settime` 使用非法 old-value 指针返回 `EFAULT`，随后
  `timer_gettime` 必须仍为未 armed；有效重试后 old value 为零且新 interval/value
  可见；
- `prlimit64` 使用非法 old-limit 指针返回 `EFAULT`，随后查询必须仍为原值；
  有效重试返回原 limit 并提交新值，最后恢复原值。

结果：

| 检查 | 结果 |
| --- | --- |
| RV 独立冷启动 | PASS，100/100 |
| LA 独立冷启动 | PASS，100/100 |
| 三类 prepare/copyout/commit 断言 | PASS，600/600 |
| panic、超时、对象误发布、状态提前提交 | 未发现 |
| 默认 RV/LA release 恢复 | PASS |

结论：`timer_create` 的发布点，以及 `timer_settime`、`prlimit64` 的状态提交点，
均位于相关用户输出成功之后。本轮未执行 `git commit`。

## 12. 2026-07-28 POSIX timer owner-exit 生命周期

专项构建设置 `TASK_A_TIMER_LIFECYCLE_TRACE=1`，在进程组退出清理 timer map 后
输出实际删除数量。探针子进程创建并 armed 3 个 10 秒 timer，随后立即退出；
父进程 wait 完成后再创建 successor。

结果：

| 检查 | 结果 |
| --- | --- |
| RV owner 退出删除计数 | PASS，100/100 轮均 `removed=3` |
| LA owner 退出删除计数 | PASS，100/100 轮均 `removed=3` |
| 实际删除 timer 总数 | 600/600 |
| successor PID | 每轮 owner=3、successor=4，严格递增 |
| panic、timeout、timer 残留计数异常 | 未发现 |
| 默认 RV/LA release 恢复 | PASS，生命周期 trace 关闭 |

PID 复用边界：

- 当前 `TidHandle::drop` 明确不调用 dealloc，分配器保持单调递增，所以现阶段无法
  在运行测试中制造 PID 复用；
- timer 同时保存 `Weak<TaskControlBlock>` owner，投递前必须成功 upgrade 且任务
  未退出；即使将来开启数字 PID 回收，也不会只凭复用后的 tgid 投递；
- 因此本轮结论是“owner-exit 清理已动态验收，PID-reuse 防线已代码审查，但
  PID-reuse 动态验收不适用当前分配策略”，不宣称后者已经运行通过。

本轮未执行 `git commit`。

## 13. 2026-07-28 futex wake/signal/timeout 三方竞态验收

新增仅在 `TASK_A_FUTEX_RACE_PROBE=1` 时运行的
`task_a_futex_race_probe`。探针通过 `MAP_SHARED | MAP_ANONYMOUS` 建立父子进程
真正共享的 futex 页，并分别制造：

1. wake 先完成：wait 返回 0，waker 必须报告恰好唤醒 1 个 waiter；
2. signal 先完成：wait 返回 `EINTR`，后到的 waker 必须报告 0；
3. timeout 先完成：wait 返回 `ETIMEDOUT`，后到的 waker 必须报告 0。

每种顺序还会等待 waker/signaler 完整退出，核对信号 handler 恰好执行一次，并在
结尾额外执行一次 `FUTEX_WAKE`，必须返回 0，以验证失败方不能覆盖 winner、队列中
没有残留 waiter。

首轮探针曾错误地在 signal handler 自身栈帧内直接调用 `sigreturn`，导致从错误
SP 读取 signal frame；修正为正常返回内核提供的 sigreturn trampoline 后再开始
正式计数。另一次试验使用普通 fork 的私有页，不能代表进程间共享 futex，已改为
内核明确支持跨 fork 共享的匿名 `MAP_SHARED` 页。上述探针问题没有计入内核失败。

结果：

| 检查 | 结果 |
| --- | --- |
| RV release 单轮三顺序 | PASS，3/3 |
| LA release 单轮三顺序 | PASS，3/3 |
| RV release 连续压力 | PASS，20/20 轮，60/60 场景 |
| LA release 连续压力 | PASS，20/20 轮，60/60 场景 |
| 双架构合计 | PASS，40/40 轮，120/120 场景 |
| wake-first 唤醒数 | 每轮均为 1 |
| signal/timeout-first 后到 wake | 每轮均为 0 |
| 每轮最终残留检查 | 额外 wake 每轮均为 0 |
| panic、timeout、重复完成、残留 waiter | 未发现 |

结论：当前单核 release 环境下，wake、signal、timeout 三类完成源满足
single-winner；loser 不会覆盖已提交结果，等待队列能够完整清理。该项关闭了
第 8.5 节中的“三方相邻注入”缺口，但线程退出 waiter 残留、SMP 压力以及
`FUTEX_CMP_REQUEUE` 用户值比较到 queue lock 的窗口后续已在第 16 节关闭。

本轮未执行 `git commit`；专项验证结束后重新构建默认 RV/LA release 内核。

## 14. 2026-07-28 futex waiter 线程退出清理验收

新增仅在 `TASK_A_FUTEX_EXIT_PROBE=1` 时运行的
`task_a_futex_exit_probe`。该探针不是用普通子进程代替线程，而是在被测 owner
进程内通过 `CLONE_VM | CLONE_SIGHAND | CLONE_THREAD` 创建真实线程：

1. waiter 线程在 `MAP_SHARED | MAP_ANONYMOUS` futex 上进入 10 秒 timed wait；
2. owner leader 确认 waiter 已开始等待后通知外层监督进程；
3. 监督进程向 owner leader 发送 `SIGKILL`，触发整个线程组退出；
4. 退出清理必须直接删除仍处于 blocked 状态的 waiter；
5. 监督进程确认 wait status 为 9，随后 `FUTEX_WAKE` 必须返回 0。

专项构建同时设置 `TASK_A_FUTEX_EXIT_TRACE=1`。`remove_futex_waiter` 会报告实际
删除的 queue 条目、wait 状态和 deadline 索引；默认构建不输出该 trace。

结果：

| 检查 | 结果 |
| --- | --- |
| RV release 连续压力 | PASS，20/20 |
| LA release 连续压力 | PASS，20/20 |
| 双架构线程退出场景 | PASS，40/40 |
| futex queue 删除 | 每轮 `queue=1`，40/40 |
| wait 状态删除 | 每轮 `wait=true`，40/40 |
| timeout deadline 删除 | 每轮 `deadline=true`，40/40 |
| owner wait status | 每轮均为 SIGKILL 9 |
| owner 退出后的额外 wake | 每轮均为 0 |
| panic、timeout、残留 waiter/deadline | 未发现 |

为使删除结果可审计，`FutexQueues::remove_tid` 现在返回实际删除数量，
`FutexWaits::cancel` 返回 wait/deadline 是否存在；默认运行语义不变。该项关闭
第 8.5 和第 13 节中的“线程退出 waiter 残留”缺口。

当时剩余 futex 专项为 SMP 压力，以及 `FUTEX_CMP_REQUEUE` 用户值比较到 queue
lock 之间的原子窗口；后者已在第 16 节关闭。本轮未执行 `git commit`；专项结束
后恢复默认双架构 release 内核。

## 15. 2026-07-28 首次提交前综合审查

按当前工作区完整差异进行逐文件复核，覆盖进程/线程退出、wait4、timer/prlimit、
clock/nanosleep、scheduler、futex 以及四个专项探针。

审查结论：

- 未发现阻止本次提交的正确性问题；
- wait4 与 timer/prlimit 的用户输出失败均发生在状态提交之前；
- 进程组退出通过共享 gate 单次提交，leader/non-leader 均执行统一线程私有清理；
- robust list、clear_child_tid、futex waiter 在用户地址空间回收前处理；
- futex wake/timeout/signal 使用 single-winner 状态，退出清理保持
  queue → waits 锁序；
- scheduler 允许最后一个 ready task 进入 timed block，并在 debug 构建检查
  ready/blocked/index/bitmap 不变量；
- clock 只声明实际支持的集合和分辨率，CPU/alarm/TAI 不再返回伪造时间；
- timer owner 使用 `Weak<TaskControlBlock>` 身份并在 owner exit 时同步删除；
- 专项 trace 与 testrunner 分支均由默认关闭的构建期开关控制。

提交前检查：

| 检查 | 结果 |
| --- | --- |
| `make check-submit MODE=release` | PASS |
| RV 默认 release 产物 | PASS，RISC-V ELF |
| LA 默认 release 产物 | PASS，LoongArch ELF |
| 全部涉及 Rust 文件 `rustfmt --check` | PASS |
| `git diff --check` | PASS |
| 默认内核专项 trace 字符串检查 | PASS，未包含 |
| 修复日志 ignore 状态 | PASS |
| 最新提交 | 仍为 `00c6822` |

专项动态证据汇总：

- wait4 失败注入：RV/LA 各 100/100，核心断言 400/400；
- timer/prlimit 失败注入：RV/LA 各 100/100，核心断言 600/600；
- POSIX timer owner-exit：RV/LA 各 100/100，删除 600/600 个 timer；
- futex scheduler 复现：100/100 启动、600/600 执行；
- futex LTP：RV/LA 双 libc 均 9/9；
- clock/nanosleep：RV/LA 双 libc 均 7/7；
- futex wake/signal/timeout：双架构 40/40 轮、120/120 场景；
- futex waiter 线程退出：双架构 40/40，每轮均删除 queue/wait/deadline。

本次可以提交，但不等同于任务 A 全部验收结束。明确保留：

- `FUTEX_CMP_REQUEUE` 原子窗口后续已在第 16 节关闭；
- 真正 SMP 并发压力；
- CPU clock 运行时间记账、TAI、suspend/boottime 差异和 wakeup alarm；
- PID 回收启用后的 timer PID-reuse 动态测试。

本轮只审查、验证和同步记录，未执行 `git commit`。

## 16. 2026-07-28 FUTEX_CMP_REQUEUE 原子窗口与 timeout 精度

### 16.1 CMP_REQUEUE 线性化

旧实现先在无 queue lock 状态读取并比较 `*uaddr`，随后才进入
`futex_requeue_common` 获取 `FUTEX_QUEUES`。比较成功到 queue mutation 之间存在
窗口，值已变化时仍可能 wake/requeue waiter。

修复：

- `futex_requeue_common` 接受可选 expected value；
- CMP 路径先在锁外检查并预触发用户页，避免正常情况下持有 no-IRQ queue lock
  时分配页面；
- 最终 `copy_from_user`、expected 比较、wake/requeue 位于同一次
  `FUTEX_QUEUES` 临界区；
- 固定并注释锁序为
  `FUTEX_QUEUES → MemorySet → FUTEX_WAITS → scheduler`；
- 普通 REQUEUE 继续复用同一 queue mutation 实现；
- source/target key 相同时仍只执行指定数量的 wake，不做无意义迁移。

专项构建开关 `TASK_A_FUTEX_CMP_REQUEUE_TEST_YIELD=1` 在获取 queue lock 前强制
让出 CPU，直到修改进程把 source word 从 expected 改变，稳定制造旧窗口。
`task_a_futex_cmp_requeue_probe` 断言：

1. CMP_REQUEUE 必须返回 `EAGAIN`；
2. target wake 必须返回 0；
3. source wake 必须返回 1；
4. waiter 回收后 source/target 额外 wake 都必须返回 0。

初版注入只执行一次 yield，第 6 轮修改进程尚未运行，比较按正确语义成功并使探针
失败；修正为等待实际值变化后才进入最终比较，再开始正式计数。该失败属于注入
不确定，不计为内核原子性失败。

结果：

| 检查 | 结果 |
| --- | --- |
| RV 强制窗口注入 | PASS，20/20 |
| LA 强制窗口注入 | PASS，20/20 |
| 双架构比较失败不迁移 | PASS，40/40 |
| target/source wake 位置断言 | PASS，80/80 |
| 最终 queue 残留断言 | PASS，80/80 |

### 16.2 futex timeout 微秒化

CMP 正常路径回归首次运行时，RV `futex_wait05` 的 musl/glibc 都出现亚毫秒级
early wake，整组为 8/9 + 8/9。原因是 nanosleep 已改为微秒 deadline，但 futex
仍使用“当前毫秒向下取整 + duration”的 deadline，最多可能提前近 1 ms。

处理：

- `FutexDeadline`、deadline map、过期扫描和相对 timeout 统一改为微秒；
- 使用硬件已有的 `get_time_us/get_timeout_us`，不提高 timer interrupt 频率；
- wake/signal/timeout 仍由原 single-winner 状态决定结果。

修复后的回归：

| 检查 | 结果 |
| --- | --- |
| RV 9 项 futex，musl/glibc | PASS，9/9 + 9/9 |
| LA 9 项 futex，musl/glibc | PASS，9/9 + 9/9 |
| 其中 `futex_cmp_requeue02` | 双架构双 libc 正常成功路径通过 |
| 其中 `futex_wait05` | 双架构双 libc 无 early wake |
| RV 三方竞态压力 | PASS，20/20 轮，60/60 场景 |
| LA 三方竞态压力 | PASS，20/20 轮，60/60 场景 |

当前单核任务范围内，CMP_REQUEUE 原子窗口已关闭。真正 SMP 下的页表并发变更和
多核 queue contention 仍未动态验收。本轮未执行 `git commit`，并将在专项测试
后恢复默认双架构 release 构建。

## 17. 2026-07-28 clock 分辨率与 realtime/monotonic 独立性

新增默认关闭的 `TASK_A_CLOCK_PROBE` 构建期开关和
`task_a_clock_probe`，直接调用 clock syscall 验证此前只由 LTP 间接覆盖的边界。

每轮断言：

- `CLOCK_REALTIME`、`CLOCK_MONOTONIC`、`CLOCK_MONOTONIC_RAW`、
  `CLOCK_BOOTTIME` 的 `clock_getres` 精确返回 1 µs；
- `CLOCK_REALTIME_COARSE`、`CLOCK_MONOTONIC_COARSE` 精确返回 1 ms；
- process/thread CPU clock、realtime/boottime alarm 和 TAI 返回 `EINVAL`；
- 将 realtime 向前调整 3600 秒后，realtime 观测到对应跳变；
- 同期 monotonic 只允许实际执行耗时增长，不得出现 realtime 的一小时跳变；
- `clock_settime(CLOCK_MONOTONIC)` 返回 `EINVAL`。

结果：

| 检查 | 结果 |
| --- | --- |
| RV release 连续压力 | PASS，20/20 |
| LA release 连续压力 | PASS，20/20 |
| fine/coarse 分辨率轮次 | PASS，40/40 |
| unsupported clock 边界轮次 | PASS，40/40 |
| realtime/monotonic 独立性轮次 | PASS，40/40 |
| monotonic 非法 settime | PASS，40/40 |
| panic、timeout、分辨率误报、monotonic 跳变 | 未发现 |

该项关闭第 9.4 节中最后两项建议性专项证据。CPU clock、TAI、suspend 语义和
wakeup alarm 仍按方案作为明确不支持能力，不用伪实现换取表面通过。本轮未执行
`git commit`，专项结束后恢复默认 RV/LA release 构建。

## 18. 2026-07-28 任务 A 最终验收矩阵复核

### 18.1 A 任务书最终清单

| 验收项 | 状态 | 证据/边界 |
| --- | --- | --- |
| 普通退出与信号退出资源语义一致 | PASS | 统一 `ExitCause` 和 process-group exit |
| process/signal/time/scheduler syscall 已分级 | PASS | 第 2 节 ABI 台账 |
| POSIX timer copyout/owner-exit 不泄漏 | PASS | A-011/A-012 |
| 异步对象不因 pid/tgid 复用误投递 | PASS（代码）/N/A（动态复用） | Weak owner；当前 PID 不回收 |
| prlimit/timer/wait 提交与 copyout 顺序有测试 | PASS | 失败注入 100/100 双架构 |
| wait copyout 失败不丢 child | PASS | 有效重试、accounting、ECHILD 断言 |
| scheduler invariant 固定回归不触发 | PASS | debug/release 与 100 次复现 |
| futex wake/timeout/signal 不重复完成 | PASS | 双架构三方竞态 40/40 |
| 退出任务无 waiter/timer 残留 | PASS | waiter queue/wait/deadline 与 timer 删除计数 |
| monotonic 不受 realtime 调整影响 | PASS | clock probe 双架构 40/40 |
| 时钟精度和支持范围真实 | PASS | 1 µs/1 ms 直读；unsupported 返回 EINVAL |
| RV/LA 双架构通过 | PASS | 构建、LTP、专项 probe |
| 相关测试连续运行无随机 hang | PASS（单核范围） | 所有正式压力轮完成 |
| A 的提交可按语义独立回退 | READY | `57046f1` + 当前待提交的 futex/clock 批次 |

### 18.2 总控最终审查

| 类别 | 状态 | 说明 |
| --- | --- | --- |
| RV/LA release 构建与产物类型 | PASS | `make check-submit MODE=release` |
| rustfmt / diff whitespace | PASS | 全部涉及文件 |
| 固定进程/同步 smoke | PASS | wait/timer/futex/clock 专项与 LTP 子集 |
| futex LTP | PASS | RV/LA，musl/glibc 均 9/9 |
| clock/nanosleep LTP | PASS | RV/LA，musl/glibc 均 7/7 |
| 资源残留 | PASS | health、timer、waiter、Zombie 专项 |
| ABI struct 与 errno | PASS | 原始 syscall probe 和失败注入 |
| syscall 层新增测试特判 | PASS | 无；仅 testrunner/trace 构建期开关 |
| 默认 RV/LA + 当前决赛镜像 smoke | PASS（启动/退出） | 正常关机、无 panic |
| 决赛镜像旧 LTP 功能结果 | N/A | 文件缺失为 ENOENT，不误报功能失败/通过 |

默认内核在当前 128 MiB 决赛镜像上运行完整 testrunner 时，RV 687 项、LA 688 项
旧 LTP 因镜像不含对应文件返回 ENOENT；两架构均正常运行到汇总并关机，无 panic。
功能结论继续采用 4 GiB 初赛完整镜像上的专项结果。

### 18.3 保留边界与合并结论

- 真正 SMP 的页表变化、queue contention 和并发 timer generation 未动态验收；
- CPU clock、TAI、suspend/boottime 差异、wakeup alarm 明确不支持；
- PID 分配器启用回收后需补 timer PID-reuse 动态测试；
- 总控 7.3 建议的五类性能中位数尚未形成正式表格；release invariant 已编译消除，
  当前专项未观察到功能性性能退化。

结论：任务 A 的四天正确性必做项已完成，当前批次提交后可以进入合并审查；上述
内容作为明确支持边界，不以伪实现补齐。本轮未执行 `git commit`。
