# RespOS 任务 A 本地修复日志

> 用途：记录任务 A 每一部分修复的原因、修改、验证结果和剩余风险。  
> 本文件已加入 `.gitignore`，仅作为本地协作记录，不进入正式提交。  
> 提交规则：Codex 不执行 `git commit`，由负责人检查后手动提交。

## 给学长的快速说明

### 这项任务要解决什么

任务 A 负责 RespOS 的“进程、同步和时钟”。通俗地说，就是保证下面这些事情
在正常情况和竞争情况中都不会出错：

- 进程或线程退出时，资源只清理一次，也不会误删仍被其他进程使用的资源；
- 父进程等待子进程时，用户地址错误不能导致子进程丢失、重复回收或重复记账；
- futex 的唤醒、超时、信号中断同时发生时，只能有一个最终结果；
- futex waiter 所在线程退出后，队列和 timeout 记录不能残留；
- 系统只报告真正实现的时钟，分辨率必须与实际精度一致；
- timer、资源限制等系统调用如果返回失败，不能偷偷留下已经生效的半成品状态；
- 调度器在暂时没有 ready task 时，仍能等待定时器或 futex 唤醒，不能误报失败；
- debug 构建能尽早发现 ready、blocked 和 exited 状态互相矛盾。

这不是增加一套新功能，而是把原来分散、容易在边界情况下互相打架的实现
整理成一致的状态机和清理顺序。

### 最终做成了什么

1. **统一退出流程**
   - 普通退出和信号退出共用同一套清理流程；
   - 一个线程组只有第一个退出请求能够执行进程级清理；
   - `robust futex`、`clear_child_tid`、普通 futex waiter、timer、文件表和地址
     空间按照固定顺序处理；
   - 外部进程仍共享地址空间或文件表时，不会提前破坏共享资源。

2. **修复 wait4 的“失败也改了状态”**
   - 先准备 status/rusage，再写入用户空间，最后才真正回收子进程和累计用量；
   - 非法用户指针返回失败后，可以再次 `wait4` 正常重试；
   - 多个 waiter 竞争同一个 child 时不再通过 `unwrap()` 触发内核 panic。

3. **把 futex 竞争收敛成单赢家**
   - wake、timeout、signal 三条路径共同竞争一个完成状态；
   - 第一个到达者决定结果，后到者不能重复唤醒或覆盖结果；
   - 线程退出会同时删除 futex 队列项、完成记录和 timeout deadline；
   - `CMP_REQUEUE` 的最终比较和队列搬运放进同一个队列临界区；
   - futex 地址必须 4 字节对齐，未对齐返回 `EINVAL`；
   - 锁内最终读取使用固定 4 字节 no-fault 读取，不在全局 futex 锁内触发
     惰性分配或 page fault。

4. **修正调度器的阻塞边界**
   - 最后一个可运行任务也可以进入 futex/nanosleep 等待；
   - ready queue 暂时为空时，调度器等待真正的 timer/futex wake；
   - debug 构建增加 ready/blocked/bitmap/task index 一致性检查，release 不带
     这项全量扫描。

5. **修正时钟和事务提交语义**
   - fine clock 公布 1 微秒分辨率，coarse clock 公布 1 毫秒分辨率；
   - nanosleep 和 futex timeout 内部使用微秒 deadline，修复约 1 毫秒的
     提前或过晚唤醒；
   - realtime 调整不会带着 monotonic 一起跳；
   - CPU clock、TAI、alarm 等未实现能力明确返回错误，不再返回假时间；
   - `timer_create`、`timer_settime`、`prlimit64` 都遵守
     “准备 → copyout → 提交”，失败不改变共享状态；
   - POSIX timer owner 退出时同步清理，并用 `Weak<TaskControlBlock>` 防止未来
     PID 复用后向错误进程投递信号。

### 修改和创建的文件说明

#### 内核实现：真正改变 RespOS 行为的文件

| 文件 | 通俗说明 |
| --- | --- |
| `os/src/task/task.rs` | 统一普通/信号退出，处理线程组单次退出、共享 mm/fd、timer 和线程清理 |
| `os/src/task/scheduler.rs` | 修复最后一个 ready task 无法阻塞的问题，并增加 debug 一致性检查 |
| `os/src/task/futex/wait.rs` | 实现 wake/timeout/signal 单赢家，清理退出 waiter，收紧 CMP_REQUEUE |
| `os/src/task/futex/queue.rs` | 支持按 tid 删除残留 waiter，并返回实际删除数量供验收 |
| `os/src/task/futex/mod.rs` | 统一检查 futex 主地址和 requeue 目标地址的 4 字节对齐 |
| `os/src/signal/mod.rs` | 信号到达时接入 futex 中断完成路径 |
| `os/src/syscall/process.rs` | 修复 wait4/waitid、prlimit、getppid 和退出相关的失败原子性及 panic 风险 |
| `os/src/syscall/time.rs` | 修复 clock、nanosleep、POSIX timer 的范围、精度和提交顺序 |
| `os/src/syscall/special_fd.rs` | timerfd 拒绝尚未实现的 alarm clock |
| `os/src/syscall/mod.rs` | 接通上述系统调用需要的公共接口 |
| `os/src/mm/mod.rs` | 增加 CMP_REQUEUE 锁内使用的固定 4 字节 no-fault 用户读取 |

#### 用户态测试：用于证明修复有效，不负责实现内核功能

| 新增文件 | 验证内容 |
| --- | --- |
| `user/src/bin/task_a_wait4_probe.rs` | wait4 的非法 status/rusage、有效重试、只回收和记账一次 |
| `user/src/bin/task_a_atomic_probe.rs` | getppid、timer 和 prlimit 失败原子性、timer owner 退出清理 |
| `user/src/bin/task_a_futex_race_probe.rs` | wake、signal、timeout 三种先后顺序只能产生一个结果 |
| `user/src/bin/task_a_futex_exit_probe.rs` | 真实线程组被杀死后，futex waiter 和 deadline 没有残留 |
| `user/src/bin/task_a_futex_cmp_requeue_probe.rs` | CMP_REQUEUE 竞争窗口、错误搬运和未对齐地址 |
| `user/src/bin/task_a_clock_probe.rs` | 时钟分辨率、未支持范围、realtime/monotonic 相互独立 |

以下现有用户态文件只增加了测试入口：

| 文件 | 作用和默认影响 |
| --- | --- |
| `user/src/syscall.rs` | 增加 wait4、timer、clock、prlimit、futex、mmap 等原始 syscall 封装 |
| `user/src/lib.rs` | 给专项 probe 提供更方便的用户态函数；本身不修改内核 |
| `user/src/bin/testrunner.rs` | 增加任务 A 专项运行开关和重复运行入口；开关默认关闭，普通比赛流程不主动运行这些 probe |

