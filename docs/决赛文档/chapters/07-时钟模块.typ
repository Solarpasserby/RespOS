= 7. 时钟模块
<7-时钟模块>
#quote(block: true)[
本章回答：RespOS 如何从两个架构的硬件计数器获得统一时间？时钟中断如何驱动调度和超时唤醒？用户态的 clock、sleep、interval timer、POSIX timer 和 timerfd 如何共享同一套时间基础？
]

时钟模块是内核中连接硬件、调度器和用户态时间接口的基础设施。它一方面读取架构相关的硬件计数器并设置下一次 timer interrupt，另一方面把"经过了多少时间"转换为不同用途的时间：用户可见的运行时间、超时和 deadline 使用的单调时间，以及 CPU 时间记账使用的时间。nanosleep、futex、interval timer、POSIX timer 和 timerfd 都建立在这些统一的时间接口之上。

RespOS 将"读取时间"和"等待事件"分开处理：读取时间只查询计数器并完成单位转换，等待事件则登记 deadline、阻塞当前任务，在时钟安全点检查到期对象并唤醒任务或投递信号。这种分工使不同用户态接口可以共享计时基础，同时保持各自的可观察语义。

== 7.1 时间模型与核心数据
<71-时间模型与核心数据>
=== 7.1.1 三种时间尺度
<711-三种时间尺度>
内核不会把所有时间需求都直接绑定到同一个频率。架构层分别提供三种逻辑时钟频率：

#figure(
  align(center)[#table(
    columns: 3,
    align: (auto,auto,auto,),
    table.header([时间尺度], [使用场景], [设计目的],),
    table.hline(),
    [hardware clock], [设置硬件中断、timeout 和 deadline], [反映底层计时器的真实运行尺度],
    [user clock], [`clock_gettime`、文件时间和用户可见运行时间], [提供稳定的用户态时间单位和精度],
    [accounting clock], [`times`、`getrusage` 等 CPU 时间记账], [为资源统计保留独立的计时口径],
  )]
  , kind: table
  )

#strong[表 7-1 RespOS 的时间尺度]

在当前架构实现中，`get_time()` 读取硬件计数器，`get_time_ms/us()` 按 user clock 转换，`get_timeout_ms/us()` 按 hardware clock 转换，`get_accounting_ms/us()` 按 accounting clock 转换。这样，超时判断不会因为用户可见时间的展示策略变化而改变，CPU 记账也不必与 wall-clock 语义混在一起。

=== 7.1.2 `TimeSpec` 与 deadline
<712-timespec-与-deadline>
用户态时间接口统一使用 `TimeSpec` 表示秒和纳秒：

```rust
#[repr(C)]
pub struct TimeSpec {
    pub sec: isize,
    pub nsec: isize,
}

impl TimeSpec {
    pub fn is_valid_duration(&self) -> bool {
        self.sec >= 0 && self.nsec >= 0 && self.nsec < 1_000_000_000
    }

    pub fn checked_duration_us(&self) -> Option<usize> {
        if !self.is_valid_duration() {
            return None;
        }
        (self.sec as usize)
            .checked_mul(1_000_000)
            .and_then(|us| us.checked_add((self.nsec as usize).div_ceil(1000)))
    }
}
```

#strong[代码片段 7-1 时间值校验与单位转换]

`TimeSpec` 的校验不仅检查秒和纳秒的范围，还在转换时使用 checked arithmetic，防止用户提供的超大时间值在内核中发生整数溢出。相对等待会被转换为 `now + duration` 的绝对 deadline；绝对等待则直接使用目标时刻。之后各类定时器只需要比较当前时钟和 deadline，而不必重复解释用户态结构体。

== 7.2 时钟源与时钟中断
<72-时钟源与时钟中断>
=== 7.2.1 RISC-V 与 LoongArch 的时钟源
<721-risc-v-与-loongarch-的时钟源>
公共内核只依赖架构层导出的 `get_time`、频率转换和 `set_next_ti_trigger` 接口，具体硬件操作收敛在两个架构目录中。

#strong[RISC-V64。] RISC-V 通过 `time` 计数器读取当前 tick，并通过 SBI `set_timer` 设置下一次 supervisor timer interrupt。`set_next_ti_trigger` 按硬件频率和固定 tick 周期计算下一次触发点，当前系统每秒设置 100 次基础时钟检查。

#strong[LoongArch64。] LoongArch 通过 `rdtime.d` 读取稳定计数器，并使用架构相关的 timer CSR 和 `set_timer` 设置下一次触发。启动阶段会通过 `CPUCFG` 读取并记录平台计时频率，用于诊断；运行时使用 board 配置提供的 hardware、user 和 accounting clock frequency，避免公共代码依赖具体平台寄存器。

