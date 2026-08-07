# BuildStorm SMP 启动与题二推进方案

## 目标与边界

目标是在不破坏当前 RV64 单核 CAgent 回归的前提下，使内核真实使用 BuildStorm 所需的多个 CPU，随后跑通 Debian glibc 镜像中的 Rust 工具链与 arceos-helloworld 全量构建。

官方 final-2026 说明要求 BuildStorm 使用 `-smp 8 -m 8G`；脚本将 `nproc` 写入 `BUILDSTORM_COMPILE`。公开 judge 的 RV64 期望核心数为 8，LoongArch64 代码中为 12，和说明文字存在待核对差异。因此 RV64 先以 8 核为硬门槛；LA64 的最终核数必须以平台启动命令为准。

第一阶段只承诺 SMP 正确性，不承诺时间分。不得伪造 `/proc/cpuinfo`、`nproc` 或 `/proc/uptime`，也不得只启动 QEMU 多 vCPU 而让次核停机。

范围：先实施 RV64；LA64 在 RV64 2/8 核稳定、公共抽象形成后接入。第一版允许全局 run queue 和全局锁，目标是可正确运行多线程 cargo/rustc；per-CPU run queue、work stealing、cache 优化属于后续性能阶段。题一 pub 入口继续保持 `SMP=1`。

## 交接状态与下一轮计划（2026-08-05，未提交工作树）

当前实现已经越过 early bring-up：RV64 次 hart 会进入 `task::run_tasks()`，全局 ready queue 有
CPU-owner/context-handoff 协议，`nproc=8`、完整 `/proc/cpuinfo` 和四路短 sleep 已通过。它**不是**
BuildStorm 就绪状态。以 QEMU `-snapshot -smp 8 -m 256M` 运行四路
`busybox timeout 3 busybox sleep 60` 并 `wait` 时，debug/release 均超过 60 秒未收敛；GDB 快照显示
并发 exit 的 `MemorySet` 页回收、全局 heap lock 与 scheduler lock 竞争。该现象应按活性/性能缺陷
调查，不能在没有等待环证据时叫作死锁，也不能归因于 TLB。

### 下一轮的可执行顺序

1. **先测准，再改代码。** 对每一条命令保存 host wall time、guest exit code、完整 serial log、
   QEMU 配置和镜像 hash；固定使用 `-snapshot`。最小矩阵为 `-smp 1/2/4/8`，每项至少 10 次：
   `true; sleep 1; true`、四路 timeout/exit、fork+exec+wait、pipe producer/consumer、futex wait/wake、
   TCP connect/close。超时后只抓 GDB/锁状态，不向 raw 镜像写诊断文件。
2. **专项审计退出路径。** 从 `TaskControlBlock::exit_process_group` 至
   `MemorySet::recycle_data_pages` / `MapArea::unmap_one`、frame allocator 和 `HEAP_ALLOCATOR`，列出
   每个锁、可能的内存分配和嵌套顺序；同时检查 `Scheduler::assert_invariants()` 是否在 scheduler lock
   内分配。用最小复现证明具体热点/等待环后才修复。
3. **补齐调度 ABI。** 当前 affinity syscall 只保存/报告 mask，`fetch_task()` 不筛选，必须实现
   affinity-aware dequeue 和对应 idle/IPI 行为，并回归 set/getaffinity、单核和多核非饥饿情形。
4. **补齐共享 MM。** `tlb_hart_mask` 是保守 residency 记录，不是 active-mask 和 request/ack
   shootdown。完成后才能开始 pthread/共享 `MemorySet` 压力；不要提前运行 BuildStorm cargo。
5. **最后才验收 BuildStorm。** 在固定官方 Debian 镜像与 `-smp 8 -m 8G` 命令下依次跑 toolchain、
   minibuild、untimed build 和 timed build；题一 CAgent 保持 SMP=1，并在 owner/调度改动后重新做
   干净 pub 单项回归。

每次内核修改后的最低门槛仍是：

```bash
make RV_MODE=debug RV_USER_FEATURES= build-rv
cargo fmt --manifest-path os/Cargo.toml -- --check
cargo fmt --manifest-path user/Cargo.toml -- --check
git diff --check
```