`user/probes/task_a_perf.c` 是单独创建的性能测量工具。它使用真实的
musl/pthread/futex 路径，对比修改前后的 getpid、yield、线程创建、无竞争
futex、竞争 futex 和 sleep/wakeup 开销。它不在 `user/src/bin/` 下，现有
Makefile 不会默认编译或打包它，因此不会改变默认内核功能。单独放置是为了
避免要求所有开发者额外安装 C/pthread 交叉编译环境。

#### 文档和协作文件

| 文件 | 作用 |
| --- | --- |
| `docs/四天内核重构-A-执行记录.md` | 正式记录任务 A 的要求映射、命令、结果和验收证据，可进入提交 |
| `docs/四天内核重构-A-修复日志.md` | 当前这份按批次记录的本地详细日志，已加入 `.gitignore` |
| `.gitignore` | 只增加对本地修复日志的忽略，避免把个人协作流水账误提交 |

### 如何证明修改是正向的

- RV 和 LA 的 release 构建均通过，最终 `make check-submit MODE=release` 通过；
- 初赛 4 GiB 完整镜像中，任务 A 选择的 futex 9 项和 clock/nanosleep 7 项在
  RV/LA、musl/glibc 上均通过；
- wait4、timer/prlimit、futex 三方竞争、futex 退出清理、CMP_REQUEUE 和 clock
  专项 probe 均完成双架构动态验证；
- 关键事务测试完成双架构 100 次冷启动，竞争类测试完成双架构各 20 轮；
- 默认 release 产物不包含 debug invariant、临时 trace、强制竞态注入和性能
  probe 标记；
- 最终代码通过 `rustfmt --check`、`git diff --check`，本批次没有新增
  `todo!`、`unimplemented!`、`panic!`、`.unwrap()` 或 `.expect()`；
- 当前 128 MiB 决赛镜像能正常启动、运行到汇总并关机，但它没有初赛 LTP 文件，
  日志里的 `ENOENT/-2` 只表示测试文件不存在，不算功能失败或功能通过。

### 必须如实说明的边界

- 当前 QEMU 使用 `SMP=1`，没有真正完成多核并行压力测试；
- CPU clock、TAI、suspend/boottime 差异和 wakeup alarm 没有实现，系统会明确
  拒绝，不伪装为已支持；
- 当前 TID/PID 分配器不回收 ID，所以 PID reuse 动态测试不适用；代码已经用
  稳定对象身份防护，但要在未来启用 ID 回收后重测；
- 性能对比中 getpid 和无竞争 futex 基本稳定，yield 增加约 4.5%～5.9%，
  pthread create/join 增加约 23.4%～32.2%，竞争 futex 增加约 22.5%～24.0%；
  增量集中在单赢家登记和退出清理路径，属于当前正确性修复的成本，后续可在
  保持现有断言的前提下继续优化；
- 任务 A 自身已经具备合并审查条件，但不能代表任务 B、任务 C 和整仓决赛
  baseline 已经由任务 A 负责人验收。

### 提交状态

- 已完成批次由负责人自行检查并提交，Codex 没有执行 `git commit`；
- 已提交的任务 A 批次为：
  - `00c6822 fix: 建立任务A基线和ABI初审`
  - `57046f1 fix: 收敛任务退出、futex 竞争与时钟语义`
  - `f35ff2d fix: 收敛futex重排竞态与时钟精度验收`
- 当前 `f35ff2d` 之后的最终补漏、性能工具和日志同步仍保留在工作区，等待负责
  人检查后决定提交；
- 本日志被 `.gitignore` 排除，即使更新也不会进入正式 commit。

## 工作约定

每完成一部分修复，按以下顺序处理：

1. 更新本日志；
2. 执行与修改风险相称的静态检查、双架构构建和回归；
3. 在日志中如实记录未运行或失败的测试；
4. 保持修改留在工作区，不自行提交；
5. 由负责人检查 diff 后决定如何拆分和提交。

每项修复记录：

```text
状态：待处理 / 进行中 / 已完成 / 已验证 / 暂缓
问题：
根因：
修改：
保持的不变量：
验证：
剩余风险：
建议提交：
```

## 已完成工作

### A-001：建立任务 A 基线和 ABI 初审

状态：已完成

问题：

- 修改前缺少任务 A 独立的构建、运行和 ABI 风险基线；
- 无法区分既有问题与重构引入的回归。

完成内容：

- 基线 commit：`3839e4e5b7fd8410b727493d48fe0646240f166d`；
- 记录 Rust、Cargo、QEMU 版本；
- 完成 process、signal、futex、scheduler、timer、clock 等家族的初步 ABI 分级；
- 深审 `timer_create`、`timer_settime`、`prlimit64`、`wait4`、进程组退出和 clock family；
- 整理首批 P0/P1、失败注入测试和跨模块接口需求。

验证：

- RV `cargo check`：PASS；
- LA `cargo check`：PASS；
- `make build-rv`：PASS；
- `make build-la`：首次冷构建因 lwext4 生成头文件时序失败，重试 PASS；
- RV、LA 均启动到 `[testrunner] start`，未观察到启动 panic。

限制：

- 当前 `img/sdcard-rv.img` 和 `img/sdcard-la.img` 缺少 basic/LTP 文件；
- 大量现有测试因 `ENOENT` 无法执行，因此目前只能确认启动 smoke，不能作为完整功能基线。

历史提交：

```text
1595d77 docs(task): record runtime ABI audit baseline
```

### A-002：修复 POSIX timer 发布和 owner-exit 清理

状态：已验证（构建与启动 smoke）

问题：

1. `timer_create` 先向全局 `POSIX_TIMERS` 插入对象，再向用户空间写 timer ID；
2. timer ID copyout 失败时，对象已存在，但用户无法取得 ID，形成不可删除泄漏；
3. POSIX timer 只保存数字 `tgid`，进程退出时没有显式清理；
4. 若未来恢复 PID/TID 复用，旧 timer 可能向无关的新任务投递信号。

根因：

- timer 创建路径没有明确区分 prepare 和 commit；
- timer 生命周期没有接入进程组退出路径；
- 异步对象 owner 最初只保存数字 `tgid`。

修改：

- `sys_timer_create` 在 timer ID 成功 copyout 后才将 timer 插入全局表；
- 新增 `remove_posix_timers_for_owner(owner_tgid)`；
- 普通进程组退出和信号进程组退出均清理 owner 的全部 POSIX timer；
- timer 同时保存 `Weak<TaskControlBlock>` 稳定 owner 身份；到期投递只向该对象
  发送信号，不再根据数字 tgid 重新查找可能已经复用的新任务；
- 数字 `owner_tgid` 仅用于权限校验和退出时批量筛选。

保持的不变量：

- 返回成功的 timer 必须能由返回的 ID 管理；
- copyout 失败不能留下不可达 timer；
- owner 完成进程组退出后不能残留异步 timer；
- 退出清理不持有 timer 锁执行 copyin、copyout 或调度。

验证：

- RV `cargo check`：PASS；
- LA `cargo check`：PASS；
- `make build-rv`：PASS；
- `make build-la`：PASS；
- `git diff --check`：PASS；
- RV、LA 启动到 testrunner：PASS。