LoongArch 的时钟频率初始化必须在清零 BSS 之后执行，因为初始化结果需要写入已经准备好的静态数据区域。随后两个架构都经过统一的 `rust_main_high` 启动路径：初始化 trap、内存和任务系统，开启 timer interrupt，设置第一次触发时间，最后进入调度器。

=== 7.2.2 Timer trap 的职责
<722-timer-trap-的职责>
一次 timer trap 主要完成三件事：重新编程下一次硬件触发、在安全点处理到期对象、请求当前任务被调度。其逻辑可以概括为图 7-1：

```text
硬件计数器到达 deadline
          │
          ▼
进入 supervisor timer trap
          │
          ├─ 设置下一次 timer trigger
          │
          ├─ timer service 安全点检查 timeout registry
          │       ├─ 唤醒 nanosleep / futex 任务
          │       ├─ 更新 interval timer / POSIX timer
          │       └─ 通知 timerfd poll waiter
          │
          └─ 请求抢占，返回调度器选择下一个任务
```

#strong[图 7-1 时钟中断到超时处理的调用链]

在 RISC-V SMP 模式下，全局 timeout 工作由 timer service hart 负责。用户态进入 timer trap 时可以直接处理；内核态 timer trap 只有在该 hart 处于 per-CPU idle context、没有持有任务相关高层锁时才执行全局扫描，其余 CPU 只重新设置本地 tick。这样既保留了多核下每个 CPU 的本地时钟，又避免 timer 中断在任意 syscall 临界区重入 task、signal 或 timer registry。

LoongArch 当前在 timer trap 中直接完成统一的到期检查和抢占。无论采用哪种架构路径，timer trap 都只负责把硬件事件转换为内核时间服务；具体的 sleep、signal 和 fd 语义仍由各自的模块维护。

== 7.3 超时管理与任务唤醒
<73-超时管理与任务唤醒>
=== 7.3.1 deadline registry
<731-deadline-registry>
RespOS 当前没有把所有定时器抽象成一个统一的时间轮或单一堆结构，而是根据使用者的语义分别维护 deadline registry：nanosleep 等等待项按时钟类别和 deadline 组织，POSIX timer 使用全局 timer 表，进程 interval timer 保存在任务控制块中，timerfd 则保留自己的周期和累计到期次数。这种设计让每种对象可以直接维护自身状态，同时由 `check_all_task_timers()` 在统一安全点触发检查。

```rust
pub fn check_all_task_timers() {
    crate::task::check_futex_timeouts();
    check_nanosleep_timeouts();
    check_timerfd_expirations();
    crate::task::check_active_itimers();
    check_posix_timers();
}
```

#strong[代码片段 7-2 统一的到期检查入口]

这个入口体现了时钟模块与其他子系统的边界：时钟模块负责提供检查时机和当前时间，futex、task、signal 和 timerfd 模块负责解释到期事件。检查函数通常先在 registry 内确定已经到期的对象，再在释放 registry 锁后执行唤醒或信号投递，避免在持有底层容器锁时进入调度器或信号系统。

=== 7.3.2 nanosleep 与阻塞唤醒
<732-nanosleep-与阻塞唤醒>
`nanosleep` 的实现首先校验用户提供的 `TimeSpec`，使用 timeout clock 计算相对 deadline，然后将当前任务登记到 nanosleep wait registry 并进入可中断阻塞。任务被唤醒后会重新检查三种情况：deadline 是否已经到达、是否收到信号、是否仍需继续等待。

```text
读取并校验 req
      │
      ▼
计算 deadline = 当前时间 + duration
      │
      ▼
登记 task → (clock_id, deadline)
      │
      ▼
阻塞并切换到其他任务
      │
      ├─ timer service 发现到期 → 标记 timed_out → 唤醒
      ├─ signal 到达 → 清理等待项 → 返回 EINTR 和剩余时间
      └─ 虚假/提前唤醒 → 重新比较 deadline
```

#strong[图 7-2 nanosleep 的登记、唤醒与返回流程]

相对 `nanosleep` 使用 timeout clock，`clock_nanosleep` 还可以使用 `CLOCK_REALTIME`、`CLOCK_MONOTONIC` 和 `CLOCK_BOOTTIME` 的绝对 deadline。被信号打断时，内核根据开始时间和请求时长计算剩余时间，并在用户 copyout 成功后返回，从而保证等待状态不会因为失败的用户地址而提前提交。

=== 7.3.3 interval timer 与 POSIX timer
<733-interval-timer-与-posix-timer>
任务控制块为三类 interval timer 保存 deadline、interval 和对应 signal。timer 到期后，内核使用原子比较交换更新下一次 deadline，再向任务投递信号；一次性 timer 清零 deadline，周期 timer 则根据 interval 重新装载。全局 `ACTIVE_ITIMER_TASKS` 只记录拥有活动 timer 的任务，timer service 只扫描这些任务，避免每次 tick 遍历所有进程。