## Phase 0/1 执行记录（2026-08-05，未提交工作树）

- 基线提交为 `cf30f64`，工作树只含本阶段 RV64 启动补丁和本方案文件；没有修改
  `testsuit/cagent-test/`、镜像或官方测试逻辑。
- Phase 0 已确认：以 `-smp 2` 启动旧内核时，QEMU/OpenSBI 有两个 hart，但 `/proc/cpuinfo`
  仍只有 CPU 0；`busybox nproc=2` 来自 `DEFAULT_CPU_AFFINITY_MASK` / `ONLINE_CPU_MASK`
  的硬编码，不能用作真实 SMP 成功证据。`SpinNoIrqLock` 才是跨核自旋锁；`NoIrqLock`
  只关闭本地 SIE，不能保护共享状态。
- Phase 1 已实现并实测：RV64 entry 按 SBI 传入的 `a0` hart id 分配 8 份 64 KiB early
  stack；第一个进入 S-mode 的 hart 以原子方式认领 boot 职责，避免错误假设 boot hart 恒为 0；
  boot 完成 MM/net/initproc 后通过 SBI HSM `hart_start` 拉起其余 hart。次 hart 在
  Acquire 观察 ready 后重载共享内核页表、安装本地 trap vector、标记 online 并 WFI。
- 当时这不是可调度的 SMP：次 hart未打开 timer，因为现有 timer trap 会访问单例
  `current_task`；`PROCESSOR`、idle context 和 scheduler 尚未 per-CPU 化。该限制已由下文
  Phase 2 的 per-CPU 基础改造解除，但仍不可用于 BuildStorm 或声称实际并行执行。
- QEMU/OpenSBI 可能选择任意 boot hart：`-smp 8` 的实测 boot hart 为 6（另一次为 1），
  修正前会停在早期等待；修正后 7 个次 hart 均到达 early-idle，boot hart 进入 Rust user shell。
  `-smp 1` 同样进入 shell；`/bin/busybox true` exit 0。多核控制台的上线文字会交错，
  是无串行 console 输出的诊断现象，不是任务调度证据。
- 已执行：`make RV_MODE=debug RV_USER_FEATURES= build-rv`、两个 `cargo fmt --check`、
  `git diff --check`；构建通过（仅既有依赖 target-feature warning）。复现启动命令：

```bash
qemu-system-riscv64 -machine virt -kernel kernel-rv -m 256M -nographic -smp 8 \
  -bios default \
  -drive file=img/sdcard-rv-pub.img,if=none,format=raw,id=x0 \
  -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -no-reboot \
  -device virtio-net-device,netdev=net -netdev user,id=net -rtc base=utc
```

下一步为 Phase 2：先建立 per-CPU processor/idle/trap/timer，再让任一 CPU 处理 timer；在此
之前禁止把 secondary 从 WFI 放入现有 run queue。

### Phase 2 前置架构结论（2026-08-05，待实施）

当前 RV64 不能安全地把 `tp` 直接当作 CPU id：用户态 `tp` 是 glibc TLS，进入
`__trap_from_user` 后仍是用户值；现有 `sscratch` 用于交换 user/kernel stack；而
`__switch` 还会保存/恢复内核 `tp`。因此只把 `PROCESSOR` 替换为数组会导致 syscall 或 timer
trap 依据用户 TLS 选错 CPU。RocketOS 从 `tp` 指针附近取 `hart_id` 的做法依赖其 task 内存布局，
且没有处理此 user-TLS/trap 边界，不能照搬。

推荐的下一步是先重构 RV64 trap ABI：用每 CPU 的固定 `TrapScratch` 保存 `kernel_sp`、用户
`sp`、以及 `PerCpu` 指针；user-trap 汇编先从 scratch 取本 CPU 身份和内核栈，再保存 user
寄存器。context switch 在锁内更新本 CPU scratch 的下一任务 kernel stack，进入 Rust handler
后只从 `PerCpu` 读取 current task。这样才能让 `tp` 完整保留给用户 ABI，并使 idle/trap/timer
真正 per-CPU。该改动涉及 trap.S、TaskContext/__switch 和 processor 的共同协议，属于下一轮
实现前应明确评审的边界，不能以局部数组补丁替代。