剩余风险：

- 尚未执行 `timer_create` 非法 `timerid` 的运行级失败注入测试；
- 尚未执行 owner exit 后的 timer 残留检查；
- `timer_settime` 的 old-value copyout/commit 顺序已在 A-005 修复，但运行级
  失败注入仍待完整测试镜像。

历史提交：

```text
163cba1 fix(timer): publish and clean up POSIX timers safely
```

## 逐项修复与验收记录（历史顺序）

### A-003：修复 `wait4` 失败原子性

状态：已验证（静态检查与双架构构建）

已知问题：

- status copyout 失败时 child 尚未被回收；
- rusage copyout 失败前，父进程已经累计 child ticks；
- 再次 wait 会重复累计 child ticks。

根因：

- `wait4` 在完成 rusage copyout 前调用 `add_child_ticks`；
- child 资源使用量在 copyout 阶段重新从 children 表查询；
- `wait4` 和 `waitid` 在最终回收时使用 `remove(...).unwrap()`，同一进程内
  多线程并发 wait 时存在可达 panic 风险。

修改：

- 选择已退出 child 时同步快照 wait status 和 child ticks；
- status 与 rusage 全部 copyout 成功后，才进入提交阶段；
- 提交阶段先确认 Zombie 确实从 children 表移除，再累计 child ticks；
- copyout 失败时不累计 child ticks、不删除 child、不移除 exited-child 记录；
- `wait4` 和 `waitid` 的最终回收不再使用 `unwrap()`，并发下失去回收权时
  返回 `ECHILD`，避免用户态竞争触发内核 panic；
- 复用现有 `rusage_from_ticks`，避免保留第二份时间换算逻辑。

最终顺序：

```text
查找 child
→ 快照 pid/status/rusage 所需 ticks
→ 完成全部 copyout
→ 回收 child
→ 累计 child ticks
→ 删除 exited-child 索引
```

保持的不变量：

- exit 与 wait 分离，copyout 失败不能丢失 Zombie；
- child ticks 对一个成功回收的 child 至多累计一次；
- 用户指针错误不能触发 `unwrap()` panic；
- stopped/continued 事件不作为 Zombie 回收；
- 不在 children 锁内执行 copyout。

验证：

- `rustfmt --check os/src/syscall/process.rs`：PASS；
- `git diff --check`：PASS；
- RV `cargo check`：PASS；
- LA `cargo check`：PASS；
- `make build-rv`：PASS；
- `make build-la`：PASS。

未完成验证：

- 当前测试镜像缺少专项用户程序，尚未运行非法 status/rusage 指针后再次
  wait 的动态回归；
- 尚未运行同一父进程内两个线程并发 wait 同一 child 的竞争回归；
- copyout 已成功但并发回收权丢失时会返回 `ECHILD`，输出缓冲区可能已经被
  填写；要提供更强的并发 wait 原子性需要给 Zombie 增加预留/领取状态，
  本次未扩大状态机。

建议提交：

```text
fix(wait): defer child accounting until copyout succeeds
```

### A-004：统一普通退出和信号退出

状态：已验证（静态检查、双架构构建与启动 smoke），P0

已知问题：

- 普通退出仅在 mm 无其他强引用时回收数据页；
- 信号退出无条件回收数据页；
- `CLONE_VM` 共享 mm 时，信号退出可能破坏仍存活任务的地址空间。

根因：

- `task_group_exit` 与 `task_group_exit_by_signal` 维护两份近似但不一致的
  资源清理流程；
- 信号路径绕过普通路径的共享 mm 判断；
- 线程普通退出和线程信号退出也分别维护 robust-list、`clear_child_tid`
  和调度器移除逻辑；
- 构造用户 signal frame 失败时调用普通 exit，将信号终止错误编码成
  普通 exit code；
- 单纯使用 `Arc::strong_count == 1` 无法区分“同一线程组内的资源 owner”
  和“通过 `CLONE_VM/CLONE_FILES` 共享资源的外部进程”。

修改：

- 引入统一 `ExitCause::{Code, Signal}`；
- 普通进程组退出和信号进程组退出统一进入 `exit_process_group`；
- 线程组共享 `group_exiting` 原子状态，只有第一个退出请求执行进程级资源清理；
- 非 leader 线程统一进入 `exit_thread_inner`，按 `ExitCause` 写入状态；
- leader 与非 leader 共用 `cleanup_exiting_thread`，确保 robust-list、
  `clear_child_tid`、futex waiter 和 scheduler 清理均发生在 mm 失效前；
- 普通/信号退出现在具有相同的线程移除、children 托管、robust-list、
  mm/fd、POSIX timer、signal 清理和 parent 通知顺序；
- 根据当前线程组内持有相同 mm/fd table 的 TCB 数量，与 Arc 总引用数
  比较，区分线程组内部共享和外部 `CLONE_VM/CLONE_FILES` owner；
- 资源只被当前线程组持有时主动清理；存在外部 owner 时不拆映射、不清空
  fd table，由最后一个 owner 的 Drop 完成最终释放；
- signal-frame copyout 失败改为信号退出，parent wait 能观察到正确的
  signal wait status；
- 移除已重复的 `task_group_exit_by_signal` 和
  `exit_thread_by_signal_detached` 状态机。

保持的不变量：

- 普通退出和信号退出执行相同范围、相同顺序的资源清理；
- robust futex 和 `clear_child_tid` 在用户地址空间失效前处理；
- 一个进程组只留下 leader Zombie，由 parent wait 回收；
- 外部 `CLONE_VM` owner 存活时不能回收共享地址空间；
- 外部 `CLONE_FILES` owner 存活时不能清空共享 fd table；
- 只有本线程组持有的 mm/fd 不延迟到 parent wait 才释放；
- signal exit 使用 signal wait status，普通 exit 使用 exit-code status；
- owner exit 清理 POSIX timer。
- 重复进程组退出不能重复执行资源回收或 parent 通知。

验证：

- `rustfmt --check`：PASS；
- `git diff --check`：PASS；
- RV `cargo check`：PASS；
- LA `cargo check`：PASS；
- `make build-rv`：PASS；
- `make build-la`：PASS；
- RV snapshot 启动：testrunner 完成两组选择测试并正常关机，无 panic；
- LA snapshot 启动：testrunner 完成两组选择测试并正常关机，无 panic。

运行限制：

- 当前镜像缺少 basic/LTP 文件，699 个选择项主要因 `ENOENT` 失败；
- 启动 smoke 能覆盖大量 fork/exec-fail/exit/wait 循环和最终关机，但不能
  替代 `CLONE_VM`、`CLONE_FILES` 和 signal wait-status 专项回归。

待补动态回归：

- 普通 exit 与 SIGTERM/SIGKILL 的 wait status；
- 多线程进程 leader/non-leader 发起 exit_group；
- 独立进程通过 `CLONE_VM` 共享 mm，一方信号退出后另一方继续读写；
- 独立进程通过 `CLONE_FILES` 共享 fd table，一方退出后另一方 fd 可用；
- 不共享资源的进程退出后 frame/fd 引用及时释放；
- robust futex 与 `clear_child_tid` 在普通/信号退出下行为一致。