POSIX timer 使用独立的全局表保存 timer id、owner、clock、signal、deadline 和 interval。`timer_create` 先准备对象并将 id copyout 到用户地址，成功后才把对象发布到全局表；`timer_settime` 先读取并校验新值，在 old value copyout 成功后再提交 deadline。进程组退出时，按照 owner tgid 一次性移除该组创建的 timer，避免 timer 残留后向已经复用的任务标识投递信号。

== 7.4 用户态时间接口
<74-用户态时间接口>
=== 7.4.1 clock\_gettime 与系统时间
<741-clock_gettime-与系统时间>
用户态可以通过 `clock_gettime` 读取不同语义的时钟：

#figure(
  align(center)[#table(
    columns: 3,
    align: (auto,auto,auto,),
    table.header([时钟], [RespOS 中的含义], [典型用途],),
    table.hline(),
    [`CLOCK_REALTIME`], [monotonic time 加可调整的 realtime offset], [日历时间、文件时间、绝对时间],
    [`CLOCK_MONOTONIC`], [持续增长的单调时间], [timeout、相对等待和 deadline],
    [`CLOCK_MONOTONIC_RAW`], [原始单调时间口径], [需要避免 realtime 调整的计时],
    [`CLOCK_BOOTTIME`], [当前运行期间持续增长的时间口径], [睡眠与系统运行时间相关接口],
    [`CLOCK_REALTIME_COARSE` / `MONOTONIC_COARSE`], [对应时间量化到毫秒], [低开销的粗粒度查询],
  )]
  , kind: table
  )

#strong[表 7-2 用户可见时钟接口]

`CLOCK_REALTIME` 的调整通过独立的 offset 完成，不修改 monotonic 基准。因此，设置系统时间不会让已经登记的 monotonic timeout 倒退或突然提前；使用 realtime 的绝对 timer 则按照 realtime 语义计算当前 deadline。粗粒度时钟通过量化结果减少高精度转换开销。

=== 7.4.2 timerfd：将时间转换为文件事件
<742-timerfd将时间转换为文件事件>
`timerfd` 将定时器包装成 `FileOp`，使定时器可以与 `poll`/`epoll` 和普通文件描述符一起等待。每个 timerfd 保存 clock id、interval、deadline、已消费的 expiration count 和 poll waiter：

```rust
#[derive(Clone, Copy, Default)]
struct TimerFdState {
    interval_ms: usize,
    deadline_ms: usize,
    consumed: u64,
}
```

#strong[代码片段 7-3 timerfd 的核心状态]

读取 timerfd 时，内核根据当前时钟计算已经发生的到期次数，并返回一个 `u64` 计数；周期 timer 即使用户态没有及时读取，也不会为每次到期创建独立事件，而是把多次到期合并为累计次数。非阻塞读取在没有 pending expiration 时返回 `EAGAIN`，阻塞读取则通过统一的 poll waiter 等待下一次通知。

timerfd 本身仍以文件描述符参与生命周期管理，关闭最后一个引用时由 `Arc` 回收对象。全局 timerfd registry 只保存弱引用，用于在 timer service 检查到期状态后通知可读等待者，不会因为 registry 而延长已经关闭的 timerfd 生命周期。

== 7.5 与其他模块的协作
<75-与其他模块的协作>
时钟模块通过统一的当前时间和到期检查入口服务多个内核子系统：

#figure(
  align(center)[#table(
    columns: 3,
    align: (auto,auto,auto,),
    table.header([协作模块], [时钟模块提供的能力], [产生的用户态效果],),
    table.hline(),
    [task / scheduler], [timer interrupt、抢占时机、任务 timeout 唤醒], [sleep、阻塞任务按 deadline 恢复执行],
    [futex], [timeout clock 和到期检查], [`FUTEX_WAIT` 的超时返回],
    [signal], [interval/POSIX timer 到期事件], [`SIGALRM` 等信号投递],
    [FS / poll], [timerfd 的可读通知], [timerfd 与 poll/epoll 统一等待],
    [syscall], [clock、sleep、timer 的 ABI 转换], [用户态读取时间和设置定时器],
    [net], [单调时间戳], [TCP/UDP 协议栈的超时与时间推进],
  )]
  , kind: table
  )

#strong[表 7-3 时钟模块与其他子系统的协作关系]

时钟模块的设计成果可以概括为：架构相关的计数器和中断控制被封装在 HAL 层；公共内核获得统一的毫秒/微秒时间接口；不同类型的等待对象通过 deadline registry 接入同一套安全点；sleep、futex、signal、timerfd 和网络协议栈分别保留自己的状态机；用户态则可以通过 clock、阻塞等待、信号和文件事件四种方式使用时间能力。

这种设计把硬件时钟的差异限制在架构目录内，同时把时间服务自然地融入调度器、信号系统和文件描述符模型，既满足 Linux 风格用户态程序的时间接口需求，也为双架构和多核运行保留了清晰的扩展边界。