### Phase 2 执行记录（2026-08-05，部分完成）

- 已实现并验证 RV64 trap scratch 协议：`sscratch` 固定指向 `PerCpu::TrapScratch`，其中偏移
  0 是当前任务的 kernel stack top；user trap 先保存 user `t0/t1/sp`，再取本 CPU 的 kernel
  stack。内核执行时 `tp` 为 `PerCpu` 指针，返回用户前恢复 TrapContext 的 user TLS `tp`。
  每次 `__switch` 到一个已 claim 的任务前，调度路径会更新该任务保存的 kernel `tp` 与本 CPU
  scratch 的 kernel stack。这个机制不依赖 S-mode 不可访问的 `mhartid` CSR，也不改变用户 TLS ABI。
- 已将 `PROCESSOR` 与 bootstrap/idle context 改为固定 8 项的 per-CPU 状态；当前 task 查询按
  内核 `tp` 取得本 CPU processor。RV64 `-smp 1` / `-smp 2` 的 pub 镜像均进入 shell，
  `/bin/busybox uname -r` 输出 `6.10.0-dev`、`/bin/busybox true` exit 0；`-smp 2` 次 hart
  打印 `online (per-cpu idle)`。LA64 debug build 也在这一抽象改动后通过。
- 已验证每核 timer idle：次 hart设置 `sstatus.SIE`、`sie.STIE` 并使用既有 SBI TIME 路径后，
  `-smp 2` 的首个 tick 从 WFI 返回；清理探针后，`-smp 8` 的全部七个次 hart均打印
  `online (per-cpu timer idle)`。idle hart没有 current task 时只重设其本地 tick，不并发扫描
  全局 task timer。
- 已修复的独立缺陷：之前 GDB 所见低 `stvec` 不是物理/高半区转换错误。链接的
  `__trap_from_kernel` 仅有 2 字节对齐，其地址低两位为 `10`，被 `stvec` 当作保留的 trap mode，
  QEMU WARL 后表现为错误地址。给汇编入口增加 `.align 2` 后，guest 读回对齐的高半区 vector
  `0xffffffc08035de18`。此前“在 `trap::init` 补 `KERNEL_BASE`”的假设已撤销；也不再根据旧探针
  推断 Sstc 或 SBI TIME 不可用。
- 已验证 IPI trap 基础：以 SBI IPI（EID `sPI`）向**已 online 的本 hart**发送 self-IPI，
  `-smp 8` 的七个次 hart 上线日志均为 `ipi=1`。handler 先清 `sip.SSIP`，只递增 per-hart
  atomic counter，不触碰 scheduler lock。一次向 HSM `start-pending` hart 的 boot-time 广播
  得到 `ipi=0`，已撤销；因此后续 scheduler 只能对 `ONLINE_HART_MASK` 中已完成 trap 初始化的
  hart 做 publish-after-kick，不能以 `hart_start()` 成功返回代替 online。
- Phase 3 前置修复已实施：全局 `fetch_task()` 在 scheduler lock 内完成 ready queue 出队和
  `Running` claim，context switch 仍在锁外。这样两个 CPU 不会在 task 尚为 `Ready` 的窗口与
  wakeup/signal 竞争。尚未启动 secondary scheduler：现有无 runnable task 路径会在已阻塞 task
  的内核栈中自旋，且 idle timer 目前不能安全并发扫描全局 timeout。必须先实现每 CPU 的
  idle↔task 返回闭环、指定单一 timer-service CPU 或等价的 timer work 串行化，再开启该路径。
- 上述两个 idle 前置已完成且只在 boot hart 调度下验证：每 CPU `IDLE_TASKS[cpu]` 保存首次
  dispatch 的 bootstrap context；无 runnable task 的 Blocked/Exited task 现在恢复该 context，
  idle loop WFI 后重新 claim ready queue。RISC-V 从 user trap 进入 S-mode 时硬件会清 `SIE`，
  所以 idle 必须在无锁状态重新启用 timer/software interrupt；否则 timeout 不会被 kernel trap
  服务。全局 timeout 目前明确只由 boot hart 的 timer service 扫描，避免次 hart 并发进入旧的
  全局 timeout 结构。`-smp 1` pub 的 BusyBox `sleep 1` 回到 shell 后 `true` 成功；secondary
  仍未进入 `run_tasks()`，不能当作多核 scheduler 验收。