剩余风险：

- 当前没有独立 Process 对象或显式 mm/fd owner 计数，使用 TCB Arc owner
  数量区分共享边界；临时 Arc 引用只会导致保守地延后清理，不会提前破坏
  外部 owner；
- `group_exiting` 已阻止重复进程级清理，但真正 SMP 下两个线程相邻
  `exit_group` 的动态竞争仍待专项回归；
- 非法指令和 breakpoint 当前仍沿用项目既有的普通负 exit code，不在本次
  signal 退出修复范围内。

建议提交：

```text
fix(task): unify process-group exit cleanup
```

### A-005：修复 `timer_settime` 和 `prlimit64` 提交顺序

状态：已验证（静态检查、双架构构建与启动 smoke）

问题：

- `timer_settime` 先修改 deadline/interval，再向用户输出 old value；
- old-value copyout 失败时 syscall 返回 `EFAULT`，但新 timer 状态已经生效；
- `prlimit64` 先调用 `set_rlimit`，再向用户输出 old limit；
- old-limit copyout 失败时 syscall 返回 `EFAULT`，但 limit 已经改变；
- 两个接口都违反“失败不能留下半提交状态”的本轮要求。

根因：

- syscall 路径没有区分输入准备、用户输出和共享状态提交；
- `RLIMIT_NOFILE` 的实现上限只在最终 setter 内检查，无法保证 copyout 后
  的 commit 阶段不再出现可提前发现的校验错误；
- `timer_settime` 在持有全局 `POSIX_TIMERS` 锁时同时计算快照和修改对象。

修改：

`prlimit64`：

- 先 copyin 新 limit；
- 完成 `cur <= max`、权限和 `RLIMIT_NOFILE` 实现上限校验；
- 再 copyout old limit；
- old-limit copyout 成功后才调用 `set_rlimit`；
- 增加统一 `validate_rlimit`，setter 同样复用该校验，避免准备与提交规则分叉。

`timer_settime`：

- 先 copyin 并校验 flags、value 和 interval；
- 在 timer 表锁内确认 owner 并复制 `PosixTimer` 快照，随后立即释放锁；
- 在锁外计算 old value 和准备新 deadline；
- old-value copyout 成功后重新获取 timer 表锁；
- 再次确认 timer ID、owner 和 clock identity，最后提交 deadline/interval；
- 不在全局 timer 锁内执行 copyin/copyout。

最终顺序：

```text
copyin 全部输入
→ 校验参数、权限和实现范围
→ 获取旧状态快照
→ prepare 新状态
→ copyout 旧状态
→ 重新确认对象身份
→ commit
```

保持的不变量：

- old-value 指针非法时，共享 timer/rlimit 状态不改变；
- 未知 flags、非法 timespec、非法 limit 或权限不足时不产生状态变化；
- timer 全局锁内不执行用户空间访问；
- timer 被 owner exit/delete 移除时不能向已失效对象提交；
- `getrlimit/setrlimit` 继续复用 `prlimit64` 的统一语义。

验证：

- `rustfmt --check`：PASS；
- `git diff --check`：PASS；
- RV `cargo check`：PASS；
- LA `cargo check`：PASS；
- `make build-rv`：PASS；
- `make build-la`：PASS；
- RV snapshot 启动完成两组测试并正常关机，无 panic；
- LA snapshot 启动完成两组测试并正常关机，无 panic。

待补动态回归：

- `timer_settime(old_value=非法地址)` 后 `timer_gettime` 仍返回旧配置；
- `timer_settime` 的 new/old 指针指向同一结构；
- owner exit/delete 与 `timer_settime` 相邻发生；
- `prlimit64(old_limit=非法地址)` 后 limit 保持不变；
- `prlimit64` 的 new/old 指针别名；
- 非 root 提升 hard limit 返回 `EPERM` 且状态不变；
- `RLIMIT_NOFILE` 超实现上限在 copyout 前返回 `EINVAL`。

剩余风险：

- 当前项目明确不支持真正 SMP；prepare 与 commit 之间若未来允许另一 CPU
  并发修改同一 timer/rlimit，需要增加对象 generation 或专用事务锁；
- timer old-value copyout 成功后若对象恰好被 owner exit/delete 移除，
  本调用返回 `EINVAL` 且不提交新状态，但用户缓冲区已收到旧快照；
- 当前测试镜像缺少 timer/prlimit 专项程序，运行级失败注入尚未完成。

建议提交：

```text
fix(runtime): defer timer and rlimit updates until copyout succeeds
```

### A-006：增加 Scheduler debug invariant

状态：已验证（debug 双架构运行、release 双架构构建）

问题：

- ready queue、bitmap、`task_index` 和 `blocked_tasks` 维护同一调度状态，
  但修改后缺少统一一致性检查；
- 重复入队、错误队列、ready/blocked 重叠或 exited task 残留可能延迟到
  随机 hang 才暴露，难以定位最早破坏点。

修改：

- 新增 `Scheduler::assert_invariants`，仅在 `debug_assertions` 下编译；
- 通过 `debug_assert_invariants` 接入以下基础状态迁移：
  - `add`
  - `fetch`
  - `remove`
  - `remove_thread_group`
  - `block`
  - `wake`
- `requeue_ready_task`、`wakeup_task` 和 stopped-task wake 通过上述基础操作
  自动接受检查。

检查范围：

- RT bitmap 与 RT 非空队列完全一致；
- normal bitmap 与 normal 非空队列完全一致；
- RT queue 0 保持为空；
- `task_index` 数量与 ready queue 中唯一 tid 数量一致；
- `task_index` 每项能在指定 ready queue 中找到；
- ready queue 每个 tid 在 `task_index` 中恰好出现一次；
- ready task 状态必须为 Ready，且实际调度属性对应当前队列；
- blocked map key 必须等于 task tid；
- blocked task 状态必须为 Blocked；
- ready 和 blocked 集合不能有交集；
- exited task 不能留在 ready 或 blocked 队列。

实现约束：

- 完整检查及 `BTreeSet` 依赖使用 `#[cfg(debug_assertions)]`；
- release 只保留可内联为空的调用壳，编译器会移除；
- 未引入新的调度算法、锁或 release 热路径扫描。

验证：

- RV debug `cargo check`：PASS；
- LA debug `cargo check`：PASS；
- `make build-rv MODE=debug`：PASS；
- `make build-la MODE=debug`：PASS；
- LA debug snapshot：完整执行两组 testrunner 循环并正常关机，无 invariant panic；
- RV debug snapshot：55 秒内执行完第一组并运行至第二组 LTP 第 495 项，
  无 invariant panic；因 debug 全表检查开销在超时前未完成全部项目；
- `make build-rv MODE=release`：PASS；
- `make build-la MODE=release`：PASS；
- release RV/LA 内核均不包含 invariant 诊断字符串，确认完整检查未进入
  release 二进制；
