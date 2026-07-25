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
| `clone/exec/exit/exit_group` | B | 项目采用 leader exit 结束线程组的简化策略；普通/信号退出清理不对称 | 统一进程组退出核心路径 |
| `wait4/waitid` | B | 基本 wait 语义存在；失败原子性仍需测试，`wait4` 的 rusage copyout 前已累计 child ticks | 修正 prepare/copyout/commit |
| `kill/tkill/tgkill/rt_sig*` | B | 有基础 signal 支持；copyout 失败、阻塞竞争和退出清理未完整验证 | 深审 `sigtimedwait` 和 signal info |
| `futex/robust_list` | B | wait/wake/bitset/requeue 已实现；完成原因和 timeout/signal/wake 竞争尚未统一 | 竞争测试后决定是否提升 |
| `sched_*` | B | 参数和属性接口存在；FIFO/RR、affinity 是否真实影响调度需验证 | 对照 scheduler 状态机 |
| `get/setpriority` | B | 基础优先级状态存在 | 验证 ready task 重排 |
| `prlimit/getrlimit/setrlimit` | B | 状态修改发生在 old-limit copyout 之前 | 修正或固定失败语义 |
| credential/capability | B | uid/gid/cap 接口存在，完整权限模型不在本轮范围 | 明确受限支持边界 |
| `timer_create/delete/get/settime` | D | create 可在 copyout 失败时泄漏；owner 只保存 tgid 且 exit 不清理 | 修复后降为 B，再补生命周期测试 |
| `get/setitimer` | B | task 内存在基础状态 | 验证 owner exit 和 signal 投递 |
| `clock_gettime/getres` | D | CPU/TAI/alarm 使用启动时间冒充，getres 固定声称 1ns | 收紧支持范围 |
| `clock_settime/settimeofday` | B | realtime offset 模型存在 | 验证不影响 monotonic |
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
  退出均按 owner tgid 清理 POSIX timer。稳定对象身份仍待后续对象模型支持。
- 最小回归：非法 `timerid` 返回 `EFAULT`，随后可观测 timer 数不增加；owner exit 后 timer 数恢复；PID 复用后新任务不收旧 signal。

### 3.2 `timer_settime`

- 当前等级：B。
- 修改对象：`POSIX_TIMERS[timerid]`。
- 第一次状态修改：更新 deadline 和 interval。
- copyout：状态修改完成后才复制 old value。
- copyout 失败状态：新配置已经生效。
- 并发问题：若改成锁外先 copyout、再按数字 ID 查表，需要定义与并发 delete/settime 的竞争语义。
- 结论：P1，需明确 prepare/commit 或用稳定的 timer 对象引用。

### 3.3 `prlimit64`

- 当前等级：B。
- 修改对象：目标进程 rlimit。
- 第一次状态修改：`set_rlimit`。
- copyout：修改后才输出 old limit。
- copyout 失败状态：limit 已改变，调用者收到 `EFAULT`。
- 结论：P1，应在提交前输出 old limit，或明确并测试 Linux 失败语义。

### 3.4 `wait4`

- 当前等级：B。
- 修改对象：child 记录、退出事件、父进程 child CPU ticks。
- status copyout：在回收前完成，失败不会立即删除 child。
- rusage copyout：回收前完成，但 child ticks 已先累计到 parent。
- copyout 失败状态：child 尚可再次 wait，但重试会重复累计 child ticks。
- 结论：P1，需要把所有输出准备和 copyout 放在 child ticks/回收提交之前。

### 3.5 进程组退出

- 当前等级：B。
- 普通退出：仅当 `memory_set` 强引用计数为 1 时回收数据页。
- 信号退出：无条件调用 `recycle_data_pages()`。
- 风险：`CLONE_VM` 或其他共享 mm owner 仍存活时，信号退出可能破坏共享地址空间。
- 两条路径均未发现 POSIX timer owner 清理。
- 结论：P0，普通退出与信号退出必须收敛到同一核心清理路径。

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
| P0 | 信号退出无条件回收可能共享的 mm | 统一退出核心路径 |
| P1 | `timer_create` copyout 失败泄漏 | 已修：copyout 成功后再提交对象；待运行回归 |
| P1 | timer owner exit 不清理且受 PID 复用影响 | 已修：exit 显式清理；稳定 owner 引用待评估 |
| P1 | `wait4` rusage copyout 失败重复累计 child ticks | 延后所有状态提交 |
| P1 | `timer_settime` old value copyout 失败但新状态已生效 | 设计 prepare/commit |
| P1 | `prlimit64` old value copyout 失败但 limit 已改变 | 设计 prepare/commit |
| P1 | CPU/TAI/alarm clock 假实现及 1ns 假精度 | 收紧 clock 支持范围 |

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