- Phase 4 基础已实施：RV64 `MemorySet` 维护“曾加载此页表的 hart”保守位图。context switch
  在恢复 task 前先置当前 bit；不在 switch-out 时清位，避免页表 writer 与仍执行旧 `satp` 的 CPU
  交错时漏发 shootdown。`flush_tlb()` 先做本地 memory barrier/`sfence.vma`，再经 SBI RFENCE
  对位图中除当前 hart 外的目标做全地址空间 `remote_sfence_vma`（当前没有 ASID allocator）。
  QEMU `-smp 8` 已以次 hart→boot hart 探针验证 RFENCE 返回成功；探针未保留在正式启动路径。
  此策略正确性优先且会 over-flush，待 BuildStorm 正确性稳定后再做 active-mask 收缩/按 ASID 优化。

## 当前代码事实（2026-08-05）

- RV64 的 `rust_main(hart_id, opaque)` 由最先进入的 hart 一次性清 BSS、初始化全局 MM/net/initproc；
  HSM 次 hart 在 ready barrier 后走 `secondary_main()`，不得重复全局初始化。early stack、`PerCpu` 与
  online/idle 位图固定为 8 项，hart ID 大于等于 8 目前不受支持。
- `task/processor.rs` 已改为每 hart 的 `PROCESSORS` 与 `IDLE_TASKS`；`current_task()` 从内核态
  `tp` 的 `PerCpu` 取得本 CPU 状态。RV64 user `tp` 继续从 trap context 恢复，保持 glibc TLS ABI。
- scheduler 仍是全局 ready/blocked 队列，但在 scheduler lock 内 claim Ready task，并通过 owner +
  idle handoff 避免保存 context 前的双运行；ready 发布会对 idle hart 发 IPI。该协议只完成有限短压。
- `MemorySet` 已有保守 TLB residency/RFENCE 基础，但没有 active-mask/request-ack；affinity syscall
  报告真实 online mask，但 dequeue 尚未执行 affinity 筛选。这两项都是后续正确性工作，不是完成态。
- 单核 trap、timer、页表切换、scheduler 已服务 CAgent；owner 栅栏之后尚未重新运行 CAgent，必须在
  下一轮调度/MM 修改前先补干净 pub 单项。

## 往年参考实现比对（2026-08-05）

本节审阅本仓库 `examples/rocketos` 的源码，不将其当作可直接合入的依赖或正确性证明。`examples/delnx0`
当前未提供可审阅的 SMP 源码，因此不从中引申结论。

| 主题 | RocketOS 的做法 | 可借鉴结论 | RespOS 的处理决定 |
| --- | --- | --- | --- |
| RV64 入口 | `entry.S` 接收 a0 hart id，以 `boot_stack_top - (hart_id << 16)` 选早期栈 | 次核必须最早按 hart 选独立栈，不能进入 Rust 后才修复 | 使用显式 `MAX_CPUS * EARLY_STACK_SIZE` 数组、范围检查和 linker 地址断言；容量至少 8，不能沿用示例的 4 核空间 |
| 次核启动 | boot hart 通过 SBI HSM `hart_start(hart, 0x80200000)` 逐核拉起 | OpenSBI HSM 是 RV64 QEMU 可行路径 | 在现有 `arch::sbi` 补标准 HSM 封装，传入次核入口与 opaque；检查/记录 SBI error，不能忽略返回值 |
| 全局初始化分支 | `IS_BOOT.compare_exchange` 让一核做 BSS/MM，其他核走另一条路径 | 必须把 boot 与 secondary 初始化分开 | 用具名 boot state（Cold/GlobalInit/Ready/Failed）和 Acquire/Release barrier；禁止次核仅靠 CAS 失败后立即访问半初始化全局对象 |
| current task | `Vec<RwLock<Processor>>` 按 hart 保存 current | current 不是全局单例 | 建立 `PerCpu`，并把 cpu id 与当前 CPU 身份绑定；不通过 task 的继承 `cpu_id` 推断当前运行 CPU |
| run queue | 每 hart 一个 `SyncUnsafeCell<Scheduler>`，按 task.cpu_id 入队 | per-CPU queue 是性能终局方向 | 第一正确性版保留受锁保护的全局队列和明确 claim；后续才引入 per-CPU queue/work stealing，不能照搬无锁 `SyncUnsafeCell` |
| idle/boot context | 所有 hart 复用同一个 `BOOT_TASK` Arc | 暴露了 idle 上下文必须 per-CPU 的问题 | 每 CPU 独立 idle context/stack，且不进入 task manager；共享 idle TCB 在 RespOS 中明确禁止 |
| TLB/IPI | SBI 常量包含 IPI/remote sfence，但该路径未构成完整 request/ack 协议 | HSM、IPI、remote fence 的架构接口可分层 | 将 IPI pending、shootdown request/ack、active CPU 位图列为 Phase 3/4 的独立门槛，不能仅调用 remote fence 即回收页表 |