- `rustfmt --check`：PASS；
- `git diff --check`：PASS。

剩余风险：

- 当前 debug invariant 是每次状态迁移后的全量扫描，RV debug 明显变慢；
  这符合本轮“只在 debug 启用”的约束，但不适合作为性能测试构建；
- invariant 只能发现状态已经被破坏，不能替代 wake/timeout/signal 的专项
  完成原因测试；
- RV debug 尚未在当前超时配置下完整跑完第二组 testrunner；
- 真正 SMP 不在本轮支持范围，检查函数没有作为并发证明。

建议提交：

```text
debug(task): assert scheduler queue invariants
```

### A-007：收敛 futex wake/timeout/signal 完成状态

状态：已完成代码收敛；专项竞态测试待具备测试镜像后补跑

静态确认的问题：

- 原实现把 waiter 是否仍在 `FUTEX_QUEUES`、定时等待是否超时
  `TIMED_FUTEX_WAITS.timed_out`、任务是否被信号中断分别保存在三处；
- timeout 路径可以先设置 `timed_out = true`，随后 wake 路径删除 waiter 并调用
  `finish_timed_wait`，但忽略其返回值；等待线程恢复后会找不到超时记录并错误地
  按正常 wake 返回；
- wake、timeout、signal 都能独立执行队列清理和 scheduler wake，没有一个共同的
  “谁先完成”判定点；
- 退出清理只依赖 robust-list 等局部路径，没有统一删除普通/定时 futex waiter。

完成状态：

```text
Pending
  ├─ wake    → Woken
  ├─ timeout → TimedOut
  └─ signal  → Interrupted
```

- `FutexWaits::complete` 是唯一完成提交点，只有第一个把 `Pending` 改为终态的
  路径能够唤醒任务；
- 后到的 wake、timeout 或 signal 会因完成状态不再是 `Pending` 而放弃重复唤醒；
- 等待线程恢复后通过 `finish_futex_wait` 取得并删除唯一完成原因，据此返回
  `0`、`ETIMEDOUT` 或 `EINTR`；
- scheduler 层的非 futex 假唤醒仍允许返回成功，符合 futex 调用方必须重新检查
  用户值的约定，同时会清理 waiter。

锁依赖：

| 已持有 | 尝试获取 | 使用位置 | 结论 |
| --- | --- | --- | --- |
| `FUTEX_QUEUES` | `FUTEX_WAITS` | 登记、wake 竞争、timeout、signal、exit | 允许，固定顺序 |
| `FUTEX_WAITS` | `FUTEX_QUEUES` | 无 | 禁止反向嵌套 |
| futex 两把锁 | `SCHEDULER` | 无 | 禁止嵌套，先释放 futex 锁再 wake |
| `FUTEX_QUEUES` | `SCHEDULER` | wait 的 blocked 状态准备 | 保留；用于维持“复查用户值、入队、准备阻塞”不丢 wake |
| `FUTEX_QUEUES` | `MemorySet(read)` | CMP_REQUEUE 最终 4 字节比较 | 允许；锁外解析页面，锁内只做 no-fault PTE 读取，不分配或处理 page fault |

代码改动：

- `os/src/task/futex/wait.rs`
  - 用 `FutexWait { deadline, completion }` 统一普通和定时 waiter 的完成记录；
  - wake、requeue-wake、timeout、signal 全部通过单一 CAS 语义的锁内
    `complete` 竞争完成权；
  - timeout 在固定锁顺序下同时提交超时并删除队列项，释放 futex 锁后才调用
    scheduler wake；
  - signal 新增 `interrupt_futex_wait`，只在抢到完成权时删除 futex 队列项；
  - exit 新增 `remove_futex_waiter`，同时删除队列、deadline 和完成记录；
  - wake 返回值只统计真正取得完成权的 waiter，不把已经 timeout/signal 的任务
    计入 `nr_wake`；
  - timed wait 在登记 waiter 后、进入 blocked 前再次检查 signal，并在 blocked
    状态刚建立后复查，关闭 signal 到达但旧 `interruptible=false` 的丢中断窗口；
  - 已经进入 blocked 后发现 signal 时，先释放 futex queue 锁，再执行 scheduler
    wake，避免完成清理路径嵌套两类全局锁。
- `os/src/task/task.rs`
  - 两条 signal 投递路径在 scheduler wake 前先竞争 futex 完成原因；
  - 单线程退出和线程组 leader 退出均清理 futex waiter。

验证：

- `rustfmt --edition 2024 --check`：PASS；
- `git diff --check`：PASS；
- RV debug `cargo check`：PASS；
- LA debug `cargo check`：PASS；
- `make build-rv MODE=debug`：PASS；
- `make build-la MODE=debug`：PASS；
- LA debug snapshot：完整执行两组 testrunner 并正常关机，无 panic、死锁或
  scheduler invariant 失败；
- RV debug snapshot：55 秒内完成第一组并进入第二组，无 panic、死锁或
  scheduler invariant 失败；
- `make build-rv MODE=release`：PASS；
- `make build-la MODE=release`：PASS；
- RV/LA release snapshot：均完整执行两组 testrunner 并正常关机；
- release RV/LA 内核不包含 scheduler invariant 诊断字符串。

验收边界与剩余风险：

- 当前 `img/sdcard-rv.img` 和 `img/sdcard-la.img` 缺少实际 basic/LTP 可执行文件，
  testrunner 中所选项目主要以 `ENOENT` 结束；本轮启动测试能证明双架构可运行且
  没有明显状态机/锁序回归，不能宣称已完成任务书 6.3 的 futex 动态验收；
- 仍需在带 futex 用户态回归的镜像上覆盖 EAGAIN、单/多 waiter、wake/timeout
  先后、signal 相邻竞争、bitset、requeue、线程退出残留，并连续运行 100 次；
- `FUTEX_CMP_REQUEUE` 的用户值比较与随后取得 queue 锁之间仍有既有竞态窗口，
  不在本次“完成原因单一化”的修改范围内，后续专项审查时处理；
- 本轮运行环境为单核 QEMU；锁内单赢家语义已建立，但仍需 SMP 压力测试作为
  更强的动态证据。

审查复核（2026-07-26）：

- 按总控 6.3、7.1、17 节和 A 任务书 3.1～3.4、5.2、6.1～6.3 逐项复核；
- 修正 timed futex signal-before-block 窗口；
- 修正 leader 退出遗漏 `clear_child_tid`；
- 增加进程组退出单次提交保护；
- 将 POSIX timer owner 改为稳定 `Weak<TaskControlBlock>` 身份；
- 复核后 RV/LA `cargo check`、debug/release build、格式和 diff 检查均通过；
- LA debug 完整运行两轮；RV debug 完成第一轮并运行到第二轮第 533 项；
- RV/LA release 均完整运行两轮并正常关机，无 panic；
- 当前结论仍是“代码和启动 smoke 通过、专项功能验收未完成”，没有把
  `ENOENT` 测试结果误报为功能 PASS。

建议提交：