这份比对改变了实施细节而不改变总体路线：Phase 1 现在明确要求由汇编在最早期依据 hart id 切栈；Phase 3 在实现 per-CPU run queue 之前先采用加锁的全局队列；boot barrier 与 idle 独占性从“建议”提升为不可跳过的不变量。

## 目标架构与不变量

```text
boot hart：一次性 BSS/MM/trap 模板/task/net/initproc 初始化
  → Release 发布 SMP_READY
  → SBI HSM 启动 secondary hart

每个 hart：专属 early/idle stack 与 PerCpu
  → 安装本核 trap/scratch/timer
  → Acquire 等待全局初始化完成并标记 online
  → 从全局 ready queue claim task；空闲时 WFI，IPI/timer 后重选任务
```

必须遵守：

1. 仅 boot hart 清 BSS、初始化 allocator/MM/net、创建 initproc；次核只进入 secondary path。
2. current task、idle context、内核栈、trap scratch、timer compare 都是 per-CPU；一个 TCB 任意时刻最多在一个 CPU 为 `Running`。
3. 初版 scheduler 可全局加锁，但 task 必须在锁内从 ready 集合 claim。更关键的是，运行中的 current
   task 不能在其 `__switch` 已保存 context 前被重新发布到 ready 集合：必须采用 handoff trampoline
   或等价协议，使“保存 current context → 发布 Ready → claim next”对其他 CPU 不暴露半完成状态。
   仅在 fetch 时设 `Running` 不足以避免双运行。
4. `add_task`、futex/signal/pipe/socket/timer 唤醒必须在发布 ready 状态后可靠 kick 一个 CPU；不能等不确定的 timer tick。
5. 关中断锁只禁止本 CPU 中断，不是跨核互斥。所有原有“单核+关中断”共享状态必须改为真实锁或原子协议。
6. 本地 context switch 刷新本 CPU TLB；并发使用同一 `MemorySet` 前必须有 active-CPU 追踪和 TLB shootdown，之后才能回收 PTE/frame。
7. `/proc/cpuinfo`、`nproc` 和 affinity 只能从真实 online CPU 集合生成。

## 分阶段实施与验收

每个内核阶段后至少执行：

```bash
make RV_MODE=debug RV_USER_FEATURES= build-rv
cargo fmt --manifest-path os/Cargo.toml -- --check
cargo fmt --manifest-path user/Cargo.toml -- --check
git diff --check
```

### Phase 0：基线、审计与资源

- 保存当前 `SMP=1` CAgent 小回归结果。
- 获取官方 BuildStorm Debian/toolchain 镜像、最终启动脚本，记录镜像 hash、QEMU 版本、宿主物理核数、内存、磁盘后端和 Linux baseline；不得用 pub 镜像替代 BuildStorm 成绩。
- 审计 main、两套 entry/trap/timer、processor/scheduler/task、frame allocator、页表修改、`SpinNoIrqLock`、futex/clone/exit 和 virtio IRQ。
- 对照 RocketOS 的 `entry.S`、`hart.rs`、`sbi.rs`、`main.rs`、processor 与 scheduler，维护一张“参考机制 / RespOS 对应所有者 / 并发安全差异 / 是否采用”的审计表；任何引用示例的实现必须补足锁、barrier 和错误路径。
- 建立最小 SMP probe：每核打印 hart/逻辑 CPU，递增独立 atomic 计数；用户态读取 `/proc/cpuinfo`、`nproc`、affinity。
- 添加独立且默认不启用的 SMP debug 启动入口；不得改变 `run-rv-pub` 默认单核。

验收：单核 boot 与 CAgent 单项不变；保存当前 `-smp 2` 的完整失败/仅 boot-hart 日志作为对照。

### Phase 1：RV64 次核 early bring-up（不调度用户任务）

- 重做 RV64 entry：读取 `mhartid`，按 hart 选择有上限检查的独立 early stack；boot hart 进主初始化，次核进 `rust_secondary_main(hartid)`。
- 调研并选择 QEMU/OpenSBI 次核协议：若 firmware 不会直接放行所有 hart，则 boot hart 在初始化完成后用 SBI HSM `hart_start` 启动其余 hart，并记录返回错误码。
- 定义固定上限至少 8 的 `PerCpu`：逻辑 CPU id、hart id、online/boot state、idle stack/context、current task。early path 不动态分配。
- boot hart Release 发布 `SMP_READY`；次核 Acquire 等待，再安装本核 trap vector、scratch、内核页表与 timer，先停在 WFI，不取用户任务。
- 为 boot state 定义失败状态和超时诊断：secondary 观察到 `Failed` 时停机并打印 hart/阶段；不得无限静默自旋掩盖 boot hart panic。

验收：`-smp 2`、`-smp 8` 都打印唯一 hart 上线记录；boot hart 可启动 shell；次核 WFI 无 trap/栈损坏；`SMP=1` 不变。

### Phase 2：per-CPU processor、idle、trap 与 timer

- 将全局 `PROCESSOR`、`IDLE_TASK` 改为按 `cpu_id()` 查询；审计全部 `current_task()`、`current_user_token()` 与 `PROCESSOR.lock()` 调用点。
- 每核建立 idle context，明确其不进 task manager、不参与 wait/reap，且不共享 boot/task stack。
- 将 RV64 `stvec`、`sscratch`、SIE 和 timer compare 全部放入每 hart 初始化；timer trap 仅抢占当前 CPU。
- 为共享 `UnsafeCell`、全局静态、driver 状态补足锁/原子协议；审计 NoIrqLock 的真实边界。

验收：2 核都能收 timer trap；任一 CPU idle 不影响另一 CPU；单核 timeout/wait/CAgent 单项回归；debug 断言无 task 双 Running。

### Phase 3：全局 run queue、多核调度与 IPI

- 初版保留全局 `Scheduler`；在锁内 claim ready task、提交 `Running`，锁外 context switch。先实现
  context-handoff trampoline（或等价的 per-task CPU owner 状态机）：抢占/yield 的 current 必须先保存
  context，才可变为可被其他 CPU claim 的 `Ready`；阻塞、抢占、退出只能由当前 CPU 提交。为每个
  TCB 增加仅调试使用的 owner CPU/迁移断言，任何 task 双 Running 立即停机记录。
- 实现 RV64 software interrupt/IPI 与 pending 位。IPI handler 只确认/清 pending 并触发本地调度，不持有高层锁做复杂切换。
- 把 `add_task`、`wakeup_task`、timer/futex/signal/pipe/socket completion 统一为 `enqueue_and_kick()`：先发布 ready 状态，再 kick idle/目标 CPU。
- 审计 `DEAD_TASKS`、task manager、tid/pid allocator、thread group、parent/children、signal 与 timer 的锁顺序；禁止持 scheduler lock 做 I/O、用户 copy、网络 poll 或睡眠。

验收：2 核运行两个 CPU-bound worker 时两个计数都增长；并发 fork/exec/wait、futex、pipe、信号中断均无任务丢失；8 核能稳定回 shell，短压力至少 30 次。

### Phase 4：MM 并发与 TLB shootdown