```text
fix(futex): make waiter completion single-winner
```

### A-008：收紧 clock 支持范围和实际分辨率

状态：已修复并完成 release 双架构专项回归

问题：

- process/thread CPU clock、TAI、alarm 等未实现时钟错误地返回开机墙上时间；
- 所有 clock 一律声称 1 ns，但公开读取路径实际只到微秒/毫秒；
- `clock_settime` 错误接受 `CLOCK_REALTIME_ALARM`；
- nanosleep wait 把 deadline 降为毫秒，真实 LTP 先测出最多约 1 ms early，
  改为向上取整后又测出约 1 ms late。

代码改动：

- `os/src/syscall/time.rs`
  - realtime 改为 hardware monotonic/timeout counter 加 offset；
  - fine clock 使用微秒值，coarse clock 显式量化到毫秒；
  - `clock_getres` 对 fine 返回 1 µs、coarse 返回 1 ms；
  - CPU、TAI、alarm clock 返回明确错误，不再伪造时间；
  - `clock_settime` 只允许 realtime；
  - POSIX timer 只接受 realtime、monotonic、boottime；
  - nanosleep wait 的记录、索引、扫描和 remaining time 全部改为微秒；
  - 保留 `register_task_timeout` 的毫秒外部接口，内部安全转换为微秒。
- `os/src/syscall/special_fd.rs`
  - timerfd 不再接受未实现的 realtime/boottime alarm。

验证：

- RV release 当前承诺的 7 项 clock/nanosleep：musl 7/7、glibc 7/7；
- LA release同组：musl 7/7、glibc 7/7；
- RV 额外运行 `clock_nanosleep04`、`clock_settime01`：2/2 + 2/2；
- `clock_getres01` 确认 CPU/alarm clock 以 unsupported/TCONF 处理；
- `clock_gettime01/02` 的 CPU clock 子项按设计返回 EINVAL；LTP 因要求 CPU
  clock 必须实现而报告失败，记录为已知支持边界，未改回假时间；
- nanosleep 微秒化后，RV futex 三用例连续 20 次冷启动全部通过，
  musl/glibc 合计 120/120；
- RV/LA 默认 release 构建已恢复，原决赛镜像均正常关机、无 panic；
- 本次涉及文件 rustfmt 与 `git diff --check` 通过；
- 未 commit。

剩余：

- CPU clock 如需支持，必须增加上下文切换级真实运行时间记账；
- 尚未动态直读断言 `clock_getres` 返回的 1 µs/1 ms 数值；
- 尚未加入“设置 realtime 不改变 monotonic”的专用差值测试；
- 不实现 suspend/boottime 区分、TAI、wakeup alarm、NTP 调频和高精度 timer。

### A-009：允许最后一个 ready task 正确进入 futex 等待

状态：已修复并完成 release 双架构专项回归

现象与根因：

- 下载并解压 2025 初赛官方完整镜像，保留原有 128 MiB 决赛镜像；
- RV 连续执行 `futex_cmp_requeue02 → futex_wait01 → futex_wait02` 时，修复前
  10 次冷启动中的第 5、8 次出现 RV-musl `futex_wait02` 误报 `EAGAIN`；
- 临时 trace 证明用户值和 futex key 一致，真正原因是子进程短暂 nanosleep
  导致 ready queue 为空，scheduler 拒绝阻塞父进程；
- 架构 `idle()` 不返回，且原 preempt 路径会无条件把 current 重新入队，因此
  不能只删除 ready-empty 判断。

代码改动：

- `os/src/task/scheduler.rs`
  - `prepare_current_task_blocked()` 不再把 ready queue 暂空当成失败；
  - `switch_to_next_task()` 无 ready task 时检查 timer 并等待真正的 wake，
    保留能够回到阻塞 syscall 的上下文；
  - `preempt_current_task()` 遇到非 Running current 时不伪造 Ready 状态，只
    dispatch 已真正 ready 的任务；
  - 删除不再使用的 `Scheduler::is_ready_empty()`。
- `os/src/task/task.rs`
  - 增加 `is_running()` 状态查询。
- `os/src/task/futex/wait.rs`
  - 定位结束后恢复 `FUTEX_TRACE = false`。
- `user/src/bin/testrunner.rs`
  - 增加构建期开关 `TASK_A_LTP_ONLY`；未设置时默认比赛流程不变。

验证：

- 修复后三用例 RV 冷启动压力：100/100 正常关机，musl/glibc 合计
  600/600 个选定执行通过；
- RV release 9 项：musl 9/9，glibc 9/9；
- LA release 9 项：musl 9/9，glibc 9/9；
- RV/LA debug 三项：均为 musl 3/3、glibc 3/3，无 scheduler invariant、
  panic 或死锁；
- RV debug 完整 9 项试跑为 8/9 + 8/9，唯一失败是 `futex_wait05` 在 debug
  开销下约 1 ms 超出严格时间阈值；同项在 RV/LA release 均通过；
- 已重新执行不带 `TASK_A_LTP_ONLY`、`LTP_CASE_FILTER` 的 RV/LA release 构建，
  最终 `kernel-rv`、`kernel-la` 已恢复为默认版本；
- 默认内核在原 128 MiB 决赛镜像均正常关机、无 panic；缺失旧 LTP 文件导致的
  ENOENT 不作为功能通过；
- 本次涉及文件 `rustfmt --edition 2024 --check`：PASS；
- `git diff --check`：PASS。

剩余：

- signal/wake/timeout 三方相邻竞争、退出 waiter 残留和 SMP 压力仍未覆盖；
- `FUTEX_CMP_REQUEUE` 比较到 queue lock 的既有窗口仍待专项处理；
- 不自行 commit，等待负责人审查和提交。

### A-010：wait4 status/rusage 失败注入

状态：已完成双架构 100 次压力验收

测试设施：

- `user/src/syscall.rs` / `user/src/lib.rs`
  - 增加测试所需的完整 `wait4` 和 `getrusage` 用户态封装；
  - 增加与内核 ABI 一致的 `RUsage`。
- `user/src/bin/task_a_wait4_probe.rs`
  - 非法 status 后有效重试；
  - 非法 rusage 后检查父进程 child usage 未变化；
  - 有效重试后检查父进程增量等于返回的 child usage；
  - 成功回收后再次 wait 必须得到 `ECHILD`。
- `user/src/bin/testrunner.rs`
  - 增加构建期开关 `TASK_A_WAIT4_PROBE`，默认关闭。

验证：

- RV 100/100 次独立冷启动通过；
- LA 100/100 次独立冷启动通过；
- 两类事务性核心断言合计 400/400 通过；
- 无 panic、timeout、child 丢失、重复回收或重复统计；
- 已重新构建默认 RV/LA release 内核；
- 未 commit。

### A-011：timer_create/timer_settime/prlimit64 失败注入

状态：已完成双架构 100 次压力验收

测试设施：

- 增加 `ITimerSpec`、`RLimit` 及 timer/prlimit 原始 syscall 用户态封装；
- 新增 `task_a_atomic_probe`；
- testrunner 增加默认关闭的 `TASK_A_ATOMIC_PROBE` 构建期开关。

验证内容：

- `timer_create`：非法 timerid 返回 EFAULT，失败对象没有进入 timer map；
- `timer_settime`：非法 old-value 返回 EFAULT，timer 保持未 armed，有效重试才
  提交新状态；
- `prlimit64`：非法 old-limit 返回 EFAULT，limit 不变，有效重试才提交并能
  恢复原值。

结果：

- RV 100/100 次独立冷启动通过；
- LA 100/100 次独立冷启动通过；
- 三类核心断言合计 600/600 通过；
- 无 panic、timeout、对象误发布或状态提前提交；
- 默认 RV/LA release 内核已恢复；
- 未 commit。

### A-012：POSIX timer owner 退出清理

状态：owner-exit 已完成双架构 100 次验收；PID reuse 动态项不适用当前分配器

实现与测试：

- `remove_posix_timers_for_owner` 在专项构建下输出实际删除数量，默认关闭；
- probe 子进程创建并 armed 3 个 timer 后退出；
- RV 100/100、LA 100/100 均观测 `owner=3 removed=3`；
- 两架构累计 600/600 个 timer 被同步清理；
- 每轮 successor PID 为 4，确认当前 PID 单调递增。

边界：

- `TidHandle::drop` 当前不回收 ID，无法制造真实 PID reuse；
- timer 投递持有 `Weak<TaskControlBlock>` 并在投递前 upgrade/检查 exited，这是
  将来启用 PID 回收后的身份防线；
- 只把 owner-exit 标为动态 PASS，PID reuse 标为当前不适用/待分配策略改变后测；
- 默认双架构 release 已恢复，未 commit。

### A-013：futex wake/signal/timeout single-winner 压力验收

状态：已完成双架构 20 轮、120 场景专项验收

测试设施：

- `user/src/syscall.rs` / `user/src/lib.rs`
  - 增加 futex 和 mmap 原始 syscall 测试封装；
- `user/src/bin/task_a_futex_race_probe.rs`
  - 使用 `MAP_SHARED | MAP_ANONYMOUS` 页保证 fork 后 futex key 真正共享；
  - 分别控制 wake、SIGUSR1、timeout 首先到达；
  - 检查 syscall 结果、实际 wake 数、信号次数和最终 waiter 残留；
- `user/src/bin/testrunner.rs`
  - 增加默认关闭的 `TASK_A_FUTEX_RACE_PROBE` 构建期开关；
  - 专项模式连续执行 20 轮。

探针自审：

- 初版 signal handler 在函数栈帧未退出时直接调用 `sigreturn`，会以错误 SP
  读取 signal frame；改为正常返回内核 trampoline；
- 普通 fork 私有页不能验证跨进程 futex，改为已有内核明确支持的匿名共享映射；
- 两项均属于探针构造错误，修正后才统计正式结果。

验证：

- RV release：20/20 轮、60/60 三方顺序场景通过；
- LA release：20/20 轮、60/60 三方顺序场景通过；
- 双架构合计 40/40 轮、120/120 场景；
- wake-first 每轮只唤醒 1 个 waiter；
- signal-first 返回 `EINTR`，timeout-first 返回 `ETIMEDOUT`，两者后到 wake
  每轮均为 0；
- 每个场景最终额外 wake 均为 0，无残留 waiter；
- 无 panic、timeout 或 winner 被 loser 覆盖；
- 专项结束后恢复默认 RV/LA release 构建；
- 未 commit。

剩余：

- 线程退出时的 waiter 清理仍需专用动态探针；
- SMP 下并发完成压力尚未执行；
- `FUTEX_CMP_REQUEUE` 用户值比较到 queue lock 之间的窗口仍待专项处理。

### A-014：futex waiter 线程退出清理

状态：已完成双架构各 20 轮动态验收

测试设施：

- 新增 `task_a_futex_exit_probe`；
- 探针在 owner 进程内用
  `CLONE_VM | CLONE_SIGHAND | CLONE_THREAD` 创建真实 waiter 线程；
- waiter 登记一个 10 秒 timed futex wait，外层监督进程随后以 `SIGKILL`
  终止整个 owner 线程组；
- 专项开关 `TASK_A_FUTEX_EXIT_TRACE` 输出退出清理的实际删除结果，默认关闭；
- testrunner 的 `TASK_A_FUTEX_EXIT_PROBE` 专项模式连续执行 20 轮。

代码审查与小幅可观测性调整：

- `FutexQueues::remove_tid` 返回实际删除条目数；
- `FutexWaits::cancel` 返回 wait 是否存在、是否带 deadline；
- `remove_futex_waiter` 仍保持 queue → waits 锁序，默认行为不变；
- 只在专项 trace 开关启用且确有删除时输出计数。

验证：

- RV 20/20、LA 20/20，双架构合计 40/40；
- 每轮精确观测 `queue=1 wait=true deadline=true`；
- 每轮 owner wait status 均为 SIGKILL 9；
- owner 退出后额外 `FUTEX_WAKE` 每轮均为 0；
- 无 panic、timeout、残留 waiter 或残留 timeout deadline；
- 专项结束后恢复默认 RV/LA release；
- 未 commit。

剩余：

- SMP 并发完成压力；
- `FUTEX_CMP_REQUEUE` 用户值比较与 waiter requeue 之间的原子窗口。

### A-015：首次提交前综合审查

状态：当前工作区可提交；任务 A 仍有后续边界

审查范围：

- 逐文件复核全部 tracked/untracked 差异；
- 复核退出顺序、single-winner、锁序、prepare/copyout/commit 和 clock 支持边界；
- 复核四个专项探针及其双架构证据；
- 检查默认构建不会启用专项 testrunner/trace 路径。

结果：

- 未发现阻止本次提交的问题；
- `make check-submit MODE=release`：PASS；
- RV/LA 默认 release 产物类型正确；
- 全量涉及文件 `rustfmt --check`：PASS；
- `git diff --check`：PASS；
- 默认内核不含 futex-exit/timer-lifecycle 专项 trace 字符串；
- 4 个探针合计增加约 RV 68 KiB、LA 96 KiB，作为可复验资产保留；
- 修复日志继续由 `.gitignore` 排除；
- 未 commit，最新提交仍为 `00c6822`。

建议首次提交信息：

```text
fix(runtime): 收敛任务退出、futex 竞争与时钟语义
```

本次提交不宣称完成：CMP_REQUEUE 原子窗口、真正 SMP、CPU clock、PID reuse
动态验证仍保留为后续工作。

### A-016：CMP_REQUEUE 原子窗口与 futex timeout 精度

状态：已完成修复和双架构专项回归；真正 SMP 保留

实现：