- 在 `MemorySet` 加 active-CPU 位图，context switch 切入/切出以明确内存序更新；同一地址空间的多个线程可占多个 CPU。
- 审计 mmap/munmap/mprotect/mremap、COW/lazy fault、fork/exec/exit、file writeback 的 PTE 与锁范围。
- 实现 request/ack shootdown：发布页表变更 → IPI → 目标 CPU 本地 `sfence.vma` → ack → 回收 frame/page table 或返回用户。初版可全地址空间 flush。
- 把现有退出后延迟页表释放扩展到远端 CPU 不再使用旧 token，不能用 Arc/frame 引用计数代替 TLB 安全。

验收：两核 pthread/futex 压力；一核反复映射/保护变化、另一核访问；fork+exec+exit 循环；无 stale mapping、double free 或 kernel fault。完成前不运行共享 MM 的多核工具链压力。

### Phase 5：真实 CPU 拓扑与 BuildStorm 功能门槛

- `/proc/cpuinfo`、`/proc/stat`、sysconf、`sched_getaffinity`、`getcpu`（如需要）从真实 online CPU 集合导出；保存 Debian `nproc` 的实际查询链路。
- 依官方顺序运行：toolchain version、cargo new+minibuild、untimed tg-xtask、timed full build。每段保存 serial log、返回码、磁盘/内存和 `/proc/uptime` 原始值。
- 遇失败按动态 loader/ELF → syscall errno → FS/MM → scheduler/futex → linker 分层定位，不能把 cargo 首个报错直接当作 SMP 错误。

验收：RV64 `-smp 8 -m 8G` 下 `nproc=8`，`BUILDSTORM_TOOLCHAIN ok`、`BUILDSTORM_MINIBUILD ok`，并至少得到 `BUILDSTORM_COMPILE mode=multi ok=true` 与 ≥500 KiB 产物。

### Phase 6：性能与 LA64

- 同机、同镜像、同 QEMU 配置取得 Linux 基线 B；至少三次记录中位数和离散度。严格使用脚本 guest `/proc/uptime` timed 区间。
- 先 profile 再优化：CPU 利用率、run queue lock、context switch、futex、page fault、page cache/FS、动态 loader、allocator、virtio block。
- 优化候选：per-CPU run queue/work stealing、per-CPU allocator cache、缩短 FS/page-cache 锁、合并 block I/O、只读 executable/page cache。每项必须有前后数据与 mmap/FS/TCP/单核回归。
- LA64 复用公共 PerCpu/scheduler/MM 协议，单独实现 firmware 次核启动、IPI、timer、TLB flush；先 2 核再按最终官方核数扩展。

## 风险与停止条件

| 风险 | 处置 |
| --- | --- |
| 多 hart 共用 BSS/boot stack | 先完成 per-hart stack 和 boot barrier；未通过不得调度次核 |
| 同一 task 双运行 | task→cpu owner debug 断言；出现一次即停止 Phase 3。已在 2026-08-05 复现：先入队、后 `__switch` 的 preempt/yield 协议会使另一 CPU 恢复尚未保存的同一 TCB；必须先完成 context-handoff trampoline |
| ready task 无人运行 | 所有 enqueue 走 kick/IPI；不能以轮询或 timer 偶然唤醒代替 |
| stale TLB | active CPU 位图与 request/ack shootdown 完成前，不允许共享 MM 多核执行 |
| 单核假设的数据竞争 | 每个共享可变对象列明 owner、锁和内存序；无法证明就先串行化 |
| 镜像/资源污染 | BuildStorm 镜像独立副本；QEMU 持有 raw image 时不运行 host debugfs/e2fsck |
| 优化掩盖正确性 | 无 profile、无前后数据、无回归的优化不合入 |

## 首轮行动清单

- [ ] 获取并校验 BuildStorm 镜像与官方最终启动命令，核实 RV64/LA64 核数差异。
- [ ] 当前内核跑一次 `SMP=2` 只读启动实验并保存完整 serial log。
- [ ] 列出 `PROCESSOR`、`IDLE_TASK`、`current_task`、`SCHEDULER`、timer/trap、`UnsafeCell`、`SpinNoIrqLock` 的调用点，形成共享状态审计表。
- [ ] 确定 RV64 OpenSBI HSM/IPI API 与现有 `arch::sbi` 的能力缺口。
- [ ] 评审 `PerCpu` 固定布局、最大 CPU 数、hart→逻辑 CPU 映射及 boot memory ordering。
- [ ] 决定 entry/链接脚本的 early stack 数组设计，并先验证其地址、对齐与映射。