- CMP_REQUEUE 在锁外预触发用户页；
- 最终用户值读取、比较和 waiter wake/requeue 合并到同一次 queue 临界区；
- 锁序明确为 `FUTEX_QUEUES → MemorySet → FUTEX_WAITS → scheduler`；
- 新增默认关闭的强制窗口注入开关；
- 新增 `task_a_futex_cmp_requeue_probe` 和用户态完整 futex syscall 封装；
- futex timeout deadline、索引和扫描从毫秒改为微秒。

探针校正：

- 初版只 yield 一次，修改进程可能尚未执行；第 6 轮比较值仍相等，正常 requeue
  导致探针失败；
- 改为专项构建中等待 source word 确实变化，再执行 queue lock 内最终比较；
- 校正后才统计正式压力结果。

验证：

- 强制 CMP 窗口：RV 20/20、LA 20/20；
- 每轮均返回 `EAGAIN`，target wake=0、source wake=1，最终两队列 wake=0；
- 首轮正常回归发现 RV `futex_wait05` 因毫秒 deadline early wake，
  musl/glibc 均为 8/9；
- deadline 微秒化后，RV/LA 9 项 futex 均为 musl 9/9、glibc 9/9；
- 微秒化后三方竞态：RV/LA 各 20/20 轮、60/60 场景；
- 无 panic、timeout、错误迁移、重复完成或 queue 残留；
- 未 commit。

剩余：

- 真正 SMP 的页表并发变化与 queue contention；
- CPU clock、PID reuse 等非本项边界。

### A-017：clock 分辨率和 clock-domain 独立性

状态：双架构各 20 轮专项通过

测试设施：

- 增加 clock_gettime/getres/settime 用户态原始 syscall 封装；
- 新增 `task_a_clock_probe`；
- testrunner 增加默认关闭的 `TASK_A_CLOCK_PROBE`，连续运行 20 轮。

验证：

- realtime/monotonic/raw/boottime 的 resolution 每轮精确为 1 µs；
- realtime/monotonic coarse 的 resolution 每轮精确为 1 ms；
- CPU、alarm、TAI clock 每轮明确返回 `EINVAL`；
- 每轮将 realtime 向前调整 3600 秒，monotonic 仅按实际运行耗时增长；
- 对 monotonic 执行 settime 每轮返回 `EINVAL`；
- RV 20/20、LA 20/20，所有分辨率和独立性断言 40/40；
- 无 panic、timeout、假分辨率或跨 clock-domain 跳变；
- 未 commit。

剩余支持边界：

- CPU clock 需要上下文切换级运行时间记账；
- 当前无 suspend、TAI offset 和 wakeup alarm；
- 上述能力按方案明确拒绝，不属于本轮伪装支持范围。

### A-018：任务 A 最终验收矩阵复核

状态：四天正确性必做项完成；当前批次提交后可进入合并审查

复核结论：

- A 任务书最终 14 项均已有 PASS、N/A 或明确 READY 结论；
- RV/LA release 构建、格式、diff、专项 probe 和 LTP 子集通过；
- 当前决赛镜像默认 smoke 均运行到汇总并正常关机，无 panic；
- 决赛镜像缺少旧 LTP，RV 687 项、LA 688 项 ENOENT 只作为启动/退出 smoke，
  不作为功能结论；
- 功能结论来自初赛 4 GiB 完整镜像；
- 当前最新提交为 `f35ff2d`，本轮性能探针批次未 commit。

明确保留：

- 真正 SMP；
- CPU clock/TAI/suspend/wakeup alarm；
- PID 回收启用后的动态复用测试；
- 五类性能中位数正式表格已由 A-019 补齐，不再是保留项。

合并建议：当前 futex/clock 批次完成提交前审查并由负责人 commit 后，可以进入
任务 A 合并评审；不需要继续扩大正确性重构范围。

### A-019：五类调度与同步性能中位数

状态：双架构修改前后 7 轮确定性对比完成

测试设施：

- 新增默认关闭的 `TASK_A_PERF_PROBE` testrunner 入口；
- 新增 `user/probes/task_a_perf.c` 静态探针；
- 使用同一初赛镜像副本，对比 `00c6822` 与 `f35ff2d` 加当前收尾批次；
- RV/LA 均为 release、SMP=1、256 MiB；
- QEMU 使用 `-icount shift=0,align=off,sleep=off`；
- 每项 7 轮并取中位数。

测量纠偏：

- 第一版使用 monotonic，但修改前 monotonic 只有毫秒刻度，不能作为同尺度对比；
- 改用修改前后均为微秒刻度、且测试期间不调整的 realtime 后重跑；
- 修改前 sleep 会提前唤醒，其 0 ns overhead 明确判为无效数据。

结论：

- getpid：RV -0.2%，LA -0.2%；
- 无竞争 futex：RV +0.1%，LA -0.5%；
- yield：RV +4.5%，LA +5.9%；
- pthread create/join：RV +23.4%，LA +32.2%；
- 竞争 futex：RV +22.5%，LA +24.0%；
- 修改后 sleep/wakeup overhead：RV 4440 ns，LA 3410 ns；
- 所有正式运行 7/7 完成、QEMU rc=0，无 panic/hang。

解释：

- 普通 syscall 和 futex fast path 稳定；
- 超过 5% 的开销集中在 deadline 扫描、线程退出清理以及
  wake/signal/timeout single-winner 登记；
- 这些路径对应已经复现并修复的提前唤醒、重复完成和退出残留问题，不回退
  正确性状态机换取旧数字；
- 后续若优化，必须保持现有竞态与资源残留断言。

未 commit。

### A-020：最终冻结审查补漏

状态：代码修复和双架构专项验证完成

发现与修复：

- futex 主地址及 requeue 目标地址缺少 4 字节对齐校验，可能对非法输入假成功；
- 增加统一对齐校验，未对齐地址返回 `EINVAL`；
- CMP_REQUEUE 锁内最终比较原先复用普通 copyin；
- 改为锁外解析用户页、锁内固定 4 字节 no-fault PTE 读取，满足原子比较且不在
  全局队列锁内惰性分配；
- `getppid` 仍有任务书红名单点名的 parent/Weak 双重 unwrap；
- 改为安全升级 parent，无 parent 时返回 0，并增加 fork 子进程 PPID 回归；
- 用相同最小启动器重测最终工作树性能并更新 A-019。

验证：

- CMP/未对齐专项：RV 20/20、LA 20/20；
- 每轮未对齐 FUTEX_WAKE 和 CMP_REQUEUE target 均返回 `EINVAL`；
- 每轮比较失败仍保持 target wake=0、source wake=1；
- getppid + timer/prlimit 原子探针：RV/LA 均 PASS；
- 最终性能探针：RV/LA 均 7/7、QEMU rc=0；
- 默认 `make check-submit MODE=release` 和当前决赛镜像 smoke：RV/LA 均正常
  汇总、关机、QEMU rc=0；
- 无 panic、hang、错误迁移或新增可达 unwrap；
- 未 commit。

冻结结论：

- 任务 A 个人验收范围满足合并条件；
- B/C 交叉审查、真正 SMP 和整仓决赛 baseline 判定仍由对应负责人/集成负责人
  完成，任务 A 不越权代签。