完成首轮清单后，才开始 Phase 1 的内核实现。

## 2026-08-05 执行记录：Phase 3 context handoff

- 8 核首次接入 `run_tasks()` 后，`nproc=8` 但 `cat /proc/cpuinfo` 可触发 `sepc=0` 指令页故障。
  GDB 和调度代码确认根因是 preempt/yield 的“先把 current 入 ready queue、后 `__switch`”窗口；
  另一 hart 可恢复尚未保存的同一 TCB。此前的 ready-queue claim 原子化不足以关闭该窗口。
- 已改为 per-CPU handoff：任务先从 `Processor.current` 移入 handoff slot，再切入本核 idle；
  idle loop 在旧 context 保存后才发布 Ready。RV64 SMP=8 已通过 `nproc`、完整 `/proc/cpuinfo` 和
  四个后台 `sleep 1` + `wait`；SMP=1 的 sleep 与 CAgent kernel 单项亦通过。
- 这只完成 Phase 3 的一个必要交接不变量。下一步应为 TCB 加 owner CPU debug 断言，随后分别做
  fork/exec/wait、futex、pipe/socket 的可重复 2/4/8 核压力；共享地址空间压力必须继续受
  Phase 4 的 active-mask/shootdown 完整性约束，不能以本次短压代替。

## 2026-08-05 执行记录：owner 栅栏后的退出压力

- owner CPU 栅栏已加入，并使 Blocked→wakeup 的 context-save 窗口与 yield/preempt 使用同一 idle
  handoff。它通过了 SMP=1 sleep 与 SMP=8 procfs/四路短 sleep，但四路 `timeout 3 sleep 60` 在
  debug/release 的 8 核 QEMU 中超过 60 秒未完成。
- GDB 显示多个 timeout/sleep 子进程同时执行 `exit_process_group` 的 `MemorySet` 回收，在全局
  buddy heap spin lock 上争用；scheduler 也在 enqueue invariant 中分配。应把”全局 allocator 和
  exit page recycle 的并发锁序/分配行为”列为 Phase 3→4 的阻塞审计项。不能仅关闭 debug assertion
  宣称修复，也不能用 per-CPU allocator 作为未经 frame/page-table 所有权审计的快速替换。

## 2026-08-07 执行记录：exec/group-exit quiescence 协议

- 实现了 Phase 3→4 的协作式 sibling 终止协议，解决 exec/exit_group 时远端 CPU 仍在旧
  `MemorySet` 上执行的安全窗口。核心机制：
  - TCB 新增 `terminate_requested: AtomicBool` 字段，`request_termination()` 以 Release 写入。
  - `can_be_claimed_on_cpu()` / `try_claim_running_on_cpu()` 以 Acquire 检查，拒绝已标记终止的 task。
  - `publish_saved_handoff()` 对已终止 task 静默丢弃，不再重新发布到 ready queue。
  - `close_other_threads_for_exec()` 和 `exit_process_group()` 采用统一四步流程：
    标记终止 + 摘除 → spin-wait CPU owner 释放 → ack 确认 → 安全回收。
- 协议不依赖 IPI 主动打断远端 CPU，而是依赖 timer preempt 使远端 CPU 进入调度路径后
  自然释放 owner。最坏等待不超过一个调度 quantum。
- 同时新增五条诊断 trace channel（`quiescetrace`/`proctrace`/`pipelifetrace`/`ldtrace`/
  `illegaltrace`）用于下一轮 BuildStorm 阻塞点定位。
- 本轮未实际运行 BuildStorm；quiescence 协议和 trace 的正确性仅通过构建门禁
  （`make build-rv/build-la`、`cargo fmt --check`、`git diff --check`）。
- 与该协议相关的上下文和诊断策略详见 `current-status.md` 和 `pitfalls.md` 同日更新。
